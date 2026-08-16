//! Process spawning on Unix.
//!
//! §7.3 puts the agent in OWNER mode, so there is nothing to sandbox here. What
//! this module owes the caller is an honest pid and a clean detachment: a GUI
//! program launched by an agent must outlive the action that started it, and
//! must not become a zombie the daemon never reaps.

use std::os::unix::process::CommandExt as _;
use std::process::{Command, Stdio};

use std::io::Read;
use std::time::{Duration, Instant};

use super::{LaunchSpec, PlatformError, Process, Result, ShellOutput};

pub struct UnixProcess;

impl UnixProcess {
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

impl Default for UnixProcess {
    fn default() -> Self {
        Self::new()
    }
}

impl Process for UnixProcess {
    fn launch(&self, spec: &LaunchSpec) -> Result<u32> {
        if spec.command.trim().is_empty() {
            return Err(PlatformError::InputRejected("no command to launch".into()));
        }

        let mut cmd = if spec.elevated {
            // `-n` so a sudo that would prompt fails immediately instead of
            // blocking the action queue on a password nobody can type.
            let mut c = Command::new("sudo");
            c.arg("-n").arg("--").arg(&spec.command);
            c
        } else {
            Command::new(&spec.command)
        };

        cmd.args(&spec.args);
        if let Some(dir) = &spec.cwd {
            cmd.current_dir(dir);
        }

        // Inherit DISPLAY and the rest of the session environment — a GUI
        // program started without it opens nothing and reports no error.
        // stdout and stderr go nowhere: a chatty program would otherwise fill
        // the daemon's pipe buffer and block on a write nobody is reading.
        cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());

        let child = cmd.spawn().map_err(|e| {
            PlatformError::InputRejected(format!("could not launch {}: {e}", spec.command))
        })?;

        let pid = child.id();
        // Dropping the handle without waiting leaves a zombie until the daemon
        // exits. Reaping in a detached thread keeps the process table clean
        // without making the caller wait for a program that never exits.
        std::thread::spawn(move || {
            let mut child = child;
            let _ = child.wait();
        });

        Ok(pid)
    }

    fn run(&self, spec: &LaunchSpec, timeout: Duration, cap: usize) -> Result<ShellOutput> {
        if spec.command.trim().is_empty() {
            return Err(PlatformError::InputRejected("no command to run".into()));
        }

        // Through `sh -c`, because §7.2's `shell.exec` takes a command line —
        // pipes, redirections, and `&&` — not an argv. `app.launch` is the argv
        // path; conflating them would make one of the two surprising.
        let mut cmd = if spec.elevated {
            let mut c = Command::new("sudo");
            c.arg("-n").arg("--").arg("sh").arg("-c").arg(&spec.command);
            c
        } else {
            let mut c = Command::new("sh");
            c.arg("-c").arg(&spec.command);
            c
        };
        if let Some(dir) = &spec.cwd {
            cmd.current_dir(dir);
        }
        cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());

        // Put the shell in its own process group. `sh -c "sleep 30"` forks the
        // sleep, and killing only the shell would leave the sleep holding the
        // stdout pipe open — the reader never sees EOF and the whole thing hangs
        // past its deadline. Killing the group takes the children too.
        //
        // SAFETY: setsid in the child before exec sets no shared state and only
        // detaches the new process into its own session and group.
        unsafe {
            cmd.pre_exec(|| {
                if libc::setsid() == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }

        let mut child = cmd
            .spawn()
            .map_err(|e| PlatformError::InputRejected(format!("could not run {}: {e}", spec.command)))?;

        // Read the pipes on threads so a program that fills stderr while we wait
        // on stdout cannot deadlock against a full pipe buffer.
        let mut out = child.stdout.take().map(reader_thread);
        let mut err = child.stderr.take().map(reader_thread);

        let deadline = Instant::now() + timeout;
        let mut timed_out = false;
        loop {
            match child.try_wait() {
                Ok(Some(_)) => break,
                Ok(None) => {
                    if Instant::now() >= deadline {
                        // Signal the whole group (negative pid), so the forked
                        // children die with the shell rather than orphaning and
                        // keeping the pipes open.
                        //
                        // SAFETY: a kill with a signal number and a pid; the pid
                        // is this child's, still live because we have not reaped
                        // it. setsid above made its pid the group id.
                        unsafe {
                            libc::kill(-(child.id() as i32), libc::SIGKILL);
                        }
                        let _ = child.wait();
                        timed_out = true;
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(e) => return Err(PlatformError::CaptureFailed(format!("waiting on the child: {e}"))),
            }
        }

        let status = child.wait().ok();
        let (mut stdout, t1) = out.take().map(|h| h.join().unwrap_or_default()).unwrap_or_default();
        let (mut stderr, t2) = err.take().map(|h| h.join().unwrap_or_default()).unwrap_or_default();

        let mut truncated = t1 || t2;
        if stdout.len() > cap {
            stdout.truncate(cap);
            truncated = true;
        }
        if stderr.len() > cap {
            stderr.truncate(cap);
            truncated = true;
        }

        // On a timeout the exit status is the SIGKILL we sent, which is not the
        // program's own. Reporting 124 — the timeout(1) convention — tells the
        // agent it was stopped rather than that it failed with a wild code.
        let exit_code = if timed_out {
            124
        } else {
            status.and_then(|s| s.code()).unwrap_or(-1)
        };

        Ok(ShellOutput { exit_code, stdout, stderr, truncated, timed_out })
    }
}

/// Reads a pipe to EOF on its own thread, stopping the capture once it is
/// hopelessly large so a runaway process cannot exhaust memory before the
/// caller's own cap is applied. Returns the bytes and whether it was cut.
fn reader_thread<R: Read + Send + 'static>(
    mut r: R,
) -> std::thread::JoinHandle<(Vec<u8>, bool)> {
    // A generous ceiling well above §8.2's 1MB cap: the caller truncates to the
    // real limit, and this only stops a `yes`-style flood from growing without
    // bound while it waits.
    const HARD_CEIL: usize = 16 * 1024 * 1024;
    std::thread::spawn(move || {
        let mut buf = Vec::new();
        let mut chunk = [0u8; 64 * 1024];
        loop {
            match r.read(&mut chunk) {
                Ok(0) => break,
                Ok(n) => {
                    if buf.len() < HARD_CEIL {
                        buf.extend_from_slice(&chunk[..n.min(HARD_CEIL - buf.len())]);
                    } else {
                        // Keep draining so the child does not block on a full
                        // pipe, but stop growing the buffer.
                        return (buf, true);
                    }
                }
                Err(_) => break,
            }
        }
        (buf, false)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(command: &str, args: &[&str]) -> LaunchSpec {
        LaunchSpec {
            command: command.into(),
            args: args.iter().map(|s| (*s).to_string()).collect(),
            cwd: None,
            elevated: false,
        }
    }

    #[test]
    fn a_launch_returns_a_pid_that_really_exists() {
        let p = UnixProcess::new();
        let pid = p.launch(&spec("/bin/sleep", &["5"])).expect("sleep did not start");
        assert!(pid > 0);

        // `kill -0` asks whether the process exists without signalling it. A
        // fabricated pid would pass a test that only checked for non-zero.
        let alive = Command::new("kill").arg("-0").arg(pid.to_string()).status();
        assert!(alive.map(|s| s.success()).unwrap_or(false), "pid {pid} does not exist");

        let _ = Command::new("kill").arg("-9").arg(pid.to_string()).status();
    }

    #[test]
    fn a_missing_executable_fails_rather_than_reporting_success() {
        // Reporting a pid for a program that never started would have the agent
        // wait for a window that is never coming.
        let p = UnixProcess::new();
        assert!(p.launch(&spec("/nonexistent/program", &[])).is_err());
    }

    #[test]
    fn an_empty_command_is_refused() {
        let p = UnixProcess::new();
        assert!(p.launch(&spec("   ", &[])).is_err());
    }

    #[test]
    fn cwd_is_applied() {
        let p = UnixProcess::new();
        let dir = std::env::temp_dir().join("iapetus-cwd-test");
        std::fs::create_dir_all(&dir).unwrap();
        let marker = dir.join("touched");
        let _ = std::fs::remove_file(&marker);

        let mut s = spec("/usr/bin/touch", &["touched"]);
        s.cwd = Some(dir.to_string_lossy().into_owned());
        if p.launch(&s).is_err() {
            return; // no /usr/bin/touch on this host; nothing to assert
        }

        for _ in 0..50 {
            if marker.exists() {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        panic!("touch did not run in the requested directory");
    }
}
