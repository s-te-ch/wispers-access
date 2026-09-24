import Foundation
import WispersAccessSdk

/// Authenticates loopback-proxy requests as coming from this app's own web
/// views. The loopback port is reachable by every process on the device, so
/// the SDK's proxy demands a secret only our web views hold: a cookie for
/// 127.0.0.1, installed into the WebKit cookie store before the first load.
/// The secret is fresh per launch.
nonisolated struct ProxyAuth: Sendable {
    static let cookieName = "__wispers_proxy_auth"

    let secret: String

    init() {
        var bytes = [UInt8](repeating: 0, count: 16)
        for i in bytes.indices { bytes[i] = UInt8.random(in: .min ... .max) }
        secret = bytes.map { String(format: "%02x", $0) }.joined()
    }

    /// What the SDK's proxy requires on every request.
    var requiredCookie: RequiredCookie {
        RequiredCookie(name: Self.cookieName, value: secret)
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
}
