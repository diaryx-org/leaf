#!/usr/bin/env bash
#
# Run the LeafUI unit tests on an iOS simulator — the UIKit peer of
# `test-swift.sh`. Most of the suite is toolkit-neutral geometry and runs the
# same either way; what this exists for is the `#if canImport(UIKit)` halves,
# which `swift test` on a Mac never compiles: the UIKit text view, and a PDF
# drawn through `UIGraphicsPDFRenderer`.
#
# Same shape as the macOS script: build the Rust staticlib for the simulator,
# then run the package's tests with it force-loaded. `xcodebuild` rather than
# `swift test`, since only Xcode can put a package's test bundle on a simulator.
#
# Usage: scripts/test-swift-ios.sh [device name]   (default: the first iPhone)
#
# The device goes to xcodebuild by id, not by name: a name resolves against
# "the latest OS", and a simulator whose runtime is not the newest installed
# is then "not found" even though `simctl` lists it as available.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TARGET=aarch64-apple-ios-sim
DEVICE="${1:-$(xcrun simctl list devices available | sed -n 's/^ *\(iPhone[^(]*\) (.*/\1/p' | head -1 | sed 's/ *$//')}"
[ -n "$DEVICE" ] || { echo "no iPhone simulator available"; exit 1; }
UDID="$(xcrun simctl list devices available | grep -F "$DEVICE (" | head -1 | sed -n 's/.*(\([0-9A-F-]*\)) (.*/\1/p')"
[ -n "$UDID" ] || { echo "no simulator named \`$DEVICE\`"; exit 1; }

echo "▸ Building leaf-ffi ($TARGET) staticlib…"
rustup target list --installed | grep -q "$TARGET" || rustup target add "$TARGET"
cargo build -p leaf-ffi --target "$TARGET" --manifest-path "$ROOT/Cargo.toml" >/dev/null
STATIC="$ROOT/target/$TARGET/debug/libleaf_ffi.a"
[ -f "$STATIC" ] || { echo "missing $STATIC"; exit 1; }

echo "▸ xcodebuild test (LeafUI) on the \`$DEVICE\` simulator…"
xcodebuild test -quiet \
  -scheme LeafFFI-Package \
  -destination "platform=iOS Simulator,id=$UDID" \
  -derivedDataPath "$ROOT/target/xcode-ios-tests" \
  OTHER_LDFLAGS="-Xlinker -force_load -Xlinker $STATIC" \
  -only-testing LeafUITests \
  "${@:2}"
