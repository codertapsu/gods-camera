import CoreMedia
import Foundation

/// Constants shared by the Camera Extension and the app that feeds it.
///
/// This file is compiled into both the GodsCameraKit package and the extension
/// target, so the two sides cannot drift apart.
public enum CameraConfig {
    public static let deviceName = "God's Camera"
    /// The app that owns the camera. Only it may write pictures into the camera.
    public static let appBundleID = "dev.godscamera.GodsCamera"
    /// Stable identifier, so client apps remember the camera between launches.
    public static let deviceUID = UUID(uuidString: "6B6F9A3E-5C0B-4E5E-9D8E-0C1A5F0D7A01")!
    public static let sourceStreamUID = UUID(uuidString: "6B6F9A3E-5C0B-4E5E-9D8E-0C1A5F0D7A02")!
    public static let sinkStreamUID = UUID(uuidString: "6B6F9A3E-5C0B-4E5E-9D8E-0C1A5F0D7A03")!

    /// Sizes the virtual camera offers to client apps: width, height.
    public static let formats: [(width: Int32, height: Int32)] = [(1280, 720), (1920, 1080)]
    /// Client apps may ask for any frame rate between these two.
    public static let minFrameRate: Int32 = 30
    public static let maxFrameRate: Int32 = 60
    /// The rate used until a client asks for another one.
    public static let frameRate: Int32 = 30
    public static let pixelFormat = kCVPixelFormatType_32BGRA
    public static var frameDuration: CMTime { CMTime(value: 1, timescale: frameRate) }

    /// Custom device properties. Each is named by a four-character code and carries text.
    public enum Property: String, CaseIterable, Sendable {
        /// Read: the extension's counters, see `VirtualCameraStats`. The system keeps a
        /// copy on the reading side, so the extension announces a new value about once a second.
        case stats = "gcst"
        /// Read and write: process numbers that may not start reading the camera, separated by commas.
        /// The extension cannot tell which app a process belongs to; the app can, and sends the numbers.
        case blockedClients = "gcbp"
        /// Read: the extension's build number. Write that same number to make the
        /// extension stand down for a few seconds so macOS can replace it.
        case handover = "gcqt"

        /// The name CoreMediaIO extensions use for a custom property.
        public var key: String { "4cc_\(rawValue)_glob_0000" }
        /// The selector a client passes to `CMIOObjectGetPropertyData`.
        public var selector: UInt32 { rawValue.utf8.reduce(0) { $0 << 8 | UInt32($1) } }
    }

    /// How long the extension stays down after a hand-over request, in seconds.
    public static let handoverSeconds = 12.0
}

public func hostTimeNanoseconds() -> UInt64 {
    UInt64(CMClockGetTime(CMClockGetHostTimeClock()).seconds * 1_000_000_000)
}

/// One app reading the virtual camera.
public struct CameraClient: Equatable, Hashable, Sendable, Identifiable {
    /// The app's code-signing identifier, for example `us.zoom.xos`.
    public var signingID: String
    public var pid: Int32

    public var id: String { "\(signingID)@\(pid)" }

    public init(signingID: String, pid: Int32) {
        self.signingID = signingID
        self.pid = pid
    }
}

/// What the extension reports through `CameraConfig.Property.stats`.
public struct VirtualCameraStats: Equatable, Sendable {
    /// Frames received from the app.
    public var framesIn: UInt64 = 0
    /// Frames delivered to client apps, slate frames included.
    public var framesOut: UInt64 = 0
    /// Slate frames delivered because the app was not sending.
    public var slateFrames: UInt64 = 0
    /// Client apps currently reading the camera.
    public var clients: Int = 0
    /// Smoothed time from capture to delivery, in milliseconds.
    public var latencyMs: Double = 0
    /// Largest such time since the counters were last read.
    public var maxLatencyMs: Double = 0
    /// The picture size and frame rate the client apps asked for.
    public var width: Int = Int(CameraConfig.formats[0].width)
    public var height: Int = Int(CameraConfig.formats[0].height)
    public var frameRate: Double = Double(CameraConfig.frameRate)
    /// The extension's process and build, used when it is replaced by a newer one.
    public var pid: Int32 = 0
    public var build: String = ""
    /// Whether the extension could confirm that the pictures come from the God's Camera app.
    public var writerVerified = false
    public var clientList: [CameraClient] = []
    /// Processes connected to the camera, whether or not they are reading it now.
    public var connected: [Int32] = []

    public init() {}

    public var encoded: String {
        let list = clientList.map(\.id).joined(separator: ",")
        let connectedList = connected.map(String.init).joined(separator: ",")
        return "in=\(framesIn);out=\(framesOut);slate=\(slateFrames);clients=\(clients);"
            + String(format: "lat=%.2f;max=%.2f;", latencyMs, maxLatencyMs)
            + "w=\(width);h=\(height);" + String(format: "fps=%.2f;", frameRate)
            + "pid=\(pid);build=\(build);writer=\(writerVerified ? 1 : 0);ids=\(list);conn=\(connectedList)"
    }

    public init?(encoded: String) {
        var fields: [String: String] = [:]
        for part in encoded.split(separator: ";") {
            let pair = part.split(separator: "=", maxSplits: 1)
            if pair.count == 2 { fields[String(pair[0])] = String(pair[1]) }
        }
        guard let framesIn = fields["in"].flatMap(UInt64.init),
            let framesOut = fields["out"].flatMap(UInt64.init),
            let slate = fields["slate"].flatMap(UInt64.init),
            let clients = fields["clients"].flatMap(Int.init),
            let latency = fields["lat"].flatMap(Double.init),
            let maxLatency = fields["max"].flatMap(Double.init)
        else { return nil }
        self.framesIn = framesIn
        self.framesOut = framesOut
        self.slateFrames = slate
        self.clients = clients
        self.latencyMs = latency
        self.maxLatencyMs = maxLatency
        // Later additions are optional, so an older extension can still be read.
        if let width = fields["w"].flatMap(Int.init), let height = fields["h"].flatMap(Int.init), width > 0, height > 0 {
            self.width = width
            self.height = height
        }
        if let rate = fields["fps"].flatMap(Double.init), rate > 0 { self.frameRate = rate }
        self.pid = fields["pid"].flatMap(Int32.init) ?? 0
        self.build = fields["build"] ?? ""
        self.writerVerified = fields["writer"] == "1"
        self.connected = (fields["conn"] ?? "").split(separator: ",").compactMap { Int32($0) }
        self.clientList = (fields["ids"] ?? "").split(separator: ",").compactMap { entry in
            let pair = entry.split(separator: "@", maxSplits: 1)
            guard pair.count == 2, let pid = Int32(pair[1]) else { return nil }
            return CameraClient(signingID: String(pair[0]), pid: pid)
        }
    }
}
