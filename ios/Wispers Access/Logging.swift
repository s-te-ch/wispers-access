import Foundation
import WispersAccessSdk
import os

extension Logger {
    static let app = Logger(subsystem: "dev.wispers.access.ios", category: "app")
    static let sdk = Logger(subsystem: "dev.wispers.access.ios", category: "sdk")
}

/// Routes the SDK's log lines into the unified log. Called from SDK threads.
nonisolated final class OSLogSink: LogSink, @unchecked Sendable {
    func log(level: LogLevel, target: String, message: String) {
        switch level {
        case .error: Logger.sdk.error("\(target, privacy: .public): \(message, privacy: .public)")
        case .warn: Logger.sdk.warning("\(target, privacy: .public): \(message, privacy: .public)")
        case .info: Logger.sdk.info("\(target, privacy: .public): \(message, privacy: .public)")
        case .debug, .trace: Logger.sdk.debug("\(target, privacy: .public): \(message, privacy: .public)")
        }
    }
}
