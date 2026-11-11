import Foundation
import dnssd

/// Tells phones and tablets on the local network where the hub is, through
/// Bonjour. The app on a device lists the Macs it finds this way.
public final class HubAdvertiser: @unchecked Sendable {
    public static let serviceType = "_godscamera._tcp"

    public enum State: Equatable, Sendable {
        case off
        /// Asked for; the system has not answered yet.
        case starting
        /// Devices can find the hub under this name.
        case announced(String)
        /// The user has not allowed the app to use the local network.
        case refused
        case failed(Int32)
    }

    private let lock = NSLock()
    private var ref: DNSServiceRef?
    private var current = State.off
    private let queue = DispatchQueue(label: "dev.godscamera.advertiser")

    public init() {}

    public var state: State {
        lock.lock()
        defer { lock.unlock() }
        return current
    }

    private func set(_ state: State) {
        lock.lock()
        current = state
        lock.unlock()
    }

    /// The service's text record: each entry is a length byte and `key=value`.
    static func textRecord(name: String, fingerprint: String) -> Data {
        var record = Data()
        for entry in ["v=1", "name=\(name)", "fp=\(fingerprint.prefix(16))"] {
            let bytes = Array(entry.utf8.prefix(255))
            record.append(UInt8(bytes.count))
            record.append(contentsOf: bytes)
        }
        return record
    }

    /// Announce the hub under `name` at `port`. `fingerprint` is the hub
    /// certificate's; its start lets a device pick its own Mac out of several.
    public func start(name: String, port: UInt16, fingerprint: String) {
        stop()
        set(.starting)
        let record = Self.textRecord(name: name, fingerprint: fingerprint)
        let context = Unmanaged.passUnretained(self).toOpaque()
        let reply: DNSServiceRegisterReply = { _, _, error, name, _, _, context in
            guard let context else { return }
            let advertiser = Unmanaged<HubAdvertiser>.fromOpaque(context).takeUnretainedValue()
            switch error {
            case DNSServiceErrorType(kDNSServiceErr_NoError):
                advertiser.set(.announced(name.map { String(cString: $0) } ?? ""))
            case DNSServiceErrorType(kDNSServiceErr_PolicyDenied): advertiser.set(.refused)
            default: advertiser.set(.failed(error))
            }
        }
        var created: DNSServiceRef?
        let status = record.withUnsafeBytes { bytes in
            DNSServiceRegister(
                &created, 0, 0, name, Self.serviceType, nil, nil, port.bigEndian, UInt16(record.count),
                bytes.baseAddress, reply, context)
        }
        guard status == kDNSServiceErr_NoError, let created else {
            set(status == kDNSServiceErr_PolicyDenied ? .refused : .failed(status))
            return
        }
        DNSServiceSetDispatchQueue(created, queue)
        lock.lock()
        ref = created
        lock.unlock()
    }

    public func stop() {
        lock.lock()
        let old = ref
        ref = nil
        current = .off
        lock.unlock()
        // Replies run on the queue; ending the service there keeps one from arriving half-way.
        if let old { queue.sync { DNSServiceRefDeallocate(old) } }
    }

    deinit { stop() }
}
