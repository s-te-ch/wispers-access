# Wispers Access × Coolify integration plan

> Status: research + scoping notes. Items **#2 (host:port)** and **#3 (WebSockets)**
> are done. **#1 (multi-share)** needs no dedicated binary work: it is realised as
> image-side glue under #4 — a reconcile wrapper plus an off-the-shelf supervisor.
> The open build is the container image (#4, including a graceful-SIGTERM handler in
> `serve`) and the runtime CLI (#7).

## Goal

Make a Coolify-hosted app reachable through Wispers Access **as easily as Coolify
exposes it to the Internet — ideally easier**, while keeping it *off* the public
Internet. You hand out invite codes instead of a public URL.

Concretely, two measurable claims to aim for:

1. **On a LAN / NAT'd box: easier than public exposure.** Public exposure from an
   internal machine needs port forwarding + public/static IP or dynamic DNS +
   router/IT cooperation, none of which Coolify automates (their own answer is the
   Cloudflare Tunnel guide). Wispers needs only *outbound* connectivity, so the
   honest comparison is "edit nothing on your network" vs. "network surgery."
2. **On any box: per-app effort no worse than typing a domain.** In Coolify,
   exposing app #2 costs one domain typed into one field. Our equivalent must be
   "add a share," one action — not "deploy another container."

The recipient asymmetry is inherent and not a bug: a public URL opens in any
browser; Wispers requires the visitor to install a client and redeem an invite.
That *is* the access control. When we say "as easy as exposing," we mean the
**host's** experience; the visitor experience is a separate bar and we should be
explicit about that in how we pitch it.

## What Coolify gives us to work with

- **Private services already exist.** A compose service with no domain assigned
  and no ports mapped stays on the internal Docker network — never touched by the
  Traefik/Caddy proxy or Let's Encrypt. "Host but don't expose" is a supported,
  documented state. Wispers just has to be the thing that then makes it reachable.
- **One-click templates** are plain docker-compose files + metadata + "magic" env
  vars (`SERVICE_FQDN_*`, `SERVICE_PASSWORD_*`, `${VAR:?}` for required input),
  submitted by PR to `templates/compose/`. Catalog entry requires the source repo
  to have **1,000+ GitHub stars**. The "load custom templates by URL" request
  ([discussion #3002](https://github.com/coollabsio/coolify/discussions/3002))
  never shipped. But the **"Docker Compose Empty"** deploy type runs the same
  machinery from a pasted compose file — so a template works today via copy-paste.
- **A real REST API** (token-scoped, `/api/v1`). `POST /services` accepts
  `docker_compose_raw` (base64 compose); full env-var / deploy / start-stop / logs
  endpoints exist. Programmatic provisioning is feasible.
- **Maintainer appetite is low for built-in integrations, fine for recipes.**
  Cloudflare Tunnel — the most mainstream "alternative exposure" — ships *zero*
  Coolify code, just a knowledge-base guide for a user-run container. Realistic
  ceiling for "official": a docs page (low bar, the slot exists) and eventually a
  catalog template (mechanical, gated on stars). A first-class UI toggle would
  require them to *want* it; nothing observed suggests they build that for anyone.

## Architecture decision: one connector per server, not per app

A pure per-app sidecar (one waserver = one share = one daemon) fails claim #2 —
each new app means deploying a whole new container. Instead:

**One Wispers connector container per Coolify server**, attached to the `coolify`
Docker network, hosting multiple shares, each share mapping to `<container>:<port>`
via Docker DNS. "Make this app reachable" becomes "add a share to the connector" —
structurally the same move as Coolify's own proxy (one Traefik fronts everything;
apps opt in via a domain field).

## Work items

### Connector engineering (waserver side)

1. **Multi-share is a container concern, not a binary feature.** "Run N shares at
   once" does *not* need one-process multiplexing in `waserver`. The container runs
   **one `waserver serve <share> <upstream>` per share under an off-the-shelf
   process supervisor** (supervisord / s6-overlay as
   PID 1). The supervisor owns the three things a naive `serve & serve & wait` gets
   wrong: SIGTERM **fan-out** to every child, per-share **crash-restart**, and
   zombie **reaping**. Per-process also buys fault isolation and add/remove-a-share
   without disturbing its siblings; the price is supervisor *config*, not Rust.

   The binary provides every verb this needs: the per-share volume layout
   (`share_config.json`, `root_key.bin`, `registration.pb` per share), `serve`
   (foreground, one share), `status` (enumerates *every* initialised share + its
   upstream, via `storage::list_shares()`), and `deinit` (deletion). There is no
   single multiplexing daemon and no in-process reconfiguration.

   The only new artifact is a thin **reconcile wrapper** in the image (built under
   #4): read the desired shares from env / a mounted config file, `waserver init`
   any that `status` doesn't list and `deinit` any it lists that are no longer
   desired, then emit one supervised `serve <share> <upstream>` per share and
   `exec` the supervisor. Identity (`connectivity_group_id` + root key) is the
   create-once part `init` guards; the upstream is just the `serve` argument, so a
   changed mapping takes effect on the next redeploy with no stored-config drift.
2. **Address `host:port`, not just port — ✅ DONE.**
   The `serve`/`start` CLI takes a `[host:]port` upstream (bare port ⇒ `127.0.0.1`;
   `app:3000` for a Docker compose service; `:3000` also accepted), validated by a
   `parse_upstream` helper, threaded as the dial address and connected via
   `TcpStream::connect` (tokio resolves DNS names). The upstream is carried as a
   string throughout, including the IPC status field. (Host-header handling is a
   separate concern — item #5.)
3. **WebSocket support — ✅ DONE.**
   `try_forward` detects an HTTP/1.1 Upgrade, preserves the `Connection`/`Upgrade`
   handshake headers, and on a `101` splices raw bytes both ways via `hyper::upgrade`
   + `tokio::io::copy_bidirectional` — `serve_connection(...).with_upgrades()` on the
   QUIC-server side, `conn.with_upgrades()` on the upstream-client side. Protocol-
   agnostic (no frame parsing), so subprotocols / binary / `permessage-deflate` pass
   through untouched. Mirrored on waclient and the Android client (the latter via Ktor
   `OutgoingContent.ProtocolUpgrade`). Verified end-to-end on desktop + a Pixel 8,
   including a foreground idle socket holding well past the ICE consent window. This
   covers the dashboards / chat / Gitea-live-updates class.
4. **Container-conventions plumbing** (small individually, all required):
   - the **reconcile wrapper + supervisor** from #1: env / mounted config file →
     `init` new shares & `deinit` removed ones (keyed off `status`, so it's
     idempotent — identity is created once) → generate the supervisord config →
     `exec` supervisord as PID 1. We use an off-the-shelf supervisor rather than
     hand-rolling signal/restart/reap logic in a shell `& … & wait`.
   - log to **stdout** instead of today's daily-rotated files (Coolify's log viewer
     reads stdout); with N processes, prefix each line with its share so the
     interleaved streams stay legible
   - graceful shutdown on SIGTERM — supervisord delivers TERM to each `serve`
     (`stopsignal`/`stopwaitsecs`), but `serve` itself must **trap** it and run the
     clean Wispers-node shutdown; it currently has no SIGTERM handler, so the process
     is killed mid-flight. This is the only binary change this section requires.
   - a healthcheck — parse `waserver status` (are all shares `serving`?)
   - multi-arch build (**arm64** — home-lab / Pi users are core Coolify audience)
   - run in **foreground** — the supervisor runs `serve` (foreground), never
     `start` (which daemonises and would detach from PID 1)

   **IPC version skew (CLI vs daemon).** Coolify's web terminal (#7) runs `waserver`
   subcommands against the *running* daemon, which after an image-pull + restart may be an
   older binary than the CLI. The local IPC (serde-JSON over a unix socket) must tolerate
   that within reason. Rule: evolve **additively** — serde ignores unknown fields by
   default, so new fields (`#[serde(default)]` / `Option`) are forward- and
   backward-compatible; never rename/retype/remove a field in place (retyping a field —
   e.g. a bare port number into a `host:port` string — is a genuine break, acceptable
   only before anything has shipped).
   Gate the rare unavoidable break behind a dedicated **IPC protocol version** — a current
   + min-compatible window that fails soft ("daemon too old, restart it"), keyed off the
   protocol, *not* the package version (which churns without wire changes). `stop` stays
   robust regardless: freeze the `Shutdown` request wire form (bare unit variant, no
   envelope, every daemon understands it) and confirm death by socket-close rather than
   parsing the reply. (In-container, the orchestrator's SIGTERM is the usual stop path —
   hence the graceful-SIGTERM bullet above.)
5. **Host-header behavior.** Clients reach shares as `<share>.localhost:<port>`;
   that Host header currently passes through. Apps validating Host/Origin (CSRF,
   absolute-URL generation) will misbehave. Decide: documented base-URL convention
   or optional per-share Host rewrite. Will surface in the first real test.

### Admin surface

**"Admin UI" is two different things with two different standard answers — split them.**

6. **Configuration** (which shares exist, what each maps to) is **Coolify-native:
   no custom surface needed.** Shares are declared as env vars / a mounted config
   file, edited in Coolify's own UI like every other service's config. We are not
   special here.
7. **Runtime secrets & actions** (generate invite, list members, **revoke**) need a
   live surface — an invite is a generated secret you read out; revoke is a runtime
   action. The standard Coolify mechanism is the **built-in web terminal** (xterm.js,
   exec into any container, gated by Coolify team RBAC): `waserver invite …`,
   `waserver members`, `waserver revoke …`. So waserver needs those CLI subcommands;
   list-members + revoke from day one ("hand out codes to coworkers" implies one
   eventually leaves).

### Design decisions that gate the build (decide before the image exists)

8. **Admin access — RESOLVED: Coolify's built-in terminal + `waserver` CLI, no
   custom admin surface.** There is no turnkey "private admin panel" in Coolify, and
   we need none of its compromises (domain + basic-auth = still public; bind
   `127.0.0.1` + SSH/Tailscale; Docker-network-only). Config goes through Coolify's
   env/UI (#6); invites/members/revoke run as `waserver` CLI through Coolify's
   built-in web terminal (#7). Nothing listens for admin traffic, so there's no new
   hole: the WC API key lives in a Coolify secret like any other, terminal access is
   gated by Coolify RBAC, and anyone with it has root-equivalent control of the box
   anyway. Fine at the **govt-pilot bar**. (The overlap between "runs Coolify" and
   "scared of a terminal" is small.)
9. **Target discovery — RESOLVED: Coolify API.** Coolify names containers
   `<service>-<uuid>`, so the connector must populate a picker rather than make the
   user type `qdm4k8s-app-1:3000`. Two ways, and they are not close:
   - **Docker socket — rejected.** `docker.sock` is the API to the *whole Docker
     daemon*, not the compose stack: it exposes **every container on the host**
     (other stacks, their DBs, Coolify's own containers). And it is **not
     read-only** — the `:ro` mount flag only freezes the socket *file*, not the API,
     so the connector could still create containers / `exec` / bind-mount host paths
     ≈ **root on the host**. The only way to actually scope it is a
     `docker-socket-proxy` sidecar whitelisting `GET /containers/json`, which erases
     the "simpler to ship" advantage.
   - **Coolify API — chosen.** One scoped API token (a Coolify secret); the
     connector lists only Coolify-managed resources via REST. No host access, small
     blast radius, cleaner story ("lists your Coolify apps" vs. "can see every
     container on your box"). Cost: an extra secret to provision + coupling to
     Coolify's API surface — acceptable at the **govt-pilot bar**.

### Coolify-facing artifact

10. **The template/compose snippet + install docs** are their own deliverable
    (everything above is connector engineering). The thing a Coolify user actually
    touches: a ~15-line compose snippet (app with no domain + connector pointing at
    it, `${WC_API_KEY:?}` prompting in the UI), tested via "Docker Compose Empty."

## Suggested order

1. Connector engineering first: the container image (#4) — reconcile wrapper +
   supervisor + conventions plumbing — which is also where #1 (multi-share) is
   realised, plus a graceful-SIGTERM handler in `serve`. None Coolify-specific, all
   needed under any outcome. (#2 host:port and #3 WebSockets are done.)
2. Both gating design decisions (#8 admin access, #9 target discovery) are settled,
   so engineering can proceed without further blocking decisions.
3. Then the runtime CLI subcommands (#7: invite/members/revoke) and host-header
   handling (#5).
4. Finally the Coolify artifact (#10): publish the image, write + test the compose
   snippet and guide.
5. Long-term distribution: docs-page contribution to Coolify's knowledge base
   (alongside Cloudflare Tunnels), then catalog template once star count allows.

## Gating decisions — resolved

Both decisions that change what the connector *is* are settled: **#8 admin access**
(no custom surface — Coolify env/UI for config, built-in terminal + `waserver` CLI
for invites/members/revoke) and **#9 target discovery** (Coolify API, not the Docker
socket). Everything else is bounded engineering.

## Sources

- [Coolify service template contribution guide](https://coolify.io/docs/get-started/contribute/service)
- [Docker Compose handling & private services](https://coolify.io/docs/knowledge-base/docker/compose)
- [API authorization](https://coolify.io/docs/api-reference/authorization)
- [Coolify OpenAPI spec](https://raw.githubusercontent.com/coollabsio/coolify/v4.x/openapi.json)
- [Cloudflare Tunnels overview](https://coolify.io/docs/knowledge-base/cloudflare/tunnels/overview)
- [Custom template discussion #3002](https://github.com/coollabsio/coolify/discussions/3002)
