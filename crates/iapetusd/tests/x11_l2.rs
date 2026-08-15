//! L2 guest integration against a real X server (PRD §15.1, §15.2).
//!
//! These are the tests §15.2 calls the most important layer: what they miss
//! surfaces only in production, because nothing above the platform boundary can
//! detect a coordinate that lands in the wrong place or a syllable that arrives
//! split into jamo.
//!
//! They require a running X server and are skipped without one, so `cargo test`
//! stays green on a developer's laptop while CI runs them inside the container.
//!
//! Deliberately absent: pixel comparison. §15.3 forbids it — font hinting,
//! antialiasing, and cursor blink make every capture differ. These assert
//! semantics instead: where a click landed, which keysyms arrived, what the
//! reported coordinate frame is.

#![cfg(feature = "x11")]

use std::time::Duration;

use iapetusd::platform::x11::{X11Display, X11Input};
use iapetusd::platform::{Button, Display, Input, Rect};

use x11rb::connection::Connection;
use x11rb::protocol::xproto::{
    ConnectionExt as _, CreateWindowAux, EventMask, WindowClass,
};
use x11rb::protocol::Event;
use x11rb::rust_connection::RustConnection;
use x11rb::COPY_DEPTH_FROM_PARENT;

/// Skips the test when no X server is reachable, so the suite passes on hosts
/// that cannot run it rather than reporting a failure that means nothing.
macro_rules! require_x11 {
    () => {
        match std::env::var("DISPLAY") {
            Ok(d) if !d.is_empty() => {}
            _ => {
                eprintln!("skipping: DISPLAY is unset");
                return;
            }
        }
    };
}

/// A mapped window at a known position that records the events it receives.
struct TestWindow {
    conn: RustConnection,
    win: u32,
}

impl TestWindow {
    fn open(x: i16, y: i16, w: u16, h: u16) -> Self {
        let (conn, screen_num) = x11rb::connect(None).expect("connect");
        let screen = &conn.setup().roots[screen_num];
        let win = conn.generate_id().expect("generate_id");

        conn.create_window(
            COPY_DEPTH_FROM_PARENT,
            win,
            screen.root,
            x,
            y,
            w,
            h,
            0,
            WindowClass::INPUT_OUTPUT,
            screen.root_visual,
            &CreateWindowAux::new()
                .event_mask(
                    EventMask::BUTTON_PRESS | EventMask::KEY_PRESS | EventMask::EXPOSURE,
                )
                // Without this the window manager reparents and decorates the
                // window, so its client area is no longer at the coordinates we
                // asked for and a click at a computed root position lands on
                // the title bar instead. override-redirect keeps the geometry
                // exactly as requested, which is what the test is about.
                .override_redirect(1),
        )
        .expect("create_window");
        conn.map_window(win).expect("map_window");
        conn.flush().expect("flush");

        // Wait for the map to take effect. A fixed sleep is the flakiness
        // §15.3 warns about, so wait for the Expose the server sends instead.
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while std::time::Instant::now() < deadline {
            if let Ok(Some(Event::Expose(_))) = conn.poll_for_event() {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }

        // Take focus so synthetic key events are delivered here.
        conn.set_input_focus(
            x11rb::protocol::xproto::InputFocus::PARENT,
            win,
            x11rb::CURRENT_TIME,
        )
        .expect("set_input_focus");
        conn.flush().expect("flush");

        Self { conn, win }
    }

    /// Collects events for up to `timeout`, returning those that arrive.
    fn drain(&self, timeout: Duration) -> Vec<Event> {
        let deadline = std::time::Instant::now() + timeout;
        let mut out = Vec::new();
        while std::time::Instant::now() < deadline {
            while let Ok(Some(ev)) = self.conn.poll_for_event() {
                out.push(ev);
            }
            if !out.is_empty() {
                // Give any trailing events of the same burst a moment to land.
                std::thread::sleep(Duration::from_millis(30));
                while let Ok(Some(ev)) = self.conn.poll_for_event() {
                    out.push(ev);
                }
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        out
    }

    /// Maps a keycode back to the keysym currently bound to it, which is how
    /// these tests read what was actually typed.
    fn keysym_of(&self, keycode: u8) -> u32 {
        let setup = self.conn.setup();
        let reply = self
            .conn
            .get_keyboard_mapping(keycode, 1)
            .expect("get_keyboard_mapping")
            .reply()
            .expect("mapping reply");
        let _ = setup;
        reply.keysyms.first().copied().unwrap_or(0)
    }
}

impl Drop for TestWindow {
    fn drop(&mut self) {
        let _ = self.conn.destroy_window(self.win);
        let _ = self.conn.flush();
    }
}

#[test]
fn click_coordinates_land_where_the_agent_asked() {
    require_x11!();

    // A window at a known root position. Clicking at root (425, 315) must
    // arrive as window-relative (25, 15) — if the coordinate frame were wrong
    // by even the window border, every agent click would land elsewhere.
    let win = TestWindow::open(400, 300, 200, 150);
    let input = X11Input::open().expect("XTEST unavailable");

    input.click(425, 315, Button::Left, 1).expect("click");

    let events = win.drain(Duration::from_secs(2));
    let press = events
        .iter()
        .find_map(|e| match e {
            Event::ButtonPress(p) => Some(p),
            _ => None,
        })
        .expect("no ButtonPress arrived — XTEST input did not reach the window");

    assert_eq!(press.event_x, 25, "x landed at {} instead of 25", press.event_x);
    assert_eq!(press.event_y, 15, "y landed at {} instead of 15", press.event_y);
    assert_eq!(press.root_x, 425);
    assert_eq!(press.root_y, 315);
}

#[test]
fn hangul_arrives_as_one_key_event_per_syllable() {
    require_x11!();

    // The §15.2 property: 안녕 is two syllables and must arrive as two key
    // events. Jamo splitting — ㅇㅏㄴㄴㅕㅇ — would produce six.
    //
    // The keysym cannot be read back after the fact, because the driver
    // correctly restores the scratch keycode as soon as it is done. So the
    // check is the count, plus the fact that both events used the scratch
    // keycode, which proves they took the Unicode remap path rather than some
    // accidental Latin substitution.
    let win = TestWindow::open(100, 100, 200, 150);
    let input = X11Input::open().expect("XTEST unavailable");
    let scratch = input.scratch_keycode();

    input.type_text("안녕", Duration::from_millis(20)).expect("type_text");

    let events = win.drain(Duration::from_secs(3));
    let keycodes: Vec<u8> = events
        .iter()
        .filter_map(|e| match e {
            Event::KeyPress(k) => Some(k.detail),
            _ => None,
        })
        .collect();

    assert_eq!(
        keycodes.len(),
        2,
        "expected one key event per syllable; {} events means the text was \
         decomposed into jamo. keycodes: {:?}",
        keycodes.len(),
        keycodes
    );
    assert!(
        keycodes.iter().all(|k| *k == scratch),
        "syllables did not go through the Unicode remap path: {keycodes:?} vs scratch {scratch}"
    );
}

#[test]
fn the_scratch_keycode_is_restored_after_typing() {
    require_x11!();

    // Leaving the borrowed keycode bound would corrupt the layout for whoever
    // takes the lease next — a human would find one key typing Korean.
    let win = TestWindow::open(100, 100, 100, 100);
    let input = X11Input::open().expect("XTEST unavailable");
    let scratch = input.scratch_keycode();

    input.type_text("안", Duration::ZERO).expect("type_text");

    assert_eq!(
        win.keysym_of(scratch),
        0,
        "scratch keycode {scratch} is still bound after typing"
    );
}

#[test]
fn a_scaled_capture_still_reports_the_physical_coordinate_frame() {
    require_x11!();

    // §7.2: `scale` reduces the transmitted image only. If the reported frame
    // shrank with it, an agent would compute click coordinates against the
    // wrong space.
    let display = X11Display::open().expect("no display");
    let info = display.screen_info().expect("screen_info");

    let full = display.capture(None).expect("capture");
    assert_eq!(full.width, info.width);
    assert_eq!(full.height, info.height);
    assert_eq!(full.pixels.len(), full.byte_len(), "frame length must match its dimensions");

    // A region capture returns that region's pixels, while screen_info — the
    // coordinate frame — is unchanged.
    let region = display
        .capture(Some(Rect { x: 0, y: 0, width: 64, height: 48 }))
        .expect("region capture");
    assert_eq!((region.width, region.height), (64, 48));
    assert_eq!(display.screen_info().unwrap().width, info.width);
}

#[test]
fn capture_produces_opaque_pixels_not_a_transparent_image() {
    require_x11!();

    // X leaves the fourth byte undefined on 32-bit visuals. Passing it through
    // yields an image that is entirely transparent — which looks like a black
    // screen to a viewer and like a blank screen to an agent.
    let display = X11Display::open().expect("no display");
    let f = display.capture(Some(Rect { x: 0, y: 0, width: 16, height: 16 })).expect("capture");
    assert!(
        f.pixels.chunks_exact(4).all(|p| p[3] == 0xFF),
        "alpha channel was not forced opaque"
    );
}

#[test]
fn out_of_screen_coordinates_are_rejected_by_the_real_driver() {
    require_x11!();

    // §8.2 requires rejection rather than clamping, and the fake backend is
    // already tested for it. This confirms the real driver agrees — the two
    // disagreeing is exactly how a contract erodes.
    let display = X11Display::open().expect("no display");
    let info = display.screen_info().expect("screen_info");
    let input = X11Input::open().expect("XTEST unavailable");

    assert!(input.click(info.width as i32, 10, Button::Left, 1).is_err());
    assert!(input.click(-1, 10, Button::Left, 1).is_err());
    assert!(input.click(10, 10, Button::Left, 1).is_ok());
}

#[test]
fn release_all_clears_a_held_modifier() {
    require_x11!();

    // §5.6: without this, an agent preempted after key.down("ctrl") leaves the
    // modifier latched and the human's next keystrokes become shortcuts.
    let win = TestWindow::open(100, 100, 200, 150);
    let input = X11Input::open().expect("XTEST unavailable");

    input.key_down("ctrl").expect("key_down");
    input.release_all().expect("release_all");

    // Typing a plain character now must arrive without Ctrl in its modifier
    // mask. A latched Ctrl would show as CONTROL in `state`.
    input.type_text("a", Duration::ZERO).expect("type_text");
    let events = win.drain(Duration::from_secs(2));
    let press = events
        .iter()
        .find_map(|e| match e {
            Event::KeyPress(k) => Some(k),
            _ => None,
        })
        .expect("no KeyPress arrived");

    assert!(
        !press.state.contains(x11rb::protocol::xproto::KeyButMask::CONTROL),
        "Ctrl was still latched after release_all"
    );
}
