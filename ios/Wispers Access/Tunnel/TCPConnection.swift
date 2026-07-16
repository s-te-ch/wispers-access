import Foundation
import Network

/// Async wrapper over a browser's loopback `NWConnection`. Conforms to
/// `ByteSource`/`ByteSink` so the same buffered-read + exchange code drives both
/// the browser side and the QUIC side.
///
/// `@unchecked Sendable`: `NWConnection` serializes its own callbacks, and the
/// relay pumps read and write independent halves, so concurrent send/receive is
/// safe. The mutable read path is only ever driven by a single reader task.
nonisolated final class TCPConnection: ByteSource, ByteSink, @unchecked Sendable {
    private let connection: NWConnection

    init(_ connection: NWConnection) {
        self.connection = connection
    }

    /// Starts the connection and waits until it's ready to carry data.
    func start(on queue: DispatchQueue) async throws {
        try await withCheckedThrowingContinuation { (cont: CheckedContinuation<Void, Error>) in
            connection.stateUpdateHandler = { [weak connection] state in
                switch state {
                case .ready:
                    connection?.stateUpdateHandler = nil
                    cont.resume()
                case .failed(let error):
                    connection?.stateUpdateHandler = nil
                    cont.resume(throwing: error)
                case .cancelled:
                    connection?.stateUpdateHandler = nil
                    cont.resume(throwing: CancellationError())
                default:
                    break
                }
            }
            connection.start(queue: queue)
        }
    }

    func readChunk(maxLength: Int) async throws -> Data {
        try await withCheckedThrowingContinuation { (cont: CheckedContinuation<Data, Error>) in
            connection.receive(minimumIncompleteLength: 1, maximumLength: maxLength) { data, _, _, error in
                if let error {
                    cont.resume(throwing: error)
                } else {
                    // Empty (isComplete with no data) means the peer closed — our
                    // buffered reader treats an empty result as end-of-stream.
                    cont.resume(returning: data ?? Data())
                }
            }
        }
    }

    func write(_ data: Data) async throws {
        try await withCheckedThrowingContinuation { (cont: CheckedContinuation<Void, Error>) in
            connection.send(content: data, completion: .contentProcessed { error in
                if let error { cont.resume(throwing: error) } else { cont.resume() }
            })
        }
    }

    /// Sends a FIN on our write side (half-close), leaving the read side open.
    func finishSending() {
        connection.send(content: nil, contentContext: .finalMessage, isComplete: true, completion: .idempotent)
    }

    func cancel() {
        connection.cancel()
    }
}
