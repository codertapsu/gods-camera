import XCTest

@testable import GodsCameraKit

final class LoopingTests: XCTestCase {
    /// Thumbnails for a scene that repeats exactly every `period` frames.
    private func periodicThumbnails(count: Int, period: Int) -> [[UInt8]] {
        (0..<count).map { frame in
            let phase = Double(frame % period) / Double(period) * 2 * .pi
            return (0..<Thumbnail.count).map { cell in
                UInt8(128 + 100 * sin(phase + Double(cell) * 0.37))
            }
        }
    }

    func testFinderPicksAWholeNumberOfPeriods() {
        let period = 20
        let thumbnails = periodicThumbnails(count: 150, period: period)
        let loop = LoopFinder.find(thumbnails: thumbnails, minimumLength: 30, crossfade: 8)
        XCTAssertEqual(loop.length % period, 0, "loop of \(loop.length) frames is not a whole number of periods")
        XCTAssertGreaterThanOrEqual(loop.length, 30)
        XCTAssertGreaterThanOrEqual(loop.start, loop.crossfade, "the fade needs frames before the start")
        XCTAssertLessThanOrEqual(loop.end, thumbnails.count)
    }

    func testFinderSkipsADisturbanceAtTheStartAndTheEnd() {
        // The first 15 and the last 10 frames flicker, as when a hand reaches for
        // the Record and Stop buttons; the frames between are perfectly still.
        var thumbnails: [[UInt8]] = []
        for frame in 0..<150 {
            let disturbed = frame < 15 || frame >= 140
            let level: UInt8 = disturbed ? (frame % 2 == 0 ? 40 : 200) : 120
            thumbnails.append([UInt8](repeating: level, count: Thumbnail.count))
        }
        let loop = LoopFinder.find(thumbnails: thumbnails, minimumLength: 30, crossfade: 8)
        XCTAssertGreaterThanOrEqual(loop.start, 15, "the loop should start after the disturbance")
        XCTAssertLessThanOrEqual(loop.end, 140, "the loop should end before the disturbance")
    }

    func testSeamMismatchTellsAMatchingEndFromAMovedOne() {
        func flat(_ level: UInt8) -> [UInt8] { [UInt8](repeating: level, count: Thumbnail.count) }
        // The picture ends as it began.
        let steady = (0..<60).map { _ in flat(100) }
        let points = LoopPoints(start: 8, end: 60, crossfade: 8)
        XCTAssertEqual(LoopFinder.seamMismatch(thumbnails: steady, points: points), 0)
        // The picture ends 30 levels brighter than it began.
        let moved = (0..<60).map { flat($0 < 30 ? 100 : 130) }
        XCTAssertEqual(LoopFinder.seamMismatch(thumbnails: moved, points: points), 30)
        XCTAssertGreaterThan(30, LoopFinder.visibleSeam)
        // Without a cross-fade, the last frame is compared with the first.
        XCTAssertEqual(
            LoopFinder.seamMismatch(thumbnails: moved, points: LoopPoints(start: 0, end: 60, crossfade: 0)), 30)
    }

    func testFinderKeepsMostOfASlowlyChangingClip() {
        // Brightness drifts steadily, as when the light or the pose changes a little
        // over five seconds. The shortest loop would hide the drift best, but
        // someone who records five seconds wants about five seconds back.
        let thumbnails: [[UInt8]] = (0..<150).map { frame in
            [UInt8](repeating: UInt8(100 + frame / 5), count: Thumbnail.count)
        }
        let loop = LoopFinder.find(thumbnails: thumbnails, minimumLength: 30, crossfade: 8)
        XCTAssertGreaterThanOrEqual(loop.length, 90, "the loop keeps most of the recording")
        XCTAssertLessThanOrEqual(loop.start, 8 + 150 / 6)
        XCTAssertGreaterThanOrEqual(loop.end, 150 - 150 / 6)
    }

    func testFinderHandlesClipsTooShortToChoose() {
        let thumbnails = periodicThumbnails(count: 6, period: 3)
        let loop = LoopFinder.find(thumbnails: thumbnails, minimumLength: 30, crossfade: 8)
        XCTAssertLessThanOrEqual(loop.end, 6)
        XCTAssertGreaterThanOrEqual(loop.length, 1)
        XCTAssertLessThanOrEqual(loop.crossfade, loop.start)
    }

    func testForwardSequenceFadesIntoTheLeadIn() {
        let points = LoopPoints(start: 10, end: 20, crossfade: 4)
        var sequencer = LoopSequencer(points: points, mode: .forward)
        let steps = (0..<20).map { _ in sequencer.next() }
        // Two passes through frames 10...19.
        XCTAssertEqual(steps.map(\.primary), Array(10..<20) + Array(10..<20))
        // No blending until the last four frames.
        XCTAssertTrue(steps[0..<6].allSatisfy { $0.secondary == nil && $0.mix == 0 })
        // Frames 16...19 blend with 6...9, more strongly each frame.
        XCTAssertEqual(steps[6..<10].map(\.secondary), [6, 7, 8, 9])
        XCTAssertEqual(steps[6..<10].map(\.mix), [0.2, 0.4, 0.6, 0.8])
        // The frame after the fade is the start itself, unblended: 9 leads into 10.
        XCTAssertEqual(steps[10], LoopStep(primary: 10, secondary: nil, mix: 0))
    }

    func testForwardSequenceWithoutRoomForAFadeJustWraps() {
        var sequencer = LoopSequencer(points: LoopPoints(start: 0, end: 3, crossfade: 8), mode: .forward)
        let steps = (0..<7).map { _ in sequencer.next() }
        XCTAssertEqual(steps.map(\.primary), [0, 1, 2, 0, 1, 2, 0])
        XCTAssertTrue(steps.allSatisfy { $0.secondary == nil })
    }

    func testPingPongSequenceReversesAtBothEnds() {
        var sequencer = LoopSequencer(points: LoopPoints(start: 2, end: 6, crossfade: 0), mode: .pingPong)
        let primaries = (0..<12).map { _ in sequencer.next().primary }
        XCTAssertEqual(primaries, [2, 3, 4, 5, 4, 3, 2, 3, 4, 5, 4, 3])
    }

    func testSingleFrameLoopsStayPut() {
        for mode in LoopMode.allCases {
            var sequencer = LoopSequencer(points: LoopPoints(start: 4, end: 5, crossfade: 2), mode: mode)
            XCTAssertEqual((0..<3).map { _ in sequencer.next().primary }, [4, 4, 4])
        }
    }

    func testStatsRoundTrip() {
        var stats = VirtualCameraStats()
        stats.framesIn = 1200
        stats.framesOut = 1234
        stats.slateFrames = 34
        stats.clients = 2
        stats.latencyMs = 3.25
        stats.maxLatencyMs = 11.5
        XCTAssertEqual(VirtualCameraStats(encoded: stats.encoded), stats)
        XCTAssertNil(VirtualCameraStats(encoded: "nonsense"))
    }
}
