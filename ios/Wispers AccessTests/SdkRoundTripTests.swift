import Foundation
import Testing
import WispersAccessSdk

@testable import Wispers_Access

/// The SDK inside the app, in the simulator, against a real host node: joins
/// with the invite code in `WA_INVITE_CODE` (skipped without one), browses an
/// app through the per-app proxy with the auth cookie, refreshes, and leaves.
/// Run with `xcodebuild test … TEST_RUNNER_WA_INVITE_CODE=wax1_…`, or with the
/// code in `/private/tmp/wa-rt/code.txt`, as the round-trip harness leaves it.
struct SdkRoundTripTests {

    @Test func joinsBrowsesRefreshesAndLeaves() async throws {
        // From the environment, or from a file on the Mac the simulator shares
        // (`xcodebuild` does not always pass TEST_RUNNER_ variables through).
        let fromFile = try? String(contentsOfFile: "/private/tmp/wa-rt/code.txt", encoding: .utf8)
        guard let code = ProcessInfo.processInfo.environment["WA_INVITE_CODE"]
            ?? fromFile?.trimmingCharacters(in: .whitespacesAndNewlines)
        else {
            print("no invite code; skipping the round trip")
            return
        }
        let dir = FileManager.default.temporaryDirectory
            .appendingPathComponent("wa-sdk-roundtrip-\(UUID().uuidString)")
        defer { try? FileManager.default.removeItem(at: dir) }

        let changes = Changes()
        let client = try Client(config: ClientConfig(
            dataDir: dir.path, secrets: KeychainSecretStore(), observer: changes))
        let share = try await client.join(inviteCode: code)
        #expect(share.state == .live)
        #expect(share.apps.map(\.id) == ["echo"])
        #expect(try client.shares().map(\.id) == [share.id])
        #expect(changes.seen.contains { $0.id == share.id })

        // The proxy: refused without the cookie, served with it.
        let auth = ProxyAuth()
        let proxy = client.startPerAppProxy(requiredCookie: auth.requiredCookie)
        let base = try await proxy.baseUrl(share: share.id, appId: "echo")
        let url = URL(string: base + "/hello")!
        let (_, bare) = try await URLSession.shared.data(from: url)
        #expect((bare as? HTTPURLResponse)?.statusCode == 403)
        var request = URLRequest(url: url)
        request.setValue("\(ProxyAuth.cookieName)=\(auth.secret)", forHTTPHeaderField: "Cookie")
        let (body, response) = try await URLSession.shared.data(for: request)
        #expect((response as? HTTPURLResponse)?.statusCode == 200)
        #expect(String(data: body, encoding: .utf8)?.contains("path=/hello") == true)

        // Nothing changed on the host since the join.
        #expect(try await client.refresh(share: share.id) == nil)

        try await client.leave(share: share.id)
        #expect(try client.shares().isEmpty)
        // The Keychain holds nothing of the share anymore.
        #expect(try KeychainSecretStore().load(share: share.id, key: "iroh_secret") == nil)
    }

    nonisolated final class Changes: Observer, @unchecked Sendable {
        private let lock = NSLock()
        private var _seen: [Share] = []
        var seen: [Share] { lock.withLock { _seen } }
        func onShareChanged(share: Share) { lock.withLock { _seen.append(share) } }
    }
}
