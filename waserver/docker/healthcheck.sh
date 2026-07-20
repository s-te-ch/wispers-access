#!/usr/bin/env bash
#
# Container healthcheck. Healthy (exit 0) only if at least one share exists and
# every share reports "serving". During startup shares are "connecting"; the
# Dockerfile HEALTHCHECK --start-period covers that window.
set -uo pipefail

out="$(waserver status 2>/dev/null)" || exit 1
[[ -z "$out" ]] && exit 1
grep -q 'No app shares found' <<<"$out" && exit 1

# Any share row (header skipped) not in the 'serving' state (connecting /
# offline / error) => unhealthy. TODO: move to `waserver status --json` + jq.
tail -n +2 <<<"$out" | awk '$2 != "serving" { bad=1 } END { exit bad }' || exit 1

exit 0
