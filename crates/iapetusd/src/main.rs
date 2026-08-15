//! The `iapetusd` entry point.
//!
//! Scope today: it supervises the display session and reports what it can and
//! cannot do. The Control Plane channel (§19.5) and the platform drivers are
//! not implemented yet, and this binary says so rather than pretending — a
//! daemon that appears healthy while doing nothing is worse than one that is
//! clear about its state.

use std::process::ExitCode;

use iapetusd::platform::fake::{FakeDisplay, FakeInput};
use iapetusd::platform::{Button, Display, Input};

/// Protocol range this build speaks (§19.4). The Control Plane picks the
/// highest common version; no overlap leaves the Desktop `DEGRADED`.
const PROTOCOL_MIN: u32 = 3;
const PROTOCOL_MAX: u32 = 5;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();

    match args.first().map(String::as_str) {
        Some("--selftest") => selftest(),
        Some("--version") | Some("-V") => {
            println!(
                "iapetusd {} (protocol {}..={})",
                env!("CARGO_PKG_VERSION"),
                PROTOCOL_MIN,
                PROTOCOL_MAX
            );
            ExitCode::SUCCESS
        }
        Some("--supervise-x11") => supervise(),
        Some(other) => {
            eprintln!("unknown argument: {other}");
            usage();
            ExitCode::from(2)
        }
        None => {
            usage();
            ExitCode::from(2)
        }
    }
}

fn usage() {
    eprintln!(
        "usage: iapetusd [--supervise-x11 | --selftest | --version]\n\
         \n\
         --supervise-x11  hold the display session open (the container entry point)\n\
         --selftest       exercise the platform layer and report\n\
         --version        print version and supported protocol range"
    );
}

/// Exercises the platform layer end to end through the in-memory backend.
///
/// This verifies the logic above the platform boundary — capture shape, bounds
/// rejection, and the input-release path §5.6 requires on handover. The real
/// X11 drivers are verified separately by the L2 checks that run against a live
/// X server, because only those can catch what this cannot.
fn selftest() -> ExitCode {
    let mut failures = 0;

    let display = FakeDisplay::new(1920, 1080);
    match display.capture(None) {
        Ok(f) if f.pixels.len() == f.byte_len() => println!("  ok   capture returns a well-formed frame"),
        Ok(f) => {
            println!("  FAIL capture length {} != declared {}", f.pixels.len(), f.byte_len());
            failures += 1;
        }
        Err(e) => {
            println!("  FAIL capture: {e}");
            failures += 1;
        }
    }

    let input = FakeInput::new().with_screen(1920, 1080);
    if input.click(1920, 0, Button::Left, 1).is_err() {
        println!("  ok   out-of-screen coordinates are rejected, not clamped");
    } else {
        println!("  FAIL an out-of-screen coordinate was accepted");
        failures += 1;
    }

    input.key_down("ctrl").ok();
    input.release_all().ok();
    if input.held_keys().is_empty() {
        println!("  ok   release_all clears held keys before a lease handover");
    } else {
        println!("  FAIL keys still held after release_all: {:?}", input.held_keys());
        failures += 1;
    }

    println!();
    if failures == 0 {
        println!("selftest passed");
        ExitCode::SUCCESS
    } else {
        println!("selftest failed: {failures} check(s)");
        ExitCode::FAILURE
    }
}

/// Holds the process alive so the display session started by the entry point
/// stays up.
///
/// Not yet implemented here: the outbound gRPC stream to the Control Plane
/// (§19.5), the Frame Source capture loop, and the X11 drivers. Until those
/// exist this is a placeholder that keeps the container usable for the L2
/// checks, and it states that plainly on startup.
fn supervise() -> ExitCode {
    println!(
        "iapetusd {} starting (protocol {}..={})",
        env!("CARGO_PKG_VERSION"),
        PROTOCOL_MIN,
        PROTOCOL_MAX
    );
    println!("display: {}", std::env::var("DISPLAY").unwrap_or_else(|_| "<unset>".into()));
    println!("NOT IMPLEMENTED: Control Plane channel (§19.5), capture loop, X11 drivers");
    println!("holding the display session open; send SIGTERM to stop");

    // No async runtime yet — parking the thread is honest about there being no
    // work loop, where a busy-wait would merely look like one.
    loop {
        std::thread::park();
    }
}
