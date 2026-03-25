// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Geoff Seemueller
// This file is part of muxox, released under the MIT License.

//! Process isolation abstraction.
//!
//! The runtime (`app.rs`) delegates to [`Isolation`] at exactly two points:
//!
//! 1. **Pre-spawn** – [`Isolation::prepare`] configures the command
//!    (process groups, uid/gid, namespaces, capabilities, …)
//! 2. **Teardown** – [`Isolation::terminate`] / [`Isolation::terminate_by_name`]
//!    stops the process tree
//!
//! All platform and privilege logic lives behind this boundary so `app.rs`
//! never changes when a new isolation strategy is added.

use crate::config::ServiceCfg;
use std::fmt::Debug;
use std::sync::Arc;
use tokio::process::Command as AsyncCommand;

/// Abstraction over how child processes are sandboxed and torn down.
pub trait Isolation: Send + Sync + Debug {
    /// Apply isolation policy to a command before it is spawned.
    fn prepare(&self, cmd: &mut AsyncCommand, cfg: &ServiceCfg);

    /// Terminate a process tree rooted at the given PID.
    fn terminate(&self, pid: u32);

    /// Fallback: terminate by name when no PID is available.
    fn terminate_by_name(&self, name: &str);
}

/// Returns the default [`Isolation`] strategy for the current platform.
pub fn default_isolation() -> Arc<dyn Isolation> {
    Arc::new(ProcessGroup)
}

// ---------------------------------------------------------------------------
// ProcessGroup – current behaviour extracted into the trait
// ---------------------------------------------------------------------------

/// Default strategy: OS process groups.
///
/// | Platform | Spawn | Teardown |
/// |----------|-------|----------|
/// | macOS | `setpgid` | `killpg(TERM)` → 250 ms → `killpg(KILL)` |
/// | Linux | `setpgid` | `killpg(TERM)` → 250 ms → `killpg(KILL)` |
/// | Windows | `CREATE_NEW_PROCESS_GROUP` | `taskkill /F /T /PID` |
#[derive(Debug)]
pub struct ProcessGroup;

// ---- macOS ----------------------------------------------------------------

#[cfg(target_os = "macos")]
impl Isolation for ProcessGroup {
    fn prepare(&self, cmd: &mut AsyncCommand, _cfg: &ServiceCfg) {
        unsafe {
            cmd.pre_exec(|| {
                let _ = nix::unistd::setpgid(
                    nix::unistd::Pid::from_raw(0),
                    nix::unistd::Pid::from_raw(0),
                );
                Ok(())
            });
        }
    }

    fn terminate(&self, pid: u32) {
        use nix::sys::signal::{Signal, killpg};
        use nix::unistd::Pid;
        use std::time::Duration;

        let pgid = Pid::from_raw(pid as i32);

        // Graceful: SIGTERM the process group (fall back to the process itself)
        if killpg(pgid, Signal::SIGTERM).is_err() {
            let _ = nix::sys::signal::kill(pgid, Signal::SIGTERM);
        }

        std::thread::sleep(Duration::from_millis(250));

        // Forceful: SIGKILL
        if killpg(pgid, Signal::SIGKILL).is_err() {
            let _ = nix::sys::signal::kill(pgid, Signal::SIGKILL);
        }
    }

    fn terminate_by_name(&self, name: &str) {
        let _ = std::process::Command::new("pkill")
            .arg("-f")
            .arg(name)
            .status();
    }
}

// ---- Linux ----------------------------------------------------------------

#[cfg(target_os = "linux")]
impl Isolation for ProcessGroup {
    fn prepare(&self, cmd: &mut AsyncCommand, _cfg: &ServiceCfg) {
        unsafe {
            cmd.pre_exec(|| {
                let _ = nix::unistd::setpgid(
                    nix::unistd::Pid::from_raw(0),
                    nix::unistd::Pid::from_raw(0),
                );
                Ok(())
            });
        }
    }

    fn terminate(&self, pid: u32) {
        use nix::sys::signal::{Signal, killpg};
        use nix::unistd::Pid;
        use std::time::Duration;

        let pgid = Pid::from_raw(pid as i32);

        if killpg(pgid, Signal::SIGTERM).is_err() {
            let _ = nix::sys::signal::kill(pgid, Signal::SIGTERM);
        }

        std::thread::sleep(Duration::from_millis(250));

        if killpg(pgid, Signal::SIGKILL).is_err() {
            let _ = nix::sys::signal::kill(pgid, Signal::SIGKILL);
        }
    }

    fn terminate_by_name(&self, name: &str) {
        let _ = std::process::Command::new("pkill")
            .arg("-f")
            .arg(name)
            .status();
    }
}

// ---- Windows --------------------------------------------------------------

#[cfg(windows)]
impl Isolation for ProcessGroup {
    fn prepare(&self, cmd: &mut AsyncCommand, _cfg: &ServiceCfg) {
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x00000200;
        const CREATE_NEW_CONSOLE: u32 = 0x00000010;
        cmd.creation_flags(CREATE_NEW_PROCESS_GROUP | CREATE_NEW_CONSOLE);
    }

    fn terminate(&self, pid: u32) {
        let _ = std::process::Command::new("taskkill")
            .args(["/F", "/T", "/PID"])
            .arg(pid.to_string())
            .status();
    }

    fn terminate_by_name(&self, name: &str) {
        let _ = std::process::Command::new("taskkill")
            .args(["/F", "/T", "/FI"])
            .arg(format!("WINDOWTITLE eq {}", name))
            .status();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_isolation_returns_process_group() {
        let iso = default_isolation();
        assert!(format!("{:?}", iso).contains("ProcessGroup"));
    }
}
