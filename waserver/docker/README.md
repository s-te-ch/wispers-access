# waserver container

This is the containerised version of waserver - the binary plus a process
supervisor.

Unlike a normal waserver, it can reach each shared app by docker's service-DNS.
You don't even have to expose ports to the host system docker runs on. It's also
provider-agnostic — it runs in any docker-compose stack, plain `docker run`, or
an orchestrator.

Platform-specific recipes (Coolify, …) live under `integrations/`.

## Quick start

We'll start with a typical setup, waserver in a docker compose stack next to an
app it shares, without any published ports. The example in `compose.yaml` uses
Excalidraw as a demo app. Change to the directory (`cd waserver/docker`), then
run

```sh
docker compose up -d
docker compose exec waserver waserver invite demo "My phone" me@example.com
```

to start the stack and create an invite. Scan the QR code with a Wispers Access
client (or, if you're using a desktop client, copy-paste the code). This adds
the share to your client and lets you open Excalidraw. Done! You've just shared
your internal app without publishing it to the internet (this works even if your
client is on a different part of the internet).

You can edit the `demo` share's configuration with

```sh
docker compose exec waserver waserver edit demo
```

This allows you to change display names, add new apps, or change an app's
upstream address (i.e. the host:port waserver proxies). You're not restricted to
upstreams within the docker compose stack - setting the upstream to
`"host.docker.internal:3000"` for example points it at port 3000 on the host
computer.

If you don't want to use compose, you can also invoke docker directly:

```sh
docker run -d --name waserver --restart unless-stopped \
  -e SHARE_ID=team -e SHARE_NAME="Awesome Team" \
  --add-host host.docker.internal:host-gateway \
  -v waserver-data:/data \
  ghcr.io/s-te-ch/wispers/access/waserver:latest

# The waserver commands still work, with a slightly different incantation
docker exec -it waserver waserver edit team
docker exec -it waserver waserver invite team "Alice's phone" alice@example.com
```

Finally, if you don't want to type the `docker exec` prefixes every time:
`docker exec -it waserver bash` gets you a shell where you can use `waserver`
commands directly.

## Configuring the container

The container can be configured through environment variables, which it reads on
the first start to create the default share. Later starts find one or more
initialised shares on `/data` and skip this step.

| variable          | default   | meaning                                                       |
|-------------------|-----------|---------------------------------------------------------------|
| `SHARE_ID`        | `default` | the default share's ID                                        |
| `SHARE_NAME`      |           | the display name of the default share                         |
| `SHARE_TRANSPORT` | `iroh`    | the peer-to-peer transport library to use                     |
| `WC_API_KEY`      |           | API key for transport `wispers-connect`                       |
| `WC_BACKEND`      |           | Optional backend URL override for transport `wispers-connect` |
| `EDITOR`          |           | the editor `waserver edit` opens                              |

Note that while the container creates a single default share on startup, you can
always invoke `waserver init` inside the container to create more. You do,
however, have to restart the container for them to get picked up by supervisord.

## Configuring a share

The easiest way to configure a share is to run `waserver edit <share>` in the
container's shell. This opens the correct TOML file and automatically reloads
the configuration in `waserver`.

The file has one `[[app]]` block per shared app. Each block has the fields `id`
(keep this stable, guest nodes refer to it), `name` (the display name), and
`upstream` (the address of the web app waserver proxies). The initial
configuration comes with comments explaining the fields.

`upstream` is `host:port` on the Docker network, or `host.docker.internal:port`
if you want to address a port on the host computer. A compose service is just
its name, e.g. `app:8080`.

There are several editors in the image: nano (the default), vim (`vim-tiny`) and
mg (micro emacs). Pick another with `select-editor` (remembered on `/data`) or
the `EDITOR` variable.

### Keeping the config outside the container

To keep a share's config in version control, or to write it before the container
exists, mount it at `/config/<share>.toml`:

```sh
docker run -d --name waserver --restart unless-stopped \
  -e SHARE_ID=team \
  -v "$PWD/team.toml:/config/team.toml:ro" \
  -v waserver-data:/data \
  ghcr.io/s-te-ch/wispers/access/waserver:latest
```

The entrypoint detects the mounted file and uses it. `SHARE_NAME` now defaults
to the `name` field in the file. If you edit the file on the host, you have to
apply the edits with `waserver reload team` (or just restart the container).
Since the file is mounted read-only, `waserver edit` within the container won't
work.

## How it works

At startup, the entrypoint checks the existence of the desired share (based on
the environment variables `SHARE_ID`, `SHARE_NAME`, and `SHARE_TRANSPORT`) and
initialises it if necessary. The state gets written to `/data`.

Once share initialisation is done, the entrypoint brings up one supervisord
program per initialised share, creating a process tree like this:

```
supervisord (PID 1)
    ├─ waserver serve <share-a>
    ├─ waserver serve <share-b>
    └─ …   (supervisord owns SIGTERM fan-out, restart, reaping)
```

## Developing the image

We provide prebuilt images, so you don't have to build your own. Every release
publishes a multi-arch (amd64 + arm64) image at
`ghcr.io/s-te-ch/wispers/access/waserver`, tagged `:X.Y.Z` and `:latest`.

If you want to work on the docker image and build your own:
`docker compose up --build` in this folder builds waserver from the checkout
instead of pulling the release, and otherwise runs the quick start. The
healthcheck flips to healthy once every share reports `serving`.

## Notes

- **Build caching:** no `cargo-chef` layer yet, so when building locally a
  source change recompiles the crates.
- **Logs:** `serve` writes stdout/stderr (captured) plus a redundant daily file
  under `/data`. `waserver logs -f` in the shell follows them. No per-share
  log prefixing in the container's own output yet.
