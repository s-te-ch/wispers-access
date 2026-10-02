# waserver container

This is the containerised version of waserver - the binary plus a process
supervisor. Unlike a normal waserver, it can reach each shared app by
service-DNS. You don't have to expose their ports to share them.

It's provider-agnostic — it runs in any docker-compose stack, plain `docker
run`, or an orchestrator. Platform-specific recipes (Coolify, …) live under
`integrations/`.

## Quick start

To start, we'll share an app listening on the host's port 3000, using the
prebuilt image. A "share" is one config file, in the format `waserver init`
writes, mounted at `/config/<name>.toml`. The file name determines the share's
ID:

```sh
cat > team.toml <<'TOML'
name = "Awesome Team"

[transport]
kind = "iroh"

[[app]]
id = "myapp"
name = "My App"
upstream = "host.docker.internal:3000"
TOML

docker run -d --name waserver --restart unless-stopped \
  -v "$PWD/team.toml:/config/team.toml:ro" \
  --add-host host.docker.internal:host-gateway \
  -v waserver-data:/data \
  ghcr.io/s-te-ch/wispers/access/waserver:latest

docker exec waserver waserver invite team "Alice's phone" alice@example.com
```

For an app in the same compose stack, the upstream is just `service:port` and
`--add-host` isn't needed. The rest of this README explains the moving parts.

## How it works

```
entrypoint.sh
  ├─ find the desired shares: one /config/<name>.toml per share
  ├─ `waserver init` any not yet initialised   (identity created once, on /data,
  │                                              on the transport the file names)
  ├─ copy each file over its share's share.toml
  ├─ generate one supervisord program per share
  └─ exec supervisord (PID 1)
		 ├─ waserver serve <share-a>
		 ├─ waserver serve <share-b>
		 └─ …   (supervisord owns SIGTERM fan-out, restart, reaping)
```

## Files

| file                  | role                                                    |
|-----------------------|---------------------------------------------------------|
| `Dockerfile`          | multi-stage: build `waserver`, then a slim runtime + supervisord |
| `entrypoint.sh`       | reconcile shares from their configs → generate supervisord config → exec |
| `supervisord.conf`    | base supervisor config; per-share programs generated into `conf.d/` |
| `healthcheck.sh`      | healthy once every share reports `serving`              |
| `share.example.toml`  | one share's config, as mounted at `/config/<name>.toml`       |
| `compose.yaml`        | generic same-stack test rig: a private `ws-echo` app + the container |

## Configuring shares

One file per share, mounted at `/config/<name>.toml`, where `<name>` is the
share's id (letters, digits, `-`, `_`). The format is the same one `waserver
init` writes: the share's `name` as guests see it, a `[transport]` section, and
one `[[app]]` block per app (`id`, `name`, `upstream`). See
`share.example.toml`.

`upstream` is `host:port` on the Docker network, or `:port` for the container
itself. A compose service is just its name, e.g. `app:8080`.

The file's `name` is what `init` registers, and the file replaces the share's
`share.toml` on every start, so editing it and restarting the container is how
apps are added or changed. Removing a file **stops serving** that share but does
**not** delete it — run `waserver deinit <name>` to destroy a share's identity
and guests (irreversible).

## Try it locally

Prerequisites: Docker.

```sh
docker compose -f waserver/docker/compose.yaml up --build
```

The container inits the `demo` share from `share.example.toml` and serves it.
The healthcheck flips to healthy once it reports `serving`. In another shell:

```sh
C="docker compose -f waserver/docker/compose.yaml exec waserver"
$C waserver status                                   # fleet table: demo | serving | ...
$C waserver status demo                              # share detail incl. guests
$C waserver invite demo alice alice@example.com      # prints an invite code
```

Redeem that invite from a Wispers Access client to reach the ws-echo page/socket
**through** the container. The app publishes no ports, so it's reachable *only*
via the share.

## State & persistence

Everything under `$HOME` (= `/data`): per-share identity (keys + registration),
IPC sockets, logs. Mount a volume at `/data`. **Without a persistent `/data`,
every boot creates a brand-new connectivity group** — so in any real deployment,
mount a volume.


## Wispers Connect (optional)

A share's `[transport]` picks iroh, which needs no account, or Wispers Connect.
A Wispers Connect share is created with the API key in `WC_API_KEY`, so pass
that to the container (`-e WC_API_KEY=…`, or the Environment Variables tab of
your platform). By default it uses the managed Wispers Connect backend; a
self-hosted one goes in `WC_BACKEND`, read once at `waserver init`, stored
per-share, and baked into the invite codes so a guest's client joins the same
hub automatically. Different shares can use different backends. See the
[wispers-hub](https://github.com/s-te-ch/wispers-hub) repo for standing up your
own hub.

## Notes

- **Prebuilt image:** every release publishes a multi-arch (amd64 + arm64) image
  at `ghcr.io/s-te-ch/wispers/access/waserver`, tagged `:X.Y.Z` and `:latest`.
  No need to build unless you're changing it.
- **Build caching:** no `cargo-chef` layer yet, so when building locally a
  source change recompiles the crates.
- **Logs:** `serve` writes stdout/stderr (captured) plus a redundant daily file
  under `/data`. No per-share log prefixing yet.
