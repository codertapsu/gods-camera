import AVFoundation
import AppKit
import Carbon.HIToolbox
import Combine
import GodsCameraKit
import SwiftUI

/// What feeds the camera.
enum SourceKind: String, CaseIterable, Identifiable {
    case camera, image, video, pattern
    var id: String { rawValue }
}

/// An app that is reading the virtual camera.
struct CameraReader: Identifiable, Equatable {
    let pid: Int32
    let app: AppIdentity
    var id: Int32 { pid }
}

/// The app's state and the actions the windows and menus call.
@MainActor
final class AppModel: ObservableObject {
    static let shared = AppModel()

    @Published var sourceKind: SourceKind
    @Published var cameras: [CameraDevice] = []
    @Published var cameraID: String?
    @Published var fileURL: URL?
    @Published var mirror: Bool
    @Published var showLoopBadge: Bool
    @Published var loopMode: LoopMode
    /// Leave the loop when the microphone hears the user speak.
    @Published var returnOnVoice: Bool
    @Published private(set) var status = CameraEngine.Status()
    /// The clip a press of "Loop" will play: the latest recording, or a saved one.
    @Published private(set) var currentClip: Clip?
    @Published private(set) var currentClipIsSaved = false
    @Published private(set) var savedClips: [ClipSummary] = []
    @Published private(set) var cameraAuthorized = CameraSource.isAuthorized
    @Published private(set) var linkOpen = false
    @Published private(set) var cameraStats: VirtualCameraStats?
    /// Apps reading the camera now.
    @Published private(set) var readers: [CameraReader] = []
    /// Apps that may not read the camera, by `AppIdentity.id`.
    @Published private(set) var blockedApps: [String]
    /// A blocked app is reading the camera, so no pictures are sent to anyone.
    @Published private(set) var withholding = false
    /// The camera is installed but macOS has not started it. Only a restart of the Mac brings it back.
    @Published private(set) var cameraNeedsRestart = false
    @Published var message: String?

    let extensionManager = ExtensionManager()
    /// The face swap: saved faces, the switch and its options.
    let face: FaceModel
    /// Shows the engine's output. Each frame's surface is handed to the layer as it is, without a copy.
    let previewLayer = CALayer()

    private let engine: CameraEngine?
    private let link = VirtualCameraLink()
    private let library: ClipLibrary?
    private let voice = VoiceTrigger()
    private var watcher: VirtualCameraWatcher?
    private var hotKeys: [HotKey] = []
    private var timer: Timer?
    private var bag: Set<AnyCancellable> = []
    private let defaults = UserDefaults.standard

    private var started = false
    /// The main window is on screen, so the preview needs pictures.
    private var windowVisible = false
    /// The source has been started for the present need for pictures.
    private var sourceStarted = false
    private var outputSize = PixelSize.hd720
    private var missingSince: Date?
    /// Which app each connected process belongs to. Looked up once per process.
    private var identities: [Int32: AppIdentity] = [:]
    /// The process numbers last sent to the extension, and the extension they were sent to.
    private var pushedBlocked: Set<Int32>?
    private var pushedToExtension: Int32 = 0

    init() {
        sourceKind = SourceKind(rawValue: defaults.string(forKey: "sourceKind") ?? "") ?? .camera
        mirror = defaults.bool(forKey: "mirror")
        showLoopBadge = defaults.bool(forKey: "showLoopBadge")
        loopMode = LoopMode(rawValue: defaults.string(forKey: "loopMode") ?? "") ?? .forward
        returnOnVoice = defaults.bool(forKey: "returnOnVoice")
        blockedApps = defaults.stringArray(forKey: "blockedApps") ?? []
        cameraID = defaults.string(forKey: "cameraID")

        let support = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask)[0]
            .appendingPathComponent("GodsCamera/Loops", isDirectory: true)
        library = try? ClipLibrary(directory: support)
        var renderer: MetalRenderer?
        do {
            renderer = try MetalRenderer()
        } catch {
            message = String(localized: "This Mac's graphics could not be started, so the camera cannot run.")
            Log.app.error("renderer failed: \(String(describing: error))")
        }
        engine = renderer.map(CameraEngine.init(renderer:))
        face = FaceModel(engine: engine, renderer: renderer)
        previewLayer.contentsGravity = .resizeAspect
        wireEngine()
        extensionManager.objectWillChange.sink { [weak self] in self?.objectWillChange.send() }.store(in: &bag)
        face.objectWillChange.sink { [weak self] in self?.objectWillChange.send() }.store(in: &bag)
    }

    // MARK: Start-up

    /// Called once at launch. During a self-test the app's usual work stays off.
    func start() {
        guard !started, SelfTest.requestedReportPath == nil else { return }
        started = true
        Log.app.info("launch, bundled extension build \(ExtensionManager.bundledBuild)")
        savedClips = library?.list() ?? []
        cameras = CameraSource.devices()
        if cameraID == nil || !cameras.contains(where: { $0.id == cameraID }) { cameraID = cameras.first?.id }
        extensionManager.refresh()
        applySettings()
        registerHotKeys()
        face.start()
        voice.onVoice = { [weak self] in Task { @MainActor in self?.voiceHeard() } }
        // The camera reports when an app starts or stops reading it; the timer covers everything else.
        watcher = VirtualCameraWatcher { [weak self] in Task { @MainActor in self?.tick() } }
        timer = Timer.scheduledTimer(withTimeInterval: 1, repeats: true) { [weak self] _ in
            Task { @MainActor in self?.tick() }
        }
        tick()
    }

    func windowAppeared() {
        windowVisible = true
        updateDemand()
    }

    func windowDisappeared() {
        windowVisible = false
        updateDemand()
    }

    private func wireEngine() {
        guard let engine else { return }
        engine.sink = link
        let layer = previewLayer
        engine.onOutput = { buffer, _ in
            guard let surface = CVPixelBufferGetIOSurface(buffer)?.takeUnretainedValue() else { return }
            DispatchQueue.main.async {
                CATransaction.begin()
                CATransaction.setDisableActions(true)
                layer.contents = surface
                CATransaction.commit()
            }
        }
        engine.onStatus = { [weak self] status in
            Task { @MainActor in
                guard let self else { return }
                let modeChanged = self.status.mode != status.mode
                self.status = status
                if modeChanged { self.updateVoiceTrigger() }
            }
        }
        engine.onClip = { [weak self] clip in
            Task { @MainActor in
                guard let self else { return }
                clip.mode = self.loopMode
                self.currentClip = clip
                self.currentClipIsSaved = false
            }
        }
    }

    /// Keep the link to the virtual camera open, read its counters and follow what its readers ask for.
    private func tick() {
        link.closeIfStale()
        watcher?.attach()
        let stats = VirtualCameraLink.readStats()
        let listed = stats != nil || VirtualCameraLink.isInstalled
        if !link.isOpen, listed {
            do {
                try link.open()
                Log.app.info("virtual camera link opened")
            } catch {
                Log.app.error("virtual camera link failed: \(String(describing: error))")
            }
        }
        linkOpen = link.isOpen
        cameraStats = stats
        cameraAuthorized = CameraSource.isAuthorized
        applyBlocking(stats)

        // Render at the size the reading apps asked for, so the camera does not have to scale.
        if let stats, stats.clients > 0 {
            let wanted = PixelSize(width: stats.width, height: stats.height)
            if wanted != outputSize {
                outputSize = wanted
                engine?.update { $0.outputSize = wanted }
                if sourceKind == .camera { restartSource() }
            }
        }
        updateDemand()

        if case .installed = extensionManager.state, !listed {
            let since = missingSince ?? Date()
            missingSince = since
            let waited = Date().timeIntervalSince(since)
            // An app that was running while the camera was installed or updated never
            // gets to see it, so reopen once. If the camera is still missing after
            // that, macOS did not start it, and only a restart of the Mac helps.
            if extensionManager.activatedInThisRun, !Relauncher.wasRelaunched {
                if waited > 3 {
                    Log.app.info("camera installed in this run but not visible; reopening the app")
                    Relauncher.relaunch()
                }
            } else {
                cameraNeedsRestart = waited > 10
            }
        } else {
            missingSince = nil
            cameraNeedsRestart = false
        }
    }

    private func registerHotKeys() {
        hotKeys = [
            HotKey(.controlOptionCommand(kVK_ANSI_L)) { [weak self] in Task { @MainActor in self?.toggleLoop() } },
            HotKey(.controlOptionCommand(kVK_ANSI_R)) { [weak self] in Task { @MainActor in self?.toggleRecording() } },
        ].compactMap { $0 }
    }

    // MARK: Source

    /// Pictures are needed while the window shows the preview, while an app
    /// reads the camera, or while a recording runs. Otherwise the real camera stays off.
    private var wantsPicture: Bool {
        windowVisible || (cameraStats?.clients ?? 0) > 0 || status.isRecording
    }

    private func updateDemand() {
        guard started, let engine else { return }
        if wantsPicture, !sourceStarted {
            sourceStarted = true
            startSource()
        } else if !wantsPicture, sourceStarted {
            sourceStarted = false
            engine.setSource(nil)
            Log.app.info("nothing is watching; source stopped")
        }
    }

    func requestCameraAccess() {
        AVCaptureDevice.requestAccess(for: .video) { granted in
            Task { @MainActor in
                self.cameraAuthorized = granted
                self.cameras = CameraSource.devices()
                if self.cameraID == nil { self.cameraID = self.cameras.first?.id }
                if granted { self.restartSource() }
            }
        }
    }

    func chooseFile() {
        let panel = NSOpenPanel()
        panel.allowedContentTypes = sourceKind == .video ? [.movie] : [.image]
        panel.allowsMultipleSelection = false
        if panel.runModal() == .OK {
            fileURL = panel.url
            restartSource()
        }
    }

    func sourceChanged() {
        defaults.set(sourceKind.rawValue, forKey: "sourceKind")
        defaults.set(cameraID, forKey: "cameraID")
        // A file picked for pictures makes no sense for video, and the other way round.
        fileURL = nil
        restartSource()
    }

    /// Apply the chosen source now, if pictures are needed.
    func restartSource() {
        sourceStarted = false
        updateDemand()
        if !sourceStarted { engine?.setSource(nil) }
    }

    private func startSource() {
        guard let engine else { return }
        message = nil
        do {
            switch sourceKind {
            case .camera:
                guard cameraAuthorized else {
                    engine.setSource(nil)
                    return
                }
                guard let cameraID else {
                    engine.setSource(nil)
                    message = String(localized: "No camera was found.")
                    return
                }
                engine.setSource(CameraSource(deviceID: cameraID, size: outputSize))
            case .image:
                guard let fileURL else { return engine.setSource(nil) }
                engine.setSource(try StillImageSource(url: fileURL))
            case .video:
                guard let fileURL else { return engine.setSource(nil) }
                engine.setSource(try VideoFileSource(url: fileURL))
            case .pattern:
                engine.setSource(TestPatternSource(size: outputSize))
            }
        } catch {
            engine.setSource(nil)
            message = String(localized: "That file could not be opened.")
            Log.app.error("source failed: \(String(describing: error))")
        }
    }

    func applySettings() {
        defaults.set(mirror, forKey: "mirror")
        defaults.set(showLoopBadge, forKey: "showLoopBadge")
        defaults.set(loopMode.rawValue, forKey: "loopMode")
        defaults.set(returnOnVoice, forKey: "returnOnVoice")
        let (mirror, badge) = (mirror, showLoopBadge)
        engine?.update {
            $0.mirror = mirror
            $0.showLoopBadge = badge
        }
        // A new loop mode applies to the clip in hand; a loop already playing picks it up on its next start.
        currentClip?.mode = loopMode
        if returnOnVoice, !VoiceTrigger.isAuthorized {
            AVCaptureDevice.requestAccess(for: .audio) { granted in
                Task { @MainActor in
                    if !granted {
                        self.returnOnVoice = false
                        self.defaults.set(false, forKey: "returnOnVoice")
                        self.message = String(
                            localized: "Without the microphone, God's Camera cannot tell when you start to speak.")
                    }
                    self.updateVoiceTrigger()
                }
            }
        }
        updateVoiceTrigger()
    }

    // MARK: Recording and looping

    var canRecord: Bool { status.mode == .live }
    var canLoop: Bool { currentClip != nil || status.mode == .looping }

    func toggleRecording() {
        guard let engine, canRecord else { return }
        if status.isRecording { engine.stopRecording() } else { engine.startRecording() }
    }

    func toggleLoop() {
        guard let engine else { return }
        if status.mode == .looping {
            engine.stopLoop()
        } else if let clip = currentClip {
            clip.mode = loopMode
            engine.startLoop(clip)
        }
    }

    /// The microphone listens only while a loop plays and the user asked for it.
    private func updateVoiceTrigger() {
        guard returnOnVoice, status.mode == .looping, VoiceTrigger.isAuthorized else {
            voice.stop()
            return
        }
        do {
            try voice.start()
        } catch {
            Log.app.error("microphone failed to start: \(String(describing: error))")
        }
    }

    private func voiceHeard() {
        guard returnOnVoice, status.mode == .looping else { return }
        Log.app.info("voice heard; back to live")
        engine?.stopLoop()
    }

    /// The saved clip's lighting differs from the room's now, so the loop may look out of place.
    var lightingDiffers: Bool {
        guard let clip = currentClip, status.mode == .live, status.liveLuma > 0 else { return false }
        return abs(clip.meanLuma - status.liveLuma) > 25
    }

    func saveCurrentClip(named name: String) {
        guard let clip = currentClip, let library else { return }
        clip.name = name.isEmpty ? clip.name : name
        do {
            try library.save(clip)
            currentClipIsSaved = true
            savedClips = library.list()
        } catch {
            message = String(localized: "The loop could not be saved.")
            Log.app.error("save failed: \(String(describing: error))")
        }
    }

    func selectSavedClip(_ id: UUID) {
        guard let library else { return }
        do {
            let clip = try library.load(id)
            currentClip = clip
            currentClipIsSaved = true
            loopMode = clip.mode
        } catch {
            message = String(localized: "That loop could not be opened.")
            Log.app.error("load failed: \(String(describing: error))")
        }
    }

    func deleteSavedClip(_ id: UUID) {
        library?.delete(id)
        savedClips = library?.list() ?? []
        if currentClip?.id == id { currentClipIsSaved = false }
    }

    // MARK: Apps reading the camera

    func block(_ appID: String) {
        guard !blockedApps.contains(appID) else { return }
        blockedApps.append(appID)
        defaults.set(blockedApps, forKey: "blockedApps")
        applyBlocking(cameraStats)
    }

    func unblock(_ appID: String) {
        blockedApps.removeAll { $0 == appID }
        defaults.set(blockedApps, forKey: "blockedApps")
        applyBlocking(cameraStats)
    }

    /// The name to show for a blocked app.
    func displayName(forBlocked appID: String) -> String {
        if let url = NSWorkspace.shared.urlForApplication(withBundleIdentifier: appID) {
            return url.deletingPathExtension().lastPathComponent
        }
        return appID.hasPrefix("/") ? URL(fileURLWithPath: appID).lastPathComponent : appID
    }

    /// Work out which processes belong to blocked apps and act on it.
    ///
    /// macOS does not tell the camera which app a process belongs to, but this
    /// app can find out. So it sends the camera the process numbers to refuse,
    /// and it stops sending pictures altogether while a blocked app is reading,
    /// because every reader receives the same pictures.
    private func applyBlocking(_ stats: VirtualCameraStats?) {
        guard let stats else {
            readers = []
            pushedBlocked = nil
            withholding = false
            link.isMuted = false
            return
        }
        let readerPids = stats.clientList.map(\.pid)
        let known = Set(stats.connected).union(readerPids)
        identities = identities.filter { known.contains($0.key) }
        for pid in known where identities[pid] == nil { identities[pid] = AppIdentity.of(pid: pid) }

        let current = readerPids.compactMap { pid in identities[pid].map { CameraReader(pid: pid, app: $0) } }
        if current != readers { readers = current }
        let blocked = Set(known.filter { pid in identities[pid].map { blockedApps.contains($0.id) } ?? false })
        // A restarted extension has forgotten the list, so it is sent again.
        if blocked != pushedBlocked || stats.pid != pushedToExtension {
            if VirtualCameraLink.setBlockedPids(blocked) {
                pushedBlocked = blocked
                pushedToExtension = stats.pid
            }
        }
        let mustWithhold = current.contains { blockedApps.contains($0.app.id) }
        if mustWithhold != withholding {
            withholding = mustWithhold
            link.isMuted = mustWithhold
            Log.app.info(mustWithhold ? "a blocked app is reading; pictures withheld" : "pictures flowing again")
        }
    }

    // MARK: Everything the app keeps

    /// Remove saved faces, saved loops and the settings around them.
    func deleteAllData() {
        face.deleteEverything()
        for clip in savedClips { library?.delete(clip.id) }
        savedClips = []
        currentClip = nil
        blockedApps = []
        defaults.removeObject(forKey: "blockedApps")
        applyBlocking(cameraStats)
        Log.app.info("all saved data deleted")
    }

    // MARK: Diagnostics

    /// Write a report the user can send, and show it in the Finder.
    func exportDiagnostics() {
        var lines = [
            "God's Camera diagnostics, \(ISO8601DateFormatter().string(from: Date()))",
            "app \(Bundle.main.object(forInfoDictionaryKey: "CFBundleShortVersionString") ?? "?") build \(Bundle.main.object(forInfoDictionaryKey: "CFBundleVersion") ?? "?")",
            "macOS \(ProcessInfo.processInfo.operatingSystemVersionString)",
            "extension state: \(extensionManager.state), bundled build \(ExtensionManager.bundledBuild)",
            "virtual camera listed: \(VirtualCameraLink.isInstalled), link open: \(link.isOpen), sent \(link.framesSent), dropped \(link.framesDropped)",
            "extension counters: \(cameraStats?.encoded ?? "unavailable")",
            "readers: \(readers.map { "\($0.app.name) (\($0.pid))" }), blocked apps: \(blockedApps), withholding: \(withholding)",
            "engine: \(status)",
            "face swap: \(face.state), \(face.faces.count) saved face(s), quality \(face.quality.rawValue)",
            "camera access: \(cameraAuthorized), cameras: \(cameras.map(\.name))",
            "", "--- log ---",
        ]
        lines.append((try? String(contentsOf: Log.fileURL, encoding: .utf8)) ?? "(no log file)")
        let url = FileManager.default.temporaryDirectory.appendingPathComponent("GodsCamera-diagnostics.txt")
        try? lines.joined(separator: "\n").write(to: url, atomically: true, encoding: .utf8)
        NSWorkspace.shared.activateFileViewerSelecting([url])
    }
}
