import CoreMediaIO
import Foundation
import IOKit.audio
import os.log

private let log = Logger(subsystem: "dev.godscamera.GodsCamera.CameraExtension", category: "device")

/// One virtual camera with two streams.
///
/// Client apps such as Zoom read the source stream. The God's Camera app writes
/// frames into the sink stream, and the device forwards each one to the source
/// stream. When the app is not sending, clients see a placeholder slate.
final class DeviceSource: NSObject, CMIOExtensionDeviceSource {
    private(set) var device: CMIOExtensionDevice!
    private var source: StreamSource!
    private var sink: StreamSource!

    private let queue = DispatchQueue(label: "dev.godscamera.device")
    private var sourceClients = 0
    private var sinkRunning = false
    private var sinkClient: CMIOExtensionClient?
    private var lastSinkFrameNs: UInt64 = 0
    private var slateTimer: DispatchSourceTimer?
    private var slatePool: CVPixelBufferPool?
    private var formatDescription: CMFormatDescription!
    private var slateFrame: UInt64 = 0

    init(localizedName: String) {
        super.init()
        device = CMIOExtensionDevice(
            localizedName: localizedName, deviceID: CameraConfig.deviceUID, legacyDeviceID: nil, source: self)

        CMVideoFormatDescriptionCreate(
            allocator: kCFAllocatorDefault, codecType: CameraConfig.pixelFormat,
            width: CameraConfig.width, height: CameraConfig.height, extensions: nil,
            formatDescriptionOut: &formatDescription)
        let attributes: [CFString: Any] = [
            kCVPixelBufferPixelFormatTypeKey: CameraConfig.pixelFormat,
            kCVPixelBufferWidthKey: CameraConfig.width,
            kCVPixelBufferHeightKey: CameraConfig.height,
            kCVPixelBufferIOSurfacePropertiesKey: [:] as CFDictionary,
        ]
        CVPixelBufferPoolCreate(kCFAllocatorDefault, nil, attributes as CFDictionary, &slatePool)

        let format = CMIOExtensionStreamFormat(
            formatDescription: formatDescription, maxFrameDuration: CameraConfig.frameDuration,
            minFrameDuration: CameraConfig.frameDuration, validFrameDurations: nil)
        source = StreamSource(
            name: "God's Camera", streamID: CameraConfig.sourceStreamUID, direction: .source,
            format: format, owner: self)
        sink = StreamSource(
            name: "God's Camera Input", streamID: CameraConfig.sinkStreamUID, direction: .sink,
            format: format, owner: self)
        do {
            try device.addStream(source.stream)
            try device.addStream(sink.stream)
        } catch {
            fatalError("Failed to add streams: \(error.localizedDescription)")
        }
    }

    // MARK: Device properties

    var availableProperties: Set<CMIOExtensionProperty> { [.deviceTransportType, .deviceModel] }

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
        return result
    }

    func setDeviceProperties(_ deviceProperties: CMIOExtensionDeviceProperties) throws {}

    // MARK: Stream lifecycle, called by the streams

    func authorize(_ client: CMIOExtensionClient, direction: CMIOExtensionStream.Direction) {
        guard direction == .sink else { return }
        queue.sync { sinkClient = client }
    }

    func start(_ direction: CMIOExtensionStream.Direction) {
        queue.sync {
            if direction == .sink {
                sinkRunning = true
                consumeNext()
            } else {
                sourceClients += 1
                if sourceClients == 1 { startSlateTimer() }
            }
        }
    }

    func stop(_ direction: CMIOExtensionStream.Direction) {
        queue.sync {
            if direction == .sink {
                sinkRunning = false
            } else {
                sourceClients = max(0, sourceClients - 1)
                if sourceClients == 0 {
                    slateTimer?.cancel()
                    slateTimer = nil
                }
            }
        }
    }

    // MARK: Sink to source forwarding

    /// Take the next frame the app wrote into the sink and pass it to the clients.
    private func consumeNext() {
        guard sinkRunning, let client = sinkClient else { return }
        sink.stream.consumeSampleBuffer(from: client) { [weak self] buffer, sequence, _, _, _ in
            guard let self else { return }
            self.queue.async {
                guard let buffer else {
                    // Nothing queued yet; look again shortly instead of spinning.
                    self.queue.asyncAfter(deadline: .now() + .milliseconds(4)) { self.consumeNext() }
                    return
                }
                let now = hostTimeNanoseconds()
                self.lastSinkFrameNs = now
                if self.sourceClients > 0 {
                    self.source.stream.send(buffer, discontinuity: [], hostTimeInNanoseconds: now)
                }
                self.sink.stream.notifyScheduledOutputChanged(
                    CMIOExtensionScheduledOutput(sequenceNumber: sequence, hostTimeInNanoseconds: now))
                self.consumeNext()
            }
        }
    }

    // MARK: Placeholder slate

    private func startSlateTimer() {
        let timer = DispatchSource.makeTimerSource(queue: queue)
        timer.schedule(deadline: .now(), repeating: 1.0 / Double(CameraConfig.frameRate), leeway: .milliseconds(2))
        timer.setEventHandler { [weak self] in self?.sendSlateIfIdle() }
        timer.resume()
        slateTimer = timer
    }

    /// Send a slate frame when the app has not written a frame for half a second.
    private func sendSlateIfIdle() {
        let now = hostTimeNanoseconds()
        guard now &- lastSinkFrameNs > 500_000_000, let pool = slatePool else { return }
        var pixelBuffer: CVPixelBuffer?
        guard CVPixelBufferPoolCreatePixelBuffer(kCFAllocatorDefault, pool, &pixelBuffer) == kCVReturnSuccess,
            let pixelBuffer
        else { return }
        drawSlate(into: pixelBuffer)

        var timing = CMSampleTimingInfo(
            duration: CameraConfig.frameDuration,
            presentationTimeStamp: CMClockGetTime(CMClockGetHostTimeClock()),
            decodeTimeStamp: .invalid)
        var sampleBuffer: CMSampleBuffer?
        let status = CMSampleBufferCreateForImageBuffer(
            allocator: kCFAllocatorDefault, imageBuffer: pixelBuffer, dataReady: true,
            makeDataReadyCallback: nil, refcon: nil, formatDescription: formatDescription,
            sampleTiming: &timing, sampleBufferOut: &sampleBuffer)
        guard status == noErr, let sampleBuffer else {
            log.error("slate sample buffer failed: \(status)")
            return
        }
        source.stream.send(sampleBuffer, discontinuity: [], hostTimeInNanoseconds: now)
    }

    /// Dark grey with a slow light band, so it is obvious the feed is alive but idle.
    private func drawSlate(into buffer: CVPixelBuffer) {
        CVPixelBufferLockBaseAddress(buffer, [])
        defer { CVPixelBufferUnlockBaseAddress(buffer, []) }
        guard let base = CVPixelBufferGetBaseAddress(buffer) else { return }
        let height = CVPixelBufferGetHeight(buffer)
        let rowBytes = CVPixelBufferGetBytesPerRow(buffer)
        memset(base, 0x20, rowBytes * height)
        let band = Int(slateFrame % UInt64(height))
        memset(base + band * rowBytes, 0x50, rowBytes * min(6, height - band))
        slateFrame &+= 2
    }
}

/// A source or sink stream of the virtual camera.
final class StreamSource: NSObject, CMIOExtensionStreamSource {
    private(set) var stream: CMIOExtensionStream!
    private let format: CMIOExtensionStreamFormat
    private let direction: CMIOExtensionStream.Direction
    private unowned let owner: DeviceSource

    init(
        name: String, streamID: UUID, direction: CMIOExtensionStream.Direction,
        format: CMIOExtensionStreamFormat, owner: DeviceSource
    ) {
        self.format = format
        self.direction = direction
        self.owner = owner
        super.init()
        stream = CMIOExtensionStream(
            localizedName: name, streamID: streamID, direction: direction, clockType: .hostTime, source: self)
    }

    var formats: [CMIOExtensionStreamFormat] { [format] }
    var activeFormatIndex: Int = 0

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
        if properties.contains(.streamActiveFormatIndex) { result.activeFormatIndex = 0 }
        if properties.contains(.streamFrameDuration) { result.frameDuration = CameraConfig.frameDuration }
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

    func setStreamProperties(_ streamProperties: CMIOExtensionStreamProperties) throws {}

    func authorizedToStartStream(for client: CMIOExtensionClient) -> Bool {
        owner.authorize(client, direction: direction)
        return true
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

    func connect(to client: CMIOExtensionClient) throws {}
    func disconnect(from client: CMIOExtensionClient) {}

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
