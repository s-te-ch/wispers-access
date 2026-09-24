#!/bin/bash
set -euo pipefail

# Builds the SDK for Apple platforms and packages it for Swift Package
# Manager: the Rust static library for iOS devices, the iOS simulator and
# macOS as an XCFramework, plus the Swift bindings UniFFI generates from it.
#
# Output, next to this script (both git-ignored):
#   WispersAccessSdkFfi.xcframework          the C module: library + header
#   Sources/WispersAccessSdk/WispersAccessSdk.swift   the Swift API over it
#
# Prerequisites:
#   - Xcode with the iOS SDK
#   - rustup target add aarch64-apple-ios aarch64-apple-ios-sim aarch64-apple-darwin
#
# Usage: ./build-xcframework.sh [--release]

SWIFT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_DIR="$(cd "$SWIFT_DIR/../.." && pwd)"
CRATE=wispers-access-sdk
LIB=libwispers_access_sdk.a
FFI_MODULE=WispersAccessSdkFfi

PROFILE="debug"
CARGO_FLAG=""
if [[ "${1:-}" == "--release" ]]; then
    PROFILE="release"
    CARGO_FLAG="--release"
fi

# Stamp the object files with the deployment targets Package.swift
# advertises. Without this they inherit the SDK's default and every link
# against them warns about a newer platform version. Cargo passes these on
# to the C and C++ builds (BoringSSL, libjuice) too.
export IPHONEOS_DEPLOYMENT_TARGET=17.0
export MACOSX_DEPLOYMENT_TARGET=14.0

for SDK in iphoneos iphonesimulator macosx; do
    if ! xcrun --sdk "$SDK" --show-sdk-path >/dev/null 2>&1; then
        echo "ERROR: no $SDK SDK. Is Xcode installed and selected (xcode-select)?"
        exit 1
    fi
done

TARGET_DIR="${CARGO_TARGET_DIR:-$REPO_DIR/target}"
BUILD_DIR="$SWIFT_DIR/build"
rm -rf "$BUILD_DIR"
mkdir -p "$BUILD_DIR"

# One slice per platform, named as the XCFramework names them. (No
# associative arrays: macOS ships bash 3.2.)
rust_target() {
    case "$1" in
        ios) echo aarch64-apple-ios ;;
        ios-simulator) echo aarch64-apple-ios-sim ;;
        macos) echo aarch64-apple-darwin ;;
    esac
}
for PLATFORM in ios ios-simulator macos; do
    TARGET="$(rust_target "$PLATFORM")"
    echo "==> Building for $TARGET ($PLATFORM)..."
    cargo build -p "$CRATE" --target "$TARGET" $CARGO_FLAG --manifest-path "$REPO_DIR/Cargo.toml"
    mkdir -p "$BUILD_DIR/$PLATFORM"
    cp "$TARGET_DIR/$TARGET/$PROFILE/$LIB" "$BUILD_DIR/$PLATFORM/$LIB"
done

# The bindings come from the host build: UniFFI reads the exported
# metadata out of the dylib. The Swift file goes to the package's source
# target, the header and module map into every slice.
echo "==> Generating the Swift bindings..."
cargo build -p "$CRATE" $CARGO_FLAG --manifest-path "$REPO_DIR/Cargo.toml"
cargo run -q -p uniffi-bindgen --manifest-path "$REPO_DIR/Cargo.toml" -- generate \
    --library "$TARGET_DIR/$PROFILE/libwispers_access_sdk.dylib" \
    --language swift --no-format \
    --out-dir "$BUILD_DIR/bindings"
mkdir -p "$SWIFT_DIR/Sources/WispersAccessSdk"
cp "$BUILD_DIR/bindings/WispersAccessSdk.swift" "$SWIFT_DIR/Sources/WispersAccessSdk/"
for PLATFORM in ios ios-simulator macos; do
    mkdir -p "$BUILD_DIR/$PLATFORM/Headers"
    cp "$BUILD_DIR/bindings/$FFI_MODULE.h" "$BUILD_DIR/$PLATFORM/Headers/"
    cp "$BUILD_DIR/bindings/$FFI_MODULE.modulemap" "$BUILD_DIR/$PLATFORM/Headers/module.modulemap"
done

echo "==> Creating the XCFramework..."
rm -rf "$SWIFT_DIR/$FFI_MODULE.xcframework"
xcodebuild -create-xcframework \
    -library "$BUILD_DIR/ios/$LIB" -headers "$BUILD_DIR/ios/Headers" \
    -library "$BUILD_DIR/ios-simulator/$LIB" -headers "$BUILD_DIR/ios-simulator/Headers" \
    -library "$BUILD_DIR/macos/$LIB" -headers "$BUILD_DIR/macos/Headers" \
    -output "$SWIFT_DIR/$FFI_MODULE.xcframework"

rm -rf "$BUILD_DIR"
echo "==> Done: $SWIFT_DIR/$FFI_MODULE.xcframework"
