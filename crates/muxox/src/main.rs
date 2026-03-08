// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Geoff Seemueller
// This file is part of muxox, released under the MIT License.

use std::path::PathBuf;

use anyhow::Result;
use clap::Parser;

mod app;
mod config;
mod modes;
mod signal;
mod utils;
mod web_ui;
mod ws_proto;

use config::load_config;
use modes::{raw::run_raw_mode, tui::run_tui_mode, web::run_web_mode};

#[derive(Debug, Parser)]
#[command(author, version, about = "Run multiple dev servers with a simple TUI.")]
struct Cli {
    /// Optional path to a services config (TOML). If omitted, looks in: $PWD/muxox.toml then app dirs.
    #[arg(short, long)]
    config: Option<PathBuf>,
    /// Run in non-interactive mode, outputting raw logs instead of TUI or Web UI
    #[arg(long)]
    raw: bool,
    /// Run in TUI mode
    #[arg(long)]
    tui: bool,
    /// Port for the Web UI (default: 8772)
    #[arg(short, long, default_value_t = 8772)]
    port: u16,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let cfg = load_config(cli.config.as_deref())?;

    if cli.raw {
        run_raw_mode(cfg).await
    } else if cli.tui {
        run_tui_mode(cfg).await
    } else {
        run_web_mode(cfg, cli.port).await
    }
}
