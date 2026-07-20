# Wispers Access on Coolify

A thin recipe over the generic [`waserver` container](../../waserver/docker/). Coolify
is "docker compose + a UI," so this is just the cross-stack pattern — one `waserver`
container joining Coolify's shared `coolify` network to front private apps — with
Coolify's specific gestures named.

## Steps

1. **Deploy the container.** New Resource → *Docker Compose Empty* → paste
   [`compose.yaml`](./compose.yaml) → Deploy.
2. **Give each fronted app a stable network name.** Coolify renames application
   containers on every redeploy (`<uuid>-<timestamp>`), so you can't use the container neame. Instead:
   - *Application* (git / Dockerfile / Docker Image): set **Custom Network
     Aliases** (Network settings) to a short name, e.g. `odoo`.
   - *Compose resource / one-click service*: the compose
     **service name** already is a stable alias. Nothing for you to do. 
3. **Set two env vars** (Environment Variables tab):
   - `WC_API_KEY` — your Wispers Connect API key.
   - `SHARES` — one or more shares, `;`-separated, each
     `name | display | <alias>:<port>`, with `<alias>` the stable name from
     step 2.
4. **Expose the apps privately.** On each app you want reachable, enable
   **"Connect To Predefined Network"** so it joins the `coolify` network. Give it **no
   domain and no published ports** — it stays off the public Internet, reachable only
   through Wispers.
5. **Hand out access.** From this resource's **Terminal**:
   ```
   waserver invite <share> <device-name> <user@email>   # prints an invite code / QR
   waserver status <share>                               # share detail incl. guests
   waserver revoke <share> <node>                        # cut one off
   ```

## Why this isn't Coolify-specific

The only Coolify token in `compose.yaml` is the **network name** `coolify`. Swap it for
any shared external network and the same file works on plain multi-stack docker, Nomad,
etc. Adding another platform is a sibling folder under `integrations/`, not a change to
the container.

## Notes

- **Image:** `ghcr.io/s-te-ch/wispers/access/waserver` — publish it before pointing
  Coolify at it (Coolify pulls the image).
- **Adding an app:** append `; name | display | alias:port` to `SHARES`, tick the
  app's "Connect To Predefined Network", redeploy. No new container.
- **Redeploys are safe:** the alias (or service name) survives app redeploys, so
  the share keeps working without touching this resource. Verified against
  Coolify 4.1.2.
