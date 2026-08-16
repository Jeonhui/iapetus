//! Pushes the screen to the stream gateway and applies the input that comes
//! back (PRD §6.3 fallback, §7.5).
//!
//! The guest **dials out** here, exactly as it does for the Control Plane
//! channel: §9.1 keeps the Desktop free of inbound ports, so the gateway never
//! connects to us.
//!
//! Input arriving on this socket is turned into ordinary Computer API actions
//! and run through the same `Dispatcher` an agent's actions go through. §7.5 is
//! explicit that there is one input path — a second one would split the audit
//! log and make lease arbitration impossible.

use std::sync::Arc;
use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use iapetus_proto::v1::{self, action::Kind};
use serde::Deserialize;
use tokio_tungstenite::tungstenite::Message;

use crate::dispatch::Dispatcher;
use crate::stream::{TileEncoder, DEFAULT_QUALITY, MAX_FPS};

/// What the viewer sends. Untagged variants would let a malformed message be
/// silently read as a different action, so the discriminator is explicit.
#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
enum ViewerInput {
    #[serde(rename = "mouse.move")]
    MouseMove { x: i32, y: i32 },
    #[serde(rename = "mouse.down")]
    MouseDown { button: String },
    #[serde(rename = "mouse.up")]
    MouseUp { button: String },
    #[serde(rename = "scroll")]
    Scroll { x: i32, y: i32, dx: i32, dy: i32 },
    #[serde(rename = "type")]
    Type { text: String },
    #[serde(rename = "key")]
    Key { keys: String },
    /// A viewer joined, or lost its place in the stream, and needs a full frame.
    #[serde(rename = "keyframe")]
    Keyframe,
}

fn button(name: &str) -> i32 {
    match name {
        "right" => v1::MouseButton::Right as i32,
        "middle" => v1::MouseButton::Middle as i32,
        _ => v1::MouseButton::Left as i32,
    }
}

/// Converts one viewer message into the action an agent would have sent.
fn to_action(input: ViewerInput) -> Option<v1::Action> {
    let kind = match input {
        ViewerInput::MouseMove { x, y } => {
            Kind::MouseMove(v1::MouseMove { to: Some(v1::Point { x, y }), duration_ms: None })
        }
        ViewerInput::MouseDown { button: b } => {
            Kind::MouseDown(v1::MouseUpDown { button: button(&b) })
        }
        ViewerInput::MouseUp { button: b } => Kind::MouseUp(v1::MouseUpDown { button: button(&b) }),
        ViewerInput::Scroll { x, y, dx, dy } => {
            Kind::Scroll(v1::Scroll { at: Some(v1::Point { x, y }), dx, dy })
        }
        // Text goes through `type` rather than synthesized keycodes so it takes
        // the IME path; §15.2 is about exactly this (Hangul would otherwise
        // arrive as jamo).
        ViewerInput::Type { text } => Kind::TypeText(v1::TypeText { text, delay_ms: None }),
        ViewerInput::Key { keys } => Kind::Key(v1::KeyPress { keys, count: Some(1) }),
        ViewerInput::Keyframe => return None,
    };
    Some(v1::Action { kind: Some(kind) })
}

/// Runs the viewer link until the socket closes.
///
/// `endpoint` is the gateway's `/ingest` URL, which the guest dials.
pub async fn run(
    endpoint: &str,
    dispatcher: Arc<Dispatcher>,
    frame_interval: Duration,
) -> std::result::Result<(), String> {
    let (socket, _) = tokio_tungstenite::connect_async(endpoint)
        .await
        .map_err(|e| format!("dialing {endpoint}: {e}"))?;
    let (mut tx, mut rx) = socket.split();

    let (kf_tx, mut kf_rx) = tokio::sync::mpsc::channel::<()>(8);

    // ── Input, viewer → guest ────────────────────────────────
    let input_task = {
        let d = Arc::clone(&dispatcher);
        tokio::spawn(async move {
            while let Some(Ok(msg)) = rx.next().await {
                let Message::Text(t) = msg else { continue };
                let Ok(parsed) = serde_json::from_str::<ViewerInput>(&t) else {
                    // A message we do not understand is dropped rather than
                    // guessed at: guessing turns a viewer bug into input the
                    // person never asked for, on a desktop they own.
                    continue;
                };
                if matches!(parsed, ViewerInput::Keyframe) {
                    let _ = kf_tx.send(()).await;
                    continue;
                }
                let Some(action) = to_action(parsed) else { continue };
                // Blocking drivers, so off the runtime thread.
                let d2 = Arc::clone(&d);
                let _ = tokio::task::spawn_blocking(move || d2.execute_reported(&action)).await;
            }
        })
    };

    // ── Screen, guest → viewer ───────────────────────────────
    let mut enc = TileEncoder::new(DEFAULT_QUALITY);
    let mut next = Instant::now();
    let mut last_capture = Instant::now() - IDLE_RESYNC;
    loop {
        // A keyframe request jumps the queue: a viewer staring at a blank
        // canvas should not have to wait out the frame interval.
        let mut wanted = kf_rx.try_recv().is_ok();
        if wanted {
            enc.invalidate();
        }

        // Spend the interval waiting on XDamage rather than sleeping through
        // it. An untouched screen then costs nothing at all — capturing it
        // would encode a picture identical to the one already sent, which is
        // the waste §6.3 asks the Frame Source to avoid by idling.
        //
        // Damage returning early does not raise the frame rate: the remainder
        // of the interval is still held, so the §6.3 ceiling stands.
        let now = Instant::now();
        if !wanted && next > now {
            let budget = next - now;
            let d = Arc::clone(&dispatcher);
            match tokio::task::spawn_blocking(move || d.wait_for_change(budget)).await {
                Ok(Ok(changed)) => wanted |= changed,
                // A display error is reported by the capture below, which has
                // somewhere to put it; treating it as "something changed" gets
                // us there rather than spinning on the wait.
                Ok(Err(_)) => wanted = true,
                Err(e) => return Err(format!("damage wait task: {e}")),
            }
            let now = Instant::now();
            if next > now {
                tokio::time::sleep(next - now).await;
            }
        }
        next = Instant::now() + frame_interval;

        if !wanted && last_capture.elapsed() < IDLE_RESYNC {
            continue;
        }
        last_capture = Instant::now();

        let d = Arc::clone(&dispatcher);
        let update = match tokio::task::spawn_blocking(move || {
            d.capture_for_stream().map(|f| (f, ()))
        })
        .await
        {
            Ok(Ok((frame, ()))) => match enc.encode(&frame, false) {
                Ok(u) => u,
                Err(e) => return Err(format!("encoding a frame: {e}")),
            },
            Ok(Err(e)) => return Err(format!("capturing: {e}")),
            Err(e) => return Err(format!("capture task: {e}")),
        };

        // An update with no tiles is not sent. On an idle Desktop that is every
        // frame, and a heartbeat of empty packets would defeat the point of
        // diffing at all.
        if !update.tiles.is_empty()
            && tx.send(Message::Binary(update.to_bytes())).await.is_err()
        {
            break;
        }

        // The pacing wait happens at the top of the loop, where it can double as
        // the damage wait. Encoding that overran the interval simply means the
        // next wait is short or absent, which is the right behaviour: it does
        // not try to catch up by capturing back to back.
        if next < Instant::now() {
            next = Instant::now();
        }
    }

    input_task.abort();
    Ok(())
}

/// How long the stream will go without capturing, however quiet XDamage is.
///
/// A safety net, not a frame rate. Damage on the root window is reported by
/// every X server tested here, but a driver or a compositor that failed to
/// report some region would otherwise freeze the viewer indefinitely, and a
/// frozen picture is indistinguishable from a still one. Two seconds bounds
/// that to a visible stutter instead, at a cost of one capture per Desktop per
/// two seconds.
pub const IDLE_RESYNC: Duration = Duration::from_secs(2);

/// The interval matching §6.3's 5–10fps fallback ceiling.
#[must_use]
pub fn frame_interval() -> Duration {
    Duration::from_millis(1000 / u64::from(MAX_FPS))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(json: &str) -> ViewerInput {
        serde_json::from_str(json).expect("did not parse")
    }

    #[test]
    fn viewer_input_becomes_the_same_actions_an_agent_sends() {
        // §7.5: one input path. If these produced a private representation the
        // audit log would have two vocabularies for the same click.
        let a = to_action(parse(r#"{"type":"mouse.move","x":10,"y":20}"#)).unwrap();
        assert!(matches!(a.kind, Some(Kind::MouseMove(_))));

        let a = to_action(parse(r#"{"type":"mouse.down","button":"right"}"#)).unwrap();
        let Some(Kind::MouseDown(b)) = a.kind else { panic!() };
        assert_eq!(b.button, v1::MouseButton::Right as i32);

        let a = to_action(parse(r#"{"type":"key","keys":"ctrl+c"}"#)).unwrap();
        let Some(Kind::Key(k)) = a.kind else { panic!() };
        assert_eq!(k.keys, "ctrl+c");
    }

    #[test]
    fn typed_text_goes_through_type_not_synthesized_keys() {
        // §15.2: Hangul synthesized key by key arrives as jamo. The viewer sends
        // the composed string and it must stay on the `type` path.
        let a = to_action(parse(r#"{"type":"type","text":"안녕"}"#)).unwrap();
        let Some(Kind::TypeText(t)) = a.kind else { panic!("text did not use type") };
        assert_eq!(t.text, "안녕");
    }

    #[test]
    fn an_unknown_button_falls_back_to_left_rather_than_failing() {
        // A click with an unrecognised button is far more likely to mean the
        // usual one than to be worth dropping the person's input over.
        let a = to_action(parse(r#"{"type":"mouse.up","button":"whatever"}"#)).unwrap();
        let Some(Kind::MouseUp(b)) = a.kind else { panic!() };
        assert_eq!(b.button, v1::MouseButton::Left as i32);
    }

    #[test]
    fn a_keyframe_request_is_not_an_action() {
        assert!(to_action(parse(r#"{"type":"keyframe"}"#)).is_none());
    }

    #[test]
    fn a_message_with_an_unknown_type_is_refused_rather_than_guessed_at() {
        // Guessing turns a viewer bug into input the person never asked for, on
        // a desktop they own.
        assert!(serde_json::from_str::<ViewerInput>(r#"{"type":"format.disk"}"#).is_err());
        assert!(serde_json::from_str::<ViewerInput>(r#"{"x":1}"#).is_err());
    }

    #[test]
    fn the_frame_interval_stays_inside_the_fallback_ceiling() {
        // §6.3 puts this path at 5–10fps. Faster would spend encoding budget
        // §12.4 has not reserved.
        let i = frame_interval();
        assert!(i >= Duration::from_millis(100), "{i:?} is faster than 10fps");
        assert!(i <= Duration::from_millis(200), "{i:?} is slower than 5fps");
    }
}
