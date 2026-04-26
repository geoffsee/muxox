// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Geoff Seemueller
// This file is part of muxox, released under the MIT License.

use std::sync::Arc;

use anyhow::Result;
use muxox_core::app::{App, AppMsg, ServiceState, Status, kill_all, start_service};
use muxox_core::config::Config;
use muxox_core::isolation::default_isolation;
use muxox_core::signal::signal_watcher;
use muxox_web::run_mcp_server;
use tokio::sync::{Mutex, mpsc};
use tokio::task;

pub async fn run_raw_mode(cfg: Config) -> Result<()> {
    let Config { service, mcp, .. } = cfg;
    let (tx, mut rx) = mpsc::unbounded_channel::<AppMsg>();
    let app = Arc::new(Mutex::new(App {
        services: service.into_iter().map(ServiceState::new).collect(),
        selected: 0,
        log_offset_from_end: 0,
        tx: tx.clone(),
        input_mode: false,
        input_buffer: String::new(),
        isolation: default_isolation(),
    }));

    // Signal watcher: on any exit signal, nuke children then exit.
    task::spawn(signal_watcher(tx.clone()));

    // Optional embedded MCP server for agent log access.
    if mcp.enabled {
        let app_for_mcp = Arc::clone(&app);
        task::spawn(async move {
            if let Err(e) = run_mcp_server(mcp, app_for_mcp).await {
                eprintln!("MCP server error: {e}");
            }
        });
    }

    // Start all services
    {
        let mut guard = app.lock().await;
        for idx in 0..guard.services.len() {
            start_service(idx, &mut guard);
        }
    }

    // Process messages and output logs
    loop {
        match rx.recv().await {
            Some(AppMsg::Log(idx, line)) => {
                let mut guard = app.lock().await;
                let service_name = guard.services[idx].cfg.name.clone();
                println!("[{}] {}", service_name, line);
                guard.services[idx].push_log(line);
            }
            Some(AppMsg::Started(idx)) => {
                let mut guard = app.lock().await;
                let service_name = guard.services[idx].cfg.name.clone();
                eprintln!("[{}] Service started", service_name);
                guard.services[idx].status = Status::Running;
            }
            Some(AppMsg::Stopped(idx, code)) => {
                let mut guard = app.lock().await;
                let service_name = guard.services[idx].cfg.name.clone();
                eprintln!(
                    "[{}] Service stopped with exit code: {}",
                    service_name, code
                );
                guard.services[idx].status = Status::Stopped;
                guard.services[idx].pid = None;
            }
            Some(AppMsg::ChildSpawned(idx, pid)) => {
                let mut guard = app.lock().await;
                let service_name = guard.services[idx].cfg.name.clone();
                eprintln!("[{}] Process started with PID {}", service_name, pid);
                guard.services[idx].pid = Some(pid);
            }
            Some(AppMsg::StdinReady(idx, stdin_tx)) => {
                let mut guard = app.lock().await;
                guard.services[idx].stdin_tx = Some(stdin_tx);
            }
            Some(AppMsg::StdinWriterReady(idx, writer)) => {
                let mut guard = app.lock().await;
                guard.services[idx].stdin_writer = Some(writer);
            }
            Some(AppMsg::AbortedAll) => {
                eprintln!("All services aborted");
                let mut guard = app.lock().await;
                kill_all(&mut guard);
                break;
            }
            None => break,
        }
    }

    Ok(())
}
