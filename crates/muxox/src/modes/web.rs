// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Geoff Seemueller
// This file is part of muxox, released under the MIT License.

use crate::app::{App, AppMsg, ServiceState, apply_msg, kill_all, start_service, stop_service};
use crate::config::Config;
use crate::signal::signal_watcher;
use crate::web_ui;
use crate::ws_proto::{ServiceInfo, WsInbound, WsOutbound};
use anyhow::Result;
use axum::{
    Router,
    extract::{
        State,
        ws::{Message, WebSocket, WebSocketUpgrade},
    },
    response::{Html, IntoResponse},
    routing::get,
};
use futures_util::{SinkExt, StreamExt};
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::{Mutex, broadcast, mpsc};
use tokio::task;

pub async fn run_web_mode(cfg: Config, port: u16) -> Result<()> {
    let (tx, mut rx) = mpsc::unbounded_channel::<AppMsg>();
    let (b_tx, _) = broadcast::channel::<Vec<u8>>(100);

    let app = Arc::new(Mutex::new(App {
        services: cfg.service.into_iter().map(ServiceState::new).collect(),
        selected: 0,
        log_offset_from_end: 0,
        tx: tx.clone(),
        input_mode: false,
        input_buffer: String::new(),
    }));

    // Start all services
    {
        let mut app_lock = app.lock().await;
        for idx in 0..app_lock.services.len() {
            start_service(idx, &mut app_lock);
        }
    }

    // Signal watcher
    task::spawn(signal_watcher(tx.clone()));

    // Broadcast messages to all connected WebSockets
    let b_tx_clone = b_tx.clone();
    let app_for_msgs = Arc::clone(&app);
    task::spawn(async move {
        while let Some(msg) = rx.recv().await {
            let mut app_lock = app_for_msgs.lock().await;
            apply_msg(&mut app_lock, msg.clone());

            let ws_msg = match &msg {
                AppMsg::Log(idx, line) => Some(WsOutbound::Log {
                    idx: *idx,
                    line: line.clone(),
                }),
                AppMsg::Started(idx) => Some(WsOutbound::Status {
                    idx: *idx,
                    status: "Running".into(),
                }),
                AppMsg::Stopped(idx, _) => Some(WsOutbound::Status {
                    idx: *idx,
                    status: "Stopped".into(),
                }),
                _ => None,
            };

            if let Some(msg) = ws_msg {
                let _ = b_tx_clone
                    .send(serde_json::to_vec(&msg).expect("WsOutbound serialization cannot fail"));
            }

            if matches!(msg, AppMsg::AbortedAll) {
                kill_all(&mut app_lock);
                std::process::exit(0);
            }
        }
    });

    let shared_state = Arc::new(WebState {
        app: Arc::clone(&app),
        broadcast_tx: b_tx,
    });

    let router = Router::new()
        .route("/", get(index_handler))
        .route("/ws", get(ws_handler))
        .with_state(shared_state);

    let addr: SocketAddr = format!("[::1]:{}", port).parse()?;
    let listener = tokio::net::TcpListener::bind(addr).await?;
    let url = format!("http://localhost:{}", port);
    println!("Web UI available at {}", url);

    if let Err(e) = open::that(url) {
        eprintln!("Failed to open browser: {}", e);
    }

    axum::serve(listener, router).await?;

    Ok(())
}

struct WebState {
    app: Arc<Mutex<App>>,
    broadcast_tx: broadcast::Sender<Vec<u8>>,
}

async fn index_handler() -> impl IntoResponse {
    Html(web_ui::render_index())
}

async fn ws_handler(ws: WebSocketUpgrade, State(state): State<Arc<WebState>>) -> impl IntoResponse {
    ws.on_upgrade(|socket| handle_socket(socket, state))
}

async fn handle_socket(socket: WebSocket, state: Arc<WebState>) {
    let (mut sender, mut receiver) = socket.split();
    let mut b_rx = state.broadcast_tx.subscribe();

    // Send initial state
    {
        let app = state.app.lock().await;
        let services: Vec<ServiceInfo> = app
            .services
            .iter()
            .map(|s| ServiceInfo {
                name: s.cfg.name.clone(),
                status: s.status.as_str().to_owned(),
                logs: s.log.iter().cloned().collect(),
                interactive: s.cfg.interactive,
            })
            .collect();
        let init_msg = WsOutbound::Init { services };
        if let Err(e) = sender.send(init_msg.to_message()).await {
            eprintln!("Failed to send init msg: {}", e);
            return;
        }
    }

    let mut send_task = task::spawn(async move {
        while let Ok(bytes) = b_rx.recv().await {
            if sender.send(Message::Binary(bytes.into())).await.is_err() {
                break;
            }
        }
    });

    let app_clone = Arc::clone(&state.app);
    let mut recv_task = task::spawn(async move {
        while let Some(Ok(msg)) = receiver.next().await {
            let Some(cmd) = WsInbound::from_message(msg) else {
                continue;
            };
            let WsInbound::Command {
                idx,
                command,
                input,
            } = cmd;
            let mut app = app_clone.lock().await;
            if idx >= app.services.len() {
                continue;
            }
            match command.as_str() {
                "start" => start_service(idx, &mut app),
                "stop" => stop_service(idx, &mut app),
                "stdin" => {
                    let input = input.unwrap_or_default();
                    if let Some(writer) = app.services[idx].stdin_writer.clone() {
                        let mut writer_guard = writer.lock().await;
                        use tokio::io::AsyncWriteExt;
                        let input_with_newline = format!("{}\n", input);
                        if let Err(e) = writer_guard.write_all(input_with_newline.as_bytes()).await
                        {
                            app.services[idx]
                                .push_log(format!("[ERROR] Failed to write to stdin: {}", e));
                        }
                        if let Err(e) = writer_guard.flush().await {
                            app.services[idx]
                                .push_log(format!("[ERROR] Failed to flush stdin: {}", e));
                        }
                    }
                }
                _ => {}
            }
        }
    });

    tokio::select! {
        _ = (&mut send_task) => recv_task.abort(),
        _ = (&mut recv_task) => send_task.abort(),
    };
}
