# Wispers Access connector image

One container per server that hosts **many shares**, attached to a Docker
network, reaching each app via service-DNS — the same model Coolify uses for its
own proxy. "Make an app reachable" becomes "add a line to the shares config,"
not "deploy another container."

This directory is the image + a local test rig. None of it requires Coolify;
Coolify is just `docker compose` + a UI on top, so you can build and prove the
whole thing locally first.

## How it works

```
entrypoint.sh
  ├─ read desired shares from /config/shares.conf
  ├─ `waserver init` any not yet initialised   (identity created once, on /data)
  ├─ generate one supervisord program per share
  └─ exec supervisord (PID 1)
         ├─ waserver serve <share-a> <upstream-a>
         ├─ waserver serve <share-b> <upstream-b>
         └─ …   (supervisord owns SIGTERM fan-out, restart, reaping)
```

## Files

| file                  | role                                                    |
|-----------------------|---------------------------------------------------------|
| `Dockerfile`          | multi-stage: build `waserver`, then a slim runtime + supervisord |
| `entrypoint.sh`       | reconcile shares → generate supervisord config → exec   |
| `supervisord.conf`    | base supervisor config; per-share programs are generated into `conf.d/` |
| `healthcheck.sh`      | healthy once every share reports `serving`              |
| `shares.example.conf` | the shares config format (`name \| display \| upstream`) |
| `compose.yaml`        | local test rig: a private `ws-echo` app + the connector |

## Try it locally

Prereqs: Docker (you have it) and a Wispers Connect **API key**.

```sh
export WC_API_KEY=...    # or put it in connector/.env as WC_API_KEY=...
docker compose -f connector/compose.yaml up --build
```

Expected: the connector inits the `demo` share, connects to the hub, and serves
it; the healthcheck flips to healthy once it reports `serving`. In another shell:

```sh
C="docker compose -f connector/compose.yaml exec connector"
$C waserver status                                   # demo (serving, upstream app:8080)
$C waserver invite demo alice alice@example.com      # prints an invite code
```

Redeem that invite from a Wispers client (Android/desktop) to reach the ws-echo
page and WebSocket **through** the connector — the same end-to-end flow as the
phone/desktop tests, but proxied out of a container. The app publishes no ports,
so it is reachable *only* via the connector, never directly.

### Graceful stop / restart

```sh
docker compose -f connector/compose.yaml stop connector   # supervisord TERMs each serve; each exits 0
```

`serve` traps SIGTERM/SIGINT and shuts its hub session down cleanly, so this is a
graceful stop, not a kill. supervisord escalates to SIGKILL only as a backstop.

## State & persistence

Everything under `$HOME` (= `/data`): per-share identity (keys + registration),
IPC sockets, logs. The `connector-state` volume persists it. **Without a
persistent `/data`, every boot creates a brand-new connectivity group** — so in
any real deployment, mount a volume there.

## Configuring shares

Edit `shares.example.conf` (or mount your own at `/config/shares.conf`). One
share per line: `name | display name | upstream`. Restart the container to
reconcile. Removing a line **stops serving** that share but does **not** delete
it — run `waserver deinit <name>` explicitly to destroy a share's identity and
members (irreversible).

## Notes / TODO

- **Build caching:** the Dockerfile recompiles all crates on every source
  change. Add [`cargo-chef`](https://github.com/LukeMathWalker/cargo-chef) to
  cache the dependency layer when iteration speed matters.
- **Multi-arch (arm64):** `docker buildx build --platform linux/amd64,linux/arm64`
  (colima/buildx already available). Core Coolify audience runs arm64 home labs.
- **Per-share log prefixing:** N `serve` processes interleave on stdout/stderr
  with timestamps but no share tag yet; add a prefix (binary log field or a
  wrapped command) if attribution gets noisy.
- **stdout vs files:** `serve` logs to stderr (captured here) *and* a daily file
  under `/data` (harmless but redundant in a container) — a `--log-stdout`-style
  switch could drop the file sink.
