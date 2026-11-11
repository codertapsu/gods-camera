import Foundation
import Security
import os.log

/// Small text files in the extension's own container.
///
/// The extension runs as a system user, not as the person at the keyboard, so
/// it cannot share preferences or an app group folder with the app. What must
/// outlive one run of the extension is kept here.
enum Store {
    private static let directory: URL? = {
        guard let base = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask).first else {
            return nil
        }
        let directory = base.appendingPathComponent("GodsCamera", isDirectory: true)
        try? FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        return directory
    }()

    static func read(_ name: String) -> String? {
        guard let url = directory?.appendingPathComponent(name) else { return nil }
        return try? String(contentsOf: url, encoding: .utf8)
    }

    static func write(_ name: String, _ text: String) {
        guard let url = directory?.appendingPathComponent(name) else { return }
        try? text.write(to: url, atomically: true, encoding: .utf8)
    }

    static func remove(_ name: String) {
        guard let url = directory?.appendingPathComponent(name) else { return }
        try? FileManager.default.removeItem(at: url)
    }
}

/// Standing down so macOS can replace this build with a newer one.
///
/// macOS stops the old build and starts the new one in the same instant. If the
/// old build is still running at that moment, the new one is never started and
/// the camera is missing until the Mac restarts. So the app first asks the old
/// build to leave, and the old build refuses to come back for a few seconds.
enum Handover {
    static let build = Bundle.main.object(forInfoDictionaryKey: "CFBundleVersion") as? String ?? "0"
    private static let file = "handover"

    /// True while this build has been asked to stay out of the way.
    static var shouldStayDown: Bool {
        guard let text = read() else { return false }
        let now = Date().timeIntervalSince1970
        // A time further ahead than one hand-over lasts cannot be right; ignore it
        // rather than keep the camera away.
        if text.build == build, now < text.until, text.until - now <= CameraConfig.handoverSeconds { return true }
        Store.remove(file)
        return false
    }

    /// Remember the request and leave. Called when the app writes the hand-over property.
    static func begin() {
        Store.write(file, "\(build) \(Date().timeIntervalSince1970 + CameraConfig.handoverSeconds)")
        // Give the reply to the app a moment to go out first.
        DispatchQueue.global().asyncAfter(deadline: .now() + .milliseconds(40)) { exit(0) }
    }

    private static func read() -> (build: String, until: Double)? {
        guard let parts = Store.read(file)?.split(separator: " "), parts.count == 2, let until = Double(parts[1])
        else { return nil }
        return (String(parts[0]), until)
    }
}

/// Who a client process is, according to its code signature.
enum ClientIdentity {
    struct Identity: Equatable {
        var signingID: String
        var team: String
    }

    /// This extension's own identity; the app that feeds it is signed by the same team.
    static let own = of(pid: getpid())

    /// The identity of a running process, or nil if the system will not say.
    static func of(pid: pid_t) -> Identity? {
        guard pid > 0 else { return nil }
        var code: SecCode?
        let attributes = [kSecGuestAttributePid as String: pid] as CFDictionary
        guard SecCodeCopyGuestWithAttributes(nil, attributes, [], &code) == errSecSuccess, let code else { return nil }
        var staticCode: SecStaticCode?
        guard SecCodeCopyStaticCode(code, [], &staticCode) == errSecSuccess, let staticCode else { return nil }
        var information: CFDictionary?
        guard
            SecCodeCopySigningInformation(staticCode, SecCSFlags(rawValue: kSecCSSigningInformation), &information)
                == errSecSuccess,
            let values = information as? [String: Any],
            let identifier = values[kSecCodeInfoIdentifier as String] as? String
        else { return nil }
        return Identity(signingID: identifier, team: values[kSecCodeInfoTeamIdentifier as String] as? String ?? "")
    }
}
