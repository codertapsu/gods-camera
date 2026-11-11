import Darwin
import Foundation

/// The app a process belongs to, as its user knows it.
///
/// A browser reads the camera from a helper process with a name of its own.
/// What the user wants to allow or block is the browser, so a process is
/// attributed to the outermost app that contains its program file.
public struct AppIdentity: Equatable, Hashable, Sendable {
    /// The bundle identifier of that app, or the path of the program file when
    /// the process is not part of an app.
    public var id: String
    /// The name to show.
    public var name: String

    public init(id: String, name: String) {
        self.id = id
        self.name = name
    }

    /// The app that running process `pid` belongs to, or nil if the process has gone.
    public static func of(pid: Int32) -> AppIdentity? {
        var buffer = [CChar](repeating: 0, count: 4096)
        guard pid > 0, proc_pidpath(pid, &buffer, UInt32(buffer.count)) > 0 else { return nil }
        return of(executablePath: String(cString: buffer))
    }

    public static func of(executablePath path: String) -> AppIdentity {
        var url = URL(fileURLWithPath: path)
        var outermost: URL?
        while url.path != "/" {
            if url.pathExtension == "app" { outermost = url }
            url.deleteLastPathComponent()
        }
        if let outermost, let identifier = Bundle(url: outermost)?.bundleIdentifier {
            return AppIdentity(id: identifier, name: outermost.deletingPathExtension().lastPathComponent)
        }
        return AppIdentity(id: path, name: URL(fileURLWithPath: path).lastPathComponent)
    }
}
