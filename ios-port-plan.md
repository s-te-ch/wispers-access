# iOS app — port plan (kickoff note)

The iOS app (`ios/`) is currently the stock Xcode template (`ContentView.swift` =
"Hello, world!"). The plan: build it out as a **feature-parity port of the Android
app**, idiomatic SwiftUI, with the **self-hosted-backend** support baked in from the
first join flow (the one remaining 🟡 in `self-hosting-plan.md` §6).

## The binding (the critical dependency — it exists)

iOS has the direct analog of Android's `dev.wispers:connect` AAR: a **Swift Package
`WispersConnect`** in the `s-te-ch/wispers-client` repo, wrapping a
`CWispersConnect.xcframework` (the crate's C-ABI FFI compiled for Apple platforms),
distributed via GitHub releases and consumed through SPM.

- Wrapper source: `~/git/wispers-client/wrappers/swift/Sources/WispersConnect/`
- Released xcframeworks: 0.8.0 … **0.11.0** (latest). **Pinned at `upToNextMinor` 0.11.0**
  (matches the reference "Wispers Files" app's usage and is the API actually verified;
  Android is on 0.9.0, waserver/waclient on 0.9.2 — the hub tolerates skew via its legacy
  `connect.hub.Hub` alias, so exact fleet match isn't required).
- API parity with the Android binding — same node lifecycle:
  - `NodeStorage.withCallbacks(_:)`, `NodeStorage.overrideHubAddr(_:)`,
    `NodeStorage.restoreOrInit() async -> (Node, NodeState)`
  - `Node.register(token:)`, `activate(activationCode:)`, `groupInfo()`,
    `startServing()`, `connectQuic(peerNodeNumber:)`
- **Same ordering rule as everywhere else:** `overrideHubAddr` must be called on the
  `NodeStorage` **before** `restoreOrInit()`.

## Reference apps to crib from

1. **Minimal** — `~/git/wispers/exp/wrapperuse/swift-test/Sources/main.swift`. Smallest
   end-to-end use of the binding.
2. **Rich** — `~/git/wispers/files/apps/ios` ("Wispers Files"), a full SwiftUI app on the
   *same* binding. Most valuable files:
   - `KeychainStorage.swift` — implements `NodeStorageCallbacksProtocol`
     (`loadRootKey`/`saveRootKey`/`delete…` + registration), Keychain-backed. **Our
     persistence pattern** — adapt from single-identity to **per-share** keying.
   - `NodeHolder.swift` — node lifecycle & caching: `NodeStorage.withCallbacks(…)` →
     `restoreOrInit()` → `register` → `activate`. Where `overrideHubAddr` slots in.
   - `QuicProtocol.swift` / `RequestHandler.swift` — HTTP-over-QUIC-stream mechanics.
   - **Caveat:** Files renders **natively** (`FsTree`/`FolderScreen`), so it does *not*
     show the WKWebView-over-proxy path Access needs. Use it for binding / QUIC-HTTP /
     storage / registration / SwiftUI patterns; the WebView glue is ours to add.

## Android → iOS map

| Android | iOS |
|---|---|
| `InviteCode.kt` + `Base32.kt` | `InviteCode.swift` + `Base32.swift` — near-direct port (parse 4th base32 backend field, https-only, fail-closed) |
| `ShareEntity`/`ShareDao`/`ShareDatabase` (Room+SQLCipher) | per-share store: Keychain (root key + registration, per `KeychainStorage`) + small SQLite/GRDB or file store for share metadata (**`backend`**, nickname, timestamps, icon) |
| `ShareRepository.storageFor(id)` (override-before-restore chokepoint) | same: `NodeStorage.withCallbacks(perShareCallbacks)`, `overrideHubAddr(backend)` **before** `restoreOrInit()` |
| `SessionManager` / `ProxyServer` / `UpstreamClient` | `NodeHolder`-style node cache + `QuicProtocol`/`RequestHandler` for HTTP-over-QUIC + **WKWebView glue** (WKURLSchemeHandler or a local server) |
| `AddShareScreen` / `ShareListScreen` / `ShareDetailScreen` (Compose) | SwiftUI screens; QR scan via AVFoundation/VisionKit |
| Hilt DI | plain init / `@Environment` |

## Self-hosted backend (the tracked feature — free in Phase 1)

Mirror the waclient/Android semantics: parse the optional `_<base32(url)>` 4th field of
the `wax_` code (fail-closed, https-only, surfaces a real reason), `overrideHubAddr`
before restore, persist per share, reapply on reconnect. `Base32` is a straight port of
the Android codec (RFC 4648 no-pad, matches Rust `data_encoding::BASE32_NOPAD`).

## Phasing

- **P1 — ✅ done.** SPM binding wired (`WispersConnect` 0.11.0, linked via the app target's
  Frameworks phase); `InviteCode` + `Base32` ported with unit tests; per-share Keychain
  secrets (`KeychainShareStore`) + Codable JSON metadata (`ShareStore`, incl. `backend`);
  `ShareManager.join` (parse invite incl. backend → `overrideHubAddr` **before**
  `restoreOrInit` → register → activate, with rollback on failure); SwiftUI share-list +
  add-share (Observation `@Observable` + `@Environment`, iOS-native). Builds + tests green
  on the iPhone 17 simulator. The stray nested `.xcodeproj` was removed.
- **P2 — ✅ built (pending live end-to-end).** The browsing tunnel + WKWebView: a
  `SessionManager` actor caches the per-share `Node` + `QuicConnection` (`connectQuic(1)`,
  one-retry + evict); a `LoopbackProxy` (`NWListener` on **127.0.0.1**, ephemeral port —
  no `*.localhost` trick) speaks raw HTTP/1.1 over a fresh QUIC stream per request
  (`Connection: close` upstream, `finish()` after the response, Set-Cookie `Domain=`
  stripped), **including the WebSocket/`101` raw bidirectional relay**; `BrowseScreen`
  points a WKWebView at the proxy. Faithful port of the Android
  `SessionManager`/`ProxyServer`/`UpstreamClient`. ATS `NSAllowsLocalNetworking` added for
  the loopback load. **HTTP/1.1 parsing (headers, chunked, content-length, EOF-framing,
  HEAD) is done by vendored llhttp** (`ios/LLHTTP` local SPM package: C target `CLLHTTP` +
  Swift `HTTP1Parser`), not hand-rolled — llhttp de-frames the body and we serialize +
  transport. (SwiftNIO was evaluated first and rejected: its HTTP codecs only run inside a
  NIO `Channel`, and the only off-socket one, `EmbeddedChannel`, is thread-pinned →
  `preconditionFailure` if driven across `await`. llhttp — the same parser NIO vendors — is
  a loop-free C callback parser that streams cleanly, at ~1/80th the dependency weight.)
  Builds + tests green (parser unit tests + a real-`NWListener` proxy integration test), and
  **live browse against a running `waserver serve` verified working** (2026-07-16).
- **P3** — polish (icons, hub-online status, pull-to-refresh, teardown/logout UI,
  QR-scan join). Foreground-only for v1 (iOS background limits are stricter than Android;
  matches current Android reality).

## Build / verify

This is a Mac — confirm `xcodebuild` + an available Simulator, then build with e.g.
`xcodebuild -scheme "Wispers Access" -destination 'platform=iOS Simulator,name=iPhone 16'`.
SPM fetches the xcframework from GitHub releases, so the build needs network + repo access.

> Note: the nested `ios/Wispers Access/Wispers Access.xcodeproj` looks like a stray
> duplicate of the outer project — check/clean it up when setting the project up.
