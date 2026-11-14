import CoreVideo
import Foundation
import GCCore
import Metal

/// How the face swap is set up. Changing any of it means making a new stage.
public struct FaceSwapConfig: Equatable, Sendable {
    public enum Model: String, Sendable, CaseIterable {
        case inswapper128
        case hyperswap256
    }

    /// Folders that hold model files, searched in order.
    public var packFolders: [URL]
    /// Folder for compiled models, so later starts are fast.
    public var cacheFolder: URL?
    public var model = Model.inswapper128
    public var cpuOnly = false

    public init(packFolders: [URL], cacheFolder: URL? = nil) {
        self.packFolders = packFolders
        self.cacheFolder = cacheFolder
    }

    /// The model files each swap model needs, relative to a pack folder.
    public static func requiredFiles(for model: Model) -> [String] {
        let detector = ["buffalo_l/det_10g.onnx"]
        switch model {
        case .inswapper128: return detector + ["inswapper_128.reflectfree.fp16.onnx", "inswapper_128.emap.bin"]
        case .hyperswap256: return detector + ["hyperswap_1b_256.onnx"]
        }
    }

    /// The model that tells the face from a hand or an object in front of it.
    /// The swap works without it.
    public static let occlusionFile = "xseg_3.onnx"

    /// The first missing file for the chosen model, or nil when everything is there.
    public var missingFile: String? {
        Self.requiredFiles(for: model).first { file in
            !packFolders.contains { FileManager.default.fileExists(atPath: $0.appendingPathComponent(file).path) }
        }
    }
}

/// What the user can change while the swap runs.
public struct FaceSwapSettings: Equatable, Sendable {
    /// Leave hair, ears and background alone (oval), or replace the whole square (box).
    public var ovalMask = true
    /// Shift the swapped face's colours towards the real face's.
    public var colourMatch = true
    /// 0 shows the real face, 1 the swapped one.
    public var opacity: Float = 1
    /// How much crisper to make the swapped face, 0 to 1.
    public var sharpen: Float = 0.4
    /// Keep a hand, a cup or a microphone in front of the swapped face.
    public var keepObjectsInFront = true
    /// Leave the real mouth showing through the swap.
    public var realMouth = false
    /// Leave the real eyes showing through the swap: the gaze and the blinks are the person's own.
    public var realEyes = false
    /// Blur the whole picture whenever the swap is not fully on the face, so
    /// the real face never shows.
    public var blurWhenUnswapped = false

    public init() {}
}

/// What a face swap wants drawn for one frame.
public struct FaceSwapOutput {
    /// The swapped face to draw, or nil when there is none.
    public var patch: RenderPatch?
    /// How strongly to blur the whole picture, 0 to 1, so the real face does not show.
    public var blur: Float

    public init(patch: RenderPatch? = nil, blur: Float = 0) {
        self.patch = patch
        self.blur = blur
    }
}

/// What a face swap says about itself.
public struct FaceSwapReport: Equatable, Sendable {
    /// A face is being followed.
    public var faceFound = false
    /// Smoothed time for one swap, and from a frame's arrival to its swapped face, in milliseconds.
    public var swapMs = 0.0
    public var swapAgeMs = 0.0
    /// What stopped the swap, if anything did.
    public var error: String?

    public init() {}
}

/// A face swap as the camera engine sees it: a frame in, what to draw out.
public protocol FaceSwapping: AnyObject, Sendable {
    /// Take in one frame and say what to draw on it. Called on the engine's queue.
    func process(_ frame: CVPixelBuffer, hostTimeNs: UInt64) -> FaceSwapOutput
    var report: FaceSwapReport { get }
}

/// The live face swap as the engine sees it: frames in, a patch and its place out.
///
/// The work happens in the shared core. This class hands it each frame's
/// memory, keeps the newest swapped patch as a texture, and turns the core's
/// answer into something the renderer can draw.
///
/// `process` is called from the engine's queue and the setters from the main
/// thread. The core is safe for that; the one value kept here has its own lock.
public final class FaceSwapStage: FaceSwapping, @unchecked Sendable {
    private let engine: FaceEngine
    private let texture: MTLTexture
    private let lock = NSLock()
    /// The number of the patch in the texture, or 0 when the texture must not be used.
    private var sequence: UInt64 = 0
    public let patchSize: Int

    /// Loads the models, which takes seconds the first time. Call it off the main thread.
    public init(config: FaceSwapConfig, renderer: MetalRenderer) throws {
        if let cache = config.cacheFolder {
            try? FileManager.default.createDirectory(at: cache, withIntermediateDirectories: true)
        }
        engine = try FaceEngine(
            config: EngineConfig(
                packs: config.packFolders.map(\.path),
                cacheDir: config.cacheFolder?.path,
                detector: .large,
                detectorSize: 640,
                swapModel: config.model == .inswapper128 ? .inswapper128 : .hyperswap256,
                cpuOnly: config.cpuOnly))
        patchSize = Int(engine.cropSize())
        guard let texture = renderer.makePatchTexture(size: patchSize) else { throw RenderError.textureUnavailable }
        self.texture = texture
        update(FaceSwapSettings())
    }

    /// The face to swap in, as an identity from `FaceEnrolment`, or nil to stop swapping.
    public func setIdentity(_ identity: [Float]?) {
        engine.setIdentity(identity: identity)
        lock.lock()
        sequence = 0
        lock.unlock()
    }

    public func update(_ settings: FaceSwapSettings) {
        engine.setSettings(
            settings: EngineSettings(
                mask: settings.ovalMask ? .oval : .box, colourMatch: settings.colourMatch,
                opacity: settings.opacity, scoreThreshold: 0.5, sharpen: settings.sharpen,
                occlusion: settings.keepObjectsInFront, realMouth: settings.realMouth, realEyes: settings.realEyes,
                blurWhenUnswapped: settings.blurWhenUnswapped))
    }

    public var stats: EngineStats { engine.stats() }

    /// The occlusion model is loaded, so `keepObjectsInFront` has an effect.
    public var canKeepObjectsInFront: Bool { engine.stats().occlusion }

    public var report: FaceSwapReport {
        let stats = engine.stats()
        var report = FaceSwapReport()
        report.faceFound = stats.tracking
        report.swapMs = Double(stats.swapMs)
        report.swapAgeMs = Double(stats.swapAgeMs)
        report.error = stats.lastError
        return report
    }

    /// Take in one frame and say what to draw on it: the swapped face, if
    /// there is one, and how much to blur the picture.
    public func process(_ frame: CVPixelBuffer, hostTimeNs: UInt64) -> FaceSwapOutput {
        // A frame the swap cannot read is not known to be free of faces.
        let unreadable = FaceSwapOutput(blur: 0)
        guard CVPixelBufferGetPixelFormatType(frame) == kCVPixelFormatType_32BGRA else { return unreadable }
        CVPixelBufferLockBaseAddress(frame, .readOnly)
        defer { CVPixelBufferUnlockBaseAddress(frame, .readOnly) }
        guard let base = CVPixelBufferGetBaseAddress(frame) else { return unreadable }
        let stride = CVPixelBufferGetBytesPerRow(frame)
        let height = CVPixelBufferGetHeight(frame)
        // The core reads the frame's memory in place for the length of this call.
        let output = engine.pushFrame(
            frame: FrameMemory(
                address: UInt64(UInt(bitPattern: base)), length: UInt64(stride * height),
                width: UInt32(CVPixelBufferGetWidth(frame)), height: UInt32(height), stride: UInt32(stride),
                layout: .bgra),
            timeNs: hostTimeNs)
        let none = FaceSwapOutput(blur: output.blur)
        guard output.face, output.patchSequence > 0, output.opacity > 0 else { return none }

        lock.lock()
        var held = sequence
        lock.unlock()
        if output.patchSequence != held, let patch = engine.patch(newerThan: held) {
            patch.rgba.withUnsafeBytes { bytes in
                texture.replace(
                    region: MTLRegionMake2D(0, 0, patchSize, patchSize), mipmapLevel: 0,
                    withBytes: bytes.baseAddress!, bytesPerRow: patchSize * 4)
            }
            held = patch.sequence
            lock.lock()
            sequence = held
            lock.unlock()
        }
        guard held > 0 else { return none }

        // The core gives patch-to-frame; the shader wants frame-to-patch.
        let (a, b, c, d) = (output.a, output.b, output.c, output.d)
        let determinant = a * d - b * c
        guard abs(determinant) > 1e-9 else { return none }
        let (ia, ib, ic, id) = (d / determinant, -b / determinant, -c / determinant, a / determinant)
        let patch = RenderPatch(
            texture: texture,
            frameToPatch: (ia, ib, -(ia * output.tx + ib * output.ty), ic, id, -(ic * output.tx + id * output.ty)),
            opacity: output.opacity)
        return FaceSwapOutput(patch: patch, blur: output.blur)
    }
}

/// A face taken from a photo, ready to be swapped in.
public struct EnrolledIdentity: Sendable {
    /// 512 numbers that stand for the face.
    public let identity: [Float]
    /// How many faces the photo showed; the largest was used.
    public let facesInPhoto: Int
    /// Where that face is in the photo: x, y, width, height in pixels.
    public let faceRect: CGRect
}

/// One identity made from several photos of a person.
public struct CombinedIdentity: Sendable {
    /// The identity to swap in. Empty when no photo could be used.
    public let identity: [Float]
    /// For each photo, in the order given: whether it went into the identity.
    /// A photo is left out when it shows somebody other than most of the others.
    public let used: [Bool]
    /// For each photo: how alike its face and the combined identity are, -1 to 1.
    public let similarity: [Float]
}

/// Turns photos into identities for the face library.
public final class FaceEnrolment {
    /// Combine the identities of several photos of one person. The average
    /// keeps what the photos share, which is the person, and loses what
    /// differs between them: expression, angle and light.
    public static func combine(_ identities: [[Float]]) -> CombinedIdentity {
        let combined = combineIdentities(identities: identities)
        return CombinedIdentity(identity: combined.identity, used: combined.used, similarity: combined.similarity)
    }

    private let enroller: FaceEnroller

    public init(packFolders: [URL], cacheFolder: URL? = nil) throws {
        enroller = try FaceEnroller(packs: packFolders.map(\.path), cacheDir: cacheFolder?.path)
    }

    /// The identity of the largest face in a picture, or nil if it shows no face.
    public func enrol(_ image: CGImage) throws -> EnrolledIdentity? {
        // Draw into plain RGBA so the core gets a known layout, whatever the file was.
        let (width, height) = (image.width, image.height)
        var pixels = [UInt8](repeating: 0, count: width * height * 4)
        let drawn = pixels.withUnsafeMutableBytes { bytes -> Bool in
            guard
                let context = CGContext(
                    data: bytes.baseAddress, width: width, height: height, bitsPerComponent: 8,
                    bytesPerRow: width * 4, space: CGColorSpace(name: CGColorSpace.sRGB)!,
                    bitmapInfo: CGImageAlphaInfo.noneSkipLast.rawValue)
            else { return false }
            context.draw(image, in: CGRect(x: 0, y: 0, width: width, height: height))
            return true
        }
        guard drawn else { return nil }
        guard
            let found = try enroller.enrol(
                pixels: Data(pixels), width: UInt32(width), height: UInt32(height), stride: UInt32(width * 4),
                layout: .rgba)
        else { return nil }
        let box = found.bbox.map(CGFloat.init)
        return EnrolledIdentity(
            identity: found.identity, facesInPhoto: Int(found.faces),
            faceRect: CGRect(x: box[0], y: box[1], width: box[2] - box[0], height: box[3] - box[1]))
    }
}
