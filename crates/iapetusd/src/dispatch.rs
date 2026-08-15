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

use crate::catalog::Catalog;
use crate::frame::FrameSource;
use crate::platform::{Button, Input, LaunchSpec, PlatformError, Process, Rect, Windows};

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
    #[error("no driver for {0} on this build")]
    NoDriver(&'static str),
    #[error("no catalog entry for `{0}`; launch it by command instead (§5.5)")]
    UnknownAppKey(String),
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
            DispatchError::NotImplemented(_) | DispatchError::NoDriver(_) => "UNSUPPORTED_ON_OS",
            // Not APP_NOT_ALLOWED: §5.5 makes the catalog a shortcut, not a
            // restriction, and §8.9 reserves that code for `restricted` mode.
            // Reporting an authority failure for a missing shortcut would send
            // the caller to the policy engine over a typo.
            DispatchError::UnknownAppKey(_) => "EXEC_FAILED",
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
    process: Option<Box<dyn Process>>,
    windows: Option<Box<dyn Windows>>,
    catalog: Catalog,
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
            process: None,
            windows: None,
            catalog: Catalog::empty(),
            last_input_at: std::sync::Mutex::new(None),
            clock_offset_ms: std::sync::atomic::AtomicI64::new(0),
        }
    }

    /// Adds the process driver. Without one, `app.launch` fails loudly rather
    /// than reporting a pid for a program that was never started.
    #[must_use]
    pub fn with_process(mut self, p: Box<dyn Process>) -> Self {
        self.process = Some(p);
        self
    }

    #[must_use]
    pub fn with_windows(mut self, w: Box<dyn Windows>) -> Self {
        self.windows = Some(w);
        self
    }

    #[must_use]
    pub fn with_catalog(mut self, c: Catalog) -> Self {
        self.catalog = c;
        self
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

    /// A frame for the stream encoder.
    ///
    /// Deliberately not freshness-gated. §6.3's freshness contract anchors a
    /// *screenshot* to the action before it, because an agent reasoning from a
    /// pre-click screen is a correctness fault. A stream has no such anchor —
    /// it is continuous — and forcing a capture per tick would defeat §6.3's
    /// single shared Frame Source, which exists so the still and video paths do
    /// not each pull their own frames.
    pub fn capture_for_stream(&self) -> crate::platform::Result<crate::platform::Frame> {
        self.frames.capture_now(None)
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
                let format = v1::ImageFormat::try_from(req.format).unwrap_or_default();
                let encoded = crate::encode::encode(&frame, format, req.quality, req.scale)?;

                // The guest always sends bytes. Whether the API answers inline
                // or with a presigned URL is §8.2's 256KB decision, and only
                // the Control Plane can make it — it is the only side that can
                // mint a URL.
                Some(v1::action_result::Value::Screenshot(v1::ScreenshotResponse {
                    payload: Some(v1::screenshot_response::Payload::Inline(encoded.bytes)),
                    // The size of the transmitted image...
                    width: encoded.width as i32,
                    height: encoded.height as i32,
                    // ...while `display` stays the physical frame the agent
                    // computes clicks against (§7.2). A `scale` that shrank
                    // both would put every click at a fraction of its target.
                    display: self.screen_info().map(|s| v1::Display {
                        width: s.width as i32,
                        height: s.height as i32,
                        dpi: s.dpi as i32,
                    }),
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
            Kind::AppLaunch(req) => {
                let spec = self.resolve_launch(req)?;
                let process = self
                    .process
                    .as_ref()
                    .ok_or(DispatchError::NoDriver("app.launch"))?;
                let pid = process.launch(&spec)?;

                // Launching changes the screen, so §6.3 must treat it the same
                // as input: a screenshot taken next has to postdate it, or the
                // agent sees the desktop as it was before the window opened.
                self.mark_input();

                let window = if req.wait_for_window.unwrap_or(false) {
                    let w = self
                        .windows
                        .as_ref()
                        .ok_or(DispatchError::NoDriver("wait_for_window"))?;
                    // A timeout is reported as an absent window rather than an
                    // error. The program is running and only the guest knows
                    // its pid, so failing here would strand a process the agent
                    // can no longer name, close, or wait on.
                    w.wait_for_window(pid, Duration::from_millis(u64::from(limits::TIMEOUT_WAIT_FOR_MS.0)))?
                        .map(|w| v1::Window {
                            id: format!("win_{}", w.id),
                            title: w.title,
                            bounds: Some(v1::Bounds {
                                x: w.bounds.x,
                                y: w.bounds.y,
                                width: w.bounds.width as i32,
                                height: w.bounds.height as i32,
                            }),
                            focused: false,
                        })
                } else {
                    None
                };

                Some(v1::action_result::Value::AppLaunch(v1::AppLaunchResult {
                    pid: pid as i32,
                    window,
                }))
            }
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

impl Dispatcher {
    /// Turns an `app.launch` target into something to execute.
    ///
    /// §5.5: a catalog key is a shortcut and an arbitrary command is equally
    /// legitimate, so both paths land here rather than one being the exception.
    fn resolve_launch(&self, req: &v1::AppLaunch) -> Result<LaunchSpec> {
        let target = req.target.as_ref().ok_or(DispatchError::Empty)?;
        let (command, mut args, cwd) = match target {
            v1::app_launch::Target::Key(key) => {
                let app = self
                    .catalog
                    .get(key)
                    .ok_or_else(|| DispatchError::UnknownAppKey(key.clone()))?;
                (app.launch.command.clone(), app.launch.args.clone(), app.launch.cwd.clone())
            }
            v1::app_launch::Target::Command(cmd) => (cmd.clone(), Vec::new(), None),
        };
        // Request arguments append to the catalog's rather than replacing them:
        // a catalog entry's flags are what make the shortcut work, and dropping
        // them because the caller added one of its own would be surprising.
        args.extend(req.args.iter().cloned());

        Ok(LaunchSpec {
            command,
            args,
            cwd: req.cwd.clone().or(cwd),
            elevated: req.elevated.unwrap_or(false),
        })
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
    fn a_screenshot_carries_pixels_and_the_physical_coordinate_frame() {
        // §7.2: `scale` reduces the transmitted image only. If `display` shrank
        // with it, an agent would compute every click against the wrong space.
        // And a response with no payload is the failure this test exists for —
        // the agent's whole view of the world arrives through this field.
        let (d, _) = dispatcher();
        let a = v1::Action {
            kind: Some(Kind::Screenshot(v1::ScreenshotRequest {
                format: v1::ImageFormat::Png as i32,
                quality: 0,
                region: None,
                scale: Some(0.25),
            })),
        };

        let r = d.execute(&a).expect("screenshot failed");
        let Some(v1::action_result::Value::Screenshot(shot)) = r.value else {
            panic!("no screenshot in the result");
        };

        let Some(v1::screenshot_response::Payload::Inline(bytes)) = shot.payload else {
            panic!("the screenshot carried no image");
        };
        let img = image::load_from_memory(&bytes).expect("the payload is not a decodable image");
        assert_eq!((img.width(), img.height()), (480, 270), "0.25 of 1920x1080");
        assert_eq!((shot.width, shot.height), (480, 270), "reported size disagrees with the bytes");

        let display = shot.display.expect("no coordinate frame was reported");
        assert_eq!(
            (display.width, display.height),
            (1920, 1080),
            "the coordinate frame was scaled along with the image"
        );
    }

    fn launch_action(target: Kind) -> v1::Action {
        v1::Action { kind: Some(target) }
    }

    fn app_dispatcher() -> (Dispatcher, std::sync::Arc<crate::platform::fake::FakeProcess>) {
        use crate::platform::fake::{FakeProcess, FakeWindows};
        let proc = std::sync::Arc::new(FakeProcess::new().with_missing("/no/such/binary"));
        let cat = crate::catalog::Catalog::parse(
            r#"{"apps":[{"key":"chrome","launch":{"command":"/usr/bin/chromium",
                 "args":["--no-first-run"],"cwd":"/tmp"}}]}"#,
            "test",
        )
        .unwrap();
        let d = Dispatcher::new(
            FrameSource::new(Box::new(FakeDisplay::new(1920, 1080))),
            Box::new(FakeInput::new().with_screen(1920, 1080)),
        )
        .with_process(Box::new(proc.clone()))
        .with_windows(Box::new(FakeWindows::new().with_window(7, "Chromium")))
        .with_catalog(cat);
        (d, proc)
    }

    #[test]
    fn a_catalog_key_launches_its_command_with_the_catalog_arguments() {
        let (d, proc) = app_dispatcher();
        let a = launch_action(Kind::AppLaunch(v1::AppLaunch {
            target: Some(v1::app_launch::Target::Key("chrome".into())),
            args: vec!["--incognito".into()],
            cwd: None,
            elevated: None,
            wait_for_window: None,
        }));

        let r = d.execute(&a).expect("launch failed");
        assert!(r.ok);

        let launched = proc.launched();
        assert_eq!(launched.len(), 1);
        assert_eq!(launched[0].command, "/usr/bin/chromium");
        // Request args append rather than replace: dropping the catalog's own
        // flags because the caller added one would break the shortcut.
        assert_eq!(launched[0].args, vec!["--no-first-run", "--incognito"]);
        assert_eq!(launched[0].cwd.as_deref(), Some("/tmp"));
    }

    #[test]
    fn an_arbitrary_command_launches_without_a_catalog_entry() {
        // §5.5: the catalog is a shortcut, not a restriction. OWNER mode (§7.3)
        // means anything on the disk is fair game.
        let (d, proc) = app_dispatcher();
        let a = launch_action(Kind::AppLaunch(v1::AppLaunch {
            target: Some(v1::app_launch::Target::Command("/opt/vendor/erp".into())),
            args: vec!["--kiosk".into()],
            cwd: None,
            elevated: Some(true),
            wait_for_window: None,
        }));

        assert!(d.execute(&a).unwrap().ok);
        let l = &proc.launched()[0];
        assert_eq!(l.command, "/opt/vendor/erp");
        assert_eq!(l.args, vec!["--kiosk"]);
        assert!(l.elevated);
    }

    #[test]
    fn an_unknown_catalog_key_is_not_reported_as_an_authority_failure() {
        // §8.9 reserves APP_NOT_ALLOWED for `restricted` mode. Returning it for
        // a missing shortcut would send the caller to the policy engine over
        // what is really a typo.
        let (d, _) = app_dispatcher();
        let a = launch_action(Kind::AppLaunch(v1::AppLaunch {
            target: Some(v1::app_launch::Target::Key("not-in-the-catalog".into())),
            args: vec![],
            cwd: None,
            elevated: None,
            wait_for_window: None,
        }));

        let e = d.execute(&a).unwrap_err();
        assert!(matches!(e, DispatchError::UnknownAppKey(_)));
        assert_eq!(e.code(), "EXEC_FAILED");
    }

    #[test]
    fn wait_for_window_returns_the_window_it_waited_for() {
        // Waiting and then not saying which window appeared leaves the agent
        // exactly where it started.
        let (d, _) = app_dispatcher();
        let a = launch_action(Kind::AppLaunch(v1::AppLaunch {
            target: Some(v1::app_launch::Target::Key("chrome".into())),
            args: vec![],
            cwd: None,
            elevated: None,
            wait_for_window: Some(true),
        }));

        let r = d.execute(&a).unwrap();
        let Some(v1::action_result::Value::AppLaunch(res)) = r.value else {
            panic!("no launch result");
        };
        assert!(res.pid > 0, "no pid was reported");
        let w = res.window.expect("wait_for_window returned no window");
        assert_eq!(w.id, "win_7", "§8.2 requires the win_ prefix");
        assert_eq!(w.title, "Chromium");
    }

    #[test]
    fn a_launch_that_failed_reports_no_pid() {
        // A pid for a program that never started has the agent wait for a
        // window that is never coming.
        let (d, _) = app_dispatcher();
        let a = launch_action(Kind::AppLaunch(v1::AppLaunch {
            target: Some(v1::app_launch::Target::Command("/no/such/binary".into())),
            args: vec![],
            cwd: None,
            elevated: None,
            wait_for_window: None,
        }));
        assert!(d.execute(&a).is_err());
    }

    #[test]
    fn app_launch_without_a_process_driver_fails_rather_than_pretending() {
        let d = Dispatcher::new(
            FrameSource::new(Box::new(FakeDisplay::new(64, 48))),
            Box::new(FakeInput::new()),
        );
        let a = launch_action(Kind::AppLaunch(v1::AppLaunch {
            target: Some(v1::app_launch::Target::Command("/bin/true".into())),
            args: vec![],
            cwd: None,
            elevated: None,
            wait_for_window: None,
        }));
        let e = d.execute(&a).unwrap_err();
        assert!(matches!(e, DispatchError::NoDriver("app.launch")));
    }

    #[test]
    fn a_screenshot_after_a_launch_postdates_it() {
        // §6.3 applies to anything that changes the screen, not just input. An
        // agent that launched a window and got the previous frame concludes the
        // launch failed.
        let (d, _) = app_dispatcher();
        d.execute(&launch_action(Kind::AppLaunch(v1::AppLaunch {
            target: Some(v1::app_launch::Target::Key("chrome".into())),
            args: vec![],
            cwd: None,
            elevated: None,
            wait_for_window: None,
        })))
        .unwrap();

        let launched_at = *d.last_input_at.lock().unwrap();
        assert!(launched_at.is_some(), "app.launch did not mark the screen as changed");
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
