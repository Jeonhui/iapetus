#!/usr/bin/env bash
# Brings up an X server, then runs the L2 suite against it.
set -euo pipefail

Xvfb "${DISPLAY}" -screen 0 "${SCREEN_GEOMETRY}" -nolisten tcp 2>/dev/null &

for _ in $(seq 1 60); do
    xdpyinfo -display "${DISPLAY}" >/dev/null 2>&1 && break
    sleep 0.1
done
xdpyinfo -display "${DISPLAY}" >/dev/null 2>&1 || { echo "X server did not start" >&2; exit 1; }

# A window manager is required for these tests: without one, nothing maps
# windows or assigns focus, and synthetic key events reach no client.
openbox >/dev/null 2>&1 &
sleep 0.5

cd /src
exec cargo test -p iapetusd --features x11 -- --test-threads=1 --nocapture
