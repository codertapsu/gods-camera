import Foundation
import SystemExtensions

/// Installs and removes the Camera Extension and reports what macOS answers.
final class ExtensionManager: NSObject, ObservableObject, OSSystemExtensionRequestDelegate {
    static let extensionID = "dev.godscamera.GodsCamera.CameraExtension"

    @Published var status = "Not installed or not checked yet."

    func activate() {
        guard Bundle.main.bundlePath.hasPrefix("/Applications/") else {
            status = "Move God's Camera to the Applications folder first. macOS only installs extensions from there."
            return
        }
        submit(.activationRequest(forExtensionWithIdentifier: Self.extensionID, queue: .main), "Activation requested…")
    }

    func deactivate() {
        submit(.deactivationRequest(forExtensionWithIdentifier: Self.extensionID, queue: .main), "Removal requested…")
    }

    private func submit(_ request: OSSystemExtensionRequest, _ message: String) {
        request.delegate = self
        OSSystemExtensionManager.shared.submitRequest(request)
        status = message
    }

    func request(
        _ request: OSSystemExtensionRequest, actionForReplacingExtension existing: OSSystemExtensionProperties,
        withExtension ext: OSSystemExtensionProperties
    ) -> OSSystemExtensionRequest.ReplacementAction {
        status = "Replacing version \(existing.bundleShortVersion) with \(ext.bundleShortVersion)."
        return .replace
    }

    func requestNeedsUserApproval(_ request: OSSystemExtensionRequest) {
        status = "Waiting for approval: System Settings > General > Login Items & Extensions > Camera Extensions."
    }

    func request(_ request: OSSystemExtensionRequest, didFinishWithResult result: OSSystemExtensionRequest.Result) {
        switch result {
        case .completed: status = "Done. The camera \"\(CameraConfig.deviceName)\" is available to other apps."
        case .willCompleteAfterReboot: status = "Will finish after a restart."
        @unknown default: status = "Finished with an unknown result (\(result.rawValue))."
        }
    }

    func request(_ request: OSSystemExtensionRequest, didFailWithError error: Error) {
        let ns = error as NSError
        status = "Failed: \(ns.localizedDescription) (\(ns.domain) \(ns.code))"
    }
}
