#!/usr/bin/env bash
#
# First-time setup for the iOS demo app (and whenever the Rust *API* changes):
#   1. build the leaf-ffi Rust lib on the host (so uniffi-bindgen can introspect it)
#   2. generate the UniFFI Swift binding + C module into crates/leaf-ffi/generated/
#      (what Package.swift compiles: generated/Sources/LeafFFI + generated/headers)
#   3. run `xcodegen generate` to (re)create LeafEditorApp.xcodeproj
#
# The Rust *staticlib* for the simulator/device is NOT built here — the Xcode
# project's pre-build script (see project.yml) does that on every build, so
# ordinary Rust edits need only ⌘R in Xcode (or the xcodebuild line below). Re-run
# this script only after changing the Rust API surface (new/renamed FFI methods),
# which requires regenerating the binding.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../../../.." && pwd)"        # repo root
OUT="$ROOT/crates/leaf-ffi/generated"

echo "▸ Building leaf-ffi (host) for bindgen introspection…"
cargo build --manifest-path "$ROOT/Cargo.toml" -p leaf-ffi

echo "▸ Generating UniFFI Swift binding + C module…"
rm -rf "$OUT" && mkdir -p "$OUT/Sources/LeafFFI" "$OUT/headers" "$OUT/tmp"
cargo run -q --manifest-path "$ROOT/Cargo.toml" -p leaf-ffi --bin uniffi-bindgen -- \
  generate --library "$ROOT/target/debug/libleaf_ffi.dylib" \
  --language swift --out-dir "$OUT/tmp" 2>&1 | grep -vi swiftformat || true
mv "$OUT/tmp/leaf_ffi.swift" "$OUT/Sources/LeafFFI/leaf_ffi.swift"
cp "$OUT/tmp/leaf_ffiFFI.h" "$OUT/headers/"
cp "$OUT/tmp/leaf_ffiFFI.modulemap" "$OUT/headers/module.modulemap"
rm -rf "$OUT/tmp"

echo "▸ Generating Xcode project…"
cd "$HERE" && xcodegen generate

echo "✓ Ready. Build & run in the simulator:"
echo "    cd $HERE"
echo "    xcodebuild -project LeafEditorApp.xcodeproj -scheme LeafEditorApp \\"
echo "      -destination 'platform=iOS Simulator,name=iPhone 17' \\"
echo "      -derivedDataPath build/DD build"
