import AppKit

/// Starts a fresh copy of the app and ends this one.
///
/// Needed after the camera is installed or updated: an app that was already
/// running does not see the new camera, however long it waits.
enum Relauncher {
    static let marker = "--relaunched"

    /// True when this copy was started by `relaunch`, so it never does so again.
    static var wasRelaunched: Bool { CommandLine.arguments.contains(marker) }

    static func relaunch(extraArguments: [String] = []) {
        let configuration = NSWorkspace.OpenConfiguration()
        configuration.createsNewApplicationInstance = true
        configuration.arguments = Array(CommandLine.arguments.dropFirst()) + [marker] + extraArguments
        NSWorkspace.shared.openApplication(at: Bundle.main.bundleURL, configuration: configuration) { _, _ in
            DispatchQueue.main.async { NSApp.terminate(nil) }
        }
    }
}
