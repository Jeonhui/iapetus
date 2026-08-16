#!/usr/bin/env bash
# The desktop service: an X session with a browser on it, streamed to the
# gateway. This is one Desktop (§5.3) — a persistent virtual computer an agent
# and a human co-own.
set -euo pipefail

GATEWAY_INGEST="${IAPETUS_GATEWAY_INGEST:-ws://gateway:8080/ingest?token=dev}"

Xvfb "${DISPLAY}" -screen 0 "${SCREEN_GEOMETRY}" -nolisten tcp &
for _ in $(seq 1 100); do
    xdpyinfo -display "${DISPLAY}" >/dev/null 2>&1 && break
    sleep 0.1
done
xdpyinfo -display "${DISPLAY}" >/dev/null 2>&1 || { echo "X server failed to start" >&2; exit 1; }

# A minimal window manager — §12.5 lever 2: the agent needs window management,
# not a full desktop environment.
openbox &
sleep 0.5

# A browser open on the welcome page, so a viewer sees something immediately.
# An agent would launch this through app.launch("chrome") instead.
chromium --no-sandbox --disable-dev-shm-usage --disable-gpu \
    --no-first-run --no-default-browser-check --window-size=1600,900 \
    "file:///opt/iapetus/s1.html" >/dev/null 2>&1 &

echo "desktop up; streaming to ${GATEWAY_INGEST}"
# Reconnects on its own if the gateway is not up yet (§19.5 backoff).
exec iapetusd --stream "${GATEWAY_INGEST}"
