// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Geoff Seemueller
// This file is part of muxox, released under the MIT License.

use anyhow::Result;
use muxox_core::app::{App, AppMsg, ServiceState, Status, kill_all, start_service};
use muxox_core::config::Config;
use muxox_core::isolation::default_isolation;
use muxox_core::signal::signal_watcher;
use tokio::sync::mpsc;
use tokio::task;

pub async fn run_raw_mode(cfg: Config) -> Result<()> {
    let (tx, mut rx) = mpsc::unbounded_channel::<AppMsg>();
    let mut app = App {
        services: cfg.service.into_iter().map(ServiceState::new).collect(),
        selected: 0,
        log_offset_from_end: 0,
        tx: tx.clone(),
        input_mode: false,
        input_buffer: String::new(),
        isolation: default_isolation(),
    };

    // Signal watcher: on any exit signal, nuke children then exit.
    task::spawn(signal_watcher(tx.clone()));

    // Start all services
    for idx in 0..app.services.len() {
        start_service(idx, &mut app);
    }

    // Process messages and output logs
    loop {
        match rx.recv().await {
            Some(AppMsg::Log(idx, line)) => {
                let service_name = &app.services[idx].cfg.name;
                println!("[{}] {}", service_name, line);
            }
            Some(AppMsg::Started(idx)) => {
                let service_name = &app.services[idx].cfg.name;
                eprintln!("[{}] Service started", service_name);
                app.services[idx].status = Status::Running;
            }
            Some(AppMsg::Stopped(idx, code)) => {
                let service_name = &app.services[idx].cfg.name;
                eprintln!(
                    "[{}] Service stopped with exit code: {}",
                    service_name, code
                );
                app.services[idx].status = Status::Stopped;
                app.services[idx].pid = None;
            }
            Some(AppMsg::ChildSpawned(idx, pid)) => {
                let service_name = &app.services[idx].cfg.name;
                eprintln!("[{}] Process started with PID {}", service_name, pid);
                app.services[idx].pid = Some(pid);
            }
            Some(AppMsg::StdinReady(idx, stdin_tx)) => {
                app.services[idx].stdin_tx = Some(stdin_tx);
            }
            Some(AppMsg::StdinWriterReady(idx, writer)) => {
                app.services[idx].stdin_writer = Some(writer);
            }
            Some(AppMsg::AbortedAll) => {
                eprintln!("All services aborted");
                kill_all(&mut app);
                break;
            }
            None => break,
        }
    }

    Ok(())
}
