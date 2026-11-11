import CoreGraphics
import CoreVideo
import CryptoKit
import ImageIO
import UniformTypeIdentifiers
import XCTest

@testable import GodsCameraKit

/// A solid colour, as blue, green, red.
private typealias BGR = (UInt8, UInt8, UInt8)

private func makeBuffer(_ size: PixelSize, fill: (Int, Int) -> BGR) -> CVPixelBuffer {
    let buffer = PixelBufferPool(size: size)!.makeBuffer()!
    _ = buffer.withLockedBytes(readOnly: false) { base, rowBytes in
        for y in 0..<size.height {
            let row = base.advanced(by: y * rowBytes).assumingMemoryBound(to: UInt8.self)
            for x in 0..<size.width {
                let (b, g, r) = fill(x, y)
                row[x * 4] = b
                row[x * 4 + 1] = g
                row[x * 4 + 2] = r
                row[x * 4 + 3] = 255
            }
        }
    }
    return buffer
}

private func pixel(_ buffer: CVPixelBuffer, x: Int, y: Int) -> BGR {
    buffer.withLockedBytes(readOnly: true) { base, rowBytes in
        let p = base.advanced(by: y * rowBytes + x * 4).assumingMemoryBound(to: UInt8.self)
        return (p[0], p[1], p[2])
    }!
}

private func assertColor(
    _ actual: BGR, _ expected: BGR, tolerance: Int = 3, _ message: String = "", file: StaticString = #filePath,
    line: UInt = #line
) {
    let close =
        abs(Int(actual.0) - Int(expected.0)) <= tolerance && abs(Int(actual.1) - Int(expected.1)) <= tolerance
        && abs(Int(actual.2) - Int(expected.2)) <= tolerance
    XCTAssertTrue(close, "\(message) got \(actual), expected \(expected)", file: file, line: line)
}

/// Collects what the engine sends out.
private final class Collector: FrameSink {
    private let lock = NSLock()
    private var numbers: [UInt32?] = []
    private var sizes: [PixelSize] = []

    func send(_ pixelBuffer: CVPixelBuffer, hostTimeNs: UInt64) {
        let number = TestPatternSource.readFrameNumber(from: pixelBuffer)
        lock.lock()
        numbers.append(number)
        sizes.append(pixelBuffer.pixelSize)
        lock.unlock()
    }

    var frameNumbers: [UInt32?] {
        lock.lock()
        defer { lock.unlock() }
        return numbers
    }

    var count: Int { frameNumbers.count }
    var lastSize: PixelSize? {
        lock.lock()
        defer { lock.unlock() }
        return sizes.last
    }
}

final class PipelineTests: XCTestCase {
    private let red: BGR = (0, 0, 255)
    private let blue: BGR = (255, 0, 0)

    /// Wait until `condition` holds, failing the test if it never does.
    private func waitUntil(
        _ what: String, timeout: TimeInterval = 10, file: StaticString = #filePath, line: UInt = #line,
        _ condition: () -> Bool
    ) {
        let deadline = Date().addingTimeInterval(timeout)
        while !condition() {
            if Date() > deadline {
                XCTFail("timed out waiting for \(what)", file: file, line: line)
                return
            }
            RunLoop.current.run(until: Date().addingTimeInterval(0.01))
        }
    }

    // MARK: Renderer

    func testRendererScalesAndMirrors() throws {
        let renderer = try MetalRenderer()
        // Left half red, right half blue.
        let source = makeBuffer(PixelSize(width: 640, height: 360)) { x, _ in x < 320 ? self.red : self.blue }
        let output = PixelBufferPool(size: .hd720)!.makeBuffer()!

        try renderer.render(primary: RenderLayer(buffer: source), into: output)
        assertColor(pixel(output, x: 100, y: 360), red, "left stays red")
        assertColor(pixel(output, x: 1180, y: 360), blue, "right stays blue")

        try renderer.render(primary: RenderLayer(buffer: source, mirrored: true), into: output)
        assertColor(pixel(output, x: 100, y: 360), blue, "mirrored: left is now blue")
        assertColor(pixel(output, x: 1180, y: 360), red, "mirrored: right is now red")
    }

    func testRendererCropsToFillInsteadOfStretching() throws {
        let renderer = try MetalRenderer()
        // A 4:3 picture with a green band across the top and bottom tenth.
        let green: BGR = (0, 255, 0)
        let source = makeBuffer(PixelSize(width: 800, height: 600)) { _, y in
            y < 60 || y >= 540 ? green : self.red
        }
        let output = PixelBufferPool(size: .hd720)!.makeBuffer()!
        try renderer.render(primary: RenderLayer(buffer: source), into: output)
        // Filling 16:9 from 4:3 keeps the width and cuts 75 source rows from the
        // top and bottom, which removes both green bands entirely.
        assertColor(pixel(output, x: 640, y: 4), red, "top band is cropped away")
        assertColor(pixel(output, x: 640, y: 715), red, "bottom band is cropped away")
    }

    func testRendererBlendsTwoPictures() throws {
        let renderer = try MetalRenderer()
        let size = PixelSize(width: 320, height: 180)
        let first = makeBuffer(size) { _, _ in (0, 0, 200) }
        let second = makeBuffer(size) { _, _ in (200, 0, 0) }
        let output = PixelBufferPool(size: size)!.makeBuffer()!
        try renderer.render(primary: RenderLayer(buffer: first), secondary: RenderLayer(buffer: second), mix: 0.25, into: output)
        assertColor(pixel(output, x: 160, y: 90), (50, 0, 150), "a quarter of the way to the second picture")
    }

    func testRendererDrawsAnOverlayOnlyInsideItsRectangle() throws {
        let renderer = try MetalRenderer()
        let size = PixelSize(width: 640, height: 360)
        let source = makeBuffer(size) { _, _ in (0, 0, 0) }
        let badge = try XCTUnwrap(renderer.makeBadge(text: "LOOP", color: CGColor(red: 1, green: 0, blue: 0, alpha: 1)))
        let output = PixelBufferPool(size: size)!.makeBuffer()!
        let overlay = RenderOverlay(texture: badge, rect: SIMD4<Float>(0.1, 0.1, 0.3, 0.2))
        try renderer.render(primary: RenderLayer(buffer: source), overlay: overlay, into: output)
        // Just inside the badge's left edge, halfway down, the red background shows.
        let inside = pixel(output, x: Int(0.11 * 640), y: Int(0.2 * 360))
        XCTAssertGreaterThan(inside.2, 200, "badge background should be red, got \(inside)")
        assertColor(pixel(output, x: 600, y: 300), (0, 0, 0), "outside the badge nothing changes")
    }

    // MARK: Test pattern

    func testPatternFrameNumberSurvivesScaling() throws {
        let renderer = try MetalRenderer()
        let source = PixelBufferPool(size: .hd1080)!.makeBuffer()!
        let output = PixelBufferPool(size: .hd720)!.makeBuffer()!
        for number: UInt32 in [0, 1, 255, 4660, 65535] {
            TestPatternSource.draw(frame: number, into: source)
            XCTAssertEqual(TestPatternSource.readFrameNumber(from: source), number)
            try renderer.render(primary: RenderLayer(buffer: source), into: output)
            XCTAssertEqual(TestPatternSource.readFrameNumber(from: output), number, "after scaling to 720p")
        }
    }

    // MARK: Sources

    func testStillImageSourceDeliversThePicture() throws {
        let url = FileManager.default.temporaryDirectory.appendingPathComponent("gc-still-\(UUID().uuidString).png")
        defer { try? FileManager.default.removeItem(at: url) }
        let context = CGContext(
            data: nil, width: 400, height: 300, bitsPerComponent: 8, bytesPerRow: 1600,
            space: CGColorSpace(name: CGColorSpace.sRGB)!, bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)!
        context.setFillColor(CGColor(srgbRed: 0, green: 0, blue: 1, alpha: 1))
        context.fill(CGRect(x: 0, y: 0, width: 400, height: 300))
        let destination = try XCTUnwrap(
            CGImageDestinationCreateWithURL(url as CFURL, UTType.png.identifier as CFString, 1, nil))
        CGImageDestinationAddImage(destination, context.makeImage()!, nil)
        XCTAssertTrue(CGImageDestinationFinalize(destination))

        let source = try StillImageSource(url: url, frameRate: 60)
        let received = expectation(description: "a frame arrives")
        received.assertForOverFulfill = false
        var size: PixelSize?
        var colour: BGR?
        source.onFrame = { frame in
            size = frame.pixelBuffer.pixelSize
            colour = pixel(frame.pixelBuffer, x: 200, y: 150)
            received.fulfill()
        }
        try source.start()
        wait(for: [received], timeout: 5)
        source.stop()
        XCTAssertEqual(size, PixelSize(width: 400, height: 300))
        assertColor(try XCTUnwrap(colour), blue, tolerance: 8, "the picture is blue")

        XCTAssertThrowsError(try StillImageSource(url: URL(fileURLWithPath: "/nonexistent.png")))
    }

    // MARK: Clips

    private func recordPattern(frames: Int, size: PixelSize = .hd720) throws -> Clip {
        let recorder = try ClipRecorder(size: size, frameRate: 30, maxSeconds: 30)
        let pool = PixelBufferPool(size: size)!
        for number in 0..<frames {
            let buffer = pool.makeBuffer()!
            TestPatternSource.draw(frame: UInt32(number), into: buffer)
            recorder.append(buffer)
        }
        return try recorder.finish(name: "test")
    }

    func testClipFramesDecodeInAnyOrder() throws {
        let clip = try recordPattern(frames: 45)
        XCTAssertEqual(clip.frameCount, 45)
        XCTAssertEqual(clip.thumbnails.count, 45)
        XCTAssertEqual(clip.duration, 1.5, accuracy: 0.001)
        XCTAssertGreaterThan(clip.meanLuma, 30)

        let decoder = try ClipDecoder(clip: clip)
        // Forwards, backwards and repeated: every frame stands on its own.
        for index in [0, 17, 44, 16, 3, 17] {
            let frame = try XCTUnwrap(decoder.frame(at: index), "frame \(index)")
            XCTAssertEqual(frame.pixelSize, .hd720)
            XCTAssertEqual(TestPatternSource.readFrameNumber(from: frame), UInt32(index))
        }
        XCTAssertNil(decoder.frame(at: 45))
        XCTAssertNil(decoder.frame(at: -1))
    }

    func testRecorderStopsAtItsLimit() throws {
        let size = PixelSize(width: 640, height: 360)
        let recorder = try ClipRecorder(size: size, frameRate: 30, maxSeconds: 0.5)
        let pool = PixelBufferPool(size: size)!
        var accepted = 0
        for number in 0..<40 {
            let buffer = pool.makeBuffer()!
            TestPatternSource.draw(frame: UInt32(number), into: buffer)
            if recorder.frameCount < recorder.maxFrames {
                recorder.append(buffer)
                accepted += 1
            } else {
                XCTAssertFalse(recorder.append(buffer))
            }
        }
        XCTAssertEqual(accepted, 15)
        XCTAssertEqual(try recorder.finish(name: "limit").frameCount, 15)
    }

    func testLibrarySavesAndLoadsAClipExactly() throws {
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent("gc-library-\(UUID().uuidString)")
        defer { try? FileManager.default.removeItem(at: directory) }
        let library = try ClipLibrary(directory: directory)
        let clip = try recordPattern(frames: 40, size: PixelSize(width: 640, height: 360))
        clip.name = "Desk, afternoon"
        clip.mode = .pingPong
        try library.save(clip)

        let listed = library.list()
        XCTAssertEqual(listed.count, 1)
        XCTAssertEqual(listed.first?.name, "Desk, afternoon")
        XCTAssertEqual(listed.first?.duration ?? 0, clip.duration, accuracy: 0.001)

        let loaded = try library.load(clip.id)
        XCTAssertEqual(loaded.frameCount, 40)
        XCTAssertEqual(loaded.thumbnails, clip.thumbnails)
        XCTAssertEqual(loaded.loop, clip.loop)
        XCTAssertEqual(loaded.mode, .pingPong)
        let decoder = try ClipDecoder(clip: loaded)
        for index in [0, 39, 20] {
            XCTAssertEqual(TestPatternSource.readFrameNumber(from: try XCTUnwrap(decoder.frame(at: index))), UInt32(index))
        }

        loaded.name = "Renamed"
        try library.update(loaded)
        XCTAssertEqual(library.list().first?.name, "Renamed")

        library.delete(clip.id)
        XCTAssertTrue(library.list().isEmpty)
        XCTAssertThrowsError(try library.load(clip.id))
    }

    // MARK: Engine

    func testEngineGoesLiveRecordsLoopsAndReturns() throws {
        let engine = CameraEngine(renderer: try MetalRenderer())
        let collector = Collector()
        engine.sink = collector
        var latest = CameraEngine.Status()
        var clips: [Clip] = []
        let lock = NSLock()
        engine.onStatus = { status in
            lock.lock()
            latest = status
            lock.unlock()
        }
        engine.onClip = { clip in
            lock.lock()
            clips.append(clip)
            lock.unlock()
        }
        func status() -> CameraEngine.Status {
            lock.lock()
            defer { lock.unlock() }
            return latest
        }
        engine.update { $0.transitionFrames = 4 }

        // Live: the output carries the source's frame numbers, in order.
        let source = try XCTUnwrap(TestPatternSource(size: .hd1080, frameRate: 60))
        engine.setSource(source)
        waitUntil("live frames") { collector.count >= 20 }
        XCTAssertEqual(status().mode, .live)
        XCTAssertEqual(collector.lastSize, .hd720, "the 1080p source is scaled to the 720p output")
        let live = collector.frameNumbers.compactMap { $0 }
        XCTAssertEqual(live, live.sorted(), "live frames arrive in order")

        // Record about a second.
        engine.startRecording()
        waitUntil("recording to start") { status().isRecording }
        let before = collector.count
        waitUntil("a second of recording") { collector.count >= before + 45 }
        engine.stopRecording()
        waitUntil("the clip") {
            lock.lock()
            defer { lock.unlock() }
            return !clips.isEmpty
        }
        let clip = clips[0]
        XCTAssertGreaterThanOrEqual(clip.frameCount, 40)
        XCTAssertFalse(status().isRecording)

        // The frame numbers that are inside the loop, read back from the clip itself.
        let decoder = try ClipDecoder(clip: clip)
        let loopNumbers = Set(
            (clip.loop.start..<clip.loop.end).compactMap { index in
                decoder.frame(at: index).flatMap { TestPatternSource.readFrameNumber(from: $0) }
            })
        XCTAssertFalse(loopNumbers.isEmpty)

        // Loop: after the fade-in, every output frame comes from the loop range.
        engine.startLoop(clip)
        waitUntil("looping") { status().mode == .looping }
        engine.sync()
        let loopStart = collector.count
        waitUntil("two passes of the loop") { collector.count >= loopStart + clip.loop.length * 2 + 8 }
        let looped = collector.frameNumbers[(loopStart + 6)...].compactMap { $0 }
        XCTAssertGreaterThan(looped.count, clip.loop.length)
        let outside = looped.filter { !loopNumbers.contains($0) }
        // Cross-faded frames blend two pictures, so a few numbers may be unreadable or mixed.
        XCTAssertLessThan(outside.count, looped.count / 4, "frames outside the loop: \(outside)")
        let newestLive = try XCTUnwrap(live.max())
        XCTAssertTrue(looped.contains { $0 > newestLive }, "the loop shows recorded frames")

        // Back to live: the frame numbers jump past everything recorded.
        engine.stopLoop()
        waitUntil("live again") { status().mode == .live }
        let resumeAt = collector.count
        waitUntil("more live frames") { collector.count >= resumeAt + 10 }
        let afterLoop = collector.frameNumbers[(resumeAt + 2)...].compactMap { $0 }
        let newestRecorded = try XCTUnwrap(loopNumbers.max())
        XCTAssertTrue(afterLoop.allSatisfy { $0 > newestRecorded }, "live frames are newer than the clip: \(afterLoop.prefix(5))")

        engine.setSource(nil)
        engine.sync()
    }

    func testEngineIgnoresLoopAndRecordRequestsThatCannotApply() throws {
        let engine = CameraEngine(renderer: try MetalRenderer())
        var statuses: [CameraEngine.Status] = []
        engine.onStatus = { statuses.append($0) }
        // No source yet: recording cannot start, and stopping a loop does nothing.
        engine.startRecording()
        engine.stopRecording()
        engine.stopLoop()
        engine.sync()
        XCTAssertTrue(statuses.allSatisfy { !$0.isRecording && $0.mode == .idle })
    }
}

final class CameraStatsTests: XCTestCase {
    func testStatsRoundTripWithClients() {
        var stats = VirtualCameraStats()
        stats.framesIn = 120
        stats.framesOut = 118
        stats.slateFrames = 3
        stats.clients = 2
        stats.latencyMs = 4.25
        stats.maxLatencyMs = 9.5
        stats.width = 1920
        stats.height = 1080
        stats.frameRate = 60
        stats.pid = 4242
        stats.build = "3"
        stats.writerVerified = true
        stats.connected = [100, 200, 300]
        stats.clientList = [
            CameraClient(signingID: "us.zoom.xos", pid: 100), CameraClient(signingID: "com.google.Chrome.helper", pid: 200),
        ]
        XCTAssertEqual(VirtualCameraStats(encoded: stats.encoded), stats)
    }

    func testStatsFromAnOlderExtensionStillRead() {
        let old = "in=5;out=4;slate=1;clients=1;lat=2.00;max=3.00"
        let stats = VirtualCameraStats(encoded: old)
        XCTAssertEqual(stats?.framesIn, 5)
        XCTAssertEqual(stats?.width, 1280)
        XCTAssertEqual(stats?.build, "")
        XCTAssertEqual(stats?.clientList, [])
        XCTAssertNil(VirtualCameraStats(encoded: "garbage"))
    }

    func testPropertySelectorsAreFourCharacterCodes() {
        XCTAssertEqual(CameraConfig.Property.stats.selector, 0x6763_7374)
        XCTAssertEqual(CameraConfig.Property.stats.key, "4cc_gcst_glob_0000")
        XCTAssertEqual(Set(CameraConfig.Property.allCases.map(\.selector)).count, CameraConfig.Property.allCases.count)
    }
}

final class VoiceActivityTests: XCTestCase {
    func testSpeechMustLastBeforeItCounts() {
        var detector = VoiceActivityDetector(thresholdDb: -30, holdSeconds: 0.25)
        // A click: loud for 40 ms, then quiet.
        XCTAssertFalse(detector.feed(levelDb: -10, seconds: 0.04))
        XCTAssertFalse(detector.feed(levelDb: -60, seconds: 0.1))
        // Speech: loud for 300 ms in 20 ms blocks.
        var fired = false
        for _ in 0..<15 where !fired { fired = detector.feed(levelDb: -20, seconds: 0.02) }
        XCTAssertTrue(fired)
    }

    func testScatteredNoiseDoesNotAddUp() {
        var detector = VoiceActivityDetector(thresholdDb: -30, holdSeconds: 0.25)
        for _ in 0..<50 {
            XCTAssertFalse(detector.feed(levelDb: -10, seconds: 0.02))
            XCTAssertFalse(detector.feed(levelDb: -70, seconds: 0.02))
        }
    }

    func testLevelOfSamples() {
        let silence = [Float](repeating: 0, count: 256)
        let full = [Float](repeating: 1, count: 256)
        let half = [Float](repeating: 0.5, count: 256)
        silence.withUnsafeBufferPointer { XCTAssertEqual(VoiceActivityDetector.level(of: $0), -120) }
        full.withUnsafeBufferPointer { XCTAssertEqual(VoiceActivityDetector.level(of: $0), 0, accuracy: 0.01) }
        half.withUnsafeBufferPointer { XCTAssertEqual(VoiceActivityDetector.level(of: $0), -6.02, accuracy: 0.01) }
    }
}

final class AppIdentityTests: XCTestCase {
    func testProcessInsideAnAppBelongsToThatApp() {
        let app = AppIdentity.of(executablePath: "/System/Applications/Calculator.app/Contents/MacOS/Calculator")
        XCTAssertEqual(app.id, "com.apple.calculator")
        XCTAssertEqual(app.name, "Calculator")
    }

    func testHelperBelongsToTheOutermostApp() {
        // Safari's helpers live inside the app; the user knows them as Safari.
        let helper = AppIdentity.of(
            executablePath: "/System/Applications/Calculator.app/Contents/PlugIns/Example.app/Contents/MacOS/Example")
        XCTAssertEqual(helper.id, "com.apple.calculator")
    }

    func testToolOutsideAnAppIsKnownByItsPath() {
        let tool = AppIdentity.of(executablePath: "/usr/bin/true")
        XCTAssertEqual(tool.id, "/usr/bin/true")
        XCTAssertEqual(tool.name, "true")
    }

    func testThisProcessCanBeIdentified() {
        XCTAssertNotNil(AppIdentity.of(pid: getpid()))
        XCTAssertNil(AppIdentity.of(pid: 0))
    }
}

final class VideoFileSourceTests: XCTestCase {
    /// Plays a one-second clip made with ffmpeg and checks that pictures arrive. Skipped where ffmpeg is missing.
    func testVideoFilePlaysAndLoops() throws {
        let candidates = ["/opt/homebrew/bin/ffmpeg", "/usr/local/bin/ffmpeg"]
        guard let ffmpeg = candidates.first(where: { FileManager.default.isExecutableFile(atPath: $0) }) else {
            throw XCTSkip("ffmpeg is not installed")
        }
        let url = FileManager.default.temporaryDirectory.appendingPathComponent("gc-test-\(UUID().uuidString).mp4")
        defer { try? FileManager.default.removeItem(at: url) }
        let make = Process()
        make.executableURL = URL(fileURLWithPath: ffmpeg)
        make.arguments = [
            "-loglevel", "error", "-f", "lavfi", "-i", "testsrc=size=640x360:rate=30:duration=1",
            "-pix_fmt", "yuv420p", "-c:v", "h264_videotoolbox", "-y", url.path,
        ]
        try make.run()
        make.waitUntilExit()
        guard make.terminationStatus == 0 else { throw XCTSkip("ffmpeg could not make the test clip") }

        let source = try VideoFileSource(url: url)
        let lock = NSLock()
        var sizes: [PixelSize] = []
        source.onFrame = { frame in
            lock.lock()
            sizes.append(frame.pixelBuffer.pixelSize)
            lock.unlock()
        }
        try source.start()
        // Two and a half seconds of a one-second clip: the second half only arrives if the clip loops.
        let done = expectation(description: "played")
        DispatchQueue.main.asyncAfter(deadline: .now() + 2.5) { done.fulfill() }
        wait(for: [done], timeout: 5)
        source.stop()
        lock.lock()
        let received = sizes
        lock.unlock()
        XCTAssertGreaterThan(received.count, 40, "expected the clip to keep playing past its end")
        XCTAssertEqual(received.first, PixelSize(width: 640, height: 360))
    }
}

final class FaceLibraryTests: XCTestCase {
    private func temporaryFolder() -> URL {
        FileManager.default.temporaryDirectory.appendingPathComponent("gc-faces-\(UUID().uuidString)")
    }

    private func sampleFace(_ name: String) -> SavedFace {
        SavedFace(
            name: name, consent: .myself, identity: (0..<512).map { Float($0) * 0.01 }, thumbnail: Data([1, 2, 3]))
    }

    func testFacesRoundTripAndAreNotReadableOnDisk() throws {
        let folder = temporaryFolder()
        defer { try? FileManager.default.removeItem(at: folder) }
        let library = try FaceLibrary(directory: folder, key: SymmetricKey(size: .bits256))
        let face = sampleFace("Somebody Recognisable")
        try library.save(face)
        XCTAssertEqual(library.list(), [face])
        XCTAssertEqual(try library.load(face.id), face)

        // Neither the name nor the numbers appear in the file.
        let raw = try Data(contentsOf: folder.appendingPathComponent("\(face.id.uuidString).face"))
        XCTAssertNil(raw.range(of: Data("Somebody".utf8)))
        XCTAssertNil(raw.range(of: Data("identity".utf8)))
    }

    func testAnotherKeyCannotOpenTheFiles() throws {
        let folder = temporaryFolder()
        defer { try? FileManager.default.removeItem(at: folder) }
        let face = sampleFace("A")
        try FaceLibrary(directory: folder, key: SymmetricKey(size: .bits256)).save(face)
        let other = try FaceLibrary(directory: folder, key: SymmetricKey(size: .bits256))
        XCTAssertTrue(other.list().isEmpty)
        XCTAssertThrowsError(try other.load(face.id))
    }

    func testAFileCannotBePassedOffAsAnotherFace() throws {
        let folder = temporaryFolder()
        defer { try? FileManager.default.removeItem(at: folder) }
        let library = try FaceLibrary(directory: folder, key: SymmetricKey(size: .bits256))
        let (a, b) = (sampleFace("A"), sampleFace("B"))
        try library.save(a)
        try library.save(b)
        // Put A's file where B's is.
        let fileA = folder.appendingPathComponent("\(a.id.uuidString).face")
        let fileB = folder.appendingPathComponent("\(b.id.uuidString).face")
        try FileManager.default.removeItem(at: fileB)
        try FileManager.default.copyItem(at: fileA, to: fileB)
        XCTAssertThrowsError(try library.load(b.id))
        XCTAssertEqual(library.list(), [a])
    }

    func testDeleteAllRemovesEveryFace() throws {
        let folder = temporaryFolder()
        defer { try? FileManager.default.removeItem(at: folder) }
        let library = try FaceLibrary(directory: folder, key: SymmetricKey(size: .bits256))
        try library.save(sampleFace("A"))
        try library.save(sampleFace("B"))
        library.delete(library.list()[0].id)
        XCTAssertEqual(library.list().count, 1)
        library.deleteAll()
        XCTAssertTrue(library.list().isEmpty)
    }
}

final class ScanHubTests: XCTestCase {
    /// A stand-in for the text reader: one page per picture, each with the same line.
    private func fakeReader(in folder: URL) throws -> URL {
        let script = folder.appendingPathComponent("reader.sh")
        let body = """
            #!/bin/sh
            printf '['
            first=1
            for f in "$@"; do
              [ $first = 1 ] || printf ','
              first=0
              printf '{"path":"%s","width":1000,"height":1000,"lines":[{"text":"Dòng chữ của trang báo","confidence":0.9,"quad":[100,100,600,100,600,140,100,140],"title":false}]}' "$f"
            done
            printf ']'
            """
        try body.write(to: script, atomically: true, encoding: .utf8)
        try FileManager.default.setAttributes([.posixPermissions: 0o755], ofItemAtPath: script.path)
        return script
    }

    private func call(_ method: String, _ url: String, token: String, body: Data? = nil) async throws -> (Int, Data) {
        var request = URLRequest(url: URL(string: url)!)
        request.httpMethod = method
        request.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization")
        if body != nil { request.setValue("application/json", forHTTPHeaderField: "Content-Type") }
        request.httpBody = body
        let (data, response) = try await URLSession.shared.data(for: request)
        return ((response as? HTTPURLResponse)?.statusCode ?? 0, data)
    }

    func testHubTakesPicturesAndListsTheDocument() async throws {
        let folder = FileManager.default.temporaryDirectory.appendingPathComponent("gc-hub-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: folder) }
        let hub = ScanHub(dataFolder: folder.appendingPathComponent("data"), reader: try fakeReader(in: folder))
        let token = "test-token-0123456789"
        try hub.start(port: 0, token: token)
        defer { hub.stop() }
        let base = try XCTUnwrap(hub.address)
        XCTAssertTrue(base.hasPrefix("http://127.0.0.1:"))
        XCTAssertTrue(hub.documents().isEmpty)

        // What the phone does: make an upload, send two pictures, start a document job.
        let (created, uploadData) = try await call("POST", "\(base)/v1/uploads", token: token)
        XCTAssertEqual(created, 200)
        let upload = try XCTUnwrap((try JSONSerialization.jsonObject(with: uploadData) as? [String: Any])?["id"] as? String)
        for name in ["page-001.jpg", "page-002.jpg"] {
            let (sent, _) = try await call(
                "PUT", "\(base)/v1/uploads/\(upload)/files/\(name)?offset=0", token: token, body: Data("picture".utf8))
            XCTAssertEqual(sent, 200)
        }
        let request = try JSONSerialization.data(withJSONObject: ["kind": "document", "upload_id": upload])
        let (started, _) = try await call("POST", "\(base)/v1/jobs", token: token, body: request)
        XCTAssertEqual(started, 200)

        var document: ScannedDocument?
        for _ in 0..<100 {
            document = hub.documents().first
            if document?.isDone == true || document?.status == "failed" { break }
            try await Task.sleep(nanoseconds: 50_000_000)
        }
        let done = try XCTUnwrap(document)
        XCTAssertEqual(done.status, "done", done.error ?? "")
        XCTAssertEqual(done.pictures, 2)
        XCTAssertEqual(done.title, "Dòng chữ của trang báo")
        XCTAssertTrue(FileManager.default.fileExists(atPath: done.markdown.path))

        // Without the token the hub answers nothing useful.
        let (refused, _) = try await call("POST", "\(base)/v1/uploads", token: "wrong-token-000000000")
        XCTAssertEqual(refused, 401)

        hub.delete(done)
        XCTAssertTrue(hub.documents().isEmpty)
        hub.stop()
        XCTAssertFalse(hub.isRunning)
    }
}

final class DeviceConnectorTests: XCTestCase {
    func testDeviceListIsReadWithItsStates() {
        let listing = """
            List of devices attached
            R52W70ABCDE\tdevice
            adb-R52W70ABCDE-t9yuQe._adb-tls-connect._tcp\tdevice
            emulator-5554\toffline
            0123456789\tunauthorized

            """
        let (ready, waiting) = DeviceConnector.parseDevices(listing)
        XCTAssertEqual(ready, ["R52W70ABCDE", "adb-R52W70ABCDE-t9yuQe._adb-tls-connect._tcp"])
        XCTAssertEqual(waiting, 2)
        XCTAssertEqual(DeviceConnector.parseDevices("List of devices attached\n").ready, [])
    }
}
