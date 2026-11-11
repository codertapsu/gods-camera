import AppKit
import GodsCameraKit
import SwiftUI

/// The app's screens, as listed in the sidebar.
enum Screen: String, CaseIterable, Identifiable {
    case camera, loop, face, scanner, settings

    var id: String { rawValue }

    var title: String {
        switch self {
        case .camera: return String(localized: "Camera")
        case .loop: return String(localized: "Loop")
        case .face: return String(localized: "Face")
        case .scanner: return String(localized: "Scanner")
        case .settings: return String(localized: "Settings")
        }
    }

    var subtitle: String {
        switch self {
        case .camera: return String(localized: "What other apps see when they choose God's Camera.")
        case .loop: return String(localized: "Record a few seconds of yourself and play them as your camera picture.")
        case .face: return String(localized: "Show another face in place of yours, live.")
        case .scanner: return String(localized: "Photograph pages with your phone or tablet. The text arrives here.")
        case .settings: return String(localized: "The camera, your data and help.")
        }
    }

    var symbol: String {
        switch self {
        case .camera: return "video"
        case .loop: return "repeat"
        case .face: return "face.smiling"
        case .scanner: return "doc.viewfinder"
        case .settings: return "gearshape"
        }
    }

    /// The screen shows the live picture, so the camera must run while it is open.
    var showsPreview: Bool { self == .camera || self == .loop || self == .face }
}

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
        // The one picture layer moves to whichever screen is showing.
        if layer.superlayer !== view.layer { view.layer?.addSublayer(layer) }
        layer.frame = view.bounds
    }
}

/// A rounded panel that groups related controls under a heading.
struct Card<Content: View>: View {
    var title: String?
    var symbol: String?
    @ViewBuilder var content: Content

    init(_ title: String? = nil, symbol: String? = nil, @ViewBuilder content: () -> Content) {
        self.title = title
        self.symbol = symbol
        self.content = content()
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            if let title {
                HStack(spacing: 7) {
                    if let symbol {
                        Image(systemName: symbol).foregroundStyle(.tint).frame(width: 18)
                    }
                    Text(title).font(.headline)
                }
            }
            content
        }
        .padding(16)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(Color(nsColor: .controlBackgroundColor), in: RoundedRectangle(cornerRadius: 14, style: .continuous))
        .overlay(RoundedRectangle(cornerRadius: 14, style: .continuous).strokeBorder(.quaternary))
    }
}

/// The title and one-line description at the top of a screen.
struct ScreenHeader: View {
    let screen: Screen

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            Text(screen.title).font(.largeTitle.bold())
            Text(screen.subtitle).font(.body).foregroundStyle(.secondary)
        }
    }
}

/// A small coloured label drawn on the picture.
struct Badge: View {
    let text: String
    let color: Color

    var body: some View {
        Text(text)
            .font(.caption.bold()).monospacedDigit()
            .padding(.horizontal, 9).padding(.vertical, 4)
            .background(color.opacity(0.92), in: Capsule())
            .foregroundStyle(.white)
    }
}

/// A line of explanation under a control.
struct Hint: View {
    let text: String
    var symbol: String?
    var tone: Color = .secondary

    init(_ text: String, symbol: String? = nil, tone: Color = .secondary) {
        self.text = text
        self.symbol = symbol
        self.tone = tone
    }

    var body: some View {
        HStack(alignment: .firstTextBaseline, spacing: 6) {
            if let symbol { Image(systemName: symbol) }
            Text(text).fixedSize(horizontal: false, vertical: true)
        }
        .font(.callout)
        .foregroundStyle(tone)
    }
}

/// The live picture with its state drawn on it. Shared by the screens that show it.
struct PreviewPane: View {
    @EnvironmentObject private var model: AppModel

    var body: some View {
        PreviewView(layer: model.previewLayer)
            .aspectRatio(16 / 9, contentMode: .fit)
            .overlay { if model.status.mode == .idle { emptyPicture } }
            .clipShape(RoundedRectangle(cornerRadius: 14, style: .continuous))
            .overlay(alignment: .topLeading) { badges.padding(10) }
            .overlay(alignment: .bottomTrailing) {
                if model.status.framesOut > 0, model.status.mode != .idle {
                    Text("\(model.status.sourceFps, specifier: "%.0f") pictures a second")
                        .font(.caption).monospacedDigit()
                        .padding(.horizontal, 8).padding(.vertical, 3)
                        .background(.black.opacity(0.45), in: Capsule())
                        .foregroundStyle(.white)
                        .padding(10)
                }
            }
            .shadow(color: .black.opacity(0.18), radius: 12, y: 5)
    }

    private var badges: some View {
        HStack(spacing: 6) {
            switch model.status.mode {
            case .looping: Badge(text: String(localized: "LOOPING"), color: .red)
            case .live:
                Badge(text: String(localized: "LIVE"), color: .green)
                if model.status.swapping { Badge(text: String(localized: "AI FACE"), color: .purple) }
                if model.status.pictureHidden { Badge(text: String(localized: "BLURRED"), color: .indigo) }
            case .idle: Badge(text: String(localized: "NO PICTURE"), color: .gray)
            }
            if model.status.isRecording {
                Badge(text: String(localized: "REC \(Int(model.status.recordedSeconds)) s"), color: .orange)
            }
        }
    }

    /// Says why there is no picture, and offers the way out where there is one.
    private var emptyPicture: some View {
        VStack(spacing: 10) {
            Image(systemName: "video.slash").font(.system(size: 34)).foregroundStyle(.white.opacity(0.7))
            if model.sourceKind == .camera, !model.cameraAuthorized {
                Text("God's Camera needs your permission to use the camera.")
                Button("Allow camera access") { model.requestCameraAccess() }.controlSize(.large)
            } else if let message = model.message {
                Text(message)
            } else if model.sourceKind == .image || model.sourceKind == .video, model.fileURL == nil {
                Text("Choose a file to show.")
                Button("Choose…") { model.chooseFile() }
            } else {
                Text("Waiting for the picture…")
            }
        }
        .font(.callout)
        .multilineTextAlignment(.center)
        .foregroundStyle(.white.opacity(0.85))
        .padding(24)
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background(.black)
    }
}

/// A screen with the live picture on the left and its controls in a column on the right.
struct StudioLayout<Under: View, Controls: View>: View {
    let screen: Screen
    @ViewBuilder var under: Under
    @ViewBuilder var controls: Controls

    var body: some View {
        HStack(alignment: .top, spacing: 0) {
            VStack(alignment: .leading, spacing: 18) {
                ScreenHeader(screen: screen)
                PreviewPane()
                under
                Spacer(minLength: 0)
            }
            .padding(24)
            .frame(minWidth: 480, maxWidth: .infinity, alignment: .topLeading)

            Divider()

            ScrollView {
                VStack(alignment: .leading, spacing: 14) { controls }
                    .padding(18)
            }
            .frame(width: 348)
        }
    }
}

/// An app's icon from its bundle identifier or program path, for lists of apps.
func appIcon(forAppID id: String) -> NSImage {
    if id.hasPrefix("/") { return NSWorkspace.shared.icon(forFile: id) }
    if let url = NSWorkspace.shared.urlForApplication(withBundleIdentifier: id) {
        return NSWorkspace.shared.icon(forFile: url.path)
    }
    return NSImage(systemSymbolName: "app.dashed", accessibilityDescription: nil) ?? NSImage()
}

/// A picture stored as PNG data, or a grey square when it cannot be read.
struct DataImage: View {
    let data: Data
    let side: CGFloat
    var round = false

    var body: some View {
        Group {
            if let image = NSImage(data: data) {
                Image(nsImage: image).resizable().scaledToFill()
            } else {
                Color.gray.opacity(0.3).overlay(Image(systemName: "person.fill").foregroundStyle(.secondary))
            }
        }
        .frame(width: side, height: side)
        .clipShape(RoundedRectangle(cornerRadius: round ? side / 2 : side * 0.22, style: .continuous))
    }
}
