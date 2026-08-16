# The Linux Desktop base image (PRD §19.2).
#
# This is the single-image form used for development and self-host. The
# production image drops the daemon COPY below and takes it from a read-only
# squashfs block device instead (§19.4), so a customer's custom image never
# pins an iapetusd version.

FROM rust:1-bookworm AS builder

# No protoc install needed: iapetus-proto uses a vendored compiler, so the
# build does not depend on what the host or base image happens to ship.

WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY crates/ ./crates/
# All three service binaries from one build, so the compose stack runs on a
# single image and a developer builds once. iapetusd needs the x11 backend.
RUN cargo build --release       -p iapetusd --features iapetusd/x11       -p iapetus-gateway       -p iapetus-controlplane

FROM debian:bookworm-slim

# fonts-noto-cjk and ibus-hangul are required, not optional: without them the
# Hangul input path in §15.2 cannot be verified at all.
#
# x11-utils and x11-apps provide xdpyinfo and xwd, which the L2 integration
# test uses to prove capture works against a real X server.
RUN apt-get update && apt-get install -y --no-install-recommends \
      xvfb x11-utils x11-apps x11-xserver-utils \
      tint2 pcmanfm xterm \
      chromium \
      openbox \
      dbus-x11 \
      fonts-noto-cjk ibus ibus-hangul \
      sudo ca-certificates curl \
 && rm -rf /var/lib/apt/lists/*

# The shared OS account (§5.2). One account, used by both the human and the
# agent, because splitting it would mean the agent could not use the human's
# logins — which is the product's reason to exist.
RUN useradd -m -s /bin/bash iapetus \
 && echo 'iapetus ALL=(ALL) NOPASSWD:ALL' > /etc/sudoers.d/iapetus \
 && chmod 0440 /etc/sudoers.d/iapetus

COPY --from=builder /src/target/release/iapetusd /usr/local/bin/iapetusd
COPY --from=builder /src/target/release/iapetus-gateway /usr/local/bin/iapetus-gateway
COPY --from=builder /src/target/release/iapetus-controlplane /usr/local/bin/iapetus-controlplane
# §5.5: shortcuts for the programs this image ships. Not a restriction —
# anything on the disk still launches by path under OWNER mode (§7.3).
COPY images/linux-base/apps.json /etc/iapetus/apps.json
COPY images/linux-base/s1.html /opt/iapetus/s1.html
COPY images/linux-base/wallpaper.png /opt/iapetus/wallpaper.png
COPY images/linux-base/tint2rc /opt/iapetus/tint2rc
COPY images/linux-base/compose-desktop.sh /usr/local/bin/compose-desktop.sh
COPY images/linux-base/entrypoint.sh /usr/local/bin/entrypoint.sh
RUN chmod +x /usr/local/bin/entrypoint.sh /usr/local/bin/compose-desktop.sh

ENV DISPLAY=:1 \
    SCREEN_GEOMETRY=1920x1080x24

USER iapetus
WORKDIR /home/iapetus
# No ENTRYPOINT: docker-compose gives each service its command. The desktop
# service runs compose-desktop.sh; the gateway and control plane run their
# binaries directly.
