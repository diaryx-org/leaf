#!/usr/bin/env bash
# Screenshot a GUI binary on a headless machine, so the two renderers in
# README.md's "Webview against native" section are captured the same way: same
# display, same size, same document, same wait.
#
#   scripts/shoot.sh <out.png> <window-title-substring> -- <command> [args...]
#
# Needs Xvfb, xdotool and ImageMagick's `import`. On Linux gpui draws through
# Vulkan, so it also needs a Vulkan driver — lavapipe (`mesa-vulkan-drivers`)
# is enough, and is what the README's numbers were taken with.
set -euo pipefail

OUT=$1
TITLE=$2
shift 2
[ "${1:-}" = "--" ] && shift

DISP=":99"
GEOMETRY="1280x900x24"
# Long enough for a cold webview or a software-rendered GPU surface to paint.
SETTLE=6

Xvfb "$DISP" -screen 0 "$GEOMETRY" >/dev/null 2>&1 &
XVFB=$!
cleanup() {
    [ -n "${APP:-}" ] && kill "$APP" 2>/dev/null || true
    kill "$XVFB" 2>/dev/null || true
}
trap cleanup EXIT
sleep 2

export DISPLAY="$DISP"
# Force X11: tao and gpui both prefer Wayland when a socket is present, and
# there is none here, but being explicit keeps the two runs alike.
export GDK_BACKEND=x11
# Without a UTF-8 locale, X cannot convert a window title containing anything
# but ASCII, and `xdotool search --name` then matches nothing at all.
export LC_ALL=C.UTF-8
export LANG=C.UTF-8
unset WAYLAND_DISPLAY

"$@" &
APP=$!

WID=""
for _ in $(seq 1 40); do
    WID=$(xdotool search --name "$TITLE" 2>/dev/null | tail -1 || true)
    [ -n "$WID" ] && break
    # A crash is worth reporting as one, rather than timing out silently.
    kill -0 "$APP" 2>/dev/null || {
        echo "shoot: $1 exited before opening a window" >&2
        exit 1
    }
    sleep 1
done

if [ -z "$WID" ]; then
    echo "shoot: no window matching '$TITLE' after 40s" >&2
    exit 1
fi

# Normalise the geometry: the webview host asks for 1000x700 itself, but
# dioxus-native takes its size from winit's defaults, and two screenshots of
# different shapes are not a comparison.
xdotool windowsize "$WID" 1000 700 2>/dev/null || true
sleep "$SETTLE"
# `import -window` on the window itself, not the root: the root is the whole
# 1280x900 virtual screen, and without a window manager the app's window is
# not necessarily at its origin.
import -window "$WID" -define png:color-type=2 "$OUT"
echo "shoot: wrote $OUT ($(identify -format '%wx%h' "$OUT"))"
