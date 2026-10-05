# Wispers Access on Coolify

This is a recipe for getting Wispers Access to work with Coolify, using the
generic [`waserver` container](../../waserver/docker/).

## Steps

1. **Deploy the container.** New Resource → *Docker Compose Empty* → paste
   [`compose.yaml`](./compose.yaml). In the **Environment Variables** tab, set
   `SHARE_NAME` to the name you want guests to see. By default, this uses the
   `iroh` transport. If setting `SHARE_TRANSPORT` to `wispers-connect`, also set
   `WC_API_KEY` to your Wispers Connect API key (and optionally `WC_BACKEND`).
   Deploy.
2. **Give each shared app a stable network name.** Unfortunately, Coolify
   renames application containers on every redeploy (`<uuid>-<timestamp>`), so
   you can't use the container name. Instead:
   - *Application* (git / Dockerfile / Docker Image): Set **Custom Network
     Aliases** (Network settings) to a short name, e.g. `odoo`.
   - *Compose resource / one-click service*: Nothing for you to do. The compose
     **service name** already is a stable alias.
3. **Expose the apps privately.** On each app you want to share, enable
   **"Connect To Predefined Network"** so it joins the `coolify` network. Give
   it **no domain and no published ports** — it stays off the public Internet,
   reachable only through Wispers.
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
   ```
