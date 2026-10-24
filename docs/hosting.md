# Hosting waserver

Guidelines for putting `waserver` on a machine and keeping it running.

## Getting the binaries

### The install script

```sh
curl -fsSL https://raw.githubusercontent.com/s-te-ch/wispers-access/main/install.sh | sh
```

The script finds the newest release of `waserver` and of `waclient`, verifies
each archive against the release's `SHA256SUMS`, and installs the two binaries
into `/usr/local/bin`, with `sudo` for that one step if the directory is not
yours. Running the script again upgrades whatever is out of date. `waclient`
comes along because it is the quickest way to check a share from the host
itself: `waclient join <code>`, then `waclient serve 8000`.

If you prefer another target directory, use the `--prefix` flag. Add
`sh -s -- --prefix ~/.local/bin` instead of just `sh` after the pipe.

The script supports Linux amd64 and arm64 and macOS on Apple silicon. For
Windows, take the zip from the releases page.

### By hand

The [releases page](https://github.com/s-te-ch/wispers-access/releases) has
one archive per platform, `waserver-<version>-<platform>.tar.gz`, holding the
binary and the licence, and a `SHA256SUMS` file with every archive's
checksum. The Linux binaries run on any distribution with glibc 2.28 or
newer, which is RHEL 8, Debian 10, Ubuntu 18.10 and everything since.
Download the archive for your platform and the checksum file, check the
archive, unpack it, and copy the binary into place. For example:

```sh
release=https://github.com/s-te-ch/wispers-access/releases/download/waserver-v1.0.0
curl -fsSLO $release/waserver-1.0.0-linux-amd64.tar.gz
curl -fsSLO $release/SHA256SUMS
sha256sum -c --ignore-missing SHA256SUMS  # On macOS: shasum -a 256 -c SHA256SUMS 2>/dev/null | grep OK
tar xzf waserver-1.0.0-linux-amd64.tar.gz
sudo install -m 755 waserver-1.0.0-linux-amd64/waserver /usr/local/bin/
```

## Running waserver

### In the foreground, or as your user

| Command                  | Description                                                                 |
|--------------------------|-----------------------------------------------------------------------------|
| `waserver serve <share>` | runs one share in the foreground and logs to stderr                         |
| `waserver start [share]` | runs the share in the background, daemonised. All shares if `share` omitted |
| `waserver stop [share]`  | stops the share, all shares if `share` omitted                              |
| `waserver logs [share]`  | shows the logs of the share, all shares if `share` omitted                  |

### As a systemd service

The template unit at
[`integrations/systemd/waserver@.service`](../integrations/systemd/waserver@.service)
runs one server per share under a dedicated `waserver` user, restarted on
failure and started at boot. The state lives in `/var/lib/waserver`. waserver's
own commands should run as that user too.

To create the user and install the unit:

```sh
sudo useradd --system --create-home --home-dir /var/lib/waserver --shell /usr/sbin/nologin waserver
sudo curl -fsSLo /etc/systemd/system/waserver@.service \
  https://raw.githubusercontent.com/s-te-ch/wispers-access/main/integrations/systemd/waserver@.service
sudo systemctl daemon-reload
```

(On Fedora and friends, the shell is `/sbin/nologin`. The unit expects the
binary in `/usr/local/bin`. Edit `ExecStart` and `ExecReload` if it is
elsewhere.)

Once the service runs, you can create a share and add your apps like so:

```sh
sudo -H -u waserver waserver init team "Awesome Team"
sudo -H -u waserver waserver edit team  # opens share.toml in an editor
```

Then enable and start the service for the share and check its status:

```sh
sudo systemctl enable --now waserver@team
sudo -H -u waserver waserver status team
```

Day to day commands:

- Invite a device: `sudo -H -u waserver waserver invite team "Alice's phone" alice@example.com`,
  as in the [quick start](../README.md#3-invite-a-device)
- Check logs: `journalctl -u waserver@team -f`
- Change the config / add a new app: `sudo -H -u waserver waserver edit team`
  lets you edit the config, then reloads the running server by itself.
  Alternatively, edit the config file directly, then run
  `sudo systemctl reload waserver@team`
- Add a new share: `waserver init` it the same way, then run
  `systemctl enable --now waserver@<share>`

Don't use `waserver start` and `stop` here, systemd owns the processes.

To remove:

```sh
sudo systemctl disable --now 'waserver@*'
sudo rm /etc/systemd/system/waserver@.service
sudo userdel --remove waserver          # deletes /var/lib/waserver, keys included
sudo rm /usr/local/bin/waserver /usr/local/bin/waclient
```

### Containers

waserver is also available in containerised form. The image
`ghcr.io/s-te-ch/wispers/access/waserver` is waserver plus a process supervisor,
built for amd64 and arm64. Tags are `:X.Y.Z` per release, plus `:latest`.

The containerised version fits best in a compose stack next to the app(s) it
shares. waserver reaches the apps by their service names on the stack's network,
so the apps don't have to publish ports at all.

The compose.yaml and team.toml files below are an example for how this works:

```yaml
# compose.yaml
services:
  myapp:
    image: example/myapp:latest

  waserver:
    image: ghcr.io/s-te-ch/wispers/access/waserver:1.0.0
    environment:
      SHARE_ID: team  # The share ID. The container inits it on first start
    volumes:
      - ./team.toml:/config/team.toml   # The share's config, below
      - waserver-state:/data

volumes:
  waserver-state:
```

```toml
# team.toml
name = "Awesome Team"

[[app]]
id = "myapp"
name = "My App"
upstream = "myapp:8080"             # service name and port on the stack's network
```

On its first start, the container creates the share `team` using the mounted
configuration, then serves the share. Subsequent starts find the already
initialised share and simply serve it.

Note that the config has `upstream = "myapp:8080"`, addressing the job in the
same docker compose stack. You can also address a port on the Docker host using
e.g. `upstream = "host.docker.internal:3000"`, although to make that work you'll
have to add `extra_hosts: ["host.docker.internal:host-gateway"]`.

The container can also work without a mounted config, allowing you to `waserver
edit` it dynamically while the container runs.

In the example above we've only used one environment variable. Here's the
complete list:

| variable          | default   | meaning                                                      |
|-------------------|-----------|--------------------------------------------------------------|
| `SHARE_ID`        | `default` | the share's ID                                               |
| `SHARE_NAME`      |           | the share's display name (required, unless a config is mounted, see below) |
| `SHARE_TRANSPORT` | `iroh`    | `iroh` or `wispers-connect`                                  |
| `WC_API_KEY`      |           | API key, for `wispers-connect` only                          |
| `WC_BACKEND`      |           | backend URL override, for `wispers-connect` only             |
| `EDITOR`          |           | the editor `waserver edit` opens; nano (default), vim and mg are in the image, `select-editor` in the shell picks one for good |

waserver's own commands run inside the container, prefixed with
`docker compose exec waserver` (or `docker exec -it <container>` without
compose):

```sh
docker compose up -d
docker compose exec waserver waserver status team
docker compose exec waserver waserver invite team "Alice's phone" alice@example.com
```

If you don't want to add the prefix every time, `docker compose exec waserver
bash` gives you a shell where `waserver` commands work directly.

Day to day:

- Logs: `docker compose logs -f waserver`
- Config change: edit `team.toml` on the host and apply it with `docker
  compose exec waserver waserver reload team`, or restart the container.
  `docker compose exec waserver waserver edit team` does both in one go and
  writes the same file, unless it is mounted read-only (`:ro`).
- Another share: `docker compose exec waserver waserver init …`, then
  restart the container so the supervisor picks it up.
- Upgrade: change the image tag, `docker compose pull`, `docker compose up -d`.
  The state on `/data` carries over.

The [container README](../waserver/docker/README.md) has a runnable example
stack with Excalidraw, the `docker run` equivalent, and how the image is put
together. For Coolify there is a ready recipe under
[integrations/coolify](../integrations/coolify/README.md).

## Apps on several origins

By default, guests open a shared app at an address of its own,
`http://<app>.<share>.wa.localhost:<port>/`, and waserver passes each request on
to the app's `upstream`. This is reverse-proxy mode, and it works for apps that
live on one origin. It also provides the best user experience on the client
side. Unfortunately, it doesn't work for apps with multiple origins, for example
apps that redirect the organisation's SSO login page, or apps that use a
different origin to reach an API (which apps under development frequently do).

For such apps there is forward-proxy mode, which gives the client-side browser
an HTTP CONNECT proxy to talk to. This lets the browser use real URLs, even if
they're internal like `https://sso.corp.internal`, tunneling the requests to
waserver where these URLs make sense. Because the tunneled connection is usually
encrypted (TLS), waserver won't add the `x-wispers-access-user` header in this
mode. The downside is that forward-proxy mode is more difficult to make work
with the various platforms and browsers, so it's not available everywhere. For example, Safari isn't supported.

Let's look at some examples. First, a dev setup with a frontend and an API on
two local ports:

```toml
[[app]]
id = "dev"
name = "Dev"
mode = "forward-proxy"
start_url = "http://localhost:4200/"
proxied_origins = [
  "http://localhost:4200",  # the frontend
  "http://localhost:8080",  # its API
]
```

An intranet app with single sign-on:

```toml
[[app]]
id = "wiki"
name = "Wiki"
mode = "forward-proxy"
start_url = "https://wiki.intranet.example.org/"
proxied_origins = [
  "https://wiki.intranet.example.org",
  "https://sso.intranet.example.org",  # the identity provider
]
```

The identity provider needs no change, since guests use the app's real URLs,
redirects included.

The rules:

- `start_url` is where the app opens. Its origin must be one of
  `proxied_origins`. Guests can reach the listed origins and nothing else on the
  host's network.
- An origin is `scheme://host[:port]`, without a path. waserver resolves the
  host name and connects to it, so `localhost` is the machine waserver runs on
  (of if you're using containers, the container waserver runs in).
- `localhost`, `127.0.0.1` and `[::1]` take `http`. Every other host takes
  `https`, IP addresses included. Not every guest platform can make the other
  combinations work, so `waserver reload` refuses them.
- A forward-proxy app has no `upstream`, and its `kind` stays `web`.
- Several apps of a share may list the same origin, such as the identity
  provider they share.

If you want to make sure this works, `waserver status <share>` shows every
origin of a forward-proxy app and whether waserver can reach it.

On the guest's side, forward-proxy mode means that each share's apps get
separate cookies and storage. The mobile apps keep a webview store per share.
The desktop app opens the apps in a browser profile per share, in the first of
Chrome, Edge, Brave, Chromium and Firefox it finds. On Android, forward-proxy
apps need Android System WebView 123 or newer. Older Wispers Access clients show
the app as needing an update.

### Certificates

Guests' devices have to trust the certificates of the `https` origins. With
certificates from a public CA, or from a CA your guests' devices trust already
(through device management, for example), there is nothing to do. Otherwise,
add your CA's certificate to the share, at the top of `share.toml`, before the
first `[[app]]`:

```toml
name = "Team"
ca_certificates = ['''
-----BEGIN CERTIFICATE-----
MIIB…
-----END CERTIFICATE-----
''']
```

Each entry is one certificate in PEM format (never a key). The client app
detects if its device doesn't trust one of the share's origins and will offer to
install the certificate, with the appropriate steps for the platform the client
app runs on. When the user later removes the share, the client app will remove
the certificate again (or, where it can't do it by itself, prompt the user to do it).

Prefer to use a CA certificate rather than a self-signed server certificate. The
latter work in desktop browsers, but Android and iOS won't trust them. If your
servers don't have certificates from a CA yet, make a small one and a
certificate per server:

```sh
# The CA, valid for ten years. Keep ca.key safe, as it can make certificates
# your guests' devices trust.
openssl ecparam -genkey -name prime256v1 -noout -out ca.key
openssl req -x509 -new -key ca.key -days 3650 -subj "/CN=Example Team CA" -out ca.pem \
  -addext "basicConstraints=critical,CA:TRUE" \
  -addext "keyUsage=critical,keyCertSign,cRLSign"

# A certificate for wiki.intranet.example.org, valid for a year.
openssl ecparam -genkey -name prime256v1 -noout -out wiki.key
openssl req -new -key wiki.key -subj "/CN=wiki.intranet.example.org" -out wiki.csr
cat > wiki.ext <<'EOF'
basicConstraints = critical, CA:FALSE
keyUsage = critical, digitalSignature
extendedKeyUsage = serverAuth
subjectAltName = DNS:wiki.intranet.example.org
EOF
openssl x509 -req -in wiki.csr -CA ca.pem -CAkey ca.key -CAcreateserial -days 365 \
  -extfile wiki.ext -out wiki.pem
```

The server gets `wiki.key` and `wiki.pem`, and `ca.pem` goes into
`ca_certificates`. Apple devices refuse server certificates that are valid for
more than 825 days. A name constraint on the CA wouldn't make it safer to
install, as Android's webviews ignore name constraints.

## Storage

Each share is one directory with two files: `share.toml`, which is yours to
edit, and `state.db`, which is the server's and holds the share's identity keys,
its guests and its invites.

|             | Linux                                | macOS                                                    |
|-------------|--------------------------------------|----------------------------------------------------------|
| Shares      | `~/.config/waserver/shares/<share>/` | `~/Library/Application Support/waserver/shares/<share>/` |
| IPC sockets | `~/.waserver/sockets/`               | `~/.waserver/sockets/`                                   |
| Logs        | `~/.local/state/waserver/<share>/`   | `~/Library/Logs/waserver/<share>/`                       |

Note that for user "waserver" (as created under "As a systemd service"), `~` is
`/var/lib/waserver` by default. In the container it's' `/data`. You can also set
the environment variable `WASERVER_DIR` to tell `waserver` to put all its files
there, as `shares/`, `sockets/` and `logs/`.

## Backups

Backing up the share directories is sufficient. Additionally, you may want to
version control `share.toml`. `state.db` is an SQLite database — copy it while
that share's server is stopped, or take a consistent snapshot with `sqlite3
state.db ".backup state-backup.db"`.

In case things go wrong: Losing `state.db` means losing the share's identity.
You'll have to create a new share and re-invite everybody.

## Network

waserver makes outbound connections only. It talks to guest nodes directly over
UDP where the networks allow it and through a relay otherwise. There is no port
to open or forward and nothing listens on a public address. The apps you share
stay reachable only through the tunnel.

## Upgrading

To upgrade, rerun the install script. Alternatively, replace the binary by hand
and restart the servers: `sudo systemctl restart 'waserver@*'` for the service,
`waserver stop` and `start` otherwise. For the container, change the image tag
and recreate it (see [Containers](#containers)).
