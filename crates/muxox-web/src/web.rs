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
        Path, State,
        ws::{Message, WebSocket, WebSocketUpgrade},
    },
    http::{HeaderValue, StatusCode, header},
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
        .route("/logs/{idx}", get(export_logs))
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

/// Download one service's captured log. `idx` matches the web UI service index.
async fn export_logs(
    Path(idx): Path<usize>,
    State(state): State<Arc<WebState>>,
) -> impl IntoResponse {
    let app = state.app.lock().await;
    let Some(svc) = app.services.get(idx) else {
        return Err(StatusCode::NOT_FOUND);
    };
    let filename = log_download_name(&svc.cfg.name);
    let body = export_log_text(svc.log.iter().map(String::as_str));
    let disposition = format!("attachment; filename=\"{filename}\"");
    Ok((
        [
            (
                header::CONTENT_TYPE,
                HeaderValue::from_static("text/plain; charset=utf-8"),
            ),
            (
                header::CONTENT_DISPOSITION,
                HeaderValue::from_str(&disposition).expect("log filename is ascii"),
            ),
        ],
        body,
    ))
}

fn log_download_name(name: &str) -> String {
    let mut safe = String::new();
    for c in name.chars() {
        if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' {
            safe.push(c);
        } else {
            safe.push('_');
        }
    }
    let trimmed = safe.trim_matches(['.', '_']);
    let base = if trimmed.is_empty() {
        "service"
    } else {
        trimmed
    };
    format!("{base}.log")
}

/// Plain-text log body. ANSI color sequences are removed so the file matches
/// the readable text in the service shell.
fn export_log_text<'a>(lines: impl IntoIterator<Item = &'a str>) -> String {
    let mut body = String::new();
    for line in lines {
        body.push_str(&strip_ansi(line));
        body.push('\n');
    }
    body
}

fn strip_ansi(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let bytes = input.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == 0x1b && i + 1 < bytes.len() && bytes[i + 1] == b'[' {
            i += 2;
            while i < bytes.len() && !bytes[i].is_ascii_alphabetic() {
                i += 1;
            }
            if i < bytes.len() {
                i += 1;
            }
            continue;
        }
        let ch = input[i..].chars().next().unwrap();
        out.push(ch);
        i += ch.len_utf8();
    }
    out
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

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::to_bytes;
    use muxox_core::app::{App, AppMsg, ServiceState, Status};
    use muxox_core::config::ServiceCfg;
    use muxox_core::isolation::default_isolation;
    use tokio::sync::mpsc;

    fn service_with_logs(name: &str, lines: &[&str]) -> ServiceState {
        let mut svc = ServiceState::new(ServiceCfg {
            name: name.to_string(),
            cmd: "true".into(),
            cwd: None,
            log_capacity: 100,
            interactive: false,
            pty: false,
            env_file: None,
            isolation: Default::default(),
            hooks: Default::default(),
            monitor: None,
        });
        svc.status = Status::Running;
        for line in lines {
            svc.push_log(*line);
        }
        svc
    }

    fn state_with(services: Vec<ServiceState>) -> Arc<WebState> {
        let (tx, _rx) = mpsc::unbounded_channel::<AppMsg>();
        let (broadcast_tx, _) = broadcast::channel(8);
        let count = services.len();
        Arc::new(WebState {
            app: Arc::new(Mutex::new(App {
                services,
                selected: 0,
                log_offset_from_end: 0,
                tx,
                input_mode: false,
                input_buffer: String::new(),
                isolation: default_isolation(),
                update_notice: None,
                hooks: Default::default(),
                config_dir: None,
                settle_tx: None,
            })),
            broadcast_tx,
            resources: empty_resource_slots(count),
            ui_name: "test".into(),
        })
    }

    #[test]
    fn log_download_name_keeps_safe_characters() {
        assert_eq!(log_download_name("sandbox-broker"), "sandbox-broker.log");
        assert_eq!(log_download_name("a/b c"), "a_b_c.log");
        assert_eq!(log_download_name(".."), "service.log");
        assert_eq!(log_download_name(""), "service.log");
    }

    #[test]
    fn export_log_text_strips_ansi_and_keeps_lines() {
        let body = export_log_text(["plain", "\u{1b}[33mwarning\u{1b}[0m"]);
        assert_eq!(body, "plain\nwarning\n");
    }

    #[tokio::test]
    async fn export_logs_downloads_named_service_log() {
        let state = state_with(vec![
            service_with_logs("alpha", &["\u{1b}[32malpha-log-line\u{1b}[0m", "second"]),
            service_with_logs("beta", &["beta-log-line"]),
        ]);

        let response = export_logs(Path(0), State(Arc::clone(&state)))
            .await
            .into_response();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers().get(header::CONTENT_DISPOSITION).unwrap(),
            "attachment; filename=\"alpha.log\""
        );
        let body = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
        assert_eq!(body.as_ref(), b"alpha-log-line\nsecond\n");

        let beta = export_logs(Path(1), State(Arc::clone(&state)))
            .await
            .into_response();
        let body = to_bytes(beta.into_body(), 64 * 1024).await.unwrap();
        assert_eq!(body.as_ref(), b"beta-log-line\n");

        let missing = export_logs(Path(9), State(state)).await.into_response();
        assert_eq!(missing.status(), StatusCode::NOT_FOUND);
    }
}
