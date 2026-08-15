//! The Frame Source (PRD §6.3).
//!
//! One capture source feeds two consumers: the still encoder that answers an
//! agent's `screenshot`, and the video encoder that feeds a watching human.
//! Capturing twice would double an already expensive operation — a 1080p RGBA
//! frame is an 8MB copy.
//!
//! The subtle part is not the sharing but the **freshness contract**. Capture is
//! driven by change events and lags a few milliseconds, so an agent calling
//! `screenshot` immediately after `click` can be handed the pre-click screen.
//! That is a correctness fault, not a latency one: the agent then reasons about
//! a world state that never existed. `capture_after` is the guard.

use std::sync::Mutex;
use std::time::{Duration, SystemTime};

use crate::platform::{Display, Frame, Rect, Result};

/// Shared capture source with a one-frame cache.
///
/// The cache is deliberately one frame deep. A deeper ring would only be useful
/// to serve *older* frames, which nothing wants — the still path needs the
/// newest frame and the video path consumes each frame once as it arrives.
pub struct FrameSource {
    display: Box<dyn Display>,
    latest: Mutex<Option<Frame>>,
}

impl FrameSource {
    pub fn new(display: Box<dyn Display>) -> Self {
        Self { display, latest: Mutex::new(None) }
    }

    /// Captures unconditionally and refreshes the cache.
    pub fn capture_now(&self, region: Option<Rect>) -> Result<Frame> {
        let frame = self.display.capture(region)?;
        // Only whole-screen captures populate the cache: a region capture would
        // otherwise satisfy a later full-screen request with partial pixels.
        if region.is_none() {
            *self.latest.lock().unwrap() = Some(frame.clone());
        }
        Ok(frame)
    }

    /// Returns a frame guaranteed to have been captured at or after `not_before`.
    ///
    /// This is the §6.3 freshness contract. Callers pass the completion time of
    /// the action they just performed; a cached frame older than that is
    /// discarded and a fresh capture forced.
    ///
    /// It cannot help where nothing anchors the wait — a page finishing its
    /// load, a dialog raised by `shell.exec`. There the caller must use
    /// `wait_for(screen_stable)` instead, because only the caller knows what it
    /// is waiting for (§6.3).
    pub fn capture_after(&self, not_before: SystemTime, region: Option<Rect>) -> Result<Frame> {
        if region.is_none() {
            if let Some(cached) = self.latest.lock().unwrap().as_ref() {
                if cached.captured_at >= not_before {
                    return Ok(cached.clone());
                }
            }
        }
        self.capture_now(region)
    }

    /// The cached frame, however old, or `None` if nothing has been captured.
    ///
    /// For the video path, which wants whatever is newest and has no freshness
    /// requirement of its own.
    pub fn latest(&self) -> Option<Frame> {
        self.latest.lock().unwrap().clone()
    }

    /// Blocks until the screen changes or the timeout elapses.
    pub fn wait_for_change(&self, timeout: Duration) -> Result<bool> {
        self.display.wait_for_change(timeout)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::fake::FakeDisplay;
    use std::sync::Arc;

    fn at(offset_ms: u64) -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_millis(1_000_000 + offset_ms)
    }

    #[test]
    fn a_second_call_reuses_the_cached_frame() {
        let display = Arc::new(FakeDisplay::new(64, 48));
        let src = FrameSource::new(Box::new(display.clone()));

        src.capture_now(None).unwrap();
        src.capture_now(None).unwrap();
        assert_eq!(display.capture_count(), 2, "capture_now always captures");

        // capture_after with a time already satisfied must not capture again.
        let taken = src.latest().unwrap().captured_at;
        src.capture_after(taken, None).unwrap();
        assert_eq!(display.capture_count(), 2, "a fresh-enough cached frame is reused");
    }

    #[test]
    fn a_stale_frame_is_never_returned() {
        // The failure this prevents: click at t=100, screenshot at t=101, and
        // the agent receives the t=90 frame from before its own click.
        let display = Arc::new(FakeDisplay::new(64, 48));
        display.set_clock(at(90));
        let src = FrameSource::new(Box::new(display.clone()));

        src.capture_now(None).unwrap(); // cached at t=90
        assert_eq!(display.capture_count(), 1);

        // An action completed at t=100. The cached frame predates it.
        display.set_clock(at(101));
        let frame = src.capture_after(at(100), None).unwrap();

        assert_eq!(display.capture_count(), 2, "the stale frame must force a re-capture");
        assert!(frame.captured_at >= at(100), "returned frame still predates the action");
    }

    #[test]
    fn region_captures_neither_read_nor_write_the_cache() {
        // A region capture holds only part of the screen. Serving it later as a
        // full-screen frame would hand the agent a partial view of the world.
        let display = Arc::new(FakeDisplay::new(64, 48));
        let src = FrameSource::new(Box::new(display.clone()));
        let region = Some(Rect { x: 0, y: 0, width: 8, height: 8 });

        src.capture_now(region).unwrap();
        assert!(src.latest().is_none(), "a region capture must not populate the cache");

        src.capture_after(at(0), region).unwrap();
        assert_eq!(display.capture_count(), 2, "a region request always captures");
    }

    #[test]
    fn frame_length_matches_its_declared_dimensions() {
        let display = FakeDisplay::new(64, 48);
        let f = display.capture(None).unwrap();
        assert_eq!(f.pixels.len(), f.byte_len());
        assert_eq!(f.byte_len(), 64 * 48 * 4);
    }
}
