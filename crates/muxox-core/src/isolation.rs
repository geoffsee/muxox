// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Geoff Seemueller
// This file is part of muxox, released under the MIT License.

//! Process isolation abstraction.
//!
//! The runtime (`app.rs`) delegates to [`Isolation`] at exactly two points:
//!
//! 1. **Pre-spawn** – [`Isolation::prepare`] configures the command
//!    (process groups, namespaces, sandbox profiles, …)
//! 2. **Teardown** – [`Isolation::terminate`] / [`Isolation::terminate_by_name`]
//!    stops the process tree
//!
//! An optional **post-spawn** hook ([`Isolation::post_spawn`]) runs after a
//! child is successfully created (used for Windows Job Objects).
//!
//! All platform and privilege logic lives behind this boundary so `app.rs`
//! never changes when a new isolation strategy is added.
//!
//! # Strategies
//!
//! | Strategy | Description |
//! |----------|-------------|
//! | [`Sandbox`] | **Default.** Process groups + opt-in process / filesystem / network isolation. |
//! | [`ProcessGroup`] | Legacy strategy: OS process groups only. |

use crate::config::ServiceCfg;
use std::fmt::Debug;
use std::sync::Arc;
use tokio::process::Command as AsyncCommand;

// ---------------------------------------------------------------------------
// Trait
// ---------------------------------------------------------------------------

/// Abstraction over how child processes are sandboxed and torn down.
pub trait Isolation: Send + Sync + Debug {
    /// Apply isolation policy to a command before it is spawned.
    fn prepare(&self, cmd: &mut AsyncCommand, cfg: &ServiceCfg);

    /// Called after a child has been successfully spawned.
    /// Default implementation is a no-op.
    fn post_spawn(&self, _pid: u32, _cfg: &ServiceCfg) {}

    /// Terminate a process tree rooted at the given PID.
    fn terminate(&self, pid: u32);

    /// Fallback: terminate by name when no PID is available.
    fn terminate_by_name(&self, name: &str);
}

/// Returns the default [`Isolation`] strategy for the current platform.
pub fn default_isolation() -> Arc<dyn Isolation> {
    Arc::new(Sandbox)
}

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

#[cfg(unix)]
fn terminate_process_group(pid: u32) {
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

#[cfg(unix)]
fn terminate_by_name_unix(name: &str) {
    let _ = std::process::Command::new("pkill")
        .arg("-f")
        .arg(name)
        .status();
}

#[cfg(windows)]
fn terminate_process_tree_windows(pid: u32) {
    let _ = std::process::Command::new("taskkill")
        .args(["/F", "/T", "/PID"])
        .arg(pid.to_string())
        .status();
}

#[cfg(windows)]
fn terminate_by_name_windows(name: &str) {
    let _ = std::process::Command::new("taskkill")
        .args(["/F", "/T", "/FI"])
        .arg(format!("WINDOWTITLE eq {}", name))
        .status();
}

// ===========================================================================
// Sandbox – default strategy
// ===========================================================================

/// Default strategy: process groups **plus** opt-in isolation.
///
/// When no isolation flags are set in [`ServiceCfg::isolation`], behaviour is
/// identical to the legacy [`ProcessGroup`] strategy.
///
/// | Flag | macOS | Linux | Windows |
/// |------|-------|-------|---------|
/// | `process` | `setsid` | `setsid` | Job Object (`KILL_ON_JOB_CLOSE`) |
/// | `filesystem` | `sandbox_init` (SBPL deny `file-write*`) | `unshare(NEWUSER\|NEWNS)` | *(logged, not yet enforced)* |
/// | `network` | `sandbox_init` (SBPL deny `network*`) | `unshare(NEWUSER\|NEWNET)` | *(logged, not yet enforced)* |
#[derive(Debug)]
pub struct Sandbox;

// ---- macOS Sandbox --------------------------------------------------------

#[cfg(target_os = "macos")]
unsafe extern "C" {
    fn sandbox_init(
        profile: *const std::ffi::c_char,
        flags: u64,
        errorbuf: *mut *mut std::ffi::c_char,
    ) -> std::ffi::c_int;
    fn sandbox_free_error(errorbuf: *mut std::ffi::c_char);
}

#[cfg(target_os = "macos")]
fn build_macos_sandbox_profile(filesystem: bool, network: bool, cwd: &std::path::Path) -> String {
    let cwd_str = cwd.to_string_lossy();
    let mut profile = String::from("(version 1)(allow default)");

    if filesystem {
        profile.push_str("(deny file-write*)");
        profile.push_str(&format!("(allow file-write* (subpath \"{cwd_str}\"))"));
        profile.push_str("(allow file-write* (subpath \"/private/tmp\"))");
        profile.push_str("(allow file-write* (subpath \"/tmp\"))");
        profile.push_str("(allow file-write* (subpath \"/dev\"))");
    }

    if network {
        profile.push_str("(deny network*)");
    }

    profile
}

#[cfg(target_os = "macos")]
impl Isolation for Sandbox {
    fn prepare(&self, cmd: &mut AsyncCommand, cfg: &ServiceCfg) {
        let iso = cfg.isolation.clone();
        let wants_sandbox = iso.filesystem || iso.network;

        // Build the sandbox profile string (and CString) in the parent
        // so the allocation happens before fork().
        let sandbox_profile = if wants_sandbox {
            let cwd = cfg
                .cwd
                .as_ref()
                .and_then(|p| std::fs::canonicalize(p).ok())
                .or_else(|| std::env::current_dir().ok())
                .unwrap_or_default();
            let profile = build_macos_sandbox_profile(iso.filesystem, iso.network, &cwd);
            std::ffi::CString::new(profile).ok()
        } else {
            None
        };

        unsafe {
            cmd.pre_exec(move || {
                if iso.process {
                    // setsid() creates both a new session and process group
                    nix::unistd::setsid()
                        .map_err(|e| std::io::Error::other(format!("setsid: {e}")))?;
                } else {
                    let _ = nix::unistd::setpgid(
                        nix::unistd::Pid::from_raw(0),
                        nix::unistd::Pid::from_raw(0),
                    );
                }

                if let Some(ref profile) = sandbox_profile {
                    let mut errorbuf: *mut std::ffi::c_char = std::ptr::null_mut();
                    let ret = sandbox_init(profile.as_ptr(), 0, &mut errorbuf);
                    if ret != 0 {
                        let msg = if !errorbuf.is_null() {
                            let s = std::ffi::CStr::from_ptr(errorbuf)
                                .to_string_lossy()
                                .into_owned();
                            sandbox_free_error(errorbuf);
                            s
                        } else {
                            "unknown sandbox error".into()
                        };
                        return Err(std::io::Error::new(
                            std::io::ErrorKind::PermissionDenied,
                            format!("sandbox_init: {msg}"),
                        ));
                    }
                }

                Ok(())
            });
        }
    }

    fn terminate(&self, pid: u32) {
        terminate_process_group(pid);
    }

    fn terminate_by_name(&self, name: &str) {
        terminate_by_name_unix(name);
    }
}

// ---- Linux Sandbox --------------------------------------------------------

#[cfg(target_os = "linux")]
impl Isolation for Sandbox {
    fn prepare(&self, cmd: &mut AsyncCommand, cfg: &ServiceCfg) {
        let iso = cfg.isolation.clone();

        unsafe {
            cmd.pre_exec(move || {
                if iso.process {
                    nix::unistd::setsid()
                        .map_err(|e| std::io::Error::other(format!("setsid: {e}")))?;
                } else {
                    let _ = nix::unistd::setpgid(
                        nix::unistd::Pid::from_raw(0),
                        nix::unistd::Pid::from_raw(0),
                    );
                }

                // Namespace-based isolation (requires user namespace support)
                let mut ns_flags = nix::sched::CloneFlags::empty();
                if iso.network {
                    ns_flags |= nix::sched::CloneFlags::CLONE_NEWUSER;
                    ns_flags |= nix::sched::CloneFlags::CLONE_NEWNET;
                }
                if iso.filesystem {
                    ns_flags |= nix::sched::CloneFlags::CLONE_NEWUSER;
                    ns_flags |= nix::sched::CloneFlags::CLONE_NEWNS;
                }
                if !ns_flags.is_empty() {
                    nix::sched::unshare(ns_flags).map_err(|e| {
                        std::io::Error::new(
                            std::io::ErrorKind::PermissionDenied,
                            format!(
                                "unshare({ns_flags:?}): {e} \
                                 (hint: check /proc/sys/kernel/unprivileged_userns_clone)"
                            ),
                        )
                    })?;
                }

                Ok(())
            });
        }
    }

    fn terminate(&self, pid: u32) {
        terminate_process_group(pid);
    }

    fn terminate_by_name(&self, name: &str) {
        terminate_by_name_unix(name);
    }
}

// ---- Windows Sandbox ------------------------------------------------------

#[cfg(windows)]
mod win_job {
    use std::ffi::c_void;

    type HANDLE = *mut c_void;
    type BOOL = i32;
    type DWORD = u32;

    const PROCESS_SET_QUOTA: DWORD = 0x0100;
    const PROCESS_TERMINATE: DWORD = 0x0001;
    const JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE: DWORD = 0x2000;
    const JOB_OBJECT_EXTENDED_LIMIT_INFORMATION: DWORD = 9;

    #[repr(C)]
    struct BasicLimitInfo {
        per_process_user_time_limit: i64,
        per_job_user_time_limit: i64,
        limit_flags: DWORD,
        minimum_working_set_size: usize,
        maximum_working_set_size: usize,
        active_process_limit: DWORD,
        affinity: usize,
        priority_class: DWORD,
        scheduling_class: DWORD,
    }

    #[repr(C)]
    struct IoCounters {
        read_operation_count: u64,
        write_operation_count: u64,
        other_operation_count: u64,
        read_transfer_count: u64,
        write_transfer_count: u64,
        other_transfer_count: u64,
    }

    #[repr(C)]
    struct ExtendedLimitInfo {
        basic: BasicLimitInfo,
        io: IoCounters,
        process_memory_limit: usize,
        job_memory_limit: usize,
        peak_process_memory_used: usize,
        peak_job_memory_used: usize,
    }

    unsafe extern "system" {
        fn CreateJobObjectW(attributes: *const c_void, name: *const u16) -> HANDLE;
        fn SetInformationJobObject(
            job: HANDLE,
            class: DWORD,
            info: *const c_void,
            len: DWORD,
        ) -> BOOL;
        fn AssignProcessToJobObject(job: HANDLE, process: HANDLE) -> BOOL;
        fn OpenProcess(access: DWORD, inherit: BOOL, pid: DWORD) -> HANDLE;
        fn CloseHandle(handle: HANDLE) -> BOOL;
    }

    /// Create a Job Object with `KILL_ON_JOB_CLOSE` and assign the process.
    ///
    /// The job handle is intentionally leaked so it stays alive for the
    /// lifetime of the muxox process.  When muxox exits the handle is closed
    /// and all children in the job are terminated.
    pub fn assign_job_kill_on_close(pid: u32) {
        unsafe {
            let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if job.is_null() {
                return;
            }

            let mut info: ExtendedLimitInfo = std::mem::zeroed();
            info.basic.limit_flags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;

            SetInformationJobObject(
                job,
                JOB_OBJECT_EXTENDED_LIMIT_INFORMATION,
                &info as *const _ as *const c_void,
                std::mem::size_of::<ExtendedLimitInfo>() as DWORD,
            );

            let process = OpenProcess(PROCESS_SET_QUOTA | PROCESS_TERMINATE, 0, pid);
            if !process.is_null() {
                AssignProcessToJobObject(job, process);
                CloseHandle(process);
            }
            // Intentionally do NOT close `job` — see doc comment above.
        }
    }
}

#[cfg(windows)]
impl Isolation for Sandbox {
    fn prepare(&self, cmd: &mut AsyncCommand, cfg: &ServiceCfg) {
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x00000200;
        const CREATE_NEW_CONSOLE: u32 = 0x00000010;
        cmd.creation_flags(CREATE_NEW_PROCESS_GROUP | CREATE_NEW_CONSOLE);

        if cfg.isolation.filesystem {
            eprintln!(
                "[muxox] warning: filesystem isolation is not yet supported on Windows \
                 (service {:?})",
                cfg.name
            );
        }
        if cfg.isolation.network {
            eprintln!(
                "[muxox] warning: network isolation is not yet supported on Windows \
                 (service {:?})",
                cfg.name
            );
        }
    }

    fn post_spawn(&self, pid: u32, cfg: &ServiceCfg) {
        if cfg.isolation.process {
            win_job::assign_job_kill_on_close(pid);
        }
    }

    fn terminate(&self, pid: u32) {
        terminate_process_tree_windows(pid);
    }

    fn terminate_by_name(&self, name: &str) {
        terminate_by_name_windows(name);
    }
}

// ===========================================================================
// ProcessGroup – legacy strategy (kept for backwards compat)
// ===========================================================================

/// Legacy strategy: OS process groups only.
///
/// Ignores [`ServiceCfg::isolation`].  Prefer [`Sandbox`] for new code.
#[derive(Debug)]
pub struct ProcessGroup;

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
        terminate_process_group(pid);
    }

    fn terminate_by_name(&self, name: &str) {
        terminate_by_name_unix(name);
    }
}

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
        terminate_process_group(pid);
    }

    fn terminate_by_name(&self, name: &str) {
        terminate_by_name_unix(name);
    }
}

#[cfg(windows)]
impl Isolation for ProcessGroup {
    fn prepare(&self, cmd: &mut AsyncCommand, _cfg: &ServiceCfg) {
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x00000200;
        const CREATE_NEW_CONSOLE: u32 = 0x00000010;
        cmd.creation_flags(CREATE_NEW_PROCESS_GROUP | CREATE_NEW_CONSOLE);
    }

    fn terminate(&self, pid: u32) {
        terminate_process_tree_windows(pid);
    }

    fn terminate_by_name(&self, name: &str) {
        terminate_by_name_windows(name);
    }
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::IsolationCfg;

    #[test]
    fn default_isolation_returns_sandbox() {
        let iso = default_isolation();
        assert!(format!("{:?}", iso).contains("Sandbox"));
    }

    #[test]
    fn sandbox_debug_format() {
        let s = Sandbox;
        assert_eq!(format!("{s:?}"), "Sandbox");
    }

    #[test]
    fn process_group_debug_format() {
        let pg = ProcessGroup;
        assert_eq!(format!("{pg:?}"), "ProcessGroup");
    }

    #[test]
    fn isolation_cfg_default_is_no_isolation() {
        let cfg = IsolationCfg::default();
        assert!(!cfg.process);
        assert!(!cfg.filesystem);
        assert!(!cfg.network);
    }

    #[test]
    fn sandbox_is_send_and_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<Sandbox>();
    }

    #[test]
    fn process_group_is_send_and_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<ProcessGroup>();
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_sandbox_profile_network_only() {
        let profile = build_macos_sandbox_profile(false, true, std::path::Path::new("/tmp/test"));
        assert!(profile.contains("(deny network*)"));
        assert!(!profile.contains("(deny file-write*)"));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_sandbox_profile_filesystem_only() {
        let profile = build_macos_sandbox_profile(true, false, std::path::Path::new("/tmp/test"));
        assert!(profile.contains("(deny file-write*)"));
        assert!(profile.contains("(allow file-write* (subpath \"/tmp/test\"))"));
        assert!(!profile.contains("(deny network*)"));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_sandbox_profile_both() {
        let profile =
            build_macos_sandbox_profile(true, true, std::path::Path::new("/home/user/project"));
        assert!(profile.contains("(deny file-write*)"));
        assert!(profile.contains("(deny network*)"));
        assert!(profile.contains("(allow file-write* (subpath \"/home/user/project\"))"));
    }
}
