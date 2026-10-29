// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Geoff Seemueller
// This file is part of muxox, released under the MIT License.

use crate::config::{HooksCfg, ServiceCfg};
use crate::hooks::{HookPhase, run_hook_async, run_hook_sync};
use crate::isolation::Isolation;
#[cfg(unix)]
use crate::utils::{interactive_args, interactive_program};
use crate::utils::{shell_exec, shell_flag, shell_program};
use serde::Serialize;
use std::collections::VecDeque;
use std::path::PathBuf;
use std::process::{Child, Stdio};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::AsyncReadExt;
use tokio::process::{ChildStdin, Command as AsyncCommand};
use tokio::sync::{Mutex, mpsc};
use tokio::task;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum Status {
    Stopped,
    Starting,
    Running,
    Stopping,
}

impl Status {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Stopped => "Stopped",
            Self::Starting => "Starting",
            Self::Running => "Running",
            Self::Stopping => "Stopping",
        }
    }
}

#[derive(Debug)]
pub struct ServiceState {
    pub cfg: ServiceCfg,
    pub status: Status,
    pub child: Option<Child>,
    pub pid: Option<u32>,
    pub log: VecDeque<String>,
    pub stdin_tx: Option<mpsc::Sender<String>>,
    pub stdin_writer: Option<Arc<Mutex<ChildStdin>>>,
}

impl ServiceState {
    pub fn new(cfg: ServiceCfg) -> Self {
        Self {
            log: VecDeque::with_capacity(cfg.log_capacity.max(256)),
            cfg,
            status: Status::Stopped,
            child: None,
            pid: None,
            stdin_tx: None,
            stdin_writer: None,
        }
    }
    pub fn push_log(&mut self, line: impl Into<String>) {
        if self.log.len() == self.cfg.log_capacity {
            self.log.pop_front();
        }
        self.log.push_back(line.into());
    }
}

pub struct App {
    pub update_notice: Option<crate::updates::UpdateNotice>,
    pub services: Vec<ServiceState>,
    pub selected: usize,
    // Number of lines from the end of the log to offset when rendering.
    // 0 means follow the tail; higher values scroll up into older logs.
    pub log_offset_from_end: u16,
    pub tx: mpsc::UnboundedSender<AppMsg>,
    // Input mode for interactive services
    pub input_mode: bool,
    pub input_buffer: String,
    pub isolation: Arc<dyn Isolation>,
    /// Global lifecycle hooks from `[hooks]` in muxox.toml.
    pub hooks: HooksCfg,
    /// Directory of the loaded config; global hooks run with this as cwd.
    pub config_dir: Option<PathBuf>,
    /// Optional settle notifications for [`start_all`] boot coordination.
    pub settle_tx: Option<mpsc::UnboundedSender<usize>>,
}

impl std::fmt::Debug for App {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("App")
            .field("services", &self.services)
            .field("selected", &self.selected)
            .field("log_offset_from_end", &self.log_offset_from_end)
            .field("input_mode", &self.input_mode)
            .field("input_buffer", &self.input_buffer)
            .field("isolation", &self.isolation)
            .field("hooks", &self.hooks)
            .field("config_dir", &self.config_dir)
            .finish()
    }
}

#[derive(Debug, Clone)]
pub enum AppMsg {
    UpdateAvailable(crate::updates::UpdateNotice),
    Started(usize),
    Stopped(usize, i32),
    Log(usize, String),
    ChildSpawned(usize, u32),
    StdinReady(usize, mpsc::Sender<String>),
    StdinWriterReady(usize, Arc<Mutex<ChildStdin>>),
    /// Start attempt finished (after before_start + spawn + after_start).
    SpawnSettled(usize),
    AbortedAll,
}

/// Mirror a global hook line to stderr and every service log buffer so web/TUI
/// panes show `[hooks]` output (global hooks have no dedicated pane).
fn log_global_hook(app: &mut App, line: String) {
    eprintln!("[hooks] {line}");
    for svc in &mut app.services {
        svc.push_log(format!("[hooks] {line}"));
    }
}

fn run_service_before_stop(idx: usize, app: &mut App) {
    let Some(cmd) = app.services[idx].cfg.hooks.before_stop.clone() else {
        return;
    };
    let cwd = app.services[idx].cfg.cwd.clone();
    let env_file = app.services[idx].cfg.env_file.clone();
    let tx = app.tx.clone();
    run_hook_sync(
        HookPhase::BeforeStop,
        &cmd,
        cwd.as_deref(),
        env_file.as_deref(),
        |line| {
            // Route through AppMsg so web/raw can live-broadcast; apply_msg
            // pushes into the service buffer when the mode loop drains it.
            let _ = tx.send(AppMsg::Log(idx, line));
        },
    );
}

pub fn start_service(idx: usize, app: &mut App) {
    if matches!(app.services[idx].status, Status::Running | Status::Starting) {
        return;
    }
    crate::log::debug(&format!(
        "start_service idx={} name={}",
        idx, app.services[idx].cfg.name
    ));
    app.services[idx].status = Status::Starting;
    let tx = app.tx.clone();
    let settle_tx = app.settle_tx.clone();
    let sc = app.services[idx].cfg.clone();
    let isolation = app.isolation.clone();

    task::spawn(async move {
        let notify_settled = || {
            if let Some(ref stx) = settle_tx {
                let _ = stx.send(idx);
            }
            let _ = tx.send(AppMsg::SpawnSettled(idx));
        };

        if let Some(ref hook) = sc.hooks.before_start {
            let tx_log = tx.clone();
            run_hook_async(
                HookPhase::BeforeStart,
                hook,
                sc.cwd.as_deref(),
                sc.env_file.as_deref(),
                |line| {
                    let _ = tx_log.send(AppMsg::Log(idx, line));
                },
            )
            .await;
        }

        // Build command; optionally wrap interactive with a PTY when requested
        #[cfg(unix)]
        let mut cmd = {
            if sc.interactive && sc.pty {
                let mut c = AsyncCommand::new(interactive_program());
                for a in interactive_args(&sc.cmd) {
                    c.arg(a);
                }
                let _ = tx.send(AppMsg::Log(
                    idx,
                    "[DEBUG] Launching interactive via PTY wrapper (script)".to_string(),
                ));
                c
            } else {
                let mut c = AsyncCommand::new(shell_program());
                c.arg(shell_flag()).arg(shell_exec(&sc.cmd));
                c
            }
        };
        #[cfg(not(unix))]
        let mut cmd = {
            let mut c = AsyncCommand::new(shell_program());
            c.arg(shell_flag()).arg(shell_exec(&sc.cmd));
            c
        };
        if let Some(cwd) = sc.cwd.clone() {
            cmd.current_dir(cwd);
        }
        cmd.env("FORCE_COLOR", "1")
            .env("CLICOLOR_FORCE", "1")
            .env("TERM", "xterm-256color")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        // Apply service-scoped environment variables from .env file
        if let Some(ref env_path) = sc.env_file {
            match crate::config::parse_env_file(env_path) {
                Ok(vars) => {
                    cmd.envs(vars);
                }
                Err(e) => {
                    let _ = tx.send(AppMsg::Log(
                        idx,
                        format!("[ERROR] Failed to load env file: {e}"),
                    ));
                }
            }
        }

        // Interactive services need stdin piped so we can send input
        if sc.interactive {
            cmd.stdin(Stdio::piped());
        }

        isolation.prepare(&mut cmd, &sc);

        crate::log::debug(&format!("[task idx={idx}] spawning command: {:?}", sc.cmd));
        match cmd.spawn() {
            Ok(mut child) => {
                let pid = child.id().unwrap_or_default();
                crate::log::debug(&format!("[task idx={idx}] spawned pid={pid}"));
                isolation.post_spawn(pid, &sc);

                if let Some(ref hook) = sc.hooks.after_start {
                    let tx_log = tx.clone();
                    run_hook_async(
                        HookPhase::AfterStart,
                        hook,
                        sc.cwd.as_deref(),
                        sc.env_file.as_deref(),
                        |line| {
                            let _ = tx_log.send(AppMsg::Log(idx, line));
                        },
                    )
                    .await;
                }

                let _ = tx.send(AppMsg::Started(idx));
                let _ = tx.send(AppMsg::ChildSpawned(idx, pid));
                notify_settled();

                // For interactive services, setup stdin forwarding
                if sc.interactive {
                    let _ = tx.send(AppMsg::Log(
                        idx,
                        "[DEBUG] Service is interactive, setting up stdin forwarding".to_string(),
                    ));
                    if let Some(stdin) = child.stdin.take() {
                        let _ = tx.send(AppMsg::Log(
                            idx,
                            "[DEBUG] child.stdin is available".to_string(),
                        ));
                        // Publish a direct-writer handle (Arc<Mutex<ChildStdin>>) back to the app
                        let writer = Arc::new(Mutex::new(stdin));
                        let _ = tx.send(AppMsg::StdinWriterReady(idx, writer));
                        let _ = tx.send(AppMsg::Log(
                            idx,
                            "[DEBUG] StdinWriterReady message sent".to_string(),
                        ));
                    }
                }

                // stdout
                if let Some(out) = child.stdout.take() {
                    let tx2 = tx.clone();
                    let is_pty = sc.pty;
                    task::spawn(async move {
                        stream_log_realtime(idx, out, tx2, false, is_pty).await;
                    });
                }
                // stderr
                if let Some(err) = child.stderr.take() {
                    let tx2 = tx.clone();
                    let is_pty = sc.pty;
                    task::spawn(async move {
                        stream_log_realtime(idx, err, tx2, true, is_pty).await;
                    });
                }

                // Waiter
                let status = child.wait().await; // process exit
                let code = status.map(|s| s.code().unwrap_or(-1)).unwrap_or(-1);

                if let Some(ref hook) = sc.hooks.after_stop {
                    let tx_log = tx.clone();
                    run_hook_async(
                        HookPhase::AfterStop,
                        hook,
                        sc.cwd.as_deref(),
                        sc.env_file.as_deref(),
                        |line| {
                            let _ = tx_log.send(AppMsg::Log(idx, line));
                        },
                    )
                    .await;
                }

                let _ = tx.send(AppMsg::Stopped(idx, code));
            }
            Err(e) => {
                crate::log::debug(&format!("[task idx={idx}] spawn FAILED: {e}"));
                let _ = tx.send(AppMsg::Log(idx, format!("spawn failed: {e}")));
                notify_settled();
                let _ = tx.send(AppMsg::Stopped(idx, -1));
            }
        }
    });
}

async fn stream_log_realtime<R>(
    idx: usize,
    stream: R,
    tx: mpsc::UnboundedSender<AppMsg>,
    _is_stderr: bool,
    is_pty: bool,
) where
    R: tokio::io::AsyncRead + Unpin + Send + 'static,
{
    let mut reader = tokio::io::BufReader::new(stream);
    let mut buf = vec![0u8; 4096];

    loop {
        match reader.read(&mut buf).await {
            Ok(0) => break, // EOF
            Ok(n) => {
                let s = String::from_utf8_lossy(&buf[..n]);
                // On PTY or certain shells, output might come with CR/LF or just LF.
                // We split into lines but need to handle potential carriage returns.
                for line in s.split('\n') {
                    let clean = line.trim_end_matches('\r');
                    if !clean.is_empty() || is_pty {
                        let _ = tx.send(AppMsg::Log(idx, clean.to_string()));
                    }
                }
            }
            Err(_) => break,
        }
    }
}

pub fn stop_service(idx: usize, app: &mut App) {
    if !matches!(app.services[idx].status, Status::Running | Status::Starting) {
        return;
    }
    app.services[idx].status = Status::Stopping;
    run_service_before_stop(idx, app);
    kill_tree(idx, app);
}

pub fn apply_msg(app: &mut App, msg: AppMsg) {
    match msg {
        AppMsg::UpdateAvailable(notice) => app.update_notice = Some(notice),
        AppMsg::Started(idx) => {
            crate::log::debug(&format!("apply_msg: Started idx={idx}"));
            app.services[idx].status = Status::Running;
        }
        AppMsg::Stopped(idx, _code) => {
            crate::log::debug(&format!("apply_msg: Stopped idx={idx} code={_code}"));
            app.services[idx].status = Status::Stopped;
            app.services[idx].child = None;
            app.services[idx].pid = None;
            app.services[idx].stdin_tx = None;
            app.services[idx].stdin_writer = None;
        }
        AppMsg::Log(idx, line) => {
            crate::log::debug(&format!("apply_msg: Log idx={idx} len={}", line.len()));
            app.services[idx].push_log(line);
        }
        AppMsg::ChildSpawned(idx, pid) => {
            app.services[idx].pid = Some(pid);
        }
        AppMsg::StdinReady(idx, tx) => {
            app.services[idx].stdin_tx = Some(tx);
        }
        AppMsg::StdinWriterReady(idx, writer) => {
            app.services[idx].stdin_writer = Some(writer);
        }
        AppMsg::SpawnSettled(_) => {
            // Handled by start_all via settle_tx; no App state change.
        }
        AppMsg::AbortedAll => {
            // Handled by the main loop
        }
    }
}

/// Run global `before_start`, start every service, wait until each start
/// attempt has settled, then run global `after_start`.
pub async fn start_all(app: &mut App) {
    let hook_cwd = app.config_dir.clone();
    if let Some(ref hook) = app.hooks.before_start.clone() {
        run_hook_sync(
            HookPhase::BeforeStart,
            hook,
            hook_cwd.as_deref(),
            None,
            |line| {
                log_global_hook(app, line);
            },
        );
    }

    let n = app.services.len();
    let (settle_tx, mut settle_rx) = mpsc::unbounded_channel();
    app.settle_tx = Some(settle_tx);

    for idx in 0..n {
        start_service(idx, app);
    }

    for _ in 0..n {
        if settle_rx.recv().await.is_none() {
            break;
        }
    }
    app.settle_tx = None;

    if let Some(ref hook) = app.hooks.after_start.clone() {
        run_hook_sync(
            HookPhase::AfterStart,
            hook,
            hook_cwd.as_deref(),
            None,
            |line| {
                log_global_hook(app, line);
            },
        );
    }
}

/// Apply any `AppMsg` values already queued on `rx` without waiting.
/// Call after [`start_all`] so boot-time service hook / spawn logs land in
/// buffers before web clients connect or the TUI takes over the terminal.
pub fn drain_pending(app: &mut App, rx: &mut mpsc::UnboundedReceiver<AppMsg>) {
    while let Ok(msg) = rx.try_recv() {
        apply_msg(app, msg);
    }
}

/// Run global `before_stop`, per-service `before_stop` + kill, brief wait,
/// then global `after_stop`.
pub fn shutdown(app: &mut App) {
    let hook_cwd = app.config_dir.clone();
    if let Some(ref hook) = app.hooks.before_stop.clone() {
        run_hook_sync(
            HookPhase::BeforeStop,
            hook,
            hook_cwd.as_deref(),
            None,
            |line| {
                log_global_hook(app, line);
            },
        );
    }

    for i in 0..app.services.len() {
        // Skipping `Stopping`: stop_service already ran before_stop.
        if matches!(app.services[i].status, Status::Running | Status::Starting) {
            run_service_before_stop(i, app);
        }
        kill_tree(i, app);
    }

    std::thread::sleep(Duration::from_millis(200));

    if let Some(ref hook) = app.hooks.after_stop.clone() {
        run_hook_sync(
            HookPhase::AfterStop,
            hook,
            hook_cwd.as_deref(),
            None,
            |line| {
                log_global_hook(app, line);
            },
        );
    }
}

pub fn kill_all(app: &mut App) {
    for i in 0..app.services.len() {
        kill_tree(i, app);
    }
}

pub fn kill_tree(idx: usize, app: &mut App) {
    let name = app.services[idx].cfg.name.clone();

    if let Some(pid) = app.services[idx].pid {
        app.isolation.terminate(pid);
        app.services[idx].push_log(format!("[killed {name} (PID {pid})]"));
    } else {
        app.isolation.terminate_by_name(&name);
        app.services[idx].push_log(format!("[killed {name}]"));
    }
}

pub fn cleanup_and_exit(app: &mut App) {
    shutdown(app);
    // Reset terminal if TUI was used (handled in run_tui_mode usually)
    std::process::exit(0);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ServiceCfg;

    #[test]
    fn test_service_state() {
        let mut state = ServiceState::new(ServiceCfg {
            name: "test".into(),
            cmd: "ls".into(),
            cwd: None,
            log_capacity: 10,
            interactive: false,
            pty: false,
            env_file: None,
            isolation: Default::default(),
            hooks: Default::default(),
        });

        assert_eq!(state.status, Status::Stopped);
        state.push_log("line 1");
        state.push_log("line 2");
        assert_eq!(state.log.len(), 2);
        assert_eq!(state.log.front().unwrap(), "line 1");

        for i in 3..=12 {
            state.push_log(format!("line {}", i));
        }

        // Should have removed oldest entries to stay within capacity
        assert_eq!(state.log.len(), 10);
        assert_eq!(state.log.front().unwrap(), "line 3");
        assert_eq!(state.log.back().unwrap(), "line 12");
    }
}
