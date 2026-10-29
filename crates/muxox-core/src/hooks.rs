// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Geoff Seemueller
// This file is part of muxox, released under the MIT License.

//! Lifecycle hook execution for global and per-service before/after commands.

use crate::config::parse_env_file;
use crate::utils::{shell_exec, shell_flag, shell_program};
use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::{Command, Stdio};
use tokio::io::{AsyncBufReadExt, BufReader as AsyncBufReader};
use tokio::process::Command as AsyncCommand;
use tokio::sync::mpsc;

/// Lifecycle phase name used in log / warn messages.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookPhase {
    BeforeStart,
    AfterStart,
    BeforeStop,
    AfterStop,
}

impl HookPhase {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::BeforeStart => "before_start",
            Self::AfterStart => "after_start",
            Self::BeforeStop => "before_stop",
            Self::AfterStop => "after_stop",
        }
    }
}

fn warn_nonzero(phase: HookPhase, code: i32, log: &mut dyn FnMut(String)) {
    if code != 0 {
        log(format!(
            "[WARN] hook {} failed with exit code {code}",
            phase.as_str()
        ));
    }
}

fn load_env(env_file: Option<&Path>, log: &mut dyn FnMut(String)) -> Vec<(String, String)> {
    let Some(path) = env_file else {
        return Vec::new();
    };
    match parse_env_file(path) {
        Ok(vars) => vars.into_iter().collect(),
        Err(e) => {
            log(format!(
                "[WARN] hook failed to load env file {}: {e}",
                path.display()
            ));
            Vec::new()
        }
    }
}

/// Run a hook synchronously (stop / shutdown paths). Streams stdout/stderr
/// lines into `log`. Returns the exit code (or -1 on spawn/wait failure).
/// Non-zero exits only produce a warning via `log`.
pub fn run_hook_sync(
    phase: HookPhase,
    cmd_str: &str,
    cwd: Option<&Path>,
    env_file: Option<&Path>,
    mut log: impl FnMut(String),
) -> i32 {
    log(format!("[hook {}] $ {cmd_str}", phase.as_str()));

    let env = load_env(env_file, &mut log);
    let mut cmd = Command::new(shell_program());
    cmd.arg(shell_flag()).arg(shell_exec(cmd_str));
    if let Some(dir) = cwd {
        cmd.current_dir(dir);
    }
    cmd.envs(env);
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            log(format!(
                "[WARN] hook {} failed to spawn: {e}",
                phase.as_str()
            ));
            return -1;
        }
    };

    if let Some(out) = child.stdout.take() {
        for line in BufReader::new(out).lines().map_while(Result::ok) {
            log(line);
        }
    }
    if let Some(err) = child.stderr.take() {
        for line in BufReader::new(err).lines().map_while(Result::ok) {
            log(line);
        }
    }

    let code = match child.wait() {
        Ok(status) => status.code().unwrap_or(-1),
        Err(e) => {
            log(format!(
                "[WARN] hook {} wait failed: {e}",
                phase.as_str()
            ));
            -1
        }
    };
    warn_nonzero(phase, code, &mut log);
    code
}

/// Run a hook asynchronously (start paths). Streams lines into `log`.
/// Returns the exit code (or -1 on failure). Non-zero exits warn via `log`.
pub async fn run_hook_async(
    phase: HookPhase,
    cmd_str: &str,
    cwd: Option<&Path>,
    env_file: Option<&Path>,
    mut log: impl FnMut(String),
) -> i32 {
    log(format!("[hook {}] $ {cmd_str}", phase.as_str()));

    let env = load_env(env_file, &mut log);
    let mut cmd = AsyncCommand::new(shell_program());
    cmd.arg(shell_flag()).arg(shell_exec(cmd_str));
    if let Some(dir) = cwd {
        cmd.current_dir(dir);
    }
    cmd.envs(env);
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            log(format!(
                "[WARN] hook {} failed to spawn: {e}",
                phase.as_str()
            ));
            return -1;
        }
    };

    let (line_tx, mut line_rx) = mpsc::unbounded_channel::<String>();

    let out_task = child.stdout.take().map(|s| {
        let tx = line_tx.clone();
        tokio::spawn(async move {
            let mut reader = AsyncBufReader::new(s).lines();
            while let Ok(Some(line)) = reader.next_line().await {
                if tx.send(line).is_err() {
                    break;
                }
            }
        })
    });
    let err_task = child.stderr.take().map(|s| {
        tokio::spawn(async move {
            let mut reader = AsyncBufReader::new(s).lines();
            while let Ok(Some(line)) = reader.next_line().await {
                if line_tx.send(line).is_err() {
                    break;
                }
            }
        })
    });

    let wait_result = child.wait().await;

    if let Some(t) = out_task {
        let _ = t.await;
    }
    if let Some(t) = err_task {
        let _ = t.await;
    }
    while let Ok(line) = line_rx.try_recv() {
        log(line);
    }

    let code = match wait_result {
        Ok(s) => s.code().unwrap_or(-1),
        Err(e) => {
            log(format!(
                "[WARN] hook {} wait failed: {e}",
                phase.as_str()
            ));
            -1
        }
    };
    warn_nonzero(phase, code, &mut log);
    code
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sync_hook_success_and_output() {
        let mut lines = Vec::new();
        let code = run_hook_sync(HookPhase::BeforeStart, "echo hook-ok", None, None, |l| {
            lines.push(l)
        });
        assert_eq!(code, 0);
        assert!(lines.iter().any(|l| l.contains("hook-ok")));
        assert!(!lines.iter().any(|l| l.contains("[WARN]")));
    }

    #[test]
    fn sync_hook_nonzero_warns_and_returns_code() {
        let mut lines = Vec::new();
        let code = run_hook_sync(HookPhase::AfterStop, "exit 42", None, None, |l| {
            lines.push(l)
        });
        assert_eq!(code, 42);
        assert!(
            lines
                .iter()
                .any(|l| l.contains("[WARN] hook after_stop failed with exit code 42"))
        );
    }

    #[tokio::test]
    async fn async_hook_success() {
        let mut lines = Vec::new();
        let code =
            run_hook_async(HookPhase::AfterStart, "echo async-ok", None, None, |l| {
                lines.push(l)
            })
            .await;
        assert_eq!(code, 0);
        assert!(lines.iter().any(|l| l.contains("async-ok")));
    }
}
