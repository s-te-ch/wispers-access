# waserver container

One container per server that hosts **many shares** on a Docker network, reaching
each app by service-DNS. "Make an app reachable" becomes "add a line to `SHARES`,"
not "deploy another container."

This is `waserver`, containerized: the binary plus a process supervisor. It's
provider-agnostic — it runs in any docker-compose stack, plain `docker run`, or an
orchestrator. Platform-specific recipes (Coolify, …) live under `integrations/`.

## How it works

```
entrypoint.sh
  ├─ read desired shares from $SHARES (or a file at /config/shares.conf)
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
| `supervisord.conf`    | base supervisor config; per-share programs generated into `conf.d/` |
| `healthcheck.sh`      | healthy once every share reports `serving`              |
| `shares.example.conf` | the file form of the shares config (`name \| display \| upstream`) |
| `compose.yaml`        | generic same-stack test rig: a private `ws-echo` app + the container |

## Configuring shares

Two equivalent ways:

- **`SHARES` env var** (simplest for compose / platform UIs): one
  `name | display | upstream` per line; `;` also separates entries so a single-line
  value works. Takes precedence over the file.
- **A mounted file** at `/config/shares.conf` (override with `$SHARES_FILE`) — same format.

`upstream` is `host:port` on the Docker network, or a bare port (host defaults to
`127.0.0.1`). A compose service is just its name, e.g. `app:8080`.

Removing a share **stops serving** it but does **not** delete it — run
`waserver deinit <name>` to destroy a share's identity and members (irreversible).

## Try it locally

Prereqs: Docker and a Wispers Connect **API key**.

```sh
export WC_API_KEY=...    # or put it in a .env file as WC_API_KEY=...
docker compose -f waserver/docker/compose.yaml up --build
```

The container inits the `demo` share, connects to the hub, and serves it; the
healthcheck flips to healthy once it reports `serving`. In another shell:

```sh
C="docker compose -f waserver/docker/compose.yaml exec waserver"
$C waserver status                                   # fleet table: demo | serving | ...
$C waserver status demo                              # share detail incl. members
$C waserver invite demo alice alice@example.com      # prints an invite code
```

Redeem that invite from a Wispers client to reach the ws-echo page/socket **through**
the container. The app publishes no ports, so it's reachable *only* via the share.

## State & persistence

Everything under `$HOME` (= `/data`): per-share identity (keys + registration), IPC
sockets, logs. Mount a volume at `/data`. **Without a persistent `/data`, every boot
creates a brand-new connectivity group** — so in any real deployment, mount a volume.


## Self-hosted backend (optional)

By default the container uses the managed Wispers Connect backend. You can
point it to your own, self-hosted backend using the flag `--backend` or by
setting `WC_BACKEND`. It's read once at `waserver init`, stored per-share,
and baked into the invite codes so a guest's client joins the same hub
automatically. This is per-share, so different shares can use different
backends. See the [wispers-hub](https://github.com/s-te-ch/wispers-hub)
repo for standing up your own hub.

## Notes

- **Prebuilt image:** every release publishes a multi-arch (amd64 + arm64) image at
  `ghcr.io/s-te-ch/wispers/access/waserver`, tagged `:X.Y.Z` and `:latest`. No need
  to build unless you're changing it.
- **Build caching:** no `cargo-chef` layer yet, so when building locally a source
  change recompiles the crates.
- **Logs:** `serve` writes stdout/stderr (captured) plus a redundant daily file under
  `/data`. No per-share log prefixing yet.
