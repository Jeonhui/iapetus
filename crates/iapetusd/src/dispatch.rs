//! Executes a Computer API action against the platform drivers.
//!
//! This is where the wire types from `iapetus-proto` meet the traits in
//! `platform`. Keeping it separate from the transport means the mapping is
//! testable without a Control Plane, and testable against both the fake backend
//! and a real X server.
//!
//! Two rules from the specification are enforced here rather than left to the
//! caller:
//!
//! * §8.2 caps — a batch larger than the limit, or text longer than the cap, is
//!   rejected before any of it executes. Half-applying a batch would leave the
//!   screen in a state the agent cannot reason about.
//! * §6.3 freshness — a screenshot taken after an input action must postdate it.

use std::time::{Duration, SystemTime};

use iapetus_proto::limits;
use iapetus_proto::v1::{self, action::Kind};

use crate::frame::FrameSource;
use crate::platform::{Button, Input, PlatformError, Rect};

#[derive(Debug, thiserror::Error)]
pub enum DispatchError {
    #[error("{0}")]
    Platform(#[from] PlatformError),
    #[error("action carried no payload")]
    Empty,
    #[error("batch of {got} exceeds the {max}-action limit")]
    BatchTooLarge { got: usize, max: usize },
    #[error("text of {got} characters exceeds the {max}-character limit")]
    TextTooLong { got: usize, max: usize },
    #[error("{0} is not implemented yet")]
    NotImplemented(&'static str),
}

impl DispatchError {
    /// The §8.9 error code this maps to, so the Control Plane does not have to
    /// re-derive it from a message string.
    #[must_use]
    pub fn code(&self) -> &'static str {
        match self {
            DispatchError::Platform(PlatformError::OutOfBounds { .. }) => "INVALID_COORDINATE",
            DispatchError::Platform(PlatformError::Unsupported(_)) => "UNSUPPORTED_ON_OS",
            DispatchError::Platform(_) => "EXEC_FAILED",
            DispatchError::Empty => "EXEC_FAILED",
            DispatchError::BatchTooLarge { .. } => "BATCH_TOO_LARGE",
            DispatchError::TextTooLong { .. } => "PAYLOAD_TOO_LARGE",
            DispatchError::NotImplemented(_) => "UNSUPPORTED_ON_OS",
        }
    }
}

pub type Result<T> = std::result::Result<T, DispatchError>;

fn button(b: i32) -> Button {
    match v1::MouseButton::try_from(b) {
        Ok(v1::MouseButton::Right) => Button::Right,
        Ok(v1::MouseButton::Middle) => Button::Middle,
        // Unspecified defaults to left: a click with no button named is far
        // more likely to mean "the usual one" than to be an error worth
        // failing an entire batch over.
        _ => Button::Left,
    }
}

fn point(p: Option<&v1::Point>) -> Option<(i32, i32)> {
    p.map(|p| (p.x, p.y))
}

fn rect(b: &v1::Bounds) -> Rect {
    Rect { x: b.x, y: b.y, width: b.width.max(0) as u32, height: b.height.max(0) as u32 }
}

/// Runs actions against one Desktop's drivers.
pub struct Dispatcher {
    frames: FrameSource,
    input: Box<dyn Input>,
    /// When the most recent input action finished. A screenshot must not return
    /// a frame captured before this (§6.3).
    last_input_at: std::sync::Mutex<Option<SystemTime>>,
    /// `control_plane - guest`, in milliseconds (§7.4).
    ///
    /// After a restore the guest clock is stale. Freshness comparisons are
    /// between two guest-side instants so a constant skew cancels out of them,
    /// but a *reported* timestamp is absolute — an agent comparing `taken_at`
    /// against its own clock would read a frame as minutes old. The offset is
    /// therefore applied at the reporting boundary and nowhere else.
    clock_offset_ms: std::sync::atomic::AtomicI64,
}

impl Dispatcher {
    pub fn new(frames: FrameSource, input: Box<dyn Input>) -> Self {
        Self {
            frames,
            input,
            last_input_at: std::sync::Mutex::new(None),
            clock_offset_ms: std::sync::atomic::AtomicI64::new(0),
        }
    }

    /// Records the guest's skew from the Control Plane (§7.4), applied to every
    /// timestamp reported from here on.
    pub fn set_clock_offset(&self, ms: i64) {
        self.clock_offset_ms.store(ms, std::sync::atomic::Ordering::SeqCst);
    }

    /// Converts a guest instant into the timestamp put on the wire.
    #[must_use]
    pub fn report_time(&self, t: SystemTime) -> prost_types::Timestamp {
        let ms = self.clock_offset_ms.load(std::sync::atomic::Ordering::SeqCst);
        // Checked, because `SystemTime - Duration` panics on underflow and the
        // offset is a value the Control Plane supplies. An absurd one should
        // leave the timestamp unadjusted, not take the daemon down.
        let adjusted = if ms >= 0 {
            t.checked_add(Duration::from_millis(ms as u64))
        } else {
            t.checked_sub(Duration::from_millis(ms.unsigned_abs()))
        };
        prost_time(adjusted.unwrap_or(t))
    }

    /// The current screen geometry, or `None` if the display is unreachable.
    #[must_use]
    pub fn screen_info(&self) -> Option<crate::platform::ScreenInfo> {
        self.frames.screen_info().ok()
    }

    /// Executes one action, reporting failure as a result rather than an error.
    ///
    /// The channel needs this shape: a failed action is an answer the agent can
    /// act on, not a reason to tear down a stream carrying seven other requests.
    pub fn execute_reported(&self, action: &v1::Action) -> v1::ActionResult {
        match self.execute(action) {
            Ok(r) => r,
            Err(e) => failed(&e),
        }
    }

    fn mark_input(&self) {
        *self.last_input_at.lock().unwrap() = Some(SystemTime::now());
    }

    /// Executes one action.
    pub fn execute(&self, action: &v1::Action) -> Result<v1::ActionResult> {
        let kind = action.kind.as_ref().ok_or(DispatchError::Empty)?;
        let started = std::time::Instant::now();

        let value = match kind {
            Kind::Screenshot(req) => {
                let region = req.region.as_ref().map(rect);
                // Anchor on the last input, not on "now": that is what stops a
                // screenshot taken right after a click from returning the
                // pre-click screen (§6.3).
                let frame = match *self.last_input_at.lock().unwrap() {
                    Some(t) => self.frames.capture_after(t, region)?,
                    None => self.frames.capture_now(region)?,
                };
                Some(v1::action_result::Value::Screenshot(v1::ScreenshotResponse {
                    payload: None, // the transport attaches the URL or inline bytes
                    width: frame.width as i32,
                    height: frame.height as i32,
                    display: None,
                    taken_at: Some(self.report_time(frame.captured_at)),
                }))
            }

            Kind::MouseMove(m) => {
                let (x, y) = point(m.to.as_ref()).ok_or(DispatchError::Empty)?;
                self.input.move_to(x, y)?;
                self.mark_input();
                None
            }
            Kind::MouseClick(c) => {
                let (x, y) = point(c.at.as_ref()).ok_or(DispatchError::Empty)?;
                self.input.click(x, y, button(c.button), c.count.max(1) as u8)?;
                self.mark_input();
                None
            }
            Kind::MouseDown(b) => {
                self.input.button_down(button(b.button))?;
                self.mark_input();
                None
            }
            Kind::MouseUp(b) => {
                self.input.button_up(button(b.button))?;
                self.mark_input();
                None
            }
            Kind::MouseDrag(d) => {
                let (fx, fy) = point(d.from.as_ref()).ok_or(DispatchError::Empty)?;
                let (tx, ty) = point(d.to.as_ref()).ok_or(DispatchError::Empty)?;
                self.input.move_to(fx, fy)?;
                self.input.button_down(Button::Left)?;
                self.input.move_to(tx, ty)?;
                self.input.button_up(Button::Left)?;
                self.mark_input();
                None
            }
            Kind::Scroll(s) => {
                if let Some((x, y)) = point(s.at.as_ref()) {
                    self.input.move_to(x, y)?;
                }
                self.input.scroll(s.dx, s.dy)?;
                self.mark_input();
                None
            }
            Kind::TypeText(t) => {
                let len = t.text.chars().count();
                if len > limits::TYPE_MAX_CHARS {
                    return Err(DispatchError::TextTooLong { got: len, max: limits::TYPE_MAX_CHARS });
                }
                let delay = Duration::from_millis(t.delay_ms.unwrap_or(0).max(0) as u64);
                self.input.type_text(&t.text, delay)?;
                self.mark_input();
                None
            }
            Kind::Key(k) => {
                for _ in 0..k.count.unwrap_or(1).max(1) {
                    self.input.key(&k.keys)?;
                }
                self.mark_input();
                None
            }
            Kind::KeyDown(k) => {
                self.input.key_down(&k.key)?;
                self.mark_input();
                None
            }
            Kind::KeyUp(k) => {
                self.input.key_up(&k.key)?;
                self.mark_input();
                None
            }

            // Deliberately unimplemented, and reported as such. A silent
            // success here would let an agent believe a password was typed.
            Kind::SecretType(_) => return Err(DispatchError::NotImplemented("secret.type")),
            Kind::AppLaunch(_) => return Err(DispatchError::NotImplemented("app.launch")),
            Kind::AppInstall(_) => return Err(DispatchError::NotImplemented("app.install")),
            Kind::ShellExec(_) => return Err(DispatchError::NotImplemented("shell.exec")),
            Kind::WaitFor(_) => return Err(DispatchError::NotImplemented("wait_for")),
        };

        Ok(v1::ActionResult {
            ok: true,
            elapsed_ms: started.elapsed().as_millis().min(i32::MAX as u128) as i32,
            error: None,
            value,
        })
    }

    /// Executes a batch, fail-fast (§7.2).
    ///
    /// GUI work cannot be rolled back, so a failure stops the batch and returns
    /// what completed plus the index that failed. The caller attaches a
    /// screenshot of that moment — the agent's only way to know how far the
    /// screen actually got.
    pub fn execute_batch(&self, actions: &[v1::Action]) -> (Vec<v1::ActionResult>, Option<usize>) {
        if actions.len() > limits::ACT_MAX_ACTIONS {
            let e = DispatchError::BatchTooLarge {
                got: actions.len(),
                max: limits::ACT_MAX_ACTIONS,
            };
            return (vec![failed(&e)], Some(0));
        }

        let mut out = Vec::with_capacity(actions.len());
        for (i, a) in actions.iter().enumerate() {
            match self.execute(a) {
                Ok(r) => out.push(r),
                Err(e) => {
                    out.push(failed(&e));
                    return (out, Some(i));
                }
            }
        }
        (out, None)
    }
}

fn failed(e: &DispatchError) -> v1::ActionResult {
    v1::ActionResult {
        ok: false,
        elapsed_ms: 0,
        error: Some(v1::Error {
            code: e.code().to_string(),
            message: e.to_string(),
            request_id: String::new(),
            details: Default::default(),
            retry_after_sec: None,
        }),
        value: None,
    }
}

/// Converts to the wire timestamp, rounding **up** to the millisecond.
///
/// §8.2 fixes the wire format at three fractional digits, but truncating
/// downward would break §6.3: a frame captured at t=1.0007s would be reported
/// as t=1.000s, and a client checking it against an action that completed at
/// t=1.0003s would conclude the screenshot predates the action it actually
/// followed. Rounding up guarantees the reported time is never earlier than
/// the real one, so a freshness check cannot produce a false negative. The
/// cost is up to 1ms of overstatement, which no caller can act on.
pub fn prost_time(t: SystemTime) -> prost_types::Timestamp {
    let d = t.duration_since(SystemTime::UNIX_EPOCH).unwrap_or_default();
    let (mut secs, sub_nanos) = (d.as_secs(), d.subsec_nanos());
    let mut nanos = sub_nanos.div_ceil(1_000_000) * 1_000_000;
    if nanos >= 1_000_000_000 {
        nanos -= 1_000_000_000;
        secs += 1;
    }
    prost_types::Timestamp { seconds: secs as i64, nanos: nanos as i32 }
}

/// The result reported for a request that carried no action.
#[must_use]
pub fn empty_action_result() -> v1::ActionResult {
    failed(&DispatchError::Empty)
}

/// The result reported when a driver panics mid-action.
///
/// A panic in unsafe FFI must not take the daemon down: the Desktop would go
/// `DEGRADED` and every other in-flight action would be lost, when only one
/// action actually failed.
#[must_use]
pub fn panic_result(detail: &str) -> v1::ActionResult {
    v1::ActionResult {
        ok: false,
        elapsed_ms: 0,
        error: Some(v1::Error {
            code: "EXEC_FAILED".to_string(),
            message: format!("the driver panicked: {detail}"),
            request_id: String::new(),
            details: Default::default(),
            retry_after_sec: None,
        }),
        value: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::fake::{FakeDisplay, FakeInput, InputEvent};
    use std::sync::Arc;

    fn dispatcher() -> (Dispatcher, Arc<FakeInput>) {
        let display = FakeDisplay::new(1920, 1080);
        let input = Arc::new(FakeInput::new().with_screen(1920, 1080));
        let d = Dispatcher::new(FrameSource::new(Box::new(display)), Box::new(input.clone()));
        (d, input)
    }

    fn act(kind: Kind) -> v1::Action {
        v1::Action { kind: Some(kind) }
    }

    #[test]
    fn a_click_reaches_the_driver_with_its_coordinates() {
        let (d, input) = dispatcher();
        d.execute(&act(Kind::MouseClick(v1::MouseClick {
            at: Some(v1::Point { x: 640, y: 120 }),
            button: v1::MouseButton::Left as i32,
            count: 1,
        })))
        .unwrap();

        assert_eq!(
            input.events(),
            vec![InputEvent::Click { x: 640, y: 120, button: Button::Left, count: 1 }]
        );
    }

    #[test]
    fn a_screenshot_after_a_click_never_predates_it() {
        // The §6.3 correctness hole: without anchoring, the agent receives the
        // frame from before its own click and reasons from a screen that never
        // existed.
        let (d, _) = dispatcher();
        d.execute(&act(Kind::Screenshot(v1::ScreenshotRequest::default()))).unwrap();

        d.execute(&act(Kind::MouseClick(v1::MouseClick {
            at: Some(v1::Point { x: 10, y: 10 }),
            button: v1::MouseButton::Left as i32,
            count: 1,
        })))
        .unwrap();
        let clicked_at = *d.last_input_at.lock().unwrap();

        let r = d.execute(&act(Kind::Screenshot(v1::ScreenshotRequest::default()))).unwrap();
        let Some(v1::action_result::Value::Screenshot(shot)) = r.value else {
            panic!("no screenshot returned");
        };
        let taken = shot.taken_at.unwrap();
        let taken = SystemTime::UNIX_EPOCH
            + Duration::new(taken.seconds as u64, taken.nanos as u32);

        assert!(taken >= clicked_at.unwrap(), "screenshot predates the click that preceded it");
    }

    #[test]
    fn the_wire_timestamp_rounds_up_so_freshness_cannot_read_as_stale() {
        // Truncating downward would report a frame captured at .0007 as .000,
        // making it look older than an action that completed at .0003 — the
        // §6.3 guarantee inverted by a serialization detail.
        let t = SystemTime::UNIX_EPOCH + Duration::new(5, 700_000);
        let w = prost_time(t);
        assert_eq!((w.seconds, w.nanos), (5, 1_000_000), "must round up, not truncate");

        // An exact millisecond stays put rather than gaining one.
        let exact = SystemTime::UNIX_EPOCH + Duration::new(5, 2_000_000);
        let w = prost_time(exact);
        assert_eq!((w.seconds, w.nanos), (5, 2_000_000));

        // Rounding up across a second boundary must carry.
        let edge = SystemTime::UNIX_EPOCH + Duration::new(5, 999_999_001);
        let w = prost_time(edge);
        assert_eq!((w.seconds, w.nanos), (6, 0), "must carry into the next second");
    }

    #[test]
    fn a_batch_stops_at_the_first_failure_and_reports_the_index() {
        // §7.2: GUI actions cannot be rolled back, so the rest must not run.
        let (d, input) = dispatcher();
        let (results, failed_at) = d.execute_batch(&[
            act(Kind::MouseClick(v1::MouseClick {
                at: Some(v1::Point { x: 1, y: 1 }),
                button: 1,
                count: 1,
            })),
            act(Kind::MouseClick(v1::MouseClick {
                at: Some(v1::Point { x: 9999, y: 1 }), // off screen
                button: 1,
                count: 1,
            })),
            act(Kind::TypeText(v1::TypeText { text: "never runs".into(), delay_ms: None })),
        ]);

        assert_eq!(failed_at, Some(1));
        assert_eq!(results.len(), 2, "the action after the failure must not run");
        assert!(results[0].ok);
        assert!(!results[1].ok);
        assert_eq!(results[1].error.as_ref().unwrap().code, "INVALID_COORDINATE");
        assert_eq!(input.events().len(), 1, "only the first click reached the driver");
    }

    #[test]
    fn an_oversized_batch_is_rejected_before_anything_runs() {
        let (d, input) = dispatcher();
        let one = act(Kind::MouseClick(v1::MouseClick {
            at: Some(v1::Point { x: 1, y: 1 }),
            button: 1,
            count: 1,
        }));
        let batch: Vec<v1::Action> = std::iter::repeat(one).take(limits::ACT_MAX_ACTIONS + 1).collect();

        let (results, failed_at) = d.execute_batch(&batch);
        assert_eq!(failed_at, Some(0));
        assert_eq!(results[0].error.as_ref().unwrap().code, "BATCH_TOO_LARGE");
        assert!(input.events().is_empty(), "no action may run when the batch is rejected");
    }

    #[test]
    fn text_beyond_the_cap_is_rejected() {
        let (d, input) = dispatcher();
        let long = "a".repeat(limits::TYPE_MAX_CHARS + 1);
        let e = d
            .execute(&act(Kind::TypeText(v1::TypeText { text: long, delay_ms: None })))
            .unwrap_err();
        assert_eq!(e.code(), "PAYLOAD_TOO_LARGE");
        assert!(input.events().is_empty());
    }

    #[test]
    fn unimplemented_actions_fail_loudly_rather_than_silently_succeeding() {
        // An agent told a password was typed when nothing happened would go on
        // to click "submit" on an empty form.
        let (d, _) = dispatcher();
        for kind in [
            Kind::SecretType(v1::SecretType { secret_ref: "sec_x".into() }),
            Kind::ShellExec(v1::ShellExec::default()),
        ] {
            let e = d.execute(&act(kind)).unwrap_err();
            assert_eq!(e.code(), "UNSUPPORTED_ON_OS");
        }
    }
}
