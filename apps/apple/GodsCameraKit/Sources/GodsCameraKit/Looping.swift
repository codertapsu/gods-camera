import Foundation

/// How a clip repeats.
public enum LoopMode: String, Codable, CaseIterable, Sendable {
    /// Start to end, then a cross-fade back to the start.
    case forward
    /// Start to end, then backwards to the start, and so on.
    case pingPong
}

/// The part of a clip that repeats.
public struct LoopPoints: Equatable, Codable, Sendable {
    /// First frame of the loop.
    public var start: Int
    /// One past the last frame of the loop.
    public var end: Int
    /// Frames over which the end fades into the lead-in before `start`.
    public var crossfade: Int

    public init(start: Int, end: Int, crossfade: Int) {
        self.start = start
        self.end = end
        self.crossfade = crossfade
    }

    public var length: Int { end - start }
}

/// Chooses where a recorded clip should loop.
public enum LoopFinder {
    /// A seam mismatch above this many 8-bit levels shows as a ghost during the
    /// cross-fade: about what a head moved by a few centimetres produces.
    public static let visibleSeam: Float = 4

    /// Mean difference between the frames that the cross-fade at the end of the
    /// loop blends: each of the last frames against its partner before `start`.
    public static func seamMismatch(thumbnails: [[UInt8]], points: LoopPoints) -> Float {
        let count = thumbnails.count
        guard count > 1, points.end <= count, points.start < points.end else { return 0 }
        let fade = min(points.crossfade, points.start, points.length - 1)
        guard fade > 0 else {
            return Thumbnail.difference(thumbnails[points.end - 1], thumbnails[points.start])
        }
        var sum: Float = 0
        for back in 1...fade {
            sum += Thumbnail.difference(thumbnails[points.end - back], thumbnails[points.start - back])
        }
        return sum / Float(fade)
    }

    /// Find where a clip should loop: nearly all of it, with the jump from the
    /// end back to the start placed where it is hardest to see.
    ///
    /// Someone who records five seconds expects about five seconds back, so
    /// the start is looked for only in the first sixth of the clip and the
    /// end only in the last sixth. Within those, the pair of moments that
    /// look most alike and are quiet wins. This also trims the moment after
    /// pressing Record and the reach for Stop.
    ///
    /// - Parameters:
    ///   - thumbnails: one small greyscale picture per recorded frame.
    ///   - minimumLength: shortest acceptable loop, in frames.
    ///   - crossfade: frames of cross-fade wanted. The loop starts at least this
    ///     far into the clip, because the fade borrows the frames before `start`.
    public static func find(thumbnails: [[UInt8]], minimumLength: Int, crossfade: Int) -> LoopPoints {
        let count = thumbnails.count
        let fade = max(0, min(crossfade, count / 4))
        let shortest = max(2, minimumLength)
        // Too short to choose: loop everything that is left after the lead-in.
        guard count >= fade + shortest + 1 else {
            return LoopPoints(start: min(fade, max(0, count - 1)), end: count, crossfade: min(fade, max(0, count - 1)))
        }

        // How much each frame differs from the one before it.
        var motion = [Float](repeating: 0, count: count)
        for i in 1..<count { motion[i] = Thumbnail.difference(thumbnails[i], thumbnails[i - 1]) }

        let window = max(1, count / 6)
        let seamOffsets = fade > 0 ? Array(Set([1, max(1, fade / 2), fade])).sorted() : [1]
        var best = LoopPoints(start: fade, end: count, crossfade: fade)
        var bestCost = Float.infinity
        let lastStart = max(fade, min(fade + window, count - shortest))
        for start in fade...lastStart {
            let firstEnd = max(start + shortest, count - window)
            guard firstEnd <= count else { continue }
            for end in firstEnd...count {
                // The frames leading up to the end must look like the frames leading
                // up to the start, because those are the ones the cross-fade blends.
                var seam: Float = 0
                var compared = 0
                for back in seamOffsets where start - back >= 0 {
                    seam += Thumbnail.difference(thumbnails[end - back], thumbnails[start - back])
                    compared += 1
                }
                seam = compared > 0 ? seam / Float(compared) : Thumbnail.difference(thumbnails[end - 1], thumbnails[start])
                // Prefer quiet moments, where a small mismatch is least visible,
                // and, between equals, the longer loop.
                let stillness = motion[start] + motion[end - 1]
                let lengthBonus = Float(end - start) / Float(count)
                let cost = seam + 0.5 * stillness - 1.5 * lengthBonus
                if cost < bestCost {
                    bestCost = cost
                    best = LoopPoints(start: start, end: end, crossfade: fade)
                }
            }
        }
        return best
    }
}

/// One output frame of a loop: a clip frame, optionally blended with a second.
public struct LoopStep: Equatable, Sendable {
    public var primary: Int
    public var secondary: Int?
    /// Weight of the secondary frame, from 0 to 1.
    public var mix: Float
}

/// Walks through a clip's frames in loop order.
public struct LoopSequencer: Sendable {
    public let points: LoopPoints
    public let mode: LoopMode
    private var position: Int
    private var direction = 1

    public init(points: LoopPoints, mode: LoopMode) {
        precondition(points.length >= 1, "a loop needs at least one frame")
        self.points = points
        self.mode = mode
        position = points.start
    }

    public mutating func next() -> LoopStep {
        let current = position
        switch mode {
        case .forward:
            position = current + 1 >= points.end ? points.start : current + 1
            // Over the last frames, fade towards the lead-in before `start`, so
            // that the frame after the fade is `start` itself.
            let fade = min(points.crossfade, points.start, points.length - 1)
            let intoFade = current - (points.end - fade)
            if fade > 0, intoFade >= 0 {
                return LoopStep(
                    primary: current, secondary: points.start - fade + intoFade,
                    mix: Float(intoFade + 1) / Float(fade + 1))
            }
            return LoopStep(primary: current, secondary: nil, mix: 0)
        case .pingPong:
            if points.length > 1 {
                if current + direction >= points.end || current + direction < points.start { direction = -direction }
                position = current + direction
            }
            return LoopStep(primary: current, secondary: nil, mix: 0)
        }
    }
}
