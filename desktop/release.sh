#!/bin/sh
#
# Release the desktop app, start to finish. Needs the Developer ID certificate
# in the login keychain, APPLE_ID / APPLE_PASSWORD / APPLE_TEAM_ID for
# notarization, and the updater's private key (TAURI_SIGNING_PRIVATE_KEY,
# defaulting to the file `tauri signer generate` wrote).
set -eu
cd "$(dirname "$0")"

# Source the env file to get required variables.
if [ -f .env.release ]; then
  set -a
  # shellcheck disable=SC1091
  . ./.env.release
  set +a
fi

export TAURI_SIGNING_PRIVATE_KEY="${TAURI_SIGNING_PRIVATE_KEY:-$HOME/.tauri/wispers-access-desktop.key}"
if [ ! -f "$TAURI_SIGNING_PRIVATE_KEY" ] && [ "${TAURI_SIGNING_PRIVATE_KEY#untrusted}" = "$TAURI_SIGNING_PRIVATE_KEY" ]; then
  echo "$0: no updater key at $TAURI_SIGNING_PRIVATE_KEY (TAURI_SIGNING_PRIVATE_KEY overrides)" >&2
  exit 1
fi

npm run tauri build
./notarize-dmg.sh
node updater-manifest.mjs
