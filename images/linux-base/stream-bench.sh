#!/usr/bin/env bash
set -euo pipefail
Xvfb "${DISPLAY}" -screen 0 "${SCREEN_GEOMETRY}" -nolisten tcp 2>/dev/null &
for _ in $(seq 1 100); do xdpyinfo -display "${DISPLAY}" >/dev/null 2>&1 && break; sleep 0.1; done
openbox >/dev/null 2>&1 &
sleep 0.5
cd /src
cargo build -q -p iapetusd --features x11
BIN=target/debug/iapetusd

cat > /tmp/motion.html <<'HEOF'
<meta charset="utf-8"><title>motion</title>
<style>html,body{margin:0;height:100%;overflow:hidden}canvas{display:block}</style>
<canvas id=c></canvas><script>
const c=document.getElementById('c'),x=c.getContext('2d');
function sz(){c.width=innerWidth;c.height=innerHeight}
sz();addEventListener('resize',sz);let t=0;
(function f(){t+=0.05;
 const g=x.createLinearGradient(0,0,c.width,c.height);
 g.addColorStop(0,`hsl(${(t*40)%360},80%,50%)`);g.addColorStop(1,`hsl(${(t*40+120)%360},80%,30%)`);
 x.fillStyle=g;x.fillRect(0,0,c.width,c.height);
 x.fillStyle='#fff';x.font='40px sans-serif';
 for(let i=0;i<20;i++)x.fillText('IAPETUS '+i,(Math.sin(t+i)*300+400)|0,60+i*45);
 requestAnimationFrame(f)})();
HEOF

start_chrome() {
  rm -rf /tmp/prof-$2 && mkdir -p /tmp/prof-$2
  /usr/bin/chromium --no-sandbox --disable-dev-shm-usage --disable-gpu \
    --no-first-run --no-default-browser-check --window-size=1920,1080 --window-position=0,0 \
    --user-data-dir=/tmp/prof-$2 \
    "$1" >/dev/null 2>&1 &
  echo $!
}

echo "############ IDLE (static page) ############"
PID=$(start_chrome "file:///opt/iapetus/s1.html" a); sleep 8
"$BIN" --screenshot /tmp/shot-a.png; "$BIN" --stream-bench 8
kill -9 "$PID" 2>/dev/null || true; sleep 1

echo; echo "############ FULL-SCREEN MOTION ############"
PID=$(start_chrome "file:///tmp/motion.html" b); sleep 8
"$BIN" --screenshot /tmp/shot-b.png; "$BIN" --stream-bench 8
kill -9 "$PID" 2>/dev/null || true

echo; echo "### what was actually on screen ###"
for f in /tmp/shot-a.png /tmp/shot-b.png; do
  [ -f "$f" ] && echo "$f: $(stat -c%s "$f") bytes"
done
cp /tmp/shot-*.png /out/ 2>/dev/null || true
