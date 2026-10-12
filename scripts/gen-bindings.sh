#!/usr/bin/env bash
#
# (Re)generate the UniFFI Swift bindings committed under
# packages/leaf-swift/uniffi-generated/ and apps/leaf-editor/LeafMermaid/ — or,
# with --check, verify that the committed bindings are byte-for-byte what
# crates/leaf-ffi produces. The second is leaf-mermaid-ffi's, which rides in
# leaf-ffi's archive only under its `mermaid` feature; the leaf-editor app is
# the one host that builds it.
#
# The binding is committed rather than built on demand because the Swift package
# is consumed by version from a bare git checkout (SwiftPM runs no generators),
# so a clone must build as-is. CI runs the --check on every push; after changing
# the FFI surface, run this and commit what it writes.
#
# Usage: scripts/gen-bindings.sh [--check]
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OUT="$ROOT/packages/leaf-swift/uniffi-generated"

echo "▸ Building leaf-ffi (host, with Mermaid) for bindgen introspection…"
cargo build --manifest-path "$ROOT/Cargo.toml" -p leaf-ffi --features mermaid

# The host cdylib the generator introspects: .dylib on macOS, .so on Linux. The
# generated Swift is identical either way — it comes from the embedded UniFFI
# metadata, not from the machine code — which is what lets CI check on Linux.
LIB="$ROOT/target/debug/libleaf_ffi.dylib"
[ -f "$LIB" ] || LIB="$ROOT/target/debug/libleaf_ffi.so"
[ -f "$LIB" ] || { echo "gen-bindings: no libleaf_ffi.{dylib,so} under target/debug" >&2; exit 1; }

STAGE="$(mktemp -d)"
trap 'rm -rf "$STAGE"' EXIT

echo "▸ Generating UniFFI Swift binding + C module…"
mkdir -p "$STAGE/out/Sources/LeafFFI" "$STAGE/out/headers"
# --no-format: the committed output must not depend on whether this machine
# happens to have swiftformat on PATH — CI's runner never does.
cargo run -q --manifest-path "$ROOT/Cargo.toml" -p leaf-ffi --features mermaid --bin uniffi-bindgen -- \
  generate "$LIB" --language swift --no-format --out-dir "$STAGE/gen"
mv "$STAGE/gen/leaf_ffi.swift" "$STAGE/out/Sources/LeafFFI/leaf_ffi.swift"
cp "$STAGE/gen/leaf_ffiFFI.h" "$STAGE/out/headers/"
cp "$STAGE/gen/leaf_ffiFFI.modulemap" "$STAGE/out/headers/module.modulemap"

# leaf-mermaid-ffi's module, beside the app's own Package.swift for it.
MERMAID="$ROOT/apps/leaf-editor/LeafMermaid"
mkdir -p "$STAGE/mermaid/Sources/LeafMermaidFFI" "$STAGE/mermaid/headers"
mv "$STAGE/gen/leaf_mermaid_ffi.swift" "$STAGE/mermaid/Sources/LeafMermaidFFI/leaf_mermaid_ffi.swift"
cp "$STAGE/gen/leaf_mermaid_ffiFFI.h" "$STAGE/mermaid/headers/"
cp "$STAGE/gen/leaf_mermaid_ffiFFI.modulemap" "$STAGE/mermaid/headers/module.modulemap"

# Compare or write one generated tree; the Mermaid one shares its directory
# with a hand-written Package.swift, so only the generated parts are touched.
sync() {
  local have="$1" want="$2" what="$3"
  shift 3
  if [ "$CHECK" = 1 ]; then
    for part in "$@"; do
      if ! diff -ru "$have/$part" "$want/$part"; then
        echo "✗ $have/$part is stale — run scripts/gen-bindings.sh and commit the result" >&2
        STALE=1
      fi
    done
    [ "$STALE" = 1 ] || echo "✓ committed $what binding matches"
  else
    for part in "$@"; do
      rm -rf "${have:?}/$part"
      mkdir -p "$(dirname "$have/$part")"
      cp -R "$want/$part" "$have/$part"
    done
    echo "✓ regenerated $have"
  fi
}

CHECK=0; STALE=0
[ "${1:-}" = "--check" ] && CHECK=1
sync "$OUT" "$STAGE/out" "crates/leaf-ffi" Sources headers
sync "$MERMAID" "$STAGE/mermaid" "crates/leaf-mermaid-ffi" Sources headers
[ "$STALE" = 0 ] || exit 1
