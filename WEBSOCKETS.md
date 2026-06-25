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

### waserver (`src/http.rs`) — hyper server on QUIC side, hyper client to local app
- [ ] `.with_upgrades()` on server conn (`http.rs:27`)
- [ ] Capture `hyper::upgrade::on(&mut req)` (peer side) before forwarding
- [ ] On `101`: capture `hyper::upgrade::on(&mut resp)` (upstream side), spawn
      task → `tokio::io::copy_bidirectional` between the two `TokioIo`-wrapped halves
- [ ] Return the `101` with upgrade headers intact

### waclient (`src/main.rs`) — hyper server to browser, hyper client on QUIC stream
- [ ] `.with_upgrades()` on server conn (`main.rs:185`)
- [ ] Skip `Connection: close` injection for upgrades (`main.rs:241-244`)
- [ ] Capture browser-side + QUIC-side `OnUpgrade`, `copy_bidirectional` on `101`
- [ ] Ensure stream is finished/dropped on teardown (credit-leak memory)

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
- [ ] Upgrade-aware header logic (preserve `Connection: upgrade` + `Upgrade`,
      keep stripping `proxy-*`/`te`/`trailers`/`keep-alive`) — touches all 3 strip fns
- [ ] Test idle socket vs ICE consent ~30s stall (access-stall-consent-freshness memory)
- [ ] Test: echo-WS upstream + browser `new WebSocket(...)`, desktop + Android separately

## Status
Not started — plan only.
