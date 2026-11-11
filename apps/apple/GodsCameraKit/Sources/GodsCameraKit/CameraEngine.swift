import CoreGraphics
import CoreVideo
import Foundation
import Metal

/// Where finished frames go: the virtual camera, a test collector, and so on.
public protocol FrameSink: AnyObject {
    func send(_ pixelBuffer: CVPixelBuffer, hostTimeNs: UInt64)
}

/// The live pipeline: takes pictures from a source, shapes them for the
/// output, and can record a clip and play it back as a loop.
///
/// All work happens on one private queue. Callbacks are delivered on that
/// queue too, so receivers hop to the main queue themselves if they need it.
public final class CameraEngine {
    public struct Settings: Equatable, Sendable {
        public var outputSize = PixelSize.hd720
        /// Flip the live picture left to right.
        public var mirror = false
        /// Draw a "LOOP" badge on the output while a loop is playing.
        public var showLoopBadge = false
        /// Frames over which live and loop pictures fade into each other.
        public var transitionFrames = 8
        /// Longest recording, in seconds.
        public var maxClipSeconds = 30.0
        public var frameRate = 30.0

        public init() {}
    }

    public enum Mode: Equatable, Sendable {
        case idle
        case live
        case looping
    }

    public struct Status: Equatable, Sendable {
        public var mode = Mode.idle
        public var isRecording = false
        public var recordedSeconds = 0.0
        public var framesOut: UInt64 = 0
        /// Smoothed time to render one frame, in milliseconds.
        public var renderMs = 0.0
        /// Average brightness of the latest live picture, 0 to 255.
        public var liveLuma: Float = 0
        /// Pictures per second arriving from the source, measured.
        public var sourceFps = 0.0

        public init() {}
    }

    public var onOutput: ((CVPixelBuffer, UInt64) -> Void)?
    public var onStatus: ((Status) -> Void)?
    /// Called when a recording ends, with the finished clip.
    public var onClip: ((Clip) -> Void)?
    public weak var sink: FrameSink?

    private let renderer: MetalRenderer
    private let queue = DispatchQueue(label: "dev.godscamera.engine", qos: .userInteractive)
    private var settings = Settings()
    private var pool: PixelBufferPool?
    private var source: FrameSource?
    private var status = Status()

    private var latestLive: VideoFrame?
    private var rateWindowStart: UInt64 = 0
    private var rateWindowFrames = 0
    private var recorder: ClipRecorder?
    private var clipCounter = 0

    private var loopClip: Clip?
    private var decoder: ClipDecoder?
    private var sequencer: LoopSequencer?
    private var loopTimer: DispatchSourceTimer?
    /// Frames left in a fade between live and loop pictures.
    private var transitionLeft = 0
    private var leavingLoop = false
    private var lastLoopFrame: CVPixelBuffer?
    private lazy var loopBadge = renderer.makeBadge(text: "LOOP", color: CGColor(red: 0.85, green: 0.2, blue: 0.2, alpha: 0.9))

    public init(renderer: MetalRenderer) {
        self.renderer = renderer
    }

    // MARK: Control

    public func update(_ change: @escaping (inout Settings) -> Void) {
        queue.async {
            let before = self.settings.outputSize
            change(&self.settings)
            if self.settings.outputSize != before {
                // A clip has one size, so a recording in progress ends here.
                self.finishRecording()
                self.pool = nil
            }
        }
    }

    /// Replace the source and start it. Pass nil to stop.
    public func setSource(_ newSource: FrameSource?) {
        queue.async {
            self.source?.onFrame = nil
            self.source?.stop()
            self.source = newSource
            self.latestLive = nil
            self.rateWindowStart = 0
            self.rateWindowFrames = 0
            self.status.sourceFps = 0
            guard let newSource else {
                if self.status.mode == .live { self.status.mode = .idle }
                self.publish()
                return
            }
            newSource.onFrame = { [weak engine = self] frame in
                engine?.queue.async { engine?.receive(frame) }
            }
            do {
                try newSource.start()
                if self.status.mode == .idle { self.status.mode = .live }
            } catch {
                Log.engine.error("source failed to start: \(String(describing: error))")
                self.source = nil
            }
            self.publish()
        }
    }

    public func startRecording() {
        queue.async {
            guard self.recorder == nil, self.status.mode == .live else { return }
            do {
                self.recorder = try ClipRecorder(
                    size: self.settings.outputSize, frameRate: self.recordingRate(),
                    maxSeconds: self.settings.maxClipSeconds)
                self.status.isRecording = true
                self.status.recordedSeconds = 0
                self.publish()
            } catch {
                Log.engine.error("recording failed to start: \(String(describing: error))")
            }
        }
    }

    public func stopRecording() {
        queue.async { self.finishRecording() }
    }

    /// Play `clip` on the output in place of the live picture.
    public func startLoop(_ clip: Clip) {
        queue.async {
            self.finishRecording()
            do {
                self.decoder = try ClipDecoder(clip: clip)
            } catch {
                Log.engine.error("loop failed to start: \(String(describing: error))")
                return
            }
            self.loopClip = clip
            self.sequencer = LoopSequencer(points: clip.loop, mode: clip.mode)
            self.leavingLoop = false
            self.transitionLeft = self.latestLive == nil ? 0 : self.settings.transitionFrames
            self.status.mode = .looping
            self.startLoopTimer(frameRate: clip.frameRate)
            self.publish()
        }
    }

    /// Fade back to the live picture.
    public func stopLoop() {
        queue.async {
            guard self.status.mode == .looping, !self.leavingLoop else { return }
            if self.latestLive == nil || self.settings.transitionFrames == 0 {
                self.endLoop()
            } else {
                self.leavingLoop = true
                self.transitionLeft = self.settings.transitionFrames
            }
        }
    }

    /// Run `body` on the engine queue after everything queued so far. For tests.
    public func sync(_ body: () -> Void = {}) {
        queue.sync(execute: body)
    }

    // MARK: Frames

    private func receive(_ frame: VideoFrame) {
        latestLive = frame
        measureRate()
        guard status.mode == .live else { return }
        guard let output = makeOutput() else { return }
        let started = hostTimeNanoseconds()
        do {
            try renderer.render(
                primary: RenderLayer(buffer: frame.pixelBuffer, mirrored: settings.mirror), into: output)
        } catch {
            Log.engine.error("render failed: \(String(describing: error))")
            return
        }
        noteRender(since: started)
        if let recorder {
            recorder.append(output)
            status.recordedSeconds = Double(recorder.frameCount) / recorder.frameRate
            if recorder.isFull { finishRecording() }
        }
        emit(output, hostTimeNs: frame.hostTimeNs)
    }

    private func loopTick() {
        guard status.mode == .looping, var sequencer, let decoder else { return }
        let step = sequencer.next()
        self.sequencer = sequencer
        guard let primary = decoder.frame(at: step.primary), let output = makeOutput() else { return }
        lastLoopFrame = primary
        let started = hostTimeNanoseconds()

        var first = RenderLayer(buffer: primary)
        var second = step.secondary.flatMap { decoder.frame(at: $0) }.map { RenderLayer(buffer: $0) }
        var mix = step.mix
        // While fading in or out, the loop picture is blended with the live one
        // instead of with its own cross-fade partner.
        if transitionLeft > 0, let live = latestLive {
            let total = Float(max(1, settings.transitionFrames))
            let progress = 1 - Float(transitionLeft - 1) / total  // rises to 1
            let liveLayer = RenderLayer(buffer: live.pixelBuffer, mirrored: settings.mirror)
            if leavingLoop {
                second = liveLayer
                mix = progress
            } else {
                first = liveLayer
                second = RenderLayer(buffer: primary)
                mix = progress
            }
            transitionLeft -= 1
        }

        var overlay: RenderOverlay?
        if settings.showLoopBadge, let badge = loopBadge {
            let height: Float = 0.06
            let width = height * Float(badge.width) / Float(badge.height) * Float(settings.outputSize.height)
                / Float(settings.outputSize.width)
            overlay = RenderOverlay(texture: badge, rect: SIMD4<Float>(0.02, 0.03, width, height), opacity: 1)
        }
        do {
            try renderer.render(primary: first, secondary: second, mix: mix, overlay: overlay, into: output)
        } catch {
            Log.engine.error("loop render failed: \(String(describing: error))")
            return
        }
        noteRender(since: started)
        emit(output, hostTimeNs: hostTimeNanoseconds())
        if leavingLoop, transitionLeft == 0 { endLoop() }
    }

    private func makeOutput() -> CVPixelBuffer? {
        if pool == nil { pool = PixelBufferPool(size: settings.outputSize) }
        return pool?.makeBuffer()
    }

    private func emit(_ output: CVPixelBuffer, hostTimeNs: UInt64) {
        status.framesOut += 1
        sink?.send(output, hostTimeNs: hostTimeNs)
        onOutput?(output, hostTimeNs)
        // The numbers change every frame; listeners are told ten times a second.
        if status.framesOut % 3 == 0 {
            if let live = latestLive, status.framesOut % 15 == 0 {
                status.liveLuma = Thumbnail.mean(Thumbnail.make(from: live.pixelBuffer))
            }
            onStatus?(status)
        }
    }

    /// Count the source's pictures over one-second windows.
    private func measureRate() {
        let now = hostTimeNanoseconds()
        rateWindowFrames += 1
        if rateWindowStart == 0 {
            rateWindowStart = now
            rateWindowFrames = 0
        } else if now - rateWindowStart >= 1_000_000_000 {
            status.sourceFps = Double(rateWindowFrames) * 1_000_000_000 / Double(now - rateWindowStart)
            rateWindowStart = now
            rateWindowFrames = 0
        }
    }

    /// The rate a new clip is labelled with: the source's measured rate when it
    /// is close to a common one, the configured rate otherwise.
    private func recordingRate() -> Double {
        let measured = status.sourceFps
        let common: [Double] = [24, 25, 30, 50, 60]
        return common.first { abs($0 - measured) / $0 < 0.08 } ?? settings.frameRate
    }

    private func noteRender(since started: UInt64) {
        let ms = Double(hostTimeNanoseconds() - started) / 1_000_000
        status.renderMs = status.renderMs == 0 ? ms : status.renderMs * 0.9 + ms * 0.1
    }

    private func publish() {
        onStatus?(status)
    }

    // MARK: Recording and looping

    private func finishRecording() {
        guard let recorder else { return }
        self.recorder = nil
        status.isRecording = false
        clipCounter += 1
        do {
            let clip = try recorder.finish(name: "Loop \(clipCounter)")
            onClip?(clip)
        } catch {
            Log.engine.error("recording failed to finish: \(String(describing: error))")
        }
        publish()
    }

    private func startLoopTimer(frameRate: Double) {
        loopTimer?.cancel()
        let timer = DispatchSource.makeTimerSource(queue: queue)
        timer.schedule(deadline: .now(), repeating: 1.0 / frameRate, leeway: .milliseconds(1))
        timer.setEventHandler { [weak self] in self?.loopTick() }
        timer.resume()
        loopTimer = timer
    }

    private func endLoop() {
        loopTimer?.cancel()
        loopTimer = nil
        decoder = nil
        sequencer = nil
        loopClip = nil
        lastLoopFrame = nil
        leavingLoop = false
        transitionLeft = 0
        status.mode = source == nil ? .idle : .live
        publish()
    }
}
