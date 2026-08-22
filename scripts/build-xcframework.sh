#!/usr/bin/env bash
#
# Build LeafFFI.xcframework from crates/leaf-ffi — the distributable prebuilt
# binary for a consumer who doesn't build Rust. The Swift package itself no
# longer involves it: the committed binding under
# packages/leaf-swift/uniffi-generated/ (kept fresh here via gen-bindings.sh) is
# what the root Package.swift compiles, and the app links the staticlib.
#
# Output (under target/xcframework/, git-ignored):
#   LeafFFI.xcframework/    the static libs for every Apple slice + C headers
#
# Prereqs:
#   rustup target add \
#     aarch64-apple-darwin x86_64-apple-darwin \
#     aarch64-apple-ios aarch64-apple-ios-sim x86_64-apple-ios-sim
#   Xcode command-line tools (xcodebuild, lipo).
#
# Usage: scripts/build-xcframework.sh [--debug]   (default: release)
set -euo pipefail

PROFILE="release"
CARGO_PROFILE_FLAG="--release"
if [[ "${1:-}" == "--debug" ]]; then
  PROFILE="debug"
  CARGO_PROFILE_FLAG=""
fi

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OUT="$ROOT/target/xcframework"
GEN="$ROOT/packages/leaf-swift/uniffi-generated"
LIB_BASENAME="libleaf_ffi.a"       # staticlib output name for the crate
TARGET_DIR="$ROOT/target"

# The Apple slices we ship. macOS and the iOS simulator each fatten two arches
# into one static lib via `lipo`; the iOS device slice is a single arch.
MACOS_ARCHES=(aarch64-apple-darwin x86_64-apple-darwin)
IOS_SIM_ARCHES=(aarch64-apple-ios-sim x86_64-apple-ios-sim)
IOS_DEVICE_ARCH=aarch64-apple-ios
ALL_ARCHES=("${MACOS_ARCHES[@]}" "${IOS_SIM_ARCHES[@]}" "$IOS_DEVICE_ARCH")

echo "▸ Building leaf-ffi staticlib for ${#ALL_ARCHES[@]} Apple targets ($PROFILE)…"
for target in "${ALL_ARCHES[@]}"; do
  echo "  · $target"
  cargo build -p leaf-ffi $CARGO_PROFILE_FLAG --target "$target"
done

# Refresh the committed binding so the xcframework's headers and the package's
# Swift are one generator run — a drift here is what CI's --check would catch.
"$ROOT/scripts/gen-bindings.sh"
rm -rf "$OUT"
mkdir -p "$OUT"

# Fatten the multi-arch slices.
echo "▸ Fattening universal slices with lipo…"
mkdir -p "$OUT/lipo/macos" "$OUT/lipo/ios-sim"
lipo -create -output "$OUT/lipo/macos/$LIB_BASENAME" \
  "${MACOS_ARCHES[@]/#/$TARGET_DIR/}" 2>/dev/null || \
  lipo -create -output "$OUT/lipo/macos/$LIB_BASENAME" \
    $(printf "$TARGET_DIR/%s/$PROFILE/$LIB_BASENAME " "${MACOS_ARCHES[@]}")
lipo -create -output "$OUT/lipo/ios-sim/$LIB_BASENAME" \
  $(printf "$TARGET_DIR/%s/$PROFILE/$LIB_BASENAME " "${IOS_SIM_ARCHES[@]}")

# Assemble the xcframework: one -library/-headers pair per platform slice.
echo "▸ Assembling xcframework…"
rm -rf "$OUT/LeafFFI.xcframework"
xcodebuild -create-xcframework \
  -library "$OUT/lipo/macos/$LIB_BASENAME"   -headers "$GEN/headers" \
  -library "$OUT/lipo/ios-sim/$LIB_BASENAME" -headers "$GEN/headers" \
  -library "$TARGET_DIR/$IOS_DEVICE_ARCH/$PROFILE/$LIB_BASENAME" -headers "$GEN/headers" \
  -output "$OUT/LeafFFI.xcframework"

rm -rf "$OUT/lipo"
echo "✓ Done:"
echo "    $OUT/LeafFFI.xcframework"
echo "  The Swift package (root Package.swift) is consumed separately; the"
echo "  xcframework is only for consumers who don't build the Rust staticlib."
