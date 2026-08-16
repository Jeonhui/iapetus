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

# A desktop colour, so the root window is not the bare black openbox leaves.
xsetroot -solid "#243447" 2>/dev/null || true

if [ "$MINIMAL" != "1" ]; then
    # A panel with a clock and a task list, so open windows are visible and
    # switchable — the "there is a desktop here" cue a bare WM lacks.
    tint2 >/dev/null 2>&1 &

    # A file manager managing the desktop: it draws the background and the
    # desktop icons (a trash can, mounted volumes), which is what makes it read
    # as a desktop rather than a floating window. Its own config sets the
    # background colour, since it otherwise paints the root window black —
    # overriding xsetroot above.
    mkdir -p "$HOME/.config/pcmanfm/default"
    cat > "$HOME/.config/pcmanfm/default/desktop-items-0.conf" <<'CFG'
[*]
wallpaper_mode=color
desktop_bg=#243447
desktop_fg=#e8edf4
desktop_shadow=#000000
show_documents=0
show_trash=1
show_mounts=1
CFG
    pcmanfm --desktop --profile=default >/dev/null 2>&1 &
    sleep 0.5

    # A terminal, so the session shows more than one app — an agent would launch
    # these through app.launch instead, but a human opening the viewer should
    # land on a populated desktop.
    xterm -geometry 80x24+40+80 -title "Terminal" >/dev/null 2>&1 &
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
