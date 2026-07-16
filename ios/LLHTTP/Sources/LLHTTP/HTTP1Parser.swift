import CLLHTTP
import Foundation

/// A parsed HTTP/1.x message head (request or response).
public struct HTTP1Head: Sendable {
    /// Request method (e.g. `GET`); nil for responses.
    public var method: String?
    /// Request target / request-URI (e.g. `/path?q`); nil for responses.
    public var target: String?
    /// Response status code; nil for requests.
    public var statusCode: Int?
    /// Response reason phrase; nil for requests.
    public var reason: String?
    public var majorVersion: Int
    public var minorVersion: Int
    public var headers: [(name: String, value: String)]
    /// The message requested a protocol upgrade (e.g. WebSocket).
    public var isUpgrade: Bool
    public var keepAlive: Bool
}

/// A streamed parse event. Bodies arrive already de-chunked / de-framed.
public enum HTTP1Event: Sendable {
    case head(HTTP1Head)
    case body(Data)
    case complete
}

public struct HTTP1ParserError: Error, CustomStringConvertible {
    public let name: String
    public let reason: String
    public var description: String { "\(name): \(reason)" }
}

/// Streaming HTTP/1 parser built on llhttp. Feed bytes as they arrive; get back
/// `head`/`body`/`complete` events. No event loop, no threading affinity — safe
/// to drive from `async` code across `await` boundaries.
public final class HTTP1Parser {
    public enum Kind { case request, response }

    private var parser = llhttp_t()
    private var settings = llhttp_settings_t()
    private let kind: Kind

    // Per-`feed` output and cross-call header assembly.
    private var events: [HTTP1Event] = []
    private var headers: [(name: String, value: String)] = []
    private var fieldBuf: [UInt8] = []
    private var valueBuf: [UInt8] = []
    private var readingValue = false
    private var methodBuf: [UInt8] = []
    private var targetBuf: [UInt8] = []
    private var reasonBuf: [UInt8] = []
    private var noBodyExpected = false

    /// Bytes left unparsed after a paused upgrade (the start of the upgraded
    /// protocol). Populated when a `head` with `isUpgrade == true` is produced.
    public private(set) var upgradeLeftover = Data()

    public init(kind: Kind) {
        self.kind = kind
        llhttp_settings_init(&settings)
        settings.on_message_begin = { p in HTTP1Parser.instance(p).onMessageBegin(); return 0 }
        settings.on_method = { p, at, len in HTTP1Parser.instance(p).appendMethod(at, len); return 0 }
        settings.on_url = { p, at, len in HTTP1Parser.instance(p).appendTarget(at, len); return 0 }
        settings.on_status = { p, at, len in HTTP1Parser.instance(p).appendReason(at, len); return 0 }
        settings.on_header_field = { p, at, len in HTTP1Parser.instance(p).appendField(at, len); return 0 }
        settings.on_header_value = { p, at, len in HTTP1Parser.instance(p).appendValue(at, len); return 0 }
        settings.on_headers_complete = { p in HTTP1Parser.instance(p).headersComplete(p) }
        settings.on_body = { p, at, len in HTTP1Parser.instance(p).appendBody(at, len); return 0 }
        settings.on_message_complete = { p in HTTP1Parser.instance(p).messageComplete(); return 0 }
        llhttp_init(&parser, kind == .request ? HTTP_REQUEST : HTTP_RESPONSE, &settings)
        parser.data = Unmanaged.passUnretained(self).toOpaque()
    }

    /// Tell the parser the next response has no body regardless of its headers —
    /// set before feeding a response to a `HEAD` request (llhttp can't know the
    /// request method on its own).
    public func expectNoBody() { noBodyExpected = true }

    /// Feeds `data`, returning any events it produced.
    public func feed(_ data: Data) throws -> [HTTP1Event] {
        events.removeAll(keepingCapacity: true)
        guard !data.isEmpty else { return events }
        let err = data.withUnsafeBytes { raw -> llhttp_errno_t in
            let base = raw.baseAddress!.assumingMemoryBound(to: CChar.self)
            let status = llhttp_execute(&parser, base, data.count)
            if status == HPE_PAUSED_UPGRADE, let pos = llhttp_get_error_pos(&parser) {
                let offset = base.distance(to: pos)
                if offset >= 0 && offset < data.count {
                    upgradeLeftover = data.subdata(in: offset..<data.count)
                }
            }
            return status
        }
        try check(err)
        return events
    }

    /// Signals EOF (a closed connection). Completes a close-delimited body.
    public func finish() throws -> [HTTP1Event] {
        events.removeAll(keepingCapacity: true)
        try check(llhttp_finish(&parser))
        return events
    }

    // MARK: - Callback handling

    private static func instance(_ p: UnsafeMutablePointer<llhttp_t>?) -> HTTP1Parser {
        Unmanaged<HTTP1Parser>.fromOpaque(p!.pointee.data).takeUnretainedValue()
    }

    private func onMessageBegin() {
        headers.removeAll(keepingCapacity: true)
        fieldBuf.removeAll(keepingCapacity: true)
        valueBuf.removeAll(keepingCapacity: true)
        methodBuf.removeAll(keepingCapacity: true)
        targetBuf.removeAll(keepingCapacity: true)
        reasonBuf.removeAll(keepingCapacity: true)
        readingValue = false
    }

    private func appendMethod(_ at: UnsafePointer<CChar>?, _ len: Int) { append(&methodBuf, at, len) }
    private func appendTarget(_ at: UnsafePointer<CChar>?, _ len: Int) { append(&targetBuf, at, len) }
    private func appendReason(_ at: UnsafePointer<CChar>?, _ len: Int) { append(&reasonBuf, at, len) }

    private func appendField(_ at: UnsafePointer<CChar>?, _ len: Int) {
        if readingValue {
            flushHeader()
            readingValue = false
        }
        append(&fieldBuf, at, len)
    }

    private func appendValue(_ at: UnsafePointer<CChar>?, _ len: Int) {
        readingValue = true
        append(&valueBuf, at, len)
    }

    private func appendBody(_ at: UnsafePointer<CChar>?, _ len: Int) {
        guard let at, len > 0 else { return }
        events.append(.body(Data(bytes: at, count: len)))
    }

    // Accessors take the callback's raw `p` (not `&self.parser`): `llhttp_execute`
    // already holds an exclusive `&parser` access while this fires, so a second
    // one would be an exclusivity violation.
    private func headersComplete(_ p: UnsafeMutablePointer<llhttp_t>?) -> Int32 {
        flushHeader()
        var head = HTTP1Head(
            method: nil, target: nil, statusCode: nil, reason: nil,
            majorVersion: Int(llhttp_get_http_major(p)),
            minorVersion: Int(llhttp_get_http_minor(p)),
            headers: headers,
            isUpgrade: llhttp_get_upgrade(p) == 1,
            keepAlive: llhttp_should_keep_alive(p) != 0
        )
        if kind == .request {
            head.method = latin1(methodBuf)
            head.target = latin1(targetBuf)
        } else {
            head.statusCode = Int(llhttp_get_status_code(p))
            head.reason = latin1(reasonBuf)
        }
        events.append(.head(head))
        // Return 1 to tell llhttp this message has no body (HEAD responses).
        return noBodyExpected ? 1 : 0
    }

    private func messageComplete() {
        events.append(.complete)
    }

    private func flushHeader() {
        guard !fieldBuf.isEmpty else { return }
        headers.append((latin1(fieldBuf), latin1(valueBuf)))
        fieldBuf.removeAll(keepingCapacity: true)
        valueBuf.removeAll(keepingCapacity: true)
    }

    private func append(_ buffer: inout [UInt8], _ at: UnsafePointer<CChar>?, _ len: Int) {
        guard let at, len > 0 else { return }
        at.withMemoryRebound(to: UInt8.self, capacity: len) {
            buffer.append(contentsOf: UnsafeBufferPointer(start: $0, count: len))
        }
    }

    private func latin1(_ bytes: [UInt8]) -> String {
        String(bytes: bytes, encoding: .isoLatin1) ?? ""
    }

    private func check(_ err: llhttp_errno_t) throws {
        guard err != HPE_OK && err != HPE_PAUSED_UPGRADE else { return }
        let name = llhttp_errno_name(err).map { String(cString: $0) } ?? "HPE_UNKNOWN"
        let reason = llhttp_get_error_reason(&parser).map { String(cString: $0) } ?? ""
        throw HTTP1ParserError(name: name, reason: reason)
    }
}
