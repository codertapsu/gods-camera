import AppKit
import GodsCameraKit
import ImageIO
import SwiftUI

/// One photo read for a face that is being added.
struct PendingPhoto: Identifiable {
    let id = UUID()
    let fileName: String
    let identity: [Float]
    let thumbnail: Data
    let facesInPhoto: Int
    /// The photo shows the same person as the others, so it goes into the face.
    var used = true
}

/// Photos that were read and now wait for the user: to become a new face, or
/// to be added to a saved one.
struct PendingFace: Identifiable {
    let id = UUID()
    var photos: [PendingPhoto]
    /// Files in which no face was found, or that could not be read.
    var skipped: [String]
    /// The saved face the photos are for, or nil for a new face.
    var target: SavedFace?

    var usedCount: Int { photos.filter(\.used).count }
    var leftOutCount: Int { photos.count - usedCount }
}

/// The face swap as the window sees it: the saved faces, the switch, and what the swap is doing.
@MainActor
final class FaceModel: ObservableObject {
    enum State: Equatable {
        case off
        case loading
        case on
        /// A model file the swap needs is not installed.
        case missing(String)
        case failed(String)
    }

    @Published private(set) var faces: [SavedFace] = []
    @Published private(set) var selectedID: UUID?
    @Published private(set) var state = State.off
    @Published private(set) var wanted: Bool
    @Published var quality: FaceSwapConfig.Model
    @Published var settings = FaceSwapSettings()
    @Published var showAIBadge: Bool
    @Published var pending: PendingFace?
    @Published var askingPolicy = false
    @Published var busyAdding = false
    @Published var message: String?

    private weak var engine: CameraEngine?
    private let renderer: MetalRenderer?
    private var library: FaceLibrary?
    private var stage: FaceSwapStage?
    private let photoReader = PhotoReader()
    private let defaults = UserDefaults.standard
    private let work = DispatchQueue(label: "dev.godscamera.face", qos: .userInitiated)

    /// Where model files are looked for: inside the app first, then the folder for packs added later.
    nonisolated static var packFolders: [URL] {
        var folders: [URL] = []
        if let bundled = Bundle.main.resourceURL?.appendingPathComponent("Packs") { folders.append(bundled) }
        folders.append(supportFolder.appendingPathComponent("Packs", isDirectory: true))
        return folders
    }

    nonisolated static var supportFolder: URL {
        FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask)[0]
            .appendingPathComponent("GodsCamera", isDirectory: true)
    }

    nonisolated static var cacheFolder: URL {
        FileManager.default.urls(for: .cachesDirectory, in: .userDomainMask)[0]
            .appendingPathComponent("GodsCamera/Models", isDirectory: true)
    }

    init(engine: CameraEngine?, renderer: MetalRenderer?) {
        self.engine = engine
        self.renderer = renderer
        wanted = defaults.bool(forKey: "swapWanted")
        quality = FaceSwapConfig.Model(rawValue: defaults.string(forKey: "swapQuality") ?? "") ?? .inswapper128
        showAIBadge = defaults.object(forKey: "showAIBadge") as? Bool ?? true
        settings.colourMatch = defaults.object(forKey: "swapColourMatch") as? Bool ?? true
        settings.sharpen = defaults.object(forKey: "swapSharpen") as? Float ?? settings.sharpen
        settings.keepObjectsInFront = defaults.object(forKey: "swapKeepObjects") as? Bool ?? true
        settings.realMouth = defaults.bool(forKey: "swapRealMouth")
        settings.blurWhenUnswapped = defaults.bool(forKey: "swapBlurWhenOff")
        selectedID = defaults.string(forKey: "selectedFace").flatMap(UUID.init)
    }

    /// The qualities whose model files are installed.
    var availableQualities: [FaceSwapConfig.Model] {
        FaceSwapConfig.Model.allCases.filter { model in
            var config = FaceSwapConfig(packFolders: Self.packFolders)
            config.model = model
            return config.missingFile == nil
        }
    }

    /// The model that keeps a hand or an object in front of the face is installed.
    let canKeepObjectsInFront = FaceModel.packFolders.contains {
        FileManager.default.fileExists(atPath: $0.appendingPathComponent(FaceSwapConfig.occlusionFile).path)
    }

    var policyAccepted: Bool { defaults.object(forKey: "usePolicyAcceptedAt") != nil }
    var selected: SavedFace? { faces.first { $0.id == selectedID } }

    /// Open the face library and carry on from the last session.
    func start() {
        do {
            library = try FaceLibrary(
                directory: Self.supportFolder.appendingPathComponent("Faces", isDirectory: true),
                key: try FaceStoreKey.load())
            faces = library?.list() ?? []
        } catch {
            message = String(localized: "Saved faces could not be opened.")
            Log.app.error("face library failed: \(String(describing: error))")
        }
        if selected == nil { selectedID = faces.first?.id }
        engine?.update { [badge = showAIBadge] in $0.showAIBadge = badge }
        if wanted { turnOn() }
    }

    // MARK: The switch

    func setWanted(_ on: Bool) {
        if on, !policyAccepted {
            askingPolicy = true
            return
        }
        wanted = on
        defaults.set(on, forKey: "swapWanted")
        if on { turnOn() } else { turnOff() }
    }

    func acceptPolicy() {
        defaults.set(Date(), forKey: "usePolicyAcceptedAt")
        askingPolicy = false
        setWanted(true)
    }

    private func turnOff() {
        engine?.setSwapStage(nil)
        state = .off
    }

    private func turnOn() {
        guard let renderer, let engine else { return }
        guard let face = selected else {
            state = .off
            message = String(localized: "Add a face first, then turn the swap on.")
            wanted = false
            defaults.set(false, forKey: "swapWanted")
            return
        }
        var config = FaceSwapConfig(packFolders: Self.packFolders, cacheFolder: Self.cacheFolder)
        config.model = quality
        if let missing = config.missingFile {
            state = .missing(missing)
            return
        }
        if let stage {
            stage.setIdentity(face.identity)
            stage.update(settings)
            engine.setSwapStage(stage)
            state = .on
            return
        }
        state = .loading
        let settings = settings
        work.async {
            // Loading compiles the models the first time, which takes several seconds.
            let result = Result { try FaceSwapStage(config: config, renderer: renderer) }
            DispatchQueue.main.async {
                switch result {
                case .success(let stage):
                    self.stage = stage
                    guard self.wanted, let face = self.selected else { return self.turnOff() }
                    stage.setIdentity(face.identity)
                    stage.update(settings)
                    engine.setSwapStage(stage)
                    self.state = .on
                    Log.app.info("face swap on, \(config.model.rawValue)")
                case .failure(let error):
                    self.state = .failed("\(error)")
                    Log.app.error("face swap failed to start: \(String(describing: error))")
                }
            }
        }
    }

    func settingsChanged() {
        defaults.set(quality.rawValue, forKey: "swapQuality")
        defaults.set(showAIBadge, forKey: "showAIBadge")
        defaults.set(settings.colourMatch, forKey: "swapColourMatch")
        defaults.set(settings.sharpen, forKey: "swapSharpen")
        defaults.set(settings.keepObjectsInFront, forKey: "swapKeepObjects")
        defaults.set(settings.realMouth, forKey: "swapRealMouth")
        defaults.set(settings.blurWhenUnswapped, forKey: "swapBlurWhenOff")
        engine?.update { [badge = showAIBadge] in $0.showAIBadge = badge }
        stage?.update(settings)
    }

    /// A different model needs a new stage.
    func qualityChanged() {
        settingsChanged()
        engine?.setSwapStage(nil)
        stage = nil
        if wanted { turnOn() }
    }

    func select(_ id: UUID) {
        selectedID = id
        defaults.set(id.uuidString, forKey: "selectedFace")
        if let face = selected, state == .on { stage?.setIdentity(face.identity) }
    }

    // MARK: Adding and removing faces

    /// Ask for photos: for a new face, or for more photos of a saved one.
    func choosePhotos(for target: SavedFace? = nil) {
        let panel = NSOpenPanel()
        panel.allowedContentTypes = [.image]
        panel.allowsMultipleSelection = true
        panel.message =
            target == nil
            ? String(localized: "Choose one or more photos of the same person. Several photos give a better likeness.")
            : String(localized: "Choose more photos of \(target?.name ?? "").")
        guard panel.runModal() == .OK, !panel.urls.isEmpty else { return }
        addPhotos(panel.urls, to: target)
    }

    /// Read photos and show what was found in them, ready to be saved.
    func addPhotos(_ urls: [URL], to target: SavedFace? = nil) {
        guard !busyAdding else { return }
        message = nil
        busyAdding = true
        let room = SavedFace.maxPhotos - (target?.photos.count ?? 0)
        let taken = Array(urls.prefix(max(0, room)))
        let reader = photoReader
        let (folders, cache) = (Self.packFolders, Self.cacheFolder)
        let existing = target?.photos.map(\.identity) ?? []
        work.async {
            var photos: [PendingPhoto] = []
            var skipped: [String] = []
            var failure: Error?
            for url in taken {
                do {
                    if let found = try reader.read(url, packFolders: folders, cacheFolder: cache) {
                        photos.append(found)
                    } else {
                        skipped.append(url.lastPathComponent)
                    }
                } catch {
                    skipped.append(url.lastPathComponent)
                    failure = error
                }
            }
            // The photos must show one person: the one in the saved face, or
            // the one most of the new photos agree on.
            let combined = FaceEnrolment.combine(existing + photos.map(\.identity))
            for index in photos.indices { photos[index].used = combined.used[existing.count + index] }
            let result = PendingFace(photos: photos, skipped: skipped, target: target)
            DispatchQueue.main.async {
                self.busyAdding = false
                if let failure { Log.app.error("a photo could not be read: \(String(describing: failure))") }
                if photos.isEmpty {
                    self.message =
                        urls.count == 1
                        ? String(localized: "No face was found in that picture.")
                        : String(localized: "No face was found in those pictures.")
                } else {
                    if urls.count > taken.count {
                        self.message = String(localized: "A face can have up to \(SavedFace.maxPhotos) photos. The rest were not read.")
                    }
                    self.pending = result
                }
            }
        }
    }

    /// Save the photos in hand: as a new face, or into the saved face they were read for.
    func savePending(name: String, consent: FaceConsent) {
        guard let pending, let library else { return }
        let added = pending.photos.filter(\.used).map { FacePhoto(identity: $0.identity, thumbnail: $0.thumbnail) }
        guard !added.isEmpty else { return }
        var face: SavedFace
        if let target = pending.target, let current = faces.first(where: { $0.id == target.id }) {
            face = current
            let photos = Array((current.photos + added).prefix(SavedFace.maxPhotos))
            face.setPhotos(photos, identity: FaceEnrolment.combine(photos.map(\.identity)).identity)
        } else {
            face = SavedFace(
                name: name.isEmpty ? String(localized: "Face \(faces.count + 1)") : name, consent: consent,
                photos: added, identity: FaceEnrolment.combine(added.map(\.identity)).identity)
        }
        do {
            try library.save(face)
            faces = library.list()
            self.pending = nil
            select(face.id)
            Log.app.info("face saved with \(face.photos.count) photo(s)")
        } catch {
            message = String(localized: "The face could not be saved.")
            Log.app.error("face save failed: \(String(describing: error))")
        }
    }

    func rename(_ id: UUID, to name: String) {
        let trimmed = name.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty, var face = faces.first(where: { $0.id == id }), let library else { return }
        face.name = trimmed
        try? library.save(face)
        faces = library.list()
    }

    /// Take one photo out of a face. The last photo cannot be removed; delete the face instead.
    func removePhoto(_ photoID: UUID, from faceID: UUID) {
        guard var face = faces.first(where: { $0.id == faceID }), face.photos.count > 1, let library else { return }
        let photos = face.photos.filter { $0.id != photoID }
        face.setPhotos(photos, identity: FaceEnrolment.combine(photos.map(\.identity)).identity)
        try? library.save(face)
        faces = library.list()
        if selectedID == faceID, state == .on { stage?.setIdentity(face.identity) }
    }

    func delete(_ id: UUID) {
        library?.delete(id)
        faces = library?.list() ?? []
        if selectedID == id {
            selectedID = faces.first?.id
            if let face = selected { stage?.setIdentity(face.identity) } else if wanted { setWanted(false) }
        }
    }

    /// Remove every saved face and the key that protected them, and forget the settings around them.
    func deleteEverything() {
        setWanted(false)
        stage = nil
        library?.deleteAll()
        FaceStoreKey.delete()
        library = nil
        faces = []
        selectedID = nil
        for key in ["selectedFace", "swapWanted", "usePolicyAcceptedAt"] { defaults.removeObject(forKey: key) }
        try? FileManager.default.removeItem(at: Self.cacheFolder)
        // A fresh key and an empty library, ready for new faces.
        start()
    }

    // MARK: Sample content

    /// Show the sheet for new photos with made-up content, for pictures of the app.
    func showSamplePending() {
        guard let sample = faces.first else { return }
        var photos = sample.photos.map {
            PendingPhoto(fileName: "photo.jpg", identity: [], thumbnail: $0.thumbnail, facesInPhoto: 1)
        }
        if photos.count > 1 { photos[photos.count - 1].used = false }
        pending = PendingFace(photos: photos, skipped: ["IMG_0412.jpg"], target: nil)
    }

    /// Fill the model with made-up faces, for pictures of the app. Nothing is saved.
    func showSamples(_ samples: [SavedFace]) {
        faces = samples
        selectedID = samples.first?.id
        wanted = true
        state = .on
    }
}

/// Reads photos for the face library. Used from one background queue only,
/// which is what keeps its lazily made enrolment safe.
final class PhotoReader: @unchecked Sendable {
    private var enrolment: FaceEnrolment?

    /// Read a photo, find its largest face and cut a small picture of it.
    func read(_ url: URL, packFolders: [URL], cacheFolder: URL) throws -> PendingPhoto? {
        guard let source = CGImageSourceCreateWithURL(url as CFURL, nil) else { throw SourceError.unreadable(url) }
        // Apply the photo's orientation and cap its size in one step.
        let options: [CFString: Any] = [
            kCGImageSourceCreateThumbnailFromImageAlways: true,
            kCGImageSourceCreateThumbnailWithTransform: true,
            kCGImageSourceThumbnailMaxPixelSize: 2048,
        ]
        guard let image = CGImageSourceCreateThumbnailAtIndex(source, 0, options as CFDictionary) else {
            throw SourceError.unreadable(url)
        }
        let enrolment = try self.enrolment ?? FaceEnrolment(packFolders: packFolders, cacheFolder: cacheFolder)
        self.enrolment = enrolment
        guard let found = try enrolment.enrol(image) else { return nil }

        // A square around the face with some margin, 160 pixels a side.
        let side = max(found.faceRect.width, found.faceRect.height) * 1.5
        let square = CGRect(
            x: found.faceRect.midX - side / 2, y: found.faceRect.midY - side / 2, width: side, height: side
        ).intersection(CGRect(x: 0, y: 0, width: image.width, height: image.height))
        var thumbnail = Data()
        if let cut = image.cropping(to: square),
            let context = CGContext(
                data: nil, width: 160, height: 160, bitsPerComponent: 8, bytesPerRow: 0,
                space: CGColorSpace(name: CGColorSpace.sRGB)!, bitmapInfo: CGImageAlphaInfo.noneSkipLast.rawValue)
        {
            context.interpolationQuality = .high
            context.draw(cut, in: CGRect(x: 0, y: 0, width: 160, height: 160))
            if let small = context.makeImage() {
                thumbnail = NSBitmapImageRep(cgImage: small).representation(using: .png, properties: [:]) ?? Data()
            }
        }
        return PendingPhoto(
            fileName: url.lastPathComponent, identity: found.identity, thumbnail: thumbnail,
            facesInPhoto: found.facesInPhoto)
    }
}
