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

### Android — Ktor inbound + hand-rolled HTTP over QUIC stream
- [ ] Inbound: respond with `OutgoingContent.ProtocolUpgrade`; copy upstream
      `101` headers (incl. `Sec-WebSocket-Accept`) into the upgrade response.
      Branch in `ProxyServer.handleProxy` (`ProxyServer.kt:49`) on `Upgrade: websocket`
- [ ] `UpstreamClient`: on `101`, two coroutines splice browser↔stream, then `finish()`/`close()`
- [ ] **Flush `StreamReader`'s buffered prefix** to browser before splice loop
      (8 KB reads already buffer post-header bytes, `UpstreamClient.kt:307-309`)
- [ ] Skip `Connection: close` (`:73`) and post-head `finish()` (`:52`) for upgrades
- [ ] Timeout (`EXCHANGE_TIMEOUT_MS`, `:42`) covers handshake only, not splice
- [ ] Verify concurrent read+write on one `QuicStream` across the JNA bridge

### Cross-cutting
- [x] Upgrade-aware header logic in waserver + waclient (preserve `Connection`+`Upgrade`
      on upgrades, keep stripping `proxy-*`/`te`/`trailers`/`keep-alive`)
- [ ] Same header logic for Android (with Android impl)
- [ ] Test idle socket vs ICE consent ~30s stall (access-stall-consent-freshness memory)
- [ ] Test: echo-WS upstream + browser `new WebSocket(...)`, desktop + Android separately

## Status
**Rust pair (waserver + waclient): implemented.** Builds clean, clippy clean,
existing tests pass. Source-verified against hyper 1.9 upgrade internals
(`upgrade::on` claims the receiver; conn keeps its own `Pending` sender, so the
upgrade still fires; `Upgraded` prepends buffered bytes via `Rewind`).
NOT yet runtime-verified end-to-end — needs the live QUIC path (provisioned
share + hub) or the Android client, neither available in this env yet.

Next: Android (Ktor `OutgoingContent.ProtocolUpgrade` + raw splice in `UpstreamClient`).
