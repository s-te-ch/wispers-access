#!/usr/bin/env bash
#
# Container healthcheck. Healthy (exit 0) only if at least one share exists and
# every share reports "serving". During startup shares are "connecting"; the
# Dockerfile HEALTHCHECK --start-period covers that window.
set -uo pipefail

out="$(waserver status --json 2>/dev/null)" || exit 1
[[ -z "$out" ]] && exit 1

# Healthy iff there is at least one share and none is in a non-'serving'
# state (connecting / offline / error).
jq -e '(.shares | length > 0) and all(.shares[]; .server.state == "serving")' \
  <<<"$out" >/dev/null || exit 1

exit 0
