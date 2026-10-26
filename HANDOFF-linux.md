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
Nothing needs to run on this machine except the desktop app. The Mac has to
stay awake for the whole session.

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
   only)”, with a "Save certificate…" button, and list the origins the device
   doesn't trust. Write down that list. It is the SDK's trust check
   (`sdk/src/certificates.rs`, `rustls-platform-verifier`).
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
4. **Trust check vs browsers.** After the import, does the card still say the
   origins are untrusted? It probably does, since `rustls-platform-verifier`
   on Linux reads the system store (`/etc/ssl/certs`), not the browsers'.
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
   test page.
8. **Leave.** The user removes the share. The dialog shouldn't mention a
   certificate, since the app installed none. The share's browser profile
   should quit and its directory be gone. A certificate the user imported
   stays where they put it.
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
