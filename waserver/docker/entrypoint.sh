#!/usr/bin/env bash
#
# waserver container entrypoint.
#
# 1. Find the desired shares: one `/config/<name>.toml` per share, using the
#    same format as the `share.toml` files that waserver itself writes (name,
#    apps). A `[transport]` section picks the transport for `init`; without
#    one the share is on iroh.
# 2. `waserver init` any shares that don't exist yet. Identity (keys, and on
#    Wispers Connect the connectivity group) is created once and then lives on
#    the /data volume, together with the transport it is for.
# 3. Copy each of the TOML files over its share's `share.toml`, so the mounted
#    config is the source of truth on every start.
# 4. Generate one supervisord program per share, each running `waserver serve`.
# 5. exec supervisord as PID 1; it owns signal fan-out, restart, and reaping.
set -euo pipefail

# One share config per share, named after it.
CONFIG_DIR="${CONFIG_DIR:-/config}"
SUPERVISORD_CONF="/etc/supervisor/supervisord.conf"
CONF_DIR="/etc/supervisor/conf.d"
# Where waserver keeps each share's `share.toml` (its config dir, under $HOME).
SHARES_DIR="${XDG_CONFIG_HOME:-$HOME/.config}/waserver/shares"

log() { printf '[entrypoint] %s\n' "$*"; }

# --- Desired shares -------------------------------------------------------
# Read the TOML files in the config directory.
toml_name() { sed -n -E '/^\[/q; s/^name[[:space:]]*=[[:space:]]*["'"'"'](.*)["'"'"'][[:space:]]*$/\1/p' "$1" | head -1; }
toml_transport() { sed -n -E '/^\[transport\]/,/^\[/{s/^kind[[:space:]]*=[[:space:]]*["'"'"'](.*)["'"'"'][[:space:]]*$/\1/p;}' "$1" | head -1; }
NAMES=(); DISPLAYS=(); TRANSPORTS=()
for file in "$CONFIG_DIR"/*.toml; do
  [[ -f "$file" ]] || continue
  name="$(basename "$file" .toml)"
  display="$(toml_name "$file")"
  if [[ -z "$display" ]]; then
    log "ERROR: $file has no 'name = \"…\"' line"
    exit 1
  fi
  transport="$(toml_transport "$file")"
  NAMES+=("$name"); DISPLAYS+=("$display"); TRANSPORTS+=("${transport:-iroh}")
done

if [[ ${#NAMES[@]} -eq 0 ]]; then
  log "ERROR: no shares configured — mount one share config per share at $CONFIG_DIR/<name>.toml"
  log "       (the format 'waserver init' writes: name, one [[app]] per app)"
  exit 1
fi

# --- Already-initialised shares (names only; `status` reads them off disk) -
# The JSON output is waserver's stable interface.
existing="$(waserver status --json 2>/dev/null | jq -r '.shares[].name' || true)"
is_existing() { grep -qxF "$1" <<<"$existing"; }
is_desired()  { local n; for n in "${NAMES[@]}"; do [[ "$n" == "$1" ]] && return 0; done; return 1; }

# --- Init shares that don't exist yet -------------------------------------
for i in "${!NAMES[@]}"; do
  name="${NAMES[$i]}"
  transport="${TRANSPORTS[$i]}"
  if is_existing "$name"; then
    log "share '$name' already initialised"
  else
    if [[ "$transport" == wispers-connect && -z "${WC_API_KEY:-}" ]]; then
      log "ERROR: share '$name' uses Wispers Connect, which needs WC_API_KEY (the Wispers Connect API key) to be created"
      exit 1
    fi
    log "initialising share '$name' (${DISPLAYS[$i]}) on $transport"
    waserver init --transport "$transport" "$name" "${DISPLAYS[$i]}"
  fi
done

# --- Shares present on disk but no longer configured ----------------------
# Deliberately NOT auto-deleted: `waserver deinit` destroys the connectivity
# group and every guest node on the backend, irreversibly. Removal stays a
# manual, deliberate action. A config typo must never nuke a share's guest
# nodes.
if [[ -n "$existing" ]]; then
  while read -r name; do
    [[ -z "$name" ]] && continue
    is_desired "$name" || log "NOTE: '$name' is initialised but no longer configured; leaving it intact (run 'waserver deinit $name' to remove)"
  done <<<"$existing"
fi

# --- The mounted config is the share's config -----------------------------
for name in "${NAMES[@]}"; do
  toml="$SHARES_DIR/$name/share.toml"
  if [[ ! -d "$(dirname "$toml")" ]]; then
    log "ERROR: share '$name' has no directory at $(dirname "$toml") after init"
    exit 1
  fi
  cp "$CONFIG_DIR/$name.toml" "$toml"
  log "share '$name': config from $CONFIG_DIR/$name.toml"
done

# --- Generate one supervised 'serve' per share ----------------------------
mkdir -p "$CONF_DIR"
rm -f "$CONF_DIR"/share-*.conf
for name in "${NAMES[@]}"; do
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
done

log "starting supervisord with ${#NAMES[@]} share(s)"
exec supervisord -n -c "$SUPERVISORD_CONF"
