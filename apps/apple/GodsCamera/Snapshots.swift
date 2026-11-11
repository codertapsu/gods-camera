import AppKit
import GodsCameraKit
import SwiftUI

/// Makes pictures of every screen, filled with made-up content, and quits.
///
/// Started with `--snapshot <folder>`. The window is drawn off screen, and the
/// camera, the hub and the camera extension are left alone, so this can run
/// beside a working copy of the app. It exists to check the screens' layout
/// and wording without a person looking at them, in both languages:
/// add `-AppleLanguages "(vi)"` for Vietnamese.
enum Snapshots {
    static var requestedFolder: String? {
        let arguments = CommandLine.arguments
        guard let index = arguments.firstIndex(of: "--snapshot"), index + 1 < arguments.count else { return nil }
        return arguments[index + 1]
    }

    private static var window: NSWindow?

    @MainActor
    static func run(folder: String) {
        NSApp.setActivationPolicy(.accessory)
        let model = AppModel.shared
        fill(model)
        let size = NSSize(width: 1180, height: 760)
        let host = NSHostingView(rootView: RootView().environmentObject(model).frame(width: size.width, height: size.height))
        let window = NSWindow(
            contentRect: NSRect(origin: .zero, size: size), styleMask: [.titled, .fullSizeContentView], backing: .buffered,
            defer: false)
        window.contentView = host
        // Far outside every display: laid out and drawn, never seen.
        window.setFrameOrigin(NSPoint(x: -20_000, y: -20_000))
        window.orderBack(nil)
        self.window = window

        let output = URL(fileURLWithPath: folder, isDirectory: true)
        try? FileManager.default.createDirectory(at: output, withIntermediateDirectories: true)
        var steps: [(name: String, prepare: () -> Void)] = Screen.allCases.map { screen in
            (screen.rawValue, { model.screen = screen })
        }
        steps.append(("scanner-code", {
            model.screen = .scanner
            model.scan.showSamplePairing(code: "482913", secondsLeft: 272)
        }))
        steps.append(("face-photos", {
            model.scan.showingPairing = false
            model.screen = .face
            model.face.showSamplePending()
        }))

        func next(_ index: Int) {
            guard index < steps.count else {
                print("snapshots written to \(output.path)")
                exit(0)
            }
            steps[index].prepare()
            DispatchQueue.main.asyncAfter(deadline: .now() + 0.9) {
                // A sheet is a window of its own; take whichever is in front.
                let view = (window.attachedSheet?.contentView) ?? host
                save(view, to: output.appendingPathComponent("\(steps[index].name).png"))
                next(index + 1)
            }
        }
        next(0)
    }

    private static func save(_ view: NSView, to url: URL) {
        view.layoutSubtreeIfNeeded()
        guard let drawn = view.bitmapImageRepForCachingDisplay(in: view.bounds) else { return }
        view.cacheDisplay(in: view.bounds, to: drawn)
        // A window's own background is not part of what a view draws, so the
        // drawing is laid over the colour a window has in the present appearance.
        guard
            let page = NSBitmapImageRep(
                bitmapDataPlanes: nil, pixelsWide: drawn.pixelsWide, pixelsHigh: drawn.pixelsHigh, bitsPerSample: 8,
                samplesPerPixel: 4, hasAlpha: true, isPlanar: false, colorSpaceName: .deviceRGB, bytesPerRow: 0,
                bitsPerPixel: 0),
            let context = NSGraphicsContext(bitmapImageRep: page)
        else { return }
        NSGraphicsContext.saveGraphicsState()
        NSGraphicsContext.current = context
        let whole = NSRect(x: 0, y: 0, width: drawn.pixelsWide, height: drawn.pixelsHigh)
        view.effectiveAppearance.performAsCurrentDrawingAppearance {
            NSColor.windowBackgroundColor.setFill()
            whole.fill()
        }
        drawn.draw(in: whole, from: .zero, operation: .sourceOver, fraction: 1, respectFlipped: false, hints: nil)
        NSGraphicsContext.restoreGraphicsState()
        try? page.representation(using: .png, properties: [:])?.write(to: url)
    }

    // MARK: Made-up content

    /// A soft portrait-like picture in two colours, standing in for a face or a camera picture.
    private static func picture(_ a: NSColor, _ b: NSColor, size: Int, person: Bool = true) -> CGImage? {
        guard
            let context = CGContext(
                data: nil, width: size, height: size, bitsPerComponent: 8, bytesPerRow: 0,
                space: CGColorSpace(name: CGColorSpace.sRGB)!, bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue),
            let gradient = CGGradient(
                colorsSpace: nil, colors: [a.cgColor, b.cgColor] as CFArray, locations: [0, 1])
        else { return nil }
        let s = CGFloat(size)
        context.drawLinearGradient(gradient, start: .zero, end: CGPoint(x: s, y: s), options: [])
        if person {
            context.setFillColor(NSColor.white.withAlphaComponent(0.55).cgColor)
            context.fillEllipse(in: CGRect(x: s * 0.33, y: s * 0.42, width: s * 0.34, height: s * 0.38))
            context.fillEllipse(in: CGRect(x: s * 0.14, y: -s * 0.36, width: s * 0.72, height: s * 0.74))
        }
        return context.makeImage()
    }

    private static func png(_ a: NSColor, _ b: NSColor) -> Data {
        picture(a, b, size: 160).flatMap { NSBitmapImageRep(cgImage: $0).representation(using: .png, properties: [:]) } ?? Data()
    }

    @MainActor
    private static func fill(_ model: AppModel) {
        let palette: [(NSColor, NSColor)] = [
            (.systemTeal, .systemIndigo), (.systemOrange, .systemPink), (.systemGreen, .systemTeal),
            (.systemPurple, .systemBlue), (.systemYellow, .systemOrange),
        ]
        func photos(_ count: Int, from start: Int) -> [FacePhoto] {
            (0..<count).map { index in
                let colours = palette[(start + index) % palette.count]
                return FacePhoto(identity: [], thumbnail: png(colours.0, colours.1))
            }
        }
        model.face.showSamples([
            SavedFace(name: String(localized: "Sample: me"), consent: .myself, photos: photos(5, from: 0), identity: []),
            SavedFace(name: String(localized: "Sample: character"), consent: .synthetic, photos: photos(1, from: 3), identity: []),
        ])
        let now = Date()
        let folder = FileManager.default.temporaryDirectory.appendingPathComponent("gc-snapshot-documents")
        try? FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
        try? String(localized: "Sample document\n\nThis is where the text of a scanned page appears, joined from all its photos and ready to copy.")
            .write(to: folder.appendingPathComponent("document.txt"), atomically: true, encoding: .utf8)
        model.scan.showSamples(
            documents: [
                ScannedDocument(
                    id: "sample-1", receivedAt: now.addingTimeInterval(-600), status: "done", progress: 1, pictures: 6,
                    title: String(localized: "Sample document"), folder: folder, error: nil),
                ScannedDocument(
                    id: "sample-2", receivedAt: now.addingTimeInterval(-30), status: "running", progress: 0.4, pictures: 9,
                    title: "", folder: folder, error: nil),
            ],
            devices: [
                PairedDevice(id: "sample", name: "Galaxy Tab S9+", pairedAt: now.addingTimeInterval(-86_400), lastSeen: now.addingTimeInterval(-20))
            ])
        model.showSamples(
            readers: [CameraReader(pid: 1, app: AppIdentity(id: "us.zoom.xos", name: "zoom.us"))],
            reopen: [AppToReopen(id: "com.google.Chrome", name: "Google Chrome", pid: 0)],
            clips: [
                ClipSummary(id: UUID(), name: String(localized: "Sample: at my desk"), duration: 6.2, createdAt: now, meanLuma: 120)
            ],
            picture: picture(.systemTeal, .systemIndigo, size: 720))
    }
}
