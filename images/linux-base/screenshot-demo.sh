#!/usr/bin/env bash
# Brings up a Desktop, opens the browser on it, and captures what is on screen.
#
# This is what "looking at a Desktop" amounts to until §6.3's stream and §7.5's
# viewer exist. The capture goes through `iapetusd --screenshot`, which uses the
# same driver and encoder an agent's `screenshot` action would, so the image is
# what an agent would have been handed rather than a separate debug path.
set -euo pipefail

OUT="${1:-/out/desktop.png}"

Xvfb "${DISPLAY}" -screen 0 "${SCREEN_GEOMETRY}" -nolisten tcp 2>/dev/null &
for _ in $(seq 1 100); do
    xdpyinfo -display "${DISPLAY}" >/dev/null 2>&1 && break
    sleep 0.1
done
xdpyinfo -display "${DISPLAY}" >/dev/null 2>&1 || { echo "X server did not start" >&2; exit 1; }

# Without a window manager nothing maps windows or assigns focus, so the
# browser would open and never receive a keystroke.
openbox >/dev/null 2>&1 &
sleep 0.5

cd /src
cargo build -q -p iapetusd --features x11

BIN=target/debug/iapetusd

# Launch through the catalog, exactly as `app.launch` with key "chrome" would.
"$BIN" --version
/usr/bin/chromium \
    --no-sandbox --disable-dev-shm-usage --disable-gpu \
    --no-first-run --no-default-browser-check \
    --window-size=1600,900 \
    "file:///opt/iapetus/s1.html" >/dev/null 2>&1 &

# Wait for the page rather than sleeping a guessed interval: the window is
# mapped well before the renderer has drawn anything, and capturing in between
# yields a white rectangle that looks like a broken capture.
title_seen() {
    # x11-utils only: read the client list off the root window, then ask each
    # window for its EWMH name. Avoids adding xdotool for one string match.
    local ids
    ids=$(xprop -root _NET_CLIENT_LIST 2>/dev/null | sed 's/.*# //; s/,//g') || return 1
    for id in $ids; do
        case "$id" in 0x*) ;; *) continue ;; esac
        if xprop -id "$id" _NET_WM_NAME 2>/dev/null | grep -q "$1"; then return 0; fi
    done
    return 1
}

for _ in $(seq 1 150); do
    title_seen "S1 READY" && break
    sleep 0.2
done
# The title appears when the document parses; give the renderer a moment to
# actually paint, or the capture is a correct picture of a blank page.
sleep 2

mkdir -p "$(dirname "$OUT")"
"$BIN" --screenshot "$OUT"
