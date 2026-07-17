#!/usr/bin/env bash
#
# Build LeafFFI.xcframework from crates/leaf-ffi and generate the Swift bindings
# alongside it — the one artifact a macOS/iOS app links to drive leaf-core.
#
# Output (under crates/leaf-ffi/generated/, git-ignored):
#   LeafFFI.xcframework/    the static libs for every Apple slice + C headers
#   Sources/LeafFFI/        the generated Swift (leaf_ffi.swift)
#
# The bundled Swift package manifest (crates/leaf-ffi/Package.swift) points at
# both, so an app just adds this directory as a local Swift package.
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
CRATE="$ROOT/crates/leaf-ffi"
OUT="$CRATE/generated"
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

echo "▸ Generating Swift bindings…"
rm -rf "$OUT"
mkdir -p "$OUT/Sources/LeafFFI" "$OUT/headers"
# Introspect any freshly-built dylib/staticlib for the metadata. A staticlib is
# fine here — the generator reads the embedded UniFFI component metadata.
LIB_FOR_GEN="$TARGET_DIR/$IOS_DEVICE_ARCH/$PROFILE/$LIB_BASENAME"
cargo run -q -p leaf-ffi --bin uniffi-bindgen -- \
  generate --library "$LIB_FOR_GEN" --language swift --out-dir "$OUT/gen-tmp"

# Split the generator's output: the .swift goes in the SPM source dir, the C
# header + modulemap go in the xcframework's Headers (renamed to the name
# `-create-xcframework` expects).
mv "$OUT/gen-tmp/leaf_ffi.swift" "$OUT/Sources/LeafFFI/leaf_ffi.swift"
cp "$OUT/gen-tmp"/leaf_ffiFFI.h "$OUT/headers/"
cp "$OUT/gen-tmp"/leaf_ffiFFI.modulemap "$OUT/headers/module.modulemap"
rm -rf "$OUT/gen-tmp"

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
  -library "$OUT/lipo/macos/$LIB_BASENAME"   -headers "$OUT/headers" \
  -library "$OUT/lipo/ios-sim/$LIB_BASENAME" -headers "$OUT/headers" \
  -library "$TARGET_DIR/$IOS_DEVICE_ARCH/$PROFILE/$LIB_BASENAME" -headers "$OUT/headers" \
  -output "$OUT/LeafFFI.xcframework"

rm -rf "$OUT/lipo" "$OUT/headers"
echo "✓ Done:"
echo "    $OUT/LeafFFI.xcframework"
echo "    $OUT/Sources/LeafFFI/leaf_ffi.swift"
echo "  Add crates/leaf-ffi (Package.swift) as a local Swift package to consume it."
