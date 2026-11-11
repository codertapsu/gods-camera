import CoreMedia
import CoreMediaIO
import CoreVideo
import Foundation

/// Writes frames into the virtual camera's sink stream.
///
/// Spike S1 feeds a generated test pattern. The live engine will hand its
/// composited frames to `send(_:)` instead.
final class SinkFeeder: ObservableObject {
    @Published var status = "Idle."
    @Published var framesSent = 0

    private var deviceID: CMIODeviceID = 0
    private var streamID: CMIOStreamID = 0
    private var bufferQueue: CMSimpleQueue?
    private var timer: DispatchSourceTimer?
    private var pool: CVPixelBufferPool?
    private var formatDescription: CMFormatDescription?
    private var frameIndex = 0
    private let queue = DispatchQueue(label: "dev.godscamera.feeder")

    var isRunning: Bool { timer != nil }

    func start() {
        guard !isRunning else { return }
        guard let device = Self.findDevice(uid: CameraConfig.deviceUID.uuidString) else {
            status = "Virtual camera not found. Activate the extension first."
            return
        }
        let streams = Self.streams(of: device)
        guard streams.count >= 2 else {
            status = "The virtual camera has no input stream."
            return
        }
        deviceID = device
        streamID = streams[1]  // The extension adds the source stream first, then the sink.

        var unmanagedQueue: Unmanaged<CMSimpleQueue>?
        let copyStatus = CMIOStreamCopyBufferQueue(streamID, { _, _, _ in }, nil, &unmanagedQueue)
        guard copyStatus == noErr, let unmanagedQueue else {
            status = "Could not open the input stream (\(copyStatus))."
            return
        }
        bufferQueue = unmanagedQueue.takeRetainedValue()
        let startStatus = CMIODeviceStartStream(deviceID, streamID)
        guard startStatus == noErr else {
            status = "Could not start the input stream (\(startStatus))."
            return
        }

        CMVideoFormatDescriptionCreate(
            allocator: kCFAllocatorDefault, codecType: CameraConfig.pixelFormat, width: CameraConfig.width,
            height: CameraConfig.height, extensions: nil, formatDescriptionOut: &formatDescription)
        let attributes: [CFString: Any] = [
            kCVPixelBufferPixelFormatTypeKey: CameraConfig.pixelFormat,
            kCVPixelBufferWidthKey: CameraConfig.width,
            kCVPixelBufferHeightKey: CameraConfig.height,
            kCVPixelBufferIOSurfacePropertiesKey: [:] as CFDictionary,
        ]
        CVPixelBufferPoolCreate(kCFAllocatorDefault, nil, attributes as CFDictionary, &pool)

        let timer = DispatchSource.makeTimerSource(queue: queue)
        timer.schedule(deadline: .now(), repeating: 1.0 / Double(CameraConfig.frameRate), leeway: .milliseconds(1))
        timer.setEventHandler { [weak self] in self?.sendTestPattern() }
        timer.resume()
        self.timer = timer
        status = "Sending the test pattern."
    }

    func stop() {
        timer?.cancel()
        timer = nil
        if streamID != 0 { CMIODeviceStopStream(deviceID, streamID) }
        bufferQueue = nil
        status = "Stopped. Other apps now see the placeholder slate."
    }

    /// Queue one frame for the virtual camera. Drops the frame if the queue is full.
    func send(_ pixelBuffer: CVPixelBuffer) {
        guard let bufferQueue, let formatDescription else { return }
        var timing = CMSampleTimingInfo(
            duration: CameraConfig.frameDuration,
            presentationTimeStamp: CMClockGetTime(CMClockGetHostTimeClock()), decodeTimeStamp: .invalid)
        var sampleBuffer: CMSampleBuffer?
        let status = CMSampleBufferCreateForImageBuffer(
            allocator: kCFAllocatorDefault, imageBuffer: pixelBuffer, dataReady: true,
            makeDataReadyCallback: nil, refcon: nil, formatDescription: formatDescription,
            sampleTiming: &timing, sampleBufferOut: &sampleBuffer)
        guard status == noErr, let sampleBuffer else { return }
        guard CMSimpleQueueGetCount(bufferQueue) < CMSimpleQueueGetCapacity(bufferQueue) else { return }
        let pointer = Unmanaged.passRetained(sampleBuffer).toOpaque()
        if CMSimpleQueueEnqueue(bufferQueue, element: pointer) != noErr {
            Unmanaged<CMSampleBuffer>.fromOpaque(pointer).release()
            return
        }
        DispatchQueue.main.async { self.framesSent += 1 }
    }

    /// Colour bars with a white block that crosses the frame once a second,
    /// so frozen, dropped or torn frames are easy to see in a client app.
    private func sendTestPattern() {
        guard let pool else { return }
        var pixelBuffer: CVPixelBuffer?
        guard CVPixelBufferPoolCreatePixelBuffer(kCFAllocatorDefault, pool, &pixelBuffer) == kCVReturnSuccess,
            let pixelBuffer
        else { return }
        CVPixelBufferLockBaseAddress(pixelBuffer, [])
        if let base = CVPixelBufferGetBaseAddress(pixelBuffer) {
            let width = CVPixelBufferGetWidth(pixelBuffer)
            let height = CVPixelBufferGetHeight(pixelBuffer)
            let rowBytes = CVPixelBufferGetBytesPerRow(pixelBuffer)
            // 0xAARRGGBB, stored little-endian as BGRA: white, yellow, cyan, green, magenta, red, blue.
            let bars: [UInt32] = [
                0xFFFF_FFFF, 0xFFFF_FF00, 0xFF00_FFFF, 0xFF00_FF00, 0xFFFF_00FF, 0xFFFF_0000, 0xFF00_00FF,
            ]
            let firstRow = base.bindMemory(to: UInt32.self, capacity: width)
            for x in 0..<width { firstRow[x] = bars[x * bars.count / width] }
            for y in 1..<height { memcpy(base + y * rowBytes, base, width * 4) }
            let blockX = (frameIndex % Int(CameraConfig.frameRate)) * (width - 80) / Int(CameraConfig.frameRate)
            for y in (height / 2 - 40)..<(height / 2 + 40) {
                memset(base + y * rowBytes + blockX * 4, 0xFF, 80 * 4)
            }
        }
        CVPixelBufferUnlockBaseAddress(pixelBuffer, [])
        frameIndex += 1
        send(pixelBuffer)
    }

    // MARK: CoreMediaIO lookups

    private static func address(_ selector: Int) -> CMIOObjectPropertyAddress {
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

    private static func findDevice(uid: String) -> CMIODeviceID? {
        objectIDs(of: CMIOObjectID(kCMIOObjectSystemObject), selector: kCMIOHardwarePropertyDevices).first { device in
            var addr = address(kCMIODevicePropertyDeviceUID)
            let size = UInt32(MemoryLayout<CFString?>.size)
            var used: UInt32 = 0
            var value: Unmanaged<CFString>?
            guard CMIOObjectGetPropertyData(device, &addr, 0, nil, size, &used, &value) == noErr,
                let value
            else { return false }
            return (value.takeRetainedValue() as String) == uid
        }
    }

    private static func streams(of device: CMIODeviceID) -> [CMIOStreamID] {
        objectIDs(of: device, selector: kCMIODevicePropertyStreams)
    }
}
