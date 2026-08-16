//! The stream gateway (PRD §6.1, §19.6) — WebSocket JPEG-diff path.
//!
//! Two facts about the guest shape this whole component:
//!
//! * **The guest opens no inbound port** (§9.1). It dials *out* to `/ingest`
//!   and holds that socket open, so nothing here ever connects to a Desktop.
//! * **The gateway never decodes** (§19.6). It splits the tile framing to cache
//!   the newest tile per position, and relays bytes. It does not touch a pixel.
//!
//! The tile cache is what lets a viewer join mid-stream. Diffs are meaningless
//! without a base frame, and asking the guest for a fresh keyframe every time
//! someone opens a tab would re-encode the whole screen per viewer — exactly
//! the cost §6.3 built the shared Frame Source to avoid. The gateway already
//! has every tile; it just has to keep the last one of each.
//!
//! Input flows the other way on the same sockets. §7.5 is firm that there is
//! **one input path**: a viewer's mouse and keyboard become the same Computer
//! API actions an agent sends. Anything else would split the audit log and make
//! lease arbitration impossible.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::response::{Html, IntoResponse};
use axum::routing::{get, post};
use axum::Router;
use iapetus_auth::{Jwks, Policy, AUDIENCE, AUDIENCE_GUEST};
use iapetus_proto::lease::{Acquired, Actor, ControlLease};
use tokio::sync::{broadcast, Mutex};

/// The session the guest holds the lease under. §5.6's human-preempts-agent
/// rule needs the guest to actually hold WRITE as an agent, so a viewer taking
/// over is a real preemption — the S4 case — rather than grabbing a free lease.
const GUEST_SESSION: &str = "guest:agent";
const HEARTBEAT: Duration = Duration::from_secs(30);

/// One cached rectangle of the screen.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Region {
    x: u16,
    y: u16,
    w: u16,
    h: u16,
    jpeg: Vec<u8>,
}

impl Region {
    /// Whether this region completely covers `other`, which can then never show
    /// through again and is dropped.
    fn covers(&self, other: &Region) -> bool {
        self.x <= other.x
            && self.y <= other.y
            && u32::from(self.x) + u32::from(self.w) >= u32::from(other.x) + u32::from(other.w)
            && u32::from(self.y) + u32::from(self.h) >= u32::from(other.y) + u32::from(other.h)
    }
}

/// Enough of the screen to bootstrap a viewer that joined mid-stream.
///
/// **In insertion order, not sorted by position.** The guest sends arbitrary
/// rectangles — one small one when a cursor blinked, one large one when much of
/// the screen changed at once — so a later region can overlap an earlier one.
/// Replaying them in position order would draw the older rectangle on top and
/// show a new viewer a patch of the past.
#[derive(Default)]
struct TileCache {
    width: u16,
    height: u16,
    regions: Vec<Region>,
}

/// Above this many cached regions the cache is dropped and the guest asked for
/// a fresh keyframe.
///
/// Partially-overlapping rectangles are never evicted by the covering rule, so
/// without a bound a long-lived stream of odd shapes could grow without limit.
/// Rebuilding is cheap and self-healing; a leak is neither.
const MAX_REGIONS: usize = 4096;

impl TileCache {
    /// Splits an update into its regions. Framing only — no pixel is examined,
    /// which is what §19.6 means by the gateway never decoding.
    ///
    /// Returns `None` on a malformed update, leaving the cache untouched.
    fn apply(&mut self, buf: &[u8]) -> Option<()> {
        if buf.len() < 12 {
            return None;
        }
        let width = u16::from_be_bytes(buf[4..6].try_into().ok()?);
        let height = u16::from_be_bytes(buf[6..8].try_into().ok()?);
        let keyframe = buf[8] == 1;
        let count = u16::from_be_bytes(buf[10..12].try_into().ok()?);

        // A resize invalidates every cached tile: the old ones describe a screen
        // that no longer exists, and handing them to a viewer would paint stale
        // regions that never get overwritten.
        if keyframe || (self.width, self.height) != (width, height) {
            self.regions.clear();
        }
        self.width = width;
        self.height = height;

        let mut off = 12usize;
        let mut parsed = Vec::with_capacity(count as usize);
        for _ in 0..count {
            if buf.len() < off + 12 {
                return None;
            }
            let x = u16::from_be_bytes(buf[off..off + 2].try_into().ok()?);
            let y = u16::from_be_bytes(buf[off + 2..off + 4].try_into().ok()?);
            let w = u16::from_be_bytes(buf[off + 4..off + 6].try_into().ok()?);
            let h = u16::from_be_bytes(buf[off + 6..off + 8].try_into().ok()?);
            let len = u32::from_be_bytes(buf[off + 8..off + 12].try_into().ok()?) as usize;
            off += 12;
            if buf.len() < off + len {
                return None;
            }
            parsed.push(Region { x, y, w, h, jpeg: buf[off..off + len].to_vec() });
            off += len;
        }

        // Applied only after the whole update parsed. Half-applying a truncated
        // one would leave the cache describing a frame that never existed, and
        // every viewer bootstrapped from it inherits the corruption.
        for r in parsed {
            self.regions.retain(|old| !r.covers(old));
            self.regions.push(r);
        }
        if self.regions.len() > MAX_REGIONS {
            self.regions.clear();
        }
        Some(())
    }

    /// Rebuilds a keyframe from what is cached, in the same wire format the
    /// guest emits, so the viewer needs no second code path to apply it.
    fn keyframe(&self) -> Option<Vec<u8>> {
        if self.regions.is_empty() {
            return None;
        }
        let mut out = Vec::new();
        out.extend_from_slice(&0u32.to_be_bytes()); // seq 0: this is a bootstrap
        out.extend_from_slice(&self.width.to_be_bytes());
        out.extend_from_slice(&self.height.to_be_bytes());
        out.push(1); // keyframe
        out.push(0);
        out.extend_from_slice(&(self.regions.len() as u16).to_be_bytes());
        for r in &self.regions {
            out.extend_from_slice(&r.x.to_be_bytes());
            out.extend_from_slice(&r.y.to_be_bytes());
            out.extend_from_slice(&r.w.to_be_bytes());
            out.extend_from_slice(&r.h.to_be_bytes());
            out.extend_from_slice(&(r.jpeg.len() as u32).to_be_bytes());
            out.extend_from_slice(&r.jpeg);
        }
        Some(out)
    }
}

struct App {
    /// Screen updates, guest → viewers.
    frames: broadcast::Sender<Arc<Vec<u8>>>,
    /// Input and control, viewers → guest.
    to_guest: broadcast::Sender<String>,
    cache: Mutex<TileCache>,
    /// The §5.6 input lease. std Mutex, not tokio: every hold is a handful of
    /// non-awaiting comparisons, and a viewer's input must not wait on an async
    /// scheduler to learn whether it may pass.
    lease: std::sync::Mutex<ControlLease>,
    /// control.granted / control.revoked, gateway → all viewers.
    control_events: broadcast::Sender<String>,
    /// Origin for the lease's monotonic `now`. Only differences matter (§5.6).
    started: Instant,
    /// Hands each viewer socket a distinct lease session id.
    next_session: AtomicU64,
    /// Agent actions awaiting a reply from the guest, keyed by the id the
    /// gateway assigned. §8.5's request/response over one connection: the guest
    /// answers on the same socket it streams on, and this matches the answer to
    /// the waiting HTTP request.
    pending: std::sync::Mutex<std::collections::HashMap<u64, tokio::sync::oneshot::Sender<String>>>,
    next_action: AtomicU64,
    /// Stands in for §19.6's SRTP key: the guest proves it belongs before it may
    /// push a screen. Not the product mechanism, but the endpoint must not be
    /// open to anything that can reach the port.
    ingest_token: String,
    /// Stands in for the §8.1 Viewer Token. In the product this is an Ed25519
    /// JWT carrying `desktop_ids` and a control level, verified against JWKS;
    /// the gateway has no JWKS here, so it compares a shared secret. What must
    /// not differ is that **an unauthenticated socket gets nothing** — a viewer
    /// endpoint open to whatever can reach the port hands a stranger both the
    /// screen and the keyboard.
    view_token: String,
    /// Whether a guest currently holds `/ingest`. One Desktop, one stream: a
    /// second guest's frames would interleave into the same broadcast and every
    /// viewer would watch two screens shuffled together.
    guest_attached: std::sync::atomic::AtomicBool,
    /// The token that additionally grants `WRITE`. §7.5 puts this check in the
    /// gateway: it verifies the session holds the lease before forwarding
    /// input, because routing input through the Control Plane would add a round
    /// trip to the 20–50ms budget and destroy the feel.
    write_token: String,
    /// The verifying keys for real §8.1 JWTs. `Some` in production, where every
    /// token is an Ed25519 JWT; `None` for local development, which falls back
    /// to the shared secrets above so the demo needs no key material.
    jwks: Option<Jwks>,
}

/// What a viewer socket is allowed to do (§7.5).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Control {
    /// V-04: observation without the lease.
    Read,
    /// V-02: full input control.
    Write,
}

#[tokio::main]
async fn main() {
    let bind = std::env::var("IAPETUS_GATEWAY_BIND").unwrap_or_else(|_| "0.0.0.0:8080".into());
    let ingest_token = std::env::var("IAPETUS_INGEST_TOKEN").unwrap_or_else(|_| "dev".into());
    let view_token = std::env::var("IAPETUS_VIEW_TOKEN").unwrap_or_else(|_| "dev".into());
    let write_token = std::env::var("IAPETUS_WRITE_TOKEN").unwrap_or_else(|_| "dev-write".into());
    let jwks = std::env::var("IAPETUS_JWKS").ok().and_then(|s| parse_jwks(&s));
    if jwks.is_some() {
        println!("verifying §8.1 JWTs against the configured JWKS");
    } else {
        println!("no JWKS configured; using development shared-secret tokens");
    }

    // Capacity 32: a viewer that cannot keep up should skip ahead to the newest
    // screen rather than fall further behind replaying old ones. Lag is
    // reported to the viewer as dropped updates and recovered with a keyframe.
    let (frames, _) = broadcast::channel(32);
    let (to_guest, _) = broadcast::channel(256);
    let (control_events, _) = broadcast::channel(64);

    let app = Arc::new(App {
        frames,
        to_guest,
        cache: Mutex::new(TileCache::default()),
        lease: std::sync::Mutex::new(ControlLease::new()),
        control_events,
        started: Instant::now(),
        next_session: AtomicU64::new(1),
        pending: std::sync::Mutex::new(std::collections::HashMap::new()),
        next_action: AtomicU64::new(1),
        guest_attached: std::sync::atomic::AtomicBool::new(false),
        ingest_token,
        view_token,
        write_token,
        jwks,
    });

    // Reap the lease on a timer, so a human who idles out or a holder that
    // stops heartbeating frees the lease even while nobody is contending for it
    // (§5.6). One second is well inside the 300s idle and 90s heartbeat windows.
    {
        let app = Arc::clone(&app);
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(Duration::from_secs(1));
            loop {
                tick.tick().await;
                app.reap_lease();
            }
        });
    }

    let router = Router::new()
        .route("/", get(|| async { Html(include_str!("viewer.html")) }))
        .route("/view", get(view_ws))
        .route("/ingest", get(ingest_ws))
        .route("/v1/action", post(action))
        .with_state(app);

    let listener = tokio::net::TcpListener::bind(&bind).await.expect("bind");
    println!("gateway listening on http://{bind}");
    axum::serve(listener, router).await.expect("serve");
}

async fn ingest_ws(
    ws: WebSocketUpgrade,
    State(app): State<Arc<App>>,
    axum::extract::Query(q): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> impl IntoResponse {
    if !app.ingest_ok(q.get("token").map(String::as_str)) {
        return (axum::http::StatusCode::UNAUTHORIZED, "bad ingest token").into_response();
    }
    if !app.try_claim_guest() {
        return (axum::http::StatusCode::CONFLICT, "a guest is already streaming").into_response();
    }
    ws.on_upgrade(move |socket| guest_socket(socket, app))
}

/// One guest, pushing screen updates and receiving input.
async fn guest_socket(socket: WebSocket, app: Arc<App>) {
    use futures_util::{SinkExt, StreamExt};
    let (mut tx, mut rx) = socket.split();
    let mut inbound = app.to_guest.subscribe();

    println!("guest attached");

    // The guest holds the lease as an agent while it is attached, so a viewer
    // pressing "operate" preempts it (§5.6) rather than picking up a free
    // lease. The agent has no input path here yet — §19.5 is the Control Plane
    // channel — but holding WRITE is what makes the human takeover a real one.
    {
        let mut lease = app.lease.lock().unwrap();
        let _ = lease.acquire(GUEST_SESSION, &Actor::agent("stream-agent"), app.now(), HEARTBEAT);
    }
    app.broadcast_control();

    let pump = tokio::spawn(async move {
        while let Ok(msg) = inbound.recv().await {
            if tx.send(Message::Text(msg.into())).await.is_err() {
                break;
            }
        }
    });

    while let Some(Ok(msg)) = rx.next().await {
        match msg {
            Message::Binary(buf) => {
                app.cache.lock().await.apply(&buf);
                // A send with no receivers is not an error: nobody is watching,
                // and the guest should keep streaming so the cache stays warm
                // for whoever opens a tab next.
                let _ = app.frames.send(Arc::new(buf.to_vec()));
            }
            // An action result: match it to the waiting request by its id and
            // hand it back. A result for an id nobody is waiting on (the request
            // timed out and gave up) is simply dropped.
            Message::Text(t) => {
                if let Some(id) = serde_json::from_str::<serde_json::Value>(&t)
                    .ok()
                    .and_then(|v| v.get("id").and_then(serde_json::Value::as_u64))
                {
                    if let Some(waiter) = app.pending.lock().unwrap().remove(&id) {
                        let _ = waiter.send(t.to_string());
                    }
                }
            }
            _ => {}
        }
    }

    pump.abort();
    {
        let mut lease = app.lease.lock().unwrap();
        let _ = lease.release(GUEST_SESSION);
    }
    app.broadcast_control();
    app.release_guest();
    println!("guest detached");
}

async fn view_ws(
    ws: WebSocketUpgrade,
    State(app): State<Arc<App>>,
    axum::extract::Query(q): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> impl IntoResponse {
    let Some(level) = app.control_for(q.get("token").map(String::as_str)) else {
        return (axum::http::StatusCode::UNAUTHORIZED, "bad viewer token").into_response();
    };
    ws.on_upgrade(move |socket| viewer_socket(socket, app, level))
}

/// One browser.
async fn viewer_socket(socket: WebSocket, app: Arc<App>, level: Control) {
    use futures_util::{SinkExt, StreamExt};
    let (mut tx, mut rx) = socket.split();

    let session = format!("v{}", app.next_session.fetch_add(1, Ordering::SeqCst));

    // Subscribe *before* sending the bootstrap, or an update landing in between
    // is lost and that region stays stale until it next changes.
    let mut frames = app.frames.subscribe();
    let mut control = app.control_events.subscribe();

    match app.cache.lock().await.keyframe() {
        Some(kf) => {
            let _ = tx.send(Message::Binary(kf.into())).await;
        }
        // Nothing cached yet — ask the guest for one rather than showing a
        // blank canvas until something on screen happens to move.
        None => {
            let _ = app.to_guest.send(r#"{"type":"keyframe"}"#.to_string());
        }
    }

    // Tell this viewer the current holder immediately, so its button reflects
    // reality before anything else happens.
    let _ = tx
        .send(Message::Text(control_snapshot(&app, &session).into()))
        .await;

    let out = tokio::spawn(async move {
        loop {
            tokio::select! {
                frame = frames.recv() => match frame {
                    Ok(buf) => {
                        if tx.send(Message::Binary(buf.as_ref().clone().into())).await.is_err() {
                            break;
                        }
                    }
                    // Fell behind; the canvas has holes, so ask for a keyframe
                    // rather than applying the next diff onto them.
                    Err(broadcast::error::RecvError::Lagged(_)) => {
                        if tx.send(Message::Text(r#"{"type":"lagged"}"#.into())).await.is_err() {
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                },
                ev = control.recv() => match ev {
                    Ok(msg) => {
                        if tx.send(Message::Text(msg.into())).await.is_err() {
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => {}
                    Err(broadcast::error::RecvError::Closed) => break,
                },
            }
        }
    });

    // Input, gated on the §5.6 lease. A WRITE *token* only means this viewer
    // may *request* the lease; the lease itself decides whether input passes,
    // which is what lets several people watch while one operates (§7.5 V-10).
    while let Some(Ok(msg)) = rx.next().await {
        let Message::Text(t) = msg else { continue };
        let Ok(env) = serde_json::from_str::<Envelope>(&t) else {
            continue; // unclassifiable — the guest would drop it too
        };

        match env.kind.as_str() {
            "acquire" => handle_acquire(&app, &session, level).await,
            "release" => handle_release(&app, &session).await,
            // A keyframe request is not input; a READ observer still needs a
            // picture. Everything else is input and requires the lease.
            "keyframe" => {
                let _ = app.to_guest.send(t.to_string());
            }
            _ => {
                if app.lease.lock().unwrap().holds_write(&session) {
                    // Every forwarded input is also a heartbeat's worth of
                    // activity, which is what keeps a busy human's lease from
                    // idling out from under them (§5.6).
                    app.lease.lock().unwrap().mark_input(&session, app.now());
                    let _ = app.to_guest.send(t.to_string());
                }
                // Not the holder: silently dropped. An observer's stray click
                // must not reach a desktop it has no lease on.
            }
        }
    }

    // A viewer that closes its tab while holding the lease must free it, or the
    // desktop is stuck until the lease idles out.
    if let Some(_r) = app.lease.lock().unwrap().release(&session) {
        let _ = app.to_guest.send(r#"{"type":"release_all"}"#.to_string());
    }
    app.broadcast_control();
    out.abort();
}

/// A viewer pressed "operate". A WRITE token is required to even try; the lease
/// then decides between granting, preempting the agent, and refusing.
async fn handle_acquire(app: &Arc<App>, session: &str, level: Control) {
    if level != Control::Write {
        // A READ token cannot operate — that is the whole distinction (§7.5).
        let _ = app.control_events.send(
            format!(r#"{{"type":"denied","session":"{session}","reason":"read_only"}}"#),
        );
        return;
    }

    let outcome = {
        let mut lease = app.lease.lock().unwrap();
        lease.acquire(session, &Actor::human(session), app.now(), HEARTBEAT)
    };

    match outcome {
        Ok(Acquired::Preempted { .. }) => {
            // §5.6: before the human's first keystroke, the agent's held keys
            // are released so a latched Ctrl does not turn typing into
            // shortcuts. The guest owns that release.
            let _ = app.to_guest.send(r#"{"type":"release_all"}"#.to_string());
            app.broadcast_control();
        }
        Ok(_) => app.broadcast_control(),
        Err(held) => {
            // CONTROL_HELD — another person is operating (§5.6 forbids one
            // human preempting another). Tell just this viewer, with the retry
            // hint, rather than broadcasting a non-event to everyone.
            let holder = match held.holder.kind {
                iapetus_proto::lease::ActorType::Human => "human",
                _ => "agent",
            };
            let _ = app.control_events.send(format!(
                r#"{{"type":"denied","session":"{session}","reason":"held","holder":"{holder}","retry_after_sec":{}}}"#,
                held.retry_after.as_secs()
            ));
        }
    }
}

async fn handle_release(app: &Arc<App>, session: &str) {
    let released = app.lease.lock().unwrap().release(session);
    if released.is_some() {
        // Even a clean release leaves a clean input state for whoever is next.
        let _ = app.to_guest.send(r#"{"type":"release_all"}"#.to_string());
        app.broadcast_control();
    }
}

/// The current holder, framed for one viewer so it can set its button state.
fn control_snapshot(app: &App, _session: &str) -> String {
    let lease = app.lease.lock().unwrap();
    let holder = lease.holder().map(|a| a.kind);
    let session = lease.holder_session().unwrap_or_default();
    let kind = match holder {
        Some(iapetus_proto::lease::ActorType::Human) => "human",
        Some(iapetus_proto::lease::ActorType::Agent) => "agent",
        _ => "none",
    };
    format!(r#"{{"type":"control","holder":"{kind}","session":"{session}"}}"#)
}

/// How long an agent action waits for the guest before giving up. Past this the
/// caller gets a timeout, and a late result is dropped when it arrives — the
/// §8.5/§19.5 rule that an action past its deadline is not answered twice.
const ACTION_TIMEOUT: Duration = Duration::from_secs(30);

/// An agent action, forwarded to the guest, its result awaited and returned.
///
/// §8.5: requests on one connection execute in arrival order, and the guest
/// answers on the same socket it streams on. The gateway assigns the id, sends
/// the action down `to_guest`, and parks on a oneshot until the guest's reply
/// comes back through `guest_socket`.
///
/// Auth reuses the viewer token: an agent driving through this path holds a
/// WRITE-capable token exactly as an operating human does, and the same control
/// level gate applies.
async fn action(
    State(app): State<Arc<App>>,
    axum::extract::Query(q): axum::extract::Query<std::collections::HashMap<String, String>>,
    body: String,
) -> impl IntoResponse {
    let level = match app.control_for(q.get("token").map(String::as_str)) {
        Some(l) => l,
        None => return (axum::http::StatusCode::UNAUTHORIZED, "bad token").into_response(),
    };
    if level != Control::Write {
        return (axum::http::StatusCode::FORBIDDEN, "a read-only token cannot act").into_response();
    }

    // The agent sends the bare action JSON; the gateway stamps the id it will
    // match the reply on, so two in-flight actions cannot be confused.
    let id = app.next_action.fetch_add(1, Ordering::SeqCst);
    let stamped = match serde_json::from_str::<serde_json::Value>(&body) {
        Ok(mut v) => {
            v["id"] = serde_json::json!(id);
            v.to_string()
        }
        Err(_) => return (axum::http::StatusCode::BAD_REQUEST, "action is not JSON").into_response(),
    };

    let (tx, rx) = tokio::sync::oneshot::channel();
    app.pending.lock().unwrap().insert(id, tx);

    if app.to_guest.send(stamped).is_err() {
        app.pending.lock().unwrap().remove(&id);
        return (axum::http::StatusCode::SERVICE_UNAVAILABLE, "no desktop attached").into_response();
    }

    match tokio::time::timeout(ACTION_TIMEOUT, rx).await {
        Ok(Ok(result)) => (
            [(axum::http::header::CONTENT_TYPE, "application/json")],
            result,
        )
            .into_response(),
        // Timed out or the guest dropped: clean up the pending slot so a late
        // reply is discarded rather than delivered to the next request.
        _ => {
            app.pending.lock().unwrap().remove(&id);
            (axum::http::StatusCode::GATEWAY_TIMEOUT, "the desktop did not answer").into_response()
        }
    }
}

/// Wall-clock seconds since the epoch, for JWT expiry. Distinct from the lease's
/// monotonic `now`: a token's exp is an absolute time, a lease's timers are
/// relative, and conflating them would check one against the wrong clock.
fn now_epoch() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Parses `IAPETUS_JWKS` — `kid:base64url-32-byte-key` entries, comma-separated.
///
/// A deliberately small format: the served `/.well-known/jwks.json` is the
/// Control Plane's job, and the gateway only needs the public keys by kid. A
/// malformed entry drops that key rather than failing startup, so one bad line
/// does not take the gateway down, but an empty result returns `None` so the
/// caller falls back to dev secrets rather than refusing every token.
fn parse_jwks(raw: &str) -> Option<Jwks> {
    use base64::Engine;
    let b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD;
    let mut jwks = Jwks::new();
    let mut any = false;
    for entry in raw.split(',').map(str::trim).filter(|e| !e.is_empty()) {
        let Some((kid, key_b64)) = entry.split_once(':') else { continue };
        let Ok(bytes) = b64.decode(key_b64) else { continue };
        let Ok(arr): Result<[u8; 32], _> = bytes.try_into() else { continue };
        let Ok(vk) = ed25519_dalek::VerifyingKey::from_bytes(&arr) else { continue };
        jwks.insert(kid.to_string(), vk);
        any = true;
    }
    any.then_some(jwks)
}

/// Just the discriminator; the guest validates the rest.
///
/// Parsed, never substring-matched. An earlier gate looked for the literal
/// `"keyframe"` anywhere in the text, which let a READ viewer smuggle input by
/// embedding that string in a `type` payload. The discriminator is the only
/// honest place to classify a message, and one that does not parse is dropped.
#[derive(serde::Deserialize)]
struct Envelope {
    #[serde(rename = "type")]
    kind: String,
}

impl App {
    /// The lease's monotonic clock, in milliseconds since the gateway started.
    fn now(&self) -> u64 {
        self.started.elapsed().as_millis() as u64
    }

    /// Tells every viewer who now holds the lease, so each renders "you are
    /// operating" / "the agent is operating" without polling.
    fn broadcast_control(&self) {
        let holder = self.lease.lock().unwrap().holder().map(|a| a.kind).map_or(
            "none",
            |k| match k {
                iapetus_proto::lease::ActorType::Human => "human",
                iapetus_proto::lease::ActorType::Agent => "agent",
                _ => "none",
            },
        );
        // Names the holding session too, so a viewer can tell whether the human
        // operating is itself or someone else at another browser (V-10).
        let session = self.lease.lock().unwrap().holder_session().unwrap_or_default();
        let _ = self
            .control_events
            .send(format!(r#"{{"type":"control","holder":"{holder}","session":"{session}"}}"#));
    }

    /// Reaps an expired or idled-out holder on a timer, so a lease that lapsed
    /// with nobody contending still frees and still tells the viewers (§5.6).
    fn reap_lease(&self) {
        let revoked = self.lease.lock().unwrap().reap(self.now());
        if let Some(_r) = revoked {
            // A reaped agent lease needs no key release — the guest already
            // released on detach — but a reaped human one does, and either way
            // the viewers must learn the lease is free.
            let _ = self.to_guest.send(r#"{"type":"release_all"}"#.to_string());
            self.broadcast_control();
        }
    }

    /// Claims the single guest slot; `false` means one is already attached.
    fn try_claim_guest(&self) -> bool {
        !self.guest_attached.swap(true, std::sync::atomic::Ordering::SeqCst)
    }

    fn release_guest(&self) {
        self.guest_attached.store(false, std::sync::atomic::Ordering::SeqCst);
    }

    /// Resolves a viewer token to what it may do, or `None` to refuse.
    ///
    /// With a JWKS configured this is a real §8.1 Viewer Token: verified, then
    /// mapped to WRITE if it carries `desktop:control` and READ otherwise. The
    /// token only grants the *right to request* the lease — the lease itself
    /// still decides whether input passes (§7.5). Without a JWKS it falls back
    /// to the development shared secrets.
    fn control_for(&self, token: Option<&str>) -> Option<Control> {
        if let Some(jwks) = &self.jwks {
            let policy = Policy {
                audience: AUDIENCE,
                lifetime_cap_sec: Some(8 * 3600), // viewer cap (§8.1)
            };
            let claims = iapetus_auth::verify(token?, jwks, &policy, now_epoch()).ok()?;
            return Some(if claims.has_scope("desktop:control") {
                Control::Write
            } else {
                Control::Read
            });
        }
        match token {
            Some(t) if t == self.write_token => Some(Control::Write),
            Some(t) if t == self.view_token => Some(Control::Read),
            _ => None,
        }
    }

    /// Whether a guest token authorizes the §19.5 stream.
    ///
    /// A Guest token carries the `iapetus-guest` audience and no scopes (§9.1);
    /// checking the audience is what stops a Viewer or Agent token from being
    /// replayed to push a screen.
    fn ingest_ok(&self, token: Option<&str>) -> bool {
        if let Some(jwks) = &self.jwks {
            let policy = Policy { audience: AUDIENCE_GUEST, lifetime_cap_sec: None };
            return token.is_some_and(|t| iapetus_auth::verify(t, jwks, &policy, now_epoch()).is_ok());
        }
        token == Some(self.ingest_token.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The bytes cached at a position, for tests that care about content.
    fn at(c: &TileCache, x: u16, y: u16) -> &[u8] {
        &c.regions.iter().rev().find(|r| r.x == x && r.y == y).expect("no region there").jpeg
    }

    fn app() -> App {
        let (frames, _) = broadcast::channel(4);
        let (to_guest, _) = broadcast::channel(4);
        let (control_events, _) = broadcast::channel(8);
        App {
            frames,
            to_guest,
            cache: Mutex::new(TileCache::default()),
            lease: std::sync::Mutex::new(ControlLease::new()),
            control_events,
            started: Instant::now(),
            next_session: AtomicU64::new(1),
            pending: std::sync::Mutex::new(std::collections::HashMap::new()),
            next_action: AtomicU64::new(1),
            guest_attached: std::sync::atomic::AtomicBool::new(false),
            ingest_token: "ing".into(),
            view_token: "look".into(),
            write_token: "drive".into(),
            jwks: None,
        }
    }

    /// A gateway whose tokens are real JWTs, for the auth-path tests.
    fn app_with_jwks() -> (App, iapetus_auth::Issuer) {
        use ed25519_dalek::SigningKey;
        let issuer = iapetus_auth::Issuer::new("k1", SigningKey::from_bytes(&[3u8; 32]), "iss");
        let mut jwks = Jwks::new();
        jwks.insert("k1", issuer.verifying_key());
        let mut a = app();
        a.jwks = Some(jwks);
        (a, issuer)
    }

    fn viewer_token(issuer: &iapetus_auth::Issuer, scopes: &[&str], aud: &str) -> String {
        let now = now_epoch();
        issuer.sign(iapetus_auth::Claims {
            jti: "j".into(),
            iss: "unset".into(),
            aud: aud.into(),
            sub: "usr".into(),
            actor_type: iapetus_auth::ActorType::Human,
            project_id: "p".into(),
            desktop_ids: vec!["dsk".into()],
            scopes: scopes.iter().map(|s| s.to_string()).collect(),
            iat: now,
            exp: now + 900,
            orig_iat: now,
        })
    }

    #[test]
    fn a_message_is_classified_by_its_parsed_discriminator_not_a_substring() {
        // The regression this guards: the gate used to substring-match
        // `"keyframe"`, so a READ viewer could embed it in a `type` payload and
        // slip input past. Classification is now by the parsed field, and an
        // unparseable message has no kind at all.
        let evil: Envelope =
            serde_json::from_str(r#"{"type":"type","text":"stolen \"keyframe\" text"}"#).unwrap();
        assert_eq!(evil.kind, "type", "a type message must classify as input, not keyframe");

        let kf: Envelope = serde_json::from_str(r#"{"type":"keyframe"}"#).unwrap();
        assert_eq!(kf.kind, "keyframe");

        assert!(serde_json::from_str::<Envelope>("not json").is_err());
    }

    #[test]
    fn a_real_jwt_maps_to_write_or_read_by_its_control_scope() {
        // With a JWKS the shared secrets are gone: a token is a §8.1 JWT, and
        // desktop:control is what separates operating from observing.
        let (a, iss) = app_with_jwks();

        let writer = viewer_token(&iss, &["desktop:control"], AUDIENCE);
        assert_eq!(a.control_for(Some(&writer)), Some(Control::Write));

        let reader = viewer_token(&iss, &["desktop:read"], AUDIENCE);
        assert_eq!(a.control_for(Some(&reader)), Some(Control::Read));

        // A forged token — signed by the wrong key — is refused outright.
        let forged = iapetus_auth::Issuer::new("k1", ed25519_dalek::SigningKey::from_bytes(&[9u8; 32]), "x")
            .sign(iapetus_auth::Claims {
                jti: "j".into(), iss: "x".into(), aud: AUDIENCE.into(), sub: "attacker".into(),
                actor_type: iapetus_auth::ActorType::Human, project_id: "p".into(),
                desktop_ids: vec![], scopes: vec!["desktop:control".into()],
                iat: now_epoch(), exp: now_epoch() + 900, orig_iat: now_epoch(),
            });
        assert_eq!(a.control_for(Some(&forged)), None, "a token signed by the wrong key was accepted");

        // The dev shared secret is not a JWT, so once a JWKS is set it is refused.
        assert_eq!(a.control_for(Some("drive")), None);
    }

    #[test]
    fn a_guest_token_is_refused_at_the_viewer_endpoint_and_vice_versa() {
        // §9.1: audiences keep the roles apart. A Guest token pushes screens; a
        // Viewer token operates. Neither may stand in for the other.
        let (a, iss) = app_with_jwks();
        let guest = viewer_token(&iss, &[], AUDIENCE_GUEST);
        let viewer = viewer_token(&iss, &["desktop:control"], AUDIENCE);

        assert_eq!(a.control_for(Some(&guest)), None, "a guest token operated a viewer");
        assert!(!a.ingest_ok(Some(&viewer)), "a viewer token pushed a screen");
        assert!(a.ingest_ok(Some(&guest)), "a valid guest token was refused ingest");
    }

    #[test]
    fn an_action_reply_is_matched_to_its_waiter_by_id() {
        // §8.5: the guest answers on the stream socket, and the gateway matches
        // the reply to the parked request by the id it stamped. A reply for an
        // id nobody waits on (the request timed out) is dropped, not misdelivered.
        let a = app();
        let (tx, rx) = tokio::sync::oneshot::channel();
        a.pending.lock().unwrap().insert(7, tx);

        // The guest's reply carries id 7.
        let reply = r#"{"type":"result","id":7,"ok":true}"#;
        let id = serde_json::from_str::<serde_json::Value>(reply)
            .unwrap()
            .get("id")
            .and_then(serde_json::Value::as_u64)
            .unwrap();
        if let Some(w) = a.pending.lock().unwrap().remove(&id) {
            w.send(reply.to_string()).unwrap();
        }
        assert_eq!(rx.blocking_recv().unwrap(), reply);

        // A reply for an unknown id finds no waiter and is simply dropped.
        assert!(a.pending.lock().unwrap().remove(&999).is_none());
    }

    #[test]
    fn only_one_guest_may_stream_at_a_time() {
        // A second guest's frames would interleave into the same broadcast and
        // every viewer would watch two screens shuffled together.
        let a = app();
        assert!(a.try_claim_guest());
        assert!(!a.try_claim_guest(), "a second guest was accepted");
        a.release_guest();
        assert!(a.try_claim_guest(), "the slot did not free on detach");
    }

    #[test]
    fn an_unauthenticated_viewer_gets_nothing() {
        // The endpoint carries both the screen and the keyboard. Open to
        // anything that can reach the port, it hands a stranger the desktop.
        let a = app();
        assert_eq!(a.control_for(None), None);
        assert_eq!(a.control_for(Some("")), None);
        assert_eq!(a.control_for(Some("guess")), None);
        // The ingest token must not double as a viewer token: the guest's
        // credential is not a person's.
        assert_eq!(a.control_for(Some("ing")), None);
    }

    #[test]
    fn a_read_token_observes_and_a_write_token_operates() {
        // §7.5: levels 2 and 3 cost the same infrastructure and differ only by
        // the control lease.
        let a = app();
        assert_eq!(a.control_for(Some("look")), Some(Control::Read));
        assert_eq!(a.control_for(Some("drive")), Some(Control::Write));
    }

    /// Builds an update in the guest's wire format.
    fn update(w: u16, h: u16, keyframe: bool, tiles: &[(u16, u16, u16, u16, &[u8])]) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&1u32.to_be_bytes());
        out.extend_from_slice(&w.to_be_bytes());
        out.extend_from_slice(&h.to_be_bytes());
        out.push(u8::from(keyframe));
        out.push(0);
        out.extend_from_slice(&(tiles.len() as u16).to_be_bytes());
        for (x, y, tw, th, jpeg) in tiles {
            out.extend_from_slice(&x.to_be_bytes());
            out.extend_from_slice(&y.to_be_bytes());
            out.extend_from_slice(&tw.to_be_bytes());
            out.extend_from_slice(&th.to_be_bytes());
            out.extend_from_slice(&(jpeg.len() as u32).to_be_bytes());
            out.extend_from_slice(jpeg);
        }
        out
    }

    #[test]
    fn a_later_diff_replaces_only_the_tiles_it_carries() {
        // This is what lets a viewer join mid-stream: the cache must hold the
        // newest version of every position, not just the last update.
        let mut c = TileCache::default();
        c.apply(&update(128, 64, true, &[(0, 0, 64, 64, b"A"), (64, 0, 64, 64, b"B")]))
            .unwrap();
        c.apply(&update(128, 64, false, &[(64, 0, 64, 64, b"B2")])).unwrap();

        // The untouched tile survives and the changed one is present; the
        // replaced copy must not linger underneath.
        assert_eq!(c.regions.len(), 2, "a superseded copy was kept");
        assert_eq!(at(&c, 0, 0), b"A", "an untouched tile was lost");
        assert_eq!(at(&c, 64, 0), b"B2", "a changed tile was not replaced");
    }

    #[test]
    fn a_keyframe_clears_tiles_the_new_frame_does_not_mention() {
        let mut c = TileCache::default();
        c.apply(&update(128, 64, true, &[(0, 0, 64, 64, b"A"), (64, 0, 64, 64, b"B")]))
            .unwrap();
        c.apply(&update(128, 64, true, &[(0, 0, 64, 64, b"C")])).unwrap();
        assert_eq!(c.regions.len(), 1, "a stale tile survived a keyframe");
    }

    #[test]
    fn a_resize_drops_every_cached_tile() {
        // The old tiles describe a screen that no longer exists. Handing them to
        // a viewer paints regions that nothing will ever overwrite.
        let mut c = TileCache::default();
        c.apply(&update(128, 64, true, &[(0, 0, 64, 64, b"A")])).unwrap();
        c.apply(&update(256, 128, false, &[(0, 0, 64, 64, b"Z")])).unwrap();
        assert_eq!(c.regions.len(), 1);
        assert_eq!((c.width, c.height), (256, 128));
    }

    #[test]
    fn a_region_that_covers_an_older_one_wins_in_the_rebuilt_keyframe() {
        // Regions are arbitrary rectangles, not a fixed grid: the guest sends a
        // small tile when one thing moved and one big rectangle when much did.
        // So a later region can cover an earlier one, and rebuilding in position
        // order would draw the stale small tile on top of the fresh big one —
        // a viewer joining late would see a patch of the past.
        let mut c = TileCache::default();
        c.apply(&update(256, 128, true, &[(0, 0, 256, 128, b"OLD-FULL")])).unwrap();
        c.apply(&update(256, 128, false, &[(100, 40, 64, 64, b"PATCH")])).unwrap();
        c.apply(&update(256, 128, false, &[(0, 0, 256, 128, b"NEW-FULL")])).unwrap();

        let kf = c.keyframe().expect("no keyframe");
        let mut round = TileCache::default();
        round.apply(&kf).unwrap();

        assert_eq!(
            round.regions.len(),
            1,
            "the covered patch was kept and would repaint stale pixels"
        );
        assert_eq!(at(&round, 0, 0), b"NEW-FULL");
    }

    #[test]
    fn the_rebuilt_keyframe_is_byte_compatible_with_the_guests_own() {
        // The viewer must not need a second parser for bootstrap frames.
        let mut c = TileCache::default();
        c.apply(&update(128, 64, true, &[(0, 0, 64, 64, b"AA"), (64, 0, 64, 64, b"BBB")]))
            .unwrap();

        let kf = c.keyframe().expect("no keyframe");
        assert_eq!(u16::from_be_bytes(kf[4..6].try_into().unwrap()), 128);
        assert_eq!(u16::from_be_bytes(kf[6..8].try_into().unwrap()), 64);
        assert_eq!(kf[8], 1, "bootstrap frame must be marked as a keyframe");
        assert_eq!(u16::from_be_bytes(kf[10..12].try_into().unwrap()), 2);

        let mut round = TileCache::default();
        round.apply(&kf).expect("the rebuilt keyframe did not parse");
        assert_eq!(round.regions, c.regions, "the rebuild is not order-preserving");
    }

    #[test]
    fn an_empty_cache_yields_no_keyframe_so_the_guest_is_asked_for_one() {
        assert!(TileCache::default().keyframe().is_none());
    }

    #[test]
    fn a_truncated_update_is_rejected_rather_than_half_applied() {
        // Half-applying leaves the cache describing a frame that never existed,
        // and every viewer bootstrapped from it inherits the corruption.
        let mut c = TileCache::default();
        c.apply(&update(128, 64, true, &[(0, 0, 64, 64, b"GOOD")])).unwrap();

        let mut buf = update(128, 64, false, &[(0, 0, 64, 64, b"AAAA"), (64, 0, 64, 64, b"BBBB")]);
        buf.truncate(buf.len() - 2);
        assert!(c.apply(&buf).is_none());
        assert_eq!(c.regions.len(), 1, "a partial update reached the cache");
        assert_eq!(at(&c, 0, 0), b"GOOD", "the good frame was overwritten by a bad one");

        let mut short = TileCache::default();
        assert!(short.apply(&[0u8; 4]).is_none());
    }

    #[test]
    fn the_cache_cannot_grow_without_bound() {
        // Partially-overlapping rectangles are never evicted by the covering
        // rule, so a long-lived stream of odd shapes would otherwise grow
        // forever. Clearing is self-healing — the next viewer asks the guest
        // for a keyframe — where a leak is not.
        let mut c = TileCache::default();
        for i in 0..(MAX_REGIONS + 10) {
            let x = (i % 500) as u16;
            c.apply(&update(2000, 2000, false, &[(x, 0, 1, 1, b"x")])).unwrap();
        }
        assert!(c.regions.len() <= MAX_REGIONS, "cache grew to {}", c.regions.len());
    }
}
