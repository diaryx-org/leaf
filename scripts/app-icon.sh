#!/usr/bin/env bash
#
# Rasterise apps/leaf-editor/App/Icon.svg into the app's asset catalog: every
# size a macOS icon needs, and the single 1024 iOS takes. Run after editing
# the SVG; the PNGs are committed, so a build needs no rsvg-convert.
#
# Usage: scripts/app-icon.sh
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
APP="$HERE/apps/leaf-editor/App"
SET="$APP/Assets.xcassets/AppIcon.appiconset"
command -v rsvg-convert >/dev/null || { echo "rsvg-convert is needed: brew install librsvg" >&2; exit 1; }

mkdir -p "$SET"
for px in 16 32 64 128 256 512 1024; do
  rsvg-convert -w "$px" -h "$px" "$APP/Icon.svg" -o "$SET/icon_$px.png"
done

cat > "$SET/Contents.json" <<'JSON'
{
  "images" : [
    { "idiom" : "mac", "size" : "16x16",   "scale" : "1x", "filename" : "icon_16.png" },
    { "idiom" : "mac", "size" : "16x16",   "scale" : "2x", "filename" : "icon_32.png" },
    { "idiom" : "mac", "size" : "32x32",   "scale" : "1x", "filename" : "icon_32.png" },
    { "idiom" : "mac", "size" : "32x32",   "scale" : "2x", "filename" : "icon_64.png" },
    { "idiom" : "mac", "size" : "128x128", "scale" : "1x", "filename" : "icon_128.png" },
    { "idiom" : "mac", "size" : "128x128", "scale" : "2x", "filename" : "icon_256.png" },
    { "idiom" : "mac", "size" : "256x256", "scale" : "1x", "filename" : "icon_256.png" },
    { "idiom" : "mac", "size" : "256x256", "scale" : "2x", "filename" : "icon_512.png" },
    { "idiom" : "mac", "size" : "512x512", "scale" : "1x", "filename" : "icon_512.png" },
    { "idiom" : "mac", "size" : "512x512", "scale" : "2x", "filename" : "icon_1024.png" },
    { "idiom" : "universal", "platform" : "ios", "size" : "1024x1024", "filename" : "icon_1024.png" }
  ],
  "info" : { "author" : "xcode", "version" : 1 }
}
JSON
echo "✓ $SET"
