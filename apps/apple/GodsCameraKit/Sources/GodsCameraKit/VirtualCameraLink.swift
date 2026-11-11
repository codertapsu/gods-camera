import CoreMedia
import CoreMediaIO
import CoreVideo
import Foundation

/// The app's side of the virtual camera: finds the Camera Extension's device,
/// writes frames into its input stream, and reads and sets its properties.
public final class VirtualCameraLink: FrameSink {
    public enum LinkError: Error, Equatable {
        /// The Camera Extension is not installed or not running.
        case deviceNotFound
        case noInputStream
        case cannotOpen(OSStatus)
        case cannotStart(OSStatus)
    }

    private var deviceID: CMIODeviceID = 0
    private var streamID: CMIOStreamID = 0
    private var bufferQueue: CMSimpleQueue?
    private var formatDescription: CMFormatDescription?
    private var formatSize = PixelSize(width: 0, height: 0)
    private let lock = NSLock()
    public private(set) var framesSent: UInt64 = 0
    public private(set) var framesDropped: UInt64 = 0
    /// Frames held back because `isMuted` was set.
    public private(set) var framesWithheld: UInt64 = 0
    private var muted = false

    public init() {}

    /// While true, no pictures leave the app and the camera shows its slate.
    /// Set while a blocked app is reading the camera: every reader gets the
    /// same pictures, so none can be sent.
    public var isMuted: Bool {
        get {
            lock.lock()
            defer { lock.unlock() }
            return muted
        }
        set {
            lock.lock()
            muted = newValue
            lock.unlock()
        }
    }

    public var isOpen: Bool {
        lock.lock()
        defer { lock.unlock() }
        return bufferQueue != nil
    }

    /// Whether the system currently lists the virtual camera.
    public static var isInstalled: Bool { findDevice() != nil }

    /// Open the camera's input stream so frames can be sent.
    public func open() throws {
        lock.lock()
        defer { lock.unlock() }
        guard bufferQueue == nil else { return }
        guard let device = Self.findDevice() else { throw LinkError.deviceNotFound }
        let streams = Self.objectIDs(of: device, selector: kCMIODevicePropertyStreams)
        // The extension adds its source stream first and its sink stream second.
        guard streams.count >= 2 else { throw LinkError.noInputStream }
        var queue: Unmanaged<CMSimpleQueue>?
        let copied = CMIOStreamCopyBufferQueue(streams[1], { _, _, _ in }, nil, &queue)
        guard copied == noErr, let queue else { throw LinkError.cannotOpen(copied) }
        let started = CMIODeviceStartStream(device, streams[1])
        guard started == noErr else { throw LinkError.cannotStart(started) }
        deviceID = device
        streamID = streams[1]
        bufferQueue = queue.takeRetainedValue()
    }

    public func close() {
        lock.lock()
        defer { lock.unlock() }
        guard bufferQueue != nil else { return }
        CMIODeviceStopStream(deviceID, streamID)
        bufferQueue = nil
    }

    /// Close the link if the camera it was opened on has gone, for example
    /// because the extension was replaced. The next `open` finds the new one.
    public func closeIfStale() {
        lock.lock()
        let stale = bufferQueue != nil && Self.findDevice() != deviceID
        if stale { bufferQueue = nil }
        lock.unlock()
    }

    /// Queue one frame for the camera. The frame is dropped if the camera has
    /// not taken the previous ones yet; a late frame is never worth waiting for.
    public func send(_ pixelBuffer: CVPixelBuffer, hostTimeNs: UInt64) {
        lock.lock()
        defer { lock.unlock() }
        guard let bufferQueue else { return }
        guard !muted else {
            framesWithheld += 1
            return
        }
        let size = pixelBuffer.pixelSize
        if formatDescription == nil || size != formatSize {
            CMVideoFormatDescriptionCreateForImageBuffer(
                allocator: kCFAllocatorDefault, imageBuffer: pixelBuffer, formatDescriptionOut: &formatDescription)
            formatSize = size
        }
        guard let formatDescription else { return }
        // The capture time travels with the frame so the extension can report
        // how long the frame took to reach it.
        var timing = CMSampleTimingInfo(
            duration: CameraConfig.frameDuration,
            presentationTimeStamp: CMTime(value: CMTimeValue(hostTimeNs), timescale: 1_000_000_000),
            decodeTimeStamp: .invalid)
        var sample: CMSampleBuffer?
        let status = CMSampleBufferCreateForImageBuffer(
            allocator: kCFAllocatorDefault, imageBuffer: pixelBuffer, dataReady: true, makeDataReadyCallback: nil,
            refcon: nil, formatDescription: formatDescription, sampleTiming: &timing, sampleBufferOut: &sample)
        guard status == noErr, let sample else { return }
        guard CMSimpleQueueGetCount(bufferQueue) < CMSimpleQueueGetCapacity(bufferQueue) else {
            framesDropped += 1
            return
        }
        let pointer = Unmanaged.passRetained(sample).toOpaque()
        if CMSimpleQueueEnqueue(bufferQueue, element: pointer) == noErr {
            framesSent += 1
        } else {
            Unmanaged<CMSampleBuffer>.fromOpaque(pointer).release()
            framesDropped += 1
        }
    }

    // MARK: Properties of the camera

    /// The extension's counters, or nil if the camera is not listed.
    public static func readStats() -> VirtualCameraStats? {
        readText(.stats).flatMap(VirtualCameraStats.init(encoded:))
    }

    /// Process numbers the camera refuses to start reading for, or nil if the camera is not listed.
    public static func blockedPids() -> Set<Int32>? {
        readText(.blockedClients).map { Set($0.split(separator: ",").compactMap { Int32($0) }) }
    }

    @discardableResult
    public static func setBlockedPids(_ pids: Set<Int32>) -> Bool {
        writeText(.blockedClients, pids.sorted().map(String.init).joined(separator: ","))
    }

    /// Ask the running extension to stand down so macOS can replace it with a
    /// newer build, and wait until it has gone.
    ///
    /// macOS starts the new build at the instant it stops the old one, and does
    /// not start it at all if the old one is still running then. Returns the
    /// time the hand-over took, or nil if no extension answered.
    public static func handOver(timeout: TimeInterval = 3) -> TimeInterval? {
        guard let stats = readStats(), stats.pid > 0, !stats.build.isEmpty,
            writeText(.handover, stats.build)
        else { return nil }
        let started = Date()
        while Date().timeIntervalSince(started) < timeout {
            // Another user's process answers "not permitted" while it lives and "no such process" once gone.
            if kill(stats.pid, 0) != 0, errno == ESRCH {
                // macOS may start the old build once more; it leaves again at once. Let that pass.
                Thread.sleep(forTimeInterval: 0.6)
                return Date().timeIntervalSince(started)
            }
            Thread.sleep(forTimeInterval: 0.02)
        }
        return nil
    }

    public static func readText(_ property: CameraConfig.Property) -> String? {
        guard let device = findDevice() else { return nil }
        var address = address(Int(property.selector))
        guard CMIOObjectHasProperty(device, &address) else { return nil }
        var value: Unmanaged<CFString>?
        var used: UInt32 = 0
        let size = UInt32(MemoryLayout<CFString?>.size)
        guard CMIOObjectGetPropertyData(device, &address, 0, nil, size, &used, &value) == noErr, let value else {
            return nil
        }
        return value.takeRetainedValue() as String
    }

    @discardableResult
    public static func writeText(_ property: CameraConfig.Property, _ text: String) -> Bool {
        guard let device = findDevice() else { return false }
        var address = address(Int(property.selector))
        guard CMIOObjectHasProperty(device, &address) else { return false }
        var value = text as CFString
        let size = UInt32(MemoryLayout<CFString>.size)
        return withUnsafePointer(to: &value) {
            CMIOObjectSetPropertyData(device, &address, 0, nil, size, $0) == noErr
        }
    }

    // MARK: CoreMediaIO lookups

    static func address(_ selector: Int) -> CMIOObjectPropertyAddress {
        CMIOObjectPropertyAddress(
            mSelector: CMIOObjectPropertySelector(selector),
            mScope: CMIOObjectPropertyScope(kCMIOObjectPropertyScopeGlobal),
            mElement: CMIOObjectPropertyElement(kCMIOObjectPropertyElementMain))
    }

    private static func objectIDs(of object: CMIOObjectID, selector: Int) -> [CMIOObjectID] {
        var addr = address(selector)
        var size: UInt32 = 0
        guard CMIOObjectGetPropertyDataSize(object, &addr, 0, nil, &size) == noErr, size > 0 else { return [] }
        var ids = [CMIOObjectID](repeating: 0, count: Int(size) / MemoryLayout<CMIOObjectID>.size)
        var used: UInt32 = 0
        guard CMIOObjectGetPropertyData(object, &addr, 0, nil, size, &used, &ids) == noErr else { return [] }
        return ids
    }

    static func findDevice() -> CMIODeviceID? {
        let wanted = CameraConfig.deviceUID.uuidString
        return objectIDs(of: CMIOObjectID(kCMIOObjectSystemObject), selector: kCMIOHardwarePropertyDevices).first {
            device in
            var addr = address(kCMIODevicePropertyDeviceUID)
            var value: Unmanaged<CFString>?
            var used: UInt32 = 0
            let size = UInt32(MemoryLayout<CFString?>.size)
            guard CMIOObjectGetPropertyData(device, &addr, 0, nil, size, &used, &value) == noErr, let value else {
                return false
            }
            return (value.takeRetainedValue() as String) == wanted
        }
    }
}

/// Calls back when the virtual camera appears or disappears, or when the apps
/// reading it or the format they asked for change.
public final class VirtualCameraWatcher {
    private let queue: DispatchQueue
    private let handler: () -> Void
    private var watchedDevice: CMIODeviceID = 0
    private var deviceBlock: CMIOObjectPropertyListenerBlock?
    private var systemBlock: CMIOObjectPropertyListenerBlock?
    /// How many times the camera itself reported a change. For the self-test.
    public private(set) var cameraEvents = 0

    /// `handler` runs on `queue`.
    public init(queue: DispatchQueue = .main, handler: @escaping () -> Void) {
        self.queue = queue
        self.handler = handler
        let block: CMIOObjectPropertyListenerBlock = { [weak self] _, _ in
            self?.attach()
            self?.handler()
        }
        systemBlock = block
        var address = VirtualCameraLink.address(kCMIOHardwarePropertyDevices)
        CMIOObjectAddPropertyListenerBlock(CMIOObjectID(kCMIOObjectSystemObject), &address, queue, block)
        attach()
    }

    deinit {
        if let systemBlock {
            var address = VirtualCameraLink.address(kCMIOHardwarePropertyDevices)
            CMIOObjectRemovePropertyListenerBlock(CMIOObjectID(kCMIOObjectSystemObject), &address, queue, systemBlock)
        }
        detach()
    }

    /// Listen to the camera that is listed now, if it is not the one already listened to.
    public func attach() {
        let current = VirtualCameraLink.findDevice() ?? 0
        guard current != watchedDevice else { return }
        detach()
        guard current != 0 else { return }
        let block: CMIOObjectPropertyListenerBlock = { [weak self] _, _ in
            self?.cameraEvents += 1
            self?.handler()
        }
        var address = VirtualCameraLink.address(Int(CameraConfig.Property.stats.selector))
        if CMIOObjectAddPropertyListenerBlock(current, &address, queue, block) == noErr {
            watchedDevice = current
            deviceBlock = block
        }
    }

    private func detach() {
        if watchedDevice != 0, let deviceBlock {
            var address = VirtualCameraLink.address(Int(CameraConfig.Property.stats.selector))
            CMIOObjectRemovePropertyListenerBlock(watchedDevice, &address, queue, deviceBlock)
        }
        watchedDevice = 0
        deviceBlock = nil
    }
}
