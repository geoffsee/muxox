use std::{
    collections::VecDeque,
    fs, io,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    time::Duration,
};

use anyhow::{Context, Result};
use clap::Parser;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyModifiers, MouseEvent, MouseEventKind};
use crossterm::terminal::{disable_raw_mode, enable_raw_mode};
use ratatui::{
    Terminal,
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, Paragraph, Wrap},
};
use serde::Deserialize;
#[cfg(windows)]
use std::os::windows::process::CommandExt as _;
use tokio::sync::mpsc::UnboundedSender;
use tokio::{io::AsyncBufReadExt, process::Command as AsyncCommand, sync::mpsc, task, time};

#[derive(Debug, Parser)]
#[command(author, version, about = "Run multiple dev servers with a simple TUI.")]
struct Cli {
    /// Optional path to a services config (TOML). If omitted, looks in: $PWD/muxox.toml then app dirs.
    #[arg(short, long)]
    config: Option<PathBuf>,
    /// Run in non-interactive mode, outputting raw logs instead of TUI
    #[arg(long)]
    raw: bool,
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
            name = "frontend"
            cmd = "pnpm client:dev"
            cwd = "./"
            log_capacity = 5000

            [[service]]
            name = "backend"
            cmd = "pnpm server:dev"
            cwd = "./"
        "#;

        let cfg: Config = toml::from_str(toml_input).expect("Valid Config");
        assert_eq!(cfg.service.len(), 2);

        assert_eq!(cfg.service[0].name, "frontend");
        assert_eq!(cfg.service[0].cmd, "pnpm client:dev");
        assert_eq!(cfg.service[0].cwd, Some(PathBuf::from("./")));
        assert_eq!(cfg.service[0].log_capacity, 5000);

        assert_eq!(cfg.service[1].name, "backend");
        assert_eq!(cfg.service[1].cmd, "pnpm server:dev");
        assert_eq!(cfg.service[1].cwd, Some(PathBuf::from("./")));
        assert_eq!(cfg.service[1].log_capacity, default_log_capacity());
    }

    #[test]
    fn test_service_state() {
        let cfg = ServiceCfg {
            name: "test".to_string(),
            cmd: "echo hello".to_string(),
            cwd: None,
            log_capacity: 10,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Status {
    Stopped,
    Starting,
    Running,
    Stopping,
}

#[derive(Debug)]
struct ServiceState {
    cfg: ServiceCfg,
    status: Status,
    child: Option<Child>,
    pid: Option<u32>,
    log: VecDeque<String>,
}

impl ServiceState {
    fn new(cfg: ServiceCfg) -> Self {
        Self {
            log: VecDeque::with_capacity(cfg.log_capacity.max(256)),
            cfg,
            status: Status::Stopped,
            child: None,
            pid: None,
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
}

#[derive(Debug)]
enum AppMsg {
    Started(usize),
    Stopped(usize, i32),
    Log(usize, String),
    ChildSpawned(usize, u32),
    AbortedAll,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let cfg = load_config(cli.config.as_deref())?;

    if cli.raw {
        // Run in raw streaming mode
        run_raw_mode(cfg).await
    } else {
        // Run in TUI mode
        run_tui_mode(cfg).await
    }
}

async fn run_raw_mode(cfg: Config) -> Result<()> {
    let (tx, mut rx) = mpsc::unbounded_channel::<AppMsg>();
    let mut app = App {
        services: cfg.service.into_iter().map(ServiceState::new).collect(),
        selected: 0,
        log_offset_from_end: 0,
        tx: tx.clone(),
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
            Some(AppMsg::AbortedAll) => {
                eprintln!("All services aborted");
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
    };

    // Signal watcher: on any exit signal, nuke children then exit.
    task::spawn(signal_watcher(tx.clone()));

    // TUI setup
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    crossterm::execute!(
        stdout,
        crossterm::terminal::EnterAlternateScreen
    )?;
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
                        if matches!(m.kind, MouseEventKind::ScrollUp | MouseEventKind::ScrollDown) {
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
                apply_msg(&mut app, msg);
            }
        }
    });

    // Wait until UI task ends (it never does gracefully). If it errors, fallthrough.
    let _ = ui_task.await;
    Ok(())
}

fn draw_ui(f: &mut ratatui::Frame, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(30), Constraint::Percentage(70)])
        .split(f.area());

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
                .title("Services  (↑/↓ select, Enter start/stop, scroll logs: mouse/trackpad or PgUp/PgDn, r restart, c clear, q quit)")
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

    let log_text: Vec<Line> = selected.log.iter().map(|l| Line::from(l.clone())).collect();
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
}

fn list_state(selected: usize) -> ratatui::widgets::ListState {
    let mut state = ratatui::widgets::ListState::default();
    state.select(Some(selected));
    state
}

fn handle_key(k: KeyEvent, app: &mut App) -> bool {
    match (k.code, k.modifiers) {
        (KeyCode::Char('q'), _) | (KeyCode::Esc, _) => {
            // Quit: perform cleanup
            cleanup_and_exit(app);
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
        (KeyCode::Enter, _) | (KeyCode::Char(' '), _) => {
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
    match app.services[idx].status {
        Status::Stopped => {
            start_service(idx, app);
        }
        Status::Running | Status::Starting => {
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
        // Build command under a shell
        let mut cmd = AsyncCommand::new(shell_program());
        cmd.arg(shell_flag()).arg(shell_exec(&sc.cmd));
        if let Some(cwd) = sc.cwd.clone() {
            cmd.current_dir(cwd);
        }
        cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
        set_process_group(&mut cmd);

        match cmd.spawn() {
            Ok(mut child) => {
                let pid = child.id().unwrap_or_default();
                let _ = tx.send(AppMsg::Started(idx));

                // Send the child handle back to be stored
                let _ = tx.send(AppMsg::ChildSpawned(idx, pid));

                // stdout
                if let Some(out) = child.stdout.take() {
                    let tx2 = tx.clone();
                    task::spawn(async move {
                        stream_log_realtime(idx, out, tx2, false).await;
                    });
                }
                // stderr
                if let Some(err) = child.stderr.take() {
                    let tx2 = tx.clone();
                    task::spawn(async move {
                        stream_log_realtime(idx, err, tx2, true).await;
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

async fn stream_log_realtime<R>(idx: usize, stream: R, tx: UnboundedSender<AppMsg>, is_stderr: bool)
where
    R: tokio::io::AsyncRead + Unpin,
{
    let mut lines = tokio::io::BufReader::new(stream).lines();

    while let Ok(Some(line)) = lines.next_line().await {
        if is_stderr {
            let _ = tx.send(AppMsg::Log(idx, format!("ERR: {}", line)));
        } else {
            let _ = tx.send(AppMsg::Log(idx, line));
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
            s.push_log(format!("[exited: code {code}]").as_str());
        }
        AppMsg::Log(i, line) => {
            app.services[i].push_log(line);
        }
        AppMsg::ChildSpawned(i, pid) => {
            app.services[i].pid = Some(pid);
            app.services[i].push_log(format!("[process started with PID {pid}]"));
        }
        AppMsg::AbortedAll => { /* UI can optionally display something */ }
    }
}

fn cleanup_and_exit(app: &mut App) {
    // Restore terminal first to avoid leaving it raw if we panic later.
    let _ = disable_raw_mode();
    let mut stdout = io::stdout();
    let _ = crossterm::execute!(
        stdout,
        crossterm::terminal::LeaveAlternateScreen
    );

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
            if let Some(proj) = directories::ProjectDirs::from("dev", "local", "devmux") {
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
fn set_process_group(_cmd: &mut AsyncCommand) {
    // Remove the unsafe pre_exec that was blocking concurrent execution
    // The process group setting is not essential for basic functionality
    // and was causing synchronous blocking in the async runtime
    //
    // If process group isolation is needed in the future, it should be
    // implemented using process_group() method or other async-safe approaches
}
#[cfg(windows)]
fn set_process_group(cmd: &mut AsyncCommand) {
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x00000200;
    const CREATE_NEW_CONSOLE: u32 = 0x00000010; // better isolation for signals
    cmd.creation_flags(CREATE_NEW_PROCESS_GROUP | CREATE_NEW_CONSOLE);
}

#[cfg(unix)]
fn shell_program() -> &'static str {
    if std::env::var("SHELL")
        .ok()
        .filter(|s| !s.is_empty())
        .is_some()
    {
        // Can't return a dynamically created String as &'static str
        // For simplicity, return a common shell path
        "/bin/bash"
    } else {
        "/bin/sh"
    }
}
#[cfg(unix)]
fn shell_flag() -> &'static str {
    "-lc"
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
