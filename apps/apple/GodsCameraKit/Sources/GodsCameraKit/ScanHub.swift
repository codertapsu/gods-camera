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

    public init(
        id: String, receivedAt: Date, status: String, progress: Double, pictures: Int, title: String, folder: URL,
        error: String?
    ) {
        self.id = id
        self.receivedAt = receivedAt
        self.status = status
        self.progress = progress
        self.pictures = pictures
        self.title = title
        self.folder = folder
        self.error = error
    }

    public var markdown: URL { folder.appendingPathComponent("document.md") }
    public var text: URL { folder.appendingPathComponent("document.txt") }
    public var isDone: Bool { status == "done" }
}

public enum ScanHubError: Error {
    case keychain(OSStatus)
    /// The hub is not running, so there is nothing to connect a device to.
    case notRunning
}

/// A phone or tablet that was connected with a code and may send scans over the network.
public struct PairedDevice: Identifiable, Equatable, Sendable {
    public let id: String
    public let name: String
    public let pairedAt: Date
    /// When the device last sent something, or nil if it has not yet.
    public let lastSeen: Date?

    public init(id: String, name: String, pairedAt: Date, lastSeen: Date?) {
        self.id = id
        self.name = name
        self.pairedAt = pairedAt
        self.lastSeen = lastSeen
    }
}

/// The state of the code on offer to a device.
public struct PairingStatus: Equatable, Sendable {
    /// A code is on offer and can still be used.
    public var isOpen: Bool
    public var secondsLeft: Int
    public var attemptsLeft: Int
    /// The device that used the code, once one has.
    public var pairedDevice: String?

    public init(isOpen: Bool, secondsLeft: Int, attemptsLeft: Int, pairedDevice: String?) {
        self.isOpen = isOpen
        self.secondsLeft = secondsLeft
        self.attemptsLeft = attemptsLeft
        self.pairedDevice = pairedDevice
    }
}

/// How the camera stands, for a paired device that works it from its Camera
/// tab (`api/remote.md`).
public struct DeviceCameraState: Equatable, Sendable {
    public struct Face: Equatable, Sendable {
        public var id: String
        public var name: String

        public init(id: String, name: String) {
            self.id = id
            self.name = name
        }
    }

    public var mode: CameraEngine.Mode
    public var recording = false
    public var recordedSeconds = 0.0
    public var canRecord = false
    public var canLoop = false
    public var swapWanted = false
    public var swapping = false
    /// Why the swap is not on the face while it is wanted: `getting_ready`,
    /// `model_missing`, `failed`, `no_face_seen` or `not_on_face`.
    public var swapProblem: String?
    public var selectedFace: String?
    /// An app is reading the camera: someone in a call sees the picture.
    public var watched = false
    /// The rules for swapping a face were accepted on this Mac. A device
    /// cannot turn the swap on before they are.
    public var rulesAccepted = false
    public var faces: [Face] = []

    public init(mode: CameraEngine.Mode) {
        self.mode = mode
    }
}

/// What a paired device asked the camera to do. The hub has refused already
/// what cannot be done now.
public enum DeviceCameraRequest: Equatable, Sendable {
    case swap(on: Bool)
    case face(id: String)
    /// Start a recording, or end the one that runs.
    case record
    /// Play the loop, or go back to live.
    case loop
}

/// The hub inside the Mac app: the local server the phone or tablet sends
/// pictures to, and the documents it has made from them.
///
/// It listens in two places. On this Mac's own loopback address, for a
/// device attached for debugging, which reaches it after `adb reverse`. And,
/// when asked to, on the local network over TLS, for devices that were
/// connected with a six-digit code: each of those has a token of its own,
/// and trusts only this hub's certificate.
public final class ScanHub: @unchecked Sendable {
    public static let defaultPort: UInt16 = 47800
    /// Where devices on the local network reach the hub.
    public static let defaultNetworkPort: UInt16 = 47801

    public let dataFolder: URL
    private let reader: URL?
    private let lock = NSLock()
    private var server: HubServer?
    private let advertiser = HubAdvertiser()
    /// Paired devices may work the camera, also after the hub starts again.
    private var cameraOffered = false

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

    /// The hub's side of the local network, for `start`.
    public struct Network: Sendable {
        /// The port to listen on; 0 lets the system choose.
        public var port: UInt16
        /// The name devices show for this hub, such as the Mac's name.
        public var name: String
        /// The hub's certificate and private key, from `ScanHub.identity()`.
        public var identity: String
        /// Tell devices on the network where the hub is. Off in tests.
        public var announce: Bool

        public init(port: UInt16 = ScanHub.defaultNetworkPort, name: String, identity: String, announce: Bool = true) {
            self.port = port
            self.name = name
            self.identity = identity
            self.announce = announce
        }
    }

    /// Start listening. Pass port 0 to let the system choose one.
    ///
    /// With `network`, the hub also listens on the local network. If that
    /// cannot start, for instance because its port is taken, the hub still
    /// starts for devices attached for debugging, and `networkPort` is nil.
    public func start(port: UInt16 = ScanHub.defaultPort, token: String, network: Network? = nil) throws {
        lock.lock()
        defer { lock.unlock() }
        guard server == nil else { return }
        try FileManager.default.createDirectory(at: dataFolder, withIntermediateDirectories: true)
        let command = reader.map { [$0.path] } ?? []
        if let network {
            do {
                let started = try HubServer.startWithLan(
                    port: port, token: token, dataDir: dataFolder.path, ocrCommand: command,
                    lan: HubLan(port: network.port, identityPem: network.identity, name: network.name))
                server = started
                if cameraOffered { started.offerCamera() }
                if network.announce, let listening = started.lanPort(), let fingerprint = started.identityFingerprint() {
                    advertiser.start(name: network.name, port: listening, fingerprint: fingerprint)
                }
                return
            } catch {
                Log.app.error("hub could not listen on the network: \(String(describing: error))")
            }
        }
        let started = try HubServer.start(port: port, token: token, dataDir: dataFolder.path, ocrCommand: command)
        server = started
        if cameraOffered { started.offerCamera() }
    }

    public func stop() {
        lock.lock()
        defer { lock.unlock() }
        advertiser.stop()
        server?.stop()
        server = nil
    }

    /// The port devices on the local network reach the hub at, while it listens there.
    public var networkPort: UInt16? {
        lock.lock()
        defer { lock.unlock() }
        return server?.lanPort()
    }

    /// The SHA-256 of the hub's certificate, in hex. Paired devices trust only this certificate.
    public var fingerprint: String? {
        lock.lock()
        defer { lock.unlock() }
        return server?.identityFingerprint()
    }

    /// Whether the hub's announcement on the local network is up, refused or failed.
    public var announcement: HubAdvertiser.State { advertiser.state }

    // MARK: Devices connected with a code

    private var running: HubServer? {
        lock.lock()
        defer { lock.unlock() }
        return server
    }

    /// Offer a six-digit code to one device and return it. The code is good
    /// for five minutes, for one device, and for five wrong tries.
    public func openPairing() throws -> String {
        guard let server = running, server.lanPort() != nil else { throw ScanHubError.notRunning }
        return try server.openPairing(seconds: 0)
    }

    /// Withdraw the code on offer.
    public func closePairing() {
        running?.closePairing()
    }

    public func pairingStatus() -> PairingStatus? {
        guard let state = running?.pairingStatus() else { return nil }
        return PairingStatus(
            isOpen: state.open, secondsLeft: Int(state.secondsLeft), attemptsLeft: Int(state.attemptsLeft),
            // The hub remembers the device that paired last; it counts only for the code that it used.
            pairedDevice: state.ended == .paired ? state.paired?.name : nil)
    }

    /// The devices that may send scans over the network.
    public func devices() -> [PairedDevice] {
        (running?.devices() ?? []).map { device in
            PairedDevice(
                id: device.id, name: device.name, pairedAt: Date(timeIntervalSince1970: TimeInterval(device.pairedAt)),
                lastSeen: device.lastSeen > device.pairedAt ? Date(timeIntervalSince1970: TimeInterval(device.lastSeen)) : nil)
        }
        .sorted { $0.pairedAt < $1.pairedAt }
    }

    /// Take a device's right to send scans away. It takes effect at once.
    public func removeDevice(_ id: String) {
        _ = try? running?.revokeDevice(id: id)
    }

    // MARK: The camera for paired devices

    /// Let paired devices work the camera from their Camera tab. The hub
    /// answers them from what `updateCamera` puts in, so a device never waits
    /// on the app, and keeps doing so when it starts again.
    public func offerCamera() {
        lock.lock()
        defer { lock.unlock() }
        cameraOffered = true
        server?.offerCamera()
    }

    /// How the camera stands now. Cheap: call it a few times a second.
    public func updateCamera(_ state: DeviceCameraState) {
        let mode: String
        switch state.mode {
        case .live: mode = "live"
        case .looping: mode = "looping"
        case .idle: mode = "idle"
        }
        running?.updateCamera(
            camera: CameraForDevices(
                mode: mode, recording: state.recording, recordedSeconds: state.recordedSeconds, canRecord: state.canRecord,
                canLoop: state.canLoop, swapWanted: state.swapWanted, swapping: state.swapping, swapProblem: state.swapProblem,
                selectedFace: state.selectedFace, watched: state.watched, rulesAccepted: state.rulesAccepted,
                faces: state.faces.map { DeviceFace(id: $0.id, name: $0.name) }))
    }

    /// Whether the hub still lacks a face's small picture.
    public func needsFacePicture(id: String) -> Bool {
        guard let server = running else { return false }
        return !server.hasFacePicture(id: id)
    }

    /// A face's small picture, PNG.
    public func updateFacePicture(id: String, png: Data) {
        running?.updateFacePicture(id: id, png: png)
    }

    /// A device asked for the picture in the last few seconds, so a new one is worth making.
    public var cameraPictureWanted: Bool { running?.cameraPictureWanted() ?? false }

    /// The picture others see, as a JPEG some 640 pixels wide.
    public func updateCameraPicture(_ jpeg: Data) {
        running?.updateCameraPicture(jpeg: jpeg)
    }

    /// What devices asked for since the last call, oldest first.
    public func takeCameraRequests() -> [DeviceCameraRequest] {
        (running?.takeCameraRequests() ?? []).map { request in
            switch request {
            case .swap(let on): return .swap(on: on)
            case .face(let id): return .face(id: id)
            case .record: return .record
            case .loop: return .loop
            }
        }
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

    // MARK: Secrets in the Keychain

    private static func keychainRead(service: String, account: String) throws -> Data? {
        let query: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: account,
            kSecReturnData as String: true,
        ]
        var found: CFTypeRef?
        let status = SecItemCopyMatching(query as CFDictionary, &found)
        if status == errSecItemNotFound { return nil }
        guard status == errSecSuccess else { throw ScanHubError.keychain(status) }
        return found as? Data
    }

    private static func keychainWrite(service: String, account: String, _ data: Data) throws {
        let query: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: account,
        ]
        var add = query
        add[kSecValueData as String] = data
        add[kSecAttrAccessible as String] = kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly
        SecItemDelete(query as CFDictionary)
        let added = SecItemAdd(add as CFDictionary, nil)
        guard added == errSecSuccess else { throw ScanHubError.keychain(added) }
    }

    /// The token a device attached for debugging must present, kept in the Keychain and made on first use.
    public static func token(service: String = "godscamera.hub") throws -> String {
        if let data = try keychainRead(service: service, account: "token"),
            let token = String(data: data, encoding: .utf8), token.count >= 16
        {
            return token
        }
        var bytes = [UInt8](repeating: 0, count: 16)
        let made = SecRandomCopyBytes(kSecRandomDefault, bytes.count, &bytes)
        guard made == errSecSuccess else { throw ScanHubError.keychain(made) }
        let token = bytes.map { String(format: "%02x", $0) }.joined()
        try keychainWrite(service: service, account: "token", Data(token.utf8))
        return token
    }

    /// The hub's certificate and private key for the local network, kept in
    /// the Keychain and made on first use. Devices that were connected with a
    /// code trust this certificate and no other, so it must stay the same.
    public static func identity(service: String = "godscamera.hub", name: String) throws -> String {
        if let data = try keychainRead(service: service, account: "identity"),
            let identity = String(data: data, encoding: .utf8), (try? hubIdentityFingerprint(identityPem: identity)) != nil
        {
            return identity
        }
        let identity = try generateHubIdentity(name: name)
        try keychainWrite(service: service, account: "identity", Data(identity.utf8))
        return identity
    }

    /// Forget the hub's certificate. Every connected device must then be connected again with a new code.
    public static func deleteIdentity(service: String = "godscamera.hub") {
        SecItemDelete(
            [
                kSecClass as String: kSecClassGenericPassword,
                kSecAttrService as String: service,
                kSecAttrAccount as String: "identity",
            ] as CFDictionary)
    }
}
