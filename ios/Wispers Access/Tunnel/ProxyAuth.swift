import Foundation

/// Authenticates loopback-proxy requests as coming from this app's own web
/// views. The loopback port is reachable by every process on the device, so
/// holding it open would otherwise hand any co-resident app read access to the
/// share. Requests must present a secret only our web views hold: a session
/// cookie for 127.0.0.1, installed into the WebKit cookie store before the
/// first load. The secret is fresh per launch; anything without it gets a 403
/// before a QUIC stream is ever opened.
nonisolated struct ProxyAuth: Sendable {
    /// One secret for the whole process: all proxies share the 127.0.0.1
    /// cookie origin anyway (cookies ignore ports), so per-share secrets
    /// would all be presented to every proxy regardless.
    static let shared = ProxyAuth()
    static let cookieName = "__wispers_proxy_auth"

    let secret: String

    init() {
        var bytes = [UInt8](repeating: 0, count: 16)
        for i in bytes.indices { bytes[i] = UInt8.random(in: .min ... .max) }
        secret = bytes.map { String(format: "%02x", $0) }.joined()
    }

    /// The cookie to install into a web view's cookie store.
    func cookie() -> HTTPCookie {
        HTTPCookie(properties: [
            .domain: "127.0.0.1",
            .path: "/",
            .name: Self.cookieName,
            .value: secret,
        ])!
    }

    /// Whether the request's Cookie headers carry the secret.
    func authorizes(_ headers: [HTTPHeader]) -> Bool {
        for header in headers where header.name.lowercased() == "cookie" {
            for (name, value) in Self.cookiePairs(header.value) {
                if name == Self.cookieName, constantTimeEquals(value, secret) {
                    return true
                }
            }
        }
        return false
    }

    /// Parses a Cookie header value (`a=1; b=2`) into name/value pairs.
    static func cookiePairs(_ headerValue: String) -> [(name: String, value: String)] {
        headerValue.split(separator: ";").compactMap { pair in
            let trimmed = pair.trimmingCharacters(in: .whitespaces)
            guard let eq = trimmed.firstIndex(of: "=") else { return nil }
            return (String(trimmed[..<eq]), String(trimmed[trimmed.index(after: eq)...]))
        }
    }

    /// Comparison whose duration doesn't depend on where the strings differ,
    /// so a local process can't binary-search the secret via response timing.
    private func constantTimeEquals(_ a: String, _ b: String) -> Bool {
        let aBytes = Array(a.utf8)
        let bBytes = Array(b.utf8)
        guard aBytes.count == bBytes.count else { return false }
        var diff: UInt8 = 0
        for i in aBytes.indices { diff |= aBytes[i] ^ bBytes[i] }
        return diff == 0
    }
}
