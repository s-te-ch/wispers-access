import Foundation
import Testing

@testable import LLHTTP

struct HTTP1ParserTests {

    // MARK: - Helpers

    private func head(_ events: [HTTP1Event]) -> HTTP1Head? {
        for case .head(let h) in events { return h }
        return nil
    }

    private func body(_ events: [HTTP1Event]) -> Data {
        var data = Data()
        for case .body(let b) in events { data.append(b) }
        return data
    }

    private func isComplete(_ events: [HTTP1Event]) -> Bool {
        events.contains { if case .complete = $0 { return true }; return false }
    }

    // MARK: - Requests

    @Test func parsesSimpleRequest() throws {
        let parser = HTTP1Parser(kind: .request)
        let events = try parser.feed(Data("GET /foo?x=1 HTTP/1.1\r\nHost: example.com\r\nAccept: */*\r\n\r\n".utf8))
        let h = try #require(head(events))
        #expect(h.method == "GET")
        #expect(h.target == "/foo?x=1")
        #expect(h.headers.contains { $0.name == "Host" && $0.value == "example.com" })
        #expect(h.headers.contains { $0.name == "Accept" && $0.value == "*/*" })
        #expect(isComplete(events))
    }

    @Test func parsesRequestAcrossSplitFeeds() throws {
        let parser = HTTP1Parser(kind: .request)
        var events = try parser.feed(Data("POST /submit HTTP/1.1\r\nContent-Len".utf8))
        events += try parser.feed(Data("gth: 5\r\n\r\nhel".utf8))
        events += try parser.feed(Data("lo".utf8))
        let h = try #require(head(events))
        #expect(h.method == "POST")
        #expect(body(events) == Data("hello".utf8))
        #expect(isComplete(events))
    }

    // MARK: - Responses

    @Test func parsesContentLengthResponse() throws {
        let parser = HTTP1Parser(kind: .response)
        let events = try parser.feed(Data("HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nHello".utf8))
        let h = try #require(head(events))
        #expect(h.statusCode == 200)
        #expect(h.reason == "OK")
        #expect(body(events) == Data("Hello".utf8))
        #expect(isComplete(events))
    }

    @Test func decodesChunkedResponse() throws {
        let parser = HTTP1Parser(kind: .response)
        let wire = "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nHello\r\n6\r\n World\r\n0\r\n\r\n"
        let events = try parser.feed(Data(wire.utf8))
        #expect(head(events)?.statusCode == 200)
        #expect(body(events) == Data("Hello World".utf8))  // de-chunked
        #expect(isComplete(events))
    }

    @Test func closeDelimitedResponseCompletesOnFinish() throws {
        let parser = HTTP1Parser(kind: .response)
        var events = try parser.feed(Data("HTTP/1.1 200 OK\r\n\r\nstreamed body".utf8))
        #expect(!isComplete(events))          // no framing yet — body runs until close
        events += try parser.finish()          // EOF
        #expect(body(events) == Data("streamed body".utf8))
        #expect(isComplete(events))
    }

    @Test func headResponseHasNoBody() throws {
        let parser = HTTP1Parser(kind: .response)
        parser.expectNoBody()                  // response is to a HEAD request
        let events = try parser.feed(Data("HTTP/1.1 200 OK\r\nContent-Length: 100\r\n\r\n".utf8))
        #expect(head(events)?.statusCode == 200)
        #expect(body(events).isEmpty)          // the 100-byte body is not present, and not awaited
        #expect(isComplete(events))
    }

    @Test func detectsWebSocketUpgrade() throws {
        let parser = HTTP1Parser(kind: .request)
        let wire = "GET /ws HTTP/1.1\r\nHost: x\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n"
            + "Sec-WebSocket-Key: dGhlIHNhbXBsZQ==\r\n\r\nFRAME"
        let events = try parser.feed(Data(wire.utf8))
        let h = try #require(head(events))
        #expect(h.isUpgrade)
        #expect(parser.upgradeLeftover == Data("FRAME".utf8))  // bytes past the handshake
    }

    @Test func rejectsMalformedRequest() {
        let parser = HTTP1Parser(kind: .request)
        #expect(throws: HTTP1ParserError.self) {
            _ = try parser.feed(Data("!!! not http\r\n\r\n".utf8))
        }
    }
}
