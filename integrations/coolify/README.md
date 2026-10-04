# Wispers Access on Coolify

A thin recipe over the generic [`waserver` container](../../waserver/docker/).
Coolify is "docker compose + a UI," so this is just the cross-stack pattern —
one `waserver` container joining Coolify's shared `coolify` network to front
private apps — with Coolify's specific gestures named.

## Steps

1. **Deploy the container.** New Resource → *Docker Compose Empty* → paste
   [`compose.yaml`](./compose.yaml) → Deploy.
2. **Give each fronted app a stable network name.** Coolify renames application
   containers on every redeploy (`<uuid>-<timestamp>`), so you can't use the container name. Instead:
   - *Application* (git / Dockerfile / Docker Image): set **Custom Network
     Aliases** (Network settings) to a short name, e.g. `odoo`.
   - *Compose resource / one-click service*: the compose
     **service name** already is a stable alias. Nothing for you to do. 
3. **Edit the share config** in the compose file's `content:` block: the share's
   name as guests see it, and one `[[app]]` per app with
   `upstream = "<alias>:<port>"`, `<alias>` being the stable name from step 2.
   On the first deploy Coolify copies the block into a file under the
   resource's **Storage** tab and mounts it at `/config/team.toml`; the file
   name is the share's id. Another share is another such volume entry.
4. **Wispers Connect only:** a share with `kind = "wispers-connect"` instead
   of iroh needs the env var `WC_API_KEY` (Environment Variables tab), your
   Wispers Connect API key. `WC_BACKEND` is optional, for a self-hosted hub.
   An iroh share needs neither.
5. **Expose the apps privately.** On each app you want reachable, enable
   **"Connect To Predefined Network"** so it joins the `coolify` network. Give it **no
   domain and no published ports** — it stays off the public Internet, reachable only
   through Wispers.
6. **Hand out access.** From this resource's **Terminal**:
   ```
   waserver invite <share> <device-name> <user@email>   # prints an invite code / QR
   waserver status <share>                               # share detail incl. guests
   waserver revoke <share> <node>                        # cut one off
   ```

## Why this isn't Coolify-specific

The Coolify tokens in `compose.yaml` are the **network name** `coolify` and the
`content:` key on the file mount, Coolify's way of creating a mounted file from
the compose file. Swap the network for any shared external network and put the
TOML in a real file, and the same setup works on plain multi-stack docker, Nomad,
etc. Adding another platform is a sibling folder under `integrations/`, not a change to
the container.

## Notes

- **Image:** `ghcr.io/s-te-ch/wispers/access/waserver` (multi-arch: amd64 + arm64),
  public, no registry credentials needed.
- **Changing a share:** edit its file under the resource's **Storage** tab, then
  restart the resource. Coolify reads the compose file's `content:` block only
  on the first deploy; after that the file is the source and edits to the block
  are ignored. The container rewrites the share's config from the file on every
  start, and the share's identity and guests survive the restart.
- **Adding an app:** add an `[[app]]` block to that file, tick the app's
  "Connect To Predefined Network", restart. No new container. A second share
  is a second file mount at `/config/<name>.toml`.
- **Redeploys are safe:** the alias (or service name) survives app redeploys, so
  the share keeps working without touching this resource. Verified against
  Coolify 4.3.23.
