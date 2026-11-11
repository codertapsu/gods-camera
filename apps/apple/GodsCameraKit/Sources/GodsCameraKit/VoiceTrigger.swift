import AVFoundation
import Foundation

/// Decides from a run of sound levels when someone has started to speak.
public struct VoiceActivityDetector: Sendable {
    /// The level a sound must reach to count as speech, in decibels below full scale.
    public var thresholdDb: Float
    /// How long the sound must last, so that a click or a cough does not count.
    public var holdSeconds: Double
    private var heard = 0.0

    public init(thresholdDb: Float = -32, holdSeconds: Double = 0.25) {
        self.thresholdDb = thresholdDb
        self.holdSeconds = holdSeconds
    }

    /// Feed the level of the next stretch of sound. Returns true once speech has lasted long enough.
    public mutating func feed(levelDb: Float, seconds: Double) -> Bool {
        if levelDb >= thresholdDb {
            heard += seconds
        } else {
            // Quiet counts double, so scattered noises do not add up to speech.
            heard = max(0, heard - seconds * 2)
        }
        return heard >= holdSeconds
    }

    public mutating func reset() { heard = 0 }

    /// The level of a block of samples, in decibels below full scale.
    public static func level(of samples: UnsafeBufferPointer<Float>) -> Float {
        guard !samples.isEmpty else { return -120 }
        var sum: Float = 0
        for sample in samples { sum += sample * sample }
        let rms = (sum / Float(samples.count)).squareRoot()
        return rms > 0 ? max(-120, 20 * log10(rms)) : -120
    }
}

/// Listens to the microphone and calls back when the user starts to speak.
/// Nothing is recorded or kept; only the loudness of each block is looked at.
public final class VoiceTrigger {
    public enum TriggerError: Error {
        case noMicrophone
    }

    private let engine = AVAudioEngine()
    private var detector: VoiceActivityDetector
    private var running = false
    /// Called on an audio thread.
    public var onVoice: (() -> Void)?

    public init(detector: VoiceActivityDetector = VoiceActivityDetector()) {
        self.detector = detector
    }

    public static var isAuthorized: Bool { AVCaptureDevice.authorizationStatus(for: .audio) == .authorized }

    public var isRunning: Bool { running }

    public func start() throws {
        guard !running else { return }
        let input = engine.inputNode
        let format = input.outputFormat(forBus: 0)
        guard format.sampleRate > 0, format.channelCount > 0 else { throw TriggerError.noMicrophone }
        detector.reset()
        let sampleRate = format.sampleRate
        // The macOS 27 form of the tap call reports failure instead of raising an exception.
        try input.__installTap(onBus: 0, bufferSize: 1024, format: format, error: ()) { [weak self] buffer, _ in
            guard let self, let channel = buffer.floatChannelData?[0] else { return }
            let count = Int(buffer.frameLength)
            let level = VoiceActivityDetector.level(of: UnsafeBufferPointer(start: channel, count: count))
            if self.detector.feed(levelDb: level, seconds: Double(count) / sampleRate) {
                self.detector.reset()
                self.onVoice?()
            }
        }
        do {
            try engine.start()
        } catch {
            input.removeTap(onBus: 0)
            throw error
        }
        running = true
    }

    public func stop() {
        guard running else { return }
        engine.inputNode.removeTap(onBus: 0)
        engine.stop()
        running = false
    }
}
