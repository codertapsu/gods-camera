import CryptoKit
import Foundation
import Security

/// What the user stated about a face when adding it. A face cannot be saved without one.
public enum FaceConsent: String, Codable, CaseIterable, Sendable {
    /// "This is me."
    case myself
    /// "I have this person's permission."
    case permission
    /// "This face is AI-generated and shows no real person."
    case synthetic
}

/// A face the user can swap in.
public struct SavedFace: Identifiable, Codable, Equatable, Sendable {
    public let id: UUID
    public var name: String
    public let addedAt: Date
    public let consent: FaceConsent
    /// 512 numbers that stand for the face. No photo is needed to use it.
    public let identity: [Float]
    /// A small picture for the list, as PNG.
    public let thumbnail: Data

    public init(id: UUID = UUID(), name: String, addedAt: Date = Date(), consent: FaceConsent, identity: [Float], thumbnail: Data) {
        self.id = id
        self.name = name
        self.addedAt = addedAt
        self.consent = consent
        self.identity = identity
        self.thumbnail = thumbnail
    }
}

public enum FaceLibraryError: Error {
    case keychain(OSStatus)
    case unreadable(UUID)
}

/// The key that encrypts saved faces. It lives in the user's Keychain, apart
/// from the files it protects.
public enum FaceStoreKey {
    public static let defaultService = "godscamera.facestore"

    /// The key for `service`, created on first use.
    public static func load(service: String = defaultService) throws -> SymmetricKey {
        let query: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: "key",
        ]
        var found: CFTypeRef?
        var read = query
        read[kSecReturnData as String] = true
        let status = SecItemCopyMatching(read as CFDictionary, &found)
        if status == errSecSuccess, let data = found as? Data, data.count == 32 {
            return SymmetricKey(data: data)
        }
        guard status == errSecItemNotFound || status == errSecSuccess else { throw FaceLibraryError.keychain(status) }
        let key = SymmetricKey(size: .bits256)
        var add = query
        add[kSecValueData as String] = key.withUnsafeBytes { Data($0) }
        add[kSecAttrAccessible as String] = kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly
        SecItemDelete(query as CFDictionary)
        let added = SecItemAdd(add as CFDictionary, nil)
        guard added == errSecSuccess else { throw FaceLibraryError.keychain(added) }
        return key
    }

    /// Remove the key. Saved faces can no longer be read afterwards.
    public static func delete(service: String = defaultService) {
        SecItemDelete(
            [
                kSecClass as String: kSecClassGenericPassword,
                kSecAttrService as String: service,
                kSecAttrAccount as String: "key",
            ] as CFDictionary)
    }
}

/// Saved faces on disk, each in its own encrypted file.
///
/// A face's identity vector is biometric data. Files are sealed with
/// ChaCha20-Poly1305 under a key kept in the Keychain, so a copy of the folder
/// alone reveals nothing.
public final class FaceLibrary {
    public let directory: URL
    private let key: SymmetricKey

    public init(directory: URL, key: SymmetricKey) throws {
        self.directory = directory
        self.key = key
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
    }

    private func url(_ id: UUID) -> URL { directory.appendingPathComponent("\(id.uuidString).face") }

    public func save(_ face: SavedFace) throws {
        let plain = try JSONEncoder().encode(face)
        // The file name is bound into the seal, so one face's file cannot be passed off as another's.
        let sealed = try ChaChaPoly.seal(plain, using: key, authenticating: Data(face.id.uuidString.utf8))
        try sealed.combined.write(to: url(face.id), options: .atomic)
    }

    public func load(_ id: UUID) throws -> SavedFace {
        do {
            let box = try ChaChaPoly.SealedBox(combined: Data(contentsOf: url(id)))
            let plain = try ChaChaPoly.open(box, using: key, authenticating: Data(id.uuidString.utf8))
            return try JSONDecoder().decode(SavedFace.self, from: plain)
        } catch {
            throw FaceLibraryError.unreadable(id)
        }
    }

    /// Every readable face, oldest first. Files that cannot be opened are left out.
    public func list() -> [SavedFace] {
        let files = (try? FileManager.default.contentsOfDirectory(at: directory, includingPropertiesForKeys: nil)) ?? []
        return files
            .filter { $0.pathExtension == "face" }
            .compactMap { UUID(uuidString: $0.deletingPathExtension().lastPathComponent) }
            .compactMap { try? load($0) }
            .sorted { $0.addedAt < $1.addedAt }
    }

    public func delete(_ id: UUID) {
        try? FileManager.default.removeItem(at: url(id))
    }

    /// Remove every saved face.
    public func deleteAll() {
        let files = (try? FileManager.default.contentsOfDirectory(at: directory, includingPropertiesForKeys: nil)) ?? []
        for file in files where file.pathExtension == "face" {
            try? FileManager.default.removeItem(at: file)
        }
    }
}
