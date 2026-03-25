// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Geoff Seemueller
// This file is part of muxox, released under the MIT License.

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use std::fs::OpenOptions;

pub fn debug_file_log(msg: &str) {
    if let Ok(path) = std::env::var("MUXOX_DEBUG_FILE")
        && let Ok(mut f) = OpenOptions::new().create(true).append(true).open(path)
    {
        use std::io::Write as _;
        let _ = writeln!(f, "[{:?}] {}", std::thread::current().id(), msg);
    }
}

#[cfg(unix)]
pub fn shell_program() -> &'static str {
    "/bin/sh"
}

#[cfg(unix)]
pub fn shell_flag() -> &'static str {
    "-c"
}

#[cfg(unix)]
pub fn shell_exec(cmd: &str) -> String {
    cmd.to_string()
}

#[cfg(windows)]
pub fn shell_program() -> &'static str {
    "cmd.exe"
}

#[cfg(windows)]
pub fn shell_flag() -> &'static str {
    "/C"
}

#[cfg(windows)]
pub fn shell_exec(cmd: &str) -> String {
    cmd.to_string()
}

#[cfg(unix)]
pub fn interactive_program() -> &'static str {
    "script"
}

#[cfg(unix)]
pub fn interactive_args(cmd: &str) -> Vec<String> {
    vec![
        "-q".to_string(),
        "/dev/null".to_string(),
        shell_program().to_string(),
        shell_flag().to_string(),
        shell_exec(cmd),
    ]
}

pub fn ansi_to_line(s: &str) -> Line<'static> {
    let mut spans: Vec<Span> = Vec::new();
    let mut style = Style::default();

    let bytes = s.as_bytes();
    let mut i = 0usize;
    let mut text_start = 0usize;

    while i < bytes.len() {
        if bytes[i] == 0x1B && i + 1 < bytes.len() && bytes[i + 1] == b'[' {
            if text_start < i {
                let seg = match std::str::from_utf8(&bytes[text_start..i]) {
                    Ok(seg) => seg.to_string(),
                    Err(_) => String::from_utf8_lossy(&bytes[text_start..i]).to_string(),
                };
                if !seg.is_empty() {
                    spans.push(Span::styled(seg, style));
                }
            }

            let mut j = i + 2;
            while j < bytes.len() {
                let b = bytes[j];
                if (0x40..=0x7E).contains(&b) {
                    break;
                }
                j += 1;
            }
            if j >= bytes.len() {
                break;
            }
            let final_byte = bytes[j] as char;
            if final_byte == 'm' {
                let params = std::str::from_utf8(&bytes[i + 2..j]).unwrap_or("");
                let params = params.trim_start_matches('?');
                apply_sgr(params, &mut style);
            }

            i = j + 1;
            text_start = i;
            continue;
        }
        i += 1;
    }

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
                if idx + 2 < parts.len() && parts[idx + 1] == "5" {
                    if let Ok(color_idx) = parts[idx + 2].parse::<u8>() {
                        let color = Color::Indexed(color_idx);
                        if code == 38 {
                            *style = style.fg(color);
                        } else {
                            *style = style.bg(color);
                        }
                        idx += 2;
                    }
                } else if idx + 4 < parts.len()
                    && parts[idx + 1] == "2"
                    && let (Ok(r), Ok(g), Ok(b)) = (
                        parts[idx + 2].parse::<u8>(),
                        parts[idx + 3].parse::<u8>(),
                        parts[idx + 4].parse::<u8>(),
                    )
                {
                    let color = Color::Rgb(r, g, b);
                    if code == 38 {
                        *style = style.fg(color);
                    } else {
                        *style = style.bg(color);
                    }
                    idx += 4;
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
        7 => Color::White,
        _ => Color::Reset,
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
        7 => Color::Gray,
        _ => Color::Reset,
    }
}
