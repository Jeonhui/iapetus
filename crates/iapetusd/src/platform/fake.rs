//! An in-memory platform used by tests and by `--selftest`.
//!
//! It exists so the logic above the platform boundary — the Frame Source,
//! freshness, input-state tracking — is testable without X11. The real X11 path
//! is verified separately inside a container (§15.1 L2).

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Mutex;
use std::time::{Duration, SystemTime};

use super::{Button, Display, Frame, Input, PlatformError, Rect, Result, ScreenInfo};

/// A record of what was asked of the input driver, so tests can assert on it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InputEvent {
    Move { x: i32, y: i32 },
    Click { x: i32, y: i32, button: Button, count: u8 },
    ButtonDown(Button),
    ButtonUp(Button),
    Scroll { dx: i32, dy: i32 },
    Type(String),
    Key(String),
    KeyDown(String),
    KeyUp(String),
}

pub struct FakeDisplay {
    width: u32,
    height: u32,
    captures: AtomicU32,
    clock: Mutex<Option<SystemTime>>,
}

impl FakeDisplay {
    #[must_use]
    pub fn new(width: u32, height: u32) -> Self {
        Self { width, height, captures: AtomicU32::new(0), clock: Mutex::new(None) }
    }

    /// Pins the timestamp future captures will carry, so freshness can be
    /// tested deterministically rather than by sleeping.
    pub fn set_clock(&self, now: SystemTime) {
        *self.clock.lock().unwrap() = Some(now);
    }

    #[must_use]
    pub fn capture_count(&self) -> u32 {
        self.captures.load(Ordering::SeqCst)
    }

    fn now(&self) -> SystemTime {
        self.clock.lock().unwrap().unwrap_or_else(SystemTime::now)
    }
}

impl Display for FakeDisplay {
    fn capture(&self, region: Option<Rect>) -> Result<Frame> {
        self.captures.fetch_add(1, Ordering::SeqCst);
        let (w, h) = match region {
            Some(r) => (r.width, r.height),
            None => (self.width, self.height),
        };
        Ok(Frame {
            width: w,
            height: h,
            pixels: vec![0u8; (w as usize) * (h as usize) * 4],
            captured_at: self.now(),
        })
    }

    fn screen_info(&self) -> Result<ScreenInfo> {
        Ok(ScreenInfo { width: self.width, height: self.height, dpi: 96, monitor_count: 1 })
    }

    fn wait_for_change(&self, _timeout: Duration) -> Result<bool> {
        Ok(false)
    }
}

impl Display for std::sync::Arc<FakeDisplay> {
    fn capture(&self, region: Option<Rect>) -> Result<Frame> {
        (**self).capture(region)
    }
    fn screen_info(&self) -> Result<ScreenInfo> {
        (**self).screen_info()
    }
    fn wait_for_change(&self, timeout: Duration) -> Result<bool> {
        (**self).wait_for_change(timeout)
    }
}

/// Records input and tracks what is currently held down.
///
/// The held-key set is the point: §5.6 requires every key and button to be
/// released before a lease changes hands, and this is where that is verified.
#[derive(Default)]
pub struct FakeInput {
    events: Mutex<Vec<InputEvent>>,
    held_keys: Mutex<Vec<String>>,
    held_buttons: Mutex<Vec<Button>>,
    screen: Option<(u32, u32)>,
    /// Makes every call take measurable time, so rules that only appear under
    /// slow execution — the §19.5 deadline, queue backpressure — can be tested
    /// without guessing at real driver latency.
    latency: Option<Duration>,
}

impl FakeInput {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Enables bounds checking, so out-of-screen coordinates are rejected
    /// rather than clamped (§8.2).
    #[must_use]
    pub fn with_screen(mut self, width: u32, height: u32) -> Self {
        self.screen = Some((width, height));
        self
    }

    #[must_use]
    pub fn events(&self) -> Vec<InputEvent> {
        self.events.lock().unwrap().clone()
    }

    #[must_use]
    pub fn held_keys(&self) -> Vec<String> {
        self.held_keys.lock().unwrap().clone()
    }

    #[must_use]
    pub fn held_buttons(&self) -> Vec<Button> {
        self.held_buttons.lock().unwrap().clone()
    }

    fn check_bounds(&self, x: i32, y: i32) -> Result<()> {
        if let Some((w, h)) = self.screen {
            if x < 0 || y < 0 || x >= w as i32 || y >= h as i32 {
                return Err(PlatformError::OutOfBounds { x, y, width: w, height: h });
            }
        }
        Ok(())
    }

    /// Makes every call block for `d`.
    #[must_use]
    pub fn with_latency(mut self, d: Duration) -> Self {
        self.latency = Some(d);
        self
    }

    fn push(&self, e: InputEvent) {
        if let Some(d) = self.latency {
            std::thread::sleep(d);
        }
        self.events.lock().unwrap().push(e);
    }
}

impl Input for FakeInput {
    fn move_to(&self, x: i32, y: i32) -> Result<()> {
        self.check_bounds(x, y)?;
        self.push(InputEvent::Move { x, y });
        Ok(())
    }

    fn click(&self, x: i32, y: i32, button: Button, count: u8) -> Result<()> {
        self.check_bounds(x, y)?;
        self.push(InputEvent::Click { x, y, button, count });
        Ok(())
    }

    fn button_down(&self, button: Button) -> Result<()> {
        self.held_buttons.lock().unwrap().push(button);
        self.push(InputEvent::ButtonDown(button));
        Ok(())
    }

    fn button_up(&self, button: Button) -> Result<()> {
        self.held_buttons.lock().unwrap().retain(|b| *b != button);
        self.push(InputEvent::ButtonUp(button));
        Ok(())
    }

    fn scroll(&self, dx: i32, dy: i32) -> Result<()> {
        self.push(InputEvent::Scroll { dx, dy });
        Ok(())
    }

    fn type_text(&self, text: &str, _delay: Duration) -> Result<()> {
        self.push(InputEvent::Type(text.to_string()));
        Ok(())
    }

    fn key(&self, combo: &str) -> Result<()> {
        self.push(InputEvent::Key(combo.to_string()));
        Ok(())
    }

    fn key_down(&self, key: &str) -> Result<()> {
        self.held_keys.lock().unwrap().push(key.to_string());
        self.push(InputEvent::KeyDown(key.to_string()));
        Ok(())
    }

    fn key_up(&self, key: &str) -> Result<()> {
        self.held_keys.lock().unwrap().retain(|k| k != key);
        self.push(InputEvent::KeyUp(key.to_string()));
        Ok(())
    }

    fn release_all(&self) -> Result<()> {
        let keys: Vec<String> = self.held_keys.lock().unwrap().drain(..).collect();
        for k in keys {
            self.push(InputEvent::KeyUp(k));
        }
        let buttons: Vec<Button> = self.held_buttons.lock().unwrap().drain(..).collect();
        for b in buttons {
            self.push(InputEvent::ButtonUp(b));
        }
        Ok(())
    }
}

/// Lets a test keep a handle for assertions while the Dispatcher owns the
/// driver. The Display side has the same forwarding impl for the same reason.
impl Input for std::sync::Arc<FakeInput> {
    fn move_to(&self, x: i32, y: i32) -> Result<()> {
        (**self).move_to(x, y)
    }
    fn click(&self, x: i32, y: i32, button: Button, count: u8) -> Result<()> {
        (**self).click(x, y, button, count)
    }
    fn button_down(&self, button: Button) -> Result<()> {
        (**self).button_down(button)
    }
    fn button_up(&self, button: Button) -> Result<()> {
        (**self).button_up(button)
    }
    fn scroll(&self, dx: i32, dy: i32) -> Result<()> {
        (**self).scroll(dx, dy)
    }
    fn type_text(&self, text: &str, delay: Duration) -> Result<()> {
        (**self).type_text(text, delay)
    }
    fn key(&self, combo: &str) -> Result<()> {
        (**self).key(combo)
    }
    fn key_down(&self, key: &str) -> Result<()> {
        (**self).key_down(key)
    }
    fn key_up(&self, key: &str) -> Result<()> {
        (**self).key_up(key)
    }
    fn release_all(&self) -> Result<()> {
        (**self).release_all()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn out_of_screen_coordinates_are_rejected_not_clamped() {
        // §8.2: silently correcting them would hide the caller's own bug.
        let input = FakeInput::new().with_screen(1920, 1080);
        assert!(input.click(100, 100, Button::Left, 1).is_ok());

        let e = input.click(1920, 500, Button::Left, 1).unwrap_err();
        assert!(matches!(e, PlatformError::OutOfBounds { x: 1920, .. }));
        assert!(input.click(-1, 0, Button::Left, 1).is_err());

        assert_eq!(input.events().len(), 1, "rejected input must not be recorded");
    }

    #[test]
    fn release_all_clears_every_held_key_and_button() {
        // §5.6: an agent preempted mid-chord would otherwise leave Ctrl held,
        // turning the human's subsequent typing into shortcuts.
        let input = FakeInput::new();
        input.key_down("ctrl").unwrap();
        input.key_down("shift").unwrap();
        input.button_down(Button::Left).unwrap();

        input.release_all().unwrap();

        assert!(input.held_keys().is_empty(), "keys still held after handover");
        assert!(input.held_buttons().is_empty(), "buttons still held after handover");

        let tail = &input.events()[3..];
        assert!(tail.contains(&InputEvent::KeyUp("ctrl".into())));
        assert!(tail.contains(&InputEvent::KeyUp("shift".into())));
        assert!(tail.contains(&InputEvent::ButtonUp(Button::Left)));
    }

    #[test]
    fn release_all_is_safe_when_nothing_is_held() {
        let input = FakeInput::new();
        input.release_all().unwrap();
        assert!(input.events().is_empty());
    }
}
