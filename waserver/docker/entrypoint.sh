#!/usr/bin/env bash
#
# waserver container entrypoint.
#
# 1. Create the share named by the environment if it doesn't exist yet
#    (`waserver init`). Identity (keys, and on Wispers Connect the connectivity
#    group) is created once and then lives on the /data volume, with the
#    transport it is for.
# 2. Generate one supervisord program per initialised share, each running
#    `waserver serve`. Shares added later with `waserver init` are served
#    from the next start on.
# 3. exec supervisord as PID 1; it owns signal fan-out, restart, and reaping.
#
# The share's apps are configured in one of two ways: `waserver edit <share>`
# in the container's shell (see the greeting there) edits the share's config
# and reloads; or a config file mounted at `/config/<share>.toml`, kept
# outside the container (e.g. in version control), which becomes the share's
# `share.toml` (a symlink) right after step 1.
set -euo pipefail

SUPERVISORD_CONF="/etc/supervisor/supervisord.conf"
CONF_DIR="/etc/supervisor/conf.d"
CONFIG_DIR="${CONFIG_DIR:-/config}"
# Where waserver keeps each share's `share.toml` (its config dir, under $HOME).
SHARES_DIR="${XDG_CONFIG_HOME:-$HOME/.config}/waserver/shares"

# The share this container creates on its first start.
SHARE_ID="${SHARE_ID:-default}"
SHARE_TRANSPORT="${SHARE_TRANSPORT:-iroh}"

log() { printf '[entrypoint] %s\n' "$*"; }

# The `name = "…"` of a share config, for SHARE_NAME when a config is mounted.
toml_name() { sed -n -E '/^\[/q; s/^name[[:space:]]*=[[:space:]]*["'"'"'](.*)["'"'"'][[:space:]]*$/\1/p' "$1" | head -1; }

# A share whose config was mounted earlier but is not now: its `share.toml` is
# a dangling symlink, so waserver does not see the share and `init` would fail
# on the directory. Say what happened instead.
for link in "$SHARES_DIR"/*/share.toml; do
  if [[ -L "$link" && ! -e "$link" ]]; then
    log "ERROR: $link points to $(readlink "$link"), which is not mounted any more; mount it again, or replace the link with a file"
    exit 1
  fi
done

# --- Initialised shares (names only; `status` reads them off disk) ---------
# The JSON output is waserver's stable interface.
existing="$(waserver status --json 2>/dev/null | jq -r '.shares[].name' || true)"
is_existing() { grep -qxF "$1" <<<"$existing"; }

# --- Create the configured share on the first start ------------------------
if is_existing "$SHARE_ID"; then
  log "share '$SHARE_ID' already initialised"
else
  if [[ -z "${SHARE_NAME:-}" && -f "$CONFIG_DIR/$SHARE_ID.toml" ]]; then
    SHARE_NAME="$(toml_name "$CONFIG_DIR/$SHARE_ID.toml")"
  fi
  if [[ -z "${SHARE_NAME:-}" ]]; then
    log "ERROR: SHARE_NAME is not set; it is the share's name as guests see it, e.g. SHARE_NAME=\"Awesome Team\""
    exit 1
  fi
  args=(--transport "$SHARE_TRANSPORT")
  if [[ "$SHARE_TRANSPORT" == wispers-connect ]]; then
    if [[ -z "${WC_API_KEY:-}" ]]; then
      log "ERROR: SHARE_TRANSPORT=wispers-connect needs WC_API_KEY (the Wispers Connect API key) to create the share"
      exit 1
    fi
    [[ -n "${WC_BACKEND:-}" ]] && args+=(--backend "$WC_BACKEND")
  fi
  log "initialising share '$SHARE_ID' ($SHARE_NAME) on $SHARE_TRANSPORT"
  waserver init "${args[@]}" "$SHARE_ID" "$SHARE_NAME"
  existing="$(waserver status --json 2>/dev/null | jq -r '.shares[].name' || true)"
fi

# --- Mounted configs become the shares' share.toml -------------------------
while read -r name; do
  [[ -n "$name" && -f "$CONFIG_DIR/$name.toml" ]] || continue
  ln -sfn "$CONFIG_DIR/$name.toml" "$SHARES_DIR/$name/share.toml"
  log "share '$name': config is $CONFIG_DIR/$name.toml"
done <<<"$existing"

# --- Generate one supervised 'serve' per share ----------------------------
mkdir -p "$CONF_DIR"
rm -f "$CONF_DIR"/share-*.conf
count=0
while read -r name; do
  [[ -z "$name" ]] && continue
  log "serving '$name'"
  cat > "$CONF_DIR/share-$name.conf" <<EOT
[program:share-$name]
command=waserver serve $name
autostart=true
autorestart=true
startsecs=3
stopsignal=TERM
stopwaitsecs=15
environment=HOME="%(ENV_HOME)s"
stdout_logfile=/dev/stdout
stdout_logfile_maxbytes=0
stderr_logfile=/dev/stderr
stderr_logfile_maxbytes=0
EOT
  count=$((count + 1))
done <<<"$existing"

log "starting supervisord with $count share(s)"
exec supervisord -n -c "$SUPERVISORD_CONF"
