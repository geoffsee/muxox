// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Geoff Seemueller
// This file is part of muxox, released under the MIT License.

use muxox_core::app::{
    App, AppMsg, ServiceState, Status, apply_msg, cleanup_and_exit, start_service, stop_service,
};
use muxox_core::config::Config;
use muxox_core::isolation::default_isolation;
use muxox_core::log::debug;
use muxox_core::signal::signal_watcher;
use muxox_core::utils::ansi_to_line;

use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyModifiers, MouseEvent, MouseEventKind};
use crossterm::terminal::{disable_raw_mode, enable_raw_mode};
use dioxus_core::{Element, VNode, VirtualDom};
use ratatui::layout::Constraint;
use ratatui::style::Color;
use ratatui::{Terminal, backend::CrosstermBackend};
use tui_bridge::render::render_view;
use tui_bridge::view::*;

use std::cell::RefCell;
use std::io;
use std::rc::Rc;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::{task, time};

// --- Dioxus context types ---

type AppRef = Rc<RefCell<App>>;
type ViewOut = Rc<RefCell<ViewNode>>;

// --- Dioxus component: reads App state, writes ViewNode tree ---

pub(crate) fn tui_root() -> Element {
    let app_ref = dioxus_core::consume_context::<AppRef>();
    let view_out = dioxus_core::consume_context::<ViewOut>();

    let app = app_ref.borrow();
    debug(&format!(
        "tui_root called: selected={} log_len={}",
        app.selected,
        app.services[app.selected].log.len()
    ));

    // Service list
    let service_list = ViewNode::StyledList(StyledListNode {
        title: " Services ".into(),
        items: app
            .services
            .iter()
            .map(|s| {
                let (symbol, color) = status_display(s.status);
                (format!("{symbol} {}", s.cfg.name), color)
            })
            .collect(),
        selected: app.selected,
    });

    // Log panel
    let selected_service = &app.services[app.selected];
    let log_panel = ViewNode::Text(TextNode {
        title: format!(" Logs: {} ", selected_service.cfg.name),
        lines: selected_service
            .log
            .iter()
            .map(|s| ansi_to_line(s))
            .collect(),
        scroll: app.log_offset_from_end,
    });

    // Main layout: services (30%) | logs (70%)
    let main = ViewNode::Row(RowNode {
        children: vec![service_list, log_panel],
        constraints: vec![Constraint::Percentage(30), Constraint::Percentage(70)],
    });

    // Help bar
    let help = if app.input_mode {
        ViewNode::HelpBar(HelpBarNode {
            bindings: vec![
                ("Enter".into(), "Send".into()),
                ("Esc".into(), "Cancel".into()),
            ],
        })
    } else {
        let mut bindings = vec![
            ("q".into(), "Quit".into()),
            ("\u{2191}\u{2193}".into(), "Navigate".into()),
            ("Space".into(), "Start/Stop".into()),
            ("r".into(), "Restart".into()),
            ("Ctrl+\u{2191}\u{2193}".into(), "Scroll Logs".into()),
        ];
        if selected_service.cfg.interactive {
            bindings.push(("i".into(), "Input".into()));
        }
        ViewNode::HelpBar(HelpBarNode { bindings })
    };

    // Optionally add input bar above the help bar
    let root = if app.input_mode {
        let input_bar = ViewNode::InputBar(InputBarNode {
            title: format!(
                " Input for {} (Enter to send, Esc to cancel) ",
                selected_service.cfg.name
            ),
            text: app.input_buffer.clone(),
        });
        ViewNode::Column(ColumnNode {
            children: vec![main, input_bar, help],
            constraints: vec![
                Constraint::Min(1),
                Constraint::Length(3),
                Constraint::Length(1),
            ],
        })
    } else {
        ViewNode::Column(ColumnNode {
            children: vec![main, help],
            constraints: vec![Constraint::Min(1), Constraint::Length(1)],
        })
    };

    *view_out.borrow_mut() = root;

    Ok(VNode::placeholder())
}

fn status_display(status: Status) -> (&'static str, Color) {
    match status {
        Status::Stopped => ("●", Color::Red),
        Status::Starting => ("◔", Color::Yellow),
        Status::Running => ("◉", Color::Green),
        Status::Stopping => ("◑", Color::Blue),
    }
}

// --- Entry point ---

pub async fn run_tui_mode(cfg: Config) -> Result<()> {
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async move {
            let (tx, mut rx) = mpsc::unbounded_channel::<AppMsg>();
            let app = App {
                services: cfg.service.into_iter().map(ServiceState::new).collect(),
                selected: 0,
                log_offset_from_end: 0,
                tx: tx.clone(),
                input_mode: false,
                input_buffer: String::new(),
                isolation: default_isolation(),
            };

            // Signal watcher (spawns on the tokio runtime, not the LocalSet)
            task::spawn(signal_watcher(tx.clone()));

            // Terminal setup
            enable_raw_mode()?;
            let mut stdout = io::stdout();
            crossterm::execute!(stdout, crossterm::terminal::EnterAlternateScreen)?;
            let mut terminal = Terminal::new(CrosstermBackend::new(stdout))?;

            // Shared state
            let app_ref: AppRef = Rc::new(RefCell::new(app));
            let view_out: ViewOut = Rc::new(RefCell::new(ViewNode::Empty));

            // Start all services
            {
                let mut app = app_ref.borrow_mut();
                for idx in 0..app.services.len() {
                    start_service(idx, &mut app);
                }
            }

            // Dioxus VirtualDom (headless — we only use it for component state)
            let mut vdom = VirtualDom::new(tui_root)
                .with_root_context(app_ref.clone())
                .with_root_context(view_out.clone());
            vdom.rebuild_in_place();

            // Render loop
            let mut last_tick = time::Instant::now();
            let tick_rate = Duration::from_millis(150);
            let mut loop_count: u64 = 0;

            loop {
                loop_count += 1;
                if loop_count <= 20 || loop_count.is_multiple_of(100) {
                    let app = app_ref.borrow();
                    let log_lens: Vec<usize> = app.services.iter().map(|s| s.log.len()).collect();
                    let statuses: Vec<&str> =
                        app.services.iter().map(|s| s.status.as_str()).collect();
                    debug(&format!(
                        "loop #{loop_count} statuses={statuses:?} log_lens={log_lens:?}"
                    ));
                    drop(app);
                }

                // 1. Run Dioxus component → populates view_out
                vdom.rebuild_in_place();

                // 2. Draw
                {
                    let view = view_out.borrow();
                    terminal.draw(|f| render_view(f, f.area(), &view)).ok();
                }

                // 3. Handle terminal input
                let timeout = tick_rate.saturating_sub(last_tick.elapsed());
                if loop_count <= 5 {
                    debug(&format!(
                        "loop #{loop_count} polling with timeout={timeout:?}"
                    ));
                }
                let poll_result = event::poll(timeout);
                if loop_count <= 5 {
                    debug(&format!("loop #{loop_count} poll returned {poll_result:?}"));
                }
                if poll_result.unwrap_or(false) {
                    match event::read().unwrap_or(Event::FocusGained) {
                        Event::Key(k) => {
                            debug(&format!("key event: {k:?}"));
                            handle_key(k, &mut app_ref.borrow_mut());
                        }
                        Event::Mouse(m) => {
                            if matches!(
                                m.kind,
                                MouseEventKind::ScrollUp | MouseEventKind::ScrollDown
                            ) {
                                handle_mouse(m, &mut app_ref.borrow_mut());
                            }
                        }
                        _ => {}
                    }
                }

                if last_tick.elapsed() >= tick_rate {
                    last_tick = time::Instant::now();
                }

                // 4. Drain channel
                let mut drained = 0u32;
                while let Ok(msg) = rx.try_recv() {
                    drained += 1;
                    if matches!(msg, AppMsg::AbortedAll) {
                        let _ = disable_raw_mode();
                        let mut stdout = io::stdout();
                        let _ =
                            crossterm::execute!(stdout, crossterm::terminal::LeaveAlternateScreen);
                        cleanup_and_exit(&mut app_ref.borrow_mut());
                    }
                    apply_msg(&mut app_ref.borrow_mut(), msg);
                }
                if drained > 0 && (loop_count <= 20 || loop_count.is_multiple_of(100)) {
                    debug(&format!("loop #{loop_count} drained {drained} messages"));
                }
            }
        })
        .await
}

// --- Input handling ---

pub(crate) fn handle_key(k: KeyEvent, app: &mut App) {
    if app.input_mode {
        match k.code {
            KeyCode::Enter => {
                let input = std::mem::take(&mut app.input_buffer);
                let idx = app.selected;
                if let Some(writer) = app.services[idx].stdin_writer.clone() {
                    tokio::spawn(async move {
                        let mut writer_guard = writer.lock().await;
                        use tokio::io::AsyncWriteExt;
                        let input_with_newline = format!("{}\n", input);
                        let _ = writer_guard.write_all(input_with_newline.as_bytes()).await;
                        let _ = writer_guard.flush().await;
                    });
                }
                app.input_mode = false;
            }
            KeyCode::Esc => {
                app.input_mode = false;
                app.input_buffer.clear();
            }
            KeyCode::Char(c) => {
                app.input_buffer.push(c);
            }
            KeyCode::Backspace => {
                app.input_buffer.pop();
            }
            _ => {}
        }
        return;
    }

    match k.code {
        KeyCode::Char('q') => {
            let _ = disable_raw_mode();
            let mut stdout = io::stdout();
            let _ = crossterm::execute!(stdout, crossterm::terminal::LeaveAlternateScreen);
            cleanup_and_exit(app);
        }
        KeyCode::Up | KeyCode::Char('k') => {
            if k.modifiers.contains(KeyModifiers::CONTROL)
                || k.modifiers.contains(KeyModifiers::SHIFT)
            {
                app.log_offset_from_end = app.log_offset_from_end.saturating_add(1);
            } else if app.selected > 0 {
                app.selected -= 1;
                app.log_offset_from_end = 0;
            }
        }
        KeyCode::Down | KeyCode::Char('j') => {
            if k.modifiers.contains(KeyModifiers::CONTROL)
                || k.modifiers.contains(KeyModifiers::SHIFT)
            {
                app.log_offset_from_end = app.log_offset_from_end.saturating_sub(1);
            } else if app.selected < app.services.len() - 1 {
                app.selected += 1;
                app.log_offset_from_end = 0;
            }
        }
        KeyCode::Char(' ') => {
            let idx = app.selected;
            match app.services[idx].status {
                Status::Stopped => start_service(idx, app),
                Status::Running | Status::Starting => stop_service(idx, app),
                Status::Stopping => {}
            }
        }
        KeyCode::Char('r') => {
            let idx = app.selected;
            stop_service(idx, app);
            start_service(idx, app);
        }
        KeyCode::Enter | KeyCode::Char('i') => {
            if app.services[app.selected].cfg.interactive {
                app.input_mode = true;
                app.input_buffer.clear();
            }
        }
        _ => {}
    }
}

pub(crate) fn handle_mouse(m: MouseEvent, app: &mut App) {
    match m.kind {
        MouseEventKind::ScrollUp => {
            app.log_offset_from_end = app.log_offset_from_end.saturating_add(3);
        }
        MouseEventKind::ScrollDown => {
            app.log_offset_from_end = app.log_offset_from_end.saturating_sub(3);
        }
        _ => {}
    }
}
