//! The §16 Phase 1 completion scenario, driven through the Computer API.
//!
//! Every other test exercises one layer. This one goes through the same path an
//! agent takes — `app.launch` with `wait_for_window`, then `type`, then `key`,
//! then `screenshot` — against a real browser on a real X server, and asserts
//! the browser actually received them.
//!
//! **What it deliberately does not do is search the internet.** A real query
//! would make the test slow, flaky, and mostly a measurement of Google's
//! uptime. What S1 exercises on our side is launch → window → focus → text →
//! key, so the fixture page reports the typed string in its window title, which
//! the guest reads through `_NET_WM_NAME`. §15.3 forbids pixel comparison, and
//! a title is exactly the kind of semantic signal it asks for instead.

#![cfg(feature = "x11")]

use std::sync::Arc;
use std::time::{Duration, Instant};

use iapetus_proto::v1::{self, action::Kind};

use iapetusd::catalog::Catalog;
use iapetusd::dispatch::Dispatcher;
use iapetusd::frame::FrameSource;
use iapetusd::platform::unix::UnixProcess;
use iapetusd::platform::x11::{X11Display, X11Input};
use iapetusd::platform::Windows;

const FIXTURE: &str = "/opt/iapetus/s1.html";

macro_rules! require {
    ($cond:expr, $why:literal) => {
        if !($cond) {
            eprintln!("skipping S1: {}", $why);
            return;
        }
    };
}

/// The same wiring `main.rs` builds, so this tests the shipped configuration
/// rather than a arrangement that only exists in a test.
fn build() -> (Dispatcher, Arc<X11Display>) {
    let display = Arc::new(X11Display::open().expect("no display"));
    let input = X11Input::open().expect("XTEST unavailable");
    let catalog = Catalog::load("/etc/iapetus/apps.json").expect("catalog did not parse");

    let d = Dispatcher::new(FrameSource::new(Box::new(display.clone())), Box::new(input))
        .with_process(Box::new(UnixProcess::new()))
        .with_windows(Box::new(display.clone()))
        .with_catalog(catalog);
    (d, display)
}

fn act(d: &Dispatcher, kind: Kind) -> v1::ActionResult {
    let a = v1::Action { kind: Some(kind) };
    d.execute(&a).unwrap_or_else(|e| panic!("action failed: {e}"))
}

/// Polls window titles until one matches, which is how this test observes the
/// browser without reading pixels.
fn wait_for_title(
    display: &X11Display,
    matches: impl Fn(&str) -> bool,
    timeout: Duration,
) -> Option<String> {
    let deadline = Instant::now() + timeout;
    loop {
        if let Ok(list) = display.list() {
            if let Some(w) = list.into_iter().find(|w| matches(&w.title)) {
                return Some(w.title);
            }
        }
        if Instant::now() >= deadline {
            return None;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

#[test]
fn s1_launch_a_browser_type_a_query_and_see_the_result() {
    match std::env::var("DISPLAY") {
        Ok(d) if !d.is_empty() => {}
        _ => {
            eprintln!("skipping S1: DISPLAY is unset");
            return;
        }
    }
    require!(std::path::Path::new(FIXTURE).exists(), "the fixture page is not installed");
    require!(
        std::path::Path::new("/usr/bin/chromium").exists(),
        "chromium is not installed in this image"
    );

    let (d, display) = build();

    // ── 1. Launch, and wait for the window rather than sleeping ──
    let r = act(
        &d,
        Kind::AppLaunch(v1::AppLaunch {
            target: Some(v1::app_launch::Target::Key("chrome".into())),
            args: vec![format!("file://{FIXTURE}")],
            cwd: None,
            elevated: None,
            wait_for_window: Some(true),
        }),
    );
    assert!(r.ok, "launch failed: {:?}", r.error);
    let Some(v1::action_result::Value::AppLaunch(launch)) = r.value else {
        panic!("no launch result");
    };
    let window = launch
        .window
        .expect("no window appeared — wait_for_window matched nothing for the browser");
    assert!(launch.pid > 0);

    // The window arriving is not the page being ready. Chromium maps its frame
    // before the renderer has run any script, so the title is the signal that
    // the document — and therefore the focused input — actually exists.
    let ready = wait_for_title(&display, |t| t.contains("S1 READY"), Duration::from_secs(30));
    assert!(ready.is_some(), "the fixture page never loaded; last window was {:?}", window.title);

    // ── 2. Type, through the same action an agent would send ──
    // No click first: the page autofocuses its input. Clicking would test the
    // coordinate frame, which `click_coordinates_land_where_the_agent_asked`
    // already covers; this is about text reaching a focused element.
    act(
        &d,
        Kind::TypeText(v1::TypeText { text: "iapetus".into(), delay_ms: Some(20) }),
    );

    // ── 3. Submit ──
    act(&d, Kind::Key(v1::KeyPress { keys: "Enter".into(), count: Some(1) }));

    // ── 4. The browser received all of it ──
    let title = wait_for_title(&display, |t| t.starts_with("S1 RESULT"), Duration::from_secs(15));
    let title = title.unwrap_or_else(|| {
        panic!(
            "the page never reported a result — the text or the Enter key did not reach it. \
             Titles now: {:?}",
            display.list().map(|l| l.into_iter().map(|w| w.title).collect::<Vec<_>>())
        )
    });
    // Browsers append their own name to the window title, so this matches the
    // prefix. The query itself is still compared exactly — a partial or
    // reordered string is the failure worth catching, and `contains` would let
    // "iapetu" through.
    assert!(
        title.starts_with("S1 RESULT iapetus"),
        "the browser received something other than what was typed: {title:?}"
    );

    // ── 5. And the agent can see the screen it just changed ──
    let shot = act(
        &d,
        Kind::Screenshot(v1::ScreenshotRequest {
            format: v1::ImageFormat::Png as i32,
            quality: 0,
            region: None,
            scale: None,
        }),
    );
    let Some(v1::action_result::Value::Screenshot(s)) = shot.value else {
        panic!("no screenshot");
    };
    let Some(v1::screenshot_response::Payload::Inline(bytes)) = s.payload else {
        panic!("the screenshot carried no image");
    };
    assert!(!bytes.is_empty());

    // Not a pixel comparison (§15.3): only that the capture is a real image of
    // the real screen, which is the one thing a blank or stale frame fails.
    let img = image::load_from_memory(&bytes).expect("payload is not a decodable image");
    assert_eq!(
        (img.width(), img.height()),
        (s.width as u32, s.height as u32),
        "reported size disagrees with the bytes"
    );
    let display_frame = s.display.expect("no coordinate frame");
    assert_eq!(img.width(), display_frame.width as u32, "unscaled capture must match the screen");

    let _ = std::process::Command::new("kill")
        .arg("-9")
        .arg(launch.pid.to_string())
        .status();
}
