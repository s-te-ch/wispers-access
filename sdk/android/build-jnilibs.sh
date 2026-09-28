#!/bin/bash
set -euo pipefail

# Builds the SDK for Android and lays it out as the Android library module
# next to this script: the Rust shared library per ABI under jniLibs, and
# the Kotlin bindings UniFFI generates from it. Release by default, since
# the library ships whole inside the APK and a debug build is ~360 MB per
# ABI.
#
# Output, next to this script (git-ignored):
#   src/main/jniLibs/<abi>/libwispers_access_sdk.so
#   src/main/kotlin/dev/wispers/access/sdk/wispers_access_sdk.kt
#
# Prerequisites:
#   - an Android NDK, found through ANDROID_NDK_HOME or the SDK's ndk/ dir
#   - cargo install cargo-ndk
#   - rustup target add aarch64-linux-android armv7-linux-androideabi x86_64-linux-android
#
# Usage: ./build-jnilibs.sh [--debug] [--abis "arm64-v8a armeabi-v7a x86_64"]
#
# --abis picks which ABIs go into jniLibs; the default is all three. CI builds
# one, enough to prove the crate cross-compiles for Android.

ANDROID_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_DIR="$(cd "$ANDROID_DIR/../.." && pwd)"
CRATE=wispers-access-sdk
LIB=libwispers_access_sdk.so

PROFILE="release"
CARGO_FLAG="--release"
ABIS="arm64-v8a armeabi-v7a x86_64"
while [[ $# -gt 0 ]]; do
    case "$1" in
        --debug) PROFILE="debug"; CARGO_FLAG=""; shift ;;
        --abis) ABIS="$2"; shift 2 ;;
        *) echo "usage: $0 [--debug] [--abis \"arm64-v8a armeabi-v7a x86_64\"]"; exit 2 ;;
    esac
done

# The host build the bindings are read from: a dylib on macOS, a .so on Linux.
case "$(uname -s)" in
    Darwin) HOST_LIB=libwispers_access_sdk.dylib ;;
    Linux) HOST_LIB=libwispers_access_sdk.so ;;
    *) echo "ERROR: unsupported host $(uname -s)"; exit 1 ;;
esac

if [[ -z "${ANDROID_NDK_HOME:-}" ]]; then
    SDK_ROOT="${ANDROID_HOME:-$HOME/Library/Android/sdk}"
    ANDROID_NDK_HOME="$(ls -d "$SDK_ROOT"/ndk/* 2>/dev/null | sort -V | tail -1 || true)"
    if [[ -z "$ANDROID_NDK_HOME" ]]; then
        echo "ERROR: no NDK found. Set ANDROID_NDK_HOME or install one through the SDK manager."
        exit 1
    fi
    export ANDROID_NDK_HOME
fi
echo "NDK: $ANDROID_NDK_HOME"

TARGET_DIR="${CARGO_TARGET_DIR:-$REPO_DIR/target}"
JNI_DIR="$ANDROID_DIR/src/main/jniLibs"
rm -rf "$JNI_DIR"

# cargo-ndk builds one Rust target per ABI. It can also copy every shared
# object it finds into a jniLibs layout, but dependency crates that declare
# a cdylib leave their own .so files behind, so pick ours by hand.
for ABI in $ABIS; do
    echo "==> Building for $ABI..."
    cargo ndk -t "$ABI" build $CARGO_FLAG -p "$CRATE" --manifest-path "$REPO_DIR/Cargo.toml"
    case "$ABI" in
        arm64-v8a) TRIPLE=aarch64-linux-android ;;
        armeabi-v7a) TRIPLE=armv7-linux-androideabi ;;
        x86_64) TRIPLE=x86_64-linux-android ;;
        *) echo "ERROR: unknown ABI '$ABI' (arm64-v8a, armeabi-v7a, x86_64)"; exit 2 ;;
    esac
    mkdir -p "$JNI_DIR/$ABI"
    cp "$TARGET_DIR/$TRIPLE/$PROFILE/$LIB" "$JNI_DIR/$ABI/$LIB"
done

# The bindings come from the host build: UniFFI reads the exported metadata
# out of the dylib. Generated in place under the module's Kotlin sources.
echo "==> Generating the Kotlin bindings..."
cargo build -p "$CRATE" $CARGO_FLAG --manifest-path "$REPO_DIR/Cargo.toml"
rm -rf "$ANDROID_DIR/src/main/kotlin"
cargo run -q -p uniffi-bindgen --manifest-path "$REPO_DIR/Cargo.toml" -- generate \
    --library "$TARGET_DIR/$PROFILE/$HOST_LIB" \
    --language kotlin --no-format \
    --out-dir "$ANDROID_DIR/src/main/kotlin"

echo "==> Done: $JNI_DIR and $ANDROID_DIR/src/main/kotlin"
