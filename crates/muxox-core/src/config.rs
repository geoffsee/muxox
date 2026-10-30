// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Geoff Seemueller
// This file is part of muxox, released under the MIT License.

use anyhow::{Context, Result};
use semver::{Version, VersionReq};
use serde::Deserialize;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

/// Version of the running `muxox` binary, used to validate the optional
/// `muxox_version` semver range declared in user configs.
pub const MUXOX_VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Debug, Deserialize, Clone, Default)]
pub struct Config {
    /// Optional display name shown in the web UI header (and page title).
    /// Defaults to `"muxox"` when omitted or blank.
    #[serde(default)]
    pub name: Option<String>,
    /// Optional semver range (e.g. `">=1.4, <2.0"`) that the running `muxox`
    /// binary must satisfy.  Loading the config fails if the range is not met.
    #[serde(default)]
    pub muxox_version: Option<String>,
    pub service: Vec<ServiceCfg>,
    /// Optional MCP (Model Context Protocol) server settings.  Disabled by
    /// default; enable with `[mcp] enabled = true` to expose service status,
    /// logs, and start/stop controls to MCP-compatible agents over HTTP.
    #[serde(default)]
    pub mcp: McpCfg,
    /// Optional global lifecycle hooks run once at Muxox boot / shutdown.
    #[serde(default)]
    pub hooks: HooksCfg,
    /// Directory containing the loaded config file.  Relative `cwd` / `env_file`
    /// values and global hook commands resolve against this (not the process
    /// CWD).  Skipped by serde; set by [`load_config`].
    #[serde(skip)]
    pub config_dir: Option<PathBuf>,
}

/// Shell commands run around service start/stop (and optionally once globally
/// at Muxox boot/shutdown).  All fields are optional; a failing hook logs a
/// warning and does not abort the lifecycle action.
#[derive(Debug, Deserialize, Clone, Default, PartialEq, Eq)]
pub struct HooksCfg {
    /// Run before the service command is spawned (or before any service at boot).
    #[serde(default)]
    pub before_start: Option<String>,
    /// Run after a successful spawn (or after every boot start has settled).
    #[serde(default)]
    pub after_start: Option<String>,
    /// Run before an explicit stop/kill (or once before shutdown kills).
    #[serde(default)]
    pub before_stop: Option<String>,
    /// Run after the process exits (or once after shutdown kills).
    #[serde(default)]
    pub after_stop: Option<String>,
}

/// Settings for the embedded MCP (Model Context Protocol) server.
///
/// When `enabled = true`, muxox exposes a JSON-RPC 2.0 endpoint at
/// `http://{bind}:{port}/mcp` that lets MCP-compatible agents list services,
/// retrieve their captured logs, and start or stop a service by name.
#[derive(Debug, Deserialize, Clone, PartialEq)]
pub struct McpCfg {
    /// Whether to start the embedded MCP server.  Default: `false`.
    #[serde(default)]
    pub enabled: bool,
    /// TCP port for the MCP server.  `0` selects a random available port
    /// (printed to stdout at startup).  Default: `0`.
    #[serde(default)]
    pub port: u16,
    /// Address to bind the MCP server to.  Default: `127.0.0.1`.
    #[serde(default = "default_mcp_bind")]
    pub bind: String,
}

impl Default for McpCfg {
    fn default() -> Self {
        Self {
            enabled: false,
            port: 0,
            bind: default_mcp_bind(),
        }
    }
}

fn default_mcp_bind() -> String {
    "127.0.0.1".to_string()
}

/// Resolve the UI display name from config, falling back to `"muxox"`.
pub fn display_name(cfg: &Config) -> &str {
    cfg.name
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("muxox")
}

/// Validate `cfg.muxox_version` against `running` (typically [`MUXOX_VERSION`]).
///
/// Returns `Ok(())` when no constraint is declared or when the running version
/// satisfies the declared range.
pub fn enforce_version_req(cfg: &Config, running: &str) -> Result<()> {
    let Some(req_str) = cfg.muxox_version.as_deref() else {
        return Ok(());
    };
    let req = VersionReq::parse(req_str)
        .with_context(|| format!("invalid muxox_version sem-range {req_str:?}"))?;
    let version = Version::parse(running)
        .with_context(|| format!("invalid running muxox version {running:?}"))?;
    if !req.matches(&version) {
        anyhow::bail!("running muxox {running} does not satisfy muxox_version = \"{req_str}\"");
    }
    Ok(())
}

#[derive(Debug, Deserialize, Clone)]
pub struct ServiceCfg {
    pub name: String,
    pub cmd: String,
    pub cwd: Option<PathBuf>,
    /// Keep last N log lines in memory
    #[serde(default = "default_log_capacity")]
    pub log_capacity: usize,
    /// Whether the service is interactive (requires stdin)
    #[serde(default)]
    pub interactive: bool,
    /// Whether to allocate a PTY for this interactive service (Unix only)
    #[serde(default)]
    pub pty: bool,
    /// Path to a .env file whose KEY=VALUE pairs are injected into the
    /// service's environment.  Relative paths resolve from the config file's
    /// directory (see [`Config::config_dir`]).
    #[serde(default)]
    pub env_file: Option<PathBuf>,
    /// Optional isolation / sandboxing settings for this service.
    #[serde(default)]
    pub isolation: IsolationCfg,
    /// Optional per-service lifecycle hooks.
    #[serde(default)]
    pub hooks: HooksCfg,
    /// Opt-in CPU, memory, and GPU sampling. Absent means this service is not sampled.
    #[serde(default)]
    pub monitor: Option<MonitorCfg>,
}

fn default_log_capacity() -> usize {
    2000
}

fn default_true() -> bool {
    true
}

/// Per-service resource sampling. `cpu` and `memory` default to on. `gpu` defaults to off.
///
/// The table is opt-in: omit [`ServiceCfg::monitor`] to leave the service unsampled.
/// A reading is collected only while some running service asks for it.
#[derive(Debug, Deserialize, Clone, PartialEq)]
pub struct MonitorCfg {
    /// Percent of one core for this service and its child processes.
    #[serde(default = "default_true")]
    pub cpu: bool,
    /// Resident memory for this service and its child processes.
    #[serde(default = "default_true")]
    pub memory: bool,
    /// GPU usage and GPU memory. This is the expensive scan, so it stays off
    /// unless a service sets it.
    #[serde(default)]
    pub gpu: bool,
}

impl MonitorCfg {
    pub fn enabled(&self) -> bool {
        self.cpu || self.memory || self.gpu
    }
}

/// Per-service isolation settings.  All flags default to `false`
/// (no isolation beyond the standard process group).
#[derive(Debug, Deserialize, Clone, Default, PartialEq)]
pub struct IsolationCfg {
    /// Session-level process isolation (`setsid` on Unix, Job Object on Windows).
    #[serde(default)]
    pub process: bool,
    /// Restrict filesystem writes to the service working directory.
    #[serde(default)]
    pub filesystem: bool,
    /// Deny all outbound network access.
    #[serde(default)]
    pub network: bool,
}

/// Parse a `.env` file into key-value pairs.
///
/// Supports `KEY=VALUE`, optional quoting (`"` or `'`), comments (`#`),
/// and blank lines.  Does **not** modify the current process environment.
pub fn parse_env_file(path: &Path) -> Result<HashMap<String, String>> {
    let data = fs::read_to_string(path).with_context(|| format!("reading env file {path:?}"))?;
    let mut map = HashMap::new();
    for line in data.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if let Some((key, val)) = trimmed.split_once('=') {
            let key = key.trim().to_string();
            let val = val.trim();
            // Strip matching outer quotes
            let val = if (val.starts_with('"') && val.ends_with('"'))
                || (val.starts_with('\'') && val.ends_with('\''))
            {
                val[1..val.len() - 1].to_string()
            } else {
                val.to_string()
            };
            map.insert(key, val);
        }
    }
    Ok(map)
}

/// Join `path` onto `base` when `path` is relative; leave absolute paths alone.
pub fn resolve_against(base: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        base.join(path)
    }
}

/// Resolve relative service `cwd` / `env_file` paths against the config
/// directory and record that directory for global hook execution.
pub fn apply_config_dir(cfg: &mut Config, config_path: &Path) {
    let dir = config_path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."));
    for svc in &mut cfg.service {
        if let Some(ref cwd) = svc.cwd {
            svc.cwd = Some(resolve_against(&dir, cwd));
        }
        if let Some(ref env_file) = svc.env_file {
            svc.env_file = Some(resolve_against(&dir, env_file));
        }
    }
    cfg.config_dir = Some(dir);
}

pub fn load_config(provided: Option<&Path>) -> Result<Config> {
    let candidates: Vec<PathBuf> = match provided {
        Some(p) => vec![p.to_path_buf()],
        None => {
            let mut v = vec![PathBuf::from("muxox.toml")];
            if let Some(proj) = directories::ProjectDirs::from("dev", "local", "muxox") {
                v.push(proj.config_dir().join("muxox.toml"));
            }
            v
        }
    };
    for path in candidates {
        if path.exists() {
            let data = fs::read_to_string(&path)?;
            let mut cfg: Config =
                toml::from_str(&data).with_context(|| format!("parsing {path:?}"))?;
            enforce_version_req(&cfg, MUXOX_VERSION)
                .with_context(|| format!("validating {path:?}"))?;
            apply_config_dir(&mut cfg, &path);
            return Ok(cfg);
        }
    }
    anyhow::bail!("No config found; create muxox.toml or pass --config <path>")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_log_capacity() {
        assert_eq!(default_log_capacity(), 2000);
    }

    #[test]
    fn test_service_cfg_deserialize() {
        let toml_input = r#"
            name = "test-service"
            cmd = "echo hello"
            cwd = "/tmp"
            log_capacity = 1000
        "#;
        let cfg: ServiceCfg = toml::from_str(toml_input).unwrap();
        assert_eq!(cfg.name, "test-service");
        assert_eq!(cfg.cmd, "echo hello");
        assert_eq!(cfg.cwd, Some(PathBuf::from("/tmp")));
        assert_eq!(cfg.log_capacity, 1000);
    }

    #[test]
    fn test_service_cfg_default_log_capacity() {
        let toml_input = r#"
            name = "test-service"
            cmd = "echo hello"
        "#;
        let cfg: ServiceCfg = toml::from_str(toml_input).unwrap();
        assert_eq!(cfg.log_capacity, 2000);
        assert!(cfg.monitor.is_none());
    }

    #[test]
    fn monitor_defaults_cpu_and_memory_on_and_gpu_off() {
        let cfg: ServiceCfg = toml::from_str(
            r#"
            name = "api"
            cmd = "true"
            monitor = {}
            "#,
        )
        .unwrap();
        let monitor = cfg.monitor.expect("monitor table");
        assert!(monitor.cpu);
        assert!(monitor.memory);
        assert!(!monitor.gpu);
        assert!(monitor.enabled());
    }

    #[test]
    fn monitor_flags_can_turn_fields_off_and_gpu_on() {
        let cfg: ServiceCfg = toml::from_str(
            r#"
            name = "api"
            cmd = "true"
            monitor = { cpu = false, memory = true, gpu = true }
            "#,
        )
        .unwrap();
        let monitor = cfg.monitor.expect("monitor table");
        assert!(!monitor.cpu);
        assert!(monitor.memory);
        assert!(monitor.gpu);
    }

    #[test]
    fn test_service_cfg_env_file() {
        let toml_input = r#"
            name = "with-env"
            cmd = "echo hi"
            env_file = ".env.local"
        "#;
        let cfg: ServiceCfg = toml::from_str(toml_input).unwrap();
        assert_eq!(cfg.env_file, Some(PathBuf::from(".env.local")));
    }

    #[test]
    fn test_service_cfg_env_file_defaults_to_none() {
        let toml_input = r#"
            name = "no-env"
            cmd = "echo hi"
        "#;
        let cfg: ServiceCfg = toml::from_str(toml_input).unwrap();
        assert_eq!(cfg.env_file, None);
    }

    #[test]
    fn test_parse_env_file() {
        let dir = std::env::temp_dir().join("muxox_test_env");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join(".env.test");
        std::fs::write(
            &path,
            r#"
# database
DB_HOST=localhost
DB_PORT=5432
SECRET="my secret"
SINGLE='quoted'

# blank lines and comments are skipped
API_KEY=abc123
"#,
        )
        .unwrap();

        let vars = parse_env_file(&path).unwrap();
        assert_eq!(vars["DB_HOST"], "localhost");
        assert_eq!(vars["DB_PORT"], "5432");
        assert_eq!(vars["SECRET"], "my secret");
        assert_eq!(vars["SINGLE"], "quoted");
        assert_eq!(vars["API_KEY"], "abc123");
        assert_eq!(vars.len(), 5);

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_isolation_cfg_defaults_to_all_false() {
        let toml_input = r#"
            name = "svc"
            cmd = "echo hi"
        "#;
        let cfg: ServiceCfg = toml::from_str(toml_input).unwrap();
        assert_eq!(cfg.isolation, IsolationCfg::default());
        assert!(!cfg.isolation.process);
        assert!(!cfg.isolation.filesystem);
        assert!(!cfg.isolation.network);
    }

    #[test]
    fn test_isolation_cfg_partial() {
        let toml_input = r#"
            name = "svc"
            cmd = "echo hi"
            isolation.network = true
        "#;
        let cfg: ServiceCfg = toml::from_str(toml_input).unwrap();
        assert!(!cfg.isolation.process);
        assert!(!cfg.isolation.filesystem);
        assert!(cfg.isolation.network);
    }

    #[test]
    fn test_isolation_cfg_all_enabled() {
        let toml_input = r#"
            name = "svc"
            cmd = "echo hi"
            [isolation]
            process = true
            filesystem = true
            network = true
        "#;
        let cfg: ServiceCfg = toml::from_str(toml_input).unwrap();
        assert!(cfg.isolation.process);
        assert!(cfg.isolation.filesystem);
        assert!(cfg.isolation.network);
    }

    #[test]
    fn test_isolation_cfg_inline_table() {
        let toml_input = r#"
            name = "svc"
            cmd = "echo hi"
            isolation = { process = true, filesystem = false, network = true }
        "#;
        let cfg: ServiceCfg = toml::from_str(toml_input).unwrap();
        assert!(cfg.isolation.process);
        assert!(!cfg.isolation.filesystem);
        assert!(cfg.isolation.network);
    }

    #[test]
    fn test_config_deserialize() {
        let toml_input = r#"
            [[service]]
            name = "svc1"
            cmd = "run1"
            [[service]]
            name = "svc2"
            cmd = "run2"
        "#;
        let cfg: Config = toml::from_str(toml_input).unwrap();
        assert_eq!(cfg.service.len(), 2);
        assert_eq!(cfg.service[0].name, "svc1");
        assert_eq!(cfg.service[1].name, "svc2");
        assert!(cfg.name.is_none());
        assert!(cfg.muxox_version.is_none());
        assert_eq!(display_name(&cfg), "muxox");
    }

    #[test]
    fn test_top_level_name_field_parses() {
        let toml_input = r#"
            name = "Acme Dev"
            [[service]]
            name = "svc1"
            cmd = "run1"
        "#;
        let cfg: Config = toml::from_str(toml_input).unwrap();
        assert_eq!(cfg.name.as_deref(), Some("Acme Dev"));
        assert_eq!(display_name(&cfg), "Acme Dev");
    }

    #[test]
    fn test_blank_top_level_name_falls_back() {
        let cfg = Config {
            name: Some("   ".into()),
            ..Default::default()
        };
        assert_eq!(display_name(&cfg), "muxox");
    }

    #[test]
    fn test_muxox_version_field_parses() {
        let toml_input = r#"
            muxox_version = ">=1.4, <2.0"
            [[service]]
            name = "svc1"
            cmd = "run1"
        "#;
        let cfg: Config = toml::from_str(toml_input).unwrap();
        assert_eq!(cfg.muxox_version.as_deref(), Some(">=1.4, <2.0"));
    }

    #[test]
    fn test_hooks_cfg_defaults_empty() {
        let toml_input = r#"
            name = "svc"
            cmd = "echo hi"
        "#;
        let cfg: ServiceCfg = toml::from_str(toml_input).unwrap();
        assert_eq!(cfg.hooks, HooksCfg::default());
    }

    #[test]
    fn test_service_hooks_inline_table() {
        let toml_input = r#"
            name = "api"
            cmd = "bun run dev"
            hooks = { before_start = "./pre.sh", after_stop = "./post.sh" }
        "#;
        let cfg: ServiceCfg = toml::from_str(toml_input).unwrap();
        assert_eq!(cfg.hooks.before_start.as_deref(), Some("./pre.sh"));
        assert!(cfg.hooks.after_start.is_none());
        assert!(cfg.hooks.before_stop.is_none());
        assert_eq!(cfg.hooks.after_stop.as_deref(), Some("./post.sh"));
    }

    #[test]
    fn test_service_hooks_subtable() {
        let toml_input = r#"
            name = "api"
            cmd = "bun run dev"
            [hooks]
            before_start = "./pre.sh"
            after_start = "./ready.sh"
            before_stop = "./drain.sh"
            after_stop = "./cleanup.sh"
        "#;
        let cfg: ServiceCfg = toml::from_str(toml_input).unwrap();
        assert_eq!(cfg.hooks.before_start.as_deref(), Some("./pre.sh"));
        assert_eq!(cfg.hooks.after_start.as_deref(), Some("./ready.sh"));
        assert_eq!(cfg.hooks.before_stop.as_deref(), Some("./drain.sh"));
        assert_eq!(cfg.hooks.after_stop.as_deref(), Some("./cleanup.sh"));
    }

    #[test]
    fn test_global_hooks_on_config() {
        let toml_input = r#"
            [hooks]
            before_start = "./boot-pre.sh"
            after_stop = "./shutdown-post.sh"
            [[service]]
            name = "svc1"
            cmd = "run1"
        "#;
        let cfg: Config = toml::from_str(toml_input).unwrap();
        assert_eq!(cfg.hooks.before_start.as_deref(), Some("./boot-pre.sh"));
        assert!(cfg.hooks.after_start.is_none());
        assert!(cfg.hooks.before_stop.is_none());
        assert_eq!(cfg.hooks.after_stop.as_deref(), Some("./shutdown-post.sh"));
        assert!(cfg.service[0].hooks.before_start.is_none());
    }

    #[test]
    fn test_enforce_version_req_satisfied() {
        let cfg = Config {
            muxox_version: Some(">=1.4, <2.0".to_string()),
            ..Default::default()
        };
        enforce_version_req(&cfg, "1.4.0").expect("1.4.0 satisfies >=1.4, <2.0");
        enforce_version_req(&cfg, "1.9.3").expect("1.9.3 satisfies >=1.4, <2.0");
    }

    #[test]
    fn test_enforce_version_req_unsatisfied() {
        let cfg = Config {
            muxox_version: Some(">=1.4, <2.0".to_string()),
            ..Default::default()
        };
        let err = enforce_version_req(&cfg, "1.3.0").expect_err("1.3.0 must not satisfy >=1.4");
        assert!(err.to_string().contains("does not satisfy"));

        let err = enforce_version_req(&cfg, "2.0.0").expect_err("2.0.0 must not satisfy <2.0");
        assert!(err.to_string().contains("does not satisfy"));
    }

    #[test]
    fn test_enforce_version_req_absent_is_ok() {
        let cfg = Config::default();
        enforce_version_req(&cfg, "0.0.1").expect("no constraint => any version is fine");
    }

    #[test]
    fn test_enforce_version_req_invalid_range() {
        let cfg = Config {
            muxox_version: Some("not-a-range".to_string()),
            ..Default::default()
        };
        let err = enforce_version_req(&cfg, "1.4.0").expect_err("garbage range must error");
        assert!(err.to_string().contains("invalid muxox_version"));
    }

    #[test]
    fn test_enforce_version_req_caret_and_tilde() {
        let cfg = Config {
            muxox_version: Some("^1.4".to_string()),
            ..Default::default()
        };
        enforce_version_req(&cfg, "1.5.7").expect("^1.4 admits 1.5.7");
        enforce_version_req(&cfg, "2.0.0").expect_err("^1.4 excludes 2.0.0");

        let cfg = Config {
            muxox_version: Some("~1.4.2".to_string()),
            ..Default::default()
        };
        enforce_version_req(&cfg, "1.4.9").expect("~1.4.2 admits 1.4.9");
        enforce_version_req(&cfg, "1.5.0").expect_err("~1.4.2 excludes 1.5.0");
    }

    #[test]
    fn test_running_muxox_version_satisfies_workspace_range() {
        let cfg = Config {
            muxox_version: Some(format!("={MUXOX_VERSION}")),
            ..Default::default()
        };
        enforce_version_req(&cfg, MUXOX_VERSION)
            .expect("running version always satisfies its own pinned range");
    }

    #[test]
    fn test_apply_config_dir_resolves_relative_paths() {
        let toml_input = r#"
            [[service]]
            name = "svc"
            cmd = "echo hi"
            cwd = "packages/svc"
            env_file = "packages/svc/.env"
        "#;
        let mut cfg: Config = toml::from_str(toml_input).unwrap();
        apply_config_dir(&mut cfg, Path::new("example/muxox.toml"));
        assert_eq!(cfg.config_dir, Some(PathBuf::from("example")));
        assert_eq!(
            cfg.service[0].cwd.as_deref(),
            Some(Path::new("example/packages/svc"))
        );
        assert_eq!(
            cfg.service[0].env_file.as_deref(),
            Some(Path::new("example/packages/svc/.env"))
        );
    }

    #[test]
    fn test_apply_config_dir_keeps_absolute_paths() {
        let toml_input = r#"
            [[service]]
            name = "svc"
            cmd = "echo hi"
            cwd = "/abs/cwd"
            env_file = "/abs/.env"
        "#;
        let mut cfg: Config = toml::from_str(toml_input).unwrap();
        apply_config_dir(&mut cfg, Path::new("/configs/muxox.toml"));
        assert_eq!(cfg.service[0].cwd.as_deref(), Some(Path::new("/abs/cwd")));
        assert_eq!(
            cfg.service[0].env_file.as_deref(),
            Some(Path::new("/abs/.env"))
        );
    }
}
