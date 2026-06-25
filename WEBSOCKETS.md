# WebSocket support — plan & checklist

Branch: `websockets`

## Goal
Tunnel HTTP/1.1 `Upgrade` (WebSocket) through all three proxies. Stay
**protocol-agnostic**: detect the upgrade, forward the handshake intact, then
splice raw bytes both ways until either end closes. No WebSocket library needed.

## Key fact
`QuicStream` is a full-duplex bidirectional byte pipe (`AsyncRead + AsyncWrite`
in Rust `wispers-connect/src/p2p.rs:393,403`; `read`/`write`/`finish` in the
Kotlin handle). A post-handshake WebSocket is exactly that — 1 socket : 1 QUIC
stream for its lifetime.

## Why it fails today
1. `Connection` + `Upgrade` are stripped as hop-by-hop in all three proxies
   (`waserver/src/http.rs:118-130`, `waclient/src/main.rs:298-310`,
   `android/.../proxy/UpstreamClient.kt:248-256`). `Sec-WebSocket-*` already
   passes through; only the trigger headers die.
2. Single request→response→close model: waclient forces `Connection: close`
   (`main.rs:241-244`), Android same + FIN after response
   (`UpstreamClient.kt:73,52`), response reader parses a framed body
   (`UpstreamClient.kt:133-173`), Android is half-duplex (`:35-53`).
3. No `serve_connection` opts into hyper upgrades.

## Checklist

### waserver (`src/http.rs`) — hyper server on QUIC side, hyper client to local app ✅ DONE
- [x] `.with_upgrades()` on server conn
- [x] `conn.with_upgrades()` on upstream client conn
- [x] Capture `hyper::upgrade::on(&mut req)` (peer side) before forwarding
- [x] On `101`: capture `hyper::upgrade::on(&mut resp)`, spawn `splice_upgrade`
      → `try_join!` both `OnUpgrade` → `copy_bidirectional` over `TokioIo` halves
- [x] Return the `101` with handshake headers intact (empty body)

### waclient (`src/main.rs`) — hyper server to browser, hyper client on QUIC stream ✅ DONE
- [x] `.with_upgrades()` on browser-facing server conn
- [x] `conn.with_upgrades()` on QUIC-stream client conn
- [x] Skip `Connection: close` injection for upgrades (exempt; FINs at socket close)
- [x] Capture browser-side + QUIC-side `OnUpgrade`, `splice_upgrade` on `101`
- [x] Stream FINs via `QuicStream::poll_shutdown` when `copy_bidirectional` ends

### Android — Ktor inbound + hand-rolled HTTP over QUIC stream ✅ DONE
All in `UpstreamClient.kt` (no `ProxyServer.kt` change needed — the branch lives in `forward`).
- [x] Inbound: respond with `OutgoingContent.ProtocolUpgrade`; upstream 101 headers
      (incl. `Sec-WebSocket-Accept`) replayed via the content's `headers` property
      (Ktor routes `Upgrade` through its `safeOnly=false` path; CIO adds none itself)
- [x] **BUGFIX (device-found):** canonicalise the `Upgrade`/`Connection` header NAMES when
      building the ProtocolUpgrade headers. Ktor's `commitHeaders` only allows the
      engine-reserved `Upgrade` through via a CASE-SENSITIVE match on `HttpHeaders.Upgrade`
      (`"Upgrade"`); hyper sends it lowercased (`upgrade`), so it hit the safe-append path
      and threw `UnsafeHeaderException("…upgrade… controlled by the engine")` → the catch
      invalidated the QUIC connection → WebView stuck "connecting". Desktop unaffected
      (hyper writes the 101 itself, no Ktor check).
- [x] `isUpgradeRequest` matches Ktor CIO's `expectHttpUpgrade` (GET + Upgrade + Connection:upgrade)
      so `respond(ProtocolUpgrade)` is never rejected
- [x] On `101`: two pumps splice browser↔stream in a `coroutineScope`; FIN via
      `stream.finish()` on browser EOF, `output.flushAndClose()` on stream EOF
- [x] Buffered prefix handled — `readSome()` already returns buffered bytes before reading fresh
- [x] Skip `Connection: close` and post-head `finish()` for upgrades
- [x] `EXCHANGE_TIMEOUT_MS` wraps only handshake; relay (respond) is outside it
- [x] Relay errors caught inside the root coroutine (Ktor only join()s it → would be uncaught)
- [x] Concurrent read+write on one `QuicStream` — source-verified safe (see Concurrency note below),
      now also exercised live on-device (relay flowing) → JNA concurrent r/w confirmed working.

## Concurrency note (the two-pump / copy_bidirectional safety)
Verified against `wispers-client/wispers-connect/src/quic.rs`. `read`/`write`/`finish`
take `&self` and lock the connection mutex ONLY for the brief quiche call — never
across the await that waits for the other direction. `read` arms a notification,
locks, tries `stream_recv`, and on `Done` releases the lock BEFORE `notified.await`,
so a parked reader holds nothing and a concurrent `write` proceeds freely. Directions
are independent (`recv_fin`/`sent_fin` atomics, separate notify). Poll path documents
"conn lock is taken with try_lock (never held across an .await)". → No deadlock; covers
both the Android two pumps and the Rust `copy_bidirectional`. Note: the library's
hub-free `loopback_pair()` tests exist but `test_loopback_poll_io` is sequential, so a
true full-duplex-on-one-stream test is a (deferred) gap one could fill in wispers-client.

### Cross-cutting
- [x] Upgrade-aware header logic in waserver + waclient (preserve `Connection`+`Upgrade`
      on upgrades, keep stripping `proxy-*`/`te`/`trailers`/`keep-alive`)
- [x] Same header logic for Android (`UPGRADE_HEADERS` kept in both directions)
- [ ] Test idle socket vs ICE consent ~30s stall (access-stall-consent-freshness memory)
- [x] Test: echo-WS upstream + browser `new WebSocket(...)`, desktop ✅ + Android ✅ (Pixel 8, device-verified)

## Status
**Rust pair: implemented + verified working end-to-end (committed).**
**Android: implemented + verified working on-device (Pixel 8).** WebView shows
"connected"; logs show `GET /ws → 101` with no follow-on error; relay flowing.
Required one device-found bugfix (Upgrade header-name casing — see BUGFIX above).
`:app:compileDebugKotlin` + `:app:lintDebug` clean. **Android changes not yet committed.**

Test upstream: `demo/ws-echo.py` (serves page + WS on one port; works for Android too).
Repro tip: `adb forward tcp:10774 tcp:10774` + replay the handshake from the Mac, and
`adb logcat -d ProxyServer:V UpstreamClient:V '*:S'` to see per-request status + errors.
waserver (daemon) logs: `~/Library/Logs/waserver/<share>/waserver.log.<date>`.
