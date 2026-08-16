#!/usr/bin/env bash
# The desktop service: a real desktop session streamed to the gateway. This is
# one Desktop (§5.3) — a persistent virtual computer an agent and a human co-own.
#
# For the demo this brings up a legible desktop — a panel, a file manager, and a
# couple of apps — so a viewer sees something recognizable rather than a bare
# window over black. §12.5 lever 2 is the production counterpart: in agent-only
# state a Desktop runs just the window manager (~30MB) and starts the panel and
# file manager when a human actually attaches. Set IAPETUS_MINIMAL=1 to see that
# lean state.
set -euo pipefail

GATEWAY_INGEST="${IAPETUS_GATEWAY_INGEST:-ws://gateway:8080/ingest?token=dev}"
MINIMAL="${IAPETUS_MINIMAL:-0}"
# The wallpaper is whatever IAPETUS_WALLPAPER points at, defaulting to the
# shipped image. A developer swaps it by mounting their own file over this path
# in compose — no rebuild — or by pointing the variable elsewhere.
WALLPAPER="${IAPETUS_WALLPAPER:-/opt/iapetus/wallpaper.png}"

Xvfb "${DISPLAY}" -screen 0 "${SCREEN_GEOMETRY}" -nolisten tcp &
for _ in $(seq 1 100); do
    xdpyinfo -display "${DISPLAY}" >/dev/null 2>&1 && break
    sleep 0.1
done
xdpyinfo -display "${DISPLAY}" >/dev/null 2>&1 || { echo "X server failed to start" >&2; exit 1; }

# The window manager. Minimal on purpose — the agent needs window management,
# not a full desktop environment, and the panel/file-manager weight is added
# only for the human-facing demo below.
openbox &
sleep 0.5

if [ "$MINIMAL" != "1" ]; then
    # The file manager draws the wallpaper and the desktop icons. Its config
    # points at the shipped wallpaper image, since openbox leaves the root
    # window black on its own.
    mkdir -p "$HOME/.config/pcmanfm/default"
    cat > "$HOME/.config/pcmanfm/default/desktop-items-0.conf" <<'CFG'
[*]
wallpaper_mode=stretch
wallpaper=WALLPAPER_PATH
desktop_bg=#8a8d92
desktop_fg=#ffffff
desktop_shadow=#000000
show_documents=0
show_trash=1
show_mounts=1
CFG
    # Substitute the chosen wallpaper path into the config.
    sed -i "s|WALLPAPER_PATH|${WALLPAPER}|" "$HOME/.config/pcmanfm/default/desktop-items-0.conf"
    pcmanfm --desktop --profile=default >/dev/null 2>&1 &
    sleep 0.5

    # A centered launcher dock — Chrome, file manager, terminal — instead of a
    # taskbar, so the session reads like the docks people know.
    mkdir -p "$HOME/.config/tint2"
    cp /opt/iapetus/tint2rc "$HOME/.config/tint2/tint2rc"
    tint2 -c "$HOME/.config/tint2/tint2rc" >/dev/null 2>&1 &

    # A terminal, so the desktop starts with an app already open.
    xterm -geometry 84x26+60+90 -title "Terminal" >/dev/null 2>&1 &
fi

# The browser, in a normal window (not kiosk) so it sits on the desktop
# alongside the other apps. --disable-gpu-compositing so Xvfb repaints the whole
# window rather than leaving stale regions.
chromium --no-sandbox --disable-dev-shm-usage --disable-gpu \
    --disable-gpu-compositing --disable-features=CalculateNativeWinOcclusion \
    --no-first-run --no-default-browser-check --window-size=1200,800 --window-position=380,120 \
    "file:///opt/iapetus/s1.html" >/dev/null 2>&1 &

echo "desktop up; streaming to ${GATEWAY_INGEST}"
# Reconnects on its own if the gateway is not up yet (§19.5 backoff).
exec iapetusd --stream "${GATEWAY_INGEST}"
