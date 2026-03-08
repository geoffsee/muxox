//! Layer 3: Ratatui renderer.
//!
//! Pure function that walks a `ViewNode` tree and paints it onto a ratatui `Frame`.

use crate::view::*;
use ratatui::Frame;
use ratatui::layout::{Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, Paragraph, Wrap};

/// Recursively render a view tree into a ratatui frame.
pub fn render_view(f: &mut Frame, area: Rect, node: &ViewNode) {
    match node {
        ViewNode::Row(row) => render_layout(
            f,
            area,
            Direction::Horizontal,
            &row.constraints,
            &row.children,
        ),
        ViewNode::Column(col) => render_layout(
            f,
            area,
            Direction::Vertical,
            &col.constraints,
            &col.children,
        ),
        ViewNode::StyledList(list) => render_styled_list(f, area, list),
        ViewNode::Text(text) => render_text(f, area, text),
        ViewNode::InputBar(input) => render_input_bar(f, area, input),
        ViewNode::HelpBar(help) => render_help_bar(f, area, help),
        ViewNode::Empty => {}
    }
}

fn render_layout(
    f: &mut Frame,
    area: Rect,
    direction: Direction,
    constraints: &[ratatui::layout::Constraint],
    children: &[ViewNode],
) {
    let chunks = Layout::default()
        .direction(direction)
        .constraints(constraints)
        .split(area);
    for (i, child) in children.iter().enumerate() {
        if let Some(&chunk) = chunks.get(i) {
            render_view(f, chunk, child);
        }
    }
}

fn render_styled_list(f: &mut Frame, area: Rect, list: &StyledListNode) {
    let items: Vec<ListItem> = list
        .items
        .iter()
        .map(|(text, color)| ListItem::new(text.clone()).style(Style::default().fg(*color)))
        .collect();
    let widget = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(list.title.clone()),
        )
        .highlight_style(
            Style::default()
                .bg(ratatui::style::Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("> ");
    let mut state = ratatui::widgets::ListState::default();
    state.select(Some(list.selected));
    f.render_stateful_widget(widget, area, &mut state);
}

fn render_text(f: &mut Frame, area: Rect, text: &TextNode) {
    let num_lines = text.lines.len();
    let display_height = area.height.saturating_sub(2) as usize;
    let scroll_pos = if num_lines > display_height {
        (num_lines - display_height).saturating_sub(text.scroll as usize)
    } else {
        0
    };
    let p = Paragraph::new(text.lines.clone())
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(text.title.clone()),
        )
        .wrap(Wrap { trim: false })
        .scroll((scroll_pos as u16, 0));
    f.render_widget(p, area);
}

fn render_help_bar(f: &mut Frame, area: Rect, help: &HelpBarNode) {
    let mut spans = Vec::new();
    for (i, (key, label)) in help.bindings.iter().enumerate() {
        if i > 0 {
            spans.push(Span::raw("  "));
        }
        spans.push(Span::styled(
            format!(" {key} "),
            Style::default().fg(Color::Black).bg(Color::DarkGray),
        ));
        spans.push(Span::styled(
            format!(" {label}"),
            Style::default().fg(Color::DarkGray),
        ));
    }
    let line = Line::from(spans);
    let p = Paragraph::new(line);
    f.render_widget(p, area);
}

fn render_input_bar(f: &mut Frame, area: Rect, input: &InputBarNode) {
    let p = Paragraph::new(input.text.as_str()).block(
        Block::default()
            .borders(Borders::ALL)
            .title(input.title.clone()),
    );
    f.render_widget(p, area);
    f.set_cursor_position((area.x + input.text.len() as u16 + 1, area.y + 1));
}
