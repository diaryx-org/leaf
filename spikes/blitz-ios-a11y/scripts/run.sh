#!/bin/sh
# Build the spike for the iOS simulator, bundle it, and run it on a simulator
# made for it: `agent-blitz-a11y`, iPhone 17, iOS 27 (made if missing; delete
# it with `xcrun simctl delete agent-blitz-a11y` when done).
set -e
cd "$(dirname "$0")/.."
cargo build --target aarch64-apple-ios-sim
APP=target/BlitzA11y.app
rm -rf "$APP" && mkdir -p "$APP"
cp target/aarch64-apple-ios-sim/debug/BlitzA11y ios/Info.plist "$APP/"
codesign --force --sign - "$APP" >/dev/null 2>&1
SIM=agent-blitz-a11y
xcrun simctl list devices | grep -q "$SIM (" || \
  xcrun simctl create "$SIM" "iPhone 17" com.apple.CoreSimulator.SimRuntime.iOS-27-0
xcrun simctl bootstatus "$SIM" -b >/dev/null
xcrun simctl install "$SIM" "$APP"
xcrun simctl terminate "$SIM" org.diaryx.leaf.agent-blitz-a11y 2>/dev/null || true
xcrun simctl launch --console-pty --terminate-running-process "$SIM" org.diaryx.leaf.agent-blitz-a11y
