import Foundation

/// A single HTTP header. Mutable so a rewriter can edit it in place.
struct HTTPHeader: Sendable {
    var name: String
    var value: String
}

/// Header transforms applied at the proxy boundary, both directions. The proxy
/// core manages framing and hop-by-hop headers around these calls, so a rewrite
/// can't accidentally break framing. Port of the Android `HeaderRewriter`.
nonisolated struct HeaderRewriter: Sendable {
    func rewriteRequest(
        method: String,
        target: String,
        headers: inout [HTTPHeader],
        shareID: ShareID
    ) {
        // The proxy-auth cookie authenticates the web view to the loopback
        // proxy; it must not travel upstream.
        for i in headers.indices where headers[i].name.lowercased() == "cookie" {
            headers[i].value = Self.stripProxyAuthPair(headers[i].value)
        }
        headers.removeAll { $0.name.lowercased() == "cookie" && $0.value.isEmpty }
    }

    func rewriteResponse(
        status: Int,
        headers: inout [HTTPHeader],
        shareID: ShareID
    ) {
        // The upstream app mustn't clobber the proxy-auth cookie in the web
        // view's store.
        headers.removeAll {
            $0.name.lowercased() == "set-cookie"
                && ProxyAuth.cookiePairs($0.value).first?.name == ProxyAuth.cookieName
        }
        // Strip the Set-Cookie `Domain=` attribute so cookies bind to the
        // loopback origin the WebView actually talks to, not whatever host the
        // upstream app assumed it was running on.
        for i in headers.indices where headers[i].name.lowercased() == "set-cookie" {
            headers[i].value = Self.stripDomainAttribute(headers[i].value)
        }
    }

    private static func stripProxyAuthPair(_ cookie: String) -> String {
        cookie
            .split(separator: ";")
            .map { $0.trimmingCharacters(in: .whitespaces) }
            .filter { !$0.hasPrefix(ProxyAuth.cookieName + "=") }
            .joined(separator: "; ")
    }

    private static func stripDomainAttribute(_ setCookie: String) -> String {
        setCookie
            .split(separator: ";", omittingEmptySubsequences: false)
            .filter { !$0.trimmingCharacters(in: .whitespaces).lowercased().hasPrefix("domain=") }
            .joined(separator: ";")
    }
}
