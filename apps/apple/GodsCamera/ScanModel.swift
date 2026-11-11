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
            try hub.start(token: token)
            address = hub.address ?? ""
            running = true
            Log.app.info("hub listening at \(address)")
        } catch {
            running = false
            message = String(localized: "The scanner could not start. Another program may be using its port.")
            Log.app.error("hub failed to start: \(String(describing: error))")
        }
    }

    private func turnOff() {
        hub.stop()
        running = false
        deviceMessage = nil
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
    }
}

/// The scanner's part of the window.
struct ScannerSection: View {
    @ObservedObject var scan: ScanModel

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            Text("Scanner").font(.headline)
            Toggle("Receive scans from my phone or tablet", isOn: Binding(get: { scan.wanted }, set: { scan.setWanted($0) }))
            if scan.running {
                Text("Attach an Android device for debugging, by USB cable or Wi-Fi, then click Connect a device. The app on the device opens, already set up.")
                    .font(.caption).foregroundStyle(.secondary)
                HStack {
                    Button("Connect a device") { scan.connectDevices() }.disabled(scan.connecting)
                    if scan.connecting { ProgressView().controlSize(.small) }
                }
                if let result = scan.deviceMessage {
                    Text(result).font(.caption)
                }
                HStack {
                    Text(scan.token).font(.caption.monospaced()).lineLimit(1).truncationMode(.middle).textSelection(.enabled)
                    Spacer()
                    Button("Copy") { scan.copyToken() }.buttonStyle(.borderless)
                }
            }
            if let message = scan.message {
                Text(message).font(.caption).foregroundStyle(.orange)
            }
            ForEach(scan.documents) { document in
                HStack(alignment: .top) {
                    VStack(alignment: .leading, spacing: 1) {
                        Text(title(of: document)).lineLimit(1)
                        Text(document.receivedAt, format: .dateTime.day().month().hour().minute())
                            .font(.caption).foregroundStyle(.secondary)
                    }
                    Spacer()
                    if document.isDone {
                        Button {
                            scan.open(document)
                        } label: {
                            Image(systemName: "doc.text")
                        }
                        .buttonStyle(.borderless).help(String(localized: "Open the document"))
                        Button {
                            scan.copyText(document)
                        } label: {
                            Image(systemName: "doc.on.doc")
                        }
                        .buttonStyle(.borderless).help(String(localized: "Copy the text"))
                        Button {
                            scan.reveal(document)
                        } label: {
                            Image(systemName: "folder")
                        }
                        .buttonStyle(.borderless).help(String(localized: "Show in Finder"))
                    }
                    Button(role: .destructive) {
                        scan.delete(document)
                    } label: {
                        Image(systemName: "trash")
                    }
                    .buttonStyle(.borderless).help(String(localized: "Delete this document"))
                }
                .font(.callout)
            }
        }
    }

    private func title(of document: ScannedDocument) -> String {
        switch document.status {
        case "done": return document.title.isEmpty ? String(localized: "Document without text") : document.title
        case "failed": return String(localized: "Could not be read")
        default: return String(localized: "Reading \(Int(document.progress * 100)) %…")
        }
    }
}
