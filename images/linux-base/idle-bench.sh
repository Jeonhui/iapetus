#!/usr/bin/env bash
set -euo pipefail
Xvfb "${DISPLAY}" -screen 0 "${SCREEN_GEOMETRY}" -nolisten tcp 2>/dev/null &
for _ in $(seq 1 100); do xdpyinfo -display "${DISPLAY}" >/dev/null 2>&1 && break; sleep 0.1; done
openbox >/dev/null 2>&1 & sleep 0.5
cd /src && cargo build -q --release -p iapetusd --features x11 -p iapetus-gateway
IAPETUS_GATEWAY_BIND=127.0.0.1:8080 target/release/iapetus-gateway >/dev/null 2>&1 &
sleep 1
HZ=$(getconf CLK_TCK); cpu(){ awk '{print $14+$15}' /proc/$1/stat; }
run() {
  target/release/iapetusd --stream "ws://127.0.0.1:8080/ingest?token=dev" >/dev/null 2>&1 &
  D=$!; sleep 3; A=$(cpu $D); sleep 12; B=$(cpu $D)
  awk -v a="$A" -v b="$B" -v hz="$HZ" -v l="$1" 'BEGIN{printf "%-26s %.2f%%\n",l,(b-a)*100/hz/12}'
  kill -9 $D 2>/dev/null; wait $D 2>/dev/null || true; sleep 1
}
run "bare root:"
/usr/bin/xclock -update 1 >/dev/null 2>&1 & sleep 2
run "xclock (1 change/sec):"
