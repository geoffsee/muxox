// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Geoff Seemueller
// This file is part of muxox, released under the MIT License.

use crate::mcp::run_mcp_server;
use crate::resources::{Resources, SampleQuery, Sampler};
use crate::ui;
use crate::ws_proto::{ServiceInfo, WsInbound, WsOutbound};
use anyhow::Result;
use axum::{
    Router,
    body::Bytes,
    extract::{
        State,
        ws::{Message, WebSocket, WebSocketUpgrade},
    },
    response::{Html, IntoResponse},
    routing::get,
};
use futures_util::{SinkExt, StreamExt};
use muxox_core::app::{
    App, AppMsg, ServiceState, Status, apply_msg, drain_pending, shutdown, start_all,
    start_service, stop_service,
};
use muxox_core::config::{Config, display_name};
use muxox_core::isolation::default_isolation;
use muxox_core::signal::signal_watcher;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{Mutex, broadcast, mpsc};
use tokio::task;

pub async fn run_web_mode(cfg: Config, port: u16) -> Result<()> {
    let ui_name = display_name(&cfg).to_string();
    let Config {
        service,
        mcp,
        hooks,
        config_dir,
        ..
    } = cfg;
    let service_count = service.len();
    let (tx, mut rx) = mpsc::unbounded_channel::<AppMsg>();
    muxox_core::updates::spawn_update_check(tx.clone());
    // Bytes rather than Vec<u8>: broadcast clones the value once per
    // receiver, and cloning Bytes is a refcount bump instead of a full copy
    // of the frame for every connected client.
    let (b_tx, _) = broadcast::channel::<Bytes>(100);

    let app = Arc::new(Mutex::new(App {
        services: service.into_iter().map(ServiceState::new).collect(),
        selected: 0,
        log_offset_from_end: 0,
        tx: tx.clone(),
        input_mode: false,
        input_buffer: String::new(),
        isolation: default_isolation(),
        update_notice: None,
        hooks,
        config_dir,
        settle_tx: None,
    }));

    {
        let mut app_lock = app.lock().await;
        start_all(&mut app_lock).await;
        // Fold boot-time hook/spawn AppMsg into log buffers before any
        // WebSocket Init snapshot, so the UI shows hooks immediately.
        drain_pending(&mut app_lock, &mut rx);
    }

    // Signal watcher
    task::spawn(signal_watcher(tx.clone()));

    // One slot per service. The sampler fills it; Init, the stop path, and
    // MCP `inspect` read it.
    let resources: Arc<Mutex<Vec<Option<Resources>>>> = empty_resource_slots(service_count);

    // Optional embedded MCP server for agent inspection and start/stop.
    if mcp.enabled {
        let app_for_mcp = Arc::clone(&app);
        let resources_for_mcp = Arc::clone(&resources);
        task::spawn(async move {
            if let Err(e) = run_mcp_server(mcp, app_for_mcp, resources_for_mcp).await {
                eprintln!("MCP server error: {e}");
            }
        });
    }

    // Broadcast messages to all connected WebSockets
    let b_tx_clone = b_tx.clone();
    let app_for_msgs = Arc::clone(&app);
    let resources_for_msgs = Arc::clone(&resources);
    task::spawn(async move {
        while let Some(msg) = rx.recv().await {
            let mut app_lock = app_for_msgs.lock().await;
            apply_msg(&mut app_lock, msg.clone());

            if let AppMsg::Stopped(idx, _) = &msg {
                clear_resources(&resources_for_msgs, &b_tx_clone, *idx).await;
            }

            let ws_msg = match &msg {
                AppMsg::UpdateAvailable(update) => Some(WsOutbound::UpdateAvailable {
                    update: update.clone(),
                }),
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
                send_outbound(&b_tx_clone, &msg);
            }

            if matches!(msg, AppMsg::AbortedAll) {
                shutdown(&mut app_lock);
                std::process::exit(0);
            }
        }
    });

    spawn_resource_sampler(Arc::clone(&app), Arc::clone(&resources), Some(b_tx.clone()));

    let shared_state = Arc::new(WebState {
        app: Arc::clone(&app),
        broadcast_tx: b_tx,
        resources,
        ui_name,
    });

    let router = Router::new()
        .route("/", get(index_handler))
        .route("/ws", get(ws_handler))
        .with_state(shared_state);

    let addr: SocketAddr = format!("[::1]:{}", port).parse()?;
    let listener = tokio::net::TcpListener::bind(addr).await?;
    let actual_port = listener.local_addr()?.port();
    let url = format!("http://localhost:{}", actual_port);
    println!("Web UI available at {}", url);

    if let Err(e) = open::that(url) {
        eprintln!("Failed to open browser: {}", e);
    }

    axum::serve(listener, router).await?;

    Ok(())
}

struct WebState {
    app: Arc<Mutex<App>>,
    broadcast_tx: broadcast::Sender<Bytes>,
    resources: Arc<Mutex<Vec<Option<Resources>>>>,
    ui_name: String,
}

async fn index_handler(State(state): State<Arc<WebState>>) -> impl IntoResponse {
    Html(ui::render_index(&state.ui_name))
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
        let slots = state.resources.lock().await;
        let services: Vec<ServiceInfo> = app
            .services
            .iter()
            .enumerate()
            .map(|(idx, s)| ServiceInfo {
                name: s.cfg.name.clone(),
                status: s.status.as_str().to_owned(),
                logs: s.log.iter().cloned().collect(),
                interactive: s.cfg.interactive,
                resources: slots.get(idx).copied().flatten(),
            })
            .collect();
        let init_msg = WsOutbound::Init {
            services,
            update: app.update_notice.clone(),
        };
        if let Err(e) = sender.send(init_msg.to_message()).await {
            eprintln!("Failed to send init msg: {}", e);
            return;
        }
    }

    let mut send_task = task::spawn(async move {
        while let Ok(bytes) = b_rx.recv().await {
            if sender.send(Message::Binary(bytes)).await.is_err() {
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

fn send_outbound(tx: &broadcast::Sender<Bytes>, msg: &WsOutbound) {
    let _ = tx.send(Bytes::from(
        serde_json::to_vec(msg).expect("WsOutbound serialization cannot fail"),
    ));
}

/// Clear one service's sample and tell clients. Caller must already hold the
/// app lock, so this only takes the resource lock (app, then resources).
async fn clear_resources(
    resources: &Mutex<Vec<Option<Resources>>>,
    tx: &broadcast::Sender<Bytes>,
    idx: usize,
) {
    let mut slots = resources.lock().await;
    if let Some(slot) = slots.get_mut(idx)
        && slot.take().is_some()
    {
        send_outbound(
            tx,
            &WsOutbound::Resources {
                idx,
                resources: None,
            },
        );
    }
}

fn service_root(service: &ServiceState) -> Option<u32> {
    if matches!(service.status, Status::Running | Status::Stopping) {
        service.pid
    } else {
        None
    }
}

fn sample_query(service: &ServiceState) -> SampleQuery {
    let monitor = service.cfg.monitor.as_ref();
    SampleQuery {
        root: service_root(service),
        cpu: monitor.is_some_and(|cfg| cfg.cpu),
        memory: monitor.is_some_and(|cfg| cfg.memory),
        gpu: monitor.is_some_and(|cfg| cfg.gpu),
    }
}

pub fn empty_resource_slots(count: usize) -> Arc<Mutex<Vec<Option<Resources>>>> {
    Arc::new(Mutex::new(vec![None; count]))
}

/// Sample opted-in services about once a second.
///
/// Does nothing when no service has an enabled `monitor` table. A tick also
/// skips the process scan while no monitored service is running. CPU, memory,
/// and GPU are included only when some live service asked for that reading.
///
/// `tx` broadcasts header updates to web clients. Pass `None` when only the
/// shared slots are needed, as in `--raw` mode.
pub fn spawn_resource_sampler(
    app: Arc<Mutex<App>>,
    resources: Arc<Mutex<Vec<Option<Resources>>>>,
    tx: Option<broadcast::Sender<Bytes>>,
) {
    task::spawn(async move {
        {
            let app = app.lock().await;
            let enabled = app.services.iter().any(|service| {
                service
                    .cfg
                    .monitor
                    .as_ref()
                    .is_some_and(|cfg| cfg.enabled())
            });
            if !enabled {
                return;
            }
        }
        let mut sampler = match Sampler::new() {
            Ok(sampler) => sampler,
            Err(e) => {
                eprintln!("resource sampler unavailable: {e}");
                return;
            }
        };
        let mut ticker = tokio::time::interval(Duration::from_secs(1));
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            ticker.tick().await;
            let queries = {
                let app = app.lock().await;
                app.services.iter().map(sample_query).collect::<Vec<_>>()
            };
            if !queries.iter().any(|query| query.wants_refresh()) {
                continue;
            }
            let (sampler_back, samples) = match task::spawn_blocking(move || {
                let mut sampler = sampler;
                let samples = sampler.sample(&queries);
                (sampler, samples)
            })
            .await
            {
                Ok(pair) => pair,
                Err(_) => return,
            };
            sampler = sampler_back;

            // Re-check status under the app lock so a stop that landed during
            // the sample cannot be overwritten by a stale tree.
            let app = app.lock().await;
            let mut slots = resources.lock().await;
            for (idx, sample) in samples.into_iter().enumerate() {
                let live = app
                    .services
                    .get(idx)
                    .is_some_and(|service| sample_query(service).wants_refresh());
                let next = if live { sample } else { None };
                let Some(slot) = slots.get_mut(idx) else {
                    continue;
                };
                if *slot == next {
                    continue;
                }
                *slot = next;
                if let Some(tx) = &tx {
                    send_outbound(
                        tx,
                        &WsOutbound::Resources {
                            idx,
                            resources: next,
                        },
                    );
                }
            }
        }
    });
}
