import AppKit
import GodsCameraKit
import SwiftUI

/// The scanner: connecting a phone or tablet, and the documents it has sent.
struct ScannerScreen: View {
    @ObservedObject var scan: ScanModel
    @State private var openDocument: String?
    @State private var removing: PairedDevice?

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 16) {
                ScreenHeader(screen: .scanner)
                HStack(alignment: .top, spacing: 16) {
                    devices
                    steps
                }
                documents
            }
            .padding(24)
            .frame(maxWidth: 980, alignment: .leading)
        }
        .sheet(isPresented: $scan.showingPairing, onDismiss: { scan.endPairing() }) {
            PairingSheet(scan: scan)
        }
        .confirmationDialog(
            String(localized: "Remove \(removing?.name ?? "")?"),
            isPresented: Binding(get: { removing != nil }, set: { if !$0 { removing = nil } })
        ) {
            Button("Remove", role: .destructive) {
                if let removing { scan.removeDevice(removing) }
                removing = nil
            }
        } message: {
            Text("The device can no longer send scans to this Mac. To use it again, connect it with a new code.")
        }
    }

    // MARK: Devices

    private var devices: some View {
        Card(String(localized: "Phones and tablets"), symbol: "ipad.and.iphone") {
            Toggle(isOn: Binding(get: { scan.wanted }, set: { scan.setWanted($0) })) {
                Text("Receive scans on this Mac")
            }
            .toggleStyle(.switch)

            if let message = scan.message {
                Hint(message, symbol: "exclamationmark.triangle", tone: .orange)
            }
            if scan.running {
                ForEach(scan.devices) { device in
                    HStack(spacing: 10) {
                        Image(systemName: "ipad.landscape").font(.title2).foregroundStyle(.tint).frame(width: 30)
                        VStack(alignment: .leading, spacing: 2) {
                            Text(device.name).font(.body.weight(.medium)).lineLimit(1)
                            Text(lastSeen(device)).font(.caption).foregroundStyle(.secondary)
                        }
                        Spacer()
                        Button("Remove") { removing = device }
                    }
                }
                if scan.devices.isEmpty {
                    Hint(String(localized: "No phone or tablet is connected yet."))
                }
                Button {
                    scan.beginPairing()
                } label: {
                    Label("Connect a phone or tablet…", systemImage: "plus")
                }
                .buttonStyle(.borderedProminent)
                .controlSize(.large)
                .disabled(!scan.networkReady)
                if !scan.networkReady {
                    Hint(
                        String(localized: "This Mac could not open its network port, so devices cannot connect with a code. Another program may be using port \(String(ScanHub.defaultNetworkPort))."),
                        symbol: "exclamationmark.triangle", tone: .orange)
                } else if scan.networkRefused {
                    Hint(
                        String(localized: "macOS is not letting God's Camera use the local network, so devices cannot find this Mac. Allow it in System Settings › Privacy & Security › Local Network, then turn this switch off and on."),
                        symbol: "wifi.exclamationmark", tone: .orange)
                } else if scan.networkWaiting {
                    Hint(
                        String(localized: "macOS is asking whether God's Camera may find devices on your local network. Choose Allow, so that your phone or tablet can find this Mac."),
                        symbol: "wifi.exclamationmark", tone: .orange)
                }

                DisclosureGroup(String(localized: "For development")) {
                    VStack(alignment: .leading, spacing: 8) {
                        Hint(String(localized: "An Android device attached for debugging, by USB cable or Wi-Fi, can be connected without a code."))
                        HStack {
                            Button("Connect over debugging") { scan.connectDevices() }.disabled(scan.connecting)
                            if scan.connecting { ProgressView().controlSize(.small) }
                        }
                        if let result = scan.deviceMessage {
                            Text(result).font(.callout)
                        }
                    }
                    .padding(.top, 6)
                }
                .font(.callout)
            } else {
                Hint(String(localized: "Turn this on, then connect your phone or tablet with a six-digit code. Both must be on the same Wi-Fi network."))
            }
        }
        .frame(maxWidth: .infinity)
    }

    private func lastSeen(_ device: PairedDevice) -> String {
        guard let seen = device.lastSeen else { return String(localized: "Connected, not used yet") }
        if Date().timeIntervalSince(seen) < 90 { return String(localized: "Active now") }
        let formatter = RelativeDateTimeFormatter()
        formatter.unitsStyle = .full
        return String(localized: "Last used \(formatter.localizedString(for: seen, relativeTo: Date()))")
    }

    private var steps: some View {
        Card(String(localized: "How it works"), symbol: "list.number") {
            step(1, String(localized: "Open God's Camera on your phone or tablet and photograph the pages. Long pages can be taken in several overlapping photos."))
            step(2, String(localized: "Tap Send. The photos go to this Mac over your Wi-Fi network, not through the internet."))
            step(3, String(localized: "This Mac reads the text, joins the photos into one document, and shows it below."))
        }
        .frame(maxWidth: .infinity)
    }

    private func step(_ number: Int, _ text: String) -> some View {
        HStack(alignment: .firstTextBaseline, spacing: 10) {
            Text("\(number)")
                .font(.callout.bold()).monospacedDigit()
                .frame(width: 22, height: 22)
                .background(.tint.opacity(0.18), in: Circle())
            Text(text).font(.callout).fixedSize(horizontal: false, vertical: true)
        }
    }

    // MARK: Documents

    private var documents: some View {
        Card(String(localized: "Documents"), symbol: "doc.text") {
            if scan.documents.isEmpty {
                Hint(String(localized: "Documents you scan appear here."))
            }
            ForEach(scan.documents) { document in
                VStack(alignment: .leading, spacing: 8) {
                    HStack(spacing: 10) {
                        Image(systemName: icon(for: document)).font(.title3).foregroundStyle(.secondary).frame(width: 26)
                        VStack(alignment: .leading, spacing: 2) {
                            Text(title(of: document)).font(.body.weight(.medium)).lineLimit(1)
                            Text(document.receivedAt, format: .dateTime.day().month().hour().minute())
                                .font(.caption).foregroundStyle(.secondary)
                        }
                        Spacer()
                        if document.isDone {
                            Button {
                                openDocument = openDocument == document.id ? nil : document.id
                            } label: {
                                Label(
                                    openDocument == document.id ? String(localized: "Hide") : String(localized: "Read"),
                                    systemImage: openDocument == document.id ? "chevron.up" : "text.alignleft")
                            }
                            Button {
                                scan.copyText(document)
                            } label: {
                                Label("Copy the text", systemImage: "doc.on.doc")
                            }
                            Menu {
                                Button("Open the document") { scan.open(document) }
                                Button("Show in Finder") { scan.reveal(document) }
                                Divider()
                                Button("Delete", role: .destructive) { scan.delete(document) }
                            } label: {
                                Image(systemName: "ellipsis.circle")
                            }
                            .menuStyle(.borderlessButton).menuIndicator(.hidden).fixedSize()
                        } else {
                            if document.status != "failed" { ProgressView(value: document.progress).frame(width: 120) }
                            Button(role: .destructive) {
                                scan.delete(document)
                            } label: {
                                Image(systemName: "trash")
                            }
                            .buttonStyle(.borderless).help(String(localized: "Delete this document"))
                        }
                    }
                    if openDocument == document.id {
                        ScrollView {
                            Text(scan.text(of: document))
                                .font(.body).textSelection(.enabled)
                                .frame(maxWidth: .infinity, alignment: .leading)
                                .padding(12)
                        }
                        .frame(maxHeight: 280)
                        .background(Color(nsColor: .textBackgroundColor), in: RoundedRectangle(cornerRadius: 8, style: .continuous))
                        .overlay(RoundedRectangle(cornerRadius: 8, style: .continuous).strokeBorder(.quaternary))
                    }
                }
                if document.id != scan.documents.last?.id { Divider() }
            }
        }
    }

    private func icon(for document: ScannedDocument) -> String {
        switch document.status {
        case "done": return "doc.text"
        case "failed": return "exclamationmark.triangle"
        default: return "doc.text.magnifyingglass"
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

/// Shows the six-digit code a phone or tablet must enter, and what came of it.
private struct PairingSheet: View {
    @ObservedObject var scan: ScanModel

    var body: some View {
        VStack(spacing: 18) {
            if let name = scan.justPaired {
                Image(systemName: "checkmark.circle.fill").font(.system(size: 54)).foregroundStyle(.green)
                Text("\(name) is connected").font(.title2.bold())
                Text("It can send scans to this Mac whenever both are on the same Wi-Fi network.")
                    .multilineTextAlignment(.center).foregroundStyle(.secondary)
                Button("Done") { scan.endPairing() }.keyboardShortcut(.defaultAction).controlSize(.large)
            } else if let code = scan.pairingCode {
                Text("Connect a phone or tablet").font(.title2.bold())
                VStack(alignment: .leading, spacing: 8) {
                    Label("Open God's Camera on the phone or tablet, on the same Wi-Fi network as this Mac.", systemImage: "1.circle.fill")
                    Label("Tap Connect to a Mac and choose “\(scan.networkName)”.", systemImage: "2.circle.fill")
                    Label("Enter this code there.", systemImage: "3.circle.fill")
                }
                .font(.callout)
                .frame(maxWidth: .infinity, alignment: .leading)

                HStack(spacing: 8) {
                    ForEach(Array(digits(code).enumerated()), id: \.offset) { index, digit in
                        Text(String(digit))
                            .font(.system(size: 38, weight: .semibold, design: .rounded)).monospacedDigit()
                            .frame(width: 50, height: 62)
                            .background(Color(nsColor: .controlBackgroundColor), in: RoundedRectangle(cornerRadius: 10, style: .continuous))
                            .overlay(RoundedRectangle(cornerRadius: 10, style: .continuous).strokeBorder(.quaternary))
                            .padding(.trailing, index == 2 ? 12 : 0)
                    }
                }
                .accessibilityElement(children: .ignore)
                .accessibilityLabel(Text(code))

                Text("The code works for \(timeLeft) more and for one device.")
                    .font(.callout).monospacedDigit().foregroundStyle(.secondary)
                Button("Cancel", role: .cancel) { scan.endPairing() }
            } else {
                Image(systemName: "clock.badge.exclamationmark").font(.system(size: 44)).foregroundStyle(.orange)
                Text("This code no longer works").font(.title2.bold())
                Text("It ran out, or a wrong code was entered too many times.")
                    .multilineTextAlignment(.center).foregroundStyle(.secondary)
                HStack {
                    Button("Cancel", role: .cancel) { scan.endPairing() }
                    Button("Show a new code") { scan.beginPairing() }.keyboardShortcut(.defaultAction)
                }
            }
        }
        .padding(26)
        .frame(width: 470)
    }

    private func digits(_ code: String) -> [Character] { code.filter(\.isNumber) }

    private var timeLeft: String {
        let seconds = max(0, scan.pairingSecondsLeft)
        return String(format: "%d:%02d", seconds / 60, seconds % 60)
    }
}
