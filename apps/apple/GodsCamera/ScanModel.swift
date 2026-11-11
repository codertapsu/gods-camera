import AppKit
import GodsCameraKit
import SwiftUI

/// The scanner as the window sees it: the hub's switch, what a device needs
/// to connect, and the documents received.
@MainActor
final class ScanModel: ObservableObject {
    @Published private(set) var running = false
    @Published private(set) var wanted: Bool
    @Published private(set) var address = ""
    @Published private(set) var token = ""
    @Published private(set) var documents: [ScannedDocument] = []
    @Published var message: String?
    /// The adb steps are running.
    @Published private(set) var connecting = false
    /// What the last attempt to connect a device came to.
    @Published private(set) var deviceMessage: String?

    /// Phones and tablets that were connected with a code, and may send scans over the network.
    @Published private(set) var devices: [PairedDevice] = []
    /// The code a device must enter, while one is on offer.
    @Published private(set) var pairingCode: String?
    @Published private(set) var pairingSecondsLeft = 0
    /// The code on offer was guessed wrongly too often, or ran out. A new one is needed.
    @Published private(set) var pairingEnded = false
    /// The device that connected with the code on offer.
    @Published private(set) var justPaired: String?
    @Published var showingPairing = false
    /// The hub listens on the local network, so devices can connect with a code.
    @Published private(set) var networkReady = false
    /// macOS has not allowed the app to use the local network, so devices cannot find this Mac.
    @Published private(set) var networkRefused = false
    /// The name devices on the network see for this Mac.
    let networkName = Host.current().localizedName ?? "Mac"

    private let hub: ScanHub
    private let defaults = UserDefaults.standard

    init() {
        hub = ScanHub(
            dataFolder: FaceModel.supportFolder.appendingPathComponent("Hub", isDirectory: true),
            reader: Bundle.main.url(forAuxiliaryExecutable: "gc-ocr"))
        wanted = defaults.bool(forKey: "hubWanted")
    }

    /// Carry on from the last session. The hub stays off until the user has turned it on.
    func start() {
        documents = hub.documents()
        if wanted { turnOn() }
    }

    func setWanted(_ on: Bool) {
        wanted = on
        defaults.set(on, forKey: "hubWanted")
        if on { turnOn() } else { turnOff() }
    }

    private func turnOn() {
        message = nil
        do {
            token = try ScanHub.token()
            // Without its certificate the hub still serves devices attached for debugging.
            var network: ScanHub.Network?
            do {
                network = ScanHub.Network(name: networkName, identity: try ScanHub.identity(name: networkName))
            } catch {
                Log.app.error("hub identity unavailable: \(String(describing: error))")
            }
            try hub.start(token: token, network: network)
            address = hub.address ?? ""
            running = true
            networkReady = hub.networkPort != nil
            devices = hub.devices()
            if let port = hub.networkPort { Log.app.info("hub listening on the network, port \(port)") }
            Log.app.info("hub listening at \(address)")
        } catch {
            running = false
            message = String(localized: "The scanner could not start. Another program may be using its port.")
            Log.app.error("hub failed to start: \(String(describing: error))")
        }
    }

    private func turnOff() {
        endPairing()
        hub.stop()
        running = false
        networkReady = false
        networkRefused = false
        deviceMessage = nil
    }

    // MARK: Devices connected with a code

    /// Offer a new six-digit code and show it.
    func beginPairing() {
        guard running else { return }
        justPaired = nil
        pairingEnded = false
        do {
            pairingCode = try hub.openPairing()
            pairingSecondsLeft = hub.pairingStatus()?.secondsLeft ?? 0
            showingPairing = true
        } catch {
            message = String(localized: "A code could not be made. Turn the scanner off and on again.")
            Log.app.error("pairing failed to open: \(String(describing: error))")
        }
    }

    /// Withdraw the code on offer.
    func endPairing() {
        if pairingCode != nil { hub.closePairing() }
        pairingCode = nil
        showingPairing = false
    }

    func removeDevice(_ device: PairedDevice) {
        hub.removeDevice(device.id)
        devices = hub.devices()
        Log.app.info("device removed: \(device.name)")
    }

    /// Follow the code on offer: how long it has left, and whether a device used it.
    private func refreshPairing() {
        let latest = hub.devices()
        if latest != devices { devices = latest }
        let refused = hub.announcement == .refused
        if refused != networkRefused { networkRefused = refused }
        guard pairingCode != nil, let status = hub.pairingStatus() else { return }
        pairingSecondsLeft = status.secondsLeft
        if let name = status.pairedDevice {
            justPaired = name
            pairingCode = nil
            Log.app.info("device connected with a code: \(name)")
        } else if !status.isOpen {
            pairingCode = nil
            pairingEnded = true
        }
    }

    /// Connect the Android devices attached for debugging: lead their localhost
    /// to this hub and open the app on them with the hub's token.
    func connectDevices() {
        guard running, !connecting else { return }
        connecting = true
        deviceMessage = nil
        let token = token
        DispatchQueue.global(qos: .userInitiated).async {
            let outcome = DeviceConnector.connect(port: ScanHub.defaultPort, token: token)
            DispatchQueue.main.async {
                self.connecting = false
                switch outcome {
                case .noTool:
                    self.deviceMessage = String(localized: "The Android debugging tool (adb) was not found on this Mac.")
                case .noDevices(let waiting):
                    self.deviceMessage =
                        waiting > 0
                        ? String(localized: "A device is attached but not ready. Unlock it and allow debugging on its screen.")
                        : String(localized: "No device found. Connect it by USB cable, or turn on Wi-Fi debugging.")
                case .connected(let devices):
                    let opened = devices.filter(\.appOpened).map(\.model)
                    let missing = devices.filter { !$0.appOpened }.map(\.model)
                    var parts: [String] = []
                    if !opened.isEmpty {
                        parts.append(String(localized: "Connected: \(opened.joined(separator: ", "))."))
                    }
                    if !missing.isEmpty {
                        parts.append(String(localized: "God's Camera could not be opened on \(missing.joined(separator: ", ")). Install it there first."))
                    }
                    self.deviceMessage = parts.joined(separator: " ")
                }
                Log.app.info("connect devices: \(String(describing: outcome))")
            }
        }
    }

    /// Called once a second while the hub runs.
    func refresh() {
        guard running else { return }
        let latest = hub.documents()
        if latest != documents { documents = latest }
        refreshPairing()
    }

    /// The text of a finished document, for showing it in the window.
    func text(of document: ScannedDocument) -> String {
        (try? String(contentsOf: document.text, encoding: .utf8)) ?? ""
    }

    func copyToken() {
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(token, forType: .string)
    }

    func open(_ document: ScannedDocument) {
        NSWorkspace.shared.open(document.markdown)
    }

    func reveal(_ document: ScannedDocument) {
        NSWorkspace.shared.activateFileViewerSelecting([document.markdown])
    }

    func copyText(_ document: ScannedDocument) {
        guard let text = try? String(contentsOf: document.text, encoding: .utf8) else { return }
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(text, forType: .string)
    }

    func delete(_ document: ScannedDocument) {
        hub.delete(document)
        documents = hub.documents()
    }

    /// Remove every received document and the pictures behind them.
    func deleteEverything() {
        for document in hub.documents() { hub.delete(document) }
        documents = []
        for device in hub.devices() { hub.removeDevice(device.id) }
        devices = []
        // A new certificate too: nothing a removed device holds is of any use afterwards.
        let wasRunning = running
        if wasRunning { turnOff() }
        ScanHub.deleteIdentity()
        if wasRunning { turnOn() }
    }

    // MARK: Sample content

    /// Show the code sheet with a made-up code, for pictures of the app.
    func showSamplePairing(code: String, secondsLeft: Int) {
        pairingCode = code
        pairingSecondsLeft = secondsLeft
        showingPairing = true
    }

    /// Fill the model with made-up content, for pictures of the app. Nothing is saved or started.
    func showSamples(documents: [ScannedDocument], devices: [PairedDevice]) {
        running = true
        wanted = true
        networkReady = true
        self.documents = documents
        self.devices = devices
    }
}
