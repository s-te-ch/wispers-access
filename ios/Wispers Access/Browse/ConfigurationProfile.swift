import Foundation
import Network
import UIKit
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
            "PayloadDisplayName": displayName(for: share),
            "PayloadDescription":
                "The certificate \(name)'s apps need. Only install it if you trust this share. Once it's fully trusted, the certificate's owner can make any website show up as secure.",
            "PayloadContent": payloads,
        ]
        return try? PropertyListSerialization.data(fromPropertyList: profile, format: .xml, options: 0)
    }

    /// The profile's name, as Settings lists it.
    static func displayName(for share: Share) -> String {
        "\(share.name.isEmpty ? "A share" : share.name) (Wispers Access)"
    }

    /// The page Safari opens to download the profile. It starts the download
    /// and keeps the remaining steps on screen, because once the user allows
    /// the download, Safari is where they are, not the app.
    static func instructionsPage(profileName: String, caSubject: String, device: String) -> Data {
        let profile = escapeHTML(profileName)
        let subject = escapeHTML(caSubject)
        let html = """
            <!doctype html>
            <html lang="en">
            <meta charset="utf-8">
            <meta name="viewport" content="width=device-width, initial-scale=1">
            <title>Install the certificate</title>
            <style>
              body { font: -apple-system-body; font-family: system-ui; background: #F7F7F2; color: #1A1A1A;
                     max-width: 34rem; margin: 0 auto; padding: 1.5rem 1.25rem; line-height: 1.45; }
              h1 { font-size: 1.5rem; margin: 0 0 1rem; }
              ol { list-style: none; padding: 0; margin: 0; }
              li { display: flex; gap: .85rem; margin: 0 0 1.1rem; }
              .n { flex: none; width: 1.75rem; height: 1.75rem; border-radius: 50%; background: #A1D283;
                   color: #4A6B36; font-weight: 700; display: flex; align-items: center; justify-content: center; }
              .done .n { background: #34A853; color: #fff; }
              b { display: block; }
              p { color: #6B6B68; font-size: .9rem; }
            </style>
            <h1>Next, in Settings</h1>
            <ol>
              <li class="done"><span class="n">✓</span><div><b>Download the profile</b>
                When Safari asks whether to allow it, tap Allow.</div></li>
              <li><span class="n">2</span><div><b>Install it</b>
                Open Settings › General › VPN &amp; Device Management. Tap “\(profile)”, then Install.</div></li>
              <li><span class="n">3</span><div><b>Turn on trust</b>
                Go back to General › About. At the very bottom, tap Certificate Trust Settings and turn on “\(subject)”.</div></li>
              <li><span class="n">4</span><div><b>Go back to Wispers Access</b>
                The app opens by itself once your \(device) trusts the certificate.</div></li>
            </ol>
            <p>If Safari didn't ask about a profile, go back to Wispers Access and tap Download Again.</p>
            <script>window.onload = function () { location.href = "\(ProfileServer.profilePath)"; };</script>
            </html>
            """
        return Data(html.utf8)
    }

    private static func escapeHTML(_ text: String) -> String {
        text.replacingOccurrences(of: "&", with: "&amp;")
            .replacingOccurrences(of: "<", with: "&lt;")
            .replacingOccurrences(of: ">", with: "&gt;")
            .replacingOccurrences(of: "\"", with: "&quot;")
    }
}

/// Serves the instructions page and its configuration profile on a loopback
/// port for Safari, for as long as it is kept. iOS reclaims a suspended app's
/// listening sockets (see `PerAppProxy.rebind`), so the server holds a
/// background task until Safari has fetched the profile.
@MainActor
final class ProfileServer {
    nonisolated static let profilePath = "/wispers-access.mobileconfig"

    private let profile: Data
    private let page: Data
    private var listener: NWListener?
    private var backgroundTask: UIBackgroundTaskIdentifier = .invalid

    init(profile: Data, page: Data) {
        self.profile = profile
        self.page = page
    }

    /// Starts listening on a free port and returns the page's URL there.
    func start() async throws -> URL {
        let parameters = NWParameters.tcp
        parameters.requiredLocalEndpoint = .hostPort(host: .ipv4(.loopback), port: .any)
        let listener = try NWListener(using: parameters)
        self.listener = listener
        listener.newConnectionHandler = { [profile, page, weak self] connection in
            Self.serve(profile: profile, page: page, on: connection) {
                Task { @MainActor in self?.endBackgroundTask() }
            }
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
        endBackgroundTask()
        backgroundTask = UIApplication.shared.beginBackgroundTask(withName: "Configuration profile") { [weak self] in
            self?.endBackgroundTask()
        }
        return URL(string: "http://localhost:\(port)/")!
    }

    func stop() {
        listener?.cancel()
        listener = nil
        endBackgroundTask()
    }

    /// Answers `/` with the page and `profilePath` with the profile, then
    /// closes. `onProfileServed` runs once the profile is sent.
    private nonisolated static func serve(
        profile: Data, page: Data, on connection: NWConnection,
        onProfileServed: @escaping @Sendable () -> Void
    ) {
        connection.start(queue: .main)
        connection.receive(minimumIncompleteLength: 1, maximumLength: 8192) { request, _, _, _ in
            let requestLine = request.flatMap { String(decoding: $0, as: UTF8.self).split(separator: "\r\n").first } ?? ""
            let path = requestLine.split(separator: " ").dropFirst().first.map(String.init) ?? ""
            let (status, type, body): (String, String, Data) = switch path {
            case "/": ("200 OK", "text/html; charset=utf-8", page)
            case profilePath: ("200 OK", "application/x-apple-aspen-config", profile)
            default: ("404 Not Found", "text/plain", Data("Not found\n".utf8))
            }
            var response = Data(
                ("HTTP/1.1 \(status)\r\nContent-Type: \(type)\r\n"
                    + "Content-Length: \(body.count)\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n").utf8)
            response.append(body)
            connection.send(content: response, completion: .contentProcessed { _ in
                connection.cancel()
                if path == profilePath { onProfileServed() }
            })
        }
    }

    private func endBackgroundTask() {
        guard backgroundTask != .invalid else { return }
        UIApplication.shared.endBackgroundTask(backgroundTask)
        backgroundTask = .invalid
    }
}
