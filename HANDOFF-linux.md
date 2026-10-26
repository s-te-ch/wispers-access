# Handoff: forward-proxy mode on Linux

Temporary file on the `forward-proxy` branch. It goes before the merge to main.

## What this is

Forward-proxy mode lets a share's app span several origins. The guest node's
proxy tunnels the share's declared origins to the host node, which connects to
the real hosts. The desktop app opens such apps in a browser profile of their
own, with the share's proxy port on its command line. If the device doesn't
trust the share's `https` hosts, the app offers the share's CA certificate. On
Linux it can't install it. It saves the certificate to a folder the user picks
and says to import it in the browser's certificate settings.

The design is in the wispers monorepo at `docs/access/forward-proxy-mode.md`,
which this machine may not have. Its "Open" list is where the questions below
come from. The Linux save path has never been tried.

## Rules

- Don't commit or push. The user reviews the diff and commits.
- The user clicks through the UI. You build, run commands to check state, and
  write down what happened.
- Write results into the Results section at the end of this file, as you go.

## The host node

The host node runs on the user's Mac, with a share called "Trust test". It
has one forward-proxy app, whose origins are test TLS servers on the Mac.
Their host names (`*.localtest.me`, `*.lvh.me`) resolve to 127.0.0.1, and the
host node resolves them, so they reach the Mac's servers through the tunnel.
Nothing needs to run on this machine except the desktop app. The Mac runs
`caffeinate` during the session. If the connection to the host node still
drops (`cached connection is dead; redialling`), ask the user to check the
Mac before suspecting the app. On Windows, every drop was the Mac sleeping.

The user gets the invite code from the Claude session on the Mac. Codes work
once, for 24 hours.

The test page is the app's start URL, `https://nc-in.localtest.me:8441/`. It
fetches one URL per case and shows OK or FAIL in a table. Each result is also
logged on the Mac, with the browser's user agent, so the Mac session can read
them too.

| Port | Host | Certificate | macOS result |
|---|---|---|---|
| 8441 | nc-in.localtest.me | from the CA constrained to `localtest.me`, inside its names | OK everywhere |
| 8442 | nc-out.lvh.me | from the same CA, outside its names | FAIL everywhere (Android WebView: OK) |
| 8443 | open.lvh.me | from a second CA the share doesn't list | FAIL |
| 8444 | ss-leaf.localtest.me | self-signed, `CA:FALSE` | FAIL (not installed) |
| 8445 | ss-ca.localtest.me | self-signed, `CA:TRUE` | FAIL (not installed) |
| 8446 | ss-plain.localtest.me | self-signed, no basic constraints | FAIL (not installed) |
| 8447 | open2.lvh.me | not one of the share's origins | FAIL (goes direct, nothing listens here) |

The share lists one certificate, "WA Test CA (localtest.me only)", the
constrained CA. So once it's trusted, only 8441 should turn OK. The app's
origin list also has `https://example.com` (trusted by every device) and
`https://nc-in.localtest.me:8499` (nothing listens, so unreachable).

## Build and run

Needs Rust, Node.js, and WebKitGTK 4.1 plus libayatana-appindicator (Debian and
Ubuntu: `libwebkit2gtk-4.1-dev`, `libayatana-appindicator3-dev`). Then

```sh
git fetch && git checkout forward-proxy && git pull
cd desktop
npm install
npm run tauri dev
```

Dev runs are "Wispers Access (staging)", identifier
`dev.wispers.access.desktop.stg`. Its data directory is
`~/.local/share/dev.wispers.access.desktop.stg`, with the browser profiles
under `profiles/<share>/<browser>`. Dev builds don't register for launch at
login. Don't run `npm run release` or `tauri build`.

The app picks the first browser on the `PATH` of Chrome, Edge, Brave, Chromium
and Firefox (`desktop/src-tauri/src/browsers.rs`). Note which are installed,
and whether they are Snap or Flatpak packages (`snap list`, `flatpak list`,
`readlink -f $(which firefox)`). To try a browser further down the list, start
it by hand with the flags the app uses, pointed at the share's port, with a
throwaway profile directory. For Firefox, copy the share profile's `user.js`.

## What to test

1. **Join.** The user joins with the invite. The share's pane should show a
   card saying the share comes with a certificate of “WA Test CA (localtest.me
   only)”, with a "Save certificate…" button. It only shows when the SDK's
   trust check (`sdk/src/certificates.rs`, `rustls-platform-verifier`) finds
   an untrusted origin. The card doesn't list the origins at this stage. If
   the check fails (no connection to the host node), the card doesn't show
   at all, and the only trace is a `console.warn` in the webview.
2. **Save.** The user saves the certificate. Check the file
   (`openssl x509 -in <file> -noout -subject`) and that the card says where it
   went.
3. **Import and browse.** Open the app from the share's pane, which starts the
   share's browser profile. The certificate has to be imported **in that
   profile**, since Chrome and Firefox keep their own stores on Linux.
   - Chrome or Chromium: Settings → Privacy and security → Security → Manage
     certificates, import as an authority trusted for websites. Find out where
     it lands. Chrome on Linux used to keep user certificates in the user's
     `~/.pki/nssdb`, shared by every profile, newer versions may keep them per
     profile. Check whether the user's own Chrome profile trusts it afterwards
     too.
   - Firefox: Settings → Privacy & Security → Certificates → View
     Certificates → Authorities → Import, trust for websites. This lands in
     the share profile's `cert9.db` only.

   Then note the test page's table, per browser, and compare with the macOS
   column above.
4. **Trust check vs browsers.** After the import, does the card still offer
   to save the certificate? It probably does, since `rustls-platform-verifier`
   on Linux reads the system store (`/etc/ssl/certs`), not the browsers', and
   the app records nothing for a saved certificate. The card rechecks when
   the share's pane is opened again.
   Then try the system store with the user's OK
   (`sudo cp <file> /usr/local/share/ca-certificates/wa-test-ca.crt && sudo
   update-ca-certificates`, or the distribution's equivalent). Note what the
   card says then, and whether either browser trusts it without its own
   import. Undo it afterwards (`sudo rm` of that literal path, then
   `sudo update-ca-certificates --fresh`). This decides what the card should
   tell Linux users.
5. **Snap and Flatpak.** If a browser is a Snap, check whether the share's
   profile starts at all. Snaps can't reach the hidden
   `~/.local/share`, which is accepted for now, but the app's behaviour then
   (error message, silent failure) matters.
6. **Socket owner check.** As a second local user
   (`sudo -u <other user> curl -x http://127.0.0.1:<share port>
   https://example.com/`), the proxy should refuse, and the app's log should
   say "refused a connection from another user", not "whose owner is
   unknown". As the user running the app, the same command should work. The
   share's port is in the app's log, or `ss -ltnp`. This is
   `sdk/src/socket_owner/linux.rs`. Do this for IPv4 and, if the proxy listens
   there, IPv6 (`[::1]`).
7. **Port rebinding.** With the profile open and the test page loaded, quit
   the app from the tray (or by closing the window, on desktops without a
   tray) and start it again. The share's port should stay the same. Reload the
   test page. Wait until the old process is gone (`ss -ltnp` no longer shows
   it) before restarting, since a restart racing the old process's shutdown
   moves the port. Also check that the port isn't left `LISTENING` while the
   browser profile keeps running. On Windows the browser inherited the
   listening socket, which `sdk/src/loopback.rs` now prevents there, and
   Unix sockets are close-on-exec.
8. **Leave.** The user removes the share. The dialog shouldn't mention a
   certificate, since the app installed none. The share's browser profile
   should quit and its directory be gone. A certificate the user imported
   stays where they put it. The share's port stays `LISTENING` until the app
   quits (known, found on Windows, answering 403).
9. **Anything else** that looks broken on Linux (window, tray, fonts, wording,
   paths), in the Results.

## Cleanup

Quit the app, delete `~/.local/share/dev.wispers.access.desktop.stg`, and look
for what the staging app left in the Secret Service (`secret-tool search
--all service dev.wispers.access.desktop.stg`, or Seahorse). Show the user what
you'd delete before deleting it. Remove the test CA from the system store if
it's still there.

## Results

(Write here: distribution and version, desktop environment, browser versions
and packaging, and per step what happened. Include exact error texts. Note
every code change you made and why.)

### Environment

- Ubuntu 24.04.4 LTS (Noble), XFCE 4 on X11. Secret Service: gnome-keyring-daemon
  (`org.freedesktop.secrets` on the session bus). `secret-tool` not installed
  (`libsecret-tools`).
- Google Chrome 151.0.7922.71, Debian package in `/opt/google/chrome`. First on
  the app's list, so it's the browser the app picks.
- Firefox 157.0.1, **Snap** (`/usr/bin/firefox` is the transitional wrapper for
  `/snap/bin/firefox`). No Edge, Brave or Chromium. No Flatpak browsers.
- IPv6 loopback present (`::1/128`), so the proxy should bind both addresses.
- Second local user for the socket owner check: `wisperstest`.
- Toolchain: Node 22.23.3, npm 10.9.9. **rustc 1.92.0 was installed, the
  desktop crate (`desktop/src-tauri/Cargo.toml`) and `sysinfo 0.39.6` need
  1.95.** `cargo run` refused with "rustc 1.92.0 is not supported by the
  following packages". Updated the stable toolchain with `rustup update stable`
  to 1.99.1, after which the build went through. Not a code change, but the
  README's build requirements might want to name the minimum.

### 1. Join

Joined with the invite. Log: `connecting to the host node over iroh`,
`activating`, then one `WARN probe timed out` (not an SDK line, a dependency's)
without visible consequence. The share's pane showed the card "This share's
apps need a certificate" naming “WA Test CA (localtest.me only)” with a "Save
certificate…" button, as designed. At this point only the reverse proxy's
port was bound (`serving shares on wa.localhost:11236`, on `127.0.0.1` and
`[::1]`); the share's forward-proxy port is bound on the first app open.

### 2. Save

The folder picker is the GTK one through `xdg-desktop-portal-gtk`. Picking
`~/Downloads` wrote `~/Downloads/WA Test CA (localtest.me only).crt` (700
bytes, PEM). `openssl x509`: subject and issuer `CN = WA Test CA (localtest.me
only)`, critical name constraints permitting `DNS:localtest.me`. The card
showed the "Saved to /home/mbs/Downloads." line. One log line at the moment of
the dialog, harmless but noisy:

```
WARN Failed to populate properties cache via GetAll: org.freedesktop.DBus.Error.UnknownMethod: Object does not exist at path “/org/freedesktop/portal/desktop/request/1_105/ashpd_bTAcnvjmho”. Property change streams will not produce values.
```

That is `ashpd` (used by `rfd`) talking to the portal, after the request
object was already gone. Nothing to do in our code; it could be filtered in
`init_logging` like `rustls_platform_verifier` if it bothers.

### 3. Import and browse

**Open.** Opening the app bound the share's forward proxy at port 45701 on
`127.0.0.1` and `[::1]`, wrote
`profiles/<share id>/chrome/wispers-access-port` with `45701`, and started
Chrome as

```
/opt/google/chrome/chrome --user-data-dir=~/.local/share/dev.wispers.access.desktop.stg/profiles/<share id>/chrome --proxy-server=127.0.0.1:45701 --proxy-bypass-list=<-loopback> --no-first-run --no-default-browser-check https://nc-in.localtest.me:8441/
```

Before any import Chrome shows "Your connection isn't private" for the test
page, as expected: the test page itself is on 8441, signed by the CA that
isn't trusted yet.

**Chrome import.** Chrome 151's certificate settings are the new
`chrome://certificate-manager` page. Importing the saved file there made the
test page load, with the table as on macOS: 8441 OK, the rest FAIL (the
name-constrained CA doesn't vouch for `nc-out.lvh.me`, so 8442 fails, as it
should).

Where it landed: **per profile**, not in `~/.pki/nssdb`. That directory's three
files are byte-for-byte unchanged (md5 before and after). The CA sits in the
share profile's `Default/ServerCertificate` SQLite database, table
`certificates(sha256hash_hex, der_cert, trust_settings)`, keyed by the CA's
SHA-256 (`c78aa0e1…ac4c`, the fingerprint of the saved file), which is Chrome's
per-profile user-certificate store since the Chrome Root Store moved off NSS.
Consequences:

- The user's own Chrome profile does not get it (the user's own Chrome shows
  no certificate warning for the test URL only because nothing listens there
  without the proxy; see the checks below for a direct answer).
- A throwaway Chrome profile started by hand through the same proxy
  (`--headless=new --user-data-dir=<tmp> --proxy-server=127.0.0.1:45701
  --proxy-bypass-list=<-loopback>`) gets `net::ERR_CERT_AUTHORITY_INVALID` on
  8441. So the import really is per profile.
- The card's wording "import it in your browser's certificate settings" is
  right in spirit, but the user has to do it in the **share's** window, not in
  their everyday Chrome. The card could say so.
- Removing the share deletes the profile directory, and the import with it. So
  on Linux leaving the share does clean up the trust, which the leave dialog
  needn't mention.

**curl through the proxy**, as the app's user, with the saved CA: 8441 → 200,
8442 → handshake failure (expected), `https://example.com/` → 200, on
`127.0.0.1:45701` and `[::1]:45701` alike.

### 5. Snap Firefox (tried by hand, the app picks Chrome here)

Firefox is the Snap. Started by hand as the app would,
`firefox -profile ~/.local/share/dev.wispers.access.desktop.stg/profiles/<share>/firefox-snaptest`
with the app's `user.js` copied in (headless, to keep it off the desktop), it
exits at once with

```
Firefox is already running, but is not responding. To use Firefox, you must first close the existing Firefox process, restart your device, or use a different profile.
```

while no Firefox is running. The profile directory stays empty except for our
`user.js`: the Snap's `home` interface excludes dotfiles and dot-directories,
so the Snap can't read or lock the profile and reports it as locked. The same
command with the profile in `~/Downloads/ff-snaptest` starts fine and
populates the profile (`cert9.db`, `cookies.sqlite`, …). So with Snap Firefox
as the only browser the app would start a process that dies with that
message, and `ProfileManager::start` would report success, since it only
spawns. The user would see a Firefox error dialog with misleading text and
nothing from the app. Options: detect `/snap/bin/firefox` (`readlink -f`) and
say so in the pane, or keep the Firefox profile somewhere visible for Snaps.
Not changed here.

The user's own Chrome: `chrome://certificate-manager` there lists no "WA Test
CA", confirming the import stayed in the share profile.

### 4. Trust check vs browsers

After the import in the share's Chrome profile, the share's pane (reloaded;
with a single share there is nothing to click away to) still shows the card
offering "Save certificate…", as predicted: `rustls-platform-verifier` reads
the system store, not Chrome's per-profile database, and the app records
nothing for a saved certificate.

### 6. Socket owner check

As `wisperstest` (`sudo -u wisperstest curl … -x http://127.0.0.1:45701
https://example.com/`, and the same with `-x 'http://[::1]:45701'`): curl got
no response (HTTP 000) on both. The log says, for both address families,

```
WARN refused a connection from another user share=9d38f3b3-… peer=127.0.0.1:40204
WARN refused a connection from another user share=9d38f3b3-… peer=[::1]:57454
```

so `/proc/net/tcp` and `/proc/net/tcp6` are parsed right; no "whose owner is
unknown". As the app's own user the same requests get 200 on both addresses.

**System store.** `sudo cp … /usr/local/share/ca-certificates/wa-test-ca.crt
&& sudo update-ca-certificates` ("1 added"; `/etc/ssl/certs/wa-test-ca.pem`
is the symlink). Afterwards:

- The card still offers "Save certificate…", with the same text. That is not
  a stale verifier: `rustls-platform-verifier` 0.7 calls
  `rustls_native_certs::load_native_certs()` on every `with_platform_verifier()`,
  and the app builds the config per check. To see the verdicts I added an
  `info` line to `check_certificates` (`desktop/src-tauri/src/certificates.rs`,
  "checked the share's origins", with the untrusted list; see Code changes).
  It shows, with the CA in the system store:

  ```
  checked the share's origins share=trust-test untrusted=["https://nc-out.lvh.me:8442", "https://open.lvh.me:8443", "https://ss-leaf.localtest.me:8444", "https://ss-ca.localtest.me:8445", "https://ss-plain.localtest.me:8446"] unchecked=false
  ```

  So 8441 **is** trusted now (8447 and 8499 are unreachable, so not listed):
  the SDK's check does read `/etc/ssl/certs`. The card shows because this test
  share has five origins the CA can never vouch for, and on Linux the card has
  no "installed" state to switch its text to: `installed` is only ever true for
  certificates the app put in a store, so the Linux card says "Save
  certificate…" for as long as any origin is untrusted, even when the user has
  done everything right. For a normal share (origins all under the CA) the card
  disappears once the CA is in the system store, and never does for an import
  into a browser profile only.
- `curl` through the proxy without `--cacert` gets 200 on 8441 (OpenSSL reads
  the same bundle).
- A fresh headless Chrome profile through the proxy still gets
  `ERR_CERT_AUTHORITY_INVALID`: Chrome 151 on Linux does not pick up CAs from
  `update-ca-certificates`. So for Chrome the only way is the per-profile
  import in the share's window.
- Firefox: see below.

**Dev-server restart (a first port-rebinding run).** The edit above made
`tauri dev` rebuild and restart the app while the share's Chrome profile was
open. The new process bound 45701 again on both addresses, Chrome kept
running, `wispers-access-port` still says 45701.

**Firefox vs system store.** A fresh Snap Firefox profile (in `~/Downloads`,
with the app's `user.js`, so `security.enterprise_roots.enabled` is on)
through the proxy shows the certificate warning for 8441 with the CA in
`/etc/ssl/certs`. So on Linux `enterprise_roots` doesn't reach the OpenSSL
bundle either (at least not from the Snap), and Firefox needs its own import
into the share profile's `cert9.db`, as the handoff expected.

**Summary for the Linux card.** Only the SDK's check (and curl) read the
system store. Neither Chrome 151 nor Snap Firefox 157 does. So "install it
system-wide" is the wrong advice for Linux; the right one is "open the app,
then import the file in that window's certificate settings", once per
browser profile, and the card can't tell afterwards whether that happened.
It stays up for as long as any origin is untrusted by the system store, and
goes away only if the user also puts the CA in the system store, which helps
no browser. Suggestion: on Linux, make the card say that the import goes in
the share's browser window, and consider wording it as a hint rather than a
state, since the check can't observe a browser import.

After `sudo rm` and `update-ca-certificates --fresh` the bundle is back to
121 certificates and the symlink is gone.

**Chrome after the dev-server restart.** The test page still loads with the
same table after the app's automatic restart, through the rebound port.

### 7. Port rebinding

(A first try quit the wrong app: the staging app's tray icon and Wispers
Files' look the same in the XFCE panel, and the user quit Wispers Files. I
started it again. Worth a distinct dev icon or a tooltip.)

Quit from the tray with the test page open in the share's Chrome profile:
the app's process ended and port 45701 went free at once, with Chrome and its
helpers still running. So the browser didn't inherit the listening socket
(close-on-exec works, as expected on Linux). Started again 4 s later, the app
bound 45701 again on `127.0.0.1` and `[::1]`, Chrome kept running, no
"port moved" restart of the profile. At startup the check ran again, now with
the CA out of the system store:

```
checked the share's origins share=trust-test untrusted=["https://nc-in.localtest.me:8441", "https://nc-out.lvh.me:8442", …8443, 8444, 8445, 8446] unchecked=false
```

so 8441 is untrusted again by the system store, as it should be.

### 8. Leave

The dialog said "This device's access will be removed on the host. You'll
need a new invitation code to rejoin." and nothing about a certificate, as
it should. Log: `told the host node we are leaving`. The profiles directory
of the share was deleted (and the Chrome import in it). The share's port
45701 was **closed** on leave, not left listening as on Windows: this branch
releases it (`PerShareForwardProxy::release`), so the handoff's expectation
is outdated, no 403 to see.

**Bug: the share's Chrome kept running** with its profile directory deleted
from under it. `ProfileManager::remove` found no process. Cause: Chrome on
Linux rewrites its argv into one space-separated string (its process title),
so `/proc/<pid>/cmdline` holds a single argument (one NUL, the terminator).
`sysinfo` dutifully reports one argument, and `main_processes` matched
`--user-data-dir=<dir>` as a whole argument and `--type=` as an argument
prefix, which never hits. On Linux the app therefore never saw a running
Chrome profile: no quit on leave, no restart when the port moves, the port
file rewritten on each open. Verified with a scratch program against
`sysinfo 0.39.6`: every Chrome process of the profile shows `args=1`.
Fixed in `profiles.rs` (see Code changes). The orphaned Chrome was killed by
hand. The fix wants a re-run of open and leave with a fresh invite.

### Cleanup notes

Secret Service (listed with Python `secretstorage`, as `secret-tool` isn't
installed): after the leave the staging app has one item left,
`client/browser_pairing_secret` for service `dev.wispers.access.desktop.stg`;
the share's `root_key` and `registration` were removed on leave. (The
production app and Wispers Files have their own items; untouched.)

## Code changes

1. `desktop/src-tauri/src/certificates.rs`: one `tracing::info!` in
   `check_certificates` listing the untrusted origins and the `unchecked`
   flag ("checked the share's origins"). The card can't show the verdicts on
   Linux, and without this line nothing says what the check found.
2. `desktop/src-tauri/src/profiles.rs`: `main_processes` now matches the
   profile argument (and `--type=`, `-profile <dir>`, `-contentproc`) at
   space boundaries in the joined command line, through `has_arguments` and
   `has_argument_prefix`, so that Chromium's single-string argv on Linux is
   recognised. Same results for a normally split argv; a profile directory
   with a space in its name isn't recognised in the single-string form,
   which Chromium's rewrite makes ambiguous anyway. Two unit tests. Not yet
   exercised against the app (needs the re-run above).

### 8b. Open and leave again, with the fix

Fresh invite, join, open the app (Chrome started), remove the share: Chrome's
window closed by itself and the share's profile directory was gone. Log as
before. Fix confirmed. (The old share directory from the first cycle held
four files again, `Local State`, `Variations`, `Default/Preferences` and a
temp file, written at the moment I killed the orphaned Chrome by hand after
the first leave: Chrome flushes its state on SIGTERM into the directory the
app had already deleted. The second cycle used a new share directory, so it
didn't touch those. Nothing to fix, but a profile that dies while the app
is deleting its directory can leave such remnants.)

### Cleanup done

App and dev server stopped. Deleted, with the user's OK:
`~/.local/share/dev.wispers.access.desktop.stg`, the Secret Service item
`client/browser_pairing_secret@dev.wispers.access.desktop.stg` (no staging
items left), `~/Downloads/ff-snaptest`, and the saved
`~/Downloads/WA Test CA (localtest.me only).crt`. The test CA was already
out of the system store (`update-ca-certificates --fresh`). The Rust
toolchain stays at 1.99.1. Wispers Files (production) is running again.

### Open points for the design doc

- Linux card wording: the import goes into the share's browser window, per
  profile; the system store helps neither Chrome nor Snap Firefox; the card
  can't observe a browser import, so it stays up for untrusted origins.
- Snap Firefox can't open a profile under `~/.local/share`; detect or relocate.
- Dev and production tray icons are hard to tell apart.
- `ashpd` portal warning in the log after the folder picker, cosmetic.
