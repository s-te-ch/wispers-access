import Foundation
import Testing

@testable import Wispers_Access

/// The proxy-auth cookie must stay strictly between the web view and the
/// loopback proxy: outbound requests lose it, and the upstream app can
/// neither read nor overwrite it.
struct HeaderRewriterTests {
    private let rewriter = HeaderRewriter()
    private let share = ShareID("test-share")

    @Test func stripsProxyAuthCookieFromRequests() {
        var headers = [
            HTTPHeader(name: "Cookie", value: "theme=dark; \(ProxyAuth.cookieName)=s3cret; session=abc")
        ]
        rewriter.rewriteRequest(method: "GET", target: "/", headers: &headers, shareID: share)
        #expect(headers.count == 1)
        #expect(headers[0].value == "theme=dark; session=abc")
    }

    @Test func dropsCookieHeaderWhenOnlyProxyAuthRemains() {
        var headers = [HTTPHeader(name: "Cookie", value: "\(ProxyAuth.cookieName)=s3cret")]
        rewriter.rewriteRequest(method: "GET", target: "/", headers: &headers, shareID: share)
        #expect(headers.isEmpty, "an empty Cookie header must not travel upstream")
    }

    @Test func dropsUpstreamSetCookieForProxyAuthName() {
        var headers = [
            HTTPHeader(name: "Set-Cookie", value: "\(ProxyAuth.cookieName)=evil; Path=/"),
            HTTPHeader(name: "Set-Cookie", value: "session=abc; Path=/"),
        ]
        rewriter.rewriteResponse(status: 200, headers: &headers, shareID: share)
        #expect(headers.count == 1)
        #expect(headers[0].value == "session=abc; Path=/")
    }

    @Test func stripsDomainAttributeFromSetCookie() {
        var headers = [
            HTTPHeader(name: "Set-Cookie", value: "session=abc; Domain=app.internal; Path=/")
        ]
        rewriter.rewriteResponse(status: 200, headers: &headers, shareID: share)
        #expect(headers[0].value == "session=abc; Path=/")
    }
}
