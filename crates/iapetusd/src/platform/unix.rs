//! Process spawning on Unix.
//!
//! §7.3 puts the agent in OWNER mode, so there is nothing to sandbox here. What
//! this module owes the caller is an honest pid and a clean detachment: a GUI
//! program launched by an agent must outlive the action that started it, and
//! must not become a zombie the daemon never reaps.

use std::process::{Command, Stdio};

use super::{LaunchSpec, PlatformError, Process, Result};

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
