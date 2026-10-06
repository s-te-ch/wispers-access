#!/bin/sh
#
# Installs the Wispers Access CLI tools, waserver and waclient, from the newest
# GitHub release of each. Re-running it upgrades the tools.
#
#   curl -fsSL https://raw.githubusercontent.com/s-te-ch/wispers-access/main/install.sh | sh
#
# Currently supported platforms are Linux (amd64, arm64) and macOS (Apple
# silicon). The binaries go to /usr/local/bin, or to the directory given with
# --prefix. If that directory is not writable, sudo is used for that one step. 
#
# This script doesn't support Windows. Instead, download the zip from
# https://github.com/s-te-ch/wispers-access/releases

set -eu

REPO="s-te-ch/wispers-access"
COMPONENTS="waserver waclient"
prefix="/usr/local/bin"

main() {
  parse_args "$@"
  target="$(detect_target)"
  tmp="$(mktemp -d)"
  trap 'rm -rf "$tmp"' EXIT
  fetch "https://api.github.com/repos/$REPO/releases?per_page=100" "$tmp/releases.json"
  upgraded=""
  for component in $COMPONENTS; do
    install_component "$component"
  done
  case ":$PATH:" in
    *":$prefix:"*) ;;
    *) say "Note: $prefix is not on your PATH." ;;
  esac
  if [ -n "$upgraded" ]; then
    say "A running waserver keeps the old version until it is restarted" \
      "(systemctl restart 'waserver@*', or waserver stop and start)."
  fi
  say ""
  say "Next: share an app with 'waserver init <share> \"<Name>\"', then 'waserver edit <share>';"
  say "see https://github.com/$REPO#quick-start. For a systemd service, see"
  say "https://github.com/$REPO/tree/main/integrations/systemd."
}

parse_args() {
  while [ $# -gt 0 ]; do
    case "$1" in
      --prefix) shift; prefix="${1:?--prefix needs a directory}" ;;
      --prefix=*) prefix="${1#--prefix=}" ;;
      -h|--help) usage; exit 0 ;;
      *) die "unknown argument '$1' (try --help)" ;;
    esac
    shift
  done
}

usage() {
  cat <<USAGE
Installs the newest release of waserver and waclient.

  curl -fsSL https://raw.githubusercontent.com/$REPO/main/install.sh | sh
  curl -fsSL https://raw.githubusercontent.com/$REPO/main/install.sh | sh -s -- --prefix ~/.local/bin

  --prefix DIR   install into DIR instead of $prefix
USAGE
}

# Detects the release target name for this machine.
detect_target() {
  os="$(uname -s)"
  arch="$(uname -m)"
  case "$os/$arch" in
    Linux/x86_64) echo linux-amd64 ;;
    Linux/aarch64 | Linux/arm64) echo linux-arm64 ;;
    Darwin/arm64) echo macos-arm64 ;;
    Darwin/x86_64) die "there is no build for Intel Macs" ;;
    *) die "unsupported platform $os/$arch; see https://github.com/$REPO/releases" ;;
  esac
}

# Installs one component at its newest release, unless that is installed
# already. Each component has its own release tags, <component>-vX.Y.Z.
install_component() {
  component="$1"
  version="$(newest_version "$component")"
  current="$(installed_version "$component")"
  if [ "$current" = "$version" ]; then
    say "$component $version is already in $prefix"
    return
  elif [ -n "$current" ]; then
    say "Upgrading $component $current to $version"
    upgraded="$upgraded $component"
  else
    say "Installing $component $version"
  fi
  archive="$component-$version-$target.tar.gz"
  base="https://github.com/$REPO/releases/download/$component-v$version"
  fetch "$base/$archive" "$tmp/$archive"
  fetch "$base/SHA256SUMS" "$tmp/$component.SHA256SUMS"
  verify "$tmp/$archive" "$tmp/$component.SHA256SUMS"
  tar -xzf "$tmp/$archive" -C "$tmp"
  install_binary "$tmp/${archive%.tar.gz}/$component"
}

# Detects the component's newest release.
newest_version() {
  # The API lists releases newest first, so just take the first match.
  version="$(sed -n 's/.*"tag_name": *"'"$1"'-v\([^"]*\)".*/\1/p' "$tmp/releases.json" | head -n 1)"
  [ -n "$version" ] || die "found no $1 release at https://github.com/$REPO/releases"
  echo "$version"
}

# Detects the already installed version of the component, or nothing.
installed_version() {
  [ -x "$prefix/$1" ] || return 0
  "$prefix/$1" --version 2>/dev/null | awk '{print $2}' || true
}

# Checks the archive against its line in the release's SHA256SUMS.
verify() {
  name="$(basename "$1")"
  expected="$(awk -v name="$name" '$2 == name {print $1}' "$2")"
  [ -n "$expected" ] || die "$name is not in the release's SHA256SUMS"
  if command -v sha256sum >/dev/null 2>&1; then
    actual="$(sha256sum "$1" | awk '{print $1}')"
  else
    actual="$(shasum -a 256 "$1" | awk '{print $1}')"
  fi
  [ "$actual" = "$expected" ] || die "checksum mismatch for $name"
}

# Puts the binary into $prefix, with sudo if the directory is not ours.
install_binary() {
  if [ -w "$prefix" ] || { [ ! -e "$prefix" ] && mkdir -p "$prefix" 2>/dev/null; }; then
    install -m 755 "$1" "$prefix/"
  elif command -v sudo >/dev/null 2>&1; then
    say "  $prefix is not writable, using sudo"
    sudo install -d "$prefix"
    sudo install -m 755 "$1" "$prefix/"
  else
    die "$prefix is not writable and there is no sudo; run as root or use --prefix"
  fi
}

fetch() {
  if command -v curl >/dev/null 2>&1; then
    curl -fsSL --retry 3 -o "$2" "$1" || die "download failed: $1"
  elif command -v wget >/dev/null 2>&1; then
    wget -qO "$2" "$1" || die "download failed: $1"
  else
    die "need curl or wget"
  fi
}

say() { printf '%s\n' "$*"; }
die() { printf 'install.sh: %s\n' "$*" >&2; exit 1; }

main "$@"
