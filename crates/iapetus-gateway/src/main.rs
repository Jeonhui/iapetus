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

use std::sync::Arc;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::response::{Html, IntoResponse};
use axum::routing::get;
use axum::Router;
use tokio::sync::{broadcast, Mutex};

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
    /// The token that additionally grants `WRITE`. §7.5 puts this check in the
    /// gateway: it verifies the session holds the lease before forwarding
    /// input, because routing input through the Control Plane would add a round
    /// trip to the 20–50ms budget and destroy the feel.
    write_token: String,
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

    // Capacity 32: a viewer that cannot keep up should skip ahead to the newest
    // screen rather than fall further behind replaying old ones. Lag is
    // reported to the viewer as dropped updates and recovered with a keyframe.
    let (frames, _) = broadcast::channel(32);
    let (to_guest, _) = broadcast::channel(256);

    let app = Arc::new(App {
        frames,
        to_guest,
        cache: Mutex::new(TileCache::default()),
        ingest_token,
        view_token,
        write_token,
    });

    let router = Router::new()
        .route("/", get(|| async { Html(include_str!("viewer.html")) }))
        .route("/view", get(view_ws))
        .route("/ingest", get(ingest_ws))
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
    if q.get("token").map(String::as_str) != Some(app.ingest_token.as_str()) {
        return (axum::http::StatusCode::UNAUTHORIZED, "bad ingest token").into_response();
    }
    ws.on_upgrade(move |socket| guest_socket(socket, app))
}

/// One guest, pushing screen updates and receiving input.
async fn guest_socket(socket: WebSocket, app: Arc<App>) {
    use futures_util::{SinkExt, StreamExt};
    let (mut tx, mut rx) = socket.split();
    let mut inbound = app.to_guest.subscribe();

    println!("guest attached");

    let pump = tokio::spawn(async move {
        while let Ok(msg) = inbound.recv().await {
            if tx.send(Message::Text(msg.into())).await.is_err() {
                break;
            }
        }
    });

    while let Some(Ok(msg)) = rx.next().await {
        if let Message::Binary(buf) = msg {
            app.cache.lock().await.apply(&buf);
            // A send with no receivers is not an error: nobody is watching, and
            // the guest should keep streaming so the cache stays warm for
            // whoever opens a tab next.
            let _ = app.frames.send(Arc::new(buf.to_vec()));
        }
    }

    pump.abort();
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

    // Subscribe *before* sending the bootstrap, or an update landing in between
    // is lost and that region stays stale until it next changes.
    let mut frames = app.frames.subscribe();

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

    let out = tokio::spawn(async move {
        loop {
            match frames.recv().await {
                Ok(buf) => {
                    if tx.send(Message::Binary(buf.as_ref().clone().into())).await.is_err() {
                        break;
                    }
                }
                // This viewer fell behind and the channel dropped updates for
                // it. Its canvas now has holes, so it needs a fresh keyframe
                // rather than the next diff.
                Err(broadcast::error::RecvError::Lagged(_)) => {
                    let _ = tx.send(Message::Text(r#"{"type":"lagged"}"#.into())).await;
                }
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
    });

    // Input, straight through. The gateway does not interpret it — §7.5 puts
    // the coalescing in the viewer and the lease check here; turning it into
    // actions is the guest's job, on the same queue as agent actions.
    while let Some(Ok(msg)) = rx.next().await {
        let Message::Text(t) = msg else { continue };
        // A keyframe request is not input: a READ viewer still needs a picture,
        // and refusing it would leave an observer staring at a blank canvas.
        let is_keyframe = t.contains(r#""keyframe""#);
        if level == Control::Read && !is_keyframe {
            continue;
        }
        let _ = app.to_guest.send(t.to_string());
    }

    out.abort();
}

impl App {
    /// Resolves a viewer token to what it may do, or `None` to refuse.
    fn control_for(&self, token: Option<&str>) -> Option<Control> {
        match token {
            Some(t) if t == self.write_token => Some(Control::Write),
            Some(t) if t == self.view_token => Some(Control::Read),
            _ => None,
        }
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
        App {
            frames,
            to_guest,
            cache: Mutex::new(TileCache::default()),
            ingest_token: "ing".into(),
            view_token: "look".into(),
            write_token: "drive".into(),
        }
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
