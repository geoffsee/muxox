#[cfg(test)]
mod tests {
    use crate::render::render_view;
    use crate::view::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::layout::Constraint;
    use ratatui::style::Color;

    use dioxus_core::{Element, NoOpMutations, ScopeId, VNode, VirtualDom};
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;

    // --- Dioxus mark_dirty + render_immediate verification ---

    type ViewOut = Rc<RefCell<ViewNode>>;

    fn counting_component() -> Element {
        let counter = dioxus_core::consume_context::<Rc<Cell<u32>>>();
        let view_out = dioxus_core::consume_context::<ViewOut>();

        let n = counter.get() + 1;
        counter.set(n);

        *view_out.borrow_mut() = ViewNode::Text(TextNode {
            title: format!(" Render #{n} "),
            lines: vec![format!("count={n}").into()],
            scroll: 0,
        });

        Ok(VNode::placeholder())
    }

    #[test]
    fn dioxus_mark_dirty_does_not_rerun_component() {
        let counter = Rc::new(Cell::new(0u32));
        let view_out: ViewOut = Rc::new(RefCell::new(ViewNode::Empty));

        let mut vdom = VirtualDom::new(counting_component)
            .with_root_context(counter.clone())
            .with_root_context(view_out.clone());

        vdom.rebuild_in_place();
        assert_eq!(counter.get(), 1, "component should run once on rebuild");

        // mark_dirty + render_immediate does NOT re-run in dioxus 0.7
        vdom.mark_dirty(ScopeId::ROOT);
        vdom.render_immediate(&mut NoOpMutations);
        assert_eq!(
            counter.get(),
            1,
            "mark_dirty+render_immediate does not re-run component in 0.7"
        );
    }

    #[test]
    fn dioxus_rebuild_in_place_reruns_component() {
        let counter = Rc::new(Cell::new(0u32));
        let view_out: ViewOut = Rc::new(RefCell::new(ViewNode::Empty));

        let mut vdom = VirtualDom::new(counting_component)
            .with_root_context(counter.clone())
            .with_root_context(view_out.clone());

        vdom.rebuild_in_place();
        assert_eq!(counter.get(), 1);

        vdom.rebuild_in_place();
        assert_eq!(counter.get(), 2, "rebuild_in_place should re-run component");

        vdom.rebuild_in_place();
        assert_eq!(counter.get(), 3);

        // Verify the view_out reflects the latest render
        let view = view_out.borrow();
        match &*view {
            ViewNode::Text(t) => {
                assert!(t.title.contains("3"), "title should reflect render #3");
            }
            _ => panic!("expected Text node"),
        }
    }

    fn test_terminal(w: u16, h: u16) -> Terminal<TestBackend> {
        Terminal::new(TestBackend::new(w, h)).unwrap()
    }

    // --- ViewNode construction ---

    #[test]
    fn empty_node_renders_without_panic() {
        let mut term = test_terminal(40, 10);
        term.draw(|f| render_view(f, f.area(), &ViewNode::Empty))
            .unwrap();
    }

    #[test]
    fn text_node_renders_title_and_content() {
        let mut term = test_terminal(40, 10);
        let node = ViewNode::Text(TextNode {
            title: " Title ".into(),
            lines: vec!["hello world".into()],
            scroll: 0,
        });
        term.draw(|f| render_view(f, f.area(), &node)).unwrap();
        let buf = term.backend().buffer().clone();
        let text: String = buf.content.iter().map(|c| c.symbol()).collect();
        assert!(text.contains("Title"), "buffer should contain the title");
        assert!(
            text.contains("hello world"),
            "buffer should contain the content"
        );
    }

    #[test]
    fn styled_list_renders_items() {
        let mut term = test_terminal(40, 10);
        let node = ViewNode::StyledList(StyledListNode {
            title: " List ".into(),
            items: vec![
                ("● Alpha".into(), Color::Green),
                ("● Bravo".into(), Color::Red),
            ],
            selected: 0,
        });
        term.draw(|f| render_view(f, f.area(), &node)).unwrap();
        let buf = term.backend().buffer().clone();
        let text: String = buf.content.iter().map(|c| c.symbol()).collect();
        assert!(text.contains("Alpha"));
        assert!(text.contains("Bravo"));
    }

    #[test]
    fn styled_list_selected_index_respected() {
        let mut term = test_terminal(40, 10);
        let node = ViewNode::StyledList(StyledListNode {
            title: " List ".into(),
            items: vec![
                ("A".into(), Color::White),
                ("B".into(), Color::White),
                ("C".into(), Color::White),
            ],
            selected: 2,
        });
        // Should not panic — selection index 2 is valid
        term.draw(|f| render_view(f, f.area(), &node)).unwrap();
    }

    #[test]
    fn input_bar_renders_text_and_title() {
        let mut term = test_terminal(60, 5);
        let node = ViewNode::InputBar(InputBarNode {
            title: " Input ".into(),
            text: "typed text".into(),
        });
        term.draw(|f| render_view(f, f.area(), &node)).unwrap();
        let buf = term.backend().buffer().clone();
        let text: String = buf.content.iter().map(|c| c.symbol()).collect();
        assert!(text.contains("Input"));
        assert!(text.contains("typed text"));
    }

    // --- Layout composition ---

    #[test]
    fn row_splits_horizontally() {
        let mut term = test_terminal(80, 10);
        let node = ViewNode::Row(RowNode {
            children: vec![
                ViewNode::Text(TextNode {
                    title: " Left ".into(),
                    lines: vec!["L".into()],
                    scroll: 0,
                }),
                ViewNode::Text(TextNode {
                    title: " Right ".into(),
                    lines: vec!["R".into()],
                    scroll: 0,
                }),
            ],
            constraints: vec![Constraint::Percentage(50), Constraint::Percentage(50)],
        });
        term.draw(|f| render_view(f, f.area(), &node)).unwrap();
        let buf = term.backend().buffer().clone();
        let text: String = buf.content.iter().map(|c| c.symbol()).collect();
        assert!(text.contains("Left"));
        assert!(text.contains("Right"));
    }

    #[test]
    fn column_splits_vertically() {
        let mut term = test_terminal(40, 20);
        let node = ViewNode::Column(ColumnNode {
            children: vec![
                ViewNode::Text(TextNode {
                    title: " Top ".into(),
                    lines: vec!["T".into()],
                    scroll: 0,
                }),
                ViewNode::Text(TextNode {
                    title: " Bottom ".into(),
                    lines: vec!["B".into()],
                    scroll: 0,
                }),
            ],
            constraints: vec![Constraint::Percentage(50), Constraint::Percentage(50)],
        });
        term.draw(|f| render_view(f, f.area(), &node)).unwrap();
        let buf = term.backend().buffer().clone();
        let text: String = buf.content.iter().map(|c| c.symbol()).collect();
        assert!(text.contains("Top"));
        assert!(text.contains("Bottom"));
    }

    #[test]
    fn nested_layout() {
        let mut term = test_terminal(80, 20);
        let node = ViewNode::Column(ColumnNode {
            children: vec![
                ViewNode::Row(RowNode {
                    children: vec![
                        ViewNode::StyledList(StyledListNode {
                            title: " Services ".into(),
                            items: vec![("● svc1".into(), Color::Green)],
                            selected: 0,
                        }),
                        ViewNode::Text(TextNode {
                            title: " Logs ".into(),
                            lines: vec!["log line 1".into()],
                            scroll: 0,
                        }),
                    ],
                    constraints: vec![Constraint::Percentage(30), Constraint::Percentage(70)],
                }),
                ViewNode::InputBar(InputBarNode {
                    title: " Input ".into(),
                    text: "hi".into(),
                }),
            ],
            constraints: vec![Constraint::Min(1), Constraint::Length(3)],
        });
        term.draw(|f| render_view(f, f.area(), &node)).unwrap();
        let buf = term.backend().buffer().clone();
        let text: String = buf.content.iter().map(|c| c.symbol()).collect();
        assert!(text.contains("Services"));
        assert!(text.contains("Logs"));
        assert!(text.contains("Input"));
    }

    // --- Scroll behavior ---

    #[test]
    fn text_scroll_offset_zero_shows_tail() {
        let mut term = test_terminal(40, 6); // 4 lines visible (6 - 2 border)
        let lines: Vec<ratatui::text::Line<'static>> =
            (0..20).map(|i| format!("line {i}").into()).collect();
        let node = ViewNode::Text(TextNode {
            title: " Log ".into(),
            lines,
            scroll: 0,
        });
        term.draw(|f| render_view(f, f.area(), &node)).unwrap();
        let buf = term.backend().buffer().clone();
        let text: String = buf.content.iter().map(|c| c.symbol()).collect();
        // With scroll=0, should show the tail (lines near 19)
        assert!(text.contains("line 19") || text.contains("line 18"));
    }

    #[test]
    fn text_scroll_offset_nonzero_shows_earlier_lines() {
        let mut term = test_terminal(40, 6);
        let lines: Vec<ratatui::text::Line<'static>> =
            (0..20).map(|i| format!("line {i}").into()).collect();
        let node = ViewNode::Text(TextNode {
            title: " Log ".into(),
            lines,
            scroll: 10,
        });
        term.draw(|f| render_view(f, f.area(), &node)).unwrap();
        let buf = term.backend().buffer().clone();
        let text: String = buf.content.iter().map(|c| c.symbol()).collect();
        // Scrolled back 10 lines from tail
        assert!(!text.contains("line 19"));
    }

    // --- Edge cases ---

    #[test]
    fn zero_size_terminal_does_not_panic() {
        // Terminals can report zero-size areas
        let mut term = test_terminal(0, 0);
        let node = ViewNode::Row(RowNode {
            children: vec![ViewNode::Empty],
            constraints: vec![Constraint::Min(1)],
        });
        // Should not panic
        let _ = term.draw(|f| render_view(f, f.area(), &node));
    }

    #[test]
    fn more_children_than_constraints() {
        let mut term = test_terminal(40, 10);
        let node = ViewNode::Row(RowNode {
            children: vec![
                ViewNode::Text(TextNode {
                    title: "A".into(),
                    lines: vec![],
                    scroll: 0,
                }),
                ViewNode::Text(TextNode {
                    title: "B".into(),
                    lines: vec![],
                    scroll: 0,
                }),
                ViewNode::Text(TextNode {
                    title: "C".into(),
                    lines: vec![],
                    scroll: 0,
                }),
            ],
            constraints: vec![Constraint::Percentage(50), Constraint::Percentage(50)],
        });
        // Third child has no constraint chunk — should not panic
        term.draw(|f| render_view(f, f.area(), &node)).unwrap();
    }
}
