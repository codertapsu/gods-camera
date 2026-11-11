import Foundation
import GCCore
import Security

/// A document made from pictures a phone or tablet sent.
public struct ScannedDocument: Identifiable, Equatable, Sendable {
    public let id: String
    public let receivedAt: Date
    /// `queued`, `running`, `done` or `failed`.
    public let status: String
    /// 0 to 1 while the text is being read.
    public let progress: Double
    public let pictures: Int
    /// The document's first line, or an empty string until it is done.
    public let title: String
    public let folder: URL
    public let error: String?

    public var markdown: URL { folder.appendingPathComponent("document.md") }
    public var text: URL { folder.appendingPathComponent("document.txt") }
    public var isDone: Bool { status == "done" }
}

public enum ScanHubError: Error {
    case keychain(OSStatus)
}

/// The hub inside the Mac app: the local server the phone or tablet sends
/// pictures to, and the documents it has made from them.
///
/// It listens only on this Mac's own loopback address. A device on a USB
/// cable reaches it after `adb reverse`.
public final class ScanHub: @unchecked Sendable {
    public static let defaultPort: UInt16 = 47800

    public let dataFolder: URL
    private let reader: URL?
    private let lock = NSLock()
    private var server: HubServer?

    /// `reader` is the program that reads text from pictures. Without it the
    /// hub runs but refuses documents.
    public init(dataFolder: URL, reader: URL?) {
        self.dataFolder = dataFolder
        self.reader = reader
    }

    public var isRunning: Bool {
        lock.lock()
        defer { lock.unlock() }
        return server != nil
    }

    /// Where the hub listens, as a URL, while it runs.
    public var address: String? {
        lock.lock()
        defer { lock.unlock() }
        return server?.address()
    }

    /// Start listening. Pass port 0 to let the system choose one.
    public func start(port: UInt16 = ScanHub.defaultPort, token: String) throws {
        lock.lock()
        defer { lock.unlock() }
        guard server == nil else { return }
        try FileManager.default.createDirectory(at: dataFolder, withIntermediateDirectories: true)
        server = try HubServer.start(
            port: port, token: token, dataDir: dataFolder.path, ocrCommand: reader.map { [$0.path] } ?? [])
    }

    public func stop() {
        lock.lock()
        defer { lock.unlock() }
        server?.stop()
        server = nil
    }

    /// The documents the hub has made or is making, newest first.
    public func documents() -> [ScannedDocument] {
        let jobs = dataFolder.appendingPathComponent("jobs")
        let folders = (try? FileManager.default.contentsOfDirectory(
            at: jobs, includingPropertiesForKeys: [.creationDateKey])) ?? []
        return folders.compactMap { folder -> ScannedDocument? in
            guard let data = try? Data(contentsOf: folder.appendingPathComponent("job.json")),
                let job = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
                job["kind"] as? String == "document", let id = job["id"] as? String
            else { return nil }
            let status = job["status"] as? String ?? "failed"
            let result = job["result"] as? [String: Any]
            var title = ""
            if status == "done",
                let text = try? String(contentsOf: folder.appendingPathComponent("document.txt"), encoding: .utf8)
            {
                title = text.split(whereSeparator: \.isNewline).first.map(String.init) ?? ""
            }
            let created = (try? folder.resourceValues(forKeys: [.creationDateKey]).creationDate) ?? Date.distantPast
            return ScannedDocument(
                id: id, receivedAt: created, status: status, progress: (job["progress"] as? Double) ?? 0,
                pictures: (result?["pictures"] as? Int) ?? 0, title: title, folder: folder,
                error: job["error"] as? String)
        }
        .sorted { $0.receivedAt > $1.receivedAt }
    }

    /// Remove a document and the pictures it came from.
    public func delete(_ document: ScannedDocument) {
        if let data = try? Data(contentsOf: document.folder.appendingPathComponent("job.json")),
            let job = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
            let upload = job["upload_id"] as? String, UUID(uuidString: upload) != nil
        {
            try? FileManager.default.removeItem(at: dataFolder.appendingPathComponent("uploads").appendingPathComponent(upload))
        }
        try? FileManager.default.removeItem(at: document.folder)
    }

    /// The token devices must present, kept in the Keychain and made on first use.
    public static func token(service: String = "godscamera.hub") throws -> String {
        let query: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: "token",
        ]
        var found: CFTypeRef?
        var read = query
        read[kSecReturnData as String] = true
        let status = SecItemCopyMatching(read as CFDictionary, &found)
        if status == errSecSuccess, let data = found as? Data, let token = String(data: data, encoding: .utf8),
            token.count >= 16
        {
            return token
        }
        guard status == errSecItemNotFound || status == errSecSuccess else { throw ScanHubError.keychain(status) }
        var bytes = [UInt8](repeating: 0, count: 16)
        let made = SecRandomCopyBytes(kSecRandomDefault, bytes.count, &bytes)
        guard made == errSecSuccess else { throw ScanHubError.keychain(made) }
        let token = bytes.map { String(format: "%02x", $0) }.joined()
        var add = query
        add[kSecValueData as String] = Data(token.utf8)
        add[kSecAttrAccessible as String] = kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly
        SecItemDelete(query as CFDictionary)
        let added = SecItemAdd(add as CFDictionary, nil)
        guard added == errSecSuccess else { throw ScanHubError.keychain(added) }
        return token
    }
}
