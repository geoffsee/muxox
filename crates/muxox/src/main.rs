// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Geoff Seemueller
// This file is part of muxox, released under the MIT License.

use std::{
    collections::VecDeque,
    fs,
    fs::OpenOptions,
    io,
    net::SocketAddr,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::Arc,
    time::Duration,
};

use anyhow::{Context, Result};
use axum::{
    Router,
    extract::{
        State,
        ws::{Message, WebSocket, WebSocketUpgrade},
    },
    response::{Html, IntoResponse},
    routing::get,
};
use clap::Parser;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyModifiers, MouseEvent, MouseEventKind};
use crossterm::terminal::{disable_raw_mode, enable_raw_mode};
use futures_util::{SinkExt, StreamExt};
use ratatui::{
    Terminal,
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Wrap},
};
use serde::{Deserialize, Serialize};
#[cfg(windows)]
use std::os::windows::process::CommandExt as _;
use tokio::{
    io::AsyncBufReadExt,
    process::{ChildStdin, Command as AsyncCommand},
    sync::{Mutex, broadcast, mpsc},
    task, time,
};

mod web_ui;
mod ws_proto;

use ws_proto::{ServiceInfo, WsInbound, WsOutbound};

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

#[derive(Debug, Deserialize, Clone)]
struct Config {
    service: Vec<ServiceCfg>,
}

#[derive(Debug, Deserialize, Clone)]
struct ServiceCfg {
    name: String,
    cmd: String,
    cwd: Option<PathBuf>,
    /// Keep last N log lines in memory
    #[serde(default = "default_log_capacity")]
    log_capacity: usize,
    /// Whether the service is interactive (requires stdin)
    #[serde(default)]
    interactive: bool,
    /// Whether to allocate a PTY for this interactive service (Unix only)
    #[serde(default)]
    pty: bool,
}
fn default_log_capacity() -> usize {
    2000
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

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

        let cfg: ServiceCfg = toml::from_str(toml_input).expect("Valid ServiceCfg");
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

        let cfg: ServiceCfg = toml::from_str(toml_input).expect("Valid ServiceCfg");
        assert_eq!(cfg.log_capacity, default_log_capacity());
    }

    #[test]
    fn test_config_deserialize() {
        let toml_input = r#"
            [[service]]
            name = "test-stdin"
            cmd = "./test-stdin.sh"
            cwd = "./example"
            log_capacity = 250
            interactive = true

            [[service]]
            name = "example-service-1"
            cmd = "bun ./index.ts"
            cwd = "example/packages/example-service-1"
            log_capacity = 5000
        "#;

        let cfg: Config = toml::from_str(toml_input).expect("Valid Config");
        assert_eq!(cfg.service.len(), 2);

        assert_eq!(cfg.service[0].name, "test-stdin");
        assert_eq!(cfg.service[0].cmd, "./test-stdin.sh");
        assert_eq!(cfg.service[0].cwd, Some(PathBuf::from("./example")));
        assert_eq!(cfg.service[0].log_capacity, 250);
        assert_eq!(cfg.service[0].interactive, true);

        assert_eq!(cfg.service[1].name, "example-service-1");
        assert_eq!(cfg.service[1].cmd, "bun ./index.ts");
        assert_eq!(
            cfg.service[1].cwd,
            Some(PathBuf::from("example/packages/example-service-1"))
        );
        assert_eq!(cfg.service[1].log_capacity, 5000);
    }

    #[test]
    fn test_service_state() {
        let cfg = ServiceCfg {
            name: "test".to_string(),
            cmd: "echo hello".to_string(),
            cwd: None,
            log_capacity: 10,
            interactive: false,
            pty: false,
        };

        let mut state = ServiceState::new(cfg.clone());

        // Test initial state
        assert_eq!(state.status, Status::Stopped);
        assert!(state.child.is_none());
        assert_eq!(state.log.len(), 0);
        // The ServiceState::new function enforces a minimum capacity of 256
        assert_eq!(state.log.capacity(), 256);

        // Test log functionality
        state.push_log("line 1");
        state.push_log("line 2");
        assert_eq!(state.log.len(), 2);

        // Test log capacity
        for i in 3..=12 {
            state.push_log(format!("line {}", i));
        }

        // Should have removed oldest entries to stay within capacity
        assert_eq!(state.log.len(), 10);
        assert_eq!(state.log.front().unwrap(), "line 3");
        assert_eq!(state.log.back().unwrap(), "line 12");
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
enum Status {
    Stopped,
    Starting,
    Running,
    Stopping,
}

impl Status {
    fn as_str(self) -> &'static str {
        match self {
            Self::Stopped => "Stopped",
            Self::Starting => "Starting",
            Self::Running => "Running",
            Self::Stopping => "Stopping",
        }
    }
}

#[derive(Debug)]
struct ServiceState {
    cfg: ServiceCfg,
    status: Status,
    child: Option<Child>,
    pid: Option<u32>,
    log: VecDeque<String>,
    stdin_tx: Option<mpsc::Sender<String>>,
    stdin_writer: Option<Arc<Mutex<ChildStdin>>>,
}

impl ServiceState {
    fn new(cfg: ServiceCfg) -> Self {
        Self {
            log: VecDeque::with_capacity(cfg.log_capacity.max(256)),
            cfg,
            status: Status::Stopped,
            child: None,
            pid: None,
            stdin_tx: None,
            stdin_writer: None,
        }
    }
    fn push_log(&mut self, line: impl Into<String>) {
        if self.log.len() == self.cfg.log_capacity {
            self.log.pop_front();
        }
        self.log.push_back(line.into());
    }
}

#[derive(Debug)]
struct App {
    services: Vec<ServiceState>,
    selected: usize,
    // Number of lines from the end of the log to offset when rendering.
    // 0 means follow the tail; higher values scroll up into older logs.
    log_offset_from_end: u16,
    tx: mpsc::UnboundedSender<AppMsg>,
    // Input mode for interactive services
    input_mode: bool,
    input_buffer: String,
}

#[derive(Debug, Clone)]
enum AppMsg {
    Started(usize),
    Stopped(usize, i32),
    Log(usize, String),
    ChildSpawned(usize, u32),
    StdinReady(usize, mpsc::Sender<String>),
    StdinWriterReady(usize, Arc<Mutex<ChildStdin>>),
    AbortedAll,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let cfg = load_config(cli.config.as_deref())?;

    if cli.raw {
        // Run in raw streaming mode
        run_raw_mode(cfg).await
    } else if cli.tui {
        // Run in TUI mode
        run_tui_mode(cfg).await
    } else {
        // Run in Web UI mode
        run_web_mode(cfg, cli.port).await
    }
}

async fn run_web_mode(cfg: Config, port: u16) -> Result<()> {
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

async fn run_raw_mode(cfg: Config) -> Result<()> {
    let (tx, mut rx) = mpsc::unbounded_channel::<AppMsg>();
    let mut app = App {
        services: cfg.service.into_iter().map(ServiceState::new).collect(),
        selected: 0,
        log_offset_from_end: 0,
        tx: tx.clone(),
        input_mode: false,
        input_buffer: String::new(),
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

async fn run_tui_mode(cfg: Config) -> Result<()> {
    let (tx, mut rx) = mpsc::unbounded_channel::<AppMsg>();
    let mut app = App {
        services: cfg.service.into_iter().map(ServiceState::new).collect(),
        selected: 0,
        log_offset_from_end: 0,
        tx: tx.clone(),
        input_mode: false,
        input_buffer: String::new(),
    };

    // Signal watcher: on any exit signal, nuke children then exit.
    task::spawn(signal_watcher(tx.clone()));

    // TUI setup
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    crossterm::execute!(stdout, crossterm::terminal::EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // Start all services automatically (like in raw mode)
    for idx in 0..app.services.len() {
        start_service(idx, &mut app);
    }

    // Render loop
    let ui_task = task::spawn(async move {
        let mut last_tick = time::Instant::now();
        let tick_rate = Duration::from_millis(150);
        loop {
            // Draw
            terminal.draw(|f| draw_ui(f, &app)).ok();

            // Input or tick
            let timeout = tick_rate.saturating_sub(last_tick.elapsed());
            let mut handled = false;
            if event::poll(timeout).unwrap_or(false) {
                match event::read().unwrap_or(Event::FocusGained) {
                    Event::Key(k) => {
                        handled = handle_key(k, &mut app);
                    }
                    Event::Mouse(m) => {
                        // Only handle scroll events, ignore mouse movements
                        if matches!(
                            m.kind,
                            MouseEventKind::ScrollUp | MouseEventKind::ScrollDown
                        ) {
                            handled = handle_mouse(m, &mut app);
                        }
                    }
                    _ => {}
                }
            }
            if handled { /* app mutated, redraw next loop */ }
            if last_tick.elapsed() >= tick_rate {
                last_tick = time::Instant::now();
            }

            // Drain channel
            while let Ok(msg) = rx.try_recv() {
                if matches!(msg, AppMsg::AbortedAll) {
                    cleanup_and_exit(&mut app);
                }
                apply_msg(&mut app, msg);
            }
        }
    });

    // Wait until UI task ends (it never does gracefully). If it errors, fallthrough.
    let _ = ui_task.await;
    Ok(())
}

fn draw_ui(f: &mut ratatui::Frame, app: &App) {
    // Split screen to add input area at bottom if in input mode
    let main_chunks = if app.input_mode {
        Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(1), Constraint::Length(3)])
            .split(f.area())
    } else {
        Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(1)])
            .split(f.area())
    };

    // Clear all drawing areas to prevent visual artifacts when layout changes
    f.render_widget(Clear, main_chunks[0]);
    if app.input_mode {
        f.render_widget(Clear, main_chunks[1]);
    }

    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(30), Constraint::Percentage(70)])
        .split(main_chunks[0]);

    let items: Vec<ListItem> = app
        .services
        .iter()
        .enumerate()
        .map(|(_i, s)| {
            let status = match s.status {
                Status::Stopped => "●",
                Status::Starting => "◔",
                Status::Running => "◉",
                Status::Stopping => "◑",
            };
            let color = match s.status {
                Status::Running => Color::Green,
                Status::Starting => Color::Yellow,
                Status::Stopping => Color::Magenta,
                Status::Stopped => Color::DarkGray,
            };
            ListItem::new(Line::from(vec![
                Span::styled(format!(" {status} "), Style::default().fg(color)),
                Span::raw(&s.cfg.name),
            ]))
        })
        .collect();

    let list = List::new(items)
        .block(
            Block::default()
                .title("Services  (↑/↓ select, i input mode, Space start/stop, r restart, c clear, q quit)")
                .borders(Borders::ALL),
        )
        .highlight_style(
            Style::default()
                .add_modifier(Modifier::BOLD)
                .bg(Color::DarkGray),
        );

    f.render_stateful_widget(list, chunks[0], &mut list_state(app.selected));

    // Right pane: logs
    let right_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Min(1)])
        .split(chunks[1]);

    let selected = &app.services[app.selected];
    let header = Paragraph::new(vec![
        Line::from(vec![
            Span::styled("Name: ", Style::default().fg(Color::DarkGray)),
            Span::raw(&selected.cfg.name),
        ]),
        Line::from(vec![
            Span::styled("Cmd:  ", Style::default().fg(Color::DarkGray)),
            Span::raw(&selected.cfg.cmd),
        ]),
        Line::from(vec![
            Span::styled("Cwd:  ", Style::default().fg(Color::DarkGray)),
            Span::raw(
                selected
                    .cfg
                    .cwd
                    .as_ref()
                    .and_then(|p| p.to_str())
                    .unwrap_or("."),
            ),
        ]),
    ])
    .block(
        Block::default()
            .title("Selected Service")
            .borders(Borders::ALL),
    );

    let log_area = right_chunks[1];
    let inner_height = log_area.height.saturating_sub(2); // account for borders
    let total_lines = selected.log.len() as u16;
    let max_top = total_lines.saturating_sub(inner_height);
    let offset_from_end = app.log_offset_from_end.min(max_top);
    let scroll_top = max_top.saturating_sub(offset_from_end);

    let log_text: Vec<Line> = selected.log.iter().map(|l| ansi_to_line(l)).collect();
    let log = Paragraph::new(log_text)
        .wrap(Wrap { trim: false })
        .scroll((scroll_top, 0))
        .block(
            Block::default()
                .title(format!("Logs - {}", selected.cfg.name))
                .borders(Borders::ALL),
        );

    f.render_widget(header, right_chunks[0]);
    f.render_widget(log, log_area);

    // Render input bar if in input mode
    if app.input_mode {
        let input_widget = Paragraph::new(format!("> {}", app.input_buffer))
            .style(Style::default().fg(Color::Yellow))
            .block(
                Block::default()
                    .title("Input Mode (ESC to exit, Enter to send)")
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(Color::Yellow)),
            );
        f.render_widget(input_widget, main_chunks[1]);
    }
}

fn list_state(selected: usize) -> ratatui::widgets::ListState {
    let mut state = ratatui::widgets::ListState::default();
    state.select(Some(selected));
    state
}

fn handle_key(k: KeyEvent, app: &mut App) -> bool {
    // Input mode handling
    if app.input_mode {
        match k.code {
            KeyCode::Esc => {
                // Exit input mode
                app.input_mode = false;
                app.input_buffer.clear();
                return true;
            }
            KeyCode::Enter => {
                // Send input to selected service (only if buffer is non-empty)
                let idx = app.selected;
                if !app.input_buffer.is_empty() {
                    let input = app.input_buffer.clone();
                    app.services[idx].push_log(format!("[DEBUG] Sending input: {:?}", input));
                    if let Some(stdin_writer) = app.services[idx].stdin_writer.clone() {
                        // Sanity logs mirroring the previous channel-based path
                        app.services[idx]
                            .push_log(format!("[DEBUG] Input queued successfully: {:?}", input));
                        app.services[idx].push_log(
                            "[DEBUG] Post-queue sanity: after queuing for direct writer"
                                .to_string(),
                        );
                        app.services[idx]
                            .push_log("[DEBUG] About to spawn direct writer task".to_string());
                        let input_for_task = input.clone();
                        let tx = app.tx.clone();
                        let idx_for_task = idx;
                        // Attempt write via async task
                        #[cfg(unix)]
                        task::spawn(async move {
                            debug_file_log("spawn: task entered");
                            let _ = tx.send(AppMsg::Log(
                                idx_for_task,
                                format!(
                                    "[DEBUG] Direct write task started for: {:?}",
                                    input_for_task
                                ),
                            ));
                            use std::os::unix::io::{AsRawFd, BorrowedFd};
                            let mut guard = stdin_writer.lock().await;
                            debug_file_log("spawn: acquired lock");
                            let _ = tx.send(AppMsg::Log(
                                idx_for_task,
                                "[DEBUG] Direct write acquired lock".to_string(),
                            ));
                            let fd = guard.as_raw_fd();
                            // Perform a direct, blocking write to the child's stdin fd.
                            let mut total = 0;
                            // Send CR to mimic Enter on a TTY; many readline-based CLIs expect "\r".
                            // Some also accept LF; if needed we can append "\n" later.
                            let data = format!("{}\r", input_for_task);
                            let bytes = data.as_bytes();
                            loop {
                                let borrowed = unsafe { BorrowedFd::borrow_raw(fd) };
                                match nix::unistd::write(borrowed, &bytes[total..]) {
                                    Ok(n) => {
                                        total += n;
                                        if total >= bytes.len() {
                                            break;
                                        }
                                    }
                                    Err(e) => {
                                        debug_file_log(&format!("spawn: nix::write failed: {}", e));
                                        let _ = tx.send(AppMsg::Log(
                                            idx_for_task,
                                            format!("[DEBUG] Direct write (fd) failed: {}", e),
                                        ));
                                        return;
                                    }
                                }
                            }
                            debug_file_log("spawn: write success");
                            let _ = tx.send(AppMsg::Log(
                                idx_for_task,
                                format!("[DEBUG] Direct write success ({} bytes)", total),
                            ));
                        });
                        #[cfg(windows)]
                        task::spawn(async move {
                            debug_file_log("spawn: task entered");
                            let _ = tx.send(AppMsg::Log(
                                idx_for_task,
                                format!(
                                    "[DEBUG] Direct write task started for: {:?}",
                                    input_for_task
                                ),
                            ));
                            use tokio::io::AsyncWriteExt;
                            let mut guard = stdin_writer.lock().await;
                            debug_file_log("spawn: acquired lock");
                            let _ = tx.send(AppMsg::Log(
                                idx_for_task,
                                "[DEBUG] Direct write acquired lock".to_string(),
                            ));
                            let data = format!("{}\r", input_for_task);
                            match guard.write_all(data.as_bytes()).await {
                                Ok(()) => {
                                    let _ = guard.flush().await;
                                    debug_file_log("spawn: write success");
                                    let _ = tx.send(AppMsg::Log(
                                        idx_for_task,
                                        format!(
                                            "[DEBUG] Direct write success ({} bytes)",
                                            data.len()
                                        ),
                                    ));
                                }
                                Err(e) => {
                                    debug_file_log(&format!("spawn: write failed: {}", e));
                                    let _ = tx.send(AppMsg::Log(
                                        idx_for_task,
                                        format!("[DEBUG] Direct write failed: {}", e),
                                    ));
                                }
                            }
                        });
                        // Also attempt write on a plain OS thread in case the async task is starved
                        {
                            let tx = app.tx.clone();
                            let stdin_writer = app.services[idx].stdin_writer.clone().unwrap();
                            let input_for_thread = input.clone();
                            let idx_for_thread = idx;
                            #[cfg(unix)]
                            std::thread::spawn(move || {
                                debug_file_log("thread: started");
                                use std::os::unix::io::{AsRawFd, BorrowedFd};
                                // Try to acquire lock a few times without async runtime
                                for _ in 0..50 {
                                    if let Ok(guard) = stdin_writer.try_lock() {
                                        let fd = guard.as_raw_fd();
                                        let data = format!("{}\r", input_for_thread);
                                        let mut total = 0usize;
                                        let bytes = data.as_bytes();
                                        loop {
                                            let borrowed = unsafe { BorrowedFd::borrow_raw(fd) };
                                            match nix::unistd::write(borrowed, &bytes[total..]) {
                                                Ok(n) => {
                                                    total += n;
                                                    if total >= bytes.len() {
                                                        break;
                                                    }
                                                }
                                                Err(e) => {
                                                    debug_file_log(&format!(
                                                        "thread: nix::write failed: {}",
                                                        e
                                                    ));
                                                    let _ = tx.send(AppMsg::Log(idx_for_thread, format!("[DEBUG] Direct write (thread) failed: {}", e)));
                                                    return;
                                                }
                                            }
                                        }
                                        debug_file_log("thread: write success");
                                        let _ = tx.send(AppMsg::Log(
                                            idx_for_thread,
                                            format!(
                                                "[DEBUG] Direct write (thread) success ({} bytes)",
                                                total
                                            ),
                                        ));
                                        return;
                                    }
                                    std::thread::sleep(std::time::Duration::from_millis(5));
                                }
                                let _ = tx.send(AppMsg::Log(
                                    idx_for_thread,
                                    "[DEBUG] Direct write (thread) could not acquire lock"
                                        .to_string(),
                                ));
                            });
                            #[cfg(windows)]
                            std::thread::spawn(move || {
                                debug_file_log("thread: started");
                                use std::io::Write;
                                for _ in 0..50 {
                                    if let Ok(mut guard) = stdin_writer.try_lock() {
                                        let data = format!("{}\r", input_for_thread);
                                        match guard.write_all(data.as_bytes()) {
                                            Ok(()) => {
                                                let _ = guard.flush();
                                                debug_file_log("thread: write success");
                                                let _ = tx.send(AppMsg::Log(
                                                    idx_for_thread,
                                                    format!(
                                                        "[DEBUG] Direct write (thread) success ({} bytes)",
                                                        data.len()
                                                    ),
                                                ));
                                            }
                                            Err(e) => {
                                                debug_file_log(&format!(
                                                    "thread: write failed: {}",
                                                    e
                                                ));
                                                let _ = tx.send(AppMsg::Log(
                                                    idx_for_thread,
                                                    format!(
                                                        "[DEBUG] Direct write (thread) failed: {}",
                                                        e
                                                    ),
                                                ));
                                            }
                                        }
                                        return;
                                    }
                                    std::thread::sleep(std::time::Duration::from_millis(5));
                                }
                                let _ = tx.send(AppMsg::Log(
                                    idx_for_thread,
                                    "[DEBUG] Direct write (thread) could not acquire lock"
                                        .to_string(),
                                ));
                            });
                        }
                        app.services[idx]
                            .push_log("[DEBUG] Spawned direct writer task".to_string());
                    } else {
                        app.services[idx]
                            .push_log("[DEBUG] stdin_writer not available yet!".to_string());
                    }
                    app.input_buffer.clear();
                } else {
                    app.services[idx].push_log("[DEBUG] Skipping empty input".to_string());
                }
                return true;
            }
            KeyCode::Backspace => {
                // Remove last character
                app.input_buffer.pop();
                return true;
            }
            KeyCode::Char(c) => {
                // Add character to buffer
                app.input_buffer.push(c);
                return true;
            }
            _ => {}
        }
        return false;
    }

    // Normal mode handling
    match (k.code, k.modifiers) {
        (KeyCode::Char('q'), _) | (KeyCode::Esc, _) => {
            // Quit: perform cleanup
            cleanup_and_exit(app);
            return true;
        }
        (KeyCode::Char('i'), _) => {
            // Enter input mode for interactive services
            let idx = app.selected;
            if app.services[idx].cfg.interactive && app.services[idx].stdin_writer.is_some() {
                app.input_mode = true;
                app.input_buffer.clear();
            }
            return true;
        }
        (KeyCode::Down, _) => {
            app.selected = (app.selected + 1).min(app.services.len() - 1);
            // Reset log scroll to follow tail when changing selection
            app.log_offset_from_end = 0;
            return true;
        }
        (KeyCode::Up, _) => {
            if app.selected > 0 {
                app.selected -= 1;
            }
            // Reset log scroll to follow tail when changing selection
            app.log_offset_from_end = 0;
            return true;
        }
        (KeyCode::Char(' '), _) => {
            toggle_selected(app);
            return true;
        }
        (KeyCode::Char('r'), _) => {
            restart_selected(app);
            return true;
        }
        (KeyCode::Char('c'), _) if k.modifiers == KeyModifiers::NONE => {
            app.services[app.selected].log.clear();
            app.log_offset_from_end = 0;
            return true;
        }
        (KeyCode::PageUp, _) => {
            // Scroll up older logs
            app.log_offset_from_end = app.log_offset_from_end.saturating_add(10);
            return true;
        }
        (KeyCode::PageDown, _) => {
            // Scroll down towards the latest logs
            app.log_offset_from_end = app.log_offset_from_end.saturating_sub(10);
            return true;
        }
        (KeyCode::Home, _) => {
            // Jump to top
            app.log_offset_from_end = u16::MAX;
            return true;
        }
        (KeyCode::End, _) => {
            // Jump to bottom (follow tail)
            app.log_offset_from_end = 0;
            return true;
        }
        _ => {}
    }
    false
}

fn toggle_selected(app: &mut App) {
    let idx = app.selected;
    let status = app.services[idx].status;
    app.services[idx].push_log(format!(
        "[DEBUG] toggle_selected called, status: {:?}",
        status
    ));
    match status {
        Status::Stopped => {
            app.services[idx].push_log("[DEBUG] Starting service from toggle".to_string());
            start_service(idx, app);
        }
        Status::Running | Status::Starting => {
            app.services[idx].push_log("[DEBUG] Stopping service from toggle".to_string());
            stop_service(idx, app);
        }
        Status::Stopping => {}
    }
}

fn restart_selected(app: &mut App) {
    let idx = app.selected;
    stop_service(idx, app);
    start_service(idx, app);
}

fn start_service(idx: usize, app: &mut App) {
    if matches!(app.services[idx].status, Status::Running | Status::Starting) {
        return;
    }
    app.services[idx].status = Status::Starting;
    let tx = app.tx.clone();
    let sc = app.services[idx].cfg.clone();

    task::spawn(async move {
        // Build command; optionally wrap interactive with a PTY when requested
        #[cfg(unix)]
        let mut cmd = {
            if sc.interactive && sc.pty {
                let mut c = AsyncCommand::new(interactive_program());
                for a in interactive_args(&sc.cmd) {
                    c.arg(a);
                }
                let _ = tx.send(AppMsg::Log(
                    idx,
                    "[DEBUG] Launching interactive via PTY wrapper (script)".to_string(),
                ));
                c
            } else {
                let mut c = AsyncCommand::new(shell_program());
                c.arg(shell_flag()).arg(shell_exec(&sc.cmd));
                c
            }
        };
        #[cfg(not(unix))]
        let mut cmd = {
            let mut c = AsyncCommand::new(shell_program());
            c.arg(shell_flag()).arg(shell_exec(&sc.cmd));
            c
        };
        if let Some(cwd) = sc.cwd.clone() {
            cmd.current_dir(cwd);
        }
        cmd.env("FORCE_COLOR", "1")
            .env("CLICOLOR_FORCE", "1")
            .env("TERM", "xterm-256color")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        // Interactive services need stdin piped so we can send input
        if sc.interactive {
            cmd.stdin(Stdio::piped());
        }

        set_process_group(&mut cmd);

        match cmd.spawn() {
            Ok(mut child) => {
                let pid = child.id().unwrap_or_default();
                let _ = tx.send(AppMsg::Started(idx));

                // Send the child handle back to be stored
                let _ = tx.send(AppMsg::ChildSpawned(idx, pid));

                // For interactive services, setup stdin forwarding
                if sc.interactive {
                    let _ = tx.send(AppMsg::Log(
                        idx,
                        "[DEBUG] Service is interactive, setting up stdin forwarding".to_string(),
                    ));
                    if let Some(stdin) = child.stdin.take() {
                        let _ = tx.send(AppMsg::Log(
                            idx,
                            "[DEBUG] child.stdin is available".to_string(),
                        ));
                        // Publish a direct-writer handle (Arc<Mutex<ChildStdin>>) back to the app
                        let writer = Arc::new(Mutex::new(stdin));
                        let _ = tx.send(AppMsg::StdinWriterReady(idx, writer));
                        let _ = tx.send(AppMsg::Log(
                            idx,
                            "[DEBUG] StdinWriterReady message sent".to_string(),
                        ));
                    }
                }

                // stdout
                if let Some(out) = child.stdout.take() {
                    let tx2 = tx.clone();
                    let is_pty = sc.pty;
                    task::spawn(async move {
                        stream_log_realtime(idx, out, tx2, false, is_pty).await;
                    });
                }
                // stderr
                if let Some(err) = child.stderr.take() {
                    let tx2 = tx.clone();
                    let is_pty = sc.pty;
                    task::spawn(async move {
                        stream_log_realtime(idx, err, tx2, true, is_pty).await;
                    });
                }

                // Waiter
                let status = child.wait().await; // process exit
                let code = status.map(|s| s.code().unwrap_or(-1)).unwrap_or(-1);
                let _ = tx.send(AppMsg::Stopped(idx, code));
            }
            Err(e) => {
                let _ = tx.send(AppMsg::Log(idx, format!("spawn failed: {e}")));
                let _ = tx.send(AppMsg::Stopped(idx, -1));
            }
        }
    });
}

async fn stream_log_realtime<R>(
    idx: usize,
    stream: R,
    tx: mpsc::UnboundedSender<AppMsg>,
    is_stderr: bool,
    is_pty: bool,
) where
    R: tokio::io::AsyncRead + Unpin,
{
    let mut lines = tokio::io::BufReader::new(stream).lines();
    let mut buffer = String::new();
    let mut last_flush = tokio::time::Instant::now();

    while let Ok(Some(line)) = lines.next_line().await {
        // Handle carriage returns for progress indicators (keep this for compatibility)
        let mut last_nonempty: Option<&str> = None;
        for piece in line.split('\r') {
            if !piece.is_empty() {
                last_nonempty = Some(piece);
            }
        }
        let piece = last_nonempty.unwrap_or(&line);
        if piece.is_empty() {
            continue;
        }

        // For PTY mode, buffer single characters
        if is_pty && piece.len() <= 2 {
            buffer.push_str(piece);

            // Flush if buffer is getting large or timeout
            let now = tokio::time::Instant::now();
            if buffer.len() >= 80 || now.duration_since(last_flush).as_millis() > 100 {
                if !buffer.is_empty() {
                    if is_stderr {
                        let _ = tx.send(AppMsg::Log(idx, format!("ERR: {}", buffer)));
                    } else {
                        let _ = tx.send(AppMsg::Log(idx, buffer.clone()));
                    }
                    buffer.clear();
                    last_flush = now;
                }
            }
        } else {
            // For longer lines or non-PTY mode, flush buffer then send line
            if !buffer.is_empty() {
                buffer.push_str(piece);
                if is_stderr {
                    let _ = tx.send(AppMsg::Log(idx, format!("ERR: {}", buffer)));
                } else {
                    let _ = tx.send(AppMsg::Log(idx, buffer.clone()));
                }
                buffer.clear();
                last_flush = tokio::time::Instant::now();
            } else {
                // Simple pass-through for non-PTY or longer lines
                if is_stderr {
                    let _ = tx.send(AppMsg::Log(idx, format!("ERR: {}", piece)));
                } else {
                    let _ = tx.send(AppMsg::Log(idx, piece.to_string()));
                }
            }
        }
    }

    // Flush any remaining buffer
    if !buffer.is_empty() {
        if is_stderr {
            let _ = tx.send(AppMsg::Log(idx, format!("ERR: {}", buffer)));
        } else {
            let _ = tx.send(AppMsg::Log(idx, buffer));
        }
    }
}

fn stop_service(idx: usize, app: &mut App) {
    let sc = &mut app.services[idx];
    if !matches!(sc.status, Status::Running | Status::Starting) {
        return;
    }
    sc.status = Status::Stopping;
    sc.push_log("Stopping...");
    if let Some(child) = sc.child.take() {
        drop(child);
    } // actual kill handled by kill_tree below
    kill_tree(idx, app);
}

fn apply_msg(app: &mut App, msg: AppMsg) {
    match msg {
        AppMsg::Started(i) => {
            app.services[i].status = Status::Running;
            app.services[i].push_log("[started]");
        }
        AppMsg::Stopped(i, code) => {
            let s = &mut app.services[i];
            s.status = Status::Stopped;
            s.pid = None; // Clear PID when process stops
            s.stdin_tx = None; // Clear stdin channel when process stops
            s.stdin_writer = None; // Clear direct writer when process stops
            s.push_log(format!("[exited: code {code}]").as_str());
        }
        AppMsg::Log(i, line) => {
            app.services[i].push_log(line);
        }
        AppMsg::ChildSpawned(i, pid) => {
            app.services[i].pid = Some(pid);
            app.services[i].push_log(format!("[process started with PID {pid}]"));
        }
        AppMsg::StdinReady(i, stdin_tx) => {
            app.services[i]
                .push_log("[DEBUG] StdinReady received in apply_msg, storing stdin_tx".to_string());
            app.services[i].stdin_tx = Some(stdin_tx);
            app.services[i].push_log("[DEBUG] stdin_tx stored successfully".to_string());
        }
        AppMsg::StdinWriterReady(i, writer) => {
            app.services[i].stdin_writer = Some(writer);
            app.services[i].push_log("[DEBUG] stdin_writer stored successfully".to_string());
        }
        AppMsg::AbortedAll => {
            // No action needed for UI state, handled by modes
        }
    }
}

fn cleanup_and_exit(app: &mut App) {
    // Restore terminal first to avoid leaving it raw if we panic later.
    let _ = disable_raw_mode();
    let mut stdout = io::stdout();
    let _ = crossterm::execute!(stdout, crossterm::terminal::LeaveAlternateScreen);

    // Kill all children forcefully
    kill_all(app);

    std::process::exit(0);
}

fn kill_all(app: &mut App) {
    for i in 0..app.services.len() {
        kill_tree(i, app);
    }
}

fn load_config(provided: Option<&Path>) -> Result<Config> {
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

#[cfg(unix)]
fn set_process_group(cmd: &mut AsyncCommand) {
    unsafe {
        cmd.pre_exec(|| {
            let _ =
                nix::unistd::setpgid(nix::unistd::Pid::from_raw(0), nix::unistd::Pid::from_raw(0));
            Ok(())
        });
    }
}
#[cfg(windows)]
fn set_process_group(cmd: &mut AsyncCommand) {
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x00000200;
    const CREATE_NEW_CONSOLE: u32 = 0x00000010; // better isolation for signals
    cmd.creation_flags(CREATE_NEW_PROCESS_GROUP | CREATE_NEW_CONSOLE);
}

#[cfg(unix)]
fn shell_program() -> &'static str {
    // Use a plain POSIX shell to avoid login-shell quirks
    "/bin/sh"
}
#[cfg(unix)]
fn shell_flag() -> &'static str {
    // Use non-login shell to avoid stdin quirks
    "-c"
}
#[cfg(unix)]
fn shell_exec(cmd: &str) -> String {
    cmd.to_string()
}

#[cfg(windows)]
fn shell_program() -> &'static str {
    "cmd.exe"
}
#[cfg(windows)]
fn shell_flag() -> &'static str {
    "/C"
}
#[cfg(windows)]
fn shell_exec(cmd: &str) -> String {
    cmd.to_string()
}

// For Unix interactive services, run the command under a PTY using `script`.
// This makes tools that require a TTY (e.g., readline-based CLIs) behave properly.
#[cfg(unix)]
fn interactive_program() -> &'static str {
    // Rely on PATH lookup for `script` (macOS: /usr/bin/script)
    "script"
}
#[cfg(unix)]
fn interactive_args(cmd: &str) -> Vec<String> {
    vec![
        "-q".to_string(),
        "/dev/null".to_string(),
        shell_program().to_string(),
        shell_flag().to_string(),
        shell_exec(cmd),
    ]
}

#[cfg(unix)]
fn kill_tree(idx: usize, app: &mut App) {
    use nix::sys::signal::{Signal, killpg};
    use nix::unistd::Pid;
    let name = app.services[idx].cfg.name.clone();

    if let Some(pid) = app.services[idx].pid {
        // Kill the specific process group using the stored PID
        let pgid = Pid::from_raw(pid as i32);

        // First try TERM signal to allow graceful shutdown
        if let Err(_) = killpg(pgid, Signal::SIGTERM) {
            // If killpg fails (process group doesn't exist), try killing the process directly
            let _ = nix::sys::signal::kill(pgid, Signal::SIGTERM);
        }

        // Wait a bit for graceful shutdown
        std::thread::sleep(Duration::from_millis(250));

        // Then force kill if still running
        if let Err(_) = killpg(pgid, Signal::SIGKILL) {
            // If killpg fails, try killing the process directly
            let _ = nix::sys::signal::kill(pgid, Signal::SIGKILL);
        }

        app.services[idx].push_log(format!("[killed {name} (PID {pid})]"));
    } else {
        // Fallback to pattern matching if no PID is stored (shouldn't happen with new code)
        let cmdline = &app.services[idx].cfg.cmd;
        let _ = Command::new("pkill")
            .arg("-TERM")
            .arg("-f")
            .arg(cmdline)
            .status();
        std::thread::sleep(Duration::from_millis(250));
        let _ = Command::new("pkill")
            .arg("-KILL")
            .arg("-f")
            .arg(cmdline)
            .status();
        app.services[idx].push_log(format!("[killed {name} (fallback)]"));
    }
}

#[cfg(windows)]
fn kill_tree(idx: usize, app: &mut App) {
    let name = app.services[idx].cfg.name.clone();
    // Use taskkill to nuke the subtree
    let _ = Command::new("taskkill")
        .args(["/F", "/T", "/FI"])
        .arg(format!("WINDOWTITLE eq {}", name))
        .status();
    // fallback: taskkill by image name is too coarse; skip
    app.services[idx].push_log(format!("[killed {name}]"));
}

#[cfg(unix)]
async fn signal_watcher(tx: mpsc::UnboundedSender<AppMsg>) {
    use tokio::signal::unix::{SignalKind, signal};

    // Create and pin futures
    let ctrlc = tokio::signal::ctrl_c();
    let sigterm = signal(SignalKind::terminate()).expect("sigterm");

    tokio::pin!(ctrlc, sigterm);

    // For Unix systems, wait for either Ctrl-C or SIGTERM
    #[allow(clippy::never_loop)]
    loop {
        tokio::select! {
            _ = &mut ctrlc => { let _=tx.send(AppMsg::AbortedAll); break; }
            _ = sigterm.recv() => { let _=tx.send(AppMsg::AbortedAll); break; }
        }
    }
}

#[cfg(not(unix))]
async fn signal_watcher(tx: mpsc::UnboundedSender<AppMsg>) {
    // Create and pin futures
    let ctrlc = tokio::signal::ctrl_c();
    tokio::pin!(ctrlc);

    // For non-Unix systems, just wait for Ctrl-C
    loop {
        tokio::select! {
            _ = &mut ctrlc => { let _=tx.send(AppMsg::AbortedAll); break; }
        }
    }
}

fn debug_file_log(msg: &str) {
    if let Ok(path) = std::env::var("MUXOX_DEBUG_FILE") {
        if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(path) {
            use std::io::Write as _;
            let _ = writeln!(f, "[{:?}] {}", std::thread::current().id(), msg);
        }
    }
}

fn handle_mouse(m: MouseEvent, app: &mut App) -> bool {
    match m.kind {
        MouseEventKind::ScrollUp => {
            // Scroll up a few lines (older logs)
            app.log_offset_from_end = app.log_offset_from_end.saturating_add(3);
            true
        }
        MouseEventKind::ScrollDown => {
            // Scroll down towards the latest logs
            app.log_offset_from_end = app.log_offset_from_end.saturating_sub(3);
            true
        }
        _ => false,
    }
}

// --- ANSI to Ratatui helpers ---
fn ansi_to_line(s: &str) -> Line {
    // Robust ANSI CSI parser that preserves UTF-8 text
    // - Skips any CSI sequence (ESC [ ... final-byte) that is not SGR ('m')
    // - For SGR, applies style to subsequent text
    // - Flushes plain text as UTF-8 slices instead of per-byte casting
    let mut spans: Vec<Span> = Vec::new();
    let mut style = Style::default();

    let bytes = s.as_bytes();
    let mut i = 0usize;
    let mut text_start = 0usize; // beginning of the next plain-text run

    while i < bytes.len() {
        if bytes[i] == 0x1B && i + 1 < bytes.len() && bytes[i + 1] == b'[' {
            // Flush preceding UTF-8 text
            if text_start < i {
                let seg = match std::str::from_utf8(&bytes[text_start..i]) {
                    Ok(seg) => seg.to_string(),
                    Err(_) => String::from_utf8_lossy(&bytes[text_start..i]).to_string(),
                };
                if !seg.is_empty() {
                    spans.push(Span::styled(seg, style));
                }
            }

            // Parse CSI: ESC [ ... final (0x40..=0x7E)
            let mut j = i + 2;
            while j < bytes.len() {
                let b = bytes[j];
                if (0x40..=0x7E).contains(&b) {
                    break;
                }
                j += 1;
            }
            if j >= bytes.len() {
                // Incomplete sequence; stop processing further
                break;
            }
            let final_byte = bytes[j] as char;
            if final_byte == 'm' {
                let params = std::str::from_utf8(&bytes[i + 2..j]).unwrap_or("");
                // Handle private mode prefix like "?25" by trimming leading '?'
                let params = params.trim_start_matches('?');
                apply_sgr(params, &mut style);
            }

            // Move past the entire CSI sequence
            i = j + 1;
            text_start = i;
            continue;
        }
        i += 1;
    }

    // Flush any remaining text
    if text_start < bytes.len() {
        let seg = match std::str::from_utf8(&bytes[text_start..]) {
            Ok(seg) => seg.to_string(),
            Err(_) => String::from_utf8_lossy(&bytes[text_start..]).to_string(),
        };
        if !seg.is_empty() {
            spans.push(Span::styled(seg, style));
        }
    }

    Line::from(spans)
}

fn apply_sgr(seq: &str, style: &mut Style) {
    if seq.is_empty() {
        // ESC[m == reset
        *style = Style::default();
        return;
    }
    let parts: Vec<&str> = seq.split(';').collect();
    let mut idx = 0usize;
    while idx < parts.len() {
        let p = parts[idx];
        let Ok(code) = p.parse::<u16>() else {
            idx += 1;
            continue;
        };
        match code {
            0 => {
                *style = Style::default();
            }
            1 => {
                *style = style.add_modifier(Modifier::BOLD);
            }
            22 => {
                *style = style.remove_modifier(Modifier::BOLD);
            }
            3 => {
                *style = style.add_modifier(Modifier::ITALIC);
            }
            23 => {
                *style = style.remove_modifier(Modifier::ITALIC);
            }
            4 => {
                *style = style.add_modifier(Modifier::UNDERLINED);
            }
            24 => {
                *style = style.remove_modifier(Modifier::UNDERLINED);
            }
            30..=37 => {
                *style = style.fg(basic_color((code - 30) as u8));
            }
            39 => {
                *style = style.fg(Color::Reset);
            }
            40..=47 => {
                *style = style.bg(basic_color((code - 40) as u8));
            }
            49 => {
                *style = style.bg(Color::Reset);
            }
            90..=97 => {
                *style = style.fg(bright_color((code - 90) as u8));
            }
            100..=107 => {
                *style = style.bg(bright_color((code - 100) as u8));
            }
            38 | 48 => {
                // Extended colors: 38;5;n or 38;2;r;g;b
                let is_fg = code == 38;
                if idx + 1 < parts.len() {
                    match parts[idx + 1] {
                        "5" => {
                            // 256-color
                            if idx + 2 < parts.len() {
                                if let Ok(n) = parts[idx + 2].parse::<u8>() {
                                    let c = Color::Indexed(n);
                                    *style = if is_fg { style.fg(c) } else { style.bg(c) };
                                }
                                idx += 2;
                            }
                        }
                        "2" => {
                            // truecolor
                            if idx + 4 < parts.len() {
                                let (r, g, b) = (
                                    parts[idx + 2].parse::<u8>().unwrap_or(0),
                                    parts[idx + 3].parse::<u8>().unwrap_or(0),
                                    parts[idx + 4].parse::<u8>().unwrap_or(0),
                                );
                                let c = Color::Rgb(r, g, b);
                                *style = if is_fg { style.fg(c) } else { style.bg(c) };
                                idx += 4;
                            }
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
        idx += 1;
    }
}

fn basic_color(n: u8) -> Color {
    match n {
        0 => Color::Black,
        1 => Color::Red,
        2 => Color::Green,
        3 => Color::Yellow,
        4 => Color::Blue,
        5 => Color::Magenta,
        6 => Color::Cyan,
        _ => Color::White,
    }
}

fn bright_color(n: u8) -> Color {
    match n {
        0 => Color::DarkGray,
        1 => Color::LightRed,
        2 => Color::LightGreen,
        3 => Color::LightYellow,
        4 => Color::LightBlue,
        5 => Color::LightMagenta,
        6 => Color::LightCyan,
        _ => Color::White,
    }
}
