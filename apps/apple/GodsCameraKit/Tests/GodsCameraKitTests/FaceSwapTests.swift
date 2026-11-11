import CoreGraphics
import CoreVideo
import ImageIO
import UniformTypeIdentifiers
import XCTest

@testable import GodsCameraKit

/// The face swap through the whole Apple path: the core's bindings, the swap
/// stage, and the renderer drawing the patch.
///
/// Needs the model files, looked for in `GC_PACKS_DIR` and then in `packs`
/// beside the repository. Skipped where they are missing.
final class FaceSwapTests: XCTestCase {
    private static let repository = URL(fileURLWithPath: #filePath)
        .deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()
        .deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()

    private func packs() throws -> URL {
        let folder = ProcessInfo.processInfo.environment["GC_PACKS_DIR"].map(URL.init(fileURLWithPath:))
            ?? Self.repository.deletingLastPathComponent().appendingPathComponent("packs")
        var config = FaceSwapConfig(packFolders: [folder])
        config.model = .inswapper128
        if let missing = config.missingFile { throw XCTSkip("model file \(missing) is not in \(folder.path)") }
        return folder
    }

    private func samplePhoto() throws -> CGImage {
        let url = Self.repository.appendingPathComponent("core/gc-infer/tests/golden/input.png")
        guard let source = CGImageSourceCreateWithURL(url as CFURL, nil),
            let image = CGImageSourceCreateImageAtIndex(source, 0, nil)
        else { throw XCTSkip("sample photo not found at \(url.path)") }
        return image
    }

    private func savePNG(_ buffer: CVPixelBuffer, to name: String) {
        let size = buffer.pixelSize
        let image = buffer.withLockedBytes(readOnly: true) { base, rowBytes -> CGImage? in
            CGContext(
                data: base, width: size.width, height: size.height, bitsPerComponent: 8, bytesPerRow: rowBytes,
                space: CGColorSpace(name: CGColorSpace.sRGB)!,
                bitmapInfo: CGImageAlphaInfo.premultipliedFirst.rawValue | CGBitmapInfo.byteOrder32Little.rawValue)?
                .makeImage()
        }
        let url = FileManager.default.temporaryDirectory.appendingPathComponent(name)
        guard let image = image ?? nil,
            let destination = CGImageDestinationCreateWithURL(url as CFURL, UTType.png.identifier as CFString, 1, nil)
        else { return }
        CGImageDestinationAddImage(destination, image, nil)
        CGImageDestinationFinalize(destination)
        print("wrote \(url.path)")
    }

    func testSwapIsDrawnOnTheFaceAndNowhereElse() throws {
        let packs = try packs()
        let photo = try samplePhoto()
        let renderer = try MetalRenderer()

        // The identity to swap in: the largest face in the photo.
        let enrolment = try FaceEnrolment(packFolders: [packs])
        let enrolled = try XCTUnwrap(try enrolment.enrol(photo))
        XCTAssertEqual(enrolled.identity.count, 512)
        XCTAssertEqual(enrolled.facesInPhoto, 6)

        // One video frame: the left third of the photo, enlarged, so one face fills it like a webcam picture.
        let frameSize = PixelSize(width: 1280, height: 720)
        let pool = try XCTUnwrap(PixelBufferPool(size: frameSize))
        let frame = try XCTUnwrap(pool.makeBuffer())
        _ = frame.withLockedBytes(readOnly: false) { base, rowBytes in
            let context = CGContext(
                data: base, width: frameSize.width, height: frameSize.height, bitsPerComponent: 8,
                bytesPerRow: rowBytes, space: CGColorSpace(name: CGColorSpace.sRGB)!,
                bitmapInfo: CGImageAlphaInfo.premultipliedFirst.rawValue | CGBitmapInfo.byteOrder32Little.rawValue)
            // Show the photo's top-left 427 x 240 points at three times the size.
            context?.interpolationQuality = .high
            context?.draw(
                photo,
                in: CGRect(
                    x: 0, y: CGFloat(frameSize.height) - CGFloat(photo.height) * 3 + 420,
                    width: CGFloat(photo.width) * 3, height: CGFloat(photo.height) * 3))
        }

        var config = FaceSwapConfig(packFolders: [packs])
        config.cacheFolder = FileManager.default.temporaryDirectory.appendingPathComponent("gc-test-model-cache")
        let stage = try FaceSwapStage(config: config, renderer: renderer)
        XCTAssertEqual(stage.patchSize, 128)
        stage.setIdentity(enrolled.identity)

        // Feed the same frame at camera pace until the swapped face has faded in.
        var patch: RenderPatch?
        var time: UInt64 = 1_000_000
        for _ in 0..<240 {
            patch = stage.process(frame, hostTimeNs: time).patch
            time += 33_333_333
            if let patch, patch.opacity >= 0.99 { break }
            Thread.sleep(forTimeInterval: 0.033)
        }
        let stats = stage.stats
        XCTAssertNil(stats.lastError)
        let drawn = try XCTUnwrap(patch, "no swapped face after eight seconds")
        XCTAssertGreaterThanOrEqual(drawn.opacity, 0.99)

        let plain = try XCTUnwrap(pool.makeBuffer())
        let swapped = try XCTUnwrap(pool.makeBuffer())
        try renderer.render(primary: RenderLayer(buffer: frame), into: plain)
        try renderer.render(primary: RenderLayer(buffer: frame), patch: drawn, into: swapped)
        savePNG(swapped, to: "gc-swap-test.png")

        // Count changed pixels in a grid: many where the face is, none far from it.
        let changed = plain.withLockedBytes(readOnly: true) { a, rowBytes -> [Bool] in
            swapped.withLockedBytes(readOnly: true) { b, _ -> [Bool] in
                var cells: [Bool] = []
                for y in stride(from: 8, to: frameSize.height, by: 16) {
                    for x in stride(from: 8, to: frameSize.width, by: 16) {
                        let p = y * rowBytes + x * 4
                        let pa = a.advanced(by: p).assumingMemoryBound(to: UInt8.self)
                        let pb = b.advanced(by: p).assumingMemoryBound(to: UInt8.self)
                        cells.append(abs(Int(pa[0]) - Int(pb[0])) + abs(Int(pa[1]) - Int(pb[1])) + abs(Int(pa[2]) - Int(pb[2])) > 12)
                    }
                }
                return cells
            } ?? []
        } ?? []
        let columns = frameSize.width / 16
        let changedCount = changed.filter { $0 }.count
        XCTAssertGreaterThan(changedCount, 40, "the swap changed almost nothing")
        XCTAssertLessThan(changedCount, changed.count / 4, "the swap changed far more than a face")
        // The changed cells sit together: their bounding box is small compared with the frame.
        let indices = changed.indices.filter { changed[$0] }
        let xs = indices.map { $0 % columns }
        let ys = indices.map { $0 / columns }
        let boxWidth = (xs.max()! - xs.min()! + 1) * 16
        let boxHeight = (ys.max()! - ys.min()! + 1) * 16
        XCTAssertLessThan(boxWidth, 520)
        XCTAssertLessThan(boxHeight, 560)
        print("swap \(stats.swapMs) ms, age \(stats.swapAgeMs) ms, changed area about \(boxWidth) x \(boxHeight) px")

        // With no identity the stage draws nothing.
        stage.setIdentity(nil)
        XCTAssertNil(stage.process(frame, hostTimeNs: time).patch)

        // With the blur option on and a face chosen, a picture without a face is
        // hidden; with the face in it, the swap comes back and the picture clears.
        var hiding = FaceSwapSettings()
        hiding.blurWhenUnswapped = true
        stage.update(hiding)
        XCTAssertEqual(stage.process(frame, hostTimeNs: time).blur, 0, "no face chosen, nothing to hide")
        stage.setIdentity(enrolled.identity)
        let empty = try XCTUnwrap(pool.makeBuffer())
        _ = empty.withLockedBytes(readOnly: false) { base, rowBytes in
            memset(base, 96, rowBytes * frameSize.height)
        }
        time += 33_333_333
        XCTAssertEqual(stage.process(empty, hostTimeNs: time).blur, 1)
        var cleared = false
        for _ in 0..<240 where !cleared {
            time += 33_333_333
            let output = stage.process(frame, hostTimeNs: time)
            cleared = output.blur == 0 && (output.patch?.opacity ?? 0) >= 0.99
            Thread.sleep(forTimeInterval: 0.033)
        }
        XCTAssertTrue(cleared, "the swap came back on the face and the blur went")

        // Several photos of one person make one identity; a photo of somebody else is left out.
        // The largest face of the photo was enrolled above; this corner of it shows only another person.
        let corner = try XCTUnwrap(photo.cropping(to: CGRect(x: 840, y: 0, width: 260, height: 290)))
        let other = try XCTUnwrap(try enrolment.enrol(corner))
        XCTAssertEqual(other.facesInPhoto, 1)
        let combined = FaceEnrolment.combine([enrolled.identity, other.identity, enrolled.identity])
        XCTAssertEqual(combined.used, [true, false, true])
        XCTAssertEqual(combined.identity.count, 512)
        XCTAssertGreaterThan(combined.similarity[0], 0.99)
        XCTAssertLessThan(combined.similarity[1], 0.3)
        XCTAssertTrue(stage.canKeepObjectsInFront, "the occlusion model is in the test packs")
    }
}
