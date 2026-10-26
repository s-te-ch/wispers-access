# Handoff: forward-proxy mode on Windows

Temporary file on the `forward-proxy` branch. It goes before the merge to main.

## What this is

Forward-proxy mode lets a share's app span several origins. The guest node's
proxy tunnels the share's declared origins to the host node, which connects to
the real hosts. The desktop app opens such apps in a browser profile of their
own, with the share's proxy port on its command line. If the device doesn't
trust the share's `https` hosts, the app offers the share's CA certificate and
installs it in the user's root store. It removes it again when the user leaves
the share.

The design is in the wispers monorepo at `docs/access/forward-proxy-mode.md`,
which this machine may not have. Its "Open" list is where the questions below
come from. The desktop app has **never run on Windows**. Only `cargo check`
has, in CI. Expect build or runtime problems, and fix them as you go.

## Rules

- Don't commit or push. The user reviews the diff and commits.
- The user clicks through the UI and the system dialogs. You build, run
  commands to check state, and write down what happened.
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
constrained CA. So after installing it, only 8441 should turn OK. The app's
origin list also has `https://example.com` (trusted by every device) and
`https://nc-in.localtest.me:8499` (nothing listens, so unreachable).

## Build and run

Needs Rust with the MSVC toolchain, the Visual Studio C++ build tools, Node.js,
and WebView2 (part of Windows 11). Then

```powershell
git fetch; git checkout forward-proxy; git pull
cd desktop
npm install
npm run tauri dev
```

Dev runs are "Wispers Access (staging)", identifier
`dev.wispers.access.desktop.stg`. Its data directory is
`%APPDATA%\dev.wispers.access.desktop.stg`, with the browser profiles under
`profiles\<share>\<browser>`. Dev builds don't register for launch at login.
Don't run `npm run release` or `tauri build`.

Have the user install whichever of Chrome, Edge and Firefox are missing. The
app picks the first it finds of Chrome, Edge, Brave, Chromium and Firefox. To
try a browser further down the list, start it by hand with the flags the app
uses (`desktop/src-tauri/src/browsers.rs`), pointed at the share's port, with a
throwaway profile directory. For Firefox, copy the share profile's `user.js`.

## What to test

1. **Join.** The user joins with the invite. The share's pane should show a
   card saying the share comes with a certificate of “WA Test CA (localtest.me
   only)”, with an Install button, and list the origins the device doesn't
   trust. Write down that list. It is the SDK's trust check
   (`sdk/src/certificates.rs`, `rustls-platform-verifier`).
2. **Install.** The user clicks Install. Windows should show its security
   warning asking to confirm the root certificate. Afterwards check
   `certutil -user -store Root` for the CA, and note what the card says now.
   Try the Cancel path too, once. It should leave the card as it was, with no
   error worse than a short message.
3. **Browse.** Open the app from the share's pane. Note which browser started,
   and the test page's table. Then do the same in each other browser (by hand,
   see above). Compare with the macOS column above.
   - Firefox: does `security.enterprise_roots.enabled` make it trust a CA in
     the **user's** root store, not only the machine's? Row 8441 answers it.
   - Edge and Chrome: any case where they disagree with each other or with
     macOS.
4. **Trust check vs browsers.** Does the card's list of untrusted origins
   match what the browsers decided? On macOS it does, except for Firefox and a
   self-signed certificate marked CA.
5. **Port rebinding.** With the browser profile open and the test page loaded,
   quit the app from the tray and start it again (`npm run tauri dev`).
   Note the share's port before and after (`netstat -ano | findstr LISTENING`,
   or the app's log). It should stay the same. If it moved, the app restarts
   the share's profile and the pane says why. Reload the test page. Repeat a
   couple of times, also with the page reloading continuously. On macOS and
   Linux, tokio sets `SO_REUSEADDR`, and the question is whether Windows
   rebinds while the browser's old connections linger.
6. **Socket owner check.** Only if a second Windows user can be logged in at
   the same time (fast user switching). From that user's session, run
   `curl.exe -x http://127.0.0.1:<share port> https://example.com/`. The proxy
   should refuse it, and the app's log should say "refused a connection from
   another user", not "whose owner is unknown". From the first
   user, the same command should work. This is the lookup in
   `sdk/src/socket_owner/windows.rs`, never run before.
7. **Leave.** The user removes the share. The dialog should say "This will
   also remove the share's certificate. Windows will ask you to confirm."
   Windows should ask, and afterwards `certutil -user -store Root` shouldn't
   list the CA. The share's browser profile should quit and its directory be
   gone.
8. **Anything else** that looks broken on Windows (window, tray, fonts,
   wording, paths), in the Results.

## Cleanup

Quit the app, delete `%APPDATA%\dev.wispers.access.desktop.stg`, and check
with `cmdkey /list` for credentials the staging app left. Show the user what
you'd delete before deleting it. Check `certutil -user -store Root` once more.

## Results

(Write here: Windows version, browser versions, and per step what happened.
Include exact error texts. Note every code change you made and why.)
