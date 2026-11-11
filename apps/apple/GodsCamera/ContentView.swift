import AVFoundation
import GodsCameraKit
import SwiftUI

/// Shows the engine's output inside a SwiftUI layout.
struct PreviewView: NSViewRepresentable {
    let layer: CALayer

    func makeNSView(context: Context) -> NSView {
        let view = NSView()
        view.wantsLayer = true
        view.layer = CALayer()
        view.layer?.backgroundColor = NSColor.black.cgColor
        layer.frame = view.bounds
        layer.autoresizingMask = [.layerWidthSizable, .layerHeightSizable]
        view.layer?.addSublayer(layer)
        return view
    }

    func updateNSView(_ view: NSView, context: Context) {
        layer.frame = view.bounds
    }
}

struct ContentView: View {
    @EnvironmentObject private var model: AppModel
    @State private var clipName = ""
    @State private var confirmingDelete = false

    var body: some View {
        HStack(alignment: .top, spacing: 0) {
            VStack(alignment: .leading, spacing: 10) {
                preview
                transport
                if let message = model.message {
                    Label(message, systemImage: "exclamationmark.triangle").foregroundStyle(.orange)
                }
                Spacer(minLength: 0)
            }
            .padding(16)
            .frame(minWidth: 560)

            Divider()

            ScrollView {
                VStack(alignment: .leading, spacing: 18) {
                    virtualCameraSection
                    sourceSection
                    FaceSection(face: model.face, status: model.status)
                    ScannerSection(scan: model.scan)
                    loopSection
                    savedLoopsSection
                    Button("Save a diagnostics report…") { model.exportDiagnostics() }
                        .buttonStyle(.link)
                    Button("Delete all my data…", role: .destructive) { confirmingDelete = true }
                        .buttonStyle(.link)
                        .confirmationDialog(
                            String(localized: "Delete every saved face and loop?"), isPresented: $confirmingDelete
                        ) {
                            Button("Delete everything", role: .destructive) { model.deleteAllData() }
                        } message: {
                            Text("Saved faces, their encryption key, saved loops, scanned documents and the list of blocked apps are removed from this Mac. This cannot be undone.")
                        }
                }
                .padding(16)
            }
            .frame(width: 320)
        }
        .frame(minHeight: 520)
    }

    // MARK: Preview and transport

    private var preview: some View {
        PreviewView(layer: model.previewLayer)
            .aspectRatio(16 / 9, contentMode: .fit)
            .clipShape(RoundedRectangle(cornerRadius: 8))
            .overlay(alignment: .topLeading) {
                HStack(spacing: 6) {
                    switch model.status.mode {
                    case .looping: Badge(text: String(localized: "LOOPING"), color: .red)
                    case .live: Badge(text: String(localized: "LIVE"), color: .green)
                        if model.status.swapping { Badge(text: String(localized: "AI FACE"), color: .purple) }
                    case .idle: Badge(text: String(localized: "NO PICTURE"), color: .gray)
                    }
                    if model.status.isRecording {
                        Badge(text: String(localized: "REC \(Int(model.status.recordedSeconds)) s"), color: .orange)
                    }
                }
                .padding(8)
            }
    }

    private var transport: some View {
        HStack(spacing: 10) {
            Button {
                model.toggleRecording()
            } label: {
                Label(
                    model.status.isRecording ? String(localized: "Stop recording") : String(localized: "Record a loop"),
                    systemImage: model.status.isRecording ? "stop.circle.fill" : "record.circle")
            }
            .disabled(!model.canRecord)
            .help(String(localized: "Control-Option-Command-R"))

            Button {
                model.toggleLoop()
            } label: {
                Label(
                    model.status.mode == .looping ? String(localized: "Back to live") : String(localized: "Play the loop"),
                    systemImage: model.status.mode == .looping ? "video.fill" : "repeat")
            }
            .disabled(!model.canLoop)
            .keyboardShortcut(.space, modifiers: [])
            .help(String(localized: "Control-Option-Command-L"))

            Spacer()
            if model.status.framesOut > 0 {
                Text("\(model.status.sourceFps, specifier: "%.0f") fps · render \(model.status.renderMs, specifier: "%.1f") ms")
                    .font(.caption).monospacedDigit().foregroundStyle(.secondary)
            }
        }
        .controlSize(.large)
    }

    // MARK: Sections

    private var virtualCameraSection: some View {
        Section(title: String(localized: "Virtual camera")) {
            switch model.extensionManager.state {
            case .installed:
                if model.cameraNeedsRestart {
                    Label(
                        String(localized: "The camera is installed, but macOS has not started it. Restart the Mac to finish."),
                        systemImage: "power"
                    )
                    .foregroundStyle(.orange)
                } else {
                    Label(String(localized: "Installed. Choose “God's Camera” in your meeting app."), systemImage: "checkmark.circle.fill")
                        .foregroundStyle(.green)
                }
            case .outdated:
                Label(String(localized: "An older version is installed."), systemImage: "arrow.triangle.2.circlepath")
                Button("Update the camera") { model.extensionManager.activate() }
            case .notInstalled, .unknown:
                Text("Other apps cannot see God's Camera until it is installed.").foregroundStyle(.secondary)
                Button("Install the camera") { model.extensionManager.activate() }
            case .waitingForApproval:
                Label(
                    String(localized: "Waiting for your approval in System Settings › General › Login Items & Extensions › Camera Extensions."),
                    systemImage: "hourglass")
                HStack {
                    Button("Open System Settings") {
                        if let url = URL(string: "x-apple.systempreferences:com.apple.LoginItems-Settings.extension") {
                            NSWorkspace.shared.open(url)
                        }
                    }
                    Button("Check again") { model.extensionManager.refresh() }
                }
            case .needsRestart:
                Label(String(localized: "Restart the Mac to finish."), systemImage: "power")
            case .failed(let reason):
                Label(reason, systemImage: "xmark.octagon").foregroundStyle(.red)
                Button("Try again") { model.extensionManager.activate() }
            }
            if let stats = model.cameraStats {
                Text(
                    "\(stats.clients) app(s) reading · \(stats.latencyMs, specifier: "%.1f") ms from capture to camera"
                )
                .font(.caption).monospacedDigit().foregroundStyle(.secondary)
                ForEach(model.readers) { reader in
                    HStack {
                        Text(reader.app.name).lineLimit(1).truncationMode(.middle)
                        Spacer()
                        if !model.blockedApps.contains(reader.app.id) {
                            Button("Block") { model.block(reader.app.id) }
                                .buttonStyle(.borderless)
                                .help(String(localized: "The app gets no picture from now on."))
                        }
                    }
                    .font(.callout)
                }
            }
            if model.withholding {
                Label(
                    String(localized: "A blocked app is reading the camera, so no picture is sent to any app."),
                    systemImage: "hand.raised.fill"
                )
                .font(.caption).foregroundStyle(.orange)
            }
            if !model.blockedApps.isEmpty {
                Text("Blocked apps").font(.caption).foregroundStyle(.secondary)
                ForEach(model.blockedApps, id: \.self) { appID in
                    HStack {
                        Text(model.displayName(forBlocked: appID)).lineLimit(1).truncationMode(.middle)
                        Spacer()
                        Button("Allow") { model.unblock(appID) }.buttonStyle(.borderless)
                    }
                    .font(.callout)
                }
            }
            if case .installed = model.extensionManager.state {
                Button("Remove the camera", role: .destructive) { model.extensionManager.deactivate() }
                    .buttonStyle(.link)
            }
        }
    }

    private var sourceSection: some View {
        Section(title: String(localized: "Picture")) {
            Picker(String(localized: "Source"), selection: $model.sourceKind) {
                Text("Camera").tag(SourceKind.camera)
                Text("Image").tag(SourceKind.image)
                Text("Video").tag(SourceKind.video)
                Text("Test pattern").tag(SourceKind.pattern)
            }
            .onChange(of: model.sourceKind) { model.sourceChanged() }

            switch model.sourceKind {
            case .camera:
                if !model.cameraAuthorized {
                    Text("God's Camera needs your permission to use the camera.").foregroundStyle(.secondary)
                    Button("Allow camera access") { model.requestCameraAccess() }
                } else {
                    Picker(String(localized: "Camera"), selection: $model.cameraID) {
                        ForEach(model.cameras) { camera in Text(camera.name).tag(Optional(camera.id)) }
                    }
                    .onChange(of: model.cameraID) { model.sourceChanged() }
                }
            case .image, .video:
                HStack {
                    Text(model.fileURL?.lastPathComponent ?? String(localized: "No file chosen"))
                        .lineLimit(1).truncationMode(.middle).foregroundStyle(.secondary)
                    Spacer()
                    Button("Choose…") { model.chooseFile() }
                }
            case .pattern:
                Text("Colour bars with a moving block, for checking the camera in other apps.")
                    .foregroundStyle(.secondary)
            }
            Toggle("Mirror the picture", isOn: $model.mirror)
                .onChange(of: model.mirror) { model.applySettings() }
        }
    }

    private var loopSection: some View {
        Section(title: String(localized: "Loop")) {
            if let clip = model.currentClip {
                Text("\(clip.name) · \(clip.duration, specifier: "%.1f") s, loops \(Double(clip.loop.length) / clip.frameRate, specifier: "%.1f") s")
                    .font(.callout)
                if model.lightingDiffers {
                    Label(
                        String(localized: "The room looks brighter or darker than in this loop."),
                        systemImage: "sun.max.trianglebadge.exclamationmark"
                    )
                    .font(.caption).foregroundStyle(.orange)
                }
                if !model.currentClipIsSaved {
                    HStack {
                        TextField(String(localized: "Name"), text: $clipName)
                        Button("Save") {
                            model.saveCurrentClip(named: clipName)
                            clipName = ""
                        }
                    }
                }
            } else {
                Text("Record a few seconds of yourself, then play them as your camera picture.")
                    .foregroundStyle(.secondary)
            }
            Picker(String(localized: "Repeat"), selection: $model.loopMode) {
                Text("Start to end").tag(LoopMode.forward)
                Text("Back and forth").tag(LoopMode.pingPong)
            }
            .onChange(of: model.loopMode) { model.applySettings() }
            Toggle("Show a LOOP badge to others", isOn: $model.showLoopBadge)
                .onChange(of: model.showLoopBadge) { model.applySettings() }
            Toggle("Go back to live when I start to speak", isOn: $model.returnOnVoice)
                .onChange(of: model.returnOnVoice) { model.applySettings() }
                .help(String(localized: "Uses the microphone only while a loop plays. Nothing is recorded."))
        }
    }

    @ViewBuilder private var savedLoopsSection: some View {
        if !model.savedClips.isEmpty {
            Section(title: String(localized: "Saved loops")) {
                ForEach(model.savedClips) { clip in
                    HStack {
                        Button {
                            model.selectSavedClip(clip.id)
                        } label: {
                            VStack(alignment: .leading) {
                                Text(clip.name)
                                Text("\(clip.duration, specifier: "%.1f") s").font(.caption).foregroundStyle(.secondary)
                            }
                        }
                        .buttonStyle(.plain)
                        Spacer()
                        if model.currentClip?.id == clip.id {
                            Image(systemName: "checkmark").foregroundStyle(.secondary)
                        }
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

private struct Section<Content: View>: View {
    let title: String
    @ViewBuilder let content: Content

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            Text(title).font(.headline)
            content
        }
    }
}

private struct Badge: View {
    let text: String
    let color: Color

    var body: some View {
        Text(text)
            .font(.caption.bold()).monospacedDigit()
            .padding(.horizontal, 8).padding(.vertical, 3)
            .background(color.opacity(0.9), in: Capsule())
            .foregroundStyle(.white)
    }
}
