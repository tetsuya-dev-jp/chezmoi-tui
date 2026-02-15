use crate::app::{App, ConfirmStep, DetailKind, InputKind, ModalState, PaneFocus};
use crate::domain::{Action, ListView};
use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::prelude::{Alignment, Color, Line, Modifier, Span, Style};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap};
use std::path::Path;

pub fn draw(frame: &mut Frame, app: &mut App) {
    let outer = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(2)])
        .split(frame.area());

    let main = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(35), Constraint::Percentage(65)])
        .split(outer[0]);

    let right = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Percentage(65), Constraint::Percentage(35)])
        .split(main[1]);

    draw_list(frame, app, main[0]);
    draw_detail(frame, app, right[0]);
    draw_logs(frame, app, right[1]);
    draw_status_bar(frame, app, outer[1]);
    draw_modal(frame, app);
}

fn draw_list(frame: &mut Frame, app: &mut App, area: Rect) {
    let items: Vec<ListItem> = app.current_items().into_iter().map(ListItem::new).collect();
    let viewport_rows = area.height.saturating_sub(2) as usize;
    app.sync_list_scroll(viewport_rows);

    let border_style = if app.focus == PaneFocus::List {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default()
    };

    let list = List::new(items)
        .block(
            Block::default()
                .title(format!(" {} ", app.view.title()))
                .borders(Borders::ALL)
                .border_style(border_style),
        )
        .highlight_style(
            Style::default()
                .fg(Color::Black)
                .bg(Color::LightGreen)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("▶ ");

    let mut state = ListState::default().with_offset(app.list_scroll());
    if app.current_len() > 0 {
        state.select(Some(app.selected_index));
    }

    frame.render_stateful_widget(list, area, &mut state);
}

fn draw_detail(frame: &mut Frame, app: &App, area: Rect) {
    let border_style = if app.focus == PaneFocus::Detail {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default()
    };

    let lines = if app.detail_text.trim().is_empty() {
        if app.view == ListView::Unmanaged && app.selected_is_directory() {
            vec![Line::from("")]
        } else {
            vec![
                Line::from("Detail is not loaded yet."),
                Line::from("Enter / d: diff, v: file preview"),
            ]
        }
    } else if app.detail_kind == DetailKind::Diff {
        colorized_diff_lines(&app.detail_text)
    } else {
        colorized_preview_lines(app.detail_target.as_deref(), &app.detail_text)
    };

    let paragraph = Paragraph::new(lines)
        .block(
            Block::default()
                .title(format!(" {} ", app.detail_title))
                .borders(Borders::ALL)
                .border_style(border_style),
        )
        .scroll((app.detail_scroll.min(u16::MAX as usize) as u16, 0))
        .wrap(Wrap { trim: false });

    frame.render_widget(paragraph, area);
}

fn draw_logs(frame: &mut Frame, app: &App, area: Rect) {
    let border_style = if app.focus == PaneFocus::Log {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default()
    };

    let lines: Vec<Line> = app
        .logs
        .iter()
        .map(|line| Line::from(line.as_str()))
        .collect();
    let scroll = log_scroll_offset(lines.len(), area.height);

    let paragraph = Paragraph::new(lines)
        .block(
            Block::default()
                .title(" Log ")
                .borders(Borders::ALL)
                .border_style(border_style),
        )
        .scroll((scroll, 0))
        .wrap(Wrap { trim: false });

    frame.render_widget(paragraph, area);
}

fn log_scroll_offset(total_lines: usize, area_height: u16) -> u16 {
    let visible_rows = area_height.saturating_sub(2) as usize;
    total_lines
        .saturating_sub(visible_rows.max(1))
        .min(u16::MAX as usize) as u16
}

fn draw_status_bar(frame: &mut Frame, app: &App, area: Rect) {
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Length(1)])
        .split(area);

    let mut top = Vec::new();
    top.extend(badge(
        if app.busy { " BUSY " } else { " IDLE " },
        if app.busy {
            Style::default().bg(Color::Yellow).fg(Color::Black)
        } else {
            Style::default().bg(Color::DarkGray).fg(Color::White)
        },
    ));
    top.push(Span::raw(" "));
    top.extend(badge(
        format!(" VIEW {} ", app.view.title()),
        Style::default().bg(Color::Blue).fg(Color::Black),
    ));
    top.push(Span::raw(" "));
    top.extend(badge(
        format!(" FOCUS {} ", focus_name(app.focus)),
        Style::default().bg(Color::Cyan).fg(Color::Black),
    ));
    top.push(Span::raw(" "));
    top.extend(badge(
        format!(" ITEMS {} ", app.current_len()),
        Style::default().bg(Color::DarkGray).fg(Color::White),
    ));

    let mut bottom = Vec::new();
    bottom.extend(hint("1/2/3", "View", false));
    bottom.extend(hint("Tab", "Focus", false));

    if app.focus == PaneFocus::Detail {
        bottom.extend(hint("j/k ↑/↓", "Scroll", true));
        bottom.extend(hint("PgUp/PgDn", "Page", true));
        bottom.extend(hint("Ctrl+u/d", "HalfPage", true));
    } else {
        bottom.extend(hint("j/k ↑/↓", "Move", true));
        bottom.extend(hint("h/l ←/→", "Fold", false));
    }

    bottom.extend(hint("d", "Diff", false));
    bottom.extend(hint("v", "Preview", false));
    bottom.extend(hint("a", "Action", false));
    bottom.extend(hint("q", "Quit", false));

    let top_paragraph = Paragraph::new(Line::from(top))
        .alignment(Alignment::Left)
        .style(Style::default().bg(Color::Black));
    let bottom_paragraph = Paragraph::new(Line::from(bottom))
        .alignment(Alignment::Left)
        .style(Style::default().bg(Color::Black));

    frame.render_widget(top_paragraph, rows[0]);
    frame.render_widget(bottom_paragraph, rows[1]);
}

fn focus_name(focus: PaneFocus) -> &'static str {
    match focus {
        PaneFocus::List => "List",
        PaneFocus::Detail => "Detail",
        PaneFocus::Log => "Log",
    }
}

fn badge<T: Into<String>>(text: T, style: Style) -> Vec<Span<'static>> {
    vec![Span::styled(text.into(), style)]
}

fn hint(key: &str, label: &str, emphasized: bool) -> Vec<Span<'static>> {
    let key_style = if emphasized {
        Style::default()
            .bg(Color::LightBlue)
            .fg(Color::Black)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().bg(Color::DarkGray).fg(Color::White)
    };

    vec![
        Span::styled(format!(" {} ", key), key_style),
        Span::raw(" "),
        Span::styled(label.to_string(), Style::default().fg(Color::Gray)),
        Span::raw("  "),
    ]
}

fn draw_modal(frame: &mut Frame, app: &App) {
    match &app.modal {
        ModalState::None => {}
        ModalState::ActionMenu { selected, filter } => {
            let area = centered_rect(60, 70, frame.area());
            frame.render_widget(Clear, area);
            let sections = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Length(3), Constraint::Min(1)])
                .split(area);

            let query = if filter.is_empty() {
                "<type to filter>".to_string()
            } else {
                filter.to_string()
            };
            let query_style = if filter.is_empty() {
                Style::default().fg(Color::DarkGray)
            } else {
                Style::default().fg(Color::Yellow)
            };
            let filter_widget = Paragraph::new(vec![
                Line::from(vec![
                    Span::styled("query: ", Style::default().fg(Color::Gray)),
                    Span::styled(query, query_style),
                ]),
                Line::from("Backspace: delete  Up/Down: select  Enter: run  Esc: close"),
            ])
            .block(
                Block::default()
                    .title(" Action Filter ")
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(Color::LightBlue)),
            )
            .wrap(Wrap { trim: false });
            frame.render_widget(filter_widget, sections[0]);

            let indices = App::action_menu_indices(app.view, filter);
            let items: Vec<ListItem> = if indices.is_empty() {
                vec![ListItem::new("No actions match the current filter")]
            } else {
                indices
                    .iter()
                    .filter_map(|index| App::action_by_index(*index))
                    .map(action_menu_item)
                    .collect()
            };

            let list = List::new(items)
                .block(
                    Block::default()
                        .title(" Action Menu ")
                        .borders(Borders::ALL)
                        .border_style(Style::default().fg(Color::Cyan)),
                )
                .highlight_style(
                    Style::default()
                        .fg(Color::Black)
                        .bg(Color::LightYellow)
                        .add_modifier(Modifier::BOLD),
                )
                .highlight_symbol("▶ ");

            let mut state = ListState::default();
            if !indices.is_empty() {
                state.select(Some((*selected).min(indices.len().saturating_sub(1))));
            }
            frame.render_stateful_widget(list, sections[1], &mut state);
        }
        ModalState::Confirm {
            request,
            step,
            typed,
        } => {
            let area = centered_rect(70, 45, frame.area());
            frame.render_widget(Clear, area);
            let title = match step {
                ConfirmStep::Primary => " Confirm Action ",
                ConfirmStep::DangerPhrase => " Dangerous Action ",
            };

            let mut lines = vec![
                Line::from(format!("action: {}", request.action.label())),
                Line::from(format!(
                    "target: {}",
                    request
                        .target
                        .as_ref()
                        .map(|p| p.display().to_string())
                        .unwrap_or_else(|| "(none)".to_string())
                )),
            ];

            if let Some(attrs) = &request.chattr_attrs {
                lines.push(Line::from(format!("attributes: {}", attrs)));
            }

            lines.push(Line::from(""));
            match step {
                ConfirmStep::Primary => {
                    lines.push(Line::from("Enter: Run  Esc: Cancel"));
                    if request.action.is_dangerous() {
                        lines.push(Line::from(
                            "This is a dangerous action. A confirmation phrase is required next.",
                        ));
                    }
                }
                ConfirmStep::DangerPhrase => {
                    lines.push(Line::from(
                        "Type the confirmation phrase and press Enter to run, Esc to cancel.",
                    ));
                    if let Some(phrase) = request.action.confirm_phrase() {
                        lines.push(
                            Line::from(format!("required: {}", phrase)).style(
                                Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
                            ),
                        );
                    }
                    lines.push(
                        Line::from(format!("input: {}", typed))
                            .style(Style::default().fg(Color::Yellow)),
                    );
                }
            }

            let p = Paragraph::new(lines)
                .block(
                    Block::default()
                        .title(title)
                        .borders(Borders::ALL)
                        .border_style(Style::default().fg(Color::LightRed)),
                )
                .wrap(Wrap { trim: false });

            frame.render_widget(p, area);
        }
        ModalState::Input {
            kind,
            request,
            value,
        } => {
            let area = centered_rect(70, 35, frame.area());
            frame.render_widget(Clear, area);

            let prompt = match kind {
                InputKind::ChattrAttrs => "chattr attributes (e.g. private,template)",
            };

            let lines = vec![
                Line::from(format!("action: {}", request.action.label())),
                Line::from(format!(
                    "target: {}",
                    request
                        .target
                        .as_ref()
                        .map(|p| p.display().to_string())
                        .unwrap_or_else(|| "(none)".to_string())
                )),
                Line::from(""),
                Line::from(prompt),
                Line::from(format!("> {}", value)).style(Style::default().fg(Color::Yellow)),
                Line::from("Enter: Confirm  Esc: Cancel"),
            ];

            let p = Paragraph::new(lines)
                .block(
                    Block::default()
                        .title(" Input ")
                        .borders(Borders::ALL)
                        .border_style(Style::default().fg(Color::LightBlue)),
                )
                .wrap(Wrap { trim: false });
            frame.render_widget(p, area);
        }
    }
}

fn action_menu_text(action: Action) -> String {
    if action.is_dangerous() {
        format!(
            "{:<10} !! {} [danger]",
            action.label(),
            action.description()
        )
    } else {
        format!("{:<10}    {}", action.label(), action.description())
    }
}

fn action_menu_item(action: Action) -> ListItem<'static> {
    let text = action_menu_text(action);
    let style = if action.is_dangerous() {
        Style::default().fg(Color::LightRed)
    } else {
        Style::default().fg(Color::Gray)
    };
    ListItem::new(Line::styled(text, style))
}

fn colorized_diff_lines(diff: &str) -> Vec<Line<'static>> {
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
        if raw.starts_with("diff --git ") {
            in_hunk = false;
            out.push(Line::from(vec![
                Span::styled(
                    " FILE ",
                    Style::default()
                        .bg(Color::Blue)
                        .fg(Color::Black)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw(" "),
                Span::styled(raw.to_string(), Style::default().fg(Color::Cyan)),
            ]));
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
            out.push(Line::from(vec![
                Span::styled(
                    " OLD ",
                    Style::default()
                        .bg(Color::Red)
                        .fg(Color::Black)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw(" "),
                Span::styled(raw.to_string(), Style::default().fg(Color::Red)),
            ]));
            continue;
        }

        if raw.starts_with("+++ ") {
            out.push(Line::from(vec![
                Span::styled(
                    " NEW ",
                    Style::default()
                        .bg(Color::Green)
                        .fg(Color::Black)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw(" "),
                Span::styled(raw.to_string(), Style::default().fg(Color::Green)),
            ]));
            continue;
        }

        if raw.starts_with("@@") {
            if let Some((old_start, new_start)) = parse_hunk_header(raw) {
                old_line = old_start;
                new_line = new_start;
                in_hunk = true;
            }
            out.push(Line::from(vec![
                Span::styled(
                    " HUNK ",
                    Style::default()
                        .bg(Color::Yellow)
                        .fg(Color::Black)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw(" "),
                Span::styled(raw.to_string(), Style::default().fg(Color::Yellow)),
            ]));
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

        if in_hunk {
            if let Some(body) = raw.strip_prefix('+') {
                out.push(render_diff_code_line(
                    None,
                    Some(new_line),
                    '+',
                    body,
                    Style::default().fg(Color::Green).bg(Color::Rgb(12, 32, 12)),
                ));
                new_line += 1;
                continue;
            }

            if let Some(body) = raw.strip_prefix('-') {
                out.push(render_diff_code_line(
                    Some(old_line),
                    None,
                    '-',
                    body,
                    Style::default().fg(Color::Red).bg(Color::Rgb(40, 14, 14)),
                ));
                old_line += 1;
                continue;
            }

            if let Some(body) = raw.strip_prefix(' ') {
                out.push(render_diff_code_line(
                    Some(old_line),
                    Some(new_line),
                    ' ',
                    body,
                    Style::default().fg(Color::Gray),
                ));
                old_line += 1;
                new_line += 1;
                continue;
            }
        }

        out.push(Line::from(raw.to_string()));
    }

    out
}

fn parse_hunk_header(header: &str) -> Option<(usize, usize)> {
    let mut parts = header.split_whitespace();
    let at1 = parts.next()?;
    let old = parts.next()?;
    let new = parts.next()?;
    let at2 = parts.next()?;

    if at1 != "@@" || at2 != "@@" {
        return None;
    }

    let old_start = old
        .strip_prefix('-')?
        .split(',')
        .next()?
        .parse::<usize>()
        .ok()?;
    let new_start = new
        .strip_prefix('+')?
        .split(',')
        .next()?
        .parse::<usize>()
        .ok()?;

    Some((old_start, new_start))
}

fn render_diff_code_line(
    old: Option<usize>,
    new: Option<usize>,
    marker: char,
    body: &str,
    body_style: Style,
) -> Line<'static> {
    let old_num = old.map_or_else(|| String::from(""), |n| n.to_string());
    let new_num = new.map_or_else(|| String::from(""), |n| n.to_string());

    let marker_style = match marker {
        '+' => Style::default()
            .fg(Color::Green)
            .add_modifier(Modifier::BOLD),
        '-' => Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
        _ => Style::default().fg(Color::DarkGray),
    };

    Line::from(vec![
        Span::styled(
            format!("{:>5}", old_num),
            Style::default().fg(Color::DarkGray),
        ),
        Span::raw(" "),
        Span::styled(
            format!("{:>5}", new_num),
            Style::default().fg(Color::DarkGray),
        ),
        Span::raw(" | "),
        Span::styled(format!("{marker} "), marker_style),
        Span::styled(body.to_string(), body_style),
    ])
}

fn colorized_preview_lines(path: Option<&Path>, content: &str) -> Vec<Line<'static>> {
    let language = detect_preview_language(path);
    content
        .lines()
        .map(|line| colorized_preview_line(line, language))
        .collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PreviewLanguage {
    Rust,
    Shell,
    Lua,
    Python,
    JsTs,
    Json,
    Toml,
    Yaml,
    Plain,
}

fn detect_preview_language(path: Option<&Path>) -> PreviewLanguage {
    let ext = path
        .and_then(|p| p.extension().and_then(|e| e.to_str()))
        .map(|e| e.to_ascii_lowercase());
    match ext.as_deref() {
        Some("rs") => PreviewLanguage::Rust,
        Some("sh") | Some("bash") | Some("zsh") | Some("fish") => PreviewLanguage::Shell,
        Some("lua") => PreviewLanguage::Lua,
        Some("py") => PreviewLanguage::Python,
        Some("js") | Some("mjs") | Some("cjs") | Some("ts") | Some("tsx") | Some("jsx") => {
            PreviewLanguage::JsTs
        }
        Some("json") => PreviewLanguage::Json,
        Some("toml") => PreviewLanguage::Toml,
        Some("yaml") | Some("yml") => PreviewLanguage::Yaml,
        _ => {
            let name = path
                .and_then(|p| p.file_name().and_then(|n| n.to_str()))
                .unwrap_or_default()
                .to_ascii_lowercase();
            match name.as_str() {
                ".zshrc" | ".bashrc" | ".bash_profile" => PreviewLanguage::Shell,
                "justfile" | "makefile" => PreviewLanguage::Plain,
                _ => PreviewLanguage::Plain,
            }
        }
    }
}

fn colorized_preview_line(line: &str, language: PreviewLanguage) -> Line<'static> {
    let (code, comment) = split_comment(line, language);
    let mut spans = colorize_code_tokens(code, language);

    if let Some(comment) = comment {
        spans.push(Span::styled(
            comment.to_string(),
            Style::default().fg(Color::DarkGray),
        ));
    }

    if spans.is_empty() {
        Line::from(String::new())
    } else {
        Line::from(spans)
    }
}

fn split_comment(line: &str, language: PreviewLanguage) -> (&str, Option<&str>) {
    let marker = match language {
        PreviewLanguage::Rust | PreviewLanguage::JsTs => Some("//"),
        PreviewLanguage::Shell
        | PreviewLanguage::Python
        | PreviewLanguage::Toml
        | PreviewLanguage::Yaml => Some("#"),
        PreviewLanguage::Lua => Some("--"),
        PreviewLanguage::Json | PreviewLanguage::Plain => None,
    };

    if let Some(marker) = marker
        && let Some(idx) = line.find(marker)
    {
        return (&line[..idx], Some(&line[idx..]));
    }

    (line, None)
}

fn colorize_code_tokens(code: &str, language: PreviewLanguage) -> Vec<Span<'static>> {
    let chars: Vec<char> = code.chars().collect();
    let mut spans = Vec::new();
    let mut i = 0usize;

    while i < chars.len() {
        let ch = chars[i];

        if ch == '"' || ch == '\'' {
            let quote = ch;
            let start = i;
            i += 1;
            while i < chars.len() {
                if chars[i] == quote && chars[i.saturating_sub(1)] != '\\' {
                    i += 1;
                    break;
                }
                i += 1;
            }
            let token: String = chars[start..i].iter().collect();
            let key_style = if is_object_key(&chars, i, language) {
                Style::default().fg(Color::LightCyan)
            } else {
                Style::default().fg(Color::Yellow)
            };
            spans.push(Span::styled(token, key_style));
            continue;
        }

        if ch.is_ascii_digit() {
            let start = i;
            i += 1;
            while i < chars.len()
                && (chars[i].is_ascii_hexdigit()
                    || chars[i] == '_'
                    || chars[i] == '.'
                    || chars[i] == 'x'
                    || chars[i] == 'X')
            {
                i += 1;
            }
            let token: String = chars[start..i].iter().collect();
            spans.push(Span::styled(token, Style::default().fg(Color::Magenta)));
            continue;
        }

        if is_word_start(ch) {
            let start = i;
            i += 1;
            while i < chars.len() && is_word(chars[i]) {
                i += 1;
            }
            let token: String = chars[start..i].iter().collect();
            if preview_keywords(language).contains(&token.as_str()) {
                spans.push(Span::styled(
                    token,
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                ));
            } else {
                spans.push(Span::from(token));
            }
            continue;
        }

        let start = i;
        i += 1;
        while i < chars.len() && !is_word_start(chars[i]) && !chars[i].is_ascii_digit() {
            if chars[i] == '"' || chars[i] == '\'' {
                break;
            }
            i += 1;
        }
        let token: String = chars[start..i].iter().collect();
        spans.push(Span::styled(token, Style::default().fg(Color::Gray)));
    }

    spans
}

fn is_word_start(ch: char) -> bool {
    ch.is_ascii_alphabetic() || ch == '_'
}

fn is_word(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || ch == '_'
}

fn is_object_key(chars: &[char], from: usize, language: PreviewLanguage) -> bool {
    if !matches!(
        language,
        PreviewLanguage::Json | PreviewLanguage::Toml | PreviewLanguage::Yaml
    ) {
        return false;
    }

    let mut i = from;
    while i < chars.len() && chars[i].is_whitespace() {
        i += 1;
    }
    i < chars.len() && chars[i] == ':'
}

fn preview_keywords(language: PreviewLanguage) -> &'static [&'static str] {
    match language {
        PreviewLanguage::Rust => &[
            "fn", "let", "mut", "pub", "struct", "enum", "impl", "use", "mod", "match", "if",
            "else", "for", "while", "loop", "return", "async", "await", "trait", "where", "self",
            "Self",
        ],
        PreviewLanguage::Shell => &[
            "if", "then", "else", "fi", "for", "in", "do", "done", "case", "esac", "function",
            "export", "local",
        ],
        PreviewLanguage::Lua => &[
            "local", "function", "if", "then", "else", "elseif", "end", "for", "in", "do", "while",
            "repeat", "until", "return",
        ],
        PreviewLanguage::Python => &[
            "def", "class", "if", "elif", "else", "for", "while", "try", "except", "finally",
            "return", "import", "from", "as", "with", "lambda",
        ],
        PreviewLanguage::JsTs => &[
            "function",
            "const",
            "let",
            "var",
            "if",
            "else",
            "for",
            "while",
            "return",
            "import",
            "from",
            "export",
            "class",
            "extends",
            "async",
            "await",
            "type",
            "interface",
        ],
        PreviewLanguage::Json => &["true", "false", "null"],
        PreviewLanguage::Toml => &["true", "false"],
        PreviewLanguage::Yaml => &["true", "false", "null"],
        PreviewLanguage::Plain => &[],
    }
}

fn centered_rect(percent_x: u16, percent_y: u16, area: Rect) -> Rect {
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(area);

    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(vertical[1])[1]
}

#[cfg(test)]
mod tests {
    use super::{action_menu_text, log_scroll_offset, parse_hunk_header};
    use crate::domain::Action;

    #[test]
    fn parse_hunk_header_extracts_line_numbers() {
        let parsed = parse_hunk_header("@@ -12,7 +30,9 @@ fn main()");
        assert_eq!(parsed, Some((12, 30)));
    }

    #[test]
    fn parse_hunk_header_rejects_invalid_header() {
        let parsed = parse_hunk_header("@ -12 +30 @");
        assert_eq!(parsed, None);
    }

    #[test]
    fn log_scroll_offset_keeps_latest_visible() {
        assert_eq!(log_scroll_offset(10, 6), 6);
        assert_eq!(log_scroll_offset(3, 10), 0);
    }

    #[test]
    fn action_menu_text_marks_only_danger_actions() {
        let safe = action_menu_text(Action::Apply);
        let danger = action_menu_text(Action::Purge);

        assert!(!safe.contains("!!"));
        assert!(!safe.contains("[danger]"));
        assert!(danger.contains("!!"));
        assert!(danger.contains("[danger]"));
    }
}
