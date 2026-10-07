# Frequently asked questions

### My shared app is broken, but waserver logs the requests

Wispers Access doesn't currently work with multi-origin web apps, that is, web
apps that send requests to more than one domain. If a page partially loads but
then runs into errors, this is often the cause.

Make your app single-origin if possible. For example, if you use foo.com for the
app and api.foo.com for the backend API, put your API behind foo.com/api instead.

### The QR code looks ragged or does not scan

Depending on terminal and font, the default QR code rendering in the terminal
can end up unscannable. In that case, you use the flag `--qr=compat` to render
larger code that is less demanding on your terminal. Or you can use `--png
invite.png` to write the QR code to a PNG file.

### My invite code gets rejected

An invite works only once and only for 24 hours. Ask your host to send you a new
one.

### I edited `share.toml` and nothing changed

A running server keeps its current configuration until you tell it to reload it
(`waserver reload`). Also, if the configuration isn't valid, the server will
also keep the old one. If you use `waserver edit <share>`, the modified
configuration automatically get validated and reloaded when you save and exit
the editor.

### Do I need to open or forward a port?

No inbound ones. waserver only makes outbound connections, directly to the guest
nodes if the network allows it, through a relay otherwise. However, it's
possible that your firewall prevents outbound connectionsk. In that case,
you need to allowlist at least the relay of the chosen peer-to-peer transport.

### Why does my desktop browser ask to pair?

Pairing with the system browser is mostly transparent, but in some cases it can
break, for example if you clear your cookies. To re-pair, simply open the web
app through your Wispers Access app (not through a bookmark or similar). This
should automatically run pairing again and install the necessary cookie.

### Should I use iroh or Wispers Connect?

iroh is the default because it requires no account. Wispers Connect is the
original transport design for Wispers Access, but it requires creating an
account at [connect.wispers.dev](https://connect.wispers.dev) to get an API key. 
