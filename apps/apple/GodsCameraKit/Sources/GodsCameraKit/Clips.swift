import CoreMedia
import CoreVideo
import Foundation
import VideoToolbox

public enum ClipError: Error {
    case encoderUnavailable(OSStatus)
    case decoderUnavailable(OSStatus)
    case empty
    case unreadable(URL)
    case writeFailed
}

/// A short recording held in memory as compressed frames.
///
/// Every frame is a key frame, so any frame can be decoded on its own. That
/// is what makes backwards playback and cross-fades cheap.
public final class Clip {
    public let id: UUID
    public var name: String
    public let size: PixelSize
    public let frameRate: Double
    public let createdAt: Date
    /// One small greyscale picture per frame.
    public let thumbnails: [[UInt8]]
    public var loop: LoopPoints
    public var mode: LoopMode
    let samples: [CMSampleBuffer]

    public var frameCount: Int { samples.count }
    public var duration: Double { Double(frameCount) / frameRate }
    /// Average brightness, used to warn when the room's lighting has changed.
    public var meanLuma: Float {
        guard !thumbnails.isEmpty else { return 0 }
        return thumbnails.reduce(0) { $0 + Thumbnail.mean($1) } / Float(thumbnails.count)
    }

    init(
        id: UUID = UUID(), name: String, size: PixelSize, frameRate: Double, createdAt: Date = Date(),
        samples: [CMSampleBuffer], thumbnails: [[UInt8]], loop: LoopPoints, mode: LoopMode
    ) {
        self.id = id
        self.name = name
        self.size = size
        self.frameRate = frameRate
        self.createdAt = createdAt
        self.samples = samples
        self.thumbnails = thumbnails
        self.loop = loop
        self.mode = mode
    }
}

/// Compresses frames into a `Clip` as they arrive.
public final class ClipRecorder {
    public let size: PixelSize
    public let frameRate: Double
    public let maxFrames: Int
    private let session: VTCompressionSession
    private let lock = NSLock()
    private var samples: [Int: CMSampleBuffer] = [:]
    private var thumbnails: [[UInt8]] = []
    private var appended = 0

    public var frameCount: Int { appended }
    public var isFull: Bool { appended >= maxFrames }

    public init(size: PixelSize, frameRate: Double, maxSeconds: Double) throws {
        self.size = size
        self.frameRate = frameRate
        maxFrames = max(1, Int(maxSeconds * frameRate))
        var created: VTCompressionSession?
        let status = VTCompressionSessionCreate(
            allocator: kCFAllocatorDefault, width: Int32(size.width), height: Int32(size.height),
            codecType: kCMVideoCodecType_HEVC, encoderSpecification: nil, imageBufferAttributes: nil,
            compressedDataAllocator: nil, outputCallback: nil, refcon: nil, compressionSessionOut: &created)
        guard status == noErr, let created else { throw ClipError.encoderUnavailable(status) }
        session = created
        VTSessionSetProperty(session, key: kVTCompressionPropertyKey_RealTime, value: kCFBooleanTrue)
        // Key frames only, in capture order.
        VTSessionSetProperty(session, key: kVTCompressionPropertyKey_MaxKeyFrameInterval, value: 1 as CFNumber)
        VTSessionSetProperty(session, key: kVTCompressionPropertyKey_AllowFrameReordering, value: kCFBooleanFalse)
        VTSessionSetProperty(session, key: kVTCompressionPropertyKey_Quality, value: 0.8 as CFNumber)
        VTCompressionSessionPrepareToEncodeFrames(session)
    }

    deinit { VTCompressionSessionInvalidate(session) }

    /// Add a frame. Returns false once the clip has reached its maximum length.
    @discardableResult
    public func append(_ buffer: CVPixelBuffer) -> Bool {
        guard !isFull else { return false }
        let index = appended
        appended += 1
        thumbnails.append(Thumbnail.make(from: buffer))
        let scale = CMTimeScale(frameRate.rounded())
        VTCompressionSessionEncodeFrame(
            session, imageBuffer: buffer, presentationTimeStamp: CMTime(value: CMTimeValue(index), timescale: scale),
            duration: CMTime(value: 1, timescale: scale), frameProperties: nil, infoFlagsOut: nil
        ) { [weak self] status, _, sample in
            guard let self, status == noErr, let sample else { return }
            self.lock.lock()
            self.samples[index] = sample
            self.lock.unlock()
        }
        return !isFull
    }

    /// Finish compressing and return the clip, with loop points chosen for it.
    public func finish(name: String, mode: LoopMode = .forward, crossfade: Int = 8) throws -> Clip {
        VTCompressionSessionCompleteFrames(session, untilPresentationTimeStamp: .invalid)
        lock.lock()
        let collected = samples
        lock.unlock()
        // Keep the leading run of frames that compressed successfully.
        var ordered: [CMSampleBuffer] = []
        for index in 0..<appended {
            guard let sample = collected[index] else { break }
            ordered.append(sample)
        }
        guard !ordered.isEmpty else { throw ClipError.empty }
        let thumbs = Array(thumbnails.prefix(ordered.count))
        let loop = LoopFinder.find(
            thumbnails: thumbs, minimumLength: min(ordered.count, Int(frameRate)), crossfade: crossfade)
        return Clip(
            name: name, size: size, frameRate: frameRate, samples: ordered, thumbnails: thumbs, loop: loop,
            mode: mode)
    }
}

/// Turns a clip's compressed frames back into pictures, in any order.
public final class ClipDecoder {
    private let clip: Clip
    private let session: VTDecompressionSession
    private var cache: [(index: Int, buffer: CVPixelBuffer)] = []
    private let cacheLimit = 12

    public init(clip: Clip) throws {
        self.clip = clip
        guard let first = clip.samples.first, let format = CMSampleBufferGetFormatDescription(first) else {
            throw ClipError.empty
        }
        let attributes: [CFString: Any] = [
            kCVPixelBufferPixelFormatTypeKey: kCVPixelFormatType_32BGRA,
            kCVPixelBufferIOSurfacePropertiesKey: [:] as CFDictionary,
            kCVPixelBufferMetalCompatibilityKey: true,
        ]
        var created: VTDecompressionSession?
        let status = VTDecompressionSessionCreate(
            allocator: kCFAllocatorDefault, formatDescription: format, decoderSpecification: nil,
            imageBufferAttributes: attributes as CFDictionary, outputCallback: nil, decompressionSessionOut: &created)
        guard status == noErr, let created else { throw ClipError.decoderUnavailable(status) }
        session = created
    }

    deinit { VTDecompressionSessionInvalidate(session) }

    /// The picture at `index`, or nil if it cannot be decoded.
    public func frame(at index: Int) -> CVPixelBuffer? {
        guard clip.samples.indices.contains(index) else { return nil }
        if let hit = cache.first(where: { $0.index == index }) { return hit.buffer }
        var decoded: CVPixelBuffer?
        // No asynchronous flag, so the handler has run by the time this returns.
        VTDecompressionSessionDecodeFrame(session, sampleBuffer: clip.samples[index], flags: [], infoFlagsOut: nil) {
            status, _, image, _, _ in
            if status == noErr { decoded = image }
        }
        guard let decoded else { return nil }
        cache.append((index, decoded))
        if cache.count > cacheLimit { cache.removeFirst() }
        return decoded
    }
}

/// What the library stores about each clip, next to the file holding its frames.
struct ClipRecord: Codable {
    var id: UUID
    var name: String
    var size: PixelSize
    var frameRate: Double
    var createdAt: Date
    var loop: LoopPoints
    var mode: LoopMode
    var meanLuma: Float
    /// The video stream's parameter sets, needed to decode any frame.
    var parameterSets: [Data]
    var nalUnitHeaderLength: Int32
    /// Compressed size of each frame, in order. Their sum is the length of the frame data.
    var frameSizes: [Int]

    var frameCount: Int { frameSizes.count }
}

/// A saved clip, as listed in the library without loading its frames.
public struct ClipSummary: Identifiable, Equatable, Sendable {
    public let id: UUID
    public var name: String
    public let duration: Double
    public let createdAt: Date
    public let meanLuma: Float
}

/// Named loops saved on disk.
///
/// Each clip is two files: a small JSON description, and a data file holding
/// the compressed frames followed by the thumbnails. Writing the frames as
/// they are keeps saving and loading fast and exact.
public final class ClipLibrary {
    public let directory: URL

    public init(directory: URL) throws {
        self.directory = directory
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
    }

    private func dataURL(_ id: UUID) -> URL { directory.appendingPathComponent("\(id.uuidString).frames") }
    private func recordURL(_ id: UUID) -> URL { directory.appendingPathComponent("\(id.uuidString).json") }

    public func save(_ clip: Clip) throws {
        guard let format = clip.samples.first.flatMap(CMSampleBufferGetFormatDescription) else { throw ClipError.empty }
        var setCount = 0
        var nalLength: Int32 = 0
        CMVideoFormatDescriptionGetHEVCParameterSetAtIndex(
            format, parameterSetIndex: 0, parameterSetPointerOut: nil, parameterSetSizeOut: nil,
            parameterSetCountOut: &setCount, nalUnitHeaderLengthOut: &nalLength)
        var parameterSets: [Data] = []
        for index in 0..<setCount {
            var pointer: UnsafePointer<UInt8>?
            var size = 0
            CMVideoFormatDescriptionGetHEVCParameterSetAtIndex(
                format, parameterSetIndex: index, parameterSetPointerOut: &pointer, parameterSetSizeOut: &size,
                parameterSetCountOut: nil, nalUnitHeaderLengthOut: nil)
            guard let pointer else { throw ClipError.writeFailed }
            parameterSets.append(Data(bytes: pointer, count: size))
        }

        var payload = Data()
        var frameSizes: [Int] = []
        for sample in clip.samples {
            guard let block = CMSampleBufferGetDataBuffer(sample) else { throw ClipError.writeFailed }
            let length = CMBlockBufferGetDataLength(block)
            var bytes = Data(count: length)
            let status = bytes.withUnsafeMutableBytes {
                CMBlockBufferCopyDataBytes(block, atOffset: 0, dataLength: length, destination: $0.baseAddress!)
            }
            guard status == noErr else { throw ClipError.writeFailed }
            payload.append(bytes)
            frameSizes.append(length)
        }
        payload.append(Data(clip.thumbnails.joined()))
        try payload.write(to: dataURL(clip.id), options: .atomic)

        let record = ClipRecord(
            id: clip.id, name: clip.name, size: clip.size, frameRate: clip.frameRate, createdAt: clip.createdAt,
            loop: clip.loop, mode: clip.mode, meanLuma: clip.meanLuma, parameterSets: parameterSets,
            nalUnitHeaderLength: nalLength, frameSizes: frameSizes)
        try JSONEncoder().encode(record).write(to: recordURL(clip.id), options: .atomic)
    }

    private func records() -> [ClipRecord] {
        let files = (try? FileManager.default.contentsOfDirectory(at: directory, includingPropertiesForKeys: nil)) ?? []
        return files.filter { $0.pathExtension == "json" }
            .compactMap { try? JSONDecoder().decode(ClipRecord.self, from: Data(contentsOf: $0)) }
            .sorted { $0.createdAt > $1.createdAt }
    }

    public func list() -> [ClipSummary] {
        records().map {
            ClipSummary(
                id: $0.id, name: $0.name, duration: Double($0.frameCount) / $0.frameRate, createdAt: $0.createdAt,
                meanLuma: $0.meanLuma)
        }
    }

    public func load(_ id: UUID) throws -> Clip {
        let url = dataURL(id)
        guard let record = records().first(where: { $0.id == id }),
            let payload = try? Data(contentsOf: url, options: .mappedIfSafe)
        else { throw ClipError.unreadable(url) }
        let frameBytes = record.frameSizes.reduce(0, +)
        guard payload.count >= frameBytes + record.frameCount * Thumbnail.count, record.frameCount > 0 else {
            throw ClipError.unreadable(url)
        }

        var format: CMFormatDescription?
        let sets = record.parameterSets.map { [UInt8]($0) }
        let status: OSStatus = sets.withUnsafePointers { pointers in
            CMVideoFormatDescriptionCreateFromHEVCParameterSets(
                allocator: kCFAllocatorDefault, parameterSetCount: sets.count, parameterSetPointers: pointers,
                parameterSetSizes: sets.map(\.count), nalUnitHeaderLength: record.nalUnitHeaderLength,
                extensions: nil, formatDescriptionOut: &format)
        }
        guard status == noErr, let format else { throw ClipError.decoderUnavailable(status) }

        var samples: [CMSampleBuffer] = []
        var offset = 0
        let scale = CMTimeScale(record.frameRate.rounded())
        for (index, length) in record.frameSizes.enumerated() {
            var block: CMBlockBuffer?
            guard
                CMBlockBufferCreateWithMemoryBlock(
                    allocator: kCFAllocatorDefault, memoryBlock: nil, blockLength: length,
                    blockAllocator: kCFAllocatorDefault, customBlockSource: nil, offsetToData: 0,
                    dataLength: length, flags: 0, blockBufferOut: &block) == noErr, let block
            else { throw ClipError.unreadable(url) }
            let copied = payload.withUnsafeBytes {
                CMBlockBufferReplaceDataBytes(
                    with: $0.baseAddress!.advanced(by: offset), blockBuffer: block, offsetIntoDestination: 0,
                    dataLength: length)
            }
            guard copied == noErr else { throw ClipError.unreadable(url) }
            var timing = CMSampleTimingInfo(
                duration: CMTime(value: 1, timescale: scale),
                presentationTimeStamp: CMTime(value: CMTimeValue(index), timescale: scale), decodeTimeStamp: .invalid)
            var sampleSize = length
            var sample: CMSampleBuffer?
            guard
                CMSampleBufferCreateReady(
                    allocator: kCFAllocatorDefault, dataBuffer: block, formatDescription: format, sampleCount: 1,
                    sampleTimingEntryCount: 1, sampleTimingArray: &timing, sampleSizeEntryCount: 1,
                    sampleSizeArray: &sampleSize, sampleBufferOut: &sample) == noErr, let sample
            else { throw ClipError.unreadable(url) }
            samples.append(sample)
            offset += length
        }

        let thumbBytes = [UInt8](payload[frameBytes..<frameBytes + record.frameCount * Thumbnail.count])
        let thumbnails = stride(from: 0, to: thumbBytes.count, by: Thumbnail.count).map {
            Array(thumbBytes[$0..<$0 + Thumbnail.count])
        }
        return Clip(
            id: record.id, name: record.name, size: record.size, frameRate: record.frameRate,
            createdAt: record.createdAt, samples: samples, thumbnails: thumbnails, loop: record.loop,
            mode: record.mode)
    }

    /// Store a clip's changed name, loop points or mode without rewriting its frames.
    public func update(_ clip: Clip) throws {
        guard var record = records().first(where: { $0.id == clip.id }) else { return }
        record.name = clip.name
        record.loop = clip.loop
        record.mode = clip.mode
        try JSONEncoder().encode(record).write(to: recordURL(clip.id), options: .atomic)
    }

    public func delete(_ id: UUID) {
        try? FileManager.default.removeItem(at: dataURL(id))
        try? FileManager.default.removeItem(at: recordURL(id))
    }
}

extension Array where Element == [UInt8] {
    /// Call `body` with a pointer to each inner array, all valid for the duration of the call.
    fileprivate func withUnsafePointers<R>(_ body: ([UnsafePointer<UInt8>]) -> R) -> R {
        func step(_ index: Int, _ collected: [UnsafePointer<UInt8>]) -> R {
            guard index < count else { return body(collected) }
            return self[index].withUnsafeBufferPointer { step(index + 1, collected + [$0.baseAddress!]) }
        }
        return step(0, [])
    }
}
