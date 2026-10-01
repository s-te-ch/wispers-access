#!/bin/sh
#
# Helper script to have the binary code-signed before `cargo run` runs it,
# preventing Keychain prompts on every rebuild on macOS.
#
# A plain `cargo build` leaves the binary ad-hoc signed, with a signature that
# changes on every build. The Keychain identifies an app by its signature when
# it decides whether to hand over an item without asking, so every rebuild would
# bring a prompt per secret. Signing with a stable identity and a fixed
# identifier makes every build the same app to the Keychain.
#
# WISPERS_ACCESS_DEV_SIGNING_IDENTITY names the identity; the default matches
# any Apple Development certificate. Without one, the binary runs unsigned and
# the prompts are back.
set -eu

if [ "${1:-}" != "run" ]; then
  exec cargo "$@"
fi
shift

# `cargo run <build args> -- <app args>`
build_args=""
app_args=""
seen_separator=no
for arg in "$@"; do
  if [ "$seen_separator" = yes ]; then
    app_args="$app_args $arg"
  elif [ "$arg" = "--" ]; then
    seen_separator=yes
  else
    build_args="$build_args $arg"
  fi
done

# shellcheck disable=SC2086 # the args are meant to split
executable=$(cargo build $build_args --message-format=json-render-diagnostics \
  | grep -o '"executable":"[^"]*"' | tail -1 | cut -d'"' -f4)
if [ -z "$executable" ]; then
  echo "cargo-with-codesign.sh: cargo built no executable" >&2
  exit 1
fi

identity="${WISPERS_ACCESS_DEV_SIGNING_IDENTITY:-Apple Development}"
if security find-identity -v -p codesigning | grep -q "$identity"; then
  codesign --force --sign "$identity" --identifier dev.wispers.access.desktop "$executable"
else
  echo "cargo-with-codesign.sh: no '$identity' signing identity; running unsigned, expect Keychain prompts" >&2
fi

# shellcheck disable=SC2086
exec "$executable" $app_args
