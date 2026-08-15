//! The guest side of the Control Plane channel (PRD §19.5).
//!
//! The guest dials out and holds one long-lived bidirectional stream open; the
//! Control Plane issues actions over that connection and never connects inward,
//! so the guest opens no inbound port (§9.1).
//!
//! The module is split in two on purpose:
//!
//! * [`run_session`] is the protocol — version negotiation, arrival-order
//!   execution, deadlines, heartbeats. It takes a stream of [`ControlFrame`] and
//!   a sink of [`GuestFrame`], so every rule below is unit-testable without a
//!   server, a certificate, or a network.
//! * [`connect_and_run`] and [`run_forever`] are the transport — mTLS, the Guest
//!   Token, and reconnection backoff.
//!
//! Four rules from §19.5 are load-bearing and are enforced here rather than
//! assumed:
//!
//! 1. **Arrival order.** Actions execute one at a time, in the order received.
//!    This is what backs §7.2's sequential `act` and §8.5's FIFO guarantee, and
//!    it is also what makes §6.3 freshness meaningful — a screenshot overtaking
//!    the click before it would return the pre-click screen.
//! 2. **In-flight depth 8, held back by flow control.** The queue is bounded at
//!    8 and the reader *awaits* a free slot rather than rejecting. Not reading
//!    stops draining the HTTP/2 window, which is exactly the backpressure §19.5
//!    describes. Rejecting instead would push the problem back as an error the
//!    Control Plane has no way to act on.
//! 3. **A missed deadline drops the response.** It does not send a failure —
//!    the Control Plane already answers with `ACTION_TIMEOUT`, and a late
//!    response arriving after that would contradict it.
//! 4. **No retransmission across a reconnect.** In-flight work is abandoned.
//!    Resending an action whose execution during the outage is unknown puts the
//!    click through twice; the retry decision belongs to the agent, which holds
//!    the idempotency key (§8.4).

use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use iapetus_proto::v1::{
    control_frame, guest_frame, ActionRequest, ActionResponse, ControlFrame, GuestFrame, Heartbeat,
    Hello, HelloAck,
};
use tokio::sync::mpsc;
use tokio_stream::{Stream, StreamExt};

use crate::dispatch::Dispatcher;

/// §19.5 caps in-flight requests at 8. The queue is this deep and no deeper:
/// the bound is what converts into HTTP/2 backpressure.
pub const IN_FLIGHT_DEPTH: usize = 8;

/// §19.5: every 5 seconds. Three misses (15s) mark the Desktop `DEGRADED`.
pub const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(5);

/// §19.5 reconnection: exponential from 1s, capped at 30s, jittered.
pub const BACKOFF_BASE: Duration = Duration::from_secs(1);
pub const BACKOFF_MAX: Duration = Duration::from_secs(30);

/// How long a session must last before a reconnect is treated as a fresh start
/// rather than a continuing failure.
pub const MIN_SESSION_FOR_BACKOFF_RESET: Duration = Duration::from_secs(60);

/// How long to wait for `HelloAck`. Three heartbeat periods, matching the point
/// at which §12.2 already considers the Desktop `DEGRADED`.
pub const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Debug, thiserror::Error)]
pub enum ChannelError {
    #[error("no protocol version in common: guest speaks {min}..={max}, the Control Plane chose {chosen}")]
    ProtocolMismatch { min: i32, max: i32, chosen: i32 },
    #[error("the Control Plane sent {0} before HelloAck")]
    HandshakeSkipped(&'static str),
    #[error("a second HelloAck arrived on an established stream")]
    DuplicateHello,
    #[error("the stream closed before HelloAck")]
    ClosedDuringHandshake,
    #[error("no HelloAck within {HANDSHAKE_TIMEOUT:?}")]
    HandshakeTimedOut,
    /// Not a failure — the Control Plane declined the connection outright.
    /// Kept distinct so [`run_forever`] stops instead of reconnecting into it.
    #[error("the Control Plane shut the daemon down during the handshake: {0}")]
    ShutdownDuringHandshake(String),
    #[error("transport: {0}")]
    Transport(String),
    /// Nothing a retry can repair. [`run_forever`] stops rather than looping.
    #[error("configuration: {0}")]
    Config(String),
}

pub type Result<T> = std::result::Result<T, ChannelError>;

/// Why a session ended. Both are ordinary outcomes; the caller decides whether
/// to reconnect.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionEnd {
    /// The Control Plane asked the daemon to stop.
    Shutdown { reason: String },
    /// The stream closed — a lost connection, a Control Plane restart, a
    /// rolling deploy. Indistinguishable from the guest, and treated the same.
    StreamClosed,
}

#[derive(Debug, Clone)]
pub struct ChannelConfig {
    /// `https://…` — §19.5 requires mTLS, so a plaintext endpoint is refused by
    /// [`connect_and_run`] rather than quietly downgraded.
    pub endpoint: String,
    pub daemon_version: String,
    pub protocol_min: i32,
    pub protocol_max: i32,
    pub heartbeat: Duration,
}

impl Default for ChannelConfig {
    fn default() -> Self {
        Self {
            endpoint: String::new(),
            daemon_version: env!("CARGO_PKG_VERSION").to_string(),
            protocol_min: 3,
            protocol_max: 5,
            heartbeat: HEARTBEAT_INTERVAL,
        }
    }
}

impl ChannelConfig {
    /// Rejects configuration that no amount of retrying can fix.
    ///
    /// [`run_forever`] retries indefinitely, which is right for a Control Plane
    /// that is down and wrong for an endpoint that is misspelled. Checking once
    /// at startup turns a permanent misconfiguration into a startup failure
    /// instead of a reconnect loop that logs the same line every thirty seconds
    /// forever.
    pub fn validate(&self) -> Result<()> {
        if !self.endpoint.starts_with("https://") {
            return Err(ChannelError::Config(format!(
                "§19.5 requires mTLS; the endpoint must be https://, got {:?}",
                self.endpoint
            )));
        }
        if self.protocol_min > self.protocol_max {
            return Err(ChannelError::Config(format!(
                "protocol range is inverted: {}..={}",
                self.protocol_min, self.protocol_max
            )));
        }
        Ok(())
    }

    /// The opening frame of §19.4 version negotiation.
    ///
    /// `os` and `display` are reported rather than left unset: the Control
    /// Plane uses them to reject an action the guest cannot perform
    /// (`UNSUPPORTED_ON_OS`, §8.9) before it travels, and to know the coordinate
    /// frame agents will be computing clicks against (§7.2).
    #[must_use]
    pub fn hello(&self, screen: Option<crate::platform::ScreenInfo>) -> Hello {
        Hello {
            daemon_version: self.daemon_version.clone(),
            protocol_min: self.protocol_min,
            protocol_max: self.protocol_max,
            os: host_os() as i32,
            display: screen.map(|s| iapetus_proto::v1::Display {
                width: s.width as i32,
                height: s.height as i32,
                dpi: s.dpi as i32,
            }),
        }
    }
}

/// The OS this build runs on, as §19.4 reports it.
#[must_use]
pub fn host_os() -> iapetus_proto::v1::Os {
    if cfg!(target_os = "linux") {
        iapetus_proto::v1::Os::Linux
    } else if cfg!(target_os = "windows") {
        iapetus_proto::v1::Os::Windows
    } else {
        // macOS is a development host, not a target (§6.2). Saying so is
        // better than claiming Linux and having the Control Plane route
        // Linux-only actions here.
        iapetus_proto::v1::Os::Unspecified
    }
}

/// Checks the Control Plane's chosen version against what this build speaks.
///
/// §19.4 lets the Control Plane pick the highest common version. It cannot pick
/// one the guest does not implement — accepting that would mean parsing frames
/// with the wrong schema, which fails later and less legibly than here.
pub fn negotiate(ack: &HelloAck, min: i32, max: i32) -> Result<i32> {
    if ack.protocol < min || ack.protocol > max {
        return Err(ChannelError::ProtocolMismatch { min, max, chosen: ack.protocol });
    }
    Ok(ack.protocol)
}

/// Exponential backoff with jitter (§19.5).
///
/// Jitter is supplied by the caller rather than drawn internally so the growth
/// curve can be tested without a random source. [`jitter_unit`] provides the
/// real one.
#[derive(Debug, Clone)]
pub struct Backoff {
    attempt: u32,
    base: Duration,
    max: Duration,
}

impl Default for Backoff {
    fn default() -> Self {
        Self::new(BACKOFF_BASE, BACKOFF_MAX)
    }
}

impl Backoff {
    #[must_use]
    pub fn new(base: Duration, max: Duration) -> Self {
        Self { attempt: 0, base, max }
    }

    /// The next delay, given a jitter value in `[0, 1)`.
    ///
    /// The window is `[delay/2, delay)`. A full-jitter `[0, delay)` would let a
    /// reconnect fire almost immediately after a failure, which for a fleet
    /// reconnecting to a Control Plane that just restarted is the thundering
    /// herd the backoff exists to prevent.
    pub fn next(&mut self, jitter: f64) -> Duration {
        let shift = self.attempt.min(16);
        let raw = self.base.saturating_mul(1u32 << shift);
        let capped = raw.min(self.max);
        self.attempt = self.attempt.saturating_add(1);

        let half = capped.as_secs_f64() / 2.0;
        Duration::from_secs_f64(half + half * jitter.clamp(0.0, 1.0))
    }

    /// Called after a session establishes, so a long-lived connection that later
    /// drops reconnects promptly instead of inheriting an hour-old delay.
    pub fn reset(&mut self) {
        self.attempt = 0;
    }
}

/// A jitter value derived from the clock's sub-second remainder.
///
/// Not random, but it does not need to be: the goal is only that two daemons
/// starting from the same event do not retry in lockstep, and they do not share
/// a nanosecond boundary.
#[must_use]
pub fn jitter_unit() -> f64 {
    let n = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |d| d.subsec_nanos());
    f64::from(n) / 1_000_000_000.0
}

/// One action waiting its turn, with the clock already running on its deadline.
struct Pending {
    id: u64,
    action: iapetus_proto::v1::Action,
    /// `None` means no deadline. Proto3 defaults an unset `deadline_ms` to 0,
    /// and treating 0 as "expires immediately" would silently drop every
    /// response from a Control Plane that simply did not set the field.
    deadline: Option<Duration>,
    /// Measured from receipt, not from the Control Plane's send time: with an
    /// unsynchronised guest clock (§7.4) an absolute deadline is unusable, and
    /// receipt is the only instant the guest can observe honestly.
    received: Instant,
}

/// Runs the protocol over an established pair of streams.
///
/// `inbound` carries frames from the Control Plane; `outbound` carries frames
/// back. Splitting it this way is what makes the rules testable — the tests
/// below drive real frames through this function with no server involved.
pub async fn run_session<S>(
    mut inbound: S,
    outbound: mpsc::Sender<GuestFrame>,
    dispatcher: Arc<Dispatcher>,
    cfg: &ChannelConfig,
) -> Result<SessionEnd>
where
    S: Stream<Item = std::result::Result<ControlFrame, tonic::Status>> + Unpin,
{
    outbound
        .send(GuestFrame {
            body: Some(guest_frame::Body::Hello(cfg.hello(dispatcher.screen_info()))),
        })
        .await
        .map_err(|_| ChannelError::Transport("the outbound stream closed".into()))?;

    // ── Handshake ────────────────────────────────────────────
    // Nothing else is accepted first. A Control Plane that sends actions before
    // acknowledging the version is asking the guest to execute them under a
    // schema neither side has agreed on.
    let ack = match tokio::time::timeout(HANDSHAKE_TIMEOUT, async {
        loop {
            match inbound.next().await {
            Some(Ok(ControlFrame { body: Some(control_frame::Body::HelloAck(a)) })) => break Ok(a),
            Some(Ok(ControlFrame { body: Some(control_frame::Body::Request(_)) })) => {
                break Err(ChannelError::HandshakeSkipped("an action"))
            }
            Some(Ok(ControlFrame { body: Some(control_frame::Body::Shutdown(s)) })) => {
                break Err(ChannelError::ShutdownDuringHandshake(s.reason))
            }
            // A heartbeat before the ack is harmless — the Control Plane may
            // have a ticker running already — so it is ignored rather than
            // treated as a violation.
            Some(Ok(_)) => continue,
            Some(Err(e)) => break Err(ChannelError::Transport(e.to_string())),
            None => break Err(ChannelError::ClosedDuringHandshake),
            }
        }
    })
    .await
    {
        Ok(r) => r?,
        // A Control Plane that accepts the connection and then says nothing
        // looks identical to a healthy link from here. Waiting forever leaves a
        // daemon that never heartbeats and never reconnects — the Desktop goes
        // DEGRADED at 15s (§12.2) and stays there. Giving up at that same
        // boundary turns it into a reconnect instead.
        Err(_) => return Err(ChannelError::HandshakeTimedOut),
    };
    negotiate(&ack, cfg.protocol_min, cfg.protocol_max)?;

    // §7.4: after a restore the guest clock is stale. Actions are *held* — not
    // rejected — until the offset is known, because a rejection would be
    // permanent while the hold resolves on the next Control Plane heartbeat.
    // Anything still held when its deadline passes is dropped by the worker,
    // which is the §19.5 behaviour for a late action either way.
    //
    // The gate is level-triggered on purpose. An edge-triggered signal only
    // wakes whoever is already waiting, and the Control Plane's own 5s ticker
    // means the resync heartbeat usually arrives *before* the first action —
    // so the wake would land on nobody and every later action would hang.
    let (clock_gate, clock_gate_rx) = tokio::sync::watch::channel(!ack.require_clock_resync);
    let mut clock_pending = ack.require_clock_resync;

    // ── The action worker ────────────────────────────────────
    // One task, one action at a time. Concurrency here would reorder input.
    let (queue_tx, mut queue_rx) = mpsc::channel::<Pending>(IN_FLIGHT_DEPTH);
    let worker = {
        let out = outbound.clone();
        let disp = Arc::clone(&dispatcher);
        let mut gate = clock_gate_rx.clone();
        let mut gated = clock_pending;
        tokio::spawn(async move {
            while let Some(p) = queue_rx.recv().await {
                if gated {
                    // Returns immediately if the gate is already open. An error
                    // means the sender is gone — the session is ending, and a
                    // held action must not be applied to a desktop whose
                    // response has nowhere left to go.
                    if gate.wait_for(|open| *open).await.is_err() {
                        break;
                    }
                    gated = false;
                }
                if expired(&p) {
                    // Already late before it ran. Executing anyway would apply
                    // input the Control Plane has given up on.
                    continue;
                }

                let d = Arc::clone(&disp);
                let action = p.action.clone();
                let result = tokio::task::spawn_blocking(move || d.execute_reported(&action))
                    .await
                    .unwrap_or_else(|e| {
                        // A panic in a driver must not take the daemon with it;
                        // the action fails and the stream stays up.
                        crate::dispatch::panic_result(&e.to_string())
                    });

                if expired(&p) {
                    // §19.5: past the deadline the guest drops the response
                    // rather than sending one. Input already applied is not
                    // undone (§8.2) — that is why the check is here and not
                    // before execution alone.
                    continue;
                }
                let frame = GuestFrame {
                    body: Some(guest_frame::Body::Response(ActionResponse {
                        id: p.id,
                        result: Some(result),
                    })),
                };
                if out.send(frame).await.is_err() {
                    break; // the stream is gone; in-flight work is abandoned
                }
            }
        })
    };

    // ── Heartbeats ───────────────────────────────────────────
    let heartbeat = {
        let out = outbound.clone();
        let period = cfg.heartbeat;
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(period);
            // Without this a runtime that falls behind fires the missed ticks
            // back to back, sending a burst that looks like a healthy daemon
            // precisely when it is not.
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            tick.tick().await; // the first tick is immediate
            loop {
                tick.tick().await;
                let hb = GuestFrame {
                    body: Some(guest_frame::Body::Heartbeat(Heartbeat {
                        ts: Some(crate::dispatch::prost_time(SystemTime::now())),
                        cpu_load: 0.0,
                        mem_used_bytes: 0,
                        capture_fps: 0.0,
                    })),
                };
                if out.send(hb).await.is_err() {
                    break;
                }
            }
        })
    };

    // ── The read loop ────────────────────────────────────────
    let end = loop {
        let frame = match inbound.next().await {
            Some(Ok(f)) => f,
            Some(Err(e)) => {
                worker.abort();
                heartbeat.abort();
                return Err(ChannelError::Transport(e.to_string()));
            }
            None => break SessionEnd::StreamClosed,
        };

        match frame.body {
            Some(control_frame::Body::Request(req)) => {
                let id = req.id;
                let Some(p) = pending(req) else {
                    // A request carrying no action is a Control Plane bug.
                    // Dropping it would make the caller wait out the full
                    // deadline for an answer that was never coming, so it is
                    // refused immediately and named.
                    let frame = GuestFrame {
                        body: Some(guest_frame::Body::Response(ActionResponse {
                            id,
                            result: Some(crate::dispatch::empty_action_result()),
                        })),
                    };
                    if outbound.send(frame).await.is_err() {
                        break SessionEnd::StreamClosed;
                    }
                    continue;
                };
                // `send` awaits when the queue is full. That is deliberate: not
                // reading leaves the HTTP/2 window unopened, which is the
                // backpressure §19.5 relies on to hold the depth at 8.
                if queue_tx.send(p).await.is_err() {
                    break SessionEnd::StreamClosed;
                }
            }
            Some(control_frame::Body::Heartbeat(hb)) => {
                if clock_pending {
                    if let Some(ts) = hb.ts {
                        dispatcher.set_clock_offset(offset_from(&ts));
                        clock_pending = false;
                        let _ = clock_gate.send(true);
                    }
                }
            }
            Some(control_frame::Body::Shutdown(s)) => break SessionEnd::Shutdown { reason: s.reason },
            Some(control_frame::Body::HelloAck(_)) => {
                worker.abort();
                heartbeat.abort();
                return Err(ChannelError::DuplicateHello);
            }
            None => continue,
        }
    };

    // Dropping the gate releases a worker still waiting on a resync that is
    // never going to arrive; without it, a session that closed before the first
    // Control Plane heartbeat would wait on the worker forever and never
    // reconnect. Dropping the queue then lets the worker finish what it has
    // already started, and aborting the heartbeat stops it writing to a stream
    // that is closing.
    drop(clock_gate);
    drop(queue_tx);
    heartbeat.abort();
    let _ = worker.await;
    Ok(end)
}

fn pending(req: ActionRequest) -> Option<Pending> {
    Some(Pending {
        id: req.id,
        action: req.action?,
        deadline: (req.deadline_ms > 0).then(|| Duration::from_millis(req.deadline_ms as u64)),
        received: Instant::now(),
    })
}

fn expired(p: &Pending) -> bool {
    p.deadline.is_some_and(|d| p.received.elapsed() > d)
}

/// How far the guest clock is from the Control Plane's, as `control - guest`.
///
/// Applied to reported timestamps only. Freshness comparisons (§6.3) are
/// between two guest-side instants, so a constant offset cancels out of them —
/// it is the absolute value on the wire that a stale clock corrupts.
fn offset_from(cp: &prost_types::Timestamp) -> i64 {
    let guest = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64);
    let control = cp.seconds * 1000 + i64::from(cp.nanos) / 1_000_000;
    control - guest
}

// ── Transport ────────────────────────────────────────────────

/// Dials the Control Plane and runs one session.
///
/// §19.5 fixes the transport at gRPC over mTLS. A plaintext endpoint is refused
/// rather than accepted with a warning: the Guest Token travels on this
/// connection, and a warning in a log nobody reads is not a control.
pub async fn connect_and_run(
    cfg: &ChannelConfig,
    tls: tonic::transport::ClientTlsConfig,
    guest_token: &str,
    dispatcher: Arc<Dispatcher>,
) -> Result<SessionEnd> {
    use iapetus_proto::v1::daemon_channel_client::DaemonChannelClient;

    // Belt and braces: `main` validates once at startup, but a caller that
    // skipped that must not get a plaintext connection carrying a Guest Token.
    cfg.validate()?;

    let channel = tonic::transport::Endpoint::from_shared(cfg.endpoint.clone())
        .map_err(|e| ChannelError::Transport(e.to_string()))?
        .tls_config(tls)
        .map_err(|e| ChannelError::Transport(e.to_string()))?
        // HTTP/2 keepalive: without it a silently dropped connection is only
        // noticed when the next action is sent, which can be minutes on an idle
        // Desktop and would delay the §12.2 DEGRADED transition past its budget.
        .http2_keep_alive_interval(Duration::from_secs(10))
        .keep_alive_timeout(Duration::from_secs(20))
        .connect()
        .await
        .map_err(|e| ChannelError::Transport(e.to_string()))?;

    // Sized against what the encoder may emit. tonic's 4MB default would turn
    // a large PNG screenshot into a stream error rather than an image, and the
    // failure would look like a network fault instead of a size limit.
    let mut client = DaemonChannelClient::new(channel)
        .max_encoding_message_size(crate::encode::WIRE_MAX_BYTES + 64 * 1024)
        .max_decoding_message_size(crate::encode::WIRE_MAX_BYTES + 64 * 1024);

    let (tx, rx) = mpsc::channel::<GuestFrame>(IN_FLIGHT_DEPTH * 2);
    let mut request = tonic::Request::new(tokio_stream::wrappers::ReceiverStream::new(rx));
    let bearer = format!("Bearer {guest_token}")
        .parse()
        .map_err(|_| ChannelError::Transport("the Guest Token is not a valid header value".into()))?;
    request.metadata_mut().insert("authorization", bearer);

    let inbound = client
        .attach(request)
        .await
        .map_err(|e| ChannelError::Transport(e.to_string()))?
        .into_inner();

    run_session(inbound, tx, dispatcher, cfg).await
}

/// Reconnects until the Control Plane asks the daemon to stop.
///
/// In-flight actions are **not** retransmitted across a reconnect (§19.5); the
/// Control Plane answers them with `ACTION_TIMEOUT` and the agent decides
/// whether to retry, holding the idempotency key that makes that safe (§8.4).
pub async fn run_forever(
    cfg: &ChannelConfig,
    tls: tonic::transport::ClientTlsConfig,
    guest_token: &str,
    dispatcher: Arc<Dispatcher>,
) {
    let mut backoff = Backoff::default();
    loop {
        let started = Instant::now();
        match connect_and_run(cfg, tls.clone(), guest_token, Arc::clone(&dispatcher)).await {
            Ok(SessionEnd::Shutdown { reason }) => {
                eprintln!("control plane requested shutdown: {reason}");
                return;
            }
            Ok(SessionEnd::StreamClosed) => eprintln!("stream closed; reconnecting"),
            Err(ChannelError::ShutdownDuringHandshake(reason)) => {
                eprintln!("control plane requested shutdown: {reason}");
                return;
            }
            Err(e @ ChannelError::Config(_)) => {
                eprintln!("{e}");
                return;
            }
            Err(e) => eprintln!("session failed: {e}"),
        }
        // Only a session that actually stayed up earns a fresh curve. Resetting
        // on any clean close lets a Control Plane that accepts and immediately
        // drops the stream — a rejected token, a node draining — be retried
        // twice a second forever, which is the storm the backoff exists to stop.
        if started.elapsed() >= MIN_SESSION_FOR_BACKOFF_RESET {
            backoff.reset();
        }
        let delay = backoff.next(jitter_unit());
        tokio::time::sleep(delay).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame::FrameSource;
    use crate::platform::fake::{FakeDisplay, FakeInput, InputEvent};
    use iapetus_proto::v1::{self as v1, action::Kind};

    fn dispatcher() -> (Arc<Dispatcher>, Arc<FakeInput>) {
        let input = Arc::new(FakeInput::new().with_screen(1920, 1080));
        let d = Dispatcher::new(
            FrameSource::new(Box::new(FakeDisplay::new(1920, 1080))),
            Box::new(input.clone()),
        );
        (Arc::new(d), input)
    }

    fn click(id: u64, x: i32, y: i32, deadline_ms: i32) -> ControlFrame {
        ControlFrame {
            body: Some(control_frame::Body::Request(ActionRequest {
                id,
                action: Some(v1::Action {
                    kind: Some(Kind::MouseClick(v1::MouseClick {
                        at: Some(v1::Point { x, y }),
                        button: 0,
                        count: 1,
                    })),
                }),
                deadline_ms,
                source: None,
            })),
        }
    }

    fn ack(protocol: i32) -> ControlFrame {
        ControlFrame {
            body: Some(control_frame::Body::HelloAck(HelloAck {
                protocol,
                degraded: false,
                degraded_reason: None,
                require_clock_resync: false,
            })),
        }
    }

    /// Drives `run_session` over a fixed script of inbound frames.
    async fn drive(frames: Vec<ControlFrame>) -> (Result<SessionEnd>, Vec<GuestFrame>, Arc<FakeInput>) {
        let (disp, input) = dispatcher();
        let (tx, mut rx) = mpsc::channel(64);
        let inbound = tokio_stream::iter(frames.into_iter().map(Ok));
        let cfg = ChannelConfig { heartbeat: Duration::from_secs(3600), ..Default::default() };

        let end = run_session(inbound, tx, disp, &cfg).await;

        let mut out = Vec::new();
        while let Ok(f) = rx.try_recv() {
            out.push(f);
        }
        (end, out, input)
    }

    #[tokio::test]
    async fn the_session_opens_with_hello_and_runs_actions_after_the_ack() {
        let (end, out, input) = drive(vec![ack(4), click(1, 100, 200, 0)]).await;

        assert_eq!(end.unwrap(), SessionEnd::StreamClosed);
        assert!(
            matches!(out.first().and_then(|f| f.body.as_ref()), Some(guest_frame::Body::Hello(_))),
            "the guest must speak first; got {:?}",
            out.first()
        );
        assert_eq!(
            input.events(),
            vec![InputEvent::Click { x: 100, y: 200, button: crate::platform::Button::Left, count: 1 }]
        );

        let responses: Vec<_> = out
            .iter()
            .filter_map(|f| match &f.body {
                Some(guest_frame::Body::Response(r)) => Some(r),
                _ => None,
            })
            .collect();
        assert_eq!(responses.len(), 1);
        assert_eq!(responses[0].id, 1);
        assert!(responses[0].result.as_ref().unwrap().ok);
    }

    #[tokio::test]
    async fn actions_execute_in_arrival_order() {
        // §19.5: the guest never reorders. §7.2's sequential `act` and §8.5's
        // FIFO guarantee both rest on this, and so does §6.3 freshness — a
        // screenshot that overtook the click before it would show the old
        // screen and the agent would conclude the click failed.
        let script = vec![
            ack(4),
            click(1, 10, 10, 0),
            click(2, 20, 20, 0),
            click(3, 30, 30, 0),
            click(4, 40, 40, 0),
        ];
        let (_, out, input) = drive(script).await;

        let xs: Vec<i32> = input
            .events()
            .iter()
            .filter_map(|e| match e {
                InputEvent::Click { x, .. } => Some(*x),
                _ => None,
            })
            .collect();
        assert_eq!(xs, vec![10, 20, 30, 40], "actions were reordered");

        let ids: Vec<u64> = out
            .iter()
            .filter_map(|f| match &f.body {
                Some(guest_frame::Body::Response(r)) => Some(r.id),
                _ => None,
            })
            .collect();
        assert_eq!(ids, vec![1, 2, 3, 4], "responses came back out of order");
    }

    #[tokio::test]
    async fn a_missed_deadline_drops_the_response_instead_of_failing_it() {
        // §19.5: past the deadline the guest sends nothing. The Control Plane
        // has already answered ACTION_TIMEOUT; a late failure arriving after
        // that would contradict an answer the agent has acted on.
        // The driver is made slow so the deadline actually elapses during
        // execution — the case the rule exists for. A fast action would pass
        // this test no matter what the code did.
        let input = Arc::new(
            FakeInput::new().with_screen(1920, 1080).with_latency(Duration::from_millis(120)),
        );
        let disp = Arc::new(Dispatcher::new(
            FrameSource::new(Box::new(FakeDisplay::new(1920, 1080))),
            Box::new(input.clone()),
        ));
        let (tx, mut rx) = mpsc::channel(64);

        let (feed_tx, feed_rx) = mpsc::channel::<std::result::Result<ControlFrame, tonic::Status>>(4);
        feed_tx.send(Ok(ack(4))).await.unwrap();
        feed_tx.send(Ok(click(7, 10, 10, 20))).await.unwrap();

        let cfg = ChannelConfig { heartbeat: Duration::from_secs(3600), ..Default::default() };
        let handle = tokio::spawn(async move {
            run_session(tokio_stream::wrappers::ReceiverStream::new(feed_rx), tx, disp, &cfg).await
        });

        tokio::time::sleep(Duration::from_millis(300)).await;
        drop(feed_tx);
        handle.await.unwrap().unwrap();

        assert_eq!(input.events().len(), 1, "the action itself must still have run: §8.2 does not undo applied input");

        let mut responses = 0;
        while let Ok(f) = rx.try_recv() {
            if matches!(f.body, Some(guest_frame::Body::Response(_))) {
                responses += 1;
            }
        }
        assert_eq!(responses, 0, "a response was sent for an action past its deadline");
    }

    #[tokio::test]
    async fn a_zero_deadline_means_no_deadline_not_an_expired_one() {
        // proto3 defaults an unset int to 0. Reading that as "expires
        // immediately" would drop every response from a Control Plane that
        // simply did not set the field — a total outage that looks like a
        // network fault.
        let (_, out, _) = drive(vec![ack(4), click(1, 5, 5, 0)]).await;
        assert!(
            out.iter().any(|f| matches!(f.body, Some(guest_frame::Body::Response(_)))),
            "deadline_ms = 0 was treated as already expired"
        );
    }

    #[tokio::test]
    async fn an_action_before_the_handshake_is_refused() {
        let (end, _, input) = drive(vec![click(1, 10, 10, 0), ack(4)]).await;
        assert!(matches!(end, Err(ChannelError::HandshakeSkipped(_))), "got {end:?}");
        assert!(input.events().is_empty(), "an action ran before the version was agreed");
    }

    #[tokio::test]
    async fn a_version_outside_the_supported_range_ends_the_session() {
        // §19.4: no overlap leaves the Desktop DEGRADED. Executing under a
        // schema the guest does not implement is the failure this prevents.
        let (end, _, input) = drive(vec![ack(9), click(1, 10, 10, 0)]).await;
        assert!(matches!(end, Err(ChannelError::ProtocolMismatch { chosen: 9, .. })), "got {end:?}");
        assert!(input.events().is_empty());
    }

    #[tokio::test]
    async fn shutdown_ends_the_session_with_its_reason() {
        let script = vec![
            ack(4),
            ControlFrame {
                body: Some(control_frame::Body::Shutdown(v1::Shutdown {
                    reason: "host drain".into(),
                })),
            },
        ];
        let (end, _, _) = drive(script).await;
        assert_eq!(end.unwrap(), SessionEnd::Shutdown { reason: "host drain".into() });
    }

    #[tokio::test]
    async fn actions_are_held_until_the_clock_resyncs_then_run() {
        // §7.4: after a restore the guest clock is stale. Held, not rejected —
        // a rejection would be permanent where the hold clears on the Control
        // Plane's next heartbeat.
        let (disp, input) = dispatcher();
        let (tx, _rx) = mpsc::channel(64);
        let (feed_tx, feed_rx) = mpsc::channel::<std::result::Result<ControlFrame, tonic::Status>>(8);

        feed_tx
            .send(Ok(ControlFrame {
                body: Some(control_frame::Body::HelloAck(HelloAck {
                    protocol: 4,
                    degraded: false,
                    degraded_reason: None,
                    require_clock_resync: true,
                })),
            }))
            .await
            .unwrap();
        feed_tx.send(Ok(click(1, 11, 11, 0))).await.unwrap();

        let cfg = ChannelConfig { heartbeat: Duration::from_secs(3600), ..Default::default() };
        let probe = Arc::clone(&input);
        let handle = tokio::spawn(async move {
            run_session(tokio_stream::wrappers::ReceiverStream::new(feed_rx), tx, disp, &cfg).await
        });

        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(probe.events().is_empty(), "the action ran before the clock resynced");

        feed_tx
            .send(Ok(ControlFrame {
                body: Some(control_frame::Body::Heartbeat(Heartbeat {
                    ts: Some(prost_types::Timestamp { seconds: 1_700_000_000, nanos: 0 }),
                    cpu_load: 0.0,
                    mem_used_bytes: 0,
                    capture_fps: 0.0,
                })),
            }))
            .await
            .unwrap();

        tokio::time::sleep(Duration::from_millis(50)).await;
        drop(feed_tx);
        handle.await.unwrap().unwrap();

        assert_eq!(probe.events().len(), 1, "the held action never ran after the resync");
    }

    #[tokio::test]
    async fn the_resync_heartbeat_may_arrive_before_any_action() {
        // The Control Plane runs its own 5s ticker, so on a resumed Desktop the
        // heartbeat almost always lands *before* the first action. A resync
        // signal that only wakes whoever is already waiting is therefore lost
        // in the common case, and every later action hangs until its deadline —
        // a total outage that reads as a network fault.
        let script = vec![
            ControlFrame {
                body: Some(control_frame::Body::HelloAck(HelloAck {
                    protocol: 4,
                    degraded: false,
                    degraded_reason: None,
                    require_clock_resync: true,
                })),
            },
            ControlFrame {
                body: Some(control_frame::Body::Heartbeat(Heartbeat {
                    ts: Some(prost_types::Timestamp { seconds: 1_700_000_000, nanos: 0 }),
                    cpu_load: 0.0,
                    mem_used_bytes: 0,
                    capture_fps: 0.0,
                })),
            },
            click(1, 12, 12, 0),
        ];

        let run = tokio::time::timeout(Duration::from_secs(3), drive(script)).await;
        let (_, _, input) = run.expect("the session hung waiting for a resync it had already been given");
        assert_eq!(input.events().len(), 1, "the action never ran after the resync");
    }

    #[tokio::test]
    async fn hello_reports_the_screen_the_agent_will_aim_at() {
        // §7.2: agents compute click coordinates against this frame. Sending it
        // unset makes the Control Plane guess, and a guess that is wrong puts
        // every click somewhere else.
        let (_, out, _) = drive(vec![ack(4)]).await;
        let Some(guest_frame::Body::Hello(h)) = out.first().and_then(|f| f.body.clone()) else {
            panic!("no Hello was sent");
        };
        let d = h.display.expect("Hello carried no display geometry");
        assert_eq!((d.width, d.height), (1920, 1080));
    }

    #[tokio::test]
    async fn a_request_with_no_action_is_refused_rather_than_ignored() {
        // Silently dropping it makes the caller wait out the whole deadline for
        // an answer that was never coming.
        let script = vec![
            ack(4),
            ControlFrame {
                body: Some(control_frame::Body::Request(ActionRequest {
                    id: 42,
                    action: None,
                    deadline_ms: 0,
                    source: None,
                })),
            },
        ];
        let (_, out, _) = drive(script).await;

        let r = out
            .iter()
            .find_map(|f| match &f.body {
                Some(guest_frame::Body::Response(r)) => Some(r),
                _ => None,
            })
            .expect("an empty request got no answer at all");
        assert_eq!(r.id, 42);
        let result = r.result.as_ref().unwrap();
        assert!(!result.ok);
        assert_eq!(result.error.as_ref().unwrap().code, "EXEC_FAILED");
    }

    #[tokio::test]
    async fn a_session_that_closes_while_an_action_is_held_still_returns() {
        // The action is waiting on a clock resync that never comes, and then
        // the stream closes. If the session waits on that worker it never
        // returns, so the daemon neither heartbeats nor reconnects — a hang
        // that outlives the outage it came from.
        let script = vec![
            ControlFrame {
                body: Some(control_frame::Body::HelloAck(HelloAck {
                    protocol: 4,
                    degraded: false,
                    degraded_reason: None,
                    require_clock_resync: true,
                })),
            },
            click(1, 13, 13, 0),
        ];

        let run = tokio::time::timeout(Duration::from_secs(3), drive(script)).await;
        let (end, _, input) = run.expect("the session hung on a worker that could never proceed");
        assert_eq!(end.unwrap(), SessionEnd::StreamClosed);
        assert!(
            input.events().is_empty(),
            "a held action was applied to a desktop whose response had nowhere to go"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_control_plane_that_never_acknowledges_is_given_up_on() {
        // Accepting the connection and then saying nothing is indistinguishable
        // from a healthy link here. Waiting forever leaves a daemon that never
        // heartbeats and never reconnects.
        let (disp, _) = dispatcher();
        let (tx, _rx) = mpsc::channel(8);
        let cfg = ChannelConfig::default();

        let end = run_session(tokio_stream::pending(), tx, disp, &cfg).await;
        assert!(matches!(end, Err(ChannelError::HandshakeTimedOut)), "got {end:?}");
    }

    #[test]
    fn backoff_grows_to_the_cap_and_never_retries_instantly() {
        // §19.5: 1s to a 30s cap, jittered. The lower half-window matters — a
        // full-jitter [0, delay) lets a fleet reconnecting to a Control Plane
        // that just restarted hit it again almost immediately.
        let mut b = Backoff::default();
        let mut prev = Duration::ZERO;
        for _ in 0..12 {
            let d = b.next(0.0);
            assert!(d >= Duration::from_millis(500), "retried after only {d:?}");
            assert!(d <= BACKOFF_MAX, "{d:?} exceeded the {BACKOFF_MAX:?} cap");
            assert!(d >= prev || prev >= BACKOFF_MAX / 2, "backoff shrank: {prev:?} -> {d:?}");
            prev = d;
        }
        assert_eq!(b.next(0.0), BACKOFF_MAX / 2);
        assert_eq!(b.next(0.999_999), BACKOFF_MAX.mul_f64(0.999_999 / 2.0 + 0.5));

        b.reset();
        assert_eq!(b.next(0.0), BACKOFF_BASE / 2, "reset did not restore the base delay");
    }

    #[test]
    fn a_configuration_error_is_caught_at_startup_not_retried_forever() {
        // run_forever retries indefinitely, which is right for an outage and
        // wrong for a typo. Without this check a misspelled scheme logs the
        // same failure every thirty seconds for the life of the Desktop.
        let plain = ChannelConfig { endpoint: "http://cp.example".into(), ..Default::default() };
        assert!(matches!(plain.validate(), Err(ChannelError::Config(_))));

        let empty = ChannelConfig::default();
        assert!(matches!(empty.validate(), Err(ChannelError::Config(_))));

        let inverted = ChannelConfig {
            endpoint: "https://cp.example".into(),
            protocol_min: 5,
            protocol_max: 3,
            ..Default::default()
        };
        assert!(matches!(inverted.validate(), Err(ChannelError::Config(_))));

        let ok = ChannelConfig { endpoint: "https://cp.example".into(), ..Default::default() };
        assert!(ok.validate().is_ok());
    }

    #[test]
    fn negotiate_accepts_only_versions_this_build_implements() {
        let a = |p| HelloAck {
            protocol: p,
            degraded: false,
            degraded_reason: None,
            require_clock_resync: false,
        };
        assert_eq!(negotiate(&a(3), 3, 5).unwrap(), 3);
        assert_eq!(negotiate(&a(5), 3, 5).unwrap(), 5);
        assert!(negotiate(&a(2), 3, 5).is_err());
        assert!(negotiate(&a(6), 3, 5).is_err());
    }
}
