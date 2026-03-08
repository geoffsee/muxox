// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Geoff Seemueller
// This file is part of muxox, released under the MIT License.

use crate::app::{App, AppMsg, ServiceState, Status, apply_msg, cleanup_and_exit, start_service, stop_service};
use crate::config::Config;
use crate::signal::signal_watcher;
use crate::utils::{ansi_to_line};
use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyModifiers, MouseEvent, MouseEventKind};
use crossterm::terminal::{disable_raw_mode, enable_raw_mode};
use ratatui::{
    Terminal,
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Wrap},
};
use std::io;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::{task, time};

pub async fn run_tui_mode(cfg: Config) -> Result<()> {
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
                    let _ = disable_raw_mode();
                    let mut stdout = io::stdout();
                    let _ = crossterm::execute!(stdout, crossterm::terminal::LeaveAlternateScreen);
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
                Status::Stopped => Color::Red,
                Status::Starting => Color::Yellow,
                Status::Running => Color::Green,
                Status::Stopping => Color::Blue,
            };
            ListItem::new(format!("{} {}", status, s.cfg.name)).style(Style::default().fg(color))
        })
        .collect();

    let list = List::new(items)
        .block(Block::default().borders(Borders::ALL).title(" Services "))
        .highlight_style(
            Style::default()
                .bg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("> ");

    let mut state = list_state(app.selected);
    f.render_stateful_widget(list, chunks[0], &mut state);

    // Logs
    let service = &app.services[app.selected];
    let log_title = format!(" Logs: {} ", service.cfg.name);

    // Filter out common escape sequences for clear rendering
    let logs: Vec<ratatui::text::Line> = service
        .log
        .iter()
        .map(|s| ansi_to_line(s))
        .collect();

    let num_logs = logs.len();
    let scroll_offset = app.log_offset_from_end;

    // To implement scrolling from the end:
    // We want to show the last N lines.
    // If scroll_offset is 0, we show the very last lines.
    // If scroll_offset > 0, we show lines further back.
    let log_paragraph = Paragraph::new(logs)
        .block(Block::default().borders(Borders::ALL).title(log_title))
        .wrap(Wrap { trim: false });

    // Calculate how many lines the paragraph will actually take
    // This is tricky because of wrapping. Ratatui doesn't give us the height easily.
    // For now, we'll use a simpler scrolling based on the number of log entries.
    let display_height = chunks[1].height.saturating_sub(2) as usize;
    let scroll_pos = if num_logs > display_height {
        (num_logs - display_height).saturating_sub(scroll_offset as usize)
    } else {
        0
    };

    f.render_widget(log_paragraph.scroll((scroll_pos as u16, 0)), chunks[1]);

    // Input area
    if app.input_mode {
        let input_title = format!(" Input for {} (Enter to send, Esc to cancel) ", service.cfg.name);
        let input = Paragraph::new(app.input_buffer.as_str())
            .block(Block::default().borders(Borders::ALL).title(input_title));
        f.render_widget(input, main_chunks[1]);
        // Set cursor position
        f.set_cursor_position((
            main_chunks[1].x + app.input_buffer.len() as u16 + 1,
            main_chunks[1].y + 1,
        ));
    }
}

fn list_state(selected: usize) -> ratatui::widgets::ListState {
    let mut state = ratatui::widgets::ListState::default();
    state.select(Some(selected));
    state
}

fn handle_key(k: KeyEvent, app: &mut App) -> bool {
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
                return true;
            }
            KeyCode::Esc => {
                app.input_mode = false;
                app.input_buffer.clear();
                return true;
            }
            KeyCode::Char(c) => {
                app.input_buffer.push(c);
                return true;
            }
            KeyCode::Backspace => {
                app.input_buffer.pop();
                return true;
            }
            _ => return false,
        }
    }

    match k.code {
        KeyCode::Char('q') => {
            let _ = disable_raw_mode();
            let mut stdout = io::stdout();
            let _ = crossterm::execute!(stdout, crossterm::terminal::LeaveAlternateScreen);
            cleanup_and_exit(app);
            true
        }
        KeyCode::Up | KeyCode::Char('k') => {
            if k.modifiers.contains(KeyModifiers::CONTROL) || k.modifiers.contains(KeyModifiers::SHIFT) {
                // Scroll logs up
                app.log_offset_from_end = app.log_offset_from_end.saturating_add(1);
            } else {
                // Navigate service list
                if app.selected > 0 {
                    app.selected -= 1;
                    app.log_offset_from_end = 0;
                }
            }
            true
        }
        KeyCode::Down | KeyCode::Char('j') => {
            if k.modifiers.contains(KeyModifiers::CONTROL) || k.modifiers.contains(KeyModifiers::SHIFT) {
                // Scroll logs down
                app.log_offset_from_end = app.log_offset_from_end.saturating_sub(1);
            } else {
                // Navigate service list
                if app.selected < app.services.len() - 1 {
                    app.selected += 1;
                    app.log_offset_from_end = 0;
                }
            }
            true
        }
        KeyCode::Char(' ') => {
            toggle_selected(app);
            true
        }
        KeyCode::Char('r') => {
            restart_selected(app);
            true
        }
        KeyCode::Enter | KeyCode::Char('i') => {
            if app.services[app.selected].cfg.interactive {
                app.input_mode = true;
                app.input_buffer.clear();
                true
            } else {
                false
            }
        }
        _ => false,
    }
}

fn toggle_selected(app: &mut App) {
    let idx = app.selected;
    match app.services[idx].status {
        Status::Stopped => start_service(idx, app),
        Status::Running | Status::Starting => stop_service(idx, app),
        Status::Stopping => {}
    }
}

fn restart_selected(app: &mut App) {
    let idx = app.selected;
    stop_service(idx, app);
    start_service(idx, app);
}

fn handle_mouse(m: MouseEvent, app: &mut App) -> bool {
    match m.kind {
        MouseEventKind::ScrollUp => {
            app.log_offset_from_end = app.log_offset_from_end.saturating_add(3);
            true
        }
        MouseEventKind::ScrollDown => {
            app.log_offset_from_end = app.log_offset_from_end.saturating_sub(3);
            true
        }
        _ => false,
    }
}
