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
            ContentView()
                .environmentObject(model)
                .onAppear { model.windowAppeared() }
                .onDisappear { model.windowDisappeared() }
        }
        .windowResizability(.contentMinSize)
        // Show the window at every launch, also when the app reopens itself after a camera update.
        .defaultLaunchBehavior(.presented)
        .restorationBehavior(.disabled)

        MenuBarExtra {
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
        case .live: Text("Live")
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
        Divider()
        Button("Show God's Camera") {
            openWindow(id: "main")
            NSApp.activate(ignoringOtherApps: true)
        }
        Button("Quit God's Camera") { NSApp.terminate(nil) }.keyboardShortcut("q")
    }
}
