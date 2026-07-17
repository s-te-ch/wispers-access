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
- **P3 — in progress.** Done: teardown/logout (best-effort `node.logout()` + confirm),
  hub-online status (`groupInfo()` poll → status dot), nickname from `groupInfo().name`,
  a **UI-parity pass** matching the Android design (wordmark from the real SVG, YOUR SHARES
  cards with avatar + status), and the **browse-UX redesign** (below, built + green
  2026-07-17): the roster is home + switcher, tap = open/resume (pushed browser), ⓘ =
  detail, back = switch, warm sessions with a ~5-min TTL — the iOS answer to Android's
  per-share task switcher (see [[ios-multi-session-switching]]). Remaining: add-flow
  redesign (Enter/Scan tabs + step progress) + QR scanner, quick actions (app-icon
  long-press), optional favicon harvesting for avatars. Foreground-only for v1 (iOS
  background limits are stricter than Android; matches current Android reality).

## Browse UX redesign — agreed 2026-07-16, ✅ built + green 2026-07-17

Decision: **the roster is the home *and* the switcher — one surface.** iOS-native, not
Android-parity (Android's tap→detail + OS task switcher don't apply; we can't use the OS
switcher, so we unify in-app). This replaced the earlier full-screen-cover browser +
separate "Open shares" tab overview, which felt like two lists + a modal "drawn over" mode.

**Model**
- Share list = `NavigationStack` **root = the landing, always** (no empty-browser state —
  the browser is never the base). Open the app → you're on your shares.
- Row = avatar · name · subtle status (online dot; a subtle "live" marker when a session
  is warm) · a trailing **ⓘ info button** (the iOS detail-disclosure accessory; the
  convention, more discoverable than swipe/long-press).
  - **Tap row → open/resume** → **push** a full-screen browser (resume the retained
    web view if the session is warm).
  - **ⓘ → detail** screen (status, joined / last-connected, backend, **Remove**).
- **Back → roster.** Backing out is also how you switch (the roster shows what's live; tap
  another). No tab overview, no second list.
- **Remove**: in detail (via ⓘ) with confirm; optionally also swipe-to-delete on the row.
- Sessions stay **warm with a TTL** (~5 min): a backgrounded session's proxy + `WKWebView`
  stay alive so re-open is instant; evict after the timeout to free resources. Satisfies
  "several shares open at once" (you view one at a time, like any full-screen app).

**Implementation (as built)**
- `RootView`: `NavigationStack` with a `navigationDestination(for: ShareRoute.self)` —
  `ShareRoute` enum = `.browse(ShareID)` / `.detail(ShareID)`. No more `fullScreenCover`.
- `BrowserView`: now takes a `shareID`, pushed via the `.browse` route. Renders only that
  share's retained `WKWebView`; reload lives in the top-bar trailing toolbar; back is the
  nav bar. On appear it calls `browser.open(share)` (ensure + mark active); on disappear
  `browser.resignActive` (start the TTL). `TabOverview` + the bottom bar are gone.
- `ShareListScreen`: each row is two side-by-side value `NavigationLink`s inside one card —
  the card → `.browse` (open/resume), a trailing 44pt **ⓘ** (`info.circle`) → `.detail`.
  A warm session shows a subtle presence badge on the avatar (`isLive` ← `browser.isWarm`).
- `ShareDetailScreen`: reached via ⓘ; "Open share ↗" is a `NavigationLink(value: .browse)`;
  Remove unchanged.
- `BrowseSessionStore`: dropped `isPresented`/`activeSession`/`switchTo`; added
  `session(for:)`, `isWarm(_:)`, `markActive`/`resignActive`, and per-share **TTL eviction**
  (`warmTTL = 300s`, cancelled on re-activate, skips if re-opened mid-timer). See
  [[ios-multi-session-switching]].

**Remaining P3 after the redesign**
- ✅ **Add-flow redesign** (built 2026-07-17) — `AddShareScreen` rewritten: Enter code / Scan
  QR segmented tabs, backend note (self-hosted invite announces itself), `PasteButton`,
  step-by-step progress (Validating / Generating identity / Registering / Activating) driven
  by a `JoinStep` callback that `ShareManager.join` now reports (single source of truth — no
  duplicated join logic like Android's view model), and a Joined summary (Open / Back to
  list). "Open" routes through the new `BrowseRouter` (env nav path; `openAfterDismiss`
  consumed in the roster's sheet `onDismiss` so we don't push while the sheet is up).
- ✅ **QR scanner** (built 2026-07-17) — `QRScannerView` wraps VisionKit
  `DataScannerViewController` (`import Vision` for `.qr`), gated on `canScan`
  (`isSupported && isAvailable`) with a `ContentUnavailableView` fallback on the Simulator.
  Camera string added as `INFOPLIST_KEY_NSCameraUsageDescription` build setting (Debug +
  Release), since the target uses `GENERATE_INFOPLIST_FILE=YES` with no `INFOPLIST_FILE`.
  Only exercises on a real device.
- ✅ **Quick actions** (built + verified working 2026-07-17) — app-icon long-press → recent
  shares. `QuickActions.swift`: builds up-to-4 dynamic `UIApplicationShortcutItem`s from the
  roster (most-recently-connected first), refreshed on scene `.background`. A minimal
  `@UIApplicationDelegateAdaptor(AppDelegate.self)` captures the chosen shortcut into a
  `QuickActionInbox` singleton (also in the environment):
  - **cold** via `application(_:configurationForConnecting:options:)` reading
    `options.shortcutItem`;
  - **warm** via a **custom `SceneDelegate`** and its `windowScene(_:performActionFor:)` —
    because SwiftUI's own scene delegate **swallows** `application(_:performActionFor:)`
    (confirmed: it never fired). `configurationForConnecting` sets `config.delegateClass =
    SceneDelegate.self`; the SceneDelegate deliberately **omits `scene(_:willConnectTo:)`** so
    SwiftUI keeps window setup (verified the roster still renders).
  `RootView` drains the inbox (cold: `onAppear`, warm: `onChange`) and `router.open`s the
  share if it still exists. Verified on the Simulator end-to-end (warm path logs
  performActionFor → handle → route → push).
- **Favicon harvesting** for avatars (optional) — WKWebView JS on page-finish → per-share
  icon store; today avatars are always the green letter tile.

> **Infra note (found 2026-07-17):** the target has **no `INFOPLIST_FILE`**, so `ios/Info.plist`
> (the `NSAllowsLocalNetworking` ATS file) is **inert** — the built `Info.plist` has no ATS
> entry, yet loopback browsing works anyway on iOS 26. Add per-key Info.plist values via
> `INFOPLIST_KEY_*` build settings (as done for the camera). If a future iOS enforces ATS on
> loopback, wire `INFOPLIST_FILE = Info.plist` or add `INFOPLIST_KEY_NSAppTransportSecurity`.

**Status (all uncommitted since the last commit, which was the llhttp pivot + .gitignore):**
nickname-from-`groupInfo().name`, teardown/logout (best-effort `node.logout()` + confirm),
the UI-parity pass (theme, avatar, status poll + dot, `ShareDetailScreen`, list redesign,
real logo SVG), the browse-UX redesign (roster = home + switcher, push browser, ⓘ detail,
TTL eviction), and a **NodeStorage-lifetime crash fix** (removing a share hit `EXC_BAD_ACCESS`
in the binding's `deleteRootKey` trampoline — the `Node` doesn't retain its `NodeStorage`,
which frees the callbacks holder on deinit). Final shape: `logoutAndDiscard` discards any
cached node and restores a fresh live-storage node just for `logout()`; `join` keeps its
storage alive through register/activate; `resolveNode` does NOT retain storage (a restored
node runs QUIC/groupInfo fine without it — retaining it wedged the status poll at "CHECKING").
See [[ios-nodestorage-lifetime]]. Also fixed a **status-stuck-at-CHECKING** bug: `join`
adds the share to the roster *before* register/activate, which restarts the roster's status
poll and restores the share mid-join → a **Pending** node that `SessionManager.resolveNode`
then **cached for the whole session** (a restart "fixed" it only by clearing the cache).
Now `resolveNode` caches only `.activated` nodes, and `join` primes the status from a fresh
node once activation has persisted. Builds + tests green on the iPhone 17 simulator.

## Build / verify

This is a Mac — confirm `xcodebuild` + an available Simulator, then build with e.g.
`xcodebuild -scheme "Wispers Access" -destination 'platform=iOS Simulator,name=iPhone 16'`.
SPM fetches the xcframework from GitHub releases, so the build needs network + repo access.

> Note: the nested `ios/Wispers Access/Wispers Access.xcodeproj` looks like a stray
> duplicate of the outer project — check/clean it up when setting the project up.
