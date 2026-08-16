#!/usr/bin/env bash
# Run the whole stack on a Linux host without Docker.
#
# Docker is a convenience, not a requirement. On Linux everything runs natively:
# the two services are cross-platform Rust binaries, and the desktop needs only
# an X server (Xvfb) and a window manager, both ordinary packages. On macOS or
# Windows the desktop still needs a Linux X11 environment — that is the one part
# Docker (or a remote Linux host) provides.
#
# Requires: cargo, Xvfb, openbox, and a browser (chromium). On Debian/Ubuntu:
#   sudo apt-get install -y xvfb openbox chromium
#
# Usage:  scripts/run-native.sh
#         then open http://localhost:8080/?token=dev-write
set -euo pipefail
cd "$(dirname "$0")/.."

export DISPLAY="${DISPLAY:-:1}"
SCREEN_GEOMETRY="${SCREEN_GEOMETRY:-1920x1080x24}"

command -v Xvfb    >/dev/null || { echo "Xvfb not found — apt-get install xvfb"; exit 1; }
command -v openbox >/dev/null || { echo "openbox not found — apt-get install openbox"; exit 1; }

echo "building the three binaries…"
cargo build --release -p iapetusd --features iapetusd/x11 -p iapetus-gateway -p iapetus-controlplane

pids=()
cleanup() { kill "${pids[@]}" 2>/dev/null || true; }
trap cleanup EXIT INT TERM

echo "starting control plane on :8090"
IAPETUS_CP_BIND=0.0.0.0:8090 IAPETUS_PROJECT_KEY="${IAPETUS_PROJECT_KEY:-sk_iap_live_demo}" \
  ./target/release/iapetus-controlplane & pids+=($!)

echo "starting gateway on :8080"
IAPETUS_GATEWAY_BIND=0.0.0.0:8080 ./target/release/iapetus-gateway & pids+=($!)
sleep 1

echo "starting the X session"
Xvfb "$DISPLAY" -screen 0 "$SCREEN_GEOMETRY" -nolisten tcp & pids+=($!)
for _ in $(seq 1 100); do xdpyinfo -display "$DISPLAY" >/dev/null 2>&1 && break; sleep 0.1; done
openbox & pids+=($!)
sleep 0.5

if command -v chromium >/dev/null; then
  chromium --no-sandbox --disable-dev-shm-usage --disable-gpu --no-first-run \
    --window-size=1600,900 "file://$PWD/images/linux-base/s1.html" >/dev/null 2>&1 & pids+=($!)
fi

echo
echo "up. open http://localhost:8080/?token=dev-write"
echo "ctrl-c to stop."
./target/release/iapetusd --stream "ws://127.0.0.1:8080/ingest?token=dev"
