// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Geoff Seemueller
// This file is part of muxox, released under the MIT License.

use anyhow::{Context, Result};
use serde::Deserialize;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Deserialize, Clone, Default)]
pub struct Config {
    #[serde(default)]
    pub service: Vec<ServiceCfg>,
    /// Optional MCP (Model Context Protocol) server settings.  Disabled by
    /// default; enable with `[mcp] enabled = true` to expose service status
    /// and logs to MCP-compatible agents over HTTP.
    #[serde(default)]
    pub mcp: McpCfg,
}

/// Settings for the embedded MCP (Model Context Protocol) server.
///
/// When `enabled = true`, muxox exposes a JSON-RPC 2.0 endpoint at
/// `http://{bind}:{port}/mcp` that lets MCP-compatible agents list services
/// and retrieve their captured logs without scraping the web UI.
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
    /// service's environment.  Relative paths resolve from the process CWD.
    #[serde(default)]
    pub env_file: Option<PathBuf>,
    /// Optional isolation / sandboxing settings for this service.
    #[serde(default)]
    pub isolation: IsolationCfg,
}

fn default_log_capacity() -> usize {
    2000
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
            return toml::from_str(&data).with_context(|| format!("parsing {path:?}"));
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
        assert!(!cfg.mcp.enabled);
    }

    #[test]
    fn test_mcp_cfg_defaults_to_disabled() {
        let toml_input = r#"
            [[service]]
            name = "svc"
            cmd = "run"
        "#;
        let cfg: Config = toml::from_str(toml_input).unwrap();
        assert_eq!(cfg.mcp, McpCfg::default());
        assert!(!cfg.mcp.enabled);
        assert_eq!(cfg.mcp.port, 0);
        assert_eq!(cfg.mcp.bind, "127.0.0.1");
    }

    #[test]
    fn test_mcp_cfg_enabled_only() {
        let toml_input = r#"
            [mcp]
            enabled = true

            [[service]]
            name = "svc"
            cmd = "run"
        "#;
        let cfg: Config = toml::from_str(toml_input).unwrap();
        assert!(cfg.mcp.enabled);
        assert_eq!(cfg.mcp.port, 0);
        assert_eq!(cfg.mcp.bind, "127.0.0.1");
    }

    #[test]
    fn test_mcp_cfg_full() {
        let toml_input = r#"
            [mcp]
            enabled = true
            port = 4242
            bind = "0.0.0.0"

            [[service]]
            name = "svc"
            cmd = "run"
        "#;
        let cfg: Config = toml::from_str(toml_input).unwrap();
        assert!(cfg.mcp.enabled);
        assert_eq!(cfg.mcp.port, 4242);
        assert_eq!(cfg.mcp.bind, "0.0.0.0");
    }
}
