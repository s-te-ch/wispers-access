import Foundation
import Network
import Testing

@testable import Wispers_Access

/// Exercises the browser-facing half of the tunnel with a real `NWListener` and a
/// real loopback TCP client — no QUIC or waserver involved. Validates the
/// ephemeral loopback port binding, the accept loop, HTTP/1.1 request parsing,
/// and the 502 error page written when the upstream can't be reached.
struct LoopbackProxyTests {

    private struct UpstreamUnavailable: Error {}

    @Test func bindsLoopbackAndServesErrorPageWhenUpstreamFails() async throws {
        // A SessionManager whose storage provider always fails, so every request
        // takes the "couldn't reach the serving node" path.
        let sessions = SessionManager(storageProvider: { _ in throw UpstreamUnavailable() })
        let auth = ProxyAuth()
        let proxy = LoopbackProxy(shareID: ShareID("test-share"), sessions: sessions, auth: auth)

        let port = try await proxy.start()
        defer { proxy.stop() }
        #expect(port != 0, "proxy should bind an ephemeral loopback port")

        let response = try await httpGet(port: port, path: "/", cookie: authCookie(auth))
        #expect(response.status == 502, "upstream failure should surface as 502 Bad Gateway")
        #expect(response.body.contains("Connection problem"))
    }

    @Test func rejectsRequestWithoutAuthCookie() async throws {
        let sessions = SessionManager(storageProvider: { _ in throw UpstreamUnavailable() })
        let proxy = LoopbackProxy(
            shareID: ShareID("test-share"), sessions: sessions, auth: ProxyAuth())

        let port = try await proxy.start()
        defer { proxy.stop() }

        let bare = try await httpGet(port: port, path: "/")
        #expect(bare.status == 403, "requests without the auth cookie must be rejected")
        #expect(!bare.body.contains("Connection problem"), "403 must not carry the error page")

        let wrong = try await httpGet(
            port: port, path: "/", cookie: "\(ProxyAuth.cookieName)=0000")
        #expect(wrong.status == 403, "a wrong secret must be rejected")
    }

    @Test func acceptsAuthCookieAmongOthers() async throws {
        let sessions = SessionManager(storageProvider: { _ in throw UpstreamUnavailable() })
        let auth = ProxyAuth()
        let proxy = LoopbackProxy(shareID: ShareID("test-share"), sessions: sessions, auth: auth)

        let port = try await proxy.start()
        defer { proxy.stop() }

        // The upstream app's own cookies ride alongside ours in one header.
        let response = try await httpGet(
            port: port, path: "/", cookie: "theme=dark; \(authCookie(auth)); session=abc")
        #expect(response.status == 502, "auth must pass even with other cookies present")
    }

    private func authCookie(_ auth: ProxyAuth) -> String {
        "\(ProxyAuth.cookieName)=\(auth.secret)"
    }

    // MARK: - Minimal loopback HTTP client

    private struct HTTPResponse {
        let status: Int
        let body: String
    }

    private func httpGet(port: UInt16, path: String, cookie: String? = nil) async throws -> HTTPResponse {
        let connection = NWConnection(
            host: "127.0.0.1",
            port: NWEndpoint.Port(rawValue: port)!,
            using: .tcp
        )
        let queue = DispatchQueue(label: "test.loopback.client")
        defer { connection.cancel() }

        try await withCheckedThrowingContinuation { (cont: CheckedContinuation<Void, Error>) in
            connection.stateUpdateHandler = { state in
                switch state {
                case .ready:
                    connection.stateUpdateHandler = nil
                    cont.resume()
                case .failed(let error):
                    connection.stateUpdateHandler = nil
                    cont.resume(throwing: error)
                default:
                    break
                }
            }
            connection.start(queue: queue)
        }

        var request = "GET \(path) HTTP/1.1\r\nHost: 127.0.0.1:\(port)\r\n"
        if let cookie { request += "Cookie: \(cookie)\r\n" }
        request += "\r\n"
        try await withCheckedThrowingContinuation { (cont: CheckedContinuation<Void, Error>) in
            connection.send(content: Data(request.utf8), completion: .contentProcessed { error in
                if let error { cont.resume(throwing: error) } else { cont.resume() }
            })
        }

        var data = Data()
        while true {
            let chunk: Data = try await withCheckedThrowingContinuation { cont in
                connection.receive(minimumIncompleteLength: 1, maximumLength: 65536) { part, _, _, error in
                    if let error { cont.resume(throwing: error) } else { cont.resume(returning: part ?? Data()) }
                }
            }
            if chunk.isEmpty { break }  // server sent FIN after Connection: close
            data.append(chunk)
        }

        let text = String(decoding: data, as: UTF8.self)
        let statusLine = text.split(separator: "\r\n", maxSplits: 1).first.map(String.init) ?? ""
        let status = statusLine.split(separator: " ").dropFirst().first.flatMap { Int($0) } ?? 0
        return HTTPResponse(status: status, body: text)
    }
}
