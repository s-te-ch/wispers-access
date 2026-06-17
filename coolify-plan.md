# Wispers Access × Coolify integration plan

> Status: research + scoping notes. Nothing built yet.

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

1. **Serve all shares from one daemon.** Multi-share in a single process. Today
   it's one share per daemon. Includes a defined volume layout for multi-share
   state (`share_config.json`, `root_key.bin`, `registration.pb` per share) and
   share **deletion**.
2. **Address `host:port`, not just port.** Upstream is hardcoded to `127.0.0.1`
   (`waserver/src/http.rs:57`); `strip_hop_by_hop` also drops the target. In a
   compose stack the app is at a Docker DNS name like `app:3000`. Needed regardless.
3. **WebSocket support — hard gap, not nice-to-have.** `try_forward` does a plain
   HTTP/1 request/response and strips the `Upgrade` header
   (`waserver/src/http.rs:128`); there's no `hyper::upgrade` tunnel path. A large
   share of self-hosted apps (dashboards, chat, Gitea live updates) break without
   it. Peer of item #2 in priority.
4. **Container-conventions plumbing** (small individually, all required):
   - non-interactive, env-driven init (`WC_API_KEY`, share name, target), idempotent
     across restarts (init only if state volume empty)
   - log to **stdout** instead of today's daily-rotated files (Coolify's log viewer
     reads stdout)
   - graceful shutdown on SIGTERM
   - a healthcheck
   - multi-arch build (**arm64** — home-lab / Pi users are core Coolify audience)
   - run in **foreground**, not the daemonize path
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

1. Connector engineering first: items #1–#4 (multi-share, host:port, WebSockets,
   container plumbing) — none Coolify-specific, all needed under any outcome.
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
