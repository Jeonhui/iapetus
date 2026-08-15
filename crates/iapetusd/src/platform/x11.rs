//! The X11 backend (PRD §6.2).
//!
//! Capture is `GetImage` on the root window; input is the XTEST extension.
//! x11rb speaks the protocol over a socket in pure Rust, so nothing here links
//! against libX11 and the container needs no `-dev` packages.
//!
//! The hard part is typing. XTEST sends *keycodes*, but a Desktop must type
//! arbitrary text including Hangul, and no keyboard layout maps those. The
//! solution is the one xdotool uses: temporarily bind a spare keycode to the
//! character's keysym, press it, and restore the map. §15.2 makes verifying
//! this mandatory in CI, because jamo splitting here is fatal in the Korean
//! market and is exactly what anglophone stacks leave untested.

use std::sync::Mutex;
use std::thread::sleep;
use std::time::{Duration, Instant, SystemTime};

use x11rb::connection::Connection;
use x11rb::protocol::damage::{self, ConnectionExt as _, ReportLevel};
use x11rb::protocol::xproto::{
    ConnectionExt as _, GetKeyboardMappingReply, ImageFormat, Keycode, Keysym, Screen, Window,
};
use x11rb::protocol::xtest::ConnectionExt as _;
use x11rb::protocol::Event;
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;

use super::{Button, Display, Frame, Input, PlatformError, Rect, Result, ScreenInfo};

fn x11_err<E: std::fmt::Display>(what: &str) -> impl FnOnce(E) -> PlatformError + '_ {
    move |e| PlatformError::CaptureFailed(format!("{what}: {e}"))
}

fn input_err<E: std::fmt::Display>(what: &str) -> impl FnOnce(E) -> PlatformError + '_ {
    move |e| PlatformError::InputRejected(format!("{what}: {e}"))
}

/// X11 event type constants used with XTEST's `fake_input`.
const MOTION_NOTIFY: u8 = 6;
const BUTTON_PRESS: u8 = 4;
const BUTTON_RELEASE: u8 = 5;
const KEY_PRESS: u8 = 2;
const KEY_RELEASE: u8 = 3;

/// X11 maps vertical scrolling onto buttons 4 and 5, horizontal onto 6 and 7.
const BTN_SCROLL_UP: u8 = 4;
const BTN_SCROLL_DOWN: u8 = 5;
const BTN_SCROLL_LEFT: u8 = 6;
const BTN_SCROLL_RIGHT: u8 = 7;

fn button_code(b: Button) -> u8 {
    match b {
        Button::Left => 1,
        Button::Middle => 2,
        Button::Right => 3,
    }
}

// ── Display ───────────────────────────────────────────────────────────────

pub struct X11Display {
    conn: RustConnection,
    root: Window,
    width: u16,
    height: u16,
    /// Present only when the DAMAGE extension is available. Without it,
    /// `wait_for_change` degrades to a timed wait and says so.
    damage: Option<damage::Damage>,
}

impl X11Display {
    pub fn open() -> Result<Self> {
        let (conn, screen_num) = x11rb::connect(None)
            .map_err(|e| PlatformError::DisplayUnavailable(e.to_string()))?;
        let screen: &Screen = &conn.setup().roots[screen_num];
        let (root, width, height) = (screen.root, screen.width_in_pixels, screen.height_in_pixels);

        // DAMAGE is what lets the capture loop idle at zero CPU when nothing
        // moves (§6.3). Its absence is a performance fault, not a correctness
        // one, so it degrades rather than failing to start.
        let damage = conn
            .damage_query_version(1, 1)
            .ok()
            .and_then(|c| c.reply().ok())
            .and_then(|_| {
                let id = conn.generate_id().ok()?;
                conn.damage_create(id, root, ReportLevel::NON_EMPTY).ok()?;
                Some(id)
            });
        conn.flush().map_err(x11_err("flush"))?;

        Ok(Self { conn, root, width, height, damage })
    }

    #[must_use]
    pub fn has_damage(&self) -> bool {
        self.damage.is_some()
    }
}

impl Display for X11Display {
    fn capture(&self, region: Option<Rect>) -> Result<Frame> {
        let (x, y, w, h) = match region {
            Some(r) => (r.x as i16, r.y as i16, r.width as u16, r.height as u16),
            None => (0, 0, self.width, self.height),
        };
        if w == 0 || h == 0 {
            return Err(PlatformError::CaptureFailed("zero-sized region".into()));
        }

        let reply = self
            .conn
            .get_image(ImageFormat::Z_PIXMAP, self.root, x, y, w, h, !0)
            .map_err(x11_err("get_image"))?
            .reply()
            .map_err(x11_err("get_image reply"))?;

        // Stamp the time the pixels were actually read, not when the call was
        // made: the §6.3 freshness contract compares against this.
        let captured_at = SystemTime::now();

        // X returns Z_PIXMAP as BGRX on little-endian 24/32-bit visuals. The
        // Frame contract is RGBA, so swap and force the alpha byte — X leaves
        // it undefined and a passthrough yields a fully transparent image.
        let src = reply.data;
        let px = (w as usize) * (h as usize);
        if src.len() < px * 4 {
            return Err(PlatformError::CaptureFailed(format!(
                "short image: {} bytes for {}x{}",
                src.len(),
                w,
                h
            )));
        }
        let mut pixels = Vec::with_capacity(px * 4);
        for chunk in src.chunks_exact(4).take(px) {
            pixels.extend_from_slice(&[chunk[2], chunk[1], chunk[0], 0xFF]);
        }

        Ok(Frame { width: w as u32, height: h as u32, pixels, captured_at })
    }

    fn screen_info(&self) -> Result<ScreenInfo> {
        Ok(ScreenInfo {
            width: self.width as u32,
            height: self.height as u32,
            // Xvfb reports no physical size, so DPI is the X default rather
            // than a measurement. §7.2 only requires that clients be told the
            // frame of reference, not that it be physically accurate.
            dpi: 96,
            monitor_count: 1,
        })
    }

    fn wait_for_change(&self, timeout: Duration) -> Result<bool> {
        let Some(_) = self.damage else {
            sleep(timeout);
            return Ok(false);
        };

        let deadline = Instant::now() + timeout;
        loop {
            while let Some(ev) = self.conn.poll_for_event().map_err(x11_err("poll_for_event"))? {
                if let Event::DamageNotify(_) = ev {
                    // Subtract so the region is reported again next time it
                    // changes; without this the first event is the only one.
                    if let Some(d) = self.damage {
                        let _ = self.conn.damage_subtract(d, 0u32, 0u32);
                        let _ = self.conn.flush();
                    }
                    return Ok(true);
                }
            }
            if Instant::now() >= deadline {
                return Ok(false);
            }
            // A 5ms tick costs ~200 idle wakeups per second, far below the
            // ~0.01 vCPU §6.3 budgets for an unwatched Desktop. Polling the
            // connection's file descriptor would remove even that; it is an
            // optimization, not a correctness fix.
            sleep(Duration::from_millis(5).min(deadline - Instant::now()));
        }
    }
}

// ── Input ─────────────────────────────────────────────────────────────────

/// Keysyms for the named keys a combo string may contain.
///
/// Only names that actually appear in `key` combos are listed. An unknown name
/// is an error rather than a silent no-op — an agent must be able to tell that
/// its keystroke did not happen (§6.2 "differences are surfaced, not hidden").
fn named_keysym(name: &str) -> Option<Keysym> {
    Some(match name.to_ascii_lowercase().as_str() {
        "enter" | "return" => 0xFF0D,
        "tab" => 0xFF09,
        "escape" | "esc" => 0xFF1B,
        "backspace" => 0xFF08,
        "delete" | "del" => 0xFFFF,
        "home" => 0xFF50,
        "end" => 0xFF57,
        "pageup" => 0xFF55,
        "pagedown" => 0xFF56,
        "left" => 0xFF51,
        "up" => 0xFF52,
        "right" => 0xFF53,
        "down" => 0xFF54,
        "space" => 0x0020,
        "ctrl" | "control" => 0xFFE3,
        "alt" => 0xFFE9,
        "shift" => 0xFFE1,
        "super" | "meta" | "win" | "cmd" => 0xFFEB,
        "f1" => 0xFFBE, "f2" => 0xFFBF, "f3" => 0xFFC0, "f4" => 0xFFC1,
        "f5" => 0xFFC2, "f6" => 0xFFC3, "f7" => 0xFFC4, "f8" => 0xFFC5,
        "f9" => 0xFFC6, "f10" => 0xFFC7, "f11" => 0xFFC8, "f12" => 0xFFC9,
        _ => return None,
    })
}

/// Resolves a key token: a named key, or a single character.
///
/// A multi-character token that is not a known name is an error rather than a
/// best guess — an agent must be able to tell its keystroke did not happen.
fn resolve_key(token: &str) -> Option<Keysym> {
    if let Some(ks) = named_keysym(token) {
        return Some(ks);
    }
    let mut chars = token.chars();
    match (chars.next(), chars.next()) {
        (Some(c), None) => Some(char_keysym(c)),
        _ => None,
    }
}

/// The keysym for a character.
///
/// Latin-1 maps directly; everything else uses the Unicode keysym range
/// (`0x01000000 + codepoint`), which is how Hangul reaches X at all.
fn char_keysym(c: char) -> Keysym {
    let cp = c as u32;
    if cp < 0x100 {
        cp
    } else {
        0x0100_0000 + cp
    }
}

pub struct X11Input {
    conn: RustConnection,
    root: Window,
    width: u16,
    height: u16,
    mapping: GetKeyboardMappingReply,
    min_keycode: Keycode,
    /// A keycode with no keysyms bound, borrowed to type characters the layout
    /// cannot otherwise produce and restored immediately afterwards.
    scratch: Keycode,
    held_keys: Mutex<Vec<Keycode>>,
    held_buttons: Mutex<Vec<u8>>,
}

impl X11Input {
    pub fn open() -> Result<Self> {
        let (conn, screen_num) = x11rb::connect(None)
            .map_err(|e| PlatformError::DisplayUnavailable(e.to_string()))?;
        let screen = &conn.setup().roots[screen_num];
        let (root, width, height) = (screen.root, screen.width_in_pixels, screen.height_in_pixels);

        conn.xtest_get_version(2, 2)
            .map_err(input_err("xtest_get_version"))?
            .reply()
            .map_err(|e| {
                PlatformError::Unsupported("XTEST extension is unavailable; input is impossible")
                    .tap(e)
            })?;

        let setup = conn.setup();
        let (min_keycode, max_keycode) = (setup.min_keycode, setup.max_keycode);
        let count = max_keycode - min_keycode + 1;
        let mapping = conn
            .get_keyboard_mapping(min_keycode, count)
            .map_err(input_err("get_keyboard_mapping"))?
            .reply()
            .map_err(input_err("get_keyboard_mapping reply"))?;

        let scratch = Self::find_scratch(&mapping, min_keycode).ok_or_else(|| {
            PlatformError::Unsupported("no unused keycode available for Unicode typing")
        })?;

        Ok(Self {
            conn,
            root,
            width,
            height,
            mapping,
            min_keycode,
            scratch,
            held_keys: Mutex::new(Vec::new()),
            held_buttons: Mutex::new(Vec::new()),
        })
    }

    /// The keycode borrowed for characters the layout cannot produce.
    ///
    /// Exposed so the L2 tests can confirm that Hangul actually took the remap
    /// path rather than some accidental substitution.
    #[must_use]
    pub fn scratch_keycode(&self) -> Keycode {
        self.scratch
    }

    /// Picks a keycode whose keysyms are all unbound, scanning from the top so
    /// the layout's real keys are left alone.
    fn find_scratch(m: &GetKeyboardMappingReply, min: Keycode) -> Option<Keycode> {
        let per = m.keysyms_per_keycode as usize;
        let n = m.keysyms.len() / per;
        (0..n)
            .rev()
            .find(|i| m.keysyms[i * per..(i + 1) * per].iter().all(|k| *k == 0))
            .map(|i| min + i as u8)
    }

    /// Finds a keycode already bound to `keysym`, and whether shift is needed.
    fn lookup(&self, keysym: Keysym) -> Option<(Keycode, bool)> {
        let per = self.mapping.keysyms_per_keycode as usize;
        for (i, chunk) in self.mapping.keysyms.chunks(per).enumerate() {
            if chunk.first() == Some(&keysym) {
                return Some((self.min_keycode + i as u8, false));
            }
            if per > 1 && chunk.get(1) == Some(&keysym) {
                return Some((self.min_keycode + i as u8, true));
            }
        }
        None
    }

    fn fake(&self, ty: u8, detail: u8, x: i16, y: i16) -> Result<()> {
        self.conn
            .xtest_fake_input(ty, detail, 0, self.root, x, y, 0)
            .map_err(input_err("xtest_fake_input"))?;
        self.conn.flush().map_err(input_err("flush"))?;
        Ok(())
    }

    /// Presses and releases `keysym`, binding it to the scratch keycode first
    /// if the layout has no key for it. Hangul always takes the scratch path.
    fn tap_keysym(&self, keysym: Keysym, shift: bool) -> Result<()> {
        let (code, needs_shift, borrowed) = match self.lookup(keysym) {
            Some((c, s)) => (c, s, false),
            None => {
                let per = self.mapping.keysyms_per_keycode as usize;
                self.conn
                    .change_keyboard_mapping(1, self.scratch, per as u8, &vec![keysym; per])
                    .map_err(input_err("change_keyboard_mapping"))?;
                self.conn.flush().map_err(input_err("flush"))?;
                // X clients cache the map; give them the MappingNotify before
                // the synthetic key arrives, or the press is decoded with the
                // stale layout and the wrong character appears.
                sleep(Duration::from_millis(10));
                (self.scratch, false, true)
            }
        };

        let shift_code = if shift || needs_shift { self.lookup(0xFFE1).map(|(c, _)| c) } else { None };
        if let Some(sc) = shift_code {
            self.fake(KEY_PRESS, sc, 0, 0)?;
        }
        self.fake(KEY_PRESS, code, 0, 0)?;
        self.fake(KEY_RELEASE, code, 0, 0)?;
        if let Some(sc) = shift_code {
            self.fake(KEY_RELEASE, sc, 0, 0)?;
        }

        if borrowed {
            // Restore immediately. Leaving the scratch key bound would corrupt
            // the layout for the human who takes over next.
            let per = self.mapping.keysyms_per_keycode as usize;
            self.conn
                .change_keyboard_mapping(1, self.scratch, per as u8, &vec![0u32; per])
                .map_err(input_err("restore keyboard mapping"))?;
            // A round trip, not a flush. `flush` only pushes the bytes onto the
            // socket; it says nothing about the server having processed them, so
            // the call could return with the scratch keycode still bound. §5.6
            // hands the lease over the moment an action completes, and the human
            // who takes it would find one key typing the agent's last syllable.
            self.conn.sync().map_err(input_err("sync after restoring the mapping"))?;
        }
        Ok(())
    }

    fn check_bounds(&self, x: i32, y: i32) -> Result<()> {
        if x < 0 || y < 0 || x >= self.width as i32 || y >= self.height as i32 {
            return Err(PlatformError::OutOfBounds {
                x,
                y,
                width: self.width as u32,
                height: self.height as u32,
            });
        }
        Ok(())
    }
}

/// Small helper so an X error can be attached to a static `Unsupported`.
trait Tap {
    fn tap<E: std::fmt::Display>(self, e: E) -> PlatformError;
}
impl Tap for PlatformError {
    fn tap<E: std::fmt::Display>(self, e: E) -> PlatformError {
        match self {
            PlatformError::Unsupported(msg) => PlatformError::InputRejected(format!("{msg}: {e}")),
            other => other,
        }
    }
}

impl Input for X11Input {
    fn move_to(&self, x: i32, y: i32) -> Result<()> {
        self.check_bounds(x, y)?;
        self.fake(MOTION_NOTIFY, 0, x as i16, y as i16)
    }

    fn click(&self, x: i32, y: i32, button: Button, count: u8) -> Result<()> {
        self.check_bounds(x, y)?;
        self.fake(MOTION_NOTIFY, 0, x as i16, y as i16)?;
        let b = button_code(button);
        for _ in 0..count.max(1) {
            self.fake(BUTTON_PRESS, b, 0, 0)?;
            self.fake(BUTTON_RELEASE, b, 0, 0)?;
        }
        Ok(())
    }

    fn button_down(&self, button: Button) -> Result<()> {
        let b = button_code(button);
        self.fake(BUTTON_PRESS, b, 0, 0)?;
        self.held_buttons.lock().unwrap().push(b);
        Ok(())
    }

    fn button_up(&self, button: Button) -> Result<()> {
        let b = button_code(button);
        self.fake(BUTTON_RELEASE, b, 0, 0)?;
        self.held_buttons.lock().unwrap().retain(|x| *x != b);
        Ok(())
    }

    fn scroll(&self, dx: i32, dy: i32) -> Result<()> {
        let step = |btn: u8, n: i32| -> Result<()> {
            for _ in 0..n.abs() {
                self.fake(BUTTON_PRESS, btn, 0, 0)?;
                self.fake(BUTTON_RELEASE, btn, 0, 0)?;
            }
            Ok(())
        };
        if dy != 0 {
            step(if dy < 0 { BTN_SCROLL_UP } else { BTN_SCROLL_DOWN }, dy)?;
        }
        if dx != 0 {
            step(if dx < 0 { BTN_SCROLL_LEFT } else { BTN_SCROLL_RIGHT }, dx)?;
        }
        Ok(())
    }

    fn type_text(&self, text: &str, delay: Duration) -> Result<()> {
        // The text is already NFC-normalized upstream (§8.2). Normalizing again
        // here would risk decomposing precisely what we must keep composed.
        for c in text.chars() {
            let shift = c.is_ascii_uppercase() || "~!@#$%^&*()_+{}|:\"<>?".contains(c);
            self.tap_keysym(char_keysym(c), shift)?;
            if !delay.is_zero() {
                sleep(delay);
            }
        }
        Ok(())
    }

    fn key(&self, combo: &str) -> Result<()> {
        let parts: Vec<&str> = combo.split('+').map(str::trim).filter(|s| !s.is_empty()).collect();
        let Some((main, mods)) = parts.split_last() else {
            return Err(PlatformError::InputRejected("empty key combo".into()));
        };

        let mut pressed = Vec::new();
        for m in mods {
            let ks = named_keysym(m)
                .ok_or_else(|| PlatformError::InputRejected(format!("unknown modifier `{m}`")))?;
            let (code, _) = self
                .lookup(ks)
                .ok_or_else(|| PlatformError::InputRejected(format!("`{m}` is not on this layout")))?;
            self.fake(KEY_PRESS, code, 0, 0)?;
            pressed.push(code);
        }

        let result = (|| {
            let ks = resolve_key(main)
                .ok_or_else(|| PlatformError::InputRejected(format!("unknown key `{main}`")))?;
            self.tap_keysym(ks, false)
        })();

        // Release modifiers even if the main key failed, so a bad combo cannot
        // leave Ctrl latched — the §5.6 failure this whole path guards against.
        for code in pressed.into_iter().rev() {
            self.fake(KEY_RELEASE, code, 0, 0)?;
        }
        result
    }

    fn key_down(&self, key: &str) -> Result<()> {
        let ks = resolve_key(key)
            .ok_or_else(|| PlatformError::InputRejected(format!("unknown key `{key}`")))?;
        let (code, _) = self
            .lookup(ks)
            .ok_or_else(|| PlatformError::InputRejected(format!("`{key}` is not on this layout")))?;
        self.fake(KEY_PRESS, code, 0, 0)?;
        self.held_keys.lock().unwrap().push(code);
        Ok(())
    }

    fn key_up(&self, key: &str) -> Result<()> {
        let ks = resolve_key(key)
            .ok_or_else(|| PlatformError::InputRejected(format!("unknown key `{key}`")))?;
        let (code, _) = self
            .lookup(ks)
            .ok_or_else(|| PlatformError::InputRejected(format!("`{key}` is not on this layout")))?;
        self.fake(KEY_RELEASE, code, 0, 0)?;
        self.held_keys.lock().unwrap().retain(|c| *c != code);
        Ok(())
    }

    fn release_all(&self) -> Result<()> {
        let keys: Vec<Keycode> = self.held_keys.lock().unwrap().drain(..).collect();
        for c in keys {
            self.fake(KEY_RELEASE, c, 0, 0)?;
        }
        let buttons: Vec<u8> = self.held_buttons.lock().unwrap().drain(..).collect();
        for b in buttons {
            self.fake(BUTTON_RELEASE, b, 0, 0)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hangul_maps_into_the_unicode_keysym_range() {
        // U+C548 안 → 0x0100C548. Getting this wrong is how jamo splitting
        // starts, so it is asserted rather than assumed.
        assert_eq!(char_keysym('안'), 0x0100_C548);
        assert_eq!(char_keysym('요'), 0x0100_C694);
    }

    #[test]
    fn latin1_maps_directly_without_the_unicode_offset() {
        assert_eq!(char_keysym('a'), 0x61);
        assert_eq!(char_keysym('Z'), 0x5A);
    }

    #[test]
    fn unknown_key_names_are_rejected_rather_than_ignored() {
        assert!(named_keysym("enter").is_some());
        assert!(named_keysym("ctrl").is_some());
        assert!(named_keysym("nonexistent").is_none());
    }

    #[test]
    fn resolve_key_takes_names_and_single_characters_only() {
        assert_eq!(resolve_key("Enter"), Some(0xFF0D));
        assert_eq!(resolve_key("c"), Some(0x63));
        assert_eq!(resolve_key("안"), Some(0x0100_C548));
        // A multi-character token that is not a known name must fail rather
        // than silently typing its first letter.
        assert_eq!(resolve_key("ctrlc"), None);
    }
}
