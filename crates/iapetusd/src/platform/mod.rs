//! Platform abstraction for capture and input (PRD §19.1).
//!
//! The Computer API is OS-agnostic (§6.2); these two traits are where that
//! promise is kept. Everything above them — the Frame Source, the daemon
//! stream, the action dispatcher — is written once and compiled for both.
//!
//! Unsafe FFI is confined to the implementations behind these traits. X11
//! (XTEST), DXGI, and SendInput are all unsafe; nothing above this module is.

use std::time::{Duration, SystemTime};

pub mod fake;

// Process spawning is an OS capability, not a display one, so it is gated on
// the family rather than on the `x11` feature: a Windows build needs its own
// implementation but the same trait.
#[cfg(unix)]
pub mod unix;

// Gated on the feature rather than the OS: x11rb is pure Rust and compiles
// anywhere, so `cargo check --features x11` gives fast feedback off Linux.
// Actually connecting requires an X server, which is why the behavioural
// checks live in the container (§15.1 L2).
#[cfg(feature = "x11")]
pub mod x11;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Button {
    Left,
    Right,
    Middle,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScreenInfo {
    pub width: u32,
    pub height: u32,
    pub dpi: u32,
    /// PRD §7.2 fixes v1 at a single monitor. The field exists so multi-monitor
    /// support in v2 does not change the shape of every response.
    pub monitor_count: u32,
}

/// How a `Frame`'s bytes are laid out.
///
/// Capture backends produce BGRX natively — X11's Z_PIXMAP on a little-endian
/// 24/32-bit visual, and DXGI's B8G8R8A8 on Windows. Converting the whole frame
/// to RGBA costs a pass over every pixel, and the streaming path does not need
/// it: a diff does not care about channel order, and only the tiles that
/// actually changed are ever encoded. So the format travels with the frame and
/// the conversion happens where it is genuinely required.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PixelFormat {
    /// Red, green, blue, alpha. What `screenshot` responses are encoded from.
    Rgba,
    /// Blue, green, red, and a byte X leaves undefined — never trust it as
    /// alpha, or the image comes out fully transparent.
    Bgrx,
}

/// One captured frame.
///
/// `captured_at` is what makes the §6.3 freshness contract checkable: a caller
/// can prove the frame postdates the action it followed.
#[derive(Debug, Clone)]
pub struct Frame {
    pub width: u32,
    pub height: u32,
    /// Tightly packed, `width * height * 4` bytes, laid out per `format`.
    pub pixels: Vec<u8>,
    pub format: PixelFormat,
    pub captured_at: SystemTime,
}

impl Frame {
    #[must_use]
    pub fn byte_len(&self) -> usize {
        (self.width as usize) * (self.height as usize) * 4
    }

    /// Reads one pixel as RGB, whatever the frame's layout.
    ///
    /// The three channels are returned rather than a converted buffer because
    /// every caller that needs RGB is writing into an encoder's input anyway.
    #[must_use]
    pub fn rgb_at(&self, offset: usize) -> [u8; 3] {
        let p = &self.pixels[offset..offset + 4];
        match self.format {
            PixelFormat::Rgba => [p[0], p[1], p[2]],
            PixelFormat::Bgrx => [p[2], p[1], p[0]],
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum PlatformError {
    #[error("display unavailable: {0}")]
    DisplayUnavailable(String),
    #[error("capture failed: {0}")]
    CaptureFailed(String),
    #[error("input rejected: {0}")]
    InputRejected(String),
    #[error("coordinate ({x}, {y}) is outside the {width}x{height} screen")]
    OutOfBounds { x: i32, y: i32, width: u32, height: u32 },
    #[error("not supported on this platform: {0}")]
    Unsupported(&'static str),
}

pub type Result<T> = std::result::Result<T, PlatformError>;

/// One window, as the guest sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowInfo {
    /// The native handle. Rendered as `win_<id>` on the wire (§8.2).
    pub id: u64,
    pub title: String,
    pub bounds: Rect,
    /// The process that owns it, when the window manager reports one. X11
    /// clients are not required to set `_NET_WM_PID`, so this can be absent
    /// even for a window that plainly belongs to a process we launched.
    pub pid: Option<u32>,
}

/// What to launch, already resolved from a catalog key or taken verbatim.
#[derive(Debug, Clone)]
pub struct LaunchSpec {
    pub command: String,
    pub args: Vec<String>,
    pub cwd: Option<String>,
    /// §7.3 grants OWNER mode, so this is a convenience for reaching root from
    /// a non-root daemon — not a privilege boundary.
    pub elevated: bool,
}

/// The outcome of a synchronous `shell.exec`.
#[derive(Debug, Clone)]
pub struct ShellOutput {
    pub exit_code: i32,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    /// Either stream hit the cap and was cut. §8.2 truncates rather than
    /// streaming here; `shell.stream` is the unbounded path.
    pub truncated: bool,
    /// The deadline elapsed and the process was killed. `exit_code` is then the
    /// signal-based code, and the agent needs to know the difference between
    /// "exited non-zero" and "we stopped waiting".
    pub timed_out: bool,
}

/// Starting programs and running commands.
pub trait Process: Send + Sync {
    /// Spawns and returns immediately with the child's pid.
    ///
    /// Deliberately not waiting: a GUI program does not exit, and §7.2's
    /// `wait_for_window` is the caller's way of asking to block on something
    /// that actually happens.
    fn launch(&self, spec: &LaunchSpec) -> Result<u32>;

    /// Runs a command through a shell and waits for it, capturing output.
    ///
    /// Synchronous by §7.2. Output is captured, capped, and truncated rather
    /// than streamed — an agent calling `shell.exec` asked for the result, not
    /// a live feed, and an uncapped capture lets a runaway `yes` exhaust the
    /// daemon's memory.
    fn run(&self, spec: &LaunchSpec, timeout: Duration, cap: usize) -> Result<ShellOutput>;
}

/// Querying and waiting on windows.
pub trait Windows: Send + Sync {
    fn list(&self) -> Result<Vec<WindowInfo>>;

    /// Blocks until a window owned by `pid` appears, or the timeout elapses.
    ///
    /// Returns `Ok(None)` on timeout rather than an error: a program that
    /// started but has not drawn yet is a different situation from one that
    /// failed to start, and collapsing them would make the agent retry a
    /// launch that already succeeded.
    fn wait_for_window(&self, pid: u32, timeout: Duration) -> Result<Option<WindowInfo>>;
}

/// Screen capture.
pub trait Display: Send + Sync {
    /// Captures the screen, or a region of it, right now.
    ///
    /// Implementations must stamp `captured_at` at the moment the pixels are
    /// read, not when the call was made — the difference is exactly what the
    /// freshness contract measures.
    ///
    /// The frame may be returned in the backend's native layout; callers read
    /// `format` rather than assuming RGBA.
    fn capture(&self, region: Option<Rect>) -> Result<Frame>;

    fn screen_info(&self) -> Result<ScreenInfo>;

    /// Blocks until the screen changes or the timeout elapses.
    ///
    /// Backed by XDamage on X11 and by DXGI's own change notification on
    /// Windows. Returning `false` on timeout is what lets the Frame Source idle
    /// at zero CPU while nothing moves (§6.3).
    fn wait_for_change(&self, timeout: Duration) -> Result<bool>;
}

/// Keyboard and pointer input.
pub trait Input: Send + Sync {
    fn move_to(&self, x: i32, y: i32) -> Result<()>;
    fn click(&self, x: i32, y: i32, button: Button, count: u8) -> Result<()>;
    fn button_down(&self, button: Button) -> Result<()>;
    fn button_up(&self, button: Button) -> Result<()>;
    fn scroll(&self, dx: i32, dy: i32) -> Result<()>;

    /// Types text. `text` arrives already NFC-normalized (§8.2); implementations
    /// must not normalize again, and must go through the IME rather than
    /// synthesizing keycodes, or Hangul jamo split (§15.2).
    fn type_text(&self, text: &str, delay: Duration) -> Result<()>;

    fn key(&self, combo: &str) -> Result<()>;
    fn key_down(&self, key: &str) -> Result<()>;
    fn key_up(&self, key: &str) -> Result<()>;

    /// Releases every held key and pointer button.
    ///
    /// Called immediately before a control lease changes hands (§5.6). Without
    /// it, an agent preempted after `key.down ctrl` leaves Ctrl latched and
    /// every subsequent keystroke by the human is read as a shortcut.
    fn release_all(&self) -> Result<()>;
}
