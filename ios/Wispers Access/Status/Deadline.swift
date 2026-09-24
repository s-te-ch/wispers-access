import Foundation

struct DeadlineExceeded: Error {}

/// Runs `body` with a deadline; whichever finishes first wins, the other is
/// cancelled. For calls into a network that may hang instead of failing.
func withDeadline<T: Sendable>(
    seconds: Double,
    _ body: @escaping @Sendable () async throws -> T
) async throws -> T {
    try await withThrowingTaskGroup(of: T.self) { group in
        group.addTask { try await body() }
        group.addTask {
            try await Task.sleep(for: .seconds(seconds))
            throw DeadlineExceeded()
        }
        let first = try await group.next()!
        group.cancelAll()
        return first
    }
}
