import Foundation
import LLHTTP
import Network
import WispersConnect

/// A loopback HTTP/1.1 proxy for a single share. Binds an `NWListener` to
/// 127.0.0.1 on an ephemeral port; the WebView loads `http://127.0.0.1:<port>/`.
/// Each accepted browser connection is driven through one `ProxyConnection`,
/// which forwards it over a QUIC stream to the share's serving node.
///
/// Loopback-only bind, no `*.localhost` subdomain routing like Android: the
/// share is fixed per proxy, so identity comes from which proxy (port) the
/// request arrived on, not from the Host header.
final class LoopbackProxy: @unchecked Sendable {
    let shareID: ShareID
    private let sessions: SessionManager
    private let rewriter = HeaderRewriter()
    private let queue = DispatchQueue(label: "dev.wispers.access.proxy", attributes: .concurrent)
    private var listener: NWListener?
    private(set) var port: UInt16 = 0

    init(shareID: ShareID, sessions: SessionManager) {
        self.shareID = shareID
        self.sessions = sessions
    }

    /// Starts listening on a loopback ephemeral port and returns it.
    func start() async throws -> UInt16 {
        let params = NWParameters.tcp
        params.requiredLocalEndpoint = NWEndpoint.hostPort(
            host: "127.0.0.1",
            port: NWEndpoint.Port(rawValue: 0)!
        )
        params.allowLocalEndpointReuse = true
        let listener = try NWListener(using: params)
        self.listener = listener

        let shareID = self.shareID
        let sessions = self.sessions
        let rewriter = self.rewriter
        let queue = self.queue
        listener.newConnectionHandler = { nwConnection in
            let connection = TCPConnection(nwConnection)
            Task.detached {
                await ProxyConnection(
                    browser: connection,
                    shareID: shareID,
                    sessions: sessions,
                    rewriter: rewriter,
                    queue: queue
                ).run()
            }
        }

        let port: UInt16 = try await withCheckedThrowingContinuation { cont in
            listener.stateUpdateHandler = { state in
                switch state {
                case .ready:
                    listener.stateUpdateHandler = nil
                    cont.resume(returning: listener.port?.rawValue ?? 0)
                case .failed(let error):
                    listener.stateUpdateHandler = nil
                    cont.resume(throwing: error)
                default:
                    break
                }
            }
            listener.start(queue: queue)
        }
        guard port != 0 else { throw TunnelError.missingPort }
        self.port = port
        return port
    }

    func stop() {
        listener?.cancel()
        listener = nil
    }
}

/// Drives one browser connection: parses its HTTP/1.1 request with `HTTP1Parser`
/// (llhttp), forwards it over a fresh QUIC stream to the serving node, and
/// streams the parsed response back. On a `101` upgrade it becomes a raw
/// bidirectional relay. Parsing/framing (chunked, content-length, EOF, HEAD) is
/// llhttp's; serialization + transport is ours.
///
/// One request per browser connection (`Connection: close`): simpler and robust
/// versus keep-alive, at the cost of the browser reopening loopback connections
/// (cheap) — the QUIC connection is reused across them by `SessionManager`.
nonisolated final class ProxyConnection: @unchecked Sendable {
    private let browser: TCPConnection
    private let shareID: ShareID
    private let sessions: SessionManager
    private let rewriter: HeaderRewriter
    private let queue: DispatchQueue
    private var responded = false

    init(
        browser: TCPConnection,
        shareID: ShareID,
        sessions: SessionManager,
        rewriter: HeaderRewriter,
        queue: DispatchQueue
    ) {
        self.browser = browser
        self.shareID = shareID
        self.sessions = sessions
        self.rewriter = rewriter
        self.queue = queue
    }

    func run() async {
        do {
            try await browser.start(on: queue)
        } catch {
            browser.cancel()
            return
        }
        do {
            guard let request = try await readRequest() else {
                browser.cancel()
                return
            }
            try await handle(request)
        } catch {
            if !responded { try? await writeErrorPage(error) }
        }
        browser.cancel()
    }

    // MARK: - Request (from the browser)

    private struct ParsedRequest {
        let method: String
        let target: String
        let headers: [HTTPHeader]
        let isUpgrade: Bool
        let body: Data
        /// Browser bytes past an upgrade handshake (usually empty).
        let leftover: Data
    }

    private func readRequest() async throws -> ParsedRequest? {
        let parser = HTTP1Parser(kind: .request)
        var head: HTTP1Head?
        var body = Data()
        loop: while true {
            let chunk = try await browser.readChunk(maxLength: Self.bufferSize)
            let events = chunk.isEmpty ? try parser.finish() : try parser.feed(chunk)
            for event in events {
                switch event {
                case .head(let h):
                    head = h
                    if h.isUpgrade { break loop }
                case .body(let piece):
                    body.append(piece)
                case .complete:
                    break loop
                }
            }
            if chunk.isEmpty { break }  // browser closed before a full request
        }
        guard let head else { return nil }
        return ParsedRequest(
            method: head.method ?? "GET",
            target: head.target ?? "/",
            headers: head.headers.map { HTTPHeader(name: $0.name, value: $0.value) },
            isUpgrade: head.isUpgrade,
            body: body,
            leftover: parser.upgradeLeftover
        )
    }

    private func handle(_ request: ParsedRequest) async throws {
        let lease: StreamLease
        do {
            lease = try await sessions.openStream(shareID)
        } catch {
            try await writeErrorPage(error)
            return
        }
        do {
            if request.isUpgrade {
                try await relayUpgrade(request, stream: lease.stream)
            } else {
                try await forward(request, stream: lease.stream)
            }
        } catch {
            await sessions.invalidate(shareID, lease)
            throw error
        }
        try? await lease.stream.shutdown()
    }

    // MARK: - Normal request/response

    private func forward(_ request: ParsedRequest, stream: QuicStream) async throws {
        // Serialize the request upstream. `Connection: close` so upstream FINs
        // after the response, giving llhttp a clean end for close-delimited bodies.
        var headers = request.headers.filter { !isHopByHopOrFraming($0.name, keepUpgrade: false) }
        if !request.body.isEmpty {
            headers.append(HTTPHeader(name: "Content-Length", value: String(request.body.count)))
        }
        headers.append(HTTPHeader(name: "Connection", value: "close"))
        rewriter.rewriteRequest(method: request.method, target: request.target, headers: &headers, shareID: shareID)
        try await stream.write(latin1(serializeHead("\(request.method) \(request.target) HTTP/1.1", headers)))
        if !request.body.isEmpty { try await stream.write(request.body) }

        // Parse + stream the response. A silently-dead connection blackholes
        // packets, so the wait for the response head is deadlined; the body pipe
        // isn't, so large/slow transfers aren't cut off.
        let isHead = request.method.uppercased() == "HEAD"
        let parser = HTTP1Parser(kind: .response)
        if isHead { parser.expectNoBody() }
        var sawHead = false
        var hasBody = false
        loop: while true {
            let chunk: Data
            if sawHead {
                chunk = try await stream.readChunk(maxLength: Self.bufferSize)
            } else {
                chunk = try await withDeadline(seconds: Self.exchangeTimeout) {
                    try await stream.readChunk(maxLength: Self.bufferSize)
                }
            }
            let events = chunk.isEmpty ? try parser.finish() : try parser.feed(chunk)
            for event in events {
                switch event {
                case .head(let h):
                    responded = true
                    sawHead = true
                    hasBody = responseHasBody(status: h.statusCode ?? 0, isHead: isHead)
                    try await writeResponseHead(h, hasBody: hasBody)
                case .body(let piece):
                    if hasBody { try await writeChunk(piece) }
                case .complete:
                    if hasBody { try await browser.write(latin1("0\r\n\r\n")) }
                    break loop
                }
            }
            if chunk.isEmpty { break }  // upstream closed
        }
        guard sawHead else { throw TunnelError.unexpectedEOF }
        // Half-close only after the full response — an early FIN makes upstream
        // hyper report an "incomplete message".
        try? await stream.finish()
        browser.finishSending()
    }

    private func writeResponseHead(_ head: HTTP1Head, hasBody: Bool) async throws {
        var headers = head.headers
            .map { HTTPHeader(name: $0.name, value: $0.value) }
            .filter { !isHopByHopOrFraming($0.name, keepUpgrade: false) }
        rewriter.rewriteResponse(status: head.statusCode ?? 0, headers: &headers, shareID: shareID)
        headers.append(HTTPHeader(name: "Connection", value: "close"))
        // llhttp de-frames the body, so we re-frame to the browser as chunked
        // (streaming, length unknown up front).
        if hasBody { headers.append(HTTPHeader(name: "Transfer-Encoding", value: "chunked")) }
        let status = head.statusCode ?? 0
        let reason = head.reason ?? ""
        try await browser.write(latin1(serializeHead("HTTP/1.1 \(status) \(reason)", headers)))
    }

    private func writeChunk(_ data: Data) async throws {
        try await browser.write(latin1(String(data.count, radix: 16) + "\r\n"))
        try await browser.write(data)
        try await browser.write(latin1("\r\n"))
    }

    // MARK: - Upgrade (WebSocket and other HTTP/1.1 upgrades)

    private func relayUpgrade(_ request: ParsedRequest, stream: QuicStream) async throws {
        // Serialize the handshake upstream, keeping the Connection/Upgrade/
        // Sec-WebSocket-* headers and holding the stream open (no Connection: close).
        var headers = request.headers.filter { !isHopByHopOrFraming($0.name, keepUpgrade: true) }
        rewriter.rewriteRequest(method: request.method, target: request.target, headers: &headers, shareID: shareID)
        try await stream.write(latin1(serializeHead("\(request.method) \(request.target) HTTP/1.1", headers)))

        // Read the 101 head.
        let parser = HTTP1Parser(kind: .response)
        var head: HTTP1Head?
        while head == nil {
            let chunk = try await withDeadline(seconds: Self.exchangeTimeout) {
                try await stream.readChunk(maxLength: Self.bufferSize)
            }
            if chunk.isEmpty { throw TunnelError.unexpectedEOF }
            for event in try parser.feed(chunk) {
                if case .head(let h) = event { head = h; break }
            }
        }
        guard let head, head.statusCode == 101 else { throw TunnelError.unexpectedEOF }

        responded = true
        var respHeaders = head.headers
            .map { HTTPHeader(name: $0.name, value: $0.value) }
            .filter { !isHopByHopOrFraming($0.name, keepUpgrade: true) }
        rewriter.rewriteResponse(status: 101, headers: &respHeaders, shareID: shareID)
        try await browser.write(latin1(serializeHead("HTTP/1.1 101 \(head.reason ?? "Switching Protocols")", respHeaders)))

        // Raw bidirectional relay, seeded with each parser's post-handshake
        // leftover so no bytes are dropped. Each pump ends at its own EOF.
        let browser = self.browser
        let requestLeftover = request.leftover
        let responseLeftover = parser.upgradeLeftover
        await withTaskGroup(of: Void.self) { group in
            group.addTask {
                do {
                    if !responseLeftover.isEmpty { try await browser.write(responseLeftover) }
                    while true {
                        let data = try await stream.readChunk(maxLength: Self.bufferSize)
                        if data.isEmpty { break }
                        try await browser.write(data)
                    }
                } catch {}
                browser.finishSending()
            }
            group.addTask {
                do {
                    if !requestLeftover.isEmpty { try await stream.write(requestLeftover) }
                    while true {
                        let data = try await browser.readChunk(maxLength: Self.bufferSize)
                        if data.isEmpty { break }
                        try await stream.write(data)
                    }
                } catch {}
                try? await stream.finish()
            }
            await group.next()
            await group.next()
        }
    }

    // MARK: - Error page

    private func writeErrorPage(_ error: Error) async throws {
        responded = true
        let bytes = Data(Self.errorHTML(error.localizedDescription).utf8)
        var head = "HTTP/1.1 502 Bad Gateway\r\n"
        head += "Content-Type: text/html; charset=utf-8\r\n"
        head += "Content-Length: \(bytes.count)\r\n"
        head += "Connection: close\r\n\r\n"
        try await browser.write(latin1(head))
        try await browser.write(bytes)
        browser.finishSending()
    }

    // MARK: - Helpers

    private func serializeHead(_ startLine: String, _ headers: [HTTPHeader]) -> String {
        var out = startLine + "\r\n"
        for header in headers { out += "\(header.name): \(header.value)\r\n" }
        out += "\r\n"
        return out
    }

    private func isHopByHopOrFraming(_ name: String, keepUpgrade: Bool) -> Bool {
        let lower = name.lowercased()
        if Self.framing.contains(lower) { return true }
        if Self.hopByHop.contains(lower) {
            return !(keepUpgrade && Self.upgradeHeaders.contains(lower))
        }
        return false
    }

    private func responseHasBody(status: Int, isHead: Bool) -> Bool {
        if isHead { return false }
        if status == 204 || status == 304 { return false }
        if (100...199).contains(status) { return false }
        return true
    }

    private func latin1(_ string: String) -> Data {
        string.data(using: .isoLatin1) ?? Data(string.utf8)
    }

    private static func errorHTML(_ message: String) -> String {
        let safe = message
            .replacingOccurrences(of: "&", with: "&amp;")
            .replacingOccurrences(of: "<", with: "&lt;")
            .replacingOccurrences(of: ">", with: "&gt;")
        return """
        <!doctype html>
        <html>
        <head>
        <meta name="viewport" content="width=device-width, initial-scale=1">
        <title>Connection problem</title>
        <style>
          body { font-family: -apple-system, sans-serif; margin: 0; min-height: 100vh; display: flex;
                 align-items: center; justify-content: center; background: #f7f7f2; color: #1a1a1a; }
          main { text-align: center; padding: 2rem; max-width: 28rem; }
          p { color: #6b6b68; font-size: 0.85rem; overflow-wrap: anywhere; }
          button { font-size: 1rem; padding: 0.75rem 2.5rem; border: none; border-radius: 1.5rem;
                   background: #a1d283; color: #4a6b36; font-weight: 600; }
        </style>
        </head>
        <body>
        <main>
          <h2>Connection problem</h2>
          <p>\(safe)</p>
          <button onclick="location.reload()">Retry</button>
        </main>
        </body>
        </html>
        """
    }

    private static let bufferSize = 16384
    private static let exchangeTimeout: Double = 15
    private static let hopByHop: Set<String> = [
        "connection", "keep-alive", "proxy-authenticate", "proxy-authorization", "te", "trailers", "upgrade",
    ]
    private static let upgradeHeaders: Set<String> = ["connection", "upgrade"]
    private static let framing: Set<String> = ["content-length", "transfer-encoding"]
}
