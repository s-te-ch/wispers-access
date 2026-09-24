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
# Usage: ./build-jnilibs.sh [--debug]

ANDROID_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_DIR="$(cd "$ANDROID_DIR/../.." && pwd)"
CRATE=wispers-access-sdk
LIB=libwispers_access_sdk.so
ABIS="arm64-v8a armeabi-v7a x86_64"

PROFILE="release"
CARGO_FLAG="--release"
if [[ "${1:-}" == "--debug" ]]; then
    PROFILE="debug"
    CARGO_FLAG=""
fi

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
    --library "$TARGET_DIR/$PROFILE/libwispers_access_sdk.dylib" \
    --language kotlin --no-format \
    --out-dir "$ANDROID_DIR/src/main/kotlin"

echo "==> Done: $JNI_DIR and $ANDROID_DIR/src/main/kotlin"
