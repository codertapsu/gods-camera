import AppKit
import GodsCameraKit
import SwiftUI

/// The camera, the loop and the face swap on one screen, so that they are
/// used together (plan, section 6.12): the picture others see, the controls
/// used during a call under it, and everything else in three tabs at the right.
struct CameraScreen: View {
    @EnvironmentObject private var model: AppModel
    @ObservedObject var face: FaceModel

    var body: some View {
        HStack(alignment: .top, spacing: 0) {
            VStack(alignment: .leading, spacing: 16) {
                header
                PreviewPane()
                    .overlay(alignment: .bottomLeading) { PictureControls().padding(10) }
                CallControls(face: face)
                statusLine
                Spacer(minLength: 0)
            }
            .padding(24)
            .frame(minWidth: 500, maxWidth: .infinity, alignment: .topLeading)

            Divider()

            VStack(spacing: 0) {
                Picker(String(localized: "Show"), selection: $model.cameraTab) {
                    ForEach(CameraTab.allCases) { tab in
                        Label(tab.title, systemImage: tab.symbol).tag(tab)
                    }
                }
                .pickerStyle(.segmented)
                .labelsHidden()
                .padding(.horizontal, 16).padding(.top, 14).padding(.bottom, 4)

                ScrollView {
                    VStack(alignment: .leading, spacing: 14) {
                        switch model.cameraTab {
                        case .face: FacePanel(face: face)
                        case .loop: LoopPanel()
                        case .source: SourcePanel()
                        }
                    }
                    .padding(16)
                }
            }
            .frame(width: 360)
        }
        // The sheets belong to the screen, not to a tab: the swap can be
        // turned on, and a face added, from the controls under the picture.
        .modifier(FaceSheets(face: face))
    }

    private var header: some View {
        HStack(alignment: .top) {
            VStack(alignment: .leading, spacing: 4) {
                Text(Screen.camera.title).font(.largeTitle.bold())
                Text(Screen.camera.subtitle).font(.body).foregroundStyle(.secondary)
            }
            Spacer()
            Button {
                model.cameraTab = .source
            } label: {
                HStack(spacing: 6) {
                    Circle().fill(meetingColour).frame(width: 8, height: 8)
                    Text(meetingWords).lineLimit(1)
                }
                .font(.callout)
                .padding(.horizontal, 11).padding(.vertical, 5)
                .background(.quaternary.opacity(0.6), in: Capsule())
            }
            .buttonStyle(.plain)
            .help(String(localized: "In your meeting app"))
        }
    }

    private var meetingWords: String {
        switch model.extensionManager.state {
        case .installed:
            let count = model.cameraStats?.clients ?? 0
            return count == 0 ? String(localized: "No app is using the camera") : String(localized: "An app is using the camera")
        case .outdated: return String(localized: "An older version is installed")
        case .notInstalled, .unknown: return String(localized: "Not installed")
        case .waitingForApproval: return String(localized: "Waiting for your approval")
        case .needsRestart: return String(localized: "Restart the Mac to finish.")
        case .failed: return String(localized: "Not installed")
        }
    }

    private var meetingColour: Color {
        switch model.extensionManager.state {
        case .installed: return (model.cameraStats?.clients ?? 0) > 0 ? .green : .gray
        case .failed: return .red
        default: return .orange
        }
    }

    /// The one line under the controls: what matters most right now.
    @ViewBuilder private var statusLine: some View {
        if model.status.isRecording {
            Hint(String(localized: "Recording. Sit still for the last second, then click Stop recording."), symbol: "record.circle")
        } else if face.wanted || face.state != .off {
            switch face.state {
            case .off:
                Hint(String(localized: "The swap is off. Others see your own face."))
            case .loading:
                HStack(spacing: 8) {
                    ProgressView().controlSize(.small)
                    Hint(String(localized: "Getting the models ready. The first time takes a little longer."))
                }
            case .on:
                if let error = model.status.swapError {
                    Hint(error, symbol: "xmark.octagon", tone: .red)
                } else if model.status.mode == .looping {
                    Hint(String(localized: "Playing the loop. The swap keeps following your face, so going back to live does not jump."), symbol: "repeat")
                } else if model.status.swapping {
                    Hint(
                        String(localized: "Swapping. A new face picture every \(Int(model.status.swapMs.rounded())) ms, \(Int(model.status.swapAgeMs.rounded())) ms behind the camera."),
                        symbol: "checkmark.circle.fill", tone: .green)
                } else if model.status.pictureHidden {
                    Hint(String(localized: "The swap is not on your face, so the picture is blurred."), symbol: "eye.slash", tone: .orange)
                } else if model.status.faceFound {
                    Hint(String(localized: "Getting the first swapped face…"))
                } else {
                    Hint(String(localized: "No face in the picture. Others see the picture as it is."), symbol: "person.crop.circle.badge.questionmark")
                }
            case .missing(let file):
                Hint(String(localized: "A model file is not installed: \(file)"), symbol: "exclamationmark.triangle", tone: .orange)
            case .failed(let reason):
                Hint(reason, symbol: "xmark.octagon", tone: .red)
            }
        } else if model.status.mode == .looping {
            Hint(String(localized: "Playing the loop. Others see the recording until you go back to live."), symbol: "repeat")
        } else if let message = model.message {
            Hint(message, symbol: "exclamationmark.triangle", tone: .orange)
        } else if face.faces.isEmpty {
            Hint(String(localized: "Add a face first: click Add a face and choose photos of the person."), symbol: "arrow.right.circle")
        } else {
            Hint(String(localized: "The swap is off. Others see your own face."))
        }
    }
}

/// The controls used during a call: the swap with the chosen face, recording
/// a loop, and playing it.
private struct CallControls: View {
    @EnvironmentObject private var model: AppModel
    @ObservedObject var face: FaceModel

    var body: some View {
        HStack(spacing: 10) {
            HStack(spacing: 2) {
                Toggle(isOn: Binding(get: { face.wanted }, set: { face.setWanted($0) })) {
                    HStack(spacing: 8) {
                        if let chosen = face.faces.first(where: { $0.id == face.selectedID }) {
                            DataImage(data: chosen.thumbnail, side: 22, round: true)
                        } else {
                            Image(systemName: "person.crop.circle")
                        }
                        Text("Face swap").fontWeight(.semibold)
                    }
                }
                .toggleStyle(.button)
                .help(String(localized: "Show another face in place of yours, live."))

                Menu {
                    ForEach(face.faces) { saved in
                        Button {
                            face.select(saved.id)
                        } label: {
                            if saved.id == face.selectedID {
                                Label(saved.name, systemImage: "checkmark")
                            } else {
                                Text(saved.name)
                            }
                        }
                    }
                    if !face.faces.isEmpty { Divider() }
                    Button("Add a face…") {
                        model.cameraTab = .face
                        face.choosePhotos()
                    }
                    Button("Face options…") { model.cameraTab = .face }
                } label: {
                    Image(systemName: "chevron.down")
                }
                .menuStyle(.borderlessButton)
                .menuIndicator(.hidden)
                .fixedSize()
                .help(String(localized: "Faces"))
            }

            recordButton
                .disabled(!model.canRecord && !model.status.isRecording)
                .help(String(localized: "Record or stop a loop") + "  (⌃⌥⌘R)")

            Button {
                model.toggleLoop()
            } label: {
                Label(
                    model.status.mode == .looping ? String(localized: "Back to live") : String(localized: "Play the loop"),
                    systemImage: model.status.mode == .looping ? "video.fill" : "play.circle.fill")
            }
            .buttonStyle(.borderedProminent)
            .disabled(!model.canLoop)
            .keyboardShortcut(.space, modifiers: [])
            .help(String(localized: "Play the loop or go back to live") + "  (⌃⌥⌘L)")

            Spacer(minLength: 0)
        }
        .controlSize(.large)
        .padding(10)
        .background(Color(nsColor: .controlBackgroundColor), in: RoundedRectangle(cornerRadius: 14, style: .continuous))
        .overlay(RoundedRectangle(cornerRadius: 14, style: .continuous).strokeBorder(.quaternary))
    }
}

extension CallControls {
    /// Red while it records: it is the one thing that should not be left on by mistake.
    @ViewBuilder var recordButton: some View {
        let button = Button {
            model.toggleRecording()
        } label: {
            Label(
                model.status.isRecording ? String(localized: "Stop recording") : String(localized: "Record a loop"),
                systemImage: model.status.isRecording ? "stop.circle.fill" : "record.circle")
        }
        if model.status.isRecording {
            button.buttonStyle(.borderedProminent).tint(.red)
        } else {
            button.buttonStyle(.bordered)
        }
    }
}

/// Where the picture comes from, and the mirror, laid on the picture at its
/// lower left: they change the picture, and the row under it keeps the call's controls.
private struct PictureControls: View {
    @EnvironmentObject private var model: AppModel

    var body: some View {
        HStack(spacing: 6) {
            Menu {
                if model.cameraAuthorized {
                    ForEach(model.cameras) { camera in
                        Button {
                            model.sourceKind = .camera
                            model.cameraID = camera.id
                            model.sourceChanged()
                        } label: {
                            if model.sourceKind == .camera && model.cameraID == camera.id {
                                Label(camera.name, systemImage: "checkmark")
                            } else {
                                Text(camera.name)
                            }
                        }
                    }
                } else {
                    Button("Allow camera access") {
                        model.sourceKind = .camera
                        model.sourceChanged()
                        model.requestCameraAccess()
                    }
                }
                Divider()
                Button("A picture…") {
                    model.sourceKind = .image
                    model.sourceChanged()
                    model.chooseFile()
                }
                Button("A video…") {
                    model.sourceKind = .video
                    model.sourceChanged()
                    model.chooseFile()
                }
                Button {
                    model.sourceKind = .pattern
                    model.sourceChanged()
                } label: {
                    if model.sourceKind == .pattern { Label("Test pattern", systemImage: "checkmark") } else { Text("Test pattern") }
                }
            } label: {
                Label(sourceName, systemImage: sourceSymbol)
            }
            .menuStyle(.borderlessButton)
            .fixedSize()
            .padding(.horizontal, 10).padding(.vertical, 5)
            .background(.black.opacity(0.55), in: Capsule())
            .help(String(localized: "Where the picture comes from"))

            Toggle(isOn: $model.mirror) {
                Image(systemName: "arrow.left.and.right.righttriangle.left.righttriangle.right")
            }
            .toggleStyle(.button)
            .onChange(of: model.mirror) { model.applySettings() }
            .help(String(localized: "Mirror the picture"))
        }
        .foregroundStyle(.white)
        .tint(.white)
        .colorScheme(.dark)
    }

    private var sourceName: String {
        switch model.sourceKind {
        case .camera: return model.cameras.first(where: { $0.id == model.cameraID })?.name ?? String(localized: "Camera")
        case .image: return model.fileURL?.lastPathComponent ?? String(localized: "Image")
        case .video: return model.fileURL?.lastPathComponent ?? String(localized: "Video")
        case .pattern: return String(localized: "Test pattern")
        }
    }

    private var sourceSymbol: String {
        switch model.sourceKind {
        case .camera: return "video"
        case .image: return "photo"
        case .video: return "film"
        case .pattern: return "square.grid.3x3.fill"
        }
    }
}

/// The Loop tab: the loop in hand, saving it, how it repeats, and the loops kept for later.
struct LoopPanel: View {
    @EnvironmentObject private var model: AppModel
    @State private var clipName = ""

    var body: some View {
        thisLoop
        repeating
        saved
        Hint(String(localized: "⌃⌥⌘R records, and ⌃⌥⌘L plays the loop, even while another app is in front."), symbol: "keyboard")
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
            Hint(String(localized: "Sit still for the first and the last second of a recording. The loop then ends as it began, and nobody sees it start again."), symbol: "lightbulb")
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
            Hint(String(localized: "Uses the microphone only while a loop plays. Nothing is recorded."))
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

/// The Camera tab: where the picture comes from, in detail, and the camera as
/// other apps see it: whether they can, and which of them do.
struct SourcePanel: View {
    @EnvironmentObject private var model: AppModel

    var body: some View {
        picture
        meetingApps
        readers
    }

    private var picture: some View {
        Card(String(localized: "Where the picture comes from"), symbol: "video") {
            Picker(String(localized: "Picture from"), selection: $model.sourceKind) {
                Text("Camera").tag(SourceKind.camera)
                Text("Image").tag(SourceKind.image)
                Text("Video").tag(SourceKind.video)
                Text("Test pattern").tag(SourceKind.pattern)
            }
            .pickerStyle(.segmented)
            .labelsHidden()
            .onChange(of: model.sourceKind) { model.sourceChanged() }

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
                    .onChange(of: model.cameraID) { model.sourceChanged() }
                }
            case .image, .video:
                HStack {
                    Button("Choose…") { model.chooseFile() }
                    Text(model.fileURL?.lastPathComponent ?? String(localized: "No file chosen"))
                        .lineLimit(1).truncationMode(.middle).foregroundStyle(.secondary)
                }
            case .pattern:
                Hint(String(localized: "Colour bars with a moving block, for checking the camera in other apps."))
            }
            Toggle("Mirror the picture", isOn: $model.mirror)
                .onChange(of: model.mirror) { model.applySettings() }
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
}
