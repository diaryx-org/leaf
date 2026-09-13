#!/usr/bin/env bash
#
# Type-check the LeafUI renderer against the real generated LeafFFI binding,
# without an Xcode project — the Swift peer of `cargo check`. Builds the host
# dylib, generates the UniFFI Swift, emits a LeafFFI .swiftmodule, then
# `-typecheck`s packages/leaf-swift/Sources/LeafUI against it, for the macOS
# and the iOS-simulator triple.
#
# LeafUI also imports the resvg-swift package (ResvgFFI, the committed
# binding, and ResvgCoreGraphics over it), so those two modules are emitted
# the same way from wherever SwiftPM resolved the package to — its checkout
# under .build/, or a local path.
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

echo "▸ Locating the resvg-swift package…"
swift package resolve --package-path "$ROOT" >/dev/null
RESVG="$(swift package show-dependencies --package-path "$ROOT" --format json \
  | python3 -c 'import json,sys; print(next(d["path"] for d in json.load(sys.stdin)["dependencies"] if d["name"] == "ResvgSwift"))')"
RESVG_HEADERS="$RESVG/packages/resvg-swift/uniffi-generated/headers"
[ -f "$RESVG_HEADERS/module.modulemap" ] || { echo "no resvg-swift binding at $RESVG" >&2; exit 1; }

# Emit ResvgFFI and ResvgCoreGraphics for one triple into $1, given the SDK
# and target flags that follow.
emit_resvg() {
  local out="$1"; shift
  mkdir -p "$out"
  swiftc -emit-module -module-name ResvgFFI \
    -emit-module-path "$out/ResvgFFI.swiftmodule" \
    "$RESVG"/packages/resvg-swift/uniffi-generated/Sources/ResvgFFI/*.swift \
    "$@" -I "$RESVG_HEADERS" -Xcc -fmodule-map-file="$RESVG_HEADERS/module.modulemap"
  swiftc -emit-module -module-name ResvgCoreGraphics \
    -emit-module-path "$out/ResvgCoreGraphics.swiftmodule" \
    "$RESVG"/packages/resvg-swift/Sources/ResvgCoreGraphics/*.swift \
    "$@" -I "$out" -I "$RESVG_HEADERS" -Xcc -fmodule-map-file="$RESVG_HEADERS/module.modulemap"
}

echo "▸ Emitting LeafFFI, ResvgFFI, ResvgCoreGraphics .swiftmodules…"
swiftc -emit-module -module-name LeafFFI \
  -emit-module-path "$WORK/LeafFFI.swiftmodule" \
  "$WORK/gen/leaf_ffi.swift" \
  -sdk "$SDK" \
  -I "$WORK/headers" -Xcc -fmodule-map-file="$WORK/headers/module.modulemap"
emit_resvg "$WORK" -sdk "$SDK"

echo "▸ Type-checking LeafUI (macOS / AppKit)…"
swiftc -typecheck -module-name LeafUI \
  "$ROOT"/packages/leaf-swift/Sources/LeafUI/*.swift \
  -sdk "$SDK" \
  -I "$WORK" \
  -I "$WORK/headers" -Xcc -fmodule-map-file="$WORK/headers/module.modulemap" \
  -I "$RESVG_HEADERS" -Xcc -fmodule-map-file="$RESVG_HEADERS/module.modulemap"
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
emit_resvg "$WORK/ios" -sdk "$SDK_IOS" -target "$TARGET_IOS"
swiftc -typecheck -module-name LeafUI \
  "$ROOT"/packages/leaf-swift/Sources/LeafUI/*.swift \
  -sdk "$SDK_IOS" -target "$TARGET_IOS" \
  -I "$WORK/ios" \
  -I "$WORK/headers" -Xcc -fmodule-map-file="$WORK/headers/module.modulemap" \
  -I "$RESVG_HEADERS" -Xcc -fmodule-map-file="$RESVG_HEADERS/module.modulemap"
echo "  ✓ iOS"

echo "✓ LeafUI type-checks against the generated LeafFFI binding (macOS + iOS)."
