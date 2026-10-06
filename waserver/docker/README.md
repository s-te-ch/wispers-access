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

## Configuring it

The [hosting guide](../../docs/hosting.md#containers) has the rest: the
variables the container reads on its first start, running waserver's commands
through `docker compose exec`, reaching apps outside the stack, keeping a
share's config outside the container, and upgrading. `share.toml` itself is
explained in the [quick start](../../README.md#2-share-your-app).

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
