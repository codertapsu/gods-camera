import Foundation
import GodsCameraKit
import SystemExtensions

/// Installs, updates and removes the Camera Extension, and reports its state.
final class ExtensionManager: NSObject, ObservableObject, OSSystemExtensionRequestDelegate {
    static let extensionID = "dev.godscamera.GodsCamera.CameraExtension"

    enum State: Equatable {
        case unknown
        case notInstalled
        /// Installed, but a different build from the one inside this app.
        case outdated(installed: String)
        case installed(version: String)
        case waitingForApproval
        case needsRestart
        case failed(String)
    }

    @Published private(set) var state = State.unknown
    /// How long the running extension took to stand down before the last replacement, if it was asked.
    private(set) var lastHandover: TimeInterval?
    /// True once a replacement has been requested in this run of the app.
    private(set) var replaced = false
    /// True once macOS has finished installing or replacing the extension in this run of the app.
    /// A running app does not get to see a camera installed after it started, so it has to reopen.
    private(set) var activatedInThisRun = false
    private var activating = false
    /// Called on the main queue whenever `state` changes. Used by the self-test.
    var onChange: ((State) -> Void)?

    /// The build number of the extension bundled inside this app.
    static var bundledBuild: String {
        let url = Bundle.main.bundleURL
            .appendingPathComponent("Contents/Library/SystemExtensions/\(extensionID).systemextension")
        return Bundle(url: url)?.object(forInfoDictionaryKey: "CFBundleVersion") as? String ?? "?"
    }

    private func set(_ new: State) {
        DispatchQueue.main.async {
            self.state = new
            self.onChange?(new)
        }
    }

    /// Ask macOS which version of the extension is installed.
    func refresh() {
        submit(.propertiesRequest(forExtensionWithIdentifier: Self.extensionID, queue: .main))
    }

    func activate() {
        guard Bundle.main.bundlePath.hasPrefix("/Applications/") else {
            set(.failed(String(localized: "Move God's Camera to the Applications folder first. macOS only installs camera extensions from there.")))
            return
        }
        activating = true
        submit(.activationRequest(forExtensionWithIdentifier: Self.extensionID, queue: .main))
    }

    func deactivate() {
        submit(.deactivationRequest(forExtensionWithIdentifier: Self.extensionID, queue: .main))
    }

    private func submit(_ request: OSSystemExtensionRequest) {
        request.delegate = self
        OSSystemExtensionManager.shared.submitRequest(request)
    }

    // MARK: OSSystemExtensionRequestDelegate

    func request(
        _ request: OSSystemExtensionRequest, actionForReplacingExtension existing: OSSystemExtensionProperties,
        withExtension ext: OSSystemExtensionProperties
    ) -> OSSystemExtensionRequest.ReplacementAction {
        // macOS starts the new build at the instant it stops the old one, and gives
        // up if the old one is still running. So the old one is asked to leave first.
        lastHandover = VirtualCameraLink.handOver()
        replaced = true
        let handover = lastHandover.map { String(format: "old build stood down in %.2f s", $0) } ?? "old build did not answer"
        Log.app.info("replacing extension build \(existing.bundleVersion) with \(ext.bundleVersion); \(handover)")
        return .replace
    }

    func requestNeedsUserApproval(_ request: OSSystemExtensionRequest) {
        set(.waitingForApproval)
    }

    func request(_ request: OSSystemExtensionRequest, foundProperties properties: [OSSystemExtensionProperties]) {
        // Several entries can exist while an update is in progress; the enabled one counts.
        guard let current = properties.first(where: { $0.isEnabled && !$0.isUninstalling }) else {
            set(properties.contains { $0.isAwaitingUserApproval } ? .waitingForApproval : .notInstalled)
            return
        }
        set(current.bundleVersion == Self.bundledBuild
            ? .installed(version: current.bundleShortVersion)
            : .outdated(installed: current.bundleVersion))
    }

    func request(_ request: OSSystemExtensionRequest, didFinishWithResult result: OSSystemExtensionRequest.Result) {
        if activating, result == .completed { activatedInThisRun = true }
        activating = false
        switch result {
        case .completed: refresh()
        case .willCompleteAfterReboot: set(.needsRestart)
        @unknown default: set(.failed("result \(result.rawValue)"))
        }
    }

    func request(_ request: OSSystemExtensionRequest, didFailWithError error: Error) {
        activating = false
        let ns = error as NSError
        Log.app.error("extension request failed: \(ns.domain) \(ns.code) \(ns.localizedDescription)")
        // A properties request fails this way when nothing is installed yet.
        if ns.domain == OSSystemExtensionErrorDomain, ns.code == OSSystemExtensionError.extensionNotFound.rawValue {
            set(.notInstalled)
        } else {
            set(.failed("\(ns.localizedDescription) (\(ns.code))"))
        }
    }
}
