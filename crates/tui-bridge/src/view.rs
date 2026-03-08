//! Layer 2: Terminal view model.
//!
//! A small enum tree that sits between Dioxus state and Ratatui rendering.
//! Components produce `ViewNode` trees; the renderer consumes them.

use ratatui::layout::Constraint;
use ratatui::style::Color;
use ratatui::text::Line;

/// A terminal-native node in the view tree.
#[derive(Clone, Debug)]
pub enum ViewNode {
    Row(RowNode),
    Column(ColumnNode),
    StyledList(StyledListNode),
    Text(TextNode),
    InputBar(InputBarNode),
    HelpBar(HelpBarNode),
    Empty,
}

#[derive(Clone, Debug)]
pub struct RowNode {
    pub children: Vec<ViewNode>,
    pub constraints: Vec<Constraint>,
}

#[derive(Clone, Debug)]
pub struct ColumnNode {
    pub children: Vec<ViewNode>,
    pub constraints: Vec<Constraint>,
}

#[derive(Clone, Debug)]
pub struct StyledListNode {
    pub title: String,
    pub items: Vec<(String, Color)>,
    pub selected: usize,
}

#[derive(Clone, Debug)]
pub struct TextNode {
    pub title: String,
    pub lines: Vec<Line<'static>>,
    pub scroll: u16,
}

#[derive(Clone, Debug)]
pub struct InputBarNode {
    pub title: String,
    pub text: String,
}

#[derive(Clone, Debug)]
pub struct HelpBarNode {
    /// Key-label pairs, e.g. [("q", "Quit"), ("↑↓", "Navigate"), ...]
    pub bindings: Vec<(String, String)>,
}
