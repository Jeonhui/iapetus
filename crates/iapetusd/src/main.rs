//! The `iapetusd` entry point.
//!
//! Scope today: it supervises the display session and reports what it can and
//! cannot do. The Control Plane channel (§19.5) and the platform drivers are
//! not implemented yet, and this binary says so rather than pretending — a
//! daemon that appears healthy while doing nothing is worse than one that is
//! clear about its state.

use std::process::ExitCode;
use std::sync::Arc;

use iapetusd::channel::{self, ChannelConfig};
use iapetusd::dispatch::Dispatcher;
use iapetusd::frame::FrameSource;
use iapetusd::platform::fake::{FakeDisplay, FakeInput};
use iapetusd::platform::{Button, Display, Input};

/// Protocol range this build speaks (§19.4). The Control Plane picks the
/// highest common version; no overlap leaves the Desktop `DEGRADED`.
const PROTOCOL_MIN: i32 = 3;
const PROTOCOL_MAX: i32 = 5;

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
        Some("--connect") => connect(),
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
        "usage: iapetusd [--connect | --supervise-x11 | --selftest | --version]\n\
         \n\
         --connect        dial the Control Plane and serve actions (§19.5)\n\
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

/// Reads a required environment variable, naming it plainly when absent.
///
/// These are configuration, not defaults: guessing an endpoint or running
/// without a token would produce a daemon that looks configured and is not.
fn require_env(key: &str) -> std::result::Result<String, String> {
    std::env::var(key).map_err(|_| format!("{key} is not set"))
}

/// Builds the dispatcher over whichever platform this build has.
///
/// The fake backend is used when the X11 driver is not compiled in, and says
/// so: a daemon silently accepting clicks that go nowhere is the failure mode
/// this avoids.
fn build_dispatcher() -> Dispatcher {
    #[cfg(feature = "x11")]
    {
        use iapetusd::platform::x11::{X11Display, X11Input};
        match (X11Display::open(), X11Input::open()) {
            (Ok(d), Ok(i)) => {
                println!("platform: X11");
                return Dispatcher::new(FrameSource::new(Box::new(d)), Box::new(i));
            }
            (Err(e), _) | (_, Err(e)) => {
                eprintln!("X11 unavailable ({e}); falling back to the in-memory platform");
            }
        }
    }
    println!("platform: in-memory (no display driver compiled in; input goes nowhere)");
    Dispatcher::new(
        FrameSource::new(Box::new(FakeDisplay::new(1920, 1080))),
        Box::new(FakeInput::new().with_screen(1920, 1080)),
    )
}

/// Dials the Control Plane and serves actions until it asks the daemon to stop.
///
/// Everything here is configuration the guest is given, not discovered: §19.5
/// fixes the transport at mTLS with a Guest Token, so a missing certificate is
/// a startup failure rather than a downgrade.
fn connect() -> ExitCode {
    let cfg = match require_env("IAPETUS_CONTROL_ENDPOINT") {
        Ok(endpoint) => ChannelConfig {
            endpoint,
            protocol_min: PROTOCOL_MIN,
            protocol_max: PROTOCOL_MAX,
            ..Default::default()
        },
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::from(2);
        }
    };

    if let Err(e) = cfg.validate() {
        eprintln!("{e}");
        return ExitCode::from(2);
    }

    let (token, tls) = match load_credentials() {
        Ok(v) => v,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::from(2);
        }
    };

    let dispatcher = Arc::new(build_dispatcher());
    println!(
        "iapetusd {} connecting to {} (protocol {}..={})",
        env!("CARGO_PKG_VERSION"),
        cfg.endpoint,
        PROTOCOL_MIN,
        PROTOCOL_MAX
    );

    let rt = match tokio::runtime::Runtime::new() {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("could not start the async runtime: {e}");
            return ExitCode::FAILURE;
        }
    };

    rt.block_on(async {
        tokio::select! {
            () = channel::run_forever(&cfg, tls, &token, dispatcher) => {}
            // SIGTERM is how the host stops a Desktop. Exiting on it rather than
            // being killed lets the reconnect loop stop cleanly instead of
            // leaving the Control Plane waiting out three heartbeats.
            _ = shutdown_signal() => println!("received SIGTERM; stopping"),
        }
    });
    ExitCode::SUCCESS
}

async fn shutdown_signal() {
    use tokio::signal::unix::{signal, SignalKind};
    let Ok(mut term) = signal(SignalKind::terminate()) else { return };
    term.recv().await;
}

/// Loads the Guest Token and the mTLS material.
fn load_credentials() -> std::result::Result<(String, tonic::transport::ClientTlsConfig), String> {
    let token = require_env("IAPETUS_GUEST_TOKEN")?;
    let read = |k: &str| -> std::result::Result<Vec<u8>, String> {
        let path = require_env(k)?;
        std::fs::read(&path).map_err(|e| format!("{k} ({path}): {e}"))
    };

    let ca = read("IAPETUS_TLS_CA")?;
    let cert = read("IAPETUS_TLS_CERT")?;
    let key = read("IAPETUS_TLS_KEY")?;

    let tls = tonic::transport::ClientTlsConfig::new()
        .ca_certificate(tonic::transport::Certificate::from_pem(ca))
        .identity(tonic::transport::Identity::from_pem(cert, key));
    Ok((token, tls))
}

/// Holds the process alive so the display session started by the entry point
/// stays up.
///
/// Use `--connect` to serve actions. This mode exists for the L2 container
/// checks, which need a display session but no Control Plane.
fn supervise() -> ExitCode {
    println!(
        "iapetusd {} starting (protocol {}..={})",
        env!("CARGO_PKG_VERSION"),
        PROTOCOL_MIN,
        PROTOCOL_MAX
    );
    println!("display: {}", std::env::var("DISPLAY").unwrap_or_else(|_| "<unset>".into()));
    println!("no Control Plane channel in this mode; use --connect to serve actions");
    println!("holding the display session open; send SIGTERM to stop");

    // No async runtime yet — parking the thread is honest about there being no
    // work loop, where a busy-wait would merely look like one.
    loop {
        std::thread::park();
    }
}
