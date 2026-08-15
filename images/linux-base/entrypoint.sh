#!/usr/bin/env bash
# Starts the display session, then hands off to iapetusd.
#
# iapetusd supervises the session itself rather than delegating to supervisord
# (§19.2): it needs to own the process tree for app.list and process.kill to be
# accurate about what is actually running.
set -euo pipefail

Xvfb "${DISPLAY}" -screen 0 "${SCREEN_GEOMETRY}" -nolisten tcp &
XVFB_PID=$!

# Wait for the X server rather than sleeping a fixed interval — a fixed sleep is
# the flakiness §15.3 warns about.
for _ in $(seq 1 50); do
    if xdpyinfo -display "${DISPLAY}" >/dev/null 2>&1; then break; fi
    sleep 0.1
done
xdpyinfo -display "${DISPLAY}" >/dev/null 2>&1 || { echo "X server failed to start" >&2; exit 1; }

# A minimal window manager, not a full desktop environment. §12.5 lever 2:
# the agent only needs window management, and the 300-400MB of XFCE is started
# lazily when a human actually attaches.
openbox &

exec "$@"
