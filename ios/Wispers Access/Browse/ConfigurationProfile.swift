import Foundation
import Network
import WispersAccessSdk

/// A share's CA certificates as a configuration profile, the only way a CA
/// gets onto an iPhone or iPad short of MDM. An app can't install one itself:
/// Safari downloads it, Settings installs it, and the user turns on full trust
/// for each CA. Certificates that aren't a CA's are left out, as iOS offers
/// full trust only for CAs.
enum ConfigurationProfile {
    /// The share's CA certificates, as the SDK describes them, with their DER.
    static func certificates(of share: Share) -> [(der: Data, description: CertificateDescription)] {
        share.caCertificates.compactMap { pem in
            guard let description = describeCertificate(pem: pem) else { return nil }
            let base64 = pem.split(separator: "\n").filter { !$0.hasPrefix("-----") }.joined()
            guard let der = Data(base64Encoded: base64) else { return nil }
            return (der, description)
        }
    }

    /// `nil` if the share has no CA certificate. Its identifier is the
    /// share's, so a profile downloaded again replaces the one installed.
    static func data(for share: Share) -> Data? {
        let payloads: [[String: Any]] = certificates(of: share)
            .filter { $0.description.kind == .ca }
            .enumerated()
            .map { index, certificate in
                [
                    "PayloadType": "com.apple.security.root",
                    "PayloadVersion": 1,
                    "PayloadIdentifier": "dev.wispers.access.share.\(share.id).\(index)",
                    "PayloadUUID": UUID().uuidString,
                    "PayloadDisplayName": certificate.description.subject,
                    "PayloadContent": certificate.der,
                ]
            }
        guard !payloads.isEmpty else { return nil }
        let name = share.name.isEmpty ? "A share" : share.name
        let profile: [String: Any] = [
            "PayloadType": "Configuration",
            "PayloadVersion": 1,
            "PayloadIdentifier": "dev.wispers.access.share.\(share.id)",
            "PayloadUUID": UUID().uuidString,
            "PayloadDisplayName": "\(name) (Wispers Access)",
            "PayloadDescription":
                "The certificate \(name)'s apps need. Only install it if you trust this share. Once it's fully trusted, the certificate's owner can make any website show up as secure.",
            "PayloadContent": payloads,
        ]
        return try? PropertyListSerialization.data(fromPropertyList: profile, format: .xml, options: 0)
    }
}

/// Serves one configuration profile on a loopback port for Safari to download,
/// for as long as it is kept. Safari fetches it at once, so iOS reclaiming a
/// suspended app's listening sockets (see `PerAppProxy.rebind`) doesn't get in
/// the way.
@MainActor
final class ProfileServer {
    private let profile: Data
    private var listener: NWListener?

    init(profile: Data) {
        self.profile = profile
    }

    /// Starts listening on a free port and returns the profile's URL there.
    func start() async throws -> URL {
        let parameters = NWParameters.tcp
        parameters.requiredLocalEndpoint = .hostPort(host: .ipv4(.loopback), port: .any)
        let listener = try NWListener(using: parameters)
        self.listener = listener
        listener.newConnectionHandler = { [profile] connection in
            Self.serve(profile, on: connection)
        }
        let port: UInt16 = try await withCheckedThrowingContinuation { continuation in
            listener.stateUpdateHandler = { state in
                switch state {
                case .ready:
                    listener.stateUpdateHandler = nil
                    continuation.resume(returning: listener.port?.rawValue ?? 0)
                case .failed(let error):
                    listener.stateUpdateHandler = nil
                    continuation.resume(throwing: error)
                default:
                    break
                }
            }
            listener.start(queue: .main)
        }
        return URL(string: "http://localhost:\(port)/wispers-access.mobileconfig")!
    }

    func stop() {
        listener?.cancel()
        listener = nil
    }

    /// Answers whatever is asked with the profile, then closes.
    private nonisolated static func serve(_ profile: Data, on connection: NWConnection) {
        connection.start(queue: .main)
        connection.receive(minimumIncompleteLength: 1, maximumLength: 8192) { _, _, _, _ in
            var response = Data(
                ("HTTP/1.1 200 OK\r\nContent-Type: application/x-apple-aspen-config\r\n"
                    + "Content-Length: \(profile.count)\r\nConnection: close\r\n\r\n").utf8)
            response.append(profile)
            connection.send(content: response, completion: .contentProcessed { _ in connection.cancel() })
        }
    }
}
