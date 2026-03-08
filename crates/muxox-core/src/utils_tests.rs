#[cfg(test)]
mod tests {
    use crate::utils::ansi_to_line;
    use ratatui::style::{Color, Modifier, Style};

    #[test]
    fn plain_text_no_escapes() {
        let line = ansi_to_line("hello world");
        assert_eq!(line.spans.len(), 1);
        assert_eq!(line.spans[0].content.as_ref(), "hello world");
        assert_eq!(line.spans[0].style, Style::default());
    }

    #[test]
    fn empty_string() {
        let line = ansi_to_line("");
        assert!(line.spans.is_empty());
    }

    #[test]
    fn reset_code_produces_default_style() {
        // "\x1b[0m" resets, text after should be default
        let line = ansi_to_line("\x1b[31mred\x1b[0mplain");
        assert_eq!(line.spans.len(), 2);
        assert_eq!(line.spans[0].style.fg, Some(Color::Red));
        assert_eq!(line.spans[1].style, Style::default());
        assert_eq!(line.spans[1].content.as_ref(), "plain");
    }

    #[test]
    fn basic_foreground_colors() {
        let cases = [
            (30, Color::Black),
            (31, Color::Red),
            (32, Color::Green),
            (33, Color::Yellow),
            (34, Color::Blue),
            (35, Color::Magenta),
            (36, Color::Cyan),
            (37, Color::White),
        ];
        for (code, expected) in cases {
            let input = format!("\x1b[{code}mtext");
            let line = ansi_to_line(&input);
            assert_eq!(
                line.spans[0].style.fg,
                Some(expected),
                "SGR code {code} should produce {expected:?}"
            );
        }
    }

    #[test]
    fn basic_background_colors() {
        let line = ansi_to_line("\x1b[41mtext");
        assert_eq!(line.spans[0].style.bg, Some(Color::Red));
    }

    #[test]
    fn bright_foreground_colors() {
        let line = ansi_to_line("\x1b[90mtext");
        assert_eq!(line.spans[0].style.fg, Some(Color::DarkGray));

        let line = ansi_to_line("\x1b[92mtext");
        assert_eq!(line.spans[0].style.fg, Some(Color::LightGreen));
    }

    #[test]
    fn bold_modifier() {
        let line = ansi_to_line("\x1b[1mbold text");
        assert!(line.spans[0].style.add_modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn italic_modifier() {
        let line = ansi_to_line("\x1b[3mitalic");
        assert!(line.spans[0].style.add_modifier.contains(Modifier::ITALIC));
    }

    #[test]
    fn underline_modifier() {
        let line = ansi_to_line("\x1b[4munderlined");
        assert!(line.spans[0]
            .style
            .add_modifier
            .contains(Modifier::UNDERLINED));
    }

    #[test]
    fn combined_bold_and_color() {
        let line = ansi_to_line("\x1b[1;31mbold red");
        let style = line.spans[0].style;
        assert_eq!(style.fg, Some(Color::Red));
        assert!(style.add_modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn color_256_indexed() {
        let line = ansi_to_line("\x1b[38;5;42mtext");
        assert_eq!(line.spans[0].style.fg, Some(Color::Indexed(42)));
    }

    #[test]
    fn color_24bit_rgb() {
        let line = ansi_to_line("\x1b[38;2;100;150;200mtext");
        assert_eq!(line.spans[0].style.fg, Some(Color::Rgb(100, 150, 200)));
    }

    #[test]
    fn background_256_indexed() {
        let line = ansi_to_line("\x1b[48;5;99mtext");
        assert_eq!(line.spans[0].style.bg, Some(Color::Indexed(99)));
    }

    #[test]
    fn background_24bit_rgb() {
        let line = ansi_to_line("\x1b[48;2;10;20;30mtext");
        assert_eq!(line.spans[0].style.bg, Some(Color::Rgb(10, 20, 30)));
    }

    #[test]
    fn multiple_segments() {
        let line = ansi_to_line("before\x1b[32mgreen\x1b[31mred\x1b[0mafter");
        assert_eq!(line.spans.len(), 4);
        assert_eq!(line.spans[0].content.as_ref(), "before");
        assert_eq!(line.spans[0].style, Style::default());
        assert_eq!(line.spans[1].content.as_ref(), "green");
        assert_eq!(line.spans[1].style.fg, Some(Color::Green));
        assert_eq!(line.spans[2].content.as_ref(), "red");
        assert_eq!(line.spans[2].style.fg, Some(Color::Red));
        assert_eq!(line.spans[3].content.as_ref(), "after");
    }

    #[test]
    fn non_sgr_escape_sequences_stripped() {
        // cursor movement codes like \x1b[2J should be consumed, not rendered
        let line = ansi_to_line("\x1b[2Jhello");
        let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(text, "hello");
    }

    #[test]
    fn unterminated_escape_drops_escape_keeps_text() {
        // escape at end of string with no terminator — text before it is preserved
        let line = ansi_to_line("text\x1b[");
        // The parser breaks on unterminated escapes; preceding text is captured
        assert!(!line.spans.is_empty());
        assert_eq!(line.spans[0].content.as_ref(), "text");
    }

    #[test]
    fn reset_fg_with_39() {
        let line = ansi_to_line("\x1b[31mred\x1b[39mreset");
        assert_eq!(line.spans[1].style.fg, Some(Color::Reset));
    }

    #[test]
    fn reset_bg_with_49() {
        let line = ansi_to_line("\x1b[41mred bg\x1b[49mreset");
        assert_eq!(line.spans[1].style.bg, Some(Color::Reset));
    }

    #[test]
    fn disable_bold_with_22() {
        let line = ansi_to_line("\x1b[1mbold\x1b[22mnot bold");
        assert!(!line.spans[1].style.add_modifier.contains(Modifier::BOLD));
    }
}
