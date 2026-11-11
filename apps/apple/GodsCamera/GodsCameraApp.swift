import SwiftUI

@main
struct GodsCameraApp: App {
    @StateObject private var extensionManager = ExtensionManager()
    @StateObject private var feeder = SinkFeeder()

    var body: some Scene {
        WindowGroup("God's Camera") {
            SpikeView(extensionManager: extensionManager, feeder: feeder)
        }
        .windowResizability(.contentSize)
    }
}

/// The spike S1 control panel: install the camera, then feed it a test pattern.
struct SpikeView: View {
    @ObservedObject var extensionManager: ExtensionManager
    @ObservedObject var feeder: SinkFeeder

    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            Text("Virtual camera").font(.headline)
            HStack {
                Button("Install camera") { extensionManager.activate() }
                Button("Remove camera") {
                    feeder.stop()
                    extensionManager.deactivate()
                }
            }
            Text(extensionManager.status).foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)

            Divider()

            Text("Test pattern").font(.headline)
            HStack {
                Button(feeder.isRunning ? "Stop test pattern" : "Send test pattern") {
                    if feeder.isRunning { feeder.stop() } else { feeder.start() }
                }
                Text("\(feeder.framesSent) frames sent").monospacedDigit().foregroundStyle(.secondary)
            }
            Text(feeder.status).foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)
        }
        .padding(20)
        .frame(width: 460)
    }
}
