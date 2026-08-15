#!/usr/bin/env bash
# Brings up a Desktop with a browser on it and streams it to the gateway.
set -euo pipefail
Xvfb "${DISPLAY}" -screen 0 "${SCREEN_GEOMETRY}" -nolisten tcp 2>/dev/null &
for _ in $(seq 1 100); do xdpyinfo -display "${DISPLAY}" >/dev/null 2>&1 && break; sleep 0.1; done
openbox >/dev/null 2>&1 &
sleep 0.5
cd /src
cargo build -q -p iapetusd --features x11 -p iapetus-gateway

IAPETUS_GATEWAY_BIND=0.0.0.0:8080 target/debug/iapetus-gateway &
sleep 1

/usr/bin/chromium --no-sandbox --disable-dev-shm-usage --disable-gpu \
  --no-first-run --no-default-browser-check --window-size=1600,900 \
  "file:///opt/iapetus/s1.html" >/dev/null 2>&1 &

exec target/debug/iapetusd --stream "ws://127.0.0.1:8080/ingest?token=dev"
