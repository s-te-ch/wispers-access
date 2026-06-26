#!/usr/bin/env bash
#
# Connector entrypoint.
#
# 1. Read the desired shares from a config file (one `name | display | upstream`
#    per line).
# 2. `waserver init` any that don't exist yet. Identity (connectivity group +
#    keys) is created once and then lives on the /data volume.
# 3. Generate one supervisord program per share, each running `waserver serve`.
# 4. exec supervisord as PID 1; it owns signal fan-out, restart, and reaping.
set -euo pipefail

: "${WC_API_KEY:?WC_API_KEY must be set (the Wispers Connect API key)}"

SHARES_FILE="${SHARES_FILE:-/config/shares.conf}"
SUPERVISORD_CONF="/etc/supervisor/supervisord.conf"
CONF_DIR="/etc/supervisor/conf.d"

log()  { printf '[entrypoint] %s\n' "$*"; }
trim() {
  local s="$1";
  s="${s#"${s%%[![:space:]]*}"}";
  s="${s%"${s##*[![:space:]]}"}";
  printf '%s' "$s";
}

if [[ ! -f "$SHARES_FILE" ]]; then
  log "ERROR: shares file not found at $SHARES_FILE"
  log "       expected one share per line:  name | display name | host:port"
  exit 1
fi

# --- Parse desired shares -------------------------------------------------
declare -a NAMES DISPLAYS UPSTREAMS
while IFS='|' read -r name display upstream || [[ -n "$name" ]]; do
  name="$(trim "$name")"
  [[ -z "$name" || "$name" == \#* ]] && continue   # skip blanks and comments
  display="$(trim "${display:-}")"
  upstream="$(trim "${upstream:-}")"
  [[ -z "$display" ]] && display="$name"
  if [[ -z "$upstream" ]]; then
    log "ERROR: share '$name' has no upstream (expected: name | display | host:port)"
    exit 1
  fi
  NAMES+=("$name"); DISPLAYS+=("$display"); UPSTREAMS+=("$upstream")
done < "$SHARES_FILE"

if [[ ${#NAMES[@]} -eq 0 ]]; then
  log "ERROR: no shares defined in $SHARES_FILE"
  exit 1
fi

# --- Already-initialised shares (names only; `status` reads them off disk) -
existing="$(waserver status 2>/dev/null | grep -v 'No app shares found' | awk '{print $1}' || true)"
is_existing() { grep -qxF "$1" <<<"$existing"; }
is_desired()  { local n; for n in "${NAMES[@]}"; do [[ "$n" == "$1" ]] && return 0; done; return 1; }

# --- Init shares that don't exist yet -------------------------------------
for i in "${!NAMES[@]}"; do
  name="${NAMES[$i]}"
  if is_existing "$name"; then
    log "share '$name' already initialised"
  else
    log "initialising share '$name' (${DISPLAYS[$i]})"
    waserver init "$name" "${DISPLAYS[$i]}"
  fi
done

# --- Shares present on disk but no longer in config -----------------------
# Deliberately NOT auto-deleted: `waserver deinit` destroys the connectivity
# group and every member on the backend, irreversibly. Removal stays a manual,
# deliberate action. A config typo must never nuke a share's members.
if [[ -n "$existing" ]]; then
  while read -r name; do
    [[ -z "$name" ]] && continue
    is_desired "$name" || log "NOTE: '$name' is initialised but not in $SHARES_FILE; leaving it intact (run 'waserver deinit $name' to remove)"
  done <<<"$existing"
fi

# --- Generate one supervised 'serve' per share ----------------------------
mkdir -p "$CONF_DIR"
rm -f "$CONF_DIR"/share-*.conf
for i in "${!NAMES[@]}"; do
  name="${NAMES[$i]}"; upstream="${UPSTREAMS[$i]}"
  log "serving '$name' -> $upstream"
  cat > "$CONF_DIR/share-$name.conf" <<EOF
[program:share-$name]
command=waserver serve $name $upstream
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
EOF
done

log "starting supervisord with ${#NAMES[@]} share(s)"
exec supervisord -n -c "$SUPERVISORD_CONF"
