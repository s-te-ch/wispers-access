# waserver container

This is the containerised version of waserver - the binary plus a process
supervisor.

Unlike a normal waserver, it can reach each shared app by docker's service-DNS.
You don't even have to expose ports to the host system docker runs on. It's also
provider-agnostic — it runs in any docker-compose stack, plain `docker run`, or
an orchestrator.

Platform-specific recipes (Coolify, …) live under `integrations/`.

## Quick start

The typical setup: waserver next to the app it shares, in one compose stack,
the app with no published ports. `compose.yaml` here does that with
Excalidraw as the app. Clone the repo (or fetch the four files in this
folder) and run:

```sh
cd waserver/docker
docker compose up -d
docker compose exec waserver waserver invite demo "My phone" me@example.com
```

Scan the QR code with a Wispers Access client. The whiteboard opens with the
Wispers logo drawn on it, served through the share; the app itself is
reachable no other way. `demo.toml` is the share's config: one `[[app]]`
block per app, with `upstream = "<service>:<port>"`. Edit it and run
`docker compose exec waserver waserver reload demo`.

Without a compose stack, the same container takes its share from variables
and gets its apps from an editor in its shell:

```sh
docker run -d --name waserver --restart unless-stopped \
  -e SHARE_ID=team -e SHARE_NAME="Awesome Team" \
  --add-host host.docker.internal:host-gateway \
  -v waserver-data:/data \
  ghcr.io/s-te-ch/wispers/access/waserver:latest

docker exec -it waserver waserver edit team      # add the app, see below
docker exec -it waserver waserver invite team "Alice's phone" alice@example.com
```

`edit` opens the share's config in nano. Add one block per app and save; the
server reloads with it. For an app on the host, `upstream =
"host.docker.internal:3000"`:

```toml
[[app]]
id = "myapp"
name = "My App"
upstream = "host.docker.internal:3000"
```

`docker exec -it waserver bash` gets you a shell that greets you with the
shares' state and these commands. The rest of this README explains the
moving parts.

## How it works

```
entrypoint.sh
  ├─ `waserver init` the share from SHARE_ID / SHARE_NAME / SHARE_TRANSPORT
  │                                   (first start only: identity created once,
  │                                    on /data, for that transport)
  ├─ link a config mounted at /config/<share>.toml as the share's share.toml
  ├─ generate one supervisord program per initialised share
  └─ exec supervisord (PID 1)
		 ├─ waserver serve <share-a>
		 ├─ waserver serve <share-b>
		 └─ …   (supervisord owns SIGTERM fan-out, restart, reaping)
```

## Files

| file               | role                                                              |
|--------------------|-------------------------------------------------------------------|
| `Dockerfile`       | multi-stage: build `waserver`, then a slim runtime + supervisord + editors |
| `entrypoint.sh`    | create the configured share once → link mounted configs → generate supervisord config → exec |
| `greeting.sh`      | what every interactive shell in the container prints first        |
| `supervisord.conf` | base supervisor config; per-share programs generated into `conf.d/` |
| `healthcheck.sh`   | healthy once every share reports `serving`                        |
| `compose.yaml`     | the quick start: a private Excalidraw + the container             |
| `demo.toml`        | the quick start's share config, mounted at `/config/demo.toml`   |
| `excalidraw.conf`, `wispers.excalidraw` | Excalidraw's nginx config and the logo scene it opens on first visit |

## Configuring the container

Environment variables, read on the first start to create the share. Later
starts find the share on `/data` and need none of them.

| variable          | default   | meaning                                                  |
|-------------------|-----------|----------------------------------------------------------|
| `SHARE_ID`        | `default` | the share's id, as used in `waserver` commands (letters, digits, `-`, `_`) |
| `SHARE_NAME`      | required  | the share's name as guests see it; a mounted config's `name` when unset |
| `SHARE_TRANSPORT` | `iroh`    | `iroh` needs no account; `wispers-connect` needs `WC_API_KEY` |
| `WC_API_KEY`      |           | Wispers Connect API key, for a `wispers-connect` share   |
| `WC_BACKEND`      |           | base URL of a self-hosted Wispers Connect hub; blank = managed |
| `EDITOR`          |           | the editor `waserver edit` opens; nano otherwise         |

## Configuring the share

In the container's shell: `waserver edit <share>` opens the share's
`share.toml`, checks it when the editor closes, and reloads the server. The
file is the share's `name` as guests see it and one `[[app]]` block per app
(`id`, `name`, `upstream`); `waserver init` leaves a commented example in it.

`upstream` is `host:port` on the Docker network, or `:port` for the container
itself. A compose service is just its name, e.g. `app:8080`. Keep each app's
`id` stable, guest nodes refer to it.

Editors in the image: nano (the default), vim (`vim-tiny`) and mg. Pick
another with `select-editor` (remembered on `/data`) or the `EDITOR`
variable.

A second share is `waserver init <id> "<name>"` in the shell, then a restart
of the container, which serves every share it finds. `waserver deinit <id>`
destroys a share's identity and guests (irreversible); nothing does that
automatically.

### Keeping the config outside the container

To keep a share's config in version control, or to write it before the
container exists, mount it at `/config/<share>.toml`:

```sh
docker run -d --name waserver --restart unless-stopped \
  -e SHARE_ID=team \
  -v "$PWD/team.toml:/config/team.toml:ro" \
  -v waserver-data:/data \
  ghcr.io/s-te-ch/wispers/access/waserver:latest
```

The mounted file *is* the share's config: the entrypoint links the share's
`share.toml` to it, `SHARE_NAME` defaults to its `name`, and edits on the host
apply with `waserver reload team` (or a restart). Mounted read-only, as above,
`waserver edit` in the container cannot save, which is the point. Keep the
mount for the share's life; a start without it stops with an error naming the
missing file.

## Developing the image

`docker compose up --build` in this folder builds waserver from the checkout
instead of pulling the release, and otherwise runs the quick start. The
healthcheck flips to healthy once every share reports `serving`.

## State & persistence

Everything under `$HOME` (= `/data`): per-share identity (keys + registration),
`share.toml`, IPC sockets, logs. Mount a volume at `/data`. **Without a
persistent `/data`, every boot creates a brand-new share** — so in any real
deployment, mount a volume. The share's config lives there too, at
`/data/.config/waserver/shares/<share>/share.toml`, unless it is mounted from
outside (see above).

## Wispers Connect (optional)

`SHARE_TRANSPORT=wispers-connect` creates the share on Wispers Connect instead
of iroh, with the API key in `WC_API_KEY` (`-e WC_API_KEY=…`, or the
Environment Variables tab of your platform). The transport is fixed from then
on. By default it uses the managed Wispers Connect backend; a self-hosted one
goes in `WC_BACKEND`, read once at `waserver init`, stored with the share, and
baked into the invite codes so a guest's client joins the same hub
automatically. See the [wispers-hub](https://github.com/s-te-ch/wispers-hub)
repo for standing up your own hub.

## Notes

- **Prebuilt image:** every release publishes a multi-arch (amd64 + arm64) image
  at `ghcr.io/s-te-ch/wispers/access/waserver`, tagged `:X.Y.Z` and `:latest`.
  No need to build unless you're changing it.
- **Build caching:** no `cargo-chef` layer yet, so when building locally a
  source change recompiles the crates.
- **Logs:** `serve` writes stdout/stderr (captured) plus a redundant daily file
  under `/data`; `waserver logs -f` in the shell follows them. No per-share
  log prefixing in the container's own output yet.
