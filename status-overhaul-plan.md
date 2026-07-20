# waserver status overhaul — strawman

Status: implemented 2026-07-20 (phases 1–4 done; hosted backend deployed to
staging + prod; standalone-hub release for self-hosters still to cut)

## Motivation

The expired-invite episode of 2026-07-18 (registration tokens silently minted
with the 5-minute "interactive" TTL) took a psql session against the backend to
diagnose. The user-facing tools should have answered it. Today:

- `waserver status` is a one-line-per-share liveness check over IPC.
- `waserver nodes <share>` is a hub query that restores the node in-process —
  while the daemon holds the same identity.
- The Coolify entrypoint scrapes `waserver status` with `awk '{print $1}'`,
  hardwiring the human output format.
- Useful data goes unsurfaced: connectivity group ID, backend URL, user IDs,
  token expiry all sit in `ShareConfig` and the integrator API's `GroupDetail`.

The server-status / share-status separation of concerns is right; the commands
just don't reflect it, and there is no machine-readable output.

## Command surface

Keep two forms of one command; the split is *scope*, not implementation:

### `waserver status` — fleet overview

One row per share on this machine: daemon state, hub connection, upstream,
member count. Answers "what's running and is it healthy".

```
SHARE     STATE     HUB        UPSTREAM         NODES
myapp     serving   connected  localhost:3000   2/3 online
blog      offline   -          -                ?
```

### `waserver status <share>` — share deep-dive

Multi-section detail. Absorbs `nodes`, which is removed outright (no users to
migrate yet) — it was always a slice of share status.

```
Share
  Name                myapp (My App)
  Backend             managed (connect.wispers.dev)
  Connectivity group  c22459ee-04ce-4320-ae8a-59f48948d896
  Created             2026-05-02 14:11 UTC

Server
  State               serving (pid 4711, up 2d 3h)
  Upstream            localhost:3000 (reachable)
  Hub                 connected

Members
  #  NAME             USER                    LAST SEEN     STATUS
  1  Server           -                       now           online
  2  Lara's Android   <redacted>   2 min ago     online
  3  Old laptop       mbs@s-te.ch             12 days ago   offline

Invites
  NODE NAME        USER                CREATED    STATUS
  Nick's iPhone    nick@example.com    1 h ago    pending (expires in 23 h)
  Lara's Android   <redacted>@…       2 h ago    used
  Lara's Android   <redacted>@…       4 h ago    expired
```

The "Share" section is the connect.wispers.dev breadcrumb: group ID + backend
host is enough to find the entry in the console. Print a full deep link once
the console has stable URLs.

"Share", "Members", and "Invites" come from disk + the integrator REST API, so
they work **even when the daemon is down** — only the "Server" section degrades
to `offline`. This also fixes a latent wart: today `nodes` calls
`restore_or_init_node` in the CLI process while the daemon holds the same
identity; going through the REST API with the stored key avoids that entirely.

## Invite status: one status per invite, with history

An invite bundles two credentials (backend registration token + hub activation
code). We deliberately do **not** surface them separately:

- Since the TTL fix, both halves live 24 h; registration-token expiry is the
  binding constraint, and one authoritative expiry per invite is less confusing
  than two.
- "Registered but not activated" is never a steady state: clients fully roll
  back a failed join, so a half-joined node deregisters itself again.

The rollback does leave one observable residue: the registration token stays
consumed (`used_at` set) while the node row is gone. The same invite can never
be redeemed again. Rendering makes this diagnosable at a glance: an invite
showing `used` with **no matching member** in the Members section means "join
rolled back — issue a new invite".

Derived status per invite: `pending (expires in …)` / `used` / `expired`.

Consequence: the listing must include *recent* invites regardless of state
(e.g. last 7 days), not just live ones, so the CLI renders history rather than
a filtered view. This morning's episode would have read as `expired` next to
the first code and `used` next to the second.

Revisit the single-status decision only if the two halves' lifetimes diverge
again.

## Machine-readable output

A `--json` flag on both `status` forms: one JSON document to stdout, no ANSI,
nothing else. (`invite --json` is a natural follow-up but out of scope for
now — see the resolved questions.)

Principles:

- The human format is explicitly **not** a stable interface; the JSON is.
- camelCase keys mirroring the integrator API wire names where they overlap
  (`connectivityGroupId`, `expiresAt` as RFC 3339, …).
- Fields we couldn't determine are `null` — a dead daemon yields
  `"server": {"state": "offline"}` with share/member data still populated.
- Exit code 0 whenever the query itself succeeded, even if things are offline;
  scripts branch on content, not exit code.

```json
{
  "shares": [{
    "name": "myapp",
    "displayName": "My App",
    "backend": null,
    "connectivityGroupId": "c22459ee-…",
    "server": { "state": "serving", "upstream": "localhost:3000", "hubConnected": true },
    "members": [{
      "nodeNumber": 2, "name": "Lara's Android",
      "userId": "<redacted>",
      "isOnline": true, "lastSeenAt": "2026-07-18T09:41:00Z"
    }],
    "invites": [{
      "nodeName": "Nick's iPhone", "userId": "nick@example.com",
      "createdAt": "…", "expiresAt": "…", "usedAt": null,
      "status": "pending"
    }]
  }]
}
```

### Coolify integration

The entrypoint's `awk` scrape becomes
`waserver status --json | jq -r '.shares[].name'` (jq added to the image), and
the entrypoint stops caring about human formatting. (A future `invite --json`
could similarly give Coolify/scripts the code, PNG path, and expiry as data.)

## Dependencies outside waserver

1. **List registration tokens** — `GET /connectivity-groups/:id/registration-tokens`
   does not exist in the hub (only POST). Add to `integratorapi.go` + both REST
   implementations (hosted + standalone), returning `nodeName`, `userId` (from
   metadata), `createdAt`, `expiresAt`, `usedAt` for recent tokens (all states,
   e.g. last 7 days). Token hashes never leave the server.
2. **Activation-code status** — not surfaced (see invite-status section);
   requires no backend work.
3. **IPC `StatusData` enrichment** — pid / uptime / connected-since are cheap
   additions to the existing `Status` request; no protocol break (JSON over a
   private socket).

## Implementation phases

The work spans three codebases: **wispers-access** (waserver), the **wispers
monorepo** (`connect/be` hosted backend + `connect/storage` sqlc queries +
`oss/connect/hub` standalone hub & shared wire types), and the **wispers-hub**
GitHub repo (read-only copybara export of `oss/connect/hub` — never edited
directly; changes flow out via `tools/export/export_hub.sh`).

Key scoping fact: `GET /connectivity-groups/:id` (GroupDetail with nodes,
`isOnline`, `lastSeenAt`, `createdAt`) already exists in both API
implementations. So the fleet NODES column and the whole Members section need
**no backend work** — only the invite listing does. That makes Phase 1
self-contained in waserver and shippable immediately.

Ordering: 1 ∥ 2 (independent), 3 needs both, 4 needs only 1.

### Phase 1 — waserver status rework (wispers-access only) — DONE

Everything except the Invites section, against APIs that exist today.

1. **wcbe.rs**: add `get_connectivity_group(cg_id) -> GroupDetail`
   (deserialize `id`, `createdAt`, `name`, `nodes[]` incl. `metadata`,
   `isOnline`, `lastSeenAt`).
2. **ipc.rs**: extend `StatusData` with `pid`, `started_at`, and hub
   `connected_since`. New fields are `Option` + `#[serde(default)]` so a new
   CLI still parses an old running daemon's reply (and vice versa).
3. **main.rs `status` (fleet)**: table SHARE / STATE / HUB / UPSTREAM / NODES.
   IPC per share for STATE/HUB/UPSTREAM; REST GroupDetail for the online/total
   count. Query shares concurrently; REST failure renders `?`, never an error.
4. **main.rs `status <share>` (deep-dive)**: Share section from `ShareConfig` +
   GroupDetail; Server section from IPC (state, pid, uptime) plus the upstream
   reachability probe (TCP connect, ~1 s timeout; upstream comes from IPC, so
   the probe only runs when the daemon is up); Members section from
   GroupDetail (parse `userId` out of node metadata). Invites section: not yet
   — render nothing in Phase 1.
5. **`nodes <share>`**: remove the command entirely (no users to migrate).
   This kills the `restore_or_init_node`-in-CLI wart immediately.
6. **`--json`** on both `status` forms per the schema above (`invites` field
   `null` for now).

Verify: `cargo build -p waserver` + unit tests for status derivation/JSON
shape; manual run against a live share and against a stopped daemon (Share +
Members must still render).

### Phase 2 — backend: list registration tokens (wispers monorepo) — DONE
(deployed to staging + prod; standalone-hub release still to cut)

`GET /connectivity-groups/:id/registration-tokens`, both implementations.
TDD per `connect/AGENTS.md` (tests first).

1. **Shared wire types** (`oss/connect/hub/connect/shared/integratorapi.go`):
   `RegistrationTokenInfo { nodeName, nodeMetadata, createdAt, expiresAt,
   usedAt }` + a list response wrapper. No token or hash field exists in the
   type — unleakable by construction.
2. **Selection semantics** (shared, so both backends agree): tokens that are
   pending (unexpired, unused) **or** created in the last 7 days; newest
   first.
3. **Standalone hub** (`oss/connect/hub/connect/hub/standalone_rest.go`):
   route + handler + sqlite query over `node_registration_tokens`
   (`*_millis` columns → RFC 3339); cases in `standalone_test.go`.
4. **Hosted backend** (`connect/be/integratorapi.go`): new query in
   `connect/storage/queries/integrator/queries.sql` + `sqlc` regenerate,
   handler, route registration incl. the route-description table; coverage in
   `integrator_routes_integration_test.go`. RLS scopes it to the caller's
   domain for free.
5. **Rollout**: deploy hosted backend to connect.wispers.dev; export via
   `tools/export/export_hub.sh` and cut a standalone-hub release for
   self-hosters.

### Phase 3 — waserver Invites section (needs 1 + 2) — DONE

1. **wcbe.rs**: `list_registration_tokens(cg_id)`.
2. **`status <share>`**: render the Invites section; derive
   `pending (expires in …)` / `used` / `expired`; populate `invites` in the
   JSON.
3. **Graceful degrade**: any listing failure renders as
   `Invites: (unavailable: …)` with JSON `invites: null` + `invitesError` —
   never failing the command. (An explicit old-backend detection existed
   briefly but was dropped once all backends were updated.)
4. Verify against a local standalone hub (old build → degrade path, new build
   → history incl. a rolled-back join showing `used` with no member).

### Phase 4 — Coolify integration + cleanup (needs 1) — DONE

1. **docker/Dockerfile**: add `jq` to the runtime image.
2. **docker/entrypoint.sh**: replace the `awk '{print $1}'` scrape with
   `waserver status --json | jq -r '.shares[].name'`.
3. Docs: README command reference.
4. Deferred (out of scope for now, matching the `--json` decision below):
   `invite --json`.

### Compatibility matrix

- New CLI ↔ old daemon (or old CLI ↔ new daemon): fine — `StatusData`
  additions are optional JSON fields.
- New waserver ↔ old backend/hub: everything works except Invites, which
  degrades to `(unavailable: …)` without failing the command (Phase 3.3).
- Old waserver ↔ new backend: untouched paths only.

## Open questions (resolved)

- Is `status <share>` the right name for the deep-dive, or `info <share>` with
  `status` purely fleet-level? Leaning toward one verb with optional arg —
  fewer commands, and the scope split stays visible as "no arg = machine,
  arg = share".
  => Decision: yes, keep "status" as suggested
- Should `status` probe upstream reachability (a TCP connect to `host:port`)?
  It's the single most common "why isn't this working" answer, so leaning yes.
  => Decision:yes
- `--json` per command vs. a global `-o json`? Per-command is less clap
  plumbing; global is more conventional. Cosmetic either way.
  => Decision: per command (right now --json is limited to "status" anyway)
