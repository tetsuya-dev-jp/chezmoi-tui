use ratatui::prelude::{Color, Line, Modifier, Span, Style};

pub(crate) fn colorized_diff_lines(diff: &str) -> Vec<Line<'static>> {
    if diff.trim().is_empty() {
        return vec![Line::from(Span::styled(
            "No diff available.",
            Style::default().fg(Color::DarkGray),
        ))];
    }

    let mut out = Vec::new();

    for raw in diff.lines() {
        if let Some(line) = ansi_line_to_spans(raw) {
            out.push(line);
            continue;
        }

        if raw.starts_with("diff --git ") {
            out.push(Line::from(Span::styled(
                raw.to_string(),
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            )));
            continue;
        }

        if raw.starts_with("index ")
            || raw.starts_with("new file mode")
            || raw.starts_with("deleted file mode")
            || raw.starts_with("similarity index")
            || raw.starts_with("rename from ")
            || raw.starts_with("rename to ")
        {
            out.push(Line::from(Span::styled(
                raw.to_string(),
                Style::default().fg(Color::DarkGray),
            )));
            continue;
        }

        if raw.starts_with("--- ") {
            out.push(Line::from(Span::styled(
                raw.to_string(),
                Style::default().fg(Color::Red),
            )));
            continue;
        }

        if raw.starts_with("+++ ") {
            out.push(Line::from(Span::styled(
                raw.to_string(),
                Style::default().fg(Color::Green),
            )));
            continue;
        }

        if raw.starts_with("@@") {
            out.push(Line::from(Span::styled(
                raw.to_string(),
                Style::default().fg(Color::Yellow),
            )));
            continue;
        }

        if raw.starts_with("\\ No newline at end of file") {
            out.push(Line::from(Span::styled(
                raw.to_string(),
                Style::default()
                    .fg(Color::DarkGray)
                    .add_modifier(Modifier::ITALIC),
            )));
            continue;
        }

        if raw.starts_with('+') {
            out.push(Line::from(Span::styled(
                raw.to_string(),
                Style::default().fg(Color::Green).bg(Color::Rgb(12, 32, 12)),
            )));
            continue;
        }

        if raw.starts_with('-') {
            out.push(Line::from(Span::styled(
                raw.to_string(),
                Style::default().fg(Color::Red).bg(Color::Rgb(40, 14, 14)),
            )));
            continue;
        }

        if raw.starts_with(' ') {
            out.push(Line::from(Span::styled(
                raw.to_string(),
                Style::default().fg(Color::Gray),
            )));
            continue;
        }

        out.push(Line::from(raw.to_string()));
    }

    out
}

fn ansi_line_to_spans(input: &str) -> Option<Line<'static>> {
    if !input.contains('\u{1b}') {
        return None;
    }

    let mut spans = Vec::new();
    let mut text = String::new();
    let mut chars = input.chars().peekable();
    let mut style = Style::default();

    while let Some(ch) = chars.next() {
        if ch == '\u{1b}' && matches!(chars.peek(), Some('[')) {
            if !text.is_empty() {
                spans.push(Span::styled(std::mem::take(&mut text), style));
            }
            chars.next();

            let mut params = String::new();
            for next in chars.by_ref() {
                if ('@'..='~').contains(&next) {
                    if next == 'm' {
                        style = apply_ansi_sgr(style, &params);
                    }
                    break;
                }
                params.push(next);
            }
            continue;
        }

        text.push(ch);
    }

    if !text.is_empty() {
        spans.push(Span::styled(text, style));
    }

    Some(Line::from(spans))
}

fn apply_ansi_sgr(mut style: Style, params: &str) -> Style {
    let codes: Vec<u16> = if params.is_empty() {
        vec![0]
    } else {
        params
            .split(';')
            .filter_map(|part| part.parse::<u16>().ok())
            .collect()
    };

    let mut i = 0usize;
    while i < codes.len() {
        match codes[i] {
            0 => style = Style::default(),
            1 => style = style.add_modifier(Modifier::BOLD),
            3 => style = style.add_modifier(Modifier::ITALIC),
            4 => style = style.add_modifier(Modifier::UNDERLINED),
            22 => style = style.remove_modifier(Modifier::BOLD),
            23 => style = style.remove_modifier(Modifier::ITALIC),
            24 => style = style.remove_modifier(Modifier::UNDERLINED),
            30..=37 => style = style.fg(ansi_basic_color(codes[i])),
            39 => style.fg = None,
            40..=47 => style = style.bg(ansi_basic_color(codes[i] - 10)),
            49 => style.bg = None,
            90..=97 => style = style.fg(ansi_bright_color(codes[i])),
            100..=107 => style = style.bg(ansi_bright_color(codes[i] - 10)),
            38 | 48 => {
                let is_fg = codes[i] == 38;
                if let Some((color, consumed)) = parse_ansi_extended_color(&codes[i + 1..]) {
                    style = if is_fg {
                        style.fg(color)
                    } else {
                        style.bg(color)
                    };
                    i += consumed;
                }
            }
            _ => {}
        }
        i += 1;
    }

    style
}

fn parse_ansi_extended_color(codes: &[u16]) -> Option<(Color, usize)> {
    match codes {
        [5, index, ..] => Some((Color::Indexed(*index as u8), 2)),
        [2, r, g, b, ..] => Some((Color::Rgb(*r as u8, *g as u8, *b as u8), 4)),
        _ => None,
    }
}

fn ansi_basic_color(code: u16) -> Color {
    match code {
        30 => Color::Black,
        31 => Color::Red,
        32 => Color::Green,
        33 => Color::Yellow,
        34 => Color::Blue,
        35 => Color::Magenta,
        36 => Color::Cyan,
        37 => Color::Gray,
        _ => Color::Reset,
    }
}

fn ansi_bright_color(code: u16) -> Color {
    match code {
        90 => Color::DarkGray,
        91 => Color::LightRed,
        92 => Color::LightGreen,
        93 => Color::LightYellow,
        94 => Color::LightBlue,
        95 => Color::LightMagenta,
        96 => Color::LightCyan,
        97 => Color::White,
        _ => Color::Reset,
    }
}
