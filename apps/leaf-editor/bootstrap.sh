#!/usr/bin/env bash
#
# First-time setup for the cross-platform editor app (and whenever the Rust *API*
# changes):
#   1. regenerate the committed UniFFI binding under
#      packages/leaf-swift/uniffi-generated/ (scripts/gen-bindings.sh — after an
#      API change, `git diff` shows the binding move and the change is committed
#      with it; CI diffs the two, so a stale binding cannot merge)
#   2. run `xcodegen generate` to (re)create LeafEditorApp.xcodeproj
#
# The Rust *staticlib* for mac/simulator/device is NOT built here — the Xcode
# project's pre-build script (see project.yml) does that on every build, so
# ordinary Rust edits need only ⌘R in Xcode (or the xcodebuild lines below). Re-run
# this script only after changing the Rust API surface (new/renamed FFI methods),
# which requires regenerating the binding.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"        # repo root

"$ROOT/scripts/gen-bindings.sh"

echo "▸ Generating Xcode project…"
cd "$HERE" && xcodegen generate

echo "✓ Ready."
echo "  Run on macOS:"
echo "    xcodebuild -project $HERE/LeafEditorApp.xcodeproj -scheme LeafEditorApp \\"
echo "      -destination 'platform=macOS' -derivedDataPath build/DD build"
echo "  Run in the iOS simulator:"
echo "    xcodebuild -project $HERE/LeafEditorApp.xcodeproj -scheme LeafEditorApp \\"
echo "      -destination 'platform=iOS Simulator,name=iPhone 17' -derivedDataPath build/DD build"
