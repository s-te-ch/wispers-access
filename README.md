<center>
  <img src="assets/access-logo-txt.svg"
	   width="256" alt="Wispers Access logo"/>
</center>

## About Wispers Access

Wispers Access makes it easy to share a web app with your coworkers or friends
without having to publish it to the internet.

This is most useful for your internal apps, for example your team's ERP
software, that vibe-coded app your team finds useful, or your private photo
archive at home. Until now, sharing these apps usually meant putting them on the
internet. If you're self-hosting, that's hard to do securely. If you put the app
on the cloud, you give your cloud provider access to your internal data.
Installing a VPN helps, but smartphone support is often limited, and if you have
multiple VPNs (like your company and home VPNs) they'll clash.

Wispers Access solves this by creating a secure peer-to-peer overlay network for
each shared app using the [Wispers Connect](https://connect.wispers.dev)
library. Internally, the secure connections work like VPN connections, but
because they're at the application level, they never clash, not even with your
existing VPN. Wispers also aims to be as cloud-independent as possible.
Peer-to-peer connection establishment still requires a cloud-hosted rendezvous
server, but that server is cryptographically unable to eavesdrop on your traffic
or to inject malicious nodes. Mesh VPNs do not offer this.

## Project status

Wispers Access is currently in open beta. The Android client is in open testing
([Play Store](https://play.google.com/store/apps/details?id=dev.wispers.access.android)).
The iOS client is in open testing
([TestFlight](https://testflight.apple.com/join/AjsJChhq)).

## Quick start

To get a first taste without installing anything on a server, grab a live invite
code at [access-demo.wispers.dev](https://access-demo.wispers.dev) and jump
straight to [step 5](#5-open-it-on-a-phone).

But you'll really want to share your _own_ app. To do this, you need `waserver`
on a machine next to the app you want to share, and a client on each guest
device.

### 1. Get an API key

Peer-to-peer rendezvous runs through a Wispers Connect hub, and that needs an
API key: sign up at [connect.wispers.dev](https://connect.wispers.dev), open the
**Default** domain, and create an API key there (for self-hosting see below).

### 2. Install waserver

Download the tarball for your platform from the
[releases page](https://github.com/s-te-ch/wispers-access/releases) and put
`waserver` on your `PATH`. Prefer containers? There's an image at
`ghcr.io/s-te-ch/wispers/access/waserver` — see
[waserver/docker](waserver/docker/README.md).

### 3. Share your app

Say the app you want to share is listening on port 3000. Then,

```sh
export WC_API_KEY=…            # the key from step 1
waserver init myapp "My App"
waserver serve myapp 3000      # `waserver start` runs it in the background instead
```

### 4. Invite a device

Invites are minted by the running server from step 3, so run this in a second
terminal (or use `waserver start`):

```sh
waserver invite myapp "Alice's phone" alice@example.com --png invite.png
```

This produces the invite in three different formats: a `wax_…` invite code to
copy-paste, an ASCII art QR code to scan with the phone, and the same QR code as
a PNG. The user ID (`alice@example.com`) is a label you choose; the shared app
sees it on every request in the `x-wispers-access-user` header, so it knows who
is connected without needing its own login.

### 5. Open it on a phone

Install Wispers Access from the
[Play Store](https://play.google.com/store/apps/details?id=dev.wispers.access.android)
(Android) or via
[TestFlight](https://testflight.apple.com/join/AjsJChhq) (iOS), tap **+**, and
scan the QR code (or copy-paste the
invite code). The shared app opens right inside Wispers Access: no VPN
profile, no open port on the internet.

### … or on a desktop

`waclient` (same releases page) serves every share you've joined on localhost:

```sh
waclient join wax_…            # the invite code from step 4
waclient serve 8000
```

It prints each share's URL, e.g. `http://my-app.localhost:8000` — open it in
your normal browser.

---

**Self-hosting:** Shares use the managed Wispers Connect backend by default. To
be fully cloud-independent, [run your own hub](https://github.com/s-te-ch/wispers-hub)
and pass `--backend https://hub.example.com` to `waserver init`. Invite codes
contain the backend, so guests land on the right hub automatically.

## Licensing

Everything you need to run Wispers Access is open source:

- **Wispers Access** (this repo) is [MIT licensed](LICENSE), as is the
  underlying [wispers-connect](https://github.com/s-te-ch/wispers-client)
  library.
- **The standalone hub**, [wispers-hub](https://github.com/s-te-ch/wispers-hub),
  is AGPL-3.0.

The managed Wispers Connect backend at
[connect.wispers.dev](https://connect.wispers.dev) offers a free personal tier.
