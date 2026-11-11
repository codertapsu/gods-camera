import Carbon.HIToolbox
import Foundation
import os.log

/// Logging: the system log for live inspection, and a file the user can send.
public enum Log {
    public static let engine = Channel(category: "engine")
    public static let app = Channel(category: "app")

    /// Where the log file lives: `~/Library/Logs/GodsCamera/godscamera.log`.
    public static let fileURL: URL = {
        let directory = FileManager.default.urls(for: .libraryDirectory, in: .userDomainMask)[0]
            .appendingPathComponent("Logs/GodsCamera", isDirectory: true)
        try? FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        return directory.appendingPathComponent("godscamera.log")
    }()

    public struct Channel: Sendable {
        let logger: Logger
        let category: String

        init(category: String) {
            logger = Logger(subsystem: "dev.godscamera.GodsCamera", category: category)
            self.category = category
        }

        public func info(_ message: String) {
            logger.info("\(message, privacy: .public)")
            FileLog.shared.write(level: "info", category: category, message)
        }

        public func error(_ message: String) {
            logger.error("\(message, privacy: .public)")
            FileLog.shared.write(level: "error", category: category, message)
        }
    }
}

/// Appends lines to the log file, starting a fresh file once it passes 2 MB.
final class FileLog: @unchecked Sendable {
    static let shared = FileLog()
    private let queue = DispatchQueue(label: "dev.godscamera.log")
    private let formatter = ISO8601DateFormatter()
    private let limit: UInt64 = 2 * 1024 * 1024

    func write(level: String, category: String, _ message: String) {
        queue.async {
            let line = "\(self.formatter.string(from: Date())) [\(level)] \(category): \(message)\n"
            let url = Log.fileURL
            if let size = try? FileManager.default.attributesOfItem(atPath: url.path)[.size] as? UInt64, size > self.limit {
                let old = url.deletingPathExtension().appendingPathExtension("old.log")
                try? FileManager.default.removeItem(at: old)
                try? FileManager.default.moveItem(at: url, to: old)
            }
            guard let data = line.data(using: .utf8) else { return }
            if let handle = try? FileHandle(forWritingTo: url) {
                defer { try? handle.close() }
                _ = try? handle.seekToEnd()
                try? handle.write(contentsOf: data)
            } else {
                try? data.write(to: url)
            }
        }
    }
}

/// A system-wide keyboard shortcut that works while another app is in front.
///
/// Uses the Carbon hot-key service, which needs no accessibility permission.
public final class HotKey {
    public struct Combo: Equatable, Sendable {
        /// A virtual key code such as `kVK_ANSI_L`.
        public var keyCode: UInt32
        /// Carbon modifier flags such as `controlKey | optionKey | cmdKey`.
        public var modifiers: UInt32

        public init(keyCode: UInt32, modifiers: UInt32) {
            self.keyCode = keyCode
            self.modifiers = modifiers
        }

        /// Control-Option-Command plus a letter key.
        public static func controlOptionCommand(_ keyCode: Int) -> Combo {
            Combo(keyCode: UInt32(keyCode), modifiers: UInt32(controlKey | optionKey | cmdKey))
        }
    }

    private static var handlers: [UInt32: () -> Void] = [:]
    private static var nextID: UInt32 = 1
    private static var installed = false

    private var reference: EventHotKeyRef?
    private let id: UInt32

    /// Register `combo`. Returns nil if another app already owns it.
    public init?(_ combo: Combo, action: @escaping () -> Void) {
        Self.installHandlerIfNeeded()
        id = Self.nextID
        Self.nextID += 1
        let hotKeyID = EventHotKeyID(signature: OSType(0x4743_414D), id: id)  // 'GCAM'
        let status = RegisterEventHotKey(
            combo.keyCode, combo.modifiers, hotKeyID, GetApplicationEventTarget(), 0, &reference)
        guard status == noErr else { return nil }
        Self.handlers[id] = action
    }

    deinit {
        if let reference { UnregisterEventHotKey(reference) }
        Self.handlers[id] = nil
    }

    private static func installHandlerIfNeeded() {
        guard !installed else { return }
        installed = true
        var spec = EventTypeSpec(eventClass: OSType(kEventClassKeyboard), eventKind: UInt32(kEventHotKeyPressed))
        InstallEventHandler(
            GetApplicationEventTarget(),
            { _, event, _ in
                var hotKeyID = EventHotKeyID()
                GetEventParameter(
                    event, EventParamName(kEventParamDirectObject), EventParamType(typeEventHotKeyID), nil,
                    MemoryLayout<EventHotKeyID>.size, nil, &hotKeyID)
                HotKey.handlers[hotKeyID.id]?()
                return noErr
            }, 1, &spec, nil, nil)
    }
}
