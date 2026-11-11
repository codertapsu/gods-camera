import CoreMedia
import Foundation

/// Constants shared by the Camera Extension and the app that feeds it.
enum CameraConfig {
    static let deviceName = "God's Camera"
    /// Stable identifier, so client apps remember the camera between launches.
    static let deviceUID = UUID(uuidString: "6B6F9A3E-5C0B-4E5E-9D8E-0C1A5F0D7A01")!
    static let sourceStreamUID = UUID(uuidString: "6B6F9A3E-5C0B-4E5E-9D8E-0C1A5F0D7A02")!
    static let sinkStreamUID = UUID(uuidString: "6B6F9A3E-5C0B-4E5E-9D8E-0C1A5F0D7A03")!
    static let width: Int32 = 1280
    static let height: Int32 = 720
    static let frameRate: Int32 = 30
    static let pixelFormat = kCVPixelFormatType_32BGRA
    static var frameDuration: CMTime { CMTime(value: 1, timescale: frameRate) }
}

func hostTimeNanoseconds() -> UInt64 {
    UInt64(CMClockGetTime(CMClockGetHostTimeClock()).seconds * 1_000_000_000)
}
