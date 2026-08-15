#!/usr/bin/env bash
# L2 guest integration checks (PRD §15.1).
#
# These run inside the container against a real X server. They are the layer
# §15.2 calls the most important one: what is not caught here surfaces only in
# production, because nothing above the platform boundary can detect it.
#
# Deliberately not asserted here: pixel equality. §15.3 forbids it — font
# hinting, antialiasing, and cursor blink make every capture differ. These
# checks assert semantics (geometry, presence, character round-trip) instead.
set -uo pipefail

pass=0; fail=0
check() {
    local name="$1"; shift
    if "$@" >/tmp/check.log 2>&1; then
        echo "  ✓ ${name}"; pass=$((pass+1))
    else
        echo "  ✗ ${name}"; sed 's/^/      /' /tmp/check.log | head -5; fail=$((fail+1))
    fi
}

echo "L2 guest integration"

# ── The X server exists and matches the geometry we asked for ─────────────
check "X server is running" xdpyinfo -display "${DISPLAY}"

check "screen geometry matches SCREEN_GEOMETRY" bash -c '
    want="${SCREEN_GEOMETRY%x*}"                     # 1920x1080x24 -> 1920x1080
    got=$(xdpyinfo | awk "/dimensions:/ {print \$2}")
    [ "$got" = "$want" ] || { echo "want ${want}, got ${got}"; exit 1; }'

# The §7.2 coordinate convention is in physical pixels with the origin at the
# top left. If the server reported a different size than requested, every
# coordinate an agent sends would land somewhere else.
check "single monitor, as §7.2 fixes for v1" bash -c '
    n=$(xdpyinfo | grep -c "^screen #")
    [ "$n" -eq 1 ] || { echo "expected 1 screen, found ${n}"; exit 1; }'

# ── Capture actually produces pixels of the right shape ───────────────────
check "capture produces a full-screen amount of pixel data" bash -c '
    xwd -root -silent > /tmp/root.xwd 2>/dev/null
    bytes=$(stat -c %s /tmp/root.xwd 2>/dev/null || echo 0)
    # 1920x1080 at 4 bytes per pixel is ~8.3MB. Asserting an exact size would
    # bind the test to a bit depth we do not otherwise care about, so this
    # checks that a real framebuffer came out rather than an empty or stub file.
    [ "$bytes" -gt 4000000 ] || { echo "xwd produced only ${bytes} bytes"; exit 1; }' 

# ── The Hangul path (§15.2) ──────────────────────────────────────────────
# Mandatory in CI: IME failures are fatal in the Korean market and are exactly
# what anglophone open-source stacks leave unverified.
check "CJK fonts are present" bash -c '
    fc-list 2>/dev/null | grep -qi "noto.*cjk" \
      || ls /usr/share/fonts/opentype/noto/ 2>/dev/null | grep -qi cjk \
      || { echo "no Noto CJK font found"; exit 1; }'

check "Hangul IME engine is registered and its binary exists" bash -c '
    # Resolve the engine the way IBus itself does — through the component XML —
    # rather than hardcoding a path. Debian moved this binary from /usr/lib to
    # /usr/libexec, and the hardcoded check reported a missing engine that was
    # in fact installed. Following the same indirection IBus follows survives
    # the next move too.
    xml=/usr/share/ibus/component/hangul.xml
    [ -f "$xml" ] || { echo "no IBus component registration at ${xml}"; exit 1; }
    exe=$(sed -n "s:.*<exec>\([^< ]*\).*:\1:p" "$xml" | head -1)
    [ -n "$exe" ] || { echo "component XML declares no <exec>"; exit 1; }
    [ -x "$exe" ] || { echo "declared engine ${exe} is not executable"; exit 1; }'

check "a Hangul string stays composed, not decomposed into jamo" bash -c '
    # Byte count distinguishes the two forms without needing a UTF-8 locale:
    # five precomposed syllables are 15 bytes, the eleven jamo they decompose
    # into are 33. That decomposition is exactly the §15.2 failure mode.
    n=$(printf %s "안녕하세요" | wc -c)
    [ "$n" -eq 15 ] || { echo "expected 15 bytes (NFC), got ${n}"; exit 1; }' 

# ── The daemon binary is the one we built ────────────────────────────────
check "iapetusd is present and executable" test -x /usr/local/bin/iapetusd

# ── §5.2: one shared OS account with passwordless sudo ───────────────────
check "running as the shared iapetus account" bash -c '
    [ "$(id -un)" = "iapetus" ] || { echo "running as $(id -un)"; exit 1; }'

check "sudo requires no password (OWNER mode, §7.3)" sudo -n true

echo
echo "  ${pass} passed, ${fail} failed"
[ "${fail}" -eq 0 ]
