# Wispers Access — self-hosting integrations plan

> Status: The **generic waserver container** (`waserver/docker/`) is built and
> validated end-to-end — a reconcile wrapper + supervisord running one `serve` per
> share, configured from a `SHARES` env var or a mounted file. It is
> platform-independent: multi-share (#1), `host:port` upstream (#2), WebSockets (#3),
> graceful SIGTERM/SIGINT, a healthcheck, and Host-header pass-through (#5) are all
> done. Proven on a real deployment: an arm64 Lima VM running **Coolify + a private
> Odoo** (no published ports), reached from an Android Access client *through the
> container* — landing page, login POST, session, and authenticated navigation all
> clean. The arm64 image ran healthy, so **#4** is down to the multi-arch *buildx*
> packaging. **Coolify is the first integration, not the scope** — a thin recipe over
> the generic container (`integrations/coolify/`), also built and validated (#10).
> Remaining: multi-arch build (#4), publish the image, the base-URL escape-hatch doc
> (#5), and future platform recipes.

## Goal

Make a **self-hosted app reachable through Wispers Access** as easily as you'd expose
it to the Internet — ideally easier — while keeping it *off* the public Internet. You
hand out invite codes instead of a public URL. **Coolify is the first target, not the
scope**: the deliverable is a generic container that drops into any docker-compose
platform, with a thin per-platform recipe on top.

Two measurable claims to aim for:

1. **On a NAT'd box: easier than public exposure.** Public exposure from an internal
   machine needs port forwarding + public/static IP or dynamic DNS + router/IT
   cooperation. Wispers needs only *outbound* connectivity, so the honest comparison is
   "edit nothing on your network" vs. "network surgery." (The standard workaround here —
   across Coolify, Dokploy, and friends alike — is a hand-set-up Cloudflare Tunnel.)
2. **On any box: per-app effort stays one action.** Exposing app #2 must cost a single
   incremental action — "add a share" — not "deploy another container." The bar is
   whatever the smallest per-app expose gesture is on your platform (a domain field, a
   label, a port map); we must be no worse.

The recipient asymmetry is inherent and not a bug: a public URL opens in any browser;
Wispers requires the visitor to install a client and redeem an invite. That *is* the
access control. "As easy as exposing" means the **host's** experience; the visitor
experience is a separate bar and we should be explicit about that in how we pitch it.

A second visitor-side caveat, by design rather than a bug: a **backgrounded Android
client drops its connection** (no foreground service) — seen in the Odoo test when the
node went `offline` after ~2 min idle. For **interactive access** this is a non-issue:
foreground means connected, and a backgrounded session has no observer to serve (the
tree falls with nobody to hear it). It only matters for **push-class apps** — chat,
live notifications — that need a live socket while backgrounded, which is outside this
use case. Foreground the client and it reconnects.

## Architecture: a generic container + thin per-platform recipes

The deliverable splits in two:

- **The generic `waserver` container** (`waserver/docker/`) — one per server, attached
  to a Docker network, hosting many shares, each mapping to `<container>:<port>` via
  Docker DNS. "Make an app reachable" becomes "add a share," structurally the same move
  as a reverse proxy (one instance fronts everything; apps opt in). It knows nothing
  about any platform.
- **Per-platform integrations** (`integrations/<platform>/`) — thin recipes over the
  container. **Invariant: integrations depend on the container, never on each other.**
  That is what keeps "add a platform" to "add a sibling folder," never a reshuffle.

**Coolify is one instance of the cross-stack pattern**: a waserver container joining a
shared external network (`coolify`) to front apps deployed as separate stacks. Swap the
network name and the same file is a generic multi-stack setup.

Why one container per server, not a per-app sidecar: a pure sidecar (one waserver = one
share = one daemon) fails claim #2 — each new app means a whole new container.

## The generic waserver container

Platform-independent. Everything below is done regardless of where it runs.

### 1. Multi-share — ✅ built in `waserver/docker/`, validated end-to-end

"Run N shares at once" does *not* need one-process multiplexing in `waserver`. The
container runs **one `waserver serve <share> <upstream>` per share under an off-the-shelf
process supervisor** (supervisord / s6-overlay as PID 1). The supervisor owns the three
things a naive `serve & serve & wait` gets wrong: SIGTERM **fan-out** to every child,
per-share **crash-restart**, and zombie **reaping**. Per-process also buys fault
isolation and add/remove-a-share without disturbing siblings; the price is supervisor
*config*, not Rust.

The binary provides every verb this needs: the per-share volume layout
(`share_config.json`, `root_key.bin`, `registration.pb`), `serve` (foreground, one
share), `status` (enumerates *every* initialised share + its upstream, via
`storage::list_shares()`), and `deinit` (deletion). No single multiplexing daemon, no
in-process reconfiguration.

The only new artifact is a thin **reconcile wrapper**: read the desired shares from env
/ a mounted file, `waserver init` any that `status` doesn't list, then emit one
supervised `serve <share> <upstream>` per share and `exec` the supervisor. Identity
(`connectivity_group_id` + root key) is the create-once part `init` guards; the upstream
is just the `serve` argument, so a changed mapping takes effect on the next redeploy with
no stored-config drift.

### 2. Address `host:port`, not just port — ✅ DONE

The `serve`/`start` CLI takes a `[host:]port` upstream (bare port ⇒ `127.0.0.1`;
`app:3000` for a Docker compose service; `:3000` also accepted), validated by a
`parse_upstream` helper, threaded as the dial address and connected via
`TcpStream::connect` (tokio resolves DNS names). The upstream is carried as a string
throughout, including the IPC status field.

### 3. WebSocket support — ✅ DONE

`try_forward` detects an HTTP/1.1 Upgrade, preserves the `Connection`/`Upgrade`
handshake headers, and on a `101` splices raw bytes both ways via `hyper::upgrade` +
`tokio::io::copy_bidirectional` — `serve_connection(...).with_upgrades()` on the
QUIC-server side, `conn.with_upgrades()` on the upstream-client side. Protocol-agnostic
(no frame parsing), so subprotocols / binary / `permessage-deflate` pass through
untouched. Mirrored on waclient and the Android client (the latter via Ktor
`OutgoingContent.ProtocolUpgrade`). Verified end-to-end on desktop + a Pixel 8,
including a foreground idle socket holding well past the ICE consent window. Covers the
dashboards / chat / Gitea-live-updates class.

### 4. Container conventions — ✅ (arm64 🟡)

Built in `waserver/docker/` (Dockerfile + the `entrypoint.sh` reconcile wrapper +
`supervisord.conf`), validated end-to-end.

- ✅ **reconcile wrapper + supervisor**: config from a `SHARES` env var (or a mounted
  file; `name | display | upstream`, `;` also separates entries) → `waserver init` new
  shares (keyed off `status`, idempotent — identity created once) → generate one
  supervisord program per share → `exec` supervisord as PID 1. Removing a share *stops
  serving* it but does **not** `deinit` it — destroying a connectivity group + its
  members on a config typo is too dangerous, so deletion stays a manual
  `waserver deinit`.
- ✅ logs reach the container's **stdout/stderr** (a platform's log viewer reads them).
  TODO: `serve` also writes a redundant daily file under `/data`; per-share log prefixing
  not done yet.
- ✅ graceful shutdown on SIGTERM — `serve` traps SIGTERM **and** SIGINT and runs the
  same clean shutdown as `waserver stop`, exiting 0 so a supervisor reads it as an
  intended stop. supervisord delivers TERM to each `serve` and escalates to KILL as the
  backstop.
- ✅ a **healthcheck** — `healthcheck.sh` parses `waserver status`; healthy once every
  share reports `serving` (Docker `--start-period` covers the connecting window).
- ✅ runs in **foreground** — supervisord runs `serve` (foreground), never `start`
  (which daemonises and would detach from PID 1).
- 🟡 **multi-arch (arm64)** — arm64 is **proven** (the arm64 image ran healthy in an
  arm64 Lima VM fronting Odoo); only the multi-arch **buildx** packaging is left
  (`docker buildx --platform linux/amd64,linux/arm64`) so one artifact also serves amd64.

(Build note: in a clean Debian builder, `wispers-connect`'s stack needs `cmake` +
`clang`/`libclang` for boring-sys/BoringSSL and `protobuf-compiler` for prost/tonic —
see `waserver/docker/Dockerfile`.)

**IPC version skew (CLI vs daemon).** A platform's web terminal (#7) runs `waserver`
subcommands against the *running* daemon, which after an image-pull + restart may be an
older binary than the CLI. The local IPC (serde-JSON over a unix socket) must tolerate
that: evolve **additively** (serde ignores unknown fields; new fields via
`#[serde(default)]`/`Option` are forward- and backward-compatible); never
rename/retype/remove a field in place. Gate any unavoidable break behind a dedicated
**IPC protocol version** that fails soft ("daemon too old, restart it"). `stop` stays
robust regardless: freeze the `Shutdown` request wire form and confirm death by
socket-close rather than parsing the reply.

### 5. Host-header behavior — RESOLVED empirically: pass-through is the default

Clients reach shares as `<share>.localhost:<port>` and that Host passes through. The
first real test — **Odoo 18 through the container** — did *not* misbehave: render, login
POST, session cookies, and authenticated navigation all worked untouched. So the
container ships **pass-through, no Host rewrite, in v1**. The escape hatch is a
**documented base-URL convention** (e.g. Odoo's `web.base.url` + `…freeze`) for the
narrower class that bakes *absolute* URLs into artifacts leaving the tunnel —
notification/reset emails, webhooks, OAuth redirect URIs — or that hard-validates
Origin. A per-share Host rewrite stays a *future* option if some app needs it, not a v1
requirement.

## Per-platform integrations

### The pattern

Every "similar platform" (Coolify, Dokploy, Portainer, …) varies along the same four
axes, and all four live *inside* `integrations/<platform>/`:

1. **How it deploys a container/compose** (Coolify: "Docker Compose Empty").
2. **How you join a shared network + its name** (Coolify: external `coolify`).
3. **Its catalog/template format**, if any (Coolify template, Portainer stack, Unraid
   XML, CasaOS manifest…).
4. **Optional glue code** against its API (Coolify's target-discovery, #9).

Because the container is generic, a new integration only answers "how does *this*
platform do those four things." The invariant (integrations → container, never
integration → integration) means any platform folder can be added, rewritten, or deleted
in isolation.

### Coolify (first, fully worked)

**What Coolify gives us to work with:**

- **Private services already exist.** A compose service with no domain assigned and no
  ports mapped stays on the internal Docker network — never touched by the Traefik/Caddy
  proxy or Let's Encrypt. "Host but don't expose" is a supported, documented state.
- **One-click templates** are plain docker-compose files + metadata + "magic" env vars
  (`SERVICE_FQDN_*`, `SERVICE_PASSWORD_*`, `${VAR:?}`), submitted by PR to
  `templates/compose/`; catalog entry requires the source repo to have **1,000+ GitHub
  stars**. The **"Docker Compose Empty"** deploy type runs the same machinery from a
  pasted compose file — so a template works today via copy-paste.
- **A real REST API** (token-scoped, `/api/v1`): `POST /services` accepts
  `docker_compose_raw`; full env-var / deploy / start-stop / logs endpoints exist.
- **Maintainer appetite is low for built-in integrations, fine for recipes.** Cloudflare
  Tunnel ships *zero* Coolify code, just a knowledge-base guide. Realistic ceiling for
  "official": a docs page + eventually a catalog template (star-gated).

**6. Configuration — Coolify-native, no custom surface.** Shares are declared as env
vars (`SHARES`) / a mounted file, edited in Coolify's own UI like any other service's
config. We are not special here.

**7. Runtime secrets & actions — Coolify's built-in web terminal.** An invite is a
generated secret you read out; revoke is a runtime action. `waserver invite`,
`waserver nodes`, `waserver revoke` run through the terminal (xterm.js, exec into the
container, gated by Coolify team RBAC). The generic concern is "a runtime exec surface";
the Terminal is Coolify's instance of it. All three subcommands exist and `invite`/`nodes`
were exercised in the Odoo test.

**8. Admin access — RESOLVED: Coolify's built-in terminal + `waserver` CLI, no custom
surface.** No admin traffic listens, so there's no new hole: the WC API key lives in a
Coolify secret, terminal access is gated by Coolify RBAC, and anyone with it has
root-equivalent control of the box anyway. Fine at the **govt-pilot bar**.

**9. Target discovery — RESOLVED: Coolify API, not the Docker socket.** Coolify names
containers `<service>-<uuid>`, so a picker beats typing `qdm4k8s-app-1:3000`.
- **Docker socket — rejected.** It exposes *every* container on the host and is not
  read-only (the `:ro` flag freezes the socket file, not the API — the container could
  still create containers / `exec` / bind-mount host paths ≈ root on the host). Scoping
  it needs a `docker-socket-proxy` sidecar, which erases the "simpler" advantage.
- **Coolify API — chosen.** One scoped token (a Coolify secret); the container lists only
  Coolify-managed resources via REST. No host access, small blast radius.

**10. The artifact — ✅ built + validated.** `integrations/coolify/compose.yaml` (paste
as "Docker Compose Empty") + a recipe README, using `${WC_API_KEY:?}` and `SHARES` env
vars, tested end-to-end against Odoo the way Coolify runs it (on the shared `coolify`
network, upstream reached by container name). Remaining: **publish the image** so Coolify
can pull it, and the docs-page / catalog-template contribution (star-gated).

### Future platforms

Dokploy, Portainer, CasaOS, Unraid, … — each a sibling folder under `integrations/`,
added when actually done. No speculative scaffolding; extract shared helpers only after
2–3 real integrations show repeated commonality (per-platform silos you can delete beat a
premature abstraction you have to live inside).

## Distribution & open items

- **Publish the image** — `ghcr.io/s-te-ch/wispers/access/waserver`; Coolify and any
  platform that pulls needs it in a registry (until then, a real dashboard deploy can't
  pull it).
- **Multi-arch buildx** (#4) — arm64 proven; `--platform linux/amd64,linux/arm64` for one
  artifact that also serves amd64. Home-lab / Pi users are core audience.
- **Base-URL escape-hatch doc** (#5) — for apps that bake absolute URLs into artifacts
  leaving the tunnel.
- **Coolify docs-page / catalog template** — knowledge-base contribution alongside
  Cloudflare Tunnels; catalog template once star count allows.

## Sources

- [Coolify service template contribution guide](https://coolify.io/docs/get-started/contribute/service)
- [Docker Compose handling & private services](https://coolify.io/docs/knowledge-base/docker/compose)
- [API authorization](https://coolify.io/docs/api-reference/authorization)
- [Coolify OpenAPI spec](https://raw.githubusercontent.com/coollabsio/coolify/v4.x/openapi.json)
- [Cloudflare Tunnels overview](https://coolify.io/docs/knowledge-base/cloudflare/tunnels/overview)
- [Custom template discussion #3002](https://github.com/coollabsio/coolify/discussions/3002)
