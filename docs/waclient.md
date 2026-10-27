# waclient

`waclient` is the command-line client of Wispers Access. It's meant for
developers and for checking a share from the host itself. Everyone else is
better off with the desktop or mobile apps.

Quick overview of commands:

```sh
waclient join wax1_...   # join a share with an invite code
waclient serve 8000      # serve the shares' apps until stopped
waclient list            # the shares this device has joined
waclient remove <share>  # leave a share
```

`serve` prints the URLs of the proxied apps, so you know where to point your
browser.

## Reverse-proxied apps

For apps shared in the default mode (reverse-proxy mode), `serve` prints URLs
that you can simply copy-paste into your normal browser. The URL pairs your
browser with `waclient` on first use. After that, the same browser can also open
the app at `http://<app>.<share>.wa.localhost:<port>/`.

## Forward-proxied apps

Forward-proxy mode is compatible with more web apps (especially multi-origin
apps), but it requires configuring your browser to use a proxy. The port stays
the same from one run to the next, so you don't constantly have to reconfigure.
For some browsers (Chrome, Firefox, Edge), profiles are a good way to manage
these proxies.

The following sections explain in more detail how to configure your browser.

### Chrome, Edge, Brave and Chromium

These browsers use the system's proxy settings, unless they're started with
`--proxy-server`. If the browser is already running it will ignore the flag, but
you can work around this by starting it with a profile directory of its own.

For example on macOS:

```sh
"/Applications/Google Chrome.app/Contents/MacOS/Google Chrome" \
  --user-data-dir="$HOME/.waclient-chrome" \
  --proxy-server=127.0.0.1:<port> \
  --proxy-bypass-list='<-loopback>'
```
`--proxy-bypass-list='<-loopback>'` makes the browser use the proxy for
localhost addresses, too. If someone shares the web app they're working on, this
is what makes that work.

On Linux, the command is `google-chrome` (or `chromium`). On Windows, it's
`"C:\Program Files\Google\Chrome\Application\chrome.exe"`, with the same flags.

### Firefox

In Settings, under General, open Network Settings. Choose "Manual proxy
configuration", enter `127.0.0.1` and the share's port as HTTP Proxy, and check
"Also use this proxy for HTTPS". To keep this apart from your normal profile,
create a profile for the share in `about:profiles` first.

For apps on `localhost`, also set `network.proxy.allow_hijacking_localhost` to
`true` in `about:config`. Without it, Firefox never sends `localhost` to a
proxy.

### Certificates

Internal apps often us certificates that only work inside the company network.
If you have URLs like `https://sso.corp`, this is your case.

For shares like these, the host can attach the necessary certificates to the
share, so guests can install them on their devices (with `waclient` this is
manual, with the mobile and desktop apps it's mostly automated).

You can get a share's certificates with

```sh
waclient certificates <share> > <share>.pem
```

Then, you need to import it:

- macOS: add it to your login keychain, trusted for SSL, with
  `security add-trusted-cert -r trustRoot -p ssl -k ~/Library/Keychains/login.keychain-db <share>.pem`.
  Chrome, Edge and Firefox use it from there.
- Windows: add it to your root certificates, with
  `certutil -user -addstore Root <share>.pem`. Chrome, Edge and Firefox use it
  from there.
- Linux: add it in the browser. In Chrome, that's Settings, Privacy and
  security, Security, Manage certificates. In Firefox, it's Settings, Privacy &
  Security, View Certificates, Authorities. Note: `waclient serve` checks the
  system's store, which these browsers don't use, so it will continue
  complaining that the origins aren't trusted.

When leaving a share (`waclient remove`), you'll to manually remove the
certificate as well. Delete it the way you imported it.
