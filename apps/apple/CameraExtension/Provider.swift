import CoreGraphics
import CoreMediaIO
import CoreText
import Foundation
import IOKit.audio
import VideoToolbox
import os.log

private let log = Logger(subsystem: "dev.godscamera.GodsCamera.CameraExtension", category: "device")

private func textState(_ text: String) -> CMIOExtensionPropertyState<AnyObject> {
    CMIOExtensionPropertyState(value: text as NSString)
}

/// One virtual camera with two streams.
///
/// Client apps such as Zoom read the source stream. The God's Camera app writes
/// frames into the sink stream, and the device forwards each one to the source
/// stream, scaled to the size the client asked for. When the app is not
/// sending, clients see a placeholder slate.
///
/// The system calls into this class on its own queue. Those calls only take
/// `lock` for a moment and never wait for `queue`, so a call from the system
/// can never be stuck behind frame work, and frame work may call the system.
final class DeviceSource: NSObject, CMIOExtensionDeviceSource {
    private(set) var device: CMIOExtensionDevice!
    private var source: StreamSource!
    private var sink: StreamSource!

    /// Frame forwarding, the slate and the client list run here.
    private let queue = DispatchQueue(label: "dev.godscamera.device", qos: .userInteractive)
    private var slateTimer: DispatchSourceTimer?
    private var clientTimer: DispatchSourceTimer?
    private var slateFrame: UInt64 = 0
    private var lastReported = ""

    /// Guards the values below, which both the system's calls and `queue` touch.
    private let lock = NSLock()
    private var sourceStarts = 0
    private var sinkRunning = false
    private var sinkClient: CMIOExtensionClient?
    private var lastSinkFrameNs: UInt64 = 0
    private var stats = VirtualCameraStats()
    private var blockedPids: Set<Int32> = []
    /// How many connections each client process holds.
    private var connections: [Int32: Int] = [:]
    private var lastPushedFrames: UInt64 = 0

    private var formatDescriptions: [CMFormatDescription] = []
    private var pools: [CVPixelBufferPool] = []
    private var slateLabels: [CGImage?] = []
    private var transfer: VTPixelTransferSession?

    private static let statsProperty = CMIOExtensionProperty(rawValue: CameraConfig.Property.stats.key)
    private static let blockedProperty = CMIOExtensionProperty(rawValue: CameraConfig.Property.blockedClients.key)
    private static let handoverProperty = CMIOExtensionProperty(rawValue: CameraConfig.Property.handover.key)

    init(localizedName: String) {
        super.init()
        device = CMIOExtensionDevice(
            localizedName: localizedName, deviceID: CameraConfig.deviceUID, legacyDeviceID: nil, source: self)
        stats.pid = getpid()
        stats.build = Handover.build

        var streamFormats: [CMIOExtensionStreamFormat] = []
        for format in CameraConfig.formats {
            var description: CMFormatDescription?
            CMVideoFormatDescriptionCreate(
                allocator: kCFAllocatorDefault, codecType: CameraConfig.pixelFormat, width: format.width,
                height: format.height, extensions: nil, formatDescriptionOut: &description)
            guard let description else { continue }
            formatDescriptions.append(description)
            // The longest frame is the slowest rate, the shortest frame the fastest.
            streamFormats.append(
                CMIOExtensionStreamFormat(
                    formatDescription: description,
                    maxFrameDuration: CMTime(value: 1, timescale: CameraConfig.minFrameRate),
                    minFrameDuration: CMTime(value: 1, timescale: CameraConfig.maxFrameRate),
                    validFrameDurations: nil))
            let attributes: [CFString: Any] = [
                kCVPixelBufferPixelFormatTypeKey: CameraConfig.pixelFormat,
                kCVPixelBufferWidthKey: format.width,
                kCVPixelBufferHeightKey: format.height,
                kCVPixelBufferIOSurfacePropertiesKey: [:] as CFDictionary,
            ]
            var pool: CVPixelBufferPool?
            CVPixelBufferPoolCreate(kCFAllocatorDefault, nil, attributes as CFDictionary, &pool)
            if let pool { pools.append(pool) }
            slateLabels.append(Self.makeSlateLabel(height: Int(format.height)))
        }
        VTPixelTransferSessionCreate(allocator: kCFAllocatorDefault, pixelTransferSessionOut: &transfer)

        source = StreamSource(
            name: "God's Camera", streamID: CameraConfig.sourceStreamUID, direction: .source,
            formats: streamFormats, owner: self)
        sink = StreamSource(
            name: "God's Camera Input", streamID: CameraConfig.sinkStreamUID, direction: .sink,
            formats: streamFormats, owner: self)
        do {
            try device.addStream(source.stream)
            try device.addStream(sink.stream)
        } catch {
            fatalError("Failed to add streams: \(error.localizedDescription)")
        }

        // Once a second, look at who is reading the camera and announce the counters if they moved.
        let timer = DispatchSource.makeTimerSource(queue: queue)
        timer.schedule(deadline: .now() + 1, repeating: 1, leeway: .milliseconds(100))
        timer.setEventHandler { [weak self] in self?.refreshClients() }
        timer.resume()
        clientTimer = timer
        log.notice("started build \(Handover.build, privacy: .public), pid \(getpid())")
    }

    // MARK: Device properties

    var availableProperties: Set<CMIOExtensionProperty> {
        [.deviceTransportType, .deviceModel, Self.statsProperty, Self.blockedProperty, Self.handoverProperty]
    }

    func deviceProperties(forProperties properties: Set<CMIOExtensionProperty>) throws
        -> CMIOExtensionDeviceProperties
    {
        let result = CMIOExtensionDeviceProperties(dictionary: [:])
        if properties.contains(.deviceTransportType) {
            result.transportType = kIOAudioDeviceTransportTypeVirtual
        }
        if properties.contains(.deviceModel) {
            result.model = "God's Camera Virtual Camera"
        }
        if properties.contains(Self.statsProperty) {
            result.setPropertyState(textState(snapshot(resetPeak: false).encoded), forProperty: Self.statsProperty)
        }
        if properties.contains(Self.blockedProperty) {
            lock.lock()
            let list = blockedPids.sorted().map(String.init).joined(separator: ",")
            lock.unlock()
            result.setPropertyState(textState(list), forProperty: Self.blockedProperty)
        }
        if properties.contains(Self.handoverProperty) {
            result.setPropertyState(textState(Handover.build), forProperty: Self.handoverProperty)
        }
        return result
    }

    func setDeviceProperties(_ deviceProperties: CMIOExtensionDeviceProperties) throws {
        let values = deviceProperties.propertiesDictionary
        if let text = values[Self.blockedProperty]?.value as? String {
            let list = Set(text.split(separator: ",").compactMap { Int32($0) })
            lock.lock()
            blockedPids = list
            lock.unlock()
            log.notice("blocked processes: \(list.sorted().map(String.init).joined(separator: ","), privacy: .public)")
        }
        if let build = values[Self.handoverProperty]?.value as? String, build == Handover.build {
            log.notice("standing down so a newer build can take over")
            Handover.begin()
        }
    }

    /// The counters with the current format, as the app reads them.
    private func snapshot(resetPeak: Bool) -> VirtualCameraStats {
        let index = min(max(source.activeFormatIndex, 0), formatDescriptions.count - 1)
        let size = CMVideoFormatDescriptionGetDimensions(formatDescriptions[index])
        let duration = source.frameDuration.seconds
        lock.lock()
        defer { lock.unlock() }
        stats.width = Int(size.width)
        stats.height = Int(size.height)
        stats.frameRate = duration > 0 ? 1 / duration : Double(CameraConfig.frameRate)
        // The list is refreshed once a second; the start count is exact at once.
        stats.clients = stats.clientList.isEmpty ? sourceStarts : stats.clientList.count
        stats.connected = connections.keys.sorted()
        let copy = stats
        if resetPeak { stats.maxLatencyMs = 0 }
        return copy
    }

    // MARK: Stream lifecycle, called by the streams

    /// The signing identifier of a client, or "unknown" when the system will not say.
    ///
    /// The system's own answer is used when it has one. Otherwise the process is
    /// looked up by its number, which works for apps the sandbox lets this
    /// extension see.
    static func identity(of client: CMIOExtensionClient) -> ClientIdentity.Identity? {
        if let resolved = ClientIdentity.of(pid: client.pid) { return resolved }
        if let given = client.signingID, !given.isEmpty, given != "unknown" {
            return ClientIdentity.Identity(signingID: given, team: "")
        }
        return nil
    }

    /// A client process connected to or disconnected from the camera.
    func clientConnected(_ pid: Int32, _ connected: Bool) {
        lock.lock()
        let count = (connections[pid] ?? 0) + (connected ? 1 : -1)
        connections[pid] = count > 0 ? count : nil
        lock.unlock()
        noteClientsChanged()
    }

    /// Whether `client` may start a stream. Only the God's Camera app may write
    /// pictures, and processes the app has blocked may not read them.
    func authorize(_ client: CMIOExtensionClient, direction: CMIOExtensionStream.Direction) -> Bool {
        let identity = Self.identity(of: client)
        let signingID = identity?.signingID ?? "unknown"
        if direction == .sink {
            // A writer that is known to be someone else is refused. A writer the
            // system will not identify is let in, or the camera could not work at all.
            if let identity {
                let sameTeam = identity.team.isEmpty || ClientIdentity.own == nil || identity.team == ClientIdentity.own?.team
                guard identity.signingID == CameraConfig.appBundleID, sameTeam else {
                    log.error("refused pictures from \(signingID, privacy: .public), pid \(client.pid)")
                    return false
                }
            }
            lock.lock()
            sinkClient = client
            stats.writerVerified = identity != nil
            lock.unlock()
            log.notice("writer \(signingID, privacy: .public), pid \(client.pid), verified \(identity != nil)")
            return true
        }
        lock.lock()
        let refused = blockedPids.contains(client.pid)
        lock.unlock()
        log.notice("reader \(signingID, privacy: .public), pid \(client.pid), refused \(refused)")
        return !refused
    }

    func start(_ direction: CMIOExtensionStream.Direction) {
        lock.lock()
        if direction == .sink { sinkRunning = true } else { sourceStarts += 1 }
        let firstReader = direction == .source && sourceStarts == 1
        lock.unlock()
        queue.async {
            if direction == .sink { self.consumeNext() }
            if firstReader { self.startSlateTimer() }
        }
        noteClientsChanged()
    }

    func stop(_ direction: CMIOExtensionStream.Direction) {
        lock.lock()
        if direction == .sink { sinkRunning = false } else { sourceStarts = max(0, sourceStarts - 1) }
        let noReaders = direction == .source && sourceStarts == 0
        lock.unlock()
        if noReaders {
            queue.async {
                self.slateTimer?.cancel()
                self.slateTimer = nil
            }
        }
        noteClientsChanged()
    }

    /// A client changed the picture size or frame rate.
    func formatChanged() {
        queue.async { self.report() }
    }

    /// The system updates its list of readers around the time it calls start and
    /// stop, so look shortly after rather than at once.
    private func noteClientsChanged() {
        queue.asyncAfter(deadline: .now() + .milliseconds(60)) { self.refreshClients() }
    }

    private func refreshClients() {
        let list = source.stream.streamingClients.map {
            CameraClient(signingID: Self.identity(of: $0)?.signingID ?? "unknown", pid: $0.pid)
        }
        lock.lock()
        stats.clientList = list
        lock.unlock()
        report()
    }

    /// Announce the counters when anything moved since the last announcement.
    ///
    /// The system keeps a copy of each property on the reading side and asks
    /// again only after an announcement, so without this the app would read
    /// the same numbers for ever.
    private func report() {
        // The peak latency covers the time since the last announcement.
        let now = snapshot(resetPeak: true)
        let summary =
            "\(now.clients)|\(now.clientList.map(\.id).joined(separator: ","))|\(now.width)x\(now.height)@\(now.frameRate)"
            + "|\(now.connected.count)"
        let frames = now.framesIn &+ now.framesOut
        guard summary != lastReported || frames != lastPushedFrames else { return }
        if summary != lastReported { log.notice("readers and format: \(summary, privacy: .public)") }
        lastReported = summary
        lastPushedFrames = frames
        device.notifyPropertiesChanged([Self.statsProperty: textState(now.encoded)])
    }

    // MARK: Sink to source forwarding

    /// Take the next frame the app wrote into the sink and pass it to the clients.
    private func consumeNext() {
        lock.lock()
        let running = sinkRunning
        let client = sinkClient
        lock.unlock()
        guard running, let client else { return }
        sink.stream.consumeSampleBuffer(from: client) { [weak self] buffer, sequence, _, _, _ in
            guard let self else { return }
            self.queue.async {
                guard let buffer else {
                    // Nothing queued yet; look again shortly instead of spinning.
                    self.queue.asyncAfter(deadline: .now() + .milliseconds(3)) { self.consumeNext() }
                    return
                }
                let now = hostTimeNanoseconds()
                self.lock.lock()
                self.lastSinkFrameNs = now
                self.stats.framesIn += 1
                let readers = self.sourceStarts
                self.lock.unlock()
                if readers > 0, let out = self.fitted(buffer) {
                    self.source.stream.send(out, discontinuity: [], hostTimeInNanoseconds: now)
                    self.noteDelivered(buffer, now: now)
                }
                self.sink.stream.notifyScheduledOutputChanged(
                    CMIOExtensionScheduledOutput(sequenceNumber: sequence, hostTimeInNanoseconds: now))
                self.consumeNext()
            }
        }
    }

    /// The app stamps each frame with its capture time; the difference to now
    /// is how long the frame took to get here.
    private func noteDelivered(_ buffer: CMSampleBuffer, now: UInt64) {
        let captured = CMSampleBufferGetPresentationTimeStamp(buffer).seconds * 1_000_000_000
        lock.lock()
        defer { lock.unlock() }
        stats.framesOut += 1
        guard captured.isFinite, captured > 0, Double(now) >= captured else { return }
        let ms = (Double(now) - captured) / 1_000_000
        guard ms < 5_000 else { return }
        stats.latencyMs = stats.latencyMs == 0 ? ms : stats.latencyMs * 0.9 + ms * 0.1
        stats.maxLatencyMs = max(stats.maxLatencyMs, ms)
    }

    /// The frame as the client's chosen format wants it: unchanged if the size
    /// already matches, scaled otherwise.
    private func fitted(_ sample: CMSampleBuffer) -> CMSampleBuffer? {
        let index = min(max(source.activeFormatIndex, 0), formatDescriptions.count - 1)
        let wanted = CMVideoFormatDescriptionGetDimensions(formatDescriptions[index])
        guard let image = CMSampleBufferGetImageBuffer(sample) else { return nil }
        if CVPixelBufferGetWidth(image) == Int(wanted.width), CVPixelBufferGetHeight(image) == Int(wanted.height) {
            return sample
        }
        guard let transfer, let scaled = makeBuffer(formatIndex: index),
            VTPixelTransferSessionTransferImage(transfer, from: image, to: scaled) == noErr
        else { return nil }
        var timing = CMSampleTimingInfo()
        CMSampleBufferGetSampleTimingInfo(sample, at: 0, timingInfoOut: &timing)
        return makeSample(scaled, formatIndex: index, timing: timing)
    }

    private func makeBuffer(formatIndex: Int) -> CVPixelBuffer? {
        guard pools.indices.contains(formatIndex) else { return nil }
        var buffer: CVPixelBuffer?
        CVPixelBufferPoolCreatePixelBuffer(kCFAllocatorDefault, pools[formatIndex], &buffer)
        return buffer
    }

    private func makeSample(_ buffer: CVPixelBuffer, formatIndex: Int, timing: CMSampleTimingInfo) -> CMSampleBuffer? {
        var timing = timing
        var sample: CMSampleBuffer?
        let status = CMSampleBufferCreateForImageBuffer(
            allocator: kCFAllocatorDefault, imageBuffer: buffer, dataReady: true, makeDataReadyCallback: nil,
            refcon: nil, formatDescription: formatDescriptions[formatIndex], sampleTiming: &timing,
            sampleBufferOut: &sample)
        if status != noErr { log.error("sample buffer creation failed: \(status)") }
        return sample
    }

    // MARK: Placeholder slate

    private func startSlateTimer() {
        slateTimer?.cancel()
        let timer = DispatchSource.makeTimerSource(queue: queue)
        timer.schedule(deadline: .now(), repeating: 1.0 / Double(CameraConfig.frameRate), leeway: .milliseconds(2))
        timer.setEventHandler { [weak self] in self?.sendSlateIfIdle() }
        timer.resume()
        slateTimer = timer
    }

    /// Send a slate frame when the app has not written a frame for half a second.
    private func sendSlateIfIdle() {
        let now = hostTimeNanoseconds()
        lock.lock()
        let idle = now &- lastSinkFrameNs > 500_000_000
        lock.unlock()
        guard idle else { return }
        let index = min(max(source.activeFormatIndex, 0), formatDescriptions.count - 1)
        guard let buffer = makeBuffer(formatIndex: index) else { return }
        drawSlate(into: buffer, label: slateLabels[index])
        let timing = CMSampleTimingInfo(
            duration: CameraConfig.frameDuration,
            presentationTimeStamp: CMClockGetTime(CMClockGetHostTimeClock()), decodeTimeStamp: .invalid)
        guard let sample = makeSample(buffer, formatIndex: index, timing: timing) else { return }
        source.stream.send(sample, discontinuity: [], hostTimeInNanoseconds: now)
        lock.lock()
        stats.framesOut += 1
        stats.slateFrames += 1
        lock.unlock()
    }

    /// Dark grey with the camera's name and a slow light band, so it is
    /// obvious the feed is alive but the app is not sending.
    private func drawSlate(into buffer: CVPixelBuffer, label: CGImage?) {
        CVPixelBufferLockBaseAddress(buffer, [])
        defer { CVPixelBufferUnlockBaseAddress(buffer, []) }
        guard let base = CVPixelBufferGetBaseAddress(buffer) else { return }
        let width = CVPixelBufferGetWidth(buffer)
        let height = CVPixelBufferGetHeight(buffer)
        let rowBytes = CVPixelBufferGetBytesPerRow(buffer)
        memset(base, 0x20, rowBytes * height)
        let band = Int(slateFrame % UInt64(height))
        memset(base + band * rowBytes, 0x50, rowBytes * min(6, height - band))
        slateFrame &+= 2
        guard let label,
            let context = CGContext(
                data: base, width: width, height: height, bitsPerComponent: 8, bytesPerRow: rowBytes,
                space: CGColorSpace(name: CGColorSpace.sRGB)!,
                bitmapInfo: CGImageAlphaInfo.premultipliedFirst.rawValue | CGBitmapInfo.byteOrder32Little.rawValue)
        else { return }
        context.draw(
            label,
            in: CGRect(
                x: (width - label.width) / 2, y: (height - label.height) / 2, width: label.width,
                height: label.height))
    }

    /// The slate's text, drawn once per output size. People in a call see it,
    /// so it is in both of the app's languages.
    private static func makeSlateLabel(height: Int) -> CGImage? {
        let lines: [(String, CGFloat, CGFloat)] = [
            (CameraConfig.deviceName, CGFloat(height) / 12, 1.0),
            ("No picture from the app yet", CGFloat(height) / 28, 0.6),
            ("Chưa có hình ảnh từ ứng dụng", CGFloat(height) / 28, 0.6),
        ]
        let typeset = lines.map { text, size, alpha -> (CTLine, CGRect) in
            let attributes: [NSAttributedString.Key: Any] = [
                NSAttributedString.Key(kCTFontAttributeName as String): CTFontCreateWithName(
                    "HelveticaNeue-Medium" as CFString, size, nil),
                NSAttributedString.Key(kCTForegroundColorAttributeName as String): CGColor(gray: 1, alpha: alpha),
            ]
            let line = CTLineCreateWithAttributedString(NSAttributedString(string: text, attributes: attributes))
            return (line, CTLineGetBoundsWithOptions(line, .useOpticalBounds))
        }
        let gap = CGFloat(height) / 40
        let width = Int(ceil(typeset.map { $0.1.width }.max() ?? 0)) + 8
        let total = Int(ceil(typeset.reduce(0) { $0 + $1.1.height } + gap * CGFloat(typeset.count - 1))) + 8
        guard width > 8,
            let context = CGContext(
                data: nil, width: width, height: total, bitsPerComponent: 8, bytesPerRow: width * 4,
                space: CGColorSpace(name: CGColorSpace.sRGB)!,
                bitmapInfo: CGImageAlphaInfo.premultipliedFirst.rawValue | CGBitmapInfo.byteOrder32Little.rawValue)
        else { return nil }
        // Core Graphics counts upwards, so the last line is drawn first, at the bottom.
        var y: CGFloat = 4
        for (line, bounds) in typeset.reversed() {
            context.textPosition = CGPoint(x: (CGFloat(width) - bounds.width) / 2 - bounds.minX, y: y - bounds.minY)
            CTLineDraw(line, context)
            y += bounds.height + gap
        }
        return context.makeImage()
    }
}

/// A source or sink stream of the virtual camera.
final class StreamSource: NSObject, CMIOExtensionStreamSource {
    private(set) var stream: CMIOExtensionStream!
    let formats: [CMIOExtensionStreamFormat]
    private let direction: CMIOExtensionStream.Direction
    private unowned let owner: DeviceSource
    private let lock = NSLock()
    private var formatIndex = 0
    private var duration = CameraConfig.frameDuration

    init(
        name: String, streamID: UUID, direction: CMIOExtensionStream.Direction,
        formats: [CMIOExtensionStreamFormat], owner: DeviceSource
    ) {
        self.formats = formats
        self.direction = direction
        self.owner = owner
        super.init()
        stream = CMIOExtensionStream(
            localizedName: name, streamID: streamID, direction: direction, clockType: .hostTime, source: self)
    }

    /// The format the client chose. Clients change it through the stream's properties.
    var activeFormatIndex: Int {
        get {
            lock.lock()
            defer { lock.unlock() }
            return formatIndex
        }
        set {
            lock.lock()
            if formats.indices.contains(newValue) { formatIndex = newValue }
            lock.unlock()
        }
    }

    /// The time between frames the client chose, kept within what the camera offers.
    var frameDuration: CMTime {
        get {
            lock.lock()
            defer { lock.unlock() }
            return duration
        }
        set {
            let seconds = newValue.seconds
            guard seconds.isFinite, seconds > 0 else { return }
            let shortest = 1 / Double(CameraConfig.maxFrameRate)
            let longest = 1 / Double(CameraConfig.minFrameRate)
            lock.lock()
            duration = seconds < shortest || seconds > longest ? CameraConfig.frameDuration : newValue
            lock.unlock()
        }
    }

    var availableProperties: Set<CMIOExtensionProperty> {
        var properties: Set<CMIOExtensionProperty> = [.streamActiveFormatIndex, .streamFrameDuration]
        if direction == .sink {
            properties.formUnion([
                .streamSinkBufferQueueSize, .streamSinkBuffersRequiredForStartup,
                .streamSinkBufferUnderrunCount, .streamSinkEndOfData,
            ])
        }
        return properties
    }

    func streamProperties(forProperties properties: Set<CMIOExtensionProperty>) throws
        -> CMIOExtensionStreamProperties
    {
        let result = CMIOExtensionStreamProperties(dictionary: [:])
        if properties.contains(.streamActiveFormatIndex) { result.activeFormatIndex = activeFormatIndex }
        if properties.contains(.streamFrameDuration) { result.frameDuration = frameDuration }
        if direction == .sink {
            if properties.contains(.streamSinkBufferQueueSize) { result.sinkBufferQueueSize = 1 }
            if properties.contains(.streamSinkBuffersRequiredForStartup) {
                result.sinkBuffersRequiredForStartup = 1
            }
            if properties.contains(.streamSinkBufferUnderrunCount) { result.sinkBufferUnderrunCount = 0 }
            if properties.contains(.streamSinkEndOfData) { result.sinkEndOfData = 0 }
        }
        return result
    }

    func setStreamProperties(_ streamProperties: CMIOExtensionStreamProperties) throws {
        if let index = streamProperties.activeFormatIndex { activeFormatIndex = index }
        if let duration = streamProperties.frameDuration { frameDuration = duration }
        if direction == .source { owner.formatChanged() }
    }

    func authorizedToStartStream(for client: CMIOExtensionClient) -> Bool {
        owner.authorize(client, direction: direction)
    }

    func startStream() throws { owner.start(direction) }
    func stopStream() throws { owner.stop(direction) }
}

final class ProviderSource: NSObject, CMIOExtensionProviderSource {
    private(set) var provider: CMIOExtensionProvider!
    private var deviceSource: DeviceSource!

    init(clientQueue: DispatchQueue?) {
        super.init()
        provider = CMIOExtensionProvider(source: self, clientQueue: clientQueue)
        deviceSource = DeviceSource(localizedName: CameraConfig.deviceName)
        do {
            try provider.addDevice(deviceSource.device)
        } catch {
            fatalError("Failed to add device: \(error.localizedDescription)")
        }
    }

    func connect(to client: CMIOExtensionClient) throws {
        deviceSource?.clientConnected(client.pid, true)
    }

    func disconnect(from client: CMIOExtensionClient) {
        deviceSource?.clientConnected(client.pid, false)
    }

    var availableProperties: Set<CMIOExtensionProperty> { [.providerManufacturer] }

    func providerProperties(forProperties properties: Set<CMIOExtensionProperty>) throws
        -> CMIOExtensionProviderProperties
    {
        let result = CMIOExtensionProviderProperties(dictionary: [:])
        if properties.contains(.providerManufacturer) { result.manufacturer = "God's Camera" }
        return result
    }

    func setProviderProperties(_ providerProperties: CMIOExtensionProviderProperties) throws {}
}
