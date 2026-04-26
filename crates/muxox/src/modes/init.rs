// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Geoff Seemueller
// This file is part of muxox, released under the MIT License.

use std::fs;
use std::path::Path;

use anyhow::{Context, Result, bail};

/// Comprehensive, fully-commented example written by `muxox init`.
///
/// Every option supported by `ServiceCfg` is mentioned here so users can copy
/// the relevant snippets into their own configuration.
pub const TEMPLATE: &str = r##"# muxox.toml -- service definitions for the muxox process orchestrator.
#
# Each service is declared as a `[[service]]` table.  Required fields are
# `name` and `cmd`; everything else is optional.  See the commented examples
# below for the full set of options.
#
# Run `muxox` (web UI), `muxox --tui` (terminal UI), or `muxox --raw`
# (plain log streaming) to start every service.
#
# ---------------------------------------------------------------------------
# Field reference
# ---------------------------------------------------------------------------
# name          (string,  required) Unique identifier shown in the UI.
# cmd           (string,  required) Shell command to execute.
# cwd           (path,    optional) Working directory for the command.
# log_capacity  (int,     optional) Lines kept in memory.  Default: 2000.
# interactive   (bool,    optional) Service consumes stdin.  Default: false.
# pty           (bool,    optional) Allocate a PTY (Unix only).  Default: false.
# env_file      (path,    optional) `.env` file injected into the environment.
# isolation     (table,   optional) Per-service sandboxing flags (see below).
#
# ---------------------------------------------------------------------------
# Minimal example -- the smallest valid service definition.
# ---------------------------------------------------------------------------
# [[service]]
# name = "hello"
# cmd  = "echo hello world"
#
# ---------------------------------------------------------------------------
# Typical web service
# ---------------------------------------------------------------------------
# [[service]]
# name         = "api"
# cmd          = "bun run dev"
# cwd          = "./packages/api"
# log_capacity = 5000
# env_file     = "./packages/api/.env.local"
#
# ---------------------------------------------------------------------------
# Interactive service (e.g. a REPL or a script that reads stdin).  Set `pty`
# to true on Unix when the program expects a real terminal (curses, prompts,
# colored output, line editing).
# ---------------------------------------------------------------------------
# [[service]]
# name         = "repl"
# cmd          = "node"
# cwd          = "."
# interactive  = true
# pty          = true
# log_capacity = 1000
#
# ---------------------------------------------------------------------------
# Sandboxed worker -- opt-in per-service isolation.  All flags default to
# `false`; enable only what you need.  Behaviour by platform:
#
#   process     setsid (Unix) / Job Object KILL_ON_JOB_CLOSE (Windows)
#   filesystem  sandbox_init SBPL (macOS) / user + mount namespaces (Linux)
#   network     sandbox_init SBPL (macOS) / user + network namespaces (Linux)
#
# Linux namespace isolation requires unprivileged user namespaces:
#   sysctl kernel.unprivileged_userns_clone=1
# ---------------------------------------------------------------------------
# [[service]]
# name = "sandboxed-worker"
# cmd  = "node worker.js"
# cwd  = "./packages/worker"
# isolation = { process = true, filesystem = true, network = true }
#
# Equivalent using a sub-table:
# [[service]]
# name = "sandboxed-worker"
# cmd  = "node worker.js"
# cwd  = "./packages/worker"
# [service.isolation]
# process    = true
# filesystem = true
# network    = true
#
# ---------------------------------------------------------------------------
# Optional embedded MCP (Model Context Protocol) server.
#
# When enabled, muxox exposes a JSON-RPC endpoint at
# http://{bind}:{port}/mcp that lets MCP-aware agents list services and
# read their captured logs without scraping the web UI.  Available in web
# mode (default) and raw mode (`muxox --raw`).
# ---------------------------------------------------------------------------
# [mcp]
# enabled = true       # default: false
# port    = 0          # default: 0 (random available port)
# bind    = "127.0.0.1"
#
# ---------------------------------------------------------------------------
# Starter service -- replace with your own.
# ---------------------------------------------------------------------------
[[service]]
name = "example"
cmd  = "echo 'edit muxox.toml to add your services'"
log_capacity = 250
"##;

pub fn run_init(path: &Path, force: bool) -> Result<()> {
    if path.exists() && !force {
        bail!(
            "{} already exists; pass --force to overwrite",
            path.display()
        );
    }
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent)
            .with_context(|| format!("creating parent directory {}", parent.display()))?;
    }
    fs::write(path, TEMPLATE).with_context(|| format!("writing {}", path.display()))?;
    println!("Created {}", path.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use muxox_core::config::Config;

    #[test]
    fn template_is_valid_toml_and_has_a_service() {
        let cfg: Config = toml::from_str(TEMPLATE).expect("template parses as Config");
        assert!(!cfg.service.is_empty(), "template should have a service");
    }

    #[test]
    fn run_init_writes_file_and_refuses_overwrite() {
        let dir = std::env::temp_dir().join(format!("muxox_init_test_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let target = dir.join("muxox.toml");

        run_init(&target, false).expect("first init succeeds");
        assert!(target.exists());

        let err = run_init(&target, false).expect_err("second init without --force errors");
        assert!(err.to_string().contains("--force"));

        run_init(&target, true).expect("init with --force overwrites");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
