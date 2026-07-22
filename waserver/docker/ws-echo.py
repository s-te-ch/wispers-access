#!/usr/bin/env python3
"""Throwaway WebSocket test upstream for Wispers Access.

Serves a tiny HTML page AND a WebSocket endpoint on the SAME port, so the whole
thing can be proxied same-origin:

    python3 waserver/docker/ws-echo.py 8080   # then point waserver at port 8080

The page (GET /) opens a WebSocket to ws://<this-host>/ws, echoes whatever you
type, and also receives an unsolicited server push every 2s. The push exercises
the server->client direction of the relay independently of client sends, so a
working page proves the bidirectional splice in both directions.

Pure stdlib: no pip install. Handles text + ping + close frames; that's all a
browser needs here.
"""

import base64
import hashlib
import os
import select
import socket
import sys
import threading
import time

WS_GUID = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11"

# Set by the --quiet flag: suppress the periodic server push so the socket sits
# truly idle (no traffic either way) — for the consent/keepalive idle test.
QUIET = False

PAGE = ("""<!doctype html>
<html>
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Wispers WS test</title>
<style>
  body { font-family: system-ui, sans-serif; margin: 0; padding: 2rem; max-width: 40rem; }
  #status { font-weight: 600; }
  .ok { color: #2e7d32; } .bad { color: #c62828; }
  .log { border: 1px solid #ddd; border-radius: .5rem; padding: .5rem .75rem; height: 9rem;
         overflow-y: auto; font-family: ui-monospace, monospace; font-size: .8rem; margin: .25rem 0 1rem; }
  input { font-size: 1rem; padding: .5rem; width: 70%; }
  button { font-size: 1rem; padding: .5rem 1rem; }
  h3 { margin: 1rem 0 .25rem; }
</style>
</head>
<body>
  <h2>Wispers Access — WebSocket test</h2>
  <p>Status: <span id="status" class="bad">connecting…</span>
     &nbsp;·&nbsp; Idle: <span id="idle">0</span>s since last frame</p>

  <h3>Echo (client → server → client)</h3>
  <div id="echo" class="log"></div>
  <input id="msg" placeholder="type a message" autofocus>
  <button id="send">Send</button>

  <h3 id="pushhead">Server push (server → client, every 2s)</h3>
  <div id="push" class="log"></div>

<script>
  const scheme = location.protocol === "https:" ? "wss" : "ws";
  const url = scheme + "://" + location.host + "/ws";
  const QUIET = __QUIET__;
  const statusEl = document.getElementById("status");
  const echoEl = document.getElementById("echo");
  const pushEl = document.getElementById("push");
  const idleEl = document.getElementById("idle");
  const pushHeadEl = document.getElementById("pushhead");
  function line(el, s) { const d = document.createElement("div"); d.textContent = s; el.appendChild(d); el.scrollTop = el.scrollHeight; }

  let lastActivity = performance.now();
  let pendingSend = null;
  function markActivity() { lastActivity = performance.now(); }
  setInterval(() => { idleEl.textContent = ((performance.now() - lastActivity) / 1000).toFixed(0); }, 250);
  if (QUIET) pushHeadEl.textContent = "Server push — OFF (quiet/idle mode): socket stays silent until you Send";

  let ws;
  function connect() {
    line(echoEl, "→ connecting to " + url);
    ws = new WebSocket(url);
    ws.onopen = () => { statusEl.textContent = "connected"; statusEl.className = "ok"; send("hello from the browser"); };
    ws.onclose = () => { statusEl.textContent = "closed"; statusEl.className = "bad"; };
    ws.onerror = () => { statusEl.textContent = "error"; statusEl.className = "bad"; };
    ws.onmessage = (e) => {
      markActivity();
      if (e.data.startsWith("push ")) { line(pushEl, e.data); return; }
      if (pendingSend !== null) {
        const ms = (performance.now() - pendingSend).toFixed(0);
        line(echoEl, "← " + e.data + "  (" + ms + " ms round-trip)");
        pendingSend = null;
      } else {
        line(echoEl, "← " + e.data);
      }
    };
  }
  function send(v) {
    if (!v || !ws || ws.readyState !== 1) return;
    const idle = ((performance.now() - lastActivity) / 1000).toFixed(0);
    line(echoEl, "→ " + v + "  (after " + idle + "s idle)");
    pendingSend = performance.now();
    markActivity();
    ws.send(v);
  }
  document.getElementById("send").onclick = () => send(document.getElementById("msg").value || "ping");
  document.getElementById("msg").addEventListener("keydown", (e) => { if (e.key === "Enter") document.getElementById("send").click(); });
  connect();
</script>
</body>
</html>
""").encode("utf-8")


class Conn:
    """Buffered reader over a blocking socket."""

    def __init__(self, sock):
        self.sock = sock
        self.buf = b""

    def recv_exact(self, n):
        while len(self.buf) < n:
            chunk = self.sock.recv(4096)
            if not chunk:
                raise ConnectionError("peer closed")
            self.buf += chunk
        out, self.buf = self.buf[:n], self.buf[n:]
        return out

    def read_request_head(self):
        while b"\r\n\r\n" not in self.buf:
            chunk = self.sock.recv(4096)
            if not chunk:
                raise ConnectionError("peer closed before request head")
            self.buf += chunk
        head, self.buf = self.buf.split(b"\r\n\r\n", 1)
        lines = head.decode("latin1").split("\r\n")
        method, path, _ = lines[0].split(" ", 2)
        headers = {}
        for ln in lines[1:]:
            if ":" in ln:
                k, v = ln.split(":", 1)
                headers[k.strip().lower()] = v.strip()
        return method, path, headers


def ws_accept(key):
    digest = hashlib.sha1((key + WS_GUID).encode()).digest()
    return base64.b64encode(digest).decode()


def encode_text(payload: bytes) -> bytes:
    n = len(payload)
    header = bytearray([0x81])  # FIN + text opcode
    if n < 126:
        header.append(n)
    elif n < 65536:
        header.append(126)
        header += n.to_bytes(2, "big")
    else:
        header.append(127)
        header += n.to_bytes(8, "big")
    return bytes(header) + payload


def read_frame(conn: Conn):
    """Returns (opcode, payload). Raises ConnectionError on EOF."""
    b1, b2 = conn.recv_exact(2)
    opcode = b1 & 0x0F
    masked = b2 & 0x80
    length = b2 & 0x7F
    if length == 126:
        length = int.from_bytes(conn.recv_exact(2), "big")
    elif length == 127:
        length = int.from_bytes(conn.recv_exact(8), "big")
    mask = conn.recv_exact(4) if masked else b"\x00\x00\x00\x00"
    payload = conn.recv_exact(length) if length else b""
    if masked and payload:
        payload = bytes(payload[i] ^ mask[i % 4] for i in range(len(payload)))
    return opcode, payload


def serve_ws(conn: Conn, key: str, peer):
    accept = ws_accept(key)
    resp = (
        "HTTP/1.1 101 Switching Protocols\r\n"
        "Upgrade: websocket\r\n"
        "Connection: Upgrade\r\n"
        f"Sec-WebSocket-Accept: {accept}\r\n\r\n"
    )
    conn.sock.sendall(resp.encode())
    print(f"[{peer}] websocket open")

    push_n = 0
    last_push = time.monotonic()
    try:
        while True:
            # Wake up at least every ~0.5s to emit server pushes even when the
            # client is silent — this drives the server->client relay direction.
            r, _, _ = select.select([conn.sock], [], [], 0.5)
            if r and not conn.buf:
                pass  # readable; fall through to read a frame
            if r or conn.buf:
                opcode, payload = read_frame(conn)
                if opcode == 0x8:  # close
                    conn.sock.sendall(bytes([0x88, 0x00]))
                    print(f"[{peer}] websocket closed by client")
                    return
                if opcode == 0x9:  # ping -> pong
                    conn.sock.sendall(bytes([0x8A, 0x00]))
                    continue
                if opcode == 0x1:  # text -> echo
                    text = payload.decode("utf-8", "replace")
                    print(f"[{peer}] recv: {text!r}")
                    conn.sock.sendall(encode_text(("echo: " + text).encode()))
            if not QUIET:
                now = time.monotonic()
                if now - last_push >= 2.0:
                    push_n += 1
                    last_push = now
                    conn.sock.sendall(encode_text(f"push #{push_n} @ {time.strftime('%H:%M:%S')}".encode()))
    except (ConnectionError, OSError):
        print(f"[{peer}] websocket dropped")


def handle(sock, addr):
    peer = f"{addr[0]}:{addr[1]}"
    conn = Conn(sock)
    try:
        method, path, headers = conn.read_request_head()
        is_ws = (
            "upgrade" in headers.get("connection", "").lower()
            and headers.get("upgrade", "").lower() == "websocket"
        )
        if path.split("?")[0] == "/ws" and is_ws:
            serve_ws(conn, headers.get("sec-websocket-key", ""), peer)
        elif method == "GET":
            body = PAGE.replace(b"__QUIET__", b"true" if QUIET else b"false")
            head = (
                "HTTP/1.1 200 OK\r\n"
                "Content-Type: text/html; charset=utf-8\r\n"
                f"Content-Length: {len(body)}\r\n"
                "Connection: close\r\n\r\n"
            )
            sock.sendall(head.encode() + body)
        else:
            sock.sendall(b"HTTP/1.1 405 Method Not Allowed\r\nConnection: close\r\n\r\n")
    except (ConnectionError, OSError, ValueError) as e:
        print(f"[{peer}] {e}")
    finally:
        try:
            sock.close()
        except OSError:
            pass


def main():
    global QUIET
    args = sys.argv[1:]
    QUIET = "--quiet" in args
    ports = [a for a in args if not a.startswith("-")]
    port = int(ports[0]) if ports else 8080
    # Bind host defaults to loopback (right for phone/desktop testing). Set
    # WS_ECHO_HOST=0.0.0.0 to make it reachable from another container.
    host = os.environ.get("WS_ECHO_HOST", "127.0.0.1")
    srv = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    srv.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    srv.bind((host, port))
    srv.listen(64)
    mode = "  [QUIET: no server push — idle test]" if QUIET else ""
    print(f"ws-echo listening on http://{host}:{port}  (page at /, websocket at /ws){mode}")
    try:
        while True:
            sock, addr = srv.accept()
            threading.Thread(target=handle, args=(sock, addr), daemon=True).start()
    except KeyboardInterrupt:
        print("\nbye")


if __name__ == "__main__":
    main()
