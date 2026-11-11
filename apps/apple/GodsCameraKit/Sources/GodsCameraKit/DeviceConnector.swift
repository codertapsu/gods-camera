import Foundation

/// Connects Android devices attached for debugging to the hub on this Mac.
///
/// The hub in development mode listens only on this Mac's own address. For
/// each device that is attached by USB cable or Wi-Fi debugging, this makes
/// the device's localhost port lead to the hub's, and opens the app on the
/// device with the hub's token, so nothing has to be typed there. It does what
/// `tools/connect-android.sh` does, from inside the app.
public enum DeviceConnector {
    public struct Device: Equatable, Sendable {
        /// The device's model name, such as `SM-X816B`.
        public let model: String
        /// False when the app is not installed on the device, so it could not be opened.
        public let appOpened: Bool
    }

    public enum Outcome: Equatable, Sendable {
        /// The Android debugging tool, adb, is not on this Mac.
        case noTool
        /// Nothing usable is attached. `waiting` counts devices that are
        /// attached but have not allowed debugging, or are offline.
        case noDevices(waiting: Int)
        case connected([Device])
    }

    /// The places adb is usually found. A GUI app does not see the shell's PATH.
    static var candidates: [String] {
        let home = FileManager.default.homeDirectoryForCurrentUser.path
        return [
            "/opt/homebrew/share/android-commandlinetools/platform-tools/adb",
            "\(home)/Library/Android/sdk/platform-tools/adb",
            "/opt/homebrew/bin/adb",
            "/usr/local/bin/adb",
        ]
    }

    public static func findTool() -> URL? {
        candidates.first { FileManager.default.isExecutableFile(atPath: $0) }.map(URL.init(fileURLWithPath:))
    }

    /// The devices `adb devices` lists as ready, and how many it lists as not ready.
    static func parseDevices(_ output: String) -> (ready: [String], waiting: Int) {
        var ready: [String] = []
        var waiting = 0
        for line in output.split(whereSeparator: \.isNewline).dropFirst() {
            let fields = line.split(whereSeparator: { $0 == "\t" || $0 == " " })
            guard fields.count >= 2 else { continue }
            if fields[1] == "device" { ready.append(String(fields[0])) } else { waiting += 1 }
        }
        return (ready, waiting)
    }

    /// Run a tool and return what it printed, or nil if it could not run or took too long.
    private static func run(_ tool: URL, _ arguments: [String], seconds: Double = 10) -> String? {
        let process = Process()
        process.executableURL = tool
        process.arguments = arguments
        let pipe = Pipe()
        process.standardOutput = pipe
        process.standardError = pipe
        do { try process.run() } catch { return nil }
        let deadline = Date().addingTimeInterval(seconds)
        while process.isRunning, Date() < deadline { Thread.sleep(forTimeInterval: 0.05) }
        if process.isRunning {
            process.terminate()
            return nil
        }
        let data = pipe.fileHandleForReading.readDataToEndOfFile()
        return String(data: data, encoding: .utf8)
    }

    /// Connect every ready device. Blocks for a second or two; call it off the main thread.
    public static func connect(port: UInt16, token: String, appID: String = "dev.godscamera.android") -> Outcome {
        guard let adb = findTool() else { return .noTool }
        guard let listing = run(adb, ["devices"]) else { return .noDevices(waiting: 0) }
        let (ready, waiting) = parseDevices(listing)
        guard !ready.isEmpty else { return .noDevices(waiting: waiting) }
        var devices: [Device] = []
        for serial in ready {
            _ = run(adb, ["-s", serial, "reverse", "tcp:\(port)", "tcp:\(port)"])
            let model = run(adb, ["-s", serial, "shell", "getprop", "ro.product.model"])?
                .trimmingCharacters(in: .whitespacesAndNewlines) ?? serial
            let started = run(
                adb, ["-s", serial, "shell", "am", "start", "-n", "\(appID)/.MainActivity", "--es", "hub_token", token])
            let opened = started.map { $0.contains("Starting:") && !$0.contains("Error") } ?? false
            devices.append(Device(model: model.isEmpty ? serial : model, appOpened: opened))
        }
        return .connected(devices)
    }
}
