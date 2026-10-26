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

### Environment

- Windows 10 Home 22H2, build 10.0.19045.6466 (not Windows 11). WebView2
  155.0.4283.45 was already installed.
- Rust stable MSVC, VS 2022 build tools, Node 24.14.1, npm 11.11.0.
- Chrome and Edge were installed. The user installed Firefox for the test.
- Baseline: no "WA Test CA" in `certutil -user -store Root`, no
  `%APPDATA%\dev.wispers.access.desktop.stg`. The production Access app is
  installed and running (`%LOCALAPPDATA%\Wispers Access`), with its data
  dir `%APPDATA%\dev.wispers.access.desktop` and a credential
  `client/browser_pairing_secret.dev.wispers.access.desktop`. Cleanup leaves
  those alone.

### Build

- First `npm run tauri dev` failed: `rustc 1.94.1 is not supported by the
  following package: sysinfo@0.39.6 requires rustc 1.95`. I ran
  `rustup update stable` (now 1.99.1) and didn't touch Cargo.lock. If
  1.94 should keep working, `sysinfo` needs pinning, or `rust-version` in
  Cargo.toml should say 1.95.
- The second build worked (2m 37s for the last crates). No code changes.
- Start-up log: `WARN port 11235 is taken, using any free one
  error=failed to bind to 127.0.0.1:11235: Only one usage of each socket
  address ... (os error 10048)`, then `serving shares on
  wa.localhost:51906`. The installed production app (PID 17824) holds
  11235. The staging app falls back as it should, but staging and
  production share the preferred port.
- Tray: the user couldn't tell the staging and production icons apart.
  Both use the same icon, and the tooltip is hard-coded to "Wispers Access"
  (`tray.rs:54`, also the "Open Wispers Access" item). Using the product
  name ("Wispers Access (staging)") there would tell them apart. The user
  closed both windows to quit, but closing only hides to the tray, so both
  kept running. I killed both processes (production with the user's OK)
  and restarted staging, which then got 11235.

### 1. Join

- Joining worked. Log: `connecting to the host node over iroh`,
  `activating`, then `WARN probe timed out` (not from this repo, most likely
  an iroh net-report probe, and harmless).
- The card showed up with the Install button. It doesn't list the untrusted
  origins before installing. That's by design (`CertificateNotice.svelte`:
  the `installable` branch names only the certificate, and only the
  `installed` and "nothing to install" branches list origins), so this
  handoff's expectation was wrong, not the app. The SDK's pre-install list
  can't be seen in the UI.

### 2. Install

- Install: Windows showed its security warning for the root certificate, and
  the user confirmed. `certutil -user -store Root` then listed
  `CN=WA Test CA (localtest.me only)`, serial
  `0bef88c1c25f810ce15c5cca0f1c955d009ba7c1`, SHA-1
  `36e163cf6bb1eb5c6ce54e7f25a148d43a377970`.
- The card then said: "The certificate is installed, but nc-out.lvh.me:8442,
  open.lvh.me:8443, ss-leaf.localtest.me:8444, ss-ca.localtest.me:8445,
  ss-plain.localtest.me:8446 still aren't trusted. Whoever runs the share has
  to fix this." That's the expected set: 8441 and example.com trusted now,
  while 8499 (unreachable) and 8447 (not an origin) aren't listed.
- Cancel path: I removed the CA (`certutil -user -delstore Root <sha1>`) to
  try Install → No.
  - Then the user switched to a second share (an access-demo share, which
    caused a flood of `agent.c:2050: Got TURN CreatePermission error
    response, code=403` warnings) and back. The "Trust test" card had
    vanished. Log: `WARN no answer to a FORWARD_PROXY stream
    share="trust-test" error="io: connection lost"`, `cached connection is
    dead; redialling error="timed out"`, and then `connecting to the host
    node over iroh` every 10-30 s without success. The Mac was awake. The
    trust check goes through the tunnel, so it failed, and the card hides
    itself when the check fails (`CertificateNotice.svelte:46`, only a
    `console.warn`). Suggestion: say the check couldn't run, instead of
    showing nothing.
  - The app keeps its own record of what it installed
    (`installed-certificates.json`) and doesn't compare it with the store.
    So after a certificate is removed by hand, the card would say
    "installed, but …" with no Install button, and there's no way to
    install it again from the app.
  - The user then tried to quit, but staging kept running. As it turned out
    later (step 5), they had used the taskbar button's "Close window",
    which only hides the window. I killed staging, set `installed-certificates.json` to `[]`
    (backup in the scratchpad), and restarted it. It connected over iroh
    straight away.
  - With the record cleared, the card showed the Install button again.
    Install → **No** in the Windows dialog: the card stayed as it was, no
    error text, nothing logged. Then Install → Yes: the CA was back in the
    store and the record, and the card showed the "installed, but …"
    message.

### 3. Browse

- Opening the app from the pane started **Chrome 155.0.8059.41** (first
  found) with `--user-data-dir=%APPDATA%\dev.wispers.access.desktop.stg\profiles\<share id>\chrome
  --proxy-server=127.0.0.1:52655 --proxy-bypass-list=<-loopback>
  --no-first-run --no-default-browser-check https://nc-in.localtest.me:8441/`.
  The share's port was 52655 both before and after the restart above.
- Chrome 155 table: 8441 OK, 8442 to 8447 FAIL. Same as macOS, and the
  same as the card's list.
- Firefox 158.0, started by hand (`-profile <tmp> -no-remote`, with the
  app's `user.js` for port 52655): 8441 OK, 8442 to 8447 FAIL. Same as macOS.
  So **`security.enterprise_roots.enabled` does pick up a CA from the
  user's root store (`CurrentUser\Root`)** on Windows, not only the
  machine's.
- Edge 156.0.4314.42: my first launch went through `cmd /c start`, which
  read `<-loopback>` as a redirect and failed. The Edge window the user then
  clicked through (first-run/update screens) was their normal profile
  (`--profile-directory=Default`), which was why it said
  `ERR_CONNECTION_REFUSED`. Not an app finding. I relaunched with
  PowerShell's `Start-Process`, with the app's flags. Table: 8441 OK, 8442 to
  8447 FAIL. Same as Chrome and macOS.
- No case where Edge and Chrome disagree.

### 4. Trust check vs browsers

- The card's untrusted list (8442 to 8446) matches all three browsers.
  8447 FAILs in every browser, but it's not one of the share's origins, so
  the card rightly leaves it out. No exception here, unlike macOS (Firefox
  with a self-signed certificate marked CA): Firefox on Windows also fails
  8445.

### 5. Port rebinding

- Quitting is hard to find on Windows. Twice the user "quit" through the
  taskbar button's menu ("Close window"), which only hides the window to
  the tray. The tray icon is behind the overflow arrow (^) on Windows 10,
  and its menu is the only way to quit. Suggestion: the first time the
  window is closed to the tray on Windows, say so (a notification such as
  "Wispers Access keeps running in the tray"), or offer Quit inside the
  window.
- After I killed the hand-started Edge and Firefox, the share's port 52655
  had `TIME_WAIT` sockets and three `CLOSE_WAIT` ones owned by the app (the
  browser closed its end, the app didn't). By the time of the quit, the
  `CLOSE_WAIT` sockets were gone.
- Run 1 (port moved, my fault): the user quit from the tray and `npm run
  tauri dev` exited with code 0, but the old process still held 11235 and
  52655 for a few more seconds (still `LISTENING`). I restarted straight
  away, so the new instance logged `port 11235 is taken, using any free
  one`, then `the share's port moved, restarting its profile
  share=f59a61a3-… browser=Chrome port=53979`. Chrome came back with
  `--proxy-server=127.0.0.1:53979`. So the move-and-restart path works on
  Windows. Note: shutdown takes a few seconds after the tray Quit, and
  a quick relaunch (by hand, or the updater's `restart`) can race it.
  The pane said, under the app: "This share's browser restarted when Wispers
  Access started, because the share had to move to a new port. Its tabs
  were restored, but anything unsaved in them was lost." Chrome restored the
  test tab, and reloading it gave the same table.
- Run 2 (**bug found**): tray Quit with the test page open in Chrome. The
  app process was gone at once, but 53979 stayed `LISTENING` under its
  dead PID for well over a minute, along with `CLOSE_WAIT`/`ESTABLISHED`
  connections to Chrome. curl through `-x http://127.0.0.1:53979` hung,
  since the kernel accepted connections but nobody served them. Chrome's main
  process was a child of the old app process. After I stopped that Chrome,
  the listener and connections went away at once. So **Chrome inherited the
  share's listening socket** (and the accepted connections), so the port
  stays bound as long as the browser runs, and every app restart with the
  browser open would have to move the port and restart the browser.
  - Cause: `loopback::bind_port` used `tokio::net::TcpListener::bind`, and
    mio 1.2.0 creates Windows sockets with plain `socket()`
    (`mio-1.2.0/src/sys/windows/net.rs:36`), whose handles are inheritable.
    `std::process::Command` starts the browser with handle inheritance on.
    No Winsock LSPs on this machine (`netsh winsock show catalog`), so this
    isn't an LSP quirk.
  - Also: mio doesn't set `SO_REUSEADDR` on Windows (deliberately, since
    there it allows hijacking a port), so the handoff's assumption that
    tokio sets it holds for macOS and Linux only.
  - **Code change**, `sdk/src/loopback.rs`: `bind_port` now binds through
    a new `listen_on`, which uses `tokio::net::TcpSocket`. That creates
    sockets with socket2, which adds `WSA_FLAG_NO_HANDLE_INHERIT` on
    Windows. It sets `SO_REUSEADDR` outside Windows, like
    `TcpListener::bind`, and listens with the same backlog (128). Should
    behave the same on macOS and Linux, but I could only run it here.
  - **Code change**, `sdk/src/socket_owner/linux.rs`: `cargo test -p
    wispers-access-sdk` didn't compile on Windows (`cannot find module or
    crate libc`), since the module is compiled for tests everywhere but
    `libc` is a Unix-only dependency. `is_same_user` is now
    `#[cfg(target_os = "linux")]` (it was `allow(dead_code)` elsewhere).
    CI's Windows `cargo check` doesn't build tests, so it never noticed.
  - With that, 48 SDK tests pass on Windows and 5 fail, the same 5 with and
    without the loopback change, so they're not caused by it:
    `per_app_ports_survive_a_new_proxy`,
    `per_app_ports_are_served_again_after_a_rebind`,
    `a_required_cookie_is_demanded_on_every_request`,
    `a_browse_url_pairs_the_browser_once` (all `remove_dir_all(data_dir)`:
    `The process cannot access the file because it is being used by another
    process`, os error 32, so something still holds a file in the data dir
    when the test deletes it), and
    `origins_are_checked_by_a_handshake_through_the_share` (`Could not
    automatically determine the process-level CryptoProvider from Rustls
    crate features`). I left them alone.
- Run 3 (with the fix): Chrome opened from the pane, test page reloaded,
  tray Quit. Right after the quit, 53979 had no `LISTENING` entry, only 75
  `TIME_WAIT`, while the app's Chrome (a child of the dead app process)
  kept running. Restarted at once: `serving shares on wa.localhost:11235`,
  the share back on **53979**, no "port moved", no browser restart. So on
  Windows `TIME_WAIT` doesn't block the rebind, even without
  `SO_REUSEADDR`.
- Run 4 (with the fix, under load): a curl loop requested the test page
  through `-x http://127.0.0.1:53979` about 9 times a second (all 200).
  Tray Quit at 13:45:54, which left 377 `TIME_WAIT` and one `SYN_SENT` on
  the port. Restarted at once, and the app took 53979 again. One request
  failed (`000`, from the ~3 s gap), and from 13:45:58 on everything was 200
  again. After that, the test page in Chrome still showed the same table.

### 6. Socket owner check

- Not tested: there's no second Windows account on this machine. Only the
  same-user half is covered. The curl loop in step 5 went through the
  proxy as this user hundreds of times and was never refused.

### 7. Leave

- The user clicked through the leave dialog too quickly to read it, so its
  text wasn't checked. (The Windows wording comes from
  `desktop/src/lib/platform.ts:13`, "Windows will ask you to confirm.")
- Windows asked to confirm removing the root certificate, and the user
  confirmed. Afterwards: no "WA Test CA" in `certutil -user -store Root`,
  `installed-certificates.json` is `[]`, `profiles\` is empty (the share's
  directory is gone), and no Chrome with the share's profile is running.
  So the app quit Chrome and deleted the profile.
- Log on leaving: `ERROR Endpoint dropped without calling
  Endpoint::close. Aborting ungracefully.` (iroh), then `told the host node
  we are leaving`. The error comes from `Client::leave` dropping the guest
  node (and so its iroh endpoint) without closing it. Harmless, but logged
  as ERROR.
- **The share's port stays bound after leaving** (not Windows-specific):
  53979 kept `LISTENING` on both loopback addresses, and a CONNECT through
  it now gets `403`. `PerShareForwardProxy` only releases ports when the
  whole proxy is dropped (`impl Drop`), with no per-share release, and
  `shares::leave` doesn't touch it. It goes away when the app quits. Fixing
  it needs a new SDK call (for example `PerShareForwardProxy::release(share)`
  for `leave` to call), so I left it for you.

### 8. Anything else

- Tray tooltip and staging/production look-alike, quitting only from the
  tray, and the port clash with production: see Build and step 5.
- The connection to the host node dropped twice, each time about 4-5 minutes
  after it was set up (`failed closing path err=LastOpenPath`, then
  `cached connection is dead; redialling error="timed out"`). The first
  time, redialling over iroh failed for minutes (until the app was
  restarted), while the Mac was awake. A second, unrelated share (access-demo)
  was joined just before, which logged a flood of `agent.c:2050: Got TURN
  CreatePermission error response, code=403`.
- Fonts, window and paths looked fine. Nothing else broke.

### Cleanup

- Staging quit. Deleted `%APPDATA%\dev.wispers.access.desktop.stg` and the
  staging credentials `client/browser_pairing_secret.dev.wispers.access.desktop.stg`
  and the access-demo share's `5daa992c-…/registration…stg` and
  `…/root_key…stg` (the Trust test share's credentials were already gone
  after leaving). The access-demo share wasn't left (it cleans itself up
  daily).
- Kept: production's data dir and its
  `client/browser_pairing_secret.dev.wispers.access.desktop`.
- `certutil -user -store Root`: no "WA Test CA".
- The production app was stopped during the session and wasn't started
  again by me.
