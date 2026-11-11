import GodsCameraKit
import SwiftUI

/// Starts the app's work at launch, whether or not a window opens, and keeps
/// the app running in the menu bar after its window is closed, so the camera
/// keeps working during a call.
final class AppDelegate: NSObject, NSApplicationDelegate {
    func applicationDidFinishLaunching(_ notification: Notification) {
        // Started with --selftest, the app checks itself and quits instead of doing its usual work.
        if let path = SelfTest.requestedReportPath {
            SelfTest.run(reportPath: path)
        } else if let folder = Snapshots.requestedFolder {
            Snapshots.run(folder: folder)
        } else {
            AppModel.shared.start()
        }
    }

    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool { false }
}

@main
struct GodsCameraApp: App {
    @NSApplicationDelegateAdaptor(AppDelegate.self) private var appDelegate
    @ObservedObject private var model = AppModel.shared

    var body: some Scene {
        Window("God's Camera", id: "main") {
            RootView()
                .environmentObject(model)
                .onAppear { model.windowAppeared() }
                .onDisappear { model.windowDisappeared() }
        }
        .windowResizability(.contentMinSize)
        .defaultSize(width: 1180, height: 760)
        // Show the window at every launch, also when the app reopens itself after a camera update.
        // While pictures of the app are being made, no window comes on screen at all.
        .defaultLaunchBehavior(Snapshots.requestedFolder == nil ? .presented : .suppressed)
        .restorationBehavior(.disabled)

        MenuBarExtra(isInserted: .constant(Snapshots.requestedFolder == nil)) {
            MenuContent().environmentObject(model)
        } label: {
            Image(systemName: model.status.mode == .looping ? "repeat.circle.fill" : "camera.circle")
        }
    }
}

/// The menu-bar menu: the controls needed during a call, without the window.
private struct MenuContent: View {
    @EnvironmentObject private var model: AppModel
    @Environment(\.openWindow) private var openWindow

    var body: some View {
        switch model.status.mode {
        case .looping: Text("Looping")
        case .live: Text(model.status.swapping ? "Live, face swapped" : "Live")
        case .idle: Text("No picture")
        }
        Divider()
        Button(model.status.mode == .looping ? String(localized: "Back to live") : String(localized: "Play the loop")) {
            model.toggleLoop()
        }
        .disabled(!model.canLoop)
        Button(model.status.isRecording ? String(localized: "Stop recording") : String(localized: "Record a loop")) {
            model.toggleRecording()
        }
        .disabled(!model.canRecord)
        if !model.face.faces.isEmpty {
            Toggle("Swap my face", isOn: Binding(get: { model.face.wanted }, set: { model.face.setWanted($0) }))
        }
        Divider()
        Button("Show God's Camera") {
            openWindow(id: "main")
            NSApp.activate(ignoringOtherApps: true)
        }
        Button("Quit God's Camera") { NSApp.terminate(nil) }.keyboardShortcut("q")
    }
}
