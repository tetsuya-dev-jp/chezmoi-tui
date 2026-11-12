use ratatui::prelude::{Color, Line, Modifier, Span, Style};

pub(crate) fn colorized_diff_lines(diff: &str) -> Vec<Line<'static>> {
    if diff.trim().is_empty() {
        return vec![Line::from(Span::styled(
            "No diff available.",
            Style::default().fg(Color::DarkGray),
        ))];
    }

    let mut out = Vec::new();
    let mut old_line = 0usize;
    let mut new_line = 0usize;
    let mut in_hunk = false;

    for raw in diff.lines() {
        let (plain, spans) = ansi_line_to_spans(raw);

        if plain.starts_with("diff --git ") {
            in_hunk = false;
            out.push(Line::from(restyle_preserving_ansi(
                spans,
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            )));
            continue;
        }

        if let Some((old_start, new_start)) = parse_hunk_header(&plain) {
            old_line = old_start;
            new_line = new_start;
            in_hunk = true;
            out.push(Line::from(restyle_preserving_ansi(
                spans,
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            )));
            continue;
        }

        if plain.starts_with("index ")
            || plain.starts_with("new file mode")
            || plain.starts_with("deleted file mode")
            || plain.starts_with("similarity index")
            || plain.starts_with("rename from ")
            || plain.starts_with("rename to ")
        {
            out.push(Line::from(restyle_preserving_ansi(
                spans,
                Style::default().fg(Color::DarkGray),
            )));
            continue;
        }

        if plain.starts_with("--- ") {
            out.push(Line::from(restyle_preserving_ansi(
                spans,
                Style::default()
                    .fg(Color::LightRed)
                    .add_modifier(Modifier::BOLD),
            )));
            continue;
        }

        if plain.starts_with("+++ ") {
            out.push(Line::from(restyle_preserving_ansi(
                spans,
                Style::default()
                    .fg(Color::LightGreen)
                    .add_modifier(Modifier::BOLD),
            )));
            continue;
        }

        if plain.starts_with("\\ No newline at end of file") {
            out.push(Line::from(restyle_preserving_ansi(
                spans,
                Style::default()
                    .fg(Color::DarkGray)
                    .add_modifier(Modifier::ITALIC),
            )));
            continue;
        }

        if in_hunk {
            let (old_number, new_number, style, advance) = match plain.as_bytes().first() {
                Some(b' ') => (
                    Some(old_line),
                    Some(new_line),
                    Style::default().fg(Color::Gray),
                    true,
                ),
                Some(b'-') => (
                    Some(old_line),
                    None,
                    Style::default().fg(Color::LightRed),
                    true,
                ),
                Some(b'+') => (
                    None,
                    Some(new_line),
                    Style::default().fg(Color::LightGreen),
                    true,
                ),
                _ => (None, None, Style::default().fg(Color::Gray), false),
            };

            let mut line_spans = Vec::new();
            if old_number.is_some() || new_number.is_some() {
                line_spans.push(Span::styled(
                    format!(
                        "{} {} │ ",
                        format_line_number(old_number),
                        format_line_number(new_number)
                    ),
                    Style::default().fg(Color::DarkGray),
                ));
            }
            line_spans.extend(restyle_preserving_ansi(spans, style));
            out.push(Line::from(line_spans));

            if advance {
                if old_number.is_some() {
                    old_line += 1;
                }
                if new_number.is_some() {
                    new_line += 1;
                }
            }
            continue;
        }

        out.push(Line::from(spans));
    }

    out
}

fn format_line_number(value: Option<usize>) -> String {
    value.map_or_else(|| "    ".to_string(), |value| format!("{value:>4}"))
}

fn parse_hunk_header(line: &str) -> Option<(usize, usize)> {
    let rest = line.strip_prefix("@@ -")?;
    let (old, rest) = rest.split_once(" +")?;
    let new = rest.split_once(" @@")?.0;
    let old_start = old.split_once(',').map_or(old, |(start, _)| start);
    let new_start = new.split_once(',').map_or(new, |(start, _)| start);
    Some((old_start.parse().ok()?, new_start.parse().ok()?))
}

fn restyle_preserving_ansi(mut spans: Vec<Span<'static>>, fallback: Style) -> Vec<Span<'static>> {
    for span in &mut spans {
        if span.style.fg.is_none() {
            span.style = span.style.fg(fallback.fg.unwrap_or(Color::Reset));
        }
        if fallback.add_modifier != Modifier::empty() {
            span.style = span.style.add_modifier(fallback.add_modifier);
        }
    }
    spans
}

fn ansi_line_to_spans(input: &str) -> (String, Vec<Span<'static>>) {
    let mut spans = Vec::new();
    let mut text = String::new();
    let mut plain = String::new();
    let mut chars = input.chars().peekable();
    let mut style = Style::default();

    while let Some(ch) = chars.next() {
        if ch != '\u{1b}' {
            if ch == '\t' || !ch.is_control() {
                text.push(ch);
                plain.push(ch);
            }
            continue;
        }

        push_ansi_text(&mut spans, &mut text, style);
        match chars.peek().copied() {
            Some('[') => {
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
            }
            Some(']') => {
                chars.next();
                consume_osc(&mut chars);
            }
            Some(_) => {
                chars.next();
            }
            None => {}
        }
    }
    push_ansi_text(&mut spans, &mut text, style);
    (plain, spans)
}

fn consume_osc(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) {
    while let Some(next) = chars.next() {
        if next == '\u{7}' {
            break;
        }
        if next == '\u{1b}' && chars.peek() == Some(&'\\') {
            chars.next();
            break;
        }
    }
}

fn push_ansi_text(spans: &mut Vec<Span<'static>>, text: &mut String, style: Style) {
    if !text.is_empty() {
        spans.push(Span::styled(std::mem::take(text), style));
    }
}

fn apply_ansi_sgr(mut style: Style, params: &str) -> Style {
    let codes: Vec<u16> = if params.is_empty() {
        vec![0]
    } else {
        params
            .split(';')
            .map(|part| part.parse::<u16>().unwrap_or(0))
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

#[cfg(test)]
mod tests {
    use super::colorized_diff_lines;

    fn text(line: &ratatui::text::Line<'_>) -> String {
        line.spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect()
    }

    #[test]
    fn line_numbers_follow_hunks_and_ansi_colors_are_preserved() {
        let diff = concat!(
            "\u{1b}[34mdiff --git a/a b/a\u{1b}[0m\n",
            "\u{1b}[33m@@ -7,2 +9,2 @@\u{1b}[0m\n",
            " context\n",
            "\u{1b}[31m-old\u{1b}[0m\n",
            "\u{1b}[32m+new\u{1b}[0m\n",
        );
        let lines = colorized_diff_lines(diff);
        assert_eq!(text(&lines[2]), "   7    9 │  context");
        assert_eq!(text(&lines[3]), "   8      │ -old");
        assert_eq!(text(&lines[4]), "       10 │ +new");
        assert!(lines.iter().all(|line| !text(line).contains('\u{1b}')));
    }

    #[test]
    fn osc_and_non_sgr_escapes_never_reach_the_buffer() {
        let diff = "\u{1b}]0;private title\u{7}\u{1b}[33m@@ -1 +1 @@\u{1b}[0m\n\u{1b}[31m-old\u{1b}[0m\n\u{1b}[32m+new\u{1b}[0m\n";
        let rendered = colorized_diff_lines(diff)
            .iter()
            .map(text)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(!rendered.contains('\u{1b}'));
        assert!(!rendered.contains("private title"));
        assert!(rendered.contains("-old"));
        assert!(rendered.contains("+new"));
    }
}
