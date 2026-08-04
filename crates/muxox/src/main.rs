// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Geoff Seemueller
// This file is part of muxox, released under the MIT License.

use std::path::PathBuf;

use anyhow::Result;
use clap::{Parser, Subcommand};

mod modes;

use modes::{
    init::run_init,
    raw::run_raw_mode,
    skill::{SkillClient, run_install_skill},
    tui::run_tui_mode,
};
use muxox_core::config::load_config;
use muxox_web::run_web_mode;

#[derive(Debug, Parser)]
#[command(author, version, about = "Run multiple dev servers with a simple TUI.")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,

    /// Optional path to a services config (TOML). If omitted, looks in: $PWD/muxox.toml then app dirs.
    #[arg(short, long, global = true)]
    config: Option<PathBuf>,
    /// Run in non-interactive mode, outputting raw logs instead of TUI or Web UI
    #[arg(long)]
    raw: bool,
    /// Run in TUI mode
    #[arg(long)]
    tui: bool,
    /// Port for the Web UI (default: random available port)
    #[arg(short, long, default_value_t = 0)]
    port: u16,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Create a `muxox.toml` in the current directory with a commented example.
    Init {
        /// Path to write the config file to. Defaults to ./muxox.toml.
        #[arg(short, long)]
        output: Option<PathBuf>,
        /// Overwrite the file if it already exists.
        #[arg(short, long)]
        force: bool,
    },
    /// Install the Muxox agent skill for supported coding agents.
    InstallSkill {
        /// Install for selected clients (comma-separated); defaults to all.
        #[arg(long, value_enum, value_delimiter = ',', default_value = "all")]
        client: Vec<SkillClient>,
        /// Show what would be installed without writing files.
        #[arg(long)]
        dry_run: bool,
        /// Replace an existing skill when its contents differ.
        #[arg(long)]
        force: bool,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    if let Some(command) = cli.command {
        return match command {
            Command::Init { output, force } => {
                let target = output.unwrap_or_else(|| PathBuf::from("muxox.toml"));
                run_init(&target, force)
            }
            Command::InstallSkill {
                client,
                dry_run,
                force,
            } => run_install_skill(&client, dry_run, force),
        };
    }

    muxox_core::log::init();

    let mode = if cli.raw {
        "raw"
    } else if cli.tui {
        "tui"
    } else {
        "web"
    };
    muxox_core::log::debug(&format!("starting mode={mode}"));
    let cfg = load_config(cli.config.as_deref())?;

    if cli.raw {
        run_raw_mode(cfg).await
    } else if cli.tui {
        run_tui_mode(cfg).await
    } else {
        run_web_mode(cfg, cli.port).await
    }
}
