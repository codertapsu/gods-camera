import CoreGraphics
import CoreVideo
import Foundation

public struct PixelSize: Equatable, Hashable, Sendable, Codable {
    public var width: Int
    public var height: Int

    public init(width: Int, height: Int) {
        self.width = width
        self.height = height
    }

    public static let hd720 = PixelSize(width: 1280, height: 720)
    public static let hd1080 = PixelSize(width: 1920, height: 1080)
}

extension CVPixelBuffer {
    public var pixelSize: PixelSize {
        PixelSize(width: CVPixelBufferGetWidth(self), height: CVPixelBufferGetHeight(self))
    }

    /// Run `body` with the buffer's memory locked.
    /// The closure receives the base address and the bytes per row.
    public func withLockedBytes<T>(
        readOnly: Bool, _ body: (UnsafeMutableRawPointer, Int) throws -> T
    ) rethrows -> T? {
        let flags: CVPixelBufferLockFlags = readOnly ? .readOnly : []
        CVPixelBufferLockBaseAddress(self, flags)
        defer { CVPixelBufferUnlockBaseAddress(self, flags) }
        guard let base = CVPixelBufferGetBaseAddress(self) else { return nil }
        return try body(base, CVPixelBufferGetBytesPerRow(self))
    }
}

/// A pool of IOSurface-backed BGRA buffers that Metal can render into and
/// that can be handed to the Camera Extension without copying.
public final class PixelBufferPool {
    public let size: PixelSize
    private let pool: CVPixelBufferPool

    public init?(size: PixelSize) {
        let attributes: [CFString: Any] = [
            kCVPixelBufferPixelFormatTypeKey: kCVPixelFormatType_32BGRA,
            kCVPixelBufferWidthKey: size.width,
            kCVPixelBufferHeightKey: size.height,
            kCVPixelBufferIOSurfacePropertiesKey: [:] as CFDictionary,
            kCVPixelBufferMetalCompatibilityKey: true,
        ]
        var created: CVPixelBufferPool?
        CVPixelBufferPoolCreate(kCFAllocatorDefault, nil, attributes as CFDictionary, &created)
        guard let created else { return nil }
        pool = created
        self.size = size
    }

    public func makeBuffer() -> CVPixelBuffer? {
        var buffer: CVPixelBuffer?
        CVPixelBufferPoolCreatePixelBuffer(kCFAllocatorDefault, pool, &buffer)
        return buffer
    }
}

/// A small greyscale picture of a frame, used to compare frames cheaply.
public enum Thumbnail {
    public static let columns = 32
    public static let rows = 18
    public static var count: Int { columns * rows }

    /// Average brightness of each cell of a `columns` x `rows` grid over a BGRA buffer.
    public static func make(from buffer: CVPixelBuffer) -> [UInt8] {
        let width = CVPixelBufferGetWidth(buffer)
        let height = CVPixelBufferGetHeight(buffer)
        var out = [UInt8](repeating: 0, count: count)
        _ = buffer.withLockedBytes(readOnly: true) { base, rowBytes in
            // Four samples per cell are enough to tell frames apart.
            let offsets: [(Double, Double)] = [(0.25, 0.25), (0.75, 0.25), (0.25, 0.75), (0.75, 0.75)]
            for row in 0..<rows {
                for column in 0..<columns {
                    var sum = 0
                    for (dx, dy) in offsets {
                        let x = min(width - 1, Int((Double(column) + dx) * Double(width) / Double(columns)))
                        let y = min(height - 1, Int((Double(row) + dy) * Double(height) / Double(rows)))
                        let pixel = base.advanced(by: y * rowBytes + x * 4).assumingMemoryBound(to: UInt8.self)
                        // Integer luma from B, G, R.
                        sum += (Int(pixel[0]) * 29 + Int(pixel[1]) * 150 + Int(pixel[2]) * 77) >> 8
                    }
                    out[row * columns + column] = UInt8(sum / offsets.count)
                }
            }
        }
        return out
    }

    public static func mean(_ thumbnail: [UInt8]) -> Float {
        guard !thumbnail.isEmpty else { return 0 }
        return Float(thumbnail.reduce(0) { $0 + Int($1) }) / Float(thumbnail.count)
    }

    /// Mean absolute difference between two thumbnails, in 8-bit levels.
    public static func difference(_ a: [UInt8], _ b: [UInt8]) -> Float {
        guard a.count == b.count, !a.isEmpty else { return .infinity }
        var sum = 0
        for i in 0..<a.count { sum += abs(Int(a[i]) - Int(b[i])) }
        return Float(sum) / Float(a.count)
    }
}
