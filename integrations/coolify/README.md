# Wispers Access on Coolify

A thin recipe over the generic [`waserver` container](../../waserver/docker/).
Coolify is "docker compose + a UI," so this is just the cross-stack pattern —
one `waserver` container joining Coolify's shared `coolify` network to front
private apps — with Coolify's specific gestures named.

## Steps

1. **Deploy the container.** New Resource → *Docker Compose Empty* → paste
   [`compose.yaml`](./compose.yaml). In the **Environment Variables** tab,
   set `SHARE_NAME` to the share's name as guests see it. **Wispers Connect
   only:** set `SHARE_TRANSPORT` to `wispers-connect` and `WC_API_KEY` to
   your Wispers Connect API key (`WC_BACKEND` is optional, for a self-hosted
   hub). An iroh share needs neither. Deploy.
2. **Give each fronted app a stable network name.** Coolify renames
   application containers on every redeploy (`<uuid>-<timestamp>`), so you
   can't use the container name. Instead:
   - *Application* (git / Dockerfile / Docker Image): set **Custom Network
     Aliases** (Network settings) to a short name, e.g. `odoo`.
   - *Compose resource / one-click service*: the compose **service name**
     already is a stable alias. Nothing for you to do.
3. **Expose the apps privately.** On each app you want reachable, enable
   **"Connect To Predefined Network"** so it joins the `coolify` network. Give
   it **no domain and no published ports** — it stays off the public
   Internet, reachable only through Wispers.
4. **Add the apps to the share.** Open the resource's **Terminal**. It greets
   you with the share's state and the commands that matter. Run
   ```
   waserver edit default
   ```
   and add one block per app, `<alias>` being the stable name from step 2:
   ```toml
   [[app]]
   id = "odoo"
   name = "Odoo"
   upstream = "odoo:8069"
   ```
   Save and close; the server reloads with it.
5. **Hand out access.** From the same Terminal:
   ```
   waserver invite default "Alice's phone" alice@example.com   # prints an invite code / QR
   waserver status default                                     # share detail incl. guests
   waserver revoke default <number>                            # cut one off
   ```

## Why this isn't Coolify-specific

The only Coolify token in `compose.yaml` is the **network name** `coolify`.
Swap it for any shared external network and the same setup works on plain
multi-stack docker, Nomad, etc. Adding another platform is a sibling folder
under `integrations/`, not a change to the container.

## Notes

- **Image:** `ghcr.io/s-te-ch/wispers/access/waserver` (multi-arch: amd64 + arm64),
  public, no registry credentials needed.
- **Changing a share:** `waserver edit default` in the Terminal, any time. The
  config lives on the resource's volume, so redeploys and image updates keep
  it, along with the share's identity and guests.
- **Adding an app:** another `[[app]]` block in that file, plus steps 2 and
  3 for the app. No new container. A second share is `waserver init <id>
  "<name>"` in the Terminal and a restart of the resource.
- **Redeploys are safe:** the alias (or service name) survives app redeploys, so
  the share keeps working without touching this resource. Verified against
  Coolify 4.3.23.
