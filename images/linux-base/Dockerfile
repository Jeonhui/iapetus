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
RUN cargo build --release -p iapetusd

FROM debian:bookworm-slim

# fonts-noto-cjk and ibus-hangul are required, not optional: without them the
# Hangul input path in §15.2 cannot be verified at all.
#
# x11-utils and x11-apps provide xdpyinfo and xwd, which the L2 integration
# test uses to prove capture works against a real X server.
RUN apt-get update && apt-get install -y --no-install-recommends \
      xvfb x11-utils x11-apps \
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
COPY images/linux-base/entrypoint.sh /usr/local/bin/entrypoint.sh
RUN chmod +x /usr/local/bin/entrypoint.sh

ENV DISPLAY=:1 \
    SCREEN_GEOMETRY=1920x1080x24

USER iapetus
WORKDIR /home/iapetus
ENTRYPOINT ["/usr/local/bin/entrypoint.sh"]
