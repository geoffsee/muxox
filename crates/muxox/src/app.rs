// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Geoff Seemueller
// This file is part of muxox, released under the MIT License.

use crate::config::ServiceCfg;
use crate::utils::{
    interactive_args, interactive_program, set_process_group, shell_flag, shell_program, shell_exec,
};
use serde::Serialize;
use std::collections::VecDeque;
use std::process::{Child, Command, Stdio};
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

#[derive(Debug)]
pub struct App {
    pub services: Vec<ServiceState>,
    pub selected: usize,
    // Number of lines from the end of the log to offset when rendering.
    // 0 means follow the tail; higher values scroll up into older logs.
    pub log_offset_from_end: u16,
    pub tx: mpsc::UnboundedSender<AppMsg>,
    // Input mode for interactive services
    pub input_mode: bool,
    pub input_buffer: String,
}

#[derive(Debug, Clone)]
pub enum AppMsg {
    Started(usize),
    Stopped(usize, i32),
    Log(usize, String),
    ChildSpawned(usize, u32),
    StdinReady(usize, mpsc::Sender<String>),
    StdinWriterReady(usize, Arc<Mutex<ChildStdin>>),
    AbortedAll,
}

pub fn start_service(idx: usize, app: &mut App) {
    if matches!(app.services[idx].status, Status::Running | Status::Starting) {
        return;
    }
    app.services[idx].status = Status::Starting;
    let tx = app.tx.clone();
    let sc = app.services[idx].cfg.clone();

    task::spawn(async move {
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

        // Interactive services need stdin piped so we can send input
        if sc.interactive {
            cmd.stdin(Stdio::piped());
        }

        set_process_group(&mut cmd);

        match cmd.spawn() {
            Ok(mut child) => {
                let pid = child.id().unwrap_or_default();
                let _ = tx.send(AppMsg::Started(idx));

                // Send the child handle back to be stored
                let _ = tx.send(AppMsg::ChildSpawned(idx, pid));

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
                let _ = tx.send(AppMsg::Stopped(idx, code));
            }
            Err(e) => {
                let _ = tx.send(AppMsg::Log(idx, format!("spawn failed: {e}")));
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
    kill_tree(idx, app);
}

pub fn apply_msg(app: &mut App, msg: AppMsg) {
    match msg {
        AppMsg::Started(idx) => {
            app.services[idx].status = Status::Running;
        }
        AppMsg::Stopped(idx, _code) => {
            app.services[idx].status = Status::Stopped;
            app.services[idx].child = None;
            app.services[idx].pid = None;
            app.services[idx].stdin_tx = None;
            app.services[idx].stdin_writer = None;
        }
        AppMsg::Log(idx, line) => {
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
        AppMsg::AbortedAll => {
            // Handled by the main loop
        }
    }
}

pub fn kill_all(app: &mut App) {
    for i in 0..app.services.len() {
        kill_tree(i, app);
    }
}

#[cfg(unix)]
pub fn kill_tree(idx: usize, app: &mut App) {
    use nix::sys::signal::{Signal, killpg};
    use nix::unistd::Pid;
    let name = app.services[idx].cfg.name.clone();

    if let Some(pid) = app.services[idx].pid {
        // Kill the specific process group using the stored PID
        let pgid = Pid::from_raw(pid as i32);

        // First try TERM signal to allow graceful shutdown
        if let Err(_) = killpg(pgid, Signal::SIGTERM) {
            // If killpg fails (process group doesn't exist), try killing the process directly
            let _ = nix::sys::signal::kill(pgid, Signal::SIGTERM);
        }

        // Wait a bit for graceful shutdown
        std::thread::sleep(Duration::from_millis(250));

        // Then force kill if still running
        if let Err(_) = killpg(pgid, Signal::SIGKILL) {
            // If killpg fails, try killing the process directly
            let _ = nix::sys::signal::kill(pgid, Signal::SIGKILL);
        }

        app.services[idx].push_log(format!("[killed {name} (PID {pid})]"));
    } else {
        // Fallback to pattern matching if no PID is stored
        let _ = Command::new("pkill").arg("-f").arg(&name).status();
        app.services[idx].push_log(format!("[killed {name}]"));
    }
}

#[cfg(not(unix))]
pub fn kill_tree(idx: usize, app: &mut App) {
    let name = app.services[idx].cfg.name.clone();
    // Use taskkill to nuke the subtree
    let _ = Command::new("taskkill")
        .args(["/F", "/T", "/FI"])
        .arg(format!("WINDOWTITLE eq {}", name))
        .status();
    app.services[idx].push_log(format!("[killed {name}]"));
}

pub fn cleanup_and_exit(app: &mut App) {
    kill_all(app);
    // Give them a moment to die
    std::thread::sleep(Duration::from_millis(200));

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
