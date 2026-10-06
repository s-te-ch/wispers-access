# waserver as a systemd service

`waserver@.service` runs one server per share under a dedicated `waserver` user,
with the state in that user's home (`/var/lib/waserver`). It expects the binary
in `/usr/local/bin`, where [install.sh](../../install.sh) puts it. Edit
`ExecStart` and `ExecReload` if yours is elsewhere.

See the [hosting guide](../../docs/hosting.md#as-a-systemd-service) for how to
set it up and run it day to day.
