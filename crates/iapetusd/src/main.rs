//! The `iapetusd` entry point.
//!
//! Scope today: it supervises the display session and reports what it can and
//! cannot do. The Control Plane channel (§19.5) and the platform drivers are
//! not implemented yet, and this binary says so rather than pretending — a
//! daemon that appears healthy while doing nothing is worse than one that is
//! clear about its state.

use std::process::ExitCode;
use std::sync::Arc;

use iapetusd::catalog::{self, Catalog};
use iapetusd::channel::{self, ChannelConfig};
use iapetusd::dispatch::Dispatcher;
use iapetusd::frame::FrameSource;
use iapetusd::platform::fake::{FakeDisplay, FakeInput};
use iapetusd::platform::unix::UnixProcess;
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
        Some("--screenshot") => screenshot(args.get(1).map(String::as_str)),
        Some("--stream") => stream(args.get(1).map(String::as_str)),
        Some("--stream-bench") => stream_bench(args.get(1).and_then(|s| s.parse().ok()).unwrap_or(10)),
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
        "usage: iapetusd [--connect | --stream URL | --screenshot FILE | --supervise-x11 | --selftest | --version]\n\
         \n\
         --connect        dial the Control Plane and serve actions (§19.5)\n\
         --screenshot F   capture the screen through the real pipeline into F (png)\n\
         --stream URL     push the screen to a stream gateway and take its input\n\
         --stream-bench N run the capture and encode loop for N seconds and report\n\
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

/// Captures the screen and writes it out as a PNG.
///
/// Not a product feature — the product path is §6.3's stream and §7.5's viewer,
/// neither of which exists yet. This exists because until they do, there is no
/// way to look at a Desktop at all, and an operator debugging a guest that
/// "does nothing" needs to see whether anything is on screen. It goes through
/// the real capture and encode path rather than calling X directly, so what it
/// shows is what an agent would have received.
fn screenshot(path: Option<&str>) -> ExitCode {
    let Some(path) = path else {
        eprintln!("--screenshot needs an output path");
        return ExitCode::from(2);
    };

    let d = build_dispatcher();
    let action = iapetus_proto::v1::Action {
        kind: Some(iapetus_proto::v1::action::Kind::Screenshot(
            iapetus_proto::v1::ScreenshotRequest {
                format: iapetus_proto::v1::ImageFormat::Png as i32,
                quality: 0,
                region: None,
                scale: None,
            },
        )),
    };

    let result = match d.execute(&action) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("capture failed: {e}");
            return ExitCode::FAILURE;
        }
    };
    let Some(iapetus_proto::v1::action_result::Value::Screenshot(shot)) = result.value else {
        eprintln!("capture returned no screenshot");
        return ExitCode::FAILURE;
    };
    let Some(iapetus_proto::v1::screenshot_response::Payload::Inline(bytes)) = shot.payload else {
        eprintln!("capture returned no image data");
        return ExitCode::FAILURE;
    };

    if let Err(e) = std::fs::write(path, &bytes) {
        eprintln!("writing {path}: {e}");
        return ExitCode::FAILURE;
    }
    println!("wrote {} ({}x{}, {} bytes)", path, shot.width, shot.height, bytes.len());
    ExitCode::SUCCESS
}

/// Streams the screen to a gateway and applies the input it sends back.
///
/// This is §6.3's WebSocket JPEG fallback, not the default path — WebRTC is
/// (§19.6). It exists because the fallback is specified, self-contained, and
/// the only way to watch a Desktop before the SFU exists.
fn stream(endpoint: Option<&str>) -> ExitCode {
    let Some(endpoint) = endpoint else {
        eprintln!("--stream needs the gateway's ingest URL, e.g. ws://gateway:8080/ingest?token=…");
        return ExitCode::from(2);
    };

    let dispatcher = Arc::new(build_dispatcher());
    println!("streaming to {endpoint}");

    let rt = match tokio::runtime::Runtime::new() {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("could not start the async runtime: {e}");
            return ExitCode::FAILURE;
        }
    };

    rt.block_on(async {
        // Reconnects on the same curve as the Control Plane channel: a gateway
        // restart should not leave a Desktop permanently unwatchable.
        let mut backoff = iapetusd::channel::Backoff::default();
        loop {
            let started = std::time::Instant::now();
            match iapetusd::viewer_link::run(
                endpoint,
                Arc::clone(&dispatcher),
                iapetusd::viewer_link::frame_interval(),
            )
            .await
            {
                Ok(()) => eprintln!("gateway closed the stream; reconnecting"),
                Err(e) => eprintln!("stream: {e}"),
            }
            if started.elapsed() >= iapetusd::channel::MIN_SESSION_FOR_BACKOFF_RESET {
                backoff.reset();
            }
            tokio::time::sleep(backoff.next(iapetusd::channel::jitter_unit())).await;
        }
    })
}

/// Measures where a streamed frame's time and bytes actually go.
///
/// §12.4 sizes hosts on encoding cost, and "the viewer feels slow" has at least
/// three different causes with three different fixes — capture, hashing, and
/// JPEG. Guessing which one is dominant is how the wrong thing gets optimised.
fn stream_bench(seconds: u64) -> ExitCode {
    use iapetusd::stream::{TileEncoder, DEFAULT_QUALITY};

    let d = build_dispatcher();
    let mut enc = TileEncoder::new(DEFAULT_QUALITY);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(seconds);

    let (mut cap_us, mut hash_us, mut jpeg_us) = (Vec::new(), Vec::new(), Vec::new());
    let (mut frames, mut changed, mut bytes) = (0u64, 0u64, 0u64);
    // The keyframe re-encodes every tile, which is exactly the all-changed
    // worst case. Reported on its own because one frame in a thousand vanishes
    // into a p95 — and it is the number host capacity has to be sized against.
    let mut key = (0usize, 0usize, 0u64, 0u64); // tiles, bytes, diff_us, jpeg_us

    while std::time::Instant::now() < deadline {
        let t0 = std::time::Instant::now();
        let frame = match d.capture_for_stream() {
            Ok(f) => f,
            Err(e) => {
                eprintln!("capture: {e}");
                return ExitCode::FAILURE;
            }
        };
        cap_us.push(t0.elapsed().as_micros() as u64);

        let update = match enc.encode(&frame, false) {
            Ok(u) => u,
            Err(e) => {
                eprintln!("encode: {e}");
                return ExitCode::FAILURE;
            }
        };
        let s = enc.stats();
        hash_us.push(s.hash.as_micros() as u64);
        jpeg_us.push(s.jpeg.as_micros() as u64);
        frames += 1;
        changed += s.tiles_changed as u64;
        bytes += s.bytes as u64;
        if s.tiles_changed > key.0 {
            key = (
                s.tiles_changed,
                s.bytes,
                s.hash.as_micros() as u64,
                s.jpeg.as_micros() as u64,
            );
        }
        let _ = update;
    }

    let pct = |v: &mut Vec<u64>, p: f64| -> u64 {
        if v.is_empty() {
            return 0;
        }
        v.sort_unstable();
        v[(((v.len() - 1) as f64) * p) as usize]
    };

    println!();
    println!("frames {frames} over {seconds}s ({:.1}/s achievable)", frames as f64 / seconds as f64);
    println!("changed tiles {changed} ({:.1}/frame), {:.0} KiB total", changed as f64 / frames.max(1) as f64, bytes as f64 / 1024.0);
    println!();
    println!("               p50        p95        (microseconds)");
    println!("capture     {:>8}   {:>8}", pct(&mut cap_us, 0.5), pct(&mut cap_us, 0.95));
    println!("diff        {:>8}   {:>8}", pct(&mut hash_us, 0.5), pct(&mut hash_us, 0.95));
    println!("jpeg        {:>8}   {:>8}", pct(&mut jpeg_us, 0.5), pct(&mut jpeg_us, 0.95));
    println!();
    println!("worst case — every tile changed (the keyframe):");
    println!("  {} tiles, {:.0} KiB, diff {}us, jpeg {}us", key.0, key.1 as f64 / 1024.0, key.2, key.3);
    let worst_total = pct(&mut cap_us, 0.5) + key.2 + key.3;
    println!("  capture+diff+jpeg = {}us → {:.1} fps ceiling under continuous full-screen change",
             worst_total, 1_000_000.0 / worst_total.max(1) as f64);
    ExitCode::SUCCESS
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
    // A malformed catalog is a broken image, so it is reported rather than
    // degraded to empty — otherwise every key looks simply absent (§5.5).
    let catalog = match Catalog::load(catalog::DEFAULT_PATH) {
        Ok(c) => {
            println!("catalog: {} app(s) from {}", c.len(), catalog::DEFAULT_PATH);
            c
        }
        Err(e) => {
            eprintln!("catalog: {e}; continuing with none — launch by command still works");
            Catalog::empty()
        }
    };

    #[cfg(feature = "x11")]
    {
        use iapetusd::platform::x11::{X11Display, X11Input};
        match (X11Display::open(), X11Input::open()) {
            (Ok(d), Ok(i)) => {
                println!("platform: X11");
                // One connection, shared: capture and window queries see the
                // same display and fail together if it goes away.
                let display = std::sync::Arc::new(d);
                return Dispatcher::new(
                    FrameSource::new(Box::new(display.clone())),
                    Box::new(i),
                )
                .with_process(Box::new(UnixProcess::new()))
                .with_windows(Box::new(display))
                .with_catalog(catalog);
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
    .with_process(Box::new(UnixProcess::new()))
    .with_catalog(catalog)
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
