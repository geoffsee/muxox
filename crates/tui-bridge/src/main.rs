//! Minimal demo: Dioxus state → ViewNode tree → Ratatui rendering.

use std::cell::RefCell;
use std::io;
use std::rc::Rc;
use std::time::Duration;

use crossterm::event::{self, Event, KeyCode};
use crossterm::terminal::{disable_raw_mode, enable_raw_mode};
use dioxus_core::{Element, VNode, VirtualDom};
use ratatui::layout::Constraint;
use ratatui::style::Color;
use ratatui::{Terminal, backend::CrosstermBackend};

use tui_bridge::render::render_view;
use tui_bridge::view::*;

// --- App state (shared between Dioxus and the event loop) ---

#[derive(Clone)]
struct AppState {
    items: Vec<String>,
    selected: usize,
}

type ViewOut = Rc<RefCell<ViewNode>>;

// --- Dioxus component: reads state, writes ViewNode ---

fn app() -> Element {
    let state = dioxus_core::consume_context::<Rc<RefCell<AppState>>>();
    let view_out = dioxus_core::consume_context::<ViewOut>();

    let st = state.borrow();

    let list = ViewNode::StyledList(StyledListNode {
        title: " Items (↑↓ navigate, q quit) ".into(),
        items: st
            .items
            .iter()
            .enumerate()
            .map(|(i, name)| {
                let color = if i == st.selected {
                    Color::Green
                } else {
                    Color::White
                };
                (format!("● {name}"), color)
            })
            .collect(),
        selected: st.selected,
    });

    let detail = ViewNode::Text(TextNode {
        title: " Detail ".into(),
        lines: vec![format!("Selected: {}", st.items[st.selected]).into()],
        scroll: 0,
    });

    *view_out.borrow_mut() = ViewNode::Row(RowNode {
        children: vec![list, detail],
        constraints: vec![Constraint::Percentage(40), Constraint::Percentage(60)],
    });

    Ok(VNode::placeholder())
}

// --- Event loop ---

fn main() -> anyhow::Result<()> {
    let state = Rc::new(RefCell::new(AppState {
        items: vec![
            "Alpha".into(),
            "Bravo".into(),
            "Charlie".into(),
            "Delta".into(),
        ],
        selected: 0,
    }));
    let view_out: ViewOut = Rc::new(RefCell::new(ViewNode::Empty));

    let mut vdom = VirtualDom::new(app)
        .with_root_context(state.clone())
        .with_root_context(view_out.clone());

    vdom.rebuild_in_place();

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    crossterm::execute!(stdout, crossterm::terminal::EnterAlternateScreen)?;
    let mut terminal = Terminal::new(CrosstermBackend::new(stdout))?;

    loop {
        vdom.rebuild_in_place();

        {
            let view = view_out.borrow();
            terminal.draw(|f| render_view(f, f.area(), &view))?;
        }

        if event::poll(Duration::from_millis(100))?
            && let Event::Key(k) = event::read()?
        {
            let mut st = state.borrow_mut();
            match k.code {
                KeyCode::Char('q') => break,
                KeyCode::Up | KeyCode::Char('k') => {
                    st.selected = st.selected.saturating_sub(1);
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    if st.selected < st.items.len() - 1 {
                        st.selected += 1;
                    }
                }
                _ => {}
            }
        }
    }

    disable_raw_mode()?;
    crossterm::execute!(
        terminal.backend_mut(),
        crossterm::terminal::LeaveAlternateScreen
    )?;
    Ok(())
}
