import GodsCameraKit
import SwiftUI

/// The window: a sidebar of screens, and the chosen screen beside it.
struct RootView: View {
    @EnvironmentObject private var model: AppModel

    var body: some View {
        Group {
            if Snapshots.requestedFolder == nil {
                NavigationSplitView {
                    sidebar.navigationSplitViewColumnWidth(min: 176, ideal: 196, max: 240)
                } detail: {
                    detail
                }
            } else {
                // A picture of the window cannot show the sidebar's see-through
                // material, so for pictures the same list stands on a plain background.
                HStack(spacing: 0) {
                    sidebar.listStyle(.plain).frame(width: 196)
                    Divider()
                    detail
                }
            }
        }
        .frame(minWidth: 1040, minHeight: 640)
    }

    private var sidebar: some View {
        List(selection: Binding<Screen?>(get: { model.screen }, set: { model.screen = $0 ?? model.screen })) {
            ForEach(Screen.allCases) { screen in
                Label(screen.title, systemImage: screen.symbol)
                    .font(.title3)
                    .padding(.vertical, 3)
                    .tag(screen)
            }
        }
        .safeAreaInset(edge: .bottom) { SidebarStatus().padding(12) }
    }

    private var detail: some View {
        Group {
            switch model.screen {
            case .camera: CameraScreen(face: model.face)
            case .scanner: ScannerScreen(scan: model.scan)
            case .settings: SettingsScreen()
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
        .background(Color(nsColor: .windowBackgroundColor))
    }
}

/// What the camera is doing, always in view at the foot of the sidebar.
private struct SidebarStatus: View {
    @EnvironmentObject private var model: AppModel

    var body: some View {
        VStack(alignment: .leading, spacing: 5) {
            HStack(spacing: 7) {
                Circle().fill(color).frame(width: 9, height: 9)
                Text(state).font(.callout.weight(.semibold))
            }
            Text(watchers).font(.caption).foregroundStyle(.secondary)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(10)
        .background(.quaternary.opacity(0.5), in: RoundedRectangle(cornerRadius: 10, style: .continuous))
    }

    private var color: Color {
        switch model.status.mode {
        case .live: return .green
        case .looping: return .red
        case .idle: return .gray
        }
    }

    private var state: String {
        switch model.status.mode {
        case .live: return model.status.swapping ? String(localized: "Live, face swapped") : String(localized: "Live")
        case .looping: return String(localized: "Playing a loop")
        case .idle: return String(localized: "Camera off")
        }
    }

    private var watchers: String {
        let count = model.cameraStats?.clients ?? 0
        if count == 0 { return String(localized: "No app is using the camera") }
        let names = model.readers.map(\.app.name)
        if names.isEmpty { return String(localized: "\(count) app(s) using the camera") }
        return String(localized: "Used by \(names.joined(separator: ", "))")
    }
}

// MARK: Settings

/// The camera's installation, the data the app keeps, and help.
struct SettingsScreen: View {
    @EnvironmentObject private var model: AppModel
    @State private var confirmingDelete = false

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 16) {
                ScreenHeader(screen: .settings)

                Card(String(localized: "The camera"), symbol: "video") {
                    LabeledContent(String(localized: "State")) { Text(cameraState) }
                    LabeledContent(String(localized: "Version")) {
                        Text("\(appVersion) · \(String(localized: "camera build")) \(ExtensionManager.bundledBuild)")
                    }
                    if case .installed = model.extensionManager.state {
                        Divider()
                        Button("Remove the camera…", role: .destructive) { model.extensionManager.deactivate() }
                        Hint(String(localized: "Other apps no longer list God's Camera. macOS asks for an administrator's password."))
                    }
                }

                Card(String(localized: "Your data"), symbol: "lock.shield") {
                    Hint(String(localized: "Saved faces, loops and scanned documents stay on this Mac. Saved faces are encrypted, with the key in your Keychain."))
                    Button("Delete all my data…", role: .destructive) { confirmingDelete = true }
                        .confirmationDialog(
                            String(localized: "Delete every saved face and loop?"), isPresented: $confirmingDelete
                        ) {
                            Button("Delete everything", role: .destructive) { model.deleteAllData() }
                        } message: {
                            Text("Saved faces, their encryption key, saved loops, scanned documents, connected devices and the list of blocked apps are removed from this Mac. This cannot be undone.")
                        }
                }

                Card(String(localized: "Help"), symbol: "lifepreserver") {
                    Hint(String(localized: "A diagnostics report holds the app's state and its log. It contains no pictures and no faces."))
                    Button("Save a diagnostics report…") { model.exportDiagnostics() }
                    Divider()
                    LabeledContent(String(localized: "Record or stop a loop")) { Text("⌃⌥⌘R").monospaced() }
                    LabeledContent(String(localized: "Play the loop or go back to live")) { Text("⌃⌥⌘L").monospaced() }
                }
            }
            .padding(24)
            .frame(maxWidth: 720, alignment: .leading)
        }
    }

    private var appVersion: String {
        Bundle.main.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String ?? "?"
    }

    private var cameraState: String {
        switch model.extensionManager.state {
        case .installed: return model.cameraNeedsRestart ? String(localized: "Installed, waiting for a restart") : String(localized: "Installed")
        case .outdated: return String(localized: "An older version is installed")
        case .notInstalled, .unknown: return String(localized: "Not installed")
        case .waitingForApproval: return String(localized: "Waiting for your approval")
        case .needsRestart: return String(localized: "Restart the Mac to finish.")
        case .failed(let reason): return reason
        }
    }
}
