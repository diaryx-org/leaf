#!/usr/bin/env bash
#
# Type-check the LeafUI renderer against the real generated LeafFFI binding,
# without an Xcode project — the Swift peer of `cargo check`. Builds the host
# dylib, generates the UniFFI Swift, emits a LeafFFI .swiftmodule, then
# `-typecheck`s packages/leaf-swift/Sources/LeafUI against it. macOS only.
#
# Usage: scripts/check-swift.sh
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
WORK="${TMPDIR:-/tmp}/leaf-swift-check"
SDK="$(xcrun --show-sdk-path)"

echo "▸ Building leaf-ffi (host) + generating Swift binding…"
cargo build -p leaf-ffi --manifest-path "$ROOT/Cargo.toml" >/dev/null
DYLIB="$ROOT/target/debug/libleaf_ffi.dylib"

rm -rf "$WORK" && mkdir -p "$WORK/headers" "$WORK/gen"
cargo run -q -p leaf-ffi --manifest-path "$ROOT/Cargo.toml" --bin uniffi-bindgen -- \
  generate --library "$DYLIB" --language swift --out-dir "$WORK/gen" 2>&1 \
  | grep -vi swiftformat || true

cp "$WORK/gen/leaf_ffiFFI.h" "$WORK/headers/"
cp "$WORK/gen/leaf_ffiFFI.modulemap" "$WORK/headers/module.modulemap"

echo "▸ Emitting LeafFFI.swiftmodule…"
swiftc -emit-module -module-name LeafFFI \
  -emit-module-path "$WORK/LeafFFI.swiftmodule" \
  "$WORK/gen/leaf_ffi.swift" \
  -sdk "$SDK" \
  -I "$WORK/headers" -Xcc -fmodule-map-file="$WORK/headers/module.modulemap"

echo "▸ Type-checking LeafUI (macOS / AppKit)…"
swiftc -typecheck -module-name LeafUI \
  "$ROOT"/packages/leaf-swift/Sources/LeafUI/*.swift \
  -sdk "$SDK" \
  -I "$WORK" \
  -I "$WORK/headers" -Xcc -fmodule-map-file="$WORK/headers/module.modulemap"
echo "  ✓ macOS"

# The generated binding is arch-neutral source, but a .swiftmodule is triple-
# specific, so emit a fresh LeafFFI for the iOS-simulator triple and check the
# UIKit path against it.
SDK_IOS="$(xcrun --sdk iphonesimulator --show-sdk-path)"
TARGET_IOS="arm64-apple-ios16.0-simulator"
mkdir -p "$WORK/ios"
echo "▸ Type-checking LeafUI (iOS / UIKit)…"
swiftc -emit-module -module-name LeafFFI \
  -emit-module-path "$WORK/ios/LeafFFI.swiftmodule" \
  "$WORK/gen/leaf_ffi.swift" \
  -sdk "$SDK_IOS" -target "$TARGET_IOS" \
  -I "$WORK/headers" -Xcc -fmodule-map-file="$WORK/headers/module.modulemap"
swiftc -typecheck -module-name LeafUI \
  "$ROOT"/packages/leaf-swift/Sources/LeafUI/*.swift \
  -sdk "$SDK_IOS" -target "$TARGET_IOS" \
  -I "$WORK/ios" \
  -I "$WORK/headers" -Xcc -fmodule-map-file="$WORK/headers/module.modulemap"
echo "  ✓ iOS"

echo "✓ LeafUI type-checks against the generated LeafFFI binding (macOS + iOS)."
