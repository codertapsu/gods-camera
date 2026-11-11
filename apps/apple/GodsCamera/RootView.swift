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
            case .camera: CameraScreen()
            case .loop: LoopScreen()
            case .face: FaceScreen(face: model.face)
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

// MARK: Camera

/// The camera as other apps see it: whether they can, which of them do, and where the picture comes from.
struct CameraScreen: View {
    @EnvironmentObject private var model: AppModel

    var body: some View {
        StudioLayout(screen: .camera) {
            picture
            if let message = model.message, model.status.mode != .idle {
                Hint(message, symbol: "exclamationmark.triangle", tone: .orange)
            }
        } controls: {
            meetingApps
            readers
        }
    }

    private var meetingApps: some View {
        Card(String(localized: "In your meeting app"), symbol: "person.2.wave.2") {
            switch model.extensionManager.state {
            case .installed:
                if model.cameraNeedsRestart {
                    Hint(
                        String(localized: "The camera is installed, but macOS has not started it. Restart the Mac to finish."),
                        symbol: "power", tone: .orange)
                } else {
                    Hint(
                        String(localized: "Ready. In Zoom, Teams, Meet or any other app, choose “God's Camera” as the camera."),
                        symbol: "checkmark.circle.fill", tone: .green)
                    if !model.appsToReopen.isEmpty {
                        VStack(alignment: .leading, spacing: 8) {
                            Hint(
                                String(localized: "These apps were opened before the camera was last installed or updated, so they do not list it yet. Quit them completely and open them again."),
                                symbol: "arrow.clockwise.circle.fill", tone: .orange)
                            ForEach(model.appsToReopen) { app in
                                HStack(spacing: 8) {
                                    Image(nsImage: appIcon(forAppID: app.id)).resizable().frame(width: 22, height: 22)
                                    Text(app.name).lineLimit(1)
                                    Spacer()
                                    Button("Quit") { model.quit(app) }
                                        .help(String(localized: "Quits the app, as its own Quit command does. Open it again afterwards."))
                                }
                            }
                        }
                    } else {
                        Hint(String(localized: "An app looks for cameras when it starts. If one does not list God's Camera, quit that app completely and open it again."))
                    }
                }
            case .outdated:
                Hint(String(localized: "An older version of the camera is installed."), symbol: "arrow.triangle.2.circlepath")
                Button("Update the camera") { model.extensionManager.activate() }.controlSize(.large)
            case .notInstalled, .unknown:
                Hint(String(localized: "Other apps cannot see God's Camera until it is installed."))
                Button("Install the camera") { model.extensionManager.activate() }
                    .controlSize(.large).buttonStyle(.borderedProminent)
            case .waitingForApproval:
                Hint(
                    String(localized: "Waiting for your approval in System Settings › General › Login Items & Extensions › Camera Extensions."),
                    symbol: "hourglass")
                HStack {
                    Button("Open System Settings") {
                        if let url = URL(string: "x-apple.systempreferences:com.apple.LoginItems-Settings.extension") {
                            NSWorkspace.shared.open(url)
                        }
                    }
                    Button("Check again") { model.extensionManager.refresh() }
                }
            case .needsRestart:
                Hint(String(localized: "Restart the Mac to finish."), symbol: "power")
            case .failed(let reason):
                Hint(reason, symbol: "xmark.octagon", tone: .red)
                Button("Try again") { model.extensionManager.activate() }
            }
        }
    }

    private var readers: some View {
        Card(String(localized: "Apps using the camera"), symbol: "app.badge.checkmark") {
            if model.readers.isEmpty {
                Hint(String(localized: "No app is using the camera right now."))
            }
            ForEach(model.readers) { reader in
                HStack(spacing: 8) {
                    Image(nsImage: appIcon(forAppID: reader.app.id)).resizable().frame(width: 22, height: 22)
                    Text(reader.app.name).lineLimit(1).truncationMode(.middle)
                    Spacer()
                    if !model.blockedApps.contains(reader.app.id) {
                        Button("Block") { model.block(reader.app.id) }
                            .help(String(localized: "The app gets no picture from now on."))
                    }
                }
            }
            if model.withholding {
                Hint(
                    String(localized: "A blocked app is reading the camera, so no picture is sent to any app."),
                    symbol: "hand.raised.fill", tone: .orange)
            }
            if !model.blockedApps.isEmpty {
                Divider()
                Text("Blocked apps").font(.subheadline).foregroundStyle(.secondary)
                ForEach(model.blockedApps, id: \.self) { appID in
                    HStack(spacing: 8) {
                        Image(nsImage: appIcon(forAppID: appID)).resizable().frame(width: 22, height: 22)
                        Text(model.displayName(forBlocked: appID)).lineLimit(1).truncationMode(.middle)
                        Spacer()
                        Button("Allow") { model.unblock(appID) }
                    }
                }
            }
            if let stats = model.cameraStats, stats.clients > 0 {
                Text("\(stats.latencyMs, specifier: "%.1f") ms from capture to camera")
                    .font(.caption).monospacedDigit().foregroundStyle(.tertiary)
            }
        }
    }

    /// Where the picture comes from. It sits under the picture it controls.
    private var picture: some View {
        VStack(alignment: .leading, spacing: 12) {
            Picker(String(localized: "Picture from"), selection: $model.sourceKind) {
                Text("Camera").tag(SourceKind.camera)
                Text("Image").tag(SourceKind.image)
                Text("Video").tag(SourceKind.video)
                Text("Test pattern").tag(SourceKind.pattern)
            }
            .pickerStyle(.segmented)
            .labelsHidden()
            .onChange(of: model.sourceKind) { model.sourceChanged() }

            HStack(spacing: 12) {
                switch model.sourceKind {
                case .camera:
                    if !model.cameraAuthorized {
                        Hint(String(localized: "God's Camera needs your permission to use the camera."))
                        Button("Allow camera access") { model.requestCameraAccess() }
                    } else {
                        Picker(String(localized: "Camera"), selection: $model.cameraID) {
                            ForEach(model.cameras) { camera in Text(camera.name).tag(Optional(camera.id)) }
                        }
                        .labelsHidden()
                        .fixedSize()
                        .onChange(of: model.cameraID) { model.sourceChanged() }
                    }
                case .image, .video:
                    Button("Choose…") { model.chooseFile() }
                    Text(model.fileURL?.lastPathComponent ?? String(localized: "No file chosen"))
                        .lineLimit(1).truncationMode(.middle).foregroundStyle(.secondary)
                case .pattern:
                    Hint(String(localized: "Colour bars with a moving block, for checking the camera in other apps."))
                }
                Spacer()
                Toggle("Mirror the picture", isOn: $model.mirror)
                    .onChange(of: model.mirror) { model.applySettings() }
            }
        }
    }
}

// MARK: Loop

/// Recording a loop, playing it, and the loops kept for later.
struct LoopScreen: View {
    @EnvironmentObject private var model: AppModel
    @State private var clipName = ""

    var body: some View {
        StudioLayout(screen: .loop) {
            HStack(spacing: 12) {
                Button {
                    model.toggleRecording()
                } label: {
                    Label(
                        model.status.isRecording ? String(localized: "Stop recording") : String(localized: "Record a loop"),
                        systemImage: model.status.isRecording ? "stop.circle.fill" : "record.circle")
                    .frame(minWidth: 150)
                }
                .tint(.red)
                .disabled(!model.canRecord)
                .help(String(localized: "Control-Option-Command-R"))

                Button {
                    model.toggleLoop()
                } label: {
                    Label(
                        model.status.mode == .looping ? String(localized: "Back to live") : String(localized: "Play the loop"),
                        systemImage: model.status.mode == .looping ? "video.fill" : "play.circle.fill")
                    .frame(minWidth: 150)
                }
                .buttonStyle(.borderedProminent)
                .disabled(!model.canLoop)
                .keyboardShortcut(.space, modifiers: [])
                .help(String(localized: "Control-Option-Command-L"))
                Spacer()
            }
            .controlSize(.extraLarge)
            Hint(String(localized: "Sit still for the first and the last second of a recording. The loop then ends as it began, and nobody sees it start again."), symbol: "lightbulb")
        } controls: {
            thisLoop
            repeating
            saved
        }
    }

    private var thisLoop: some View {
        Card(String(localized: "This loop"), symbol: "film") {
            if let clip = model.currentClip {
                VStack(alignment: .leading, spacing: 3) {
                    Text(clip.name).font(.body.weight(.medium))
                    Text("Recorded \(clip.duration, specifier: "%.1f") s · loops \(Double(clip.loop.length) / clip.frameRate, specifier: "%.1f") s")
                        .font(.callout).monospacedDigit().foregroundStyle(.secondary)
                }
                if model.lightingDiffers {
                    Hint(
                        String(localized: "The room looks brighter or darker than in this loop."),
                        symbol: "sun.max.trianglebadge.exclamationmark", tone: .orange)
                }
                if !model.currentClipIsSaved {
                    HStack {
                        TextField(String(localized: "Name"), text: $clipName)
                            .textFieldStyle(.roundedBorder)
                        Button("Save") {
                            model.saveCurrentClip(named: clipName)
                            clipName = ""
                        }
                    }
                }
            } else {
                Hint(String(localized: "No loop yet. Click Record a loop, wait a few seconds, and click Stop recording."))
            }
        }
    }

    private var repeating: some View {
        Card(String(localized: "How it repeats"), symbol: "arrow.triangle.2.circlepath") {
            Picker(String(localized: "Repeat"), selection: $model.loopStyle) {
                Text("Automatic").tag(LoopStyle.automatic)
                Text("Start to end").tag(LoopStyle.forward)
                Text("Back and forth").tag(LoopStyle.pingPong)
            }
            .onChange(of: model.loopStyle) { model.applySettings() }
            Hint(repeatExplanation)
            Toggle("Show a LOOP badge to others", isOn: $model.showLoopBadge)
                .onChange(of: model.showLoopBadge) { model.applySettings() }
            Toggle("Go back to live when I start to speak", isOn: $model.returnOnVoice)
                .onChange(of: model.returnOnVoice) { model.applySettings() }
                .help(String(localized: "Uses the microphone only while a loop plays. Nothing is recorded."))
        }
    }

    private var repeatExplanation: String {
        switch model.loopStyle {
        case .automatic:
            guard let clip = model.currentClip else {
                return String(localized: "Start to end when a loop ends as it began. Otherwise back and forth, which has no jump.")
            }
            return clip.suggestedMode == .forward
                ? String(localized: "This loop ends as it began, so it plays start to end.")
                : String(localized: "You moved between the start and the end of this loop, so it plays back and forth, without a jump.")
        case .forward: return String(localized: "Plays to the end, then fades back to the start.")
        case .pingPong: return String(localized: "Plays to the end, then backwards to the start. There is no jump, but movement runs in reverse half the time.")
        }
    }

    @ViewBuilder private var saved: some View {
        if !model.savedClips.isEmpty {
            Card(String(localized: "Saved loops"), symbol: "tray.full") {
                ForEach(model.savedClips) { clip in
                    HStack {
                        Button {
                            model.selectSavedClip(clip.id)
                        } label: {
                            HStack {
                                Image(systemName: model.currentClip?.id == clip.id ? "checkmark.circle.fill" : "circle")
                                    .foregroundStyle(model.currentClip?.id == clip.id ? AnyShapeStyle(.tint) : AnyShapeStyle(.tertiary))
                                VStack(alignment: .leading) {
                                    Text(clip.name)
                                    Text("\(clip.duration, specifier: "%.1f") s").font(.caption).foregroundStyle(.secondary)
                                }
                                Spacer()
                            }
                            .contentShape(Rectangle())
                        }
                        .buttonStyle(.plain)
                        Button(role: .destructive) {
                            model.deleteSavedClip(clip.id)
                        } label: {
                            Image(systemName: "trash")
                        }
                        .buttonStyle(.borderless)
                        .help(String(localized: "Delete this loop"))
                    }
                }
            }
        }
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
