// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Geoff Seemueller
// This file is part of muxox, released under the MIT License.

use anyhow::{Context, Result};
use serde::Deserialize;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Deserialize, Clone)]
pub struct Config {
    pub service: Vec<ServiceCfg>,
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
}

fn default_log_capacity() -> usize {
    2000
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
            return Ok(toml::from_str(&data).with_context(|| format!("parsing {path:?}"))?);
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
    }
}
