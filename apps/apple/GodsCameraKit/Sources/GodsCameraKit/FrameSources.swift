import AVFoundation
import CoreGraphics
import CoreMedia
import CoreVideo
import Foundation
import ImageIO

/// One picture on its way through the engine.
public struct VideoFrame {
    public let pixelBuffer: CVPixelBuffer
    /// When the picture was captured, on the host clock, in nanoseconds.
    public let hostTimeNs: UInt64

    public init(pixelBuffer: CVPixelBuffer, hostTimeNs: UInt64) {
        self.pixelBuffer = pixelBuffer
        self.hostTimeNs = hostTimeNs
    }
}

/// Anything that produces pictures for the engine: a camera, a file, a test pattern.
public protocol FrameSource: AnyObject {
    /// Called for every picture, on a queue the source owns.
    var onFrame: ((VideoFrame) -> Void)? { get set }
    func start() throws
    func stop()
}

public enum SourceError: Error {
    case unreadable(URL)
    case noVideoTrack(URL)
    case cameraUnavailable
    case cameraNotAuthorized
}

/// Base for sources that produce pictures on a timer.
public class TimedSource: FrameSource {
    public var onFrame: ((VideoFrame) -> Void)?
    public let frameRate: Double
    private let queue: DispatchQueue
    private var timer: DispatchSourceTimer?

    public init(frameRate: Double, label: String) {
        self.frameRate = frameRate
        queue = DispatchQueue(label: label, qos: .userInteractive)
    }

    /// Subclasses return the next picture, or nil to skip a tick.
    func nextBuffer() -> CVPixelBuffer? { nil }

    public func start() throws {
        guard timer == nil else { return }
        let timer = DispatchSource.makeTimerSource(queue: queue)
        timer.schedule(deadline: .now(), repeating: 1.0 / frameRate, leeway: .milliseconds(1))
        timer.setEventHandler { [weak self] in
            guard let self, let buffer = self.nextBuffer() else { return }
            self.onFrame?(VideoFrame(pixelBuffer: buffer, hostTimeNs: hostTimeNanoseconds()))
        }
        timer.resume()
        self.timer = timer
    }

    public func stop() {
        timer?.cancel()
        timer = nil
    }
}

/// Colour bars with a moving block and a machine-readable frame number.
///
/// The frame number is drawn as a row of black and white cells along the top,
/// so a test can tell exactly which frame came out the other end.
public final class TestPatternSource: TimedSource {
    public static let counterBits = 16
    private let pool: PixelBufferPool
    public private(set) var frameIndex: UInt32 = 0

    public init?(size: PixelSize = .hd720, frameRate: Double = 30) {
        guard let pool = PixelBufferPool(size: size) else { return nil }
        self.pool = pool
        super.init(frameRate: frameRate, label: "dev.godscamera.source.pattern")
    }

    override func nextBuffer() -> CVPixelBuffer? {
        guard let buffer = pool.makeBuffer() else { return nil }
        Self.draw(frame: frameIndex, into: buffer, frameRate: Int(frameRate))
        frameIndex &+= 1
        return buffer
    }

    /// Draw frame number `frame` into a BGRA buffer.
    public static func draw(frame: UInt32, into buffer: CVPixelBuffer, frameRate: Int = 30) {
        let width = CVPixelBufferGetWidth(buffer)
        let height = CVPixelBufferGetHeight(buffer)
        _ = buffer.withLockedBytes(readOnly: false) { base, rowBytes in
            // 0xAARRGGBB, stored little-endian as BGRA: white, yellow, cyan, green, magenta, red, blue.
            let bars: [UInt32] = [
                0xFFFF_FFFF, 0xFFFF_FF00, 0xFF00_FFFF, 0xFF00_FF00, 0xFFFF_00FF, 0xFFFF_0000, 0xFF00_00FF,
            ]
            let firstRow = base.bindMemory(to: UInt32.self, capacity: width)
            for x in 0..<width { firstRow[x] = bars[x * bars.count / width] }
            for y in 1..<height { memcpy(base + y * rowBytes, base, width * 4) }

            // A white block that crosses the frame once a second.
            let block = max(8, width / 16)
            let blockX = (Int(frame) % frameRate) * (width - block) / frameRate
            for y in (height / 2 - block / 2)..<(height / 2 + block / 2) {
                memset(base + y * rowBytes + blockX * 4, 0xFF, block * 4)
            }

            // The frame number, most significant bit first, in the top strip.
            let stripHeight = max(8, height / 12)
            let cell = width / counterBits
            for bit in 0..<counterBits {
                let on = (frame >> UInt32(counterBits - 1 - bit)) & 1 == 1
                for y in 0..<stripHeight {
                    memset(base + y * rowBytes + bit * cell * 4, on ? 0xFF : 0x00, cell * 4)
                }
            }
        }
    }

    /// Read the frame number back from a picture, or nil if the strip is unclear.
    public static func readFrameNumber(from buffer: CVPixelBuffer) -> UInt32? {
        let width = CVPixelBufferGetWidth(buffer)
        let height = CVPixelBufferGetHeight(buffer)
        return buffer.withLockedBytes(readOnly: true) { base, rowBytes -> UInt32? in
            let y = max(2, height / 24)  // the middle of the strip
            let cell = width / counterBits
            var value: UInt32 = 0
            for bit in 0..<counterBits {
                let x = bit * cell + cell / 2
                let pixel = base.advanced(by: y * rowBytes + x * 4).assumingMemoryBound(to: UInt8.self)
                let level = (Int(pixel[0]) + Int(pixel[1]) + Int(pixel[2])) / 3
                if level > 96 && level < 160 { return nil }
                value = (value << 1) | (level >= 160 ? 1 : 0)
            }
            return value
        } ?? nil
    }
}

/// Shows one picture from a file, repeated at a steady rate.
public final class StillImageSource: TimedSource {
    private let buffer: CVPixelBuffer

    public init(url: URL, frameRate: Double = 30, maxDimension: Int = 1920) throws {
        guard let source = CGImageSourceCreateWithURL(url as CFURL, nil) else {
            throw SourceError.unreadable(url)
        }
        // Let ImageIO apply the photo's orientation and scale it down in one step.
        let options: [CFString: Any] = [
            kCGImageSourceCreateThumbnailFromImageAlways: true,
            kCGImageSourceCreateThumbnailWithTransform: true,
            kCGImageSourceThumbnailMaxPixelSize: maxDimension,
        ]
        guard let image = CGImageSourceCreateThumbnailAtIndex(source, 0, options as CFDictionary),
            let buffer = Self.makeBuffer(from: image)
        else { throw SourceError.unreadable(url) }
        self.buffer = buffer
        super.init(frameRate: frameRate, label: "dev.godscamera.source.image")
    }

    override func nextBuffer() -> CVPixelBuffer? { buffer }

    static func makeBuffer(from image: CGImage) -> CVPixelBuffer? {
        guard let pool = PixelBufferPool(size: PixelSize(width: image.width, height: image.height)),
            let buffer = pool.makeBuffer()
        else { return nil }
        let drawn = buffer.withLockedBytes(readOnly: false) { base, rowBytes -> Bool in
            // Frames leave the app untagged and are read as sRGB, so pictures in
            // any other colour space are converted to sRGB here.
            guard
                let context = CGContext(
                    data: base, width: image.width, height: image.height, bitsPerComponent: 8,
                    bytesPerRow: rowBytes, space: CGColorSpace(name: CGColorSpace.sRGB)!,
                    bitmapInfo: CGImageAlphaInfo.premultipliedFirst.rawValue
                        | CGBitmapInfo.byteOrder32Little.rawValue)
            else { return false }
            context.draw(image, in: CGRect(x: 0, y: 0, width: image.width, height: image.height))
            return true
        }
        return drawn == true ? buffer : nil
    }
}

/// Plays a video file in a loop, without sound.
public final class VideoFileSource: TimedSource {
    private let player: AVQueuePlayer
    private let looper: AVPlayerLooper
    private let output: AVPlayerItemVideoOutput

    public init(url: URL) throws {
        guard FileManager.default.isReadableFile(atPath: url.path) else { throw SourceError.unreadable(url) }
        let item = AVPlayerItem(url: url)
        output = AVPlayerItemVideoOutput(
            pixelBufferAttributes: CVPixelBufferAttributes(rawAttributes: [
                kCVPixelBufferPixelFormatTypeKey as String: kCVPixelFormatType_32BGRA,
                kCVPixelBufferIOSurfacePropertiesKey as String: [String: String](),
                kCVPixelBufferMetalCompatibilityKey as String: true,
            ]))
        player = AVQueuePlayer()
        player.isMuted = true
        // The looper plays copies of the item, so each copy needs the output attached.
        looper = AVPlayerLooper(player: player, templateItem: item)
        super.init(frameRate: 60, label: "dev.godscamera.source.video")
    }

    public override func start() throws {
        player.play()
        try super.start()
    }

    public override func stop() {
        super.stop()
        player.pause()
    }

    // Polled at 60 Hz; a picture is produced only when the player has a new one.
    override func nextBuffer() -> CVPixelBuffer? {
        guard let item = player.currentItem else { return nil }
        if !item.outputs.contains(output) {
            for other in player.items() where other !== item { other.remove(output) }
            item.add(output)
        }
        let time = item.currentTime()
        guard output.hasNewPixelBuffer(forItemTime: time) else { return nil }
        return output.pixelBufferAndDisplayTime(forItemTime: time).pixelBuffer?.withUnsafeBuffer { $0 }
    }
}

/// A camera the user can pick as a source.
public struct CameraDevice: Identifiable, Hashable, Sendable {
    public let id: String
    public let name: String
}

/// Live pictures from a camera.
public final class CameraSource: NSObject, FrameSource, AVCaptureVideoDataOutputSampleBufferDelegate {
    public var onFrame: ((VideoFrame) -> Void)?
    private let session = AVCaptureSession()
    private let queue = DispatchQueue(label: "dev.godscamera.source.camera", qos: .userInteractive)
    private let deviceID: String
    private let size: PixelSize

    /// `size` is the picture size the output needs; the camera is asked for the nearest one it has.
    public init(deviceID: String, size: PixelSize = .hd720) {
        self.deviceID = deviceID
        self.size = size
    }

    /// Cameras on this Mac, leaving out God's Camera itself so it cannot feed on its own output.
    public static func devices() -> [CameraDevice] {
        let discovery = AVCaptureDevice.DiscoverySession(
            deviceTypes: [.builtInWideAngleCamera, .external, .continuityCamera], mediaType: .video,
            position: .unspecified)
        return discovery.devices
            .filter { $0.uniqueID != CameraConfig.deviceUID.uuidString && $0.localizedName != CameraConfig.deviceName }
            .map { CameraDevice(id: $0.uniqueID, name: $0.localizedName) }
    }

    public static var isAuthorized: Bool { AVCaptureDevice.authorizationStatus(for: .video) == .authorized }

    public func start() throws {
        guard Self.isAuthorized else { throw SourceError.cameraNotAuthorized }
        guard let device = AVCaptureDevice(uniqueID: deviceID),
            let input = try? AVCaptureDeviceInput(device: device)
        else { throw SourceError.cameraUnavailable }
        session.beginConfiguration()
        let wanted: AVCaptureSession.Preset = size.height > 720 ? .hd1920x1080 : .hd1280x720
        if session.canSetSessionPreset(wanted) {
            session.sessionPreset = wanted
        } else if session.canSetSessionPreset(.hd1280x720) {
            session.sessionPreset = .hd1280x720
        } else {
            session.sessionPreset = .high
        }
        if session.canAddInput(input) { session.addInput(input) }
        let output = AVCaptureVideoDataOutput()
        output.videoSettings = [kCVPixelBufferPixelFormatTypeKey as String: kCVPixelFormatType_32BGRA]
        // A late frame is worth less than the next one.
        output.alwaysDiscardsLateVideoFrames = true
        output.setSampleBufferDelegate(self, queue: queue)
        if session.canAddOutput(output) { session.addOutput(output) }
        session.commitConfiguration()
        session.startRunning()
    }

    public func stop() {
        session.stopRunning()
        for input in session.inputs { session.removeInput(input) }
        for output in session.outputs { session.removeOutput(output) }
    }

    public func captureOutput(
        _ output: AVCaptureOutput, didOutput sampleBuffer: CMSampleBuffer, from connection: AVCaptureConnection
    ) {
        guard let buffer = CMSampleBufferGetImageBuffer(sampleBuffer) else { return }
        let seconds = CMSampleBufferGetPresentationTimeStamp(sampleBuffer).seconds
        let hostTime = seconds.isFinite && seconds > 0 ? UInt64(seconds * 1_000_000_000) : hostTimeNanoseconds()
        onFrame?(VideoFrame(pixelBuffer: buffer, hostTimeNs: hostTime))
    }
}
