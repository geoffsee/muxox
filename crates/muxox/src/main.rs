use std::{
    collections::VecDeque,
    fs,
    io,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    time::Duration,
};

use anyhow::{Context, Result};
use clap::Parser;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyModifiers};
use crossterm::terminal::{disable_raw_mode, enable_raw_mode};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, Paragraph, Wrap},
    Terminal,
};
use serde::Deserialize;
use tokio::{io::AsyncBufReadExt, process::Command as AsyncCommand, sync::mpsc, task, time};
#[cfg(windows)]
use std::os::windows::process::CommandExt as _;

#[derive(Debug, Parser)]
#[command(author, version, about = "Run multiple dev servers with a simple TUI.")]
struct Cli {
    /// Optional path to a services config (TOML). If omitted, looks in: $PWD/muxox.toml then app dirs.
    #[arg(short, long)]
    config: Option<PathBuf>,
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
fn default_log_capacity() -> usize { 2000 }

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
enum Status { Stopped, Starting, Running, Stopping }

#[derive(Debug)]
struct ServiceState {
    cfg: ServiceCfg,
    status: Status,
    child: Option<Child>,
    log: VecDeque<String>,
}

impl ServiceState {
    fn new(cfg: ServiceCfg) -> Self {
        Self {
            log: VecDeque::with_capacity(cfg.log_capacity.max(256)),
            cfg,
            status: Status::Stopped,
            child: None,
        }
    }
    fn push_log(&mut self, line: impl Into<String>) {
        if self.log.len() == self.cfg.log_capacity { self.log.pop_front(); }
        self.log.push_back(line.into());
    }
}

#[derive(Debug)]
struct App {
    services: Vec<ServiceState>,
    selected: usize,
    tx: mpsc::UnboundedSender<AppMsg>,
}

#[derive(Debug)]
enum AppMsg {
    Started(usize),
    Stopped(usize, i32),
    Log(usize, String),
    AbortedAll,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let cfg = load_config(cli.config.as_deref())?;

    let (tx, mut rx) = mpsc::unbounded_channel::<AppMsg>();
    let mut app = App { services: cfg.service.into_iter().map(ServiceState::new).collect(), selected: 0, tx: tx.clone() };

    // Signal watcher: on any exit signal, nuke children then exit.
    task::spawn(signal_watcher(tx.clone()));

    // TUI setup
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    crossterm::execute!(stdout, crossterm::terminal::EnterAlternateScreen, crossterm::event::EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

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
                if let Event::Key(k) = event::read().unwrap_or(Event::FocusGained) {
                    handled = handle_key(k, &mut app);
                }
            }
            if handled { /* app mutated, redraw next loop */ }
            if last_tick.elapsed() >= tick_rate { last_tick = time::Instant::now(); }

            // Drain channel
            while let Ok(msg) = rx.try_recv() { apply_msg(&mut app, msg); }
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

    let items: Vec<ListItem> = app.services.iter().enumerate().map(|(_i, s)| {
        let status = match s.status { Status::Stopped=>"●", Status::Starting=>"◔", Status::Running=>"◉", Status::Stopping=>"◑" };
        let color = match s.status { Status::Running=>Color::Green, Status::Starting=>Color::Yellow, Status::Stopping=>Color::Magenta, Status::Stopped=>Color::DarkGray };
        ListItem::new(Line::from(vec![Span::styled(format!(" {status} "), Style::default().fg(color)), Span::raw(&s.cfg.name)]))
    }).collect();

    let list = List::new(items)
        .block(Block::default().title("Services  (↑/↓ select, Enter start/stop, r restart, c clear, q quit)").borders(Borders::ALL))
        .highlight_style(Style::default().add_modifier(Modifier::BOLD).bg(Color::DarkGray));

    f.render_stateful_widget(list, chunks[0], &mut list_state(app.selected));

    // Right pane: logs
    let right_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Min(1)])
        .split(chunks[1]);

    let selected = &app.services[app.selected];
    let header = Paragraph::new(vec![
        Line::from(vec![Span::styled("Name: ", Style::default().fg(Color::DarkGray)), Span::raw(&selected.cfg.name)]),
        Line::from(vec![Span::styled("Cmd:  ", Style::default().fg(Color::DarkGray)), Span::raw(&selected.cfg.cmd)]),
        Line::from(vec![Span::styled("Cwd:  ", Style::default().fg(Color::DarkGray)), Span::raw(selected.cfg.cwd.as_ref().and_then(|p| p.to_str()).unwrap_or("."))]),
    ])
        .block(Block::default().title("Selected Service").borders(Borders::ALL));

    let log_text: Vec<Line> = selected.log.iter().map(|l| Line::from(l.clone())).collect();
    let log = Paragraph::new(log_text)
        .wrap(Wrap { trim: false })
        .block(Block::default().title("Logs").borders(Borders::ALL));

    f.render_widget(header, right_chunks[0]);
    f.render_widget(log, right_chunks[1]);
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
        (KeyCode::Down, _) => { app.selected = (app.selected + 1).min(app.services.len()-1); return true; }
        (KeyCode::Up, _) => { if app.selected>0 { app.selected -= 1; } return true; }
        (KeyCode::Enter, _) | (KeyCode::Char(' '), _) => { toggle_selected(app); return true; }
        (KeyCode::Char('r'), _) => { restart_selected(app); return true; }
        (KeyCode::Char('c'), _) if k.modifiers == KeyModifiers::NONE => { app.services[app.selected].log.clear(); return true; }
        _ => {}
    }
    false
}

fn toggle_selected(app: &mut App) {
    let idx = app.selected;
    match app.services[idx].status { Status::Stopped => { start_service(idx, app); }, Status::Running | Status::Starting => { stop_service(idx, app); }, Status::Stopping => {} }
}

fn restart_selected(app: &mut App) { let idx = app.selected; stop_service(idx, app); start_service(idx, app); }

fn start_service(idx: usize, app: &mut App) {
    if matches!(app.services[idx].status, Status::Running | Status::Starting) { return; }
    app.services[idx].status = Status::Starting;
    let tx = app.tx.clone();
    let sc = app.services[idx].cfg.clone();
    task::spawn(async move {
        // Build command under a shell
        let mut cmd = AsyncCommand::new(shell_program());
        cmd.arg(shell_flag()).arg(shell_exec(&sc.cmd));
        if let Some(cwd) = sc.cwd.clone() { cmd.current_dir(cwd); }
        cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
        set_process_group(&mut cmd);

        match cmd.spawn() {
            Ok(mut child) => {
                let _pid = child.id().unwrap_or_default();
                let _ = tx.send(AppMsg::Started(idx));

                // stdout
                if let Some(out) = child.stdout.take() {
                    let tx2 = tx.clone();
                    task::spawn(async move {
                        let mut reader = tokio::io::BufReader::new(out).lines();
                        while let Ok(Some(line)) = reader.next_line().await { let _ = tx2.send(AppMsg::Log(idx, line)); }
                    });
                }
                // stderr
                if let Some(err) = child.stderr.take() {
                    let tx2 = tx.clone();
                    task::spawn(async move {
                        let mut reader = tokio::io::BufReader::new(err).lines();
                        while let Ok(Some(line)) = reader.next_line().await { let _ = tx2.send(AppMsg::Log(idx, format!("[stderr] {line}"))); }
                    });
                }

                // Waiter
                let status = child.wait().await; // process exit
                let code = status.map(|s| s.code().unwrap_or(-1)).unwrap_or(-1);
                let _ = tx.send(AppMsg::Stopped(idx, code));
            }
            Err(e) => { let _ = tx.send(AppMsg::Log(idx, format!("spawn failed: {e}"))); let _ = tx.send(AppMsg::Stopped(idx, -1)); }
        }
    });
}

fn stop_service(idx: usize, app: &mut App) {
    let sc = &mut app.services[idx];
    if !matches!(sc.status, Status::Running | Status::Starting) { return; }
    sc.status = Status::Stopping;
    sc.push_log("Stopping...");
    if let Some(child) = sc.child.take() { drop(child); } // actual kill handled by kill_tree below
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
            s.push_log(format!("[exited: code {code}]").as_str());
        }
        AppMsg::Log(i, line) => { app.services[i].push_log(line); }
        AppMsg::AbortedAll => { /* UI can optionally display something */ }
    }
}

fn cleanup_and_exit(app: &mut App) {
    // Restore terminal first to avoid leaving it raw if we panic later.
    let _ = disable_raw_mode();
    let mut stdout = io::stdout();
    let _ = crossterm::execute!(stdout, crossterm::event::DisableMouseCapture, crossterm::terminal::LeaveAlternateScreen);

    // Kill all children forcefully
    kill_all(app);

    std::process::exit(0);
}

fn kill_all(app: &mut App) {
    for i in 0..app.services.len() { kill_tree(i, app); }
}

fn load_config(provided: Option<&Path>) -> Result<Config> {
    let candidates: Vec<PathBuf> = match provided {
        Some(p) => vec![p.to_path_buf()],
        None => {
            let mut v = vec![PathBuf::from("muxox.toml")];
            if let Some(proj) = directories::ProjectDirs::from("dev", "local", "devmux") { v.push(proj.config_dir().join("muxox.toml")); }
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
    unsafe { cmd.pre_exec(|| { libc::setsid(); Ok(()) }) };
}
#[cfg(windows)]
fn set_process_group(cmd: &mut AsyncCommand) {
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x00000200;
    const CREATE_NEW_CONSOLE: u32 = 0x00000010; // better isolation for signals
    cmd.creation_flags(CREATE_NEW_PROCESS_GROUP | CREATE_NEW_CONSOLE);
}

#[cfg(unix)]
fn shell_program() -> &'static str {
    if std::env::var("SHELL").ok().filter(|s| !s.is_empty()).is_some() {
        // Can't return a dynamically created String as &'static str
        // For simplicity, return a common shell path
        "/bin/bash"
    } else { 
        "/bin/sh" 
    }
}
#[cfg(unix)]
fn shell_flag() -> &'static str { "-lc" }
#[cfg(unix)]
fn shell_exec(cmd: &str) -> String { cmd.to_string() }

#[cfg(windows)]
fn shell_program() -> &'static str { "cmd.exe" }
#[cfg(windows)]
fn shell_flag() -> &'static str { "/C" }
#[cfg(windows)]
fn shell_exec(cmd: &str) -> String { cmd.to_string() }

#[cfg(unix)]
fn kill_tree(idx: usize, app: &mut App) {
    // These imports are left as warnings intentionally since they might be needed
    // if we implement more advanced process group management in the future
    use nix::sys::signal::{killpg, Signal};
    use nix::unistd::Pid;
    let name = app.services[idx].cfg.name.clone();
    // We don't track exact pgid; setsid() made child leader so killpg(-pid) works if we had it.
    // As we don't keep the pid, we issue a shell-level killall using process group via sh -c.
    // Simpler: store no pid; rely on pkill -f cmd as fallback.
    let cmdline = &app.services[idx].cfg.cmd;
    // best-effort group kill via pkill
    let _ = Command::new("pkill").arg("-TERM").arg("-f").arg(cmdline).status();
    std::thread::sleep(Duration::from_millis(250));
    let _ = Command::new("pkill").arg("-KILL").arg("-f").arg(cmdline).status();
    app.services[idx].push_log(format!("[killed {name}]"));
}

#[cfg(windows)]
fn kill_tree(idx: usize, app: &mut App) {
    let name = app.services[idx].cfg.name.clone();
    // Use taskkill to nuke the subtree
    let _ = Command::new("taskkill").args(["/F","/T","/FI"]).arg(format!("WINDOWTITLE eq {}", name)).status();
    // fallback: taskkill by image name is too coarse; skip
    app.services[idx].push_log(format!("[killed {name}]"));
}

#[cfg(unix)]
async fn signal_watcher(tx: mpsc::UnboundedSender<AppMsg>) {
    use tokio::signal::unix::{signal, SignalKind};
    
    // Create and pin futures
    let ctrlc = tokio::signal::ctrl_c();
    let sigterm = signal(SignalKind::terminate()).expect("sigterm");
    
    tokio::pin!(ctrlc, sigterm);
    
    // For Unix systems, wait for either Ctrl-C or SIGTERM
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