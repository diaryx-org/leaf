#!/usr/bin/env bash
#
# Run the LeafUI renderer unit tests (macOS host). The peer of `check-swift.sh`,
# but this compiles and *runs* an XCTest bundle rather than just type-checking.
#
# The tests build `Row`/`DocView` fixtures in pure Swift and assert on the
# CoreText geometry + attribute mapping, so they need no Rust runtime — but the
# `LeafFFI` module they import still references the FFI symbols, so the test
# binary must link the Rust staticlib. We force-load it (the same way the app does
# in `apps/leaf-editor/project.yml`), which also avoids any runtime rpath dance.
#
# Usage: scripts/test-swift.sh [extra `swift test` args…]
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

echo "▸ Building leaf-ffi (host) staticlib…"
cargo build -p leaf-ffi --manifest-path "$ROOT/Cargo.toml" >/dev/null
STATIC="$ROOT/target/debug/libleaf_ffi.a"
[ -f "$STATIC" ] || { echo "missing $STATIC"; exit 1; }

echo "▸ swift test (LeafUI)…"
swift test --package-path "$ROOT/packages/leaf-swift" \
  -Xlinker -force_load -Xlinker "$STATIC" "$@"
