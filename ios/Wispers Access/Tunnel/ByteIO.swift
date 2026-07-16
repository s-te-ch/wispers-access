import Foundation
import WispersConnect

/// A source of bytes read incrementally. An empty `Data` result signals
/// end-of-stream (a QUIC FIN or a closed socket).
protocol ByteSource: Sendable {
    func readChunk(maxLength: Int) async throws -> Data
}

/// A sink that accepts bytes to write.
protocol ByteSink: Sendable {
    func write(_ data: Data) async throws
}

// `QuicStream.read(maxLen:)`/`write(_:)` already match; adapt the read name.
extension QuicStream: ByteSource {
    func readChunk(maxLength: Int) async throws -> Data {
        try await read(maxLen: maxLength)
    }
}

extension QuicStream: ByteSink {}

/// Errors raised inside the browsing tunnel.
enum TunnelError: LocalizedError {
    case unexpectedEOF
    case malformedRequestLine(String)
    case malformedStatusLine(String)
    case malformedChunk
    case timedOut
    case missingPort

    var errorDescription: String? {
        switch self {
        case .unexpectedEOF: return "The connection closed unexpectedly."
        case .malformedRequestLine(let line): return "Malformed request line: \(line)"
        case .malformedStatusLine(let line): return "Malformed status line: \(line)"
        case .malformedChunk: return "Malformed chunked body."
        case .timedOut: return "The connection timed out."
        case .missingPort: return "The local proxy did not get a port."
        }
    }
}

/// Runs `operation`, throwing `TunnelError.timedOut` if it doesn't finish within
/// `seconds`. The losing child task is cancelled. Note this only interrupts
/// cancellation-aware work. A truly wedged native call still unblocks the caller.
nonisolated func withDeadline<T: Sendable>(
    seconds: Double,
    _ operation: @escaping @Sendable () async throws -> T
) async throws -> T {
    try await withThrowingTaskGroup(of: T.self) { group in
        group.addTask { try await operation() }
        group.addTask {
            try await Task.sleep(nanoseconds: UInt64(seconds * 1_000_000_000))
            throw TunnelError.timedOut
        }
        defer { group.cancelAll() }
        // The first task to finish (the operation, or the timeout) wins.
        guard let result = try await group.next() else { throw TunnelError.timedOut }
        return result
    }
}
