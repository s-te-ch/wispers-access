#!/bin/sh
#
# Notarise and staple the DMG that `tauri build` made.
#
# Same credentials Tauri's bundler uses: APPLE_ID, APPLE_PASSWORD (an
# app-specific password) and APPLE_TEAM_ID.
set -eu

for var in APPLE_ID APPLE_PASSWORD APPLE_TEAM_ID; do
  if [ -z "$(eval "echo \${$var:-}")" ]; then
    echo "$0: $var is not set" >&2
    exit 1
  fi
done

target=$(cargo metadata --format-version 1 --no-deps --manifest-path src-tauri/Cargo.toml \
  | python3 -c 'import json, sys; print(json.load(sys.stdin)["target_directory"])')
set -- "$target"/release/bundle/dmg/*.dmg
if [ ! -f "$1" ]; then
  echo "$0: no DMG under $target/release/bundle/dmg; run 'npm run tauri build' first" >&2
  exit 1
fi

for dmg in "$@"; do
  echo "Notarizing $dmg"
  xcrun notarytool submit "$dmg" \
    --apple-id "$APPLE_ID" --password "$APPLE_PASSWORD" --team-id "$APPLE_TEAM_ID" \
    --wait
  xcrun stapler staple "$dmg"
  spctl -a -vv -t open --context context:primary-signature "$dmg"
done
