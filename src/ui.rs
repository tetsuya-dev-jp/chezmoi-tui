use crate::app::{App, ConfirmStep, DetailKind, InputKind, ModalState, PaneFocus};
use crate::domain::Action;
use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::prelude::{Alignment, Color, Line, Modifier, Span, Style};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap};
use std::path::Path;

pub fn draw(frame: &mut Frame, app: &App) {
    let outer = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(1)])
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

fn draw_list(frame: &mut Frame, app: &App, area: Rect) {
    let items: Vec<ListItem> = app.current_items().into_iter().map(ListItem::new).collect();

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

    let mut state = ListState::default();
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
        vec![
            Line::from("詳細が未ロードです。"),
            Line::from("Enter / d: diff, v: ファイル本文プレビュー"),
        ]
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
        .rev()
        .take(200)
        .rev()
        .map(|line| Line::from(line.as_str()))
        .collect();

    let paragraph = Paragraph::new(lines)
        .block(
            Block::default()
                .title(" Log ")
                .borders(Borders::ALL)
                .border_style(border_style),
        )
        .wrap(Wrap { trim: false });

    frame.render_widget(paragraph, area);
}

fn draw_status_bar(frame: &mut Frame, app: &App, area: Rect) {
    let busy = if app.busy { "BUSY" } else { "IDLE" };
    let text = Line::from(vec![
        Span::styled(
            format!(" {} ", busy),
            if app.busy {
                Style::default().bg(Color::Yellow).fg(Color::Black)
            } else {
                Style::default().bg(Color::DarkGray).fg(Color::White)
            },
        ),
        Span::raw("  "),
        Span::styled(
            "1:status 2:managed 3:unmanaged",
            Style::default().fg(Color::Gray),
        ),
        Span::raw("  "),
        Span::styled(
            "tab focus | List:j/k move | Detail:j/k PgUp/PgDn Ctrl+u/d scroll | d diff v preview",
            Style::default().fg(Color::Gray),
        ),
    ]);

    let paragraph = Paragraph::new(text).alignment(Alignment::Left);
    frame.render_widget(paragraph, area);
}

fn draw_modal(frame: &mut Frame, app: &App) {
    match &app.modal {
        ModalState::None => {}
        ModalState::ActionMenu { selected } => {
            let area = centered_rect(60, 70, frame.area());
            frame.render_widget(Clear, area);

            let items: Vec<ListItem> = Action::ALL
                .iter()
                .map(|action| {
                    ListItem::new(format!("{:<10} {}", action.label(), action.description()))
                })
                .collect();

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
            state.select(Some(*selected));
            frame.render_stateful_widget(list, area, &mut state);
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
                    lines.push(Line::from("Enter: 実行  Esc: キャンセル"));
                    if request.action.is_dangerous() {
                        lines.push(Line::from(
                            "危険操作のため、次のステップで確認文字列が必要です。",
                        ));
                    }
                }
                ConfirmStep::DangerPhrase => {
                    lines.push(Line::from("確認文字列を入力して Enter で実行、Escで中止"));
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
                InputKind::ChattrAttrs => "chattr attributes (例: private,template)",
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
                Line::from("Enter: 確定  Esc: キャンセル"),
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

fn colorized_diff_lines(diff: &str) -> Vec<Line<'static>> {
    diff.lines()
        .map(|line| {
            if line.starts_with("+++") || line.starts_with("---") {
                Line::from(Span::styled(
                    line.to_string(),
                    Style::default().fg(Color::Cyan),
                ))
            } else if line.starts_with("@@") {
                Line::from(Span::styled(
                    line.to_string(),
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD),
                ))
            } else if line.starts_with('+') {
                Line::from(Span::styled(
                    line.to_string(),
                    Style::default().fg(Color::Green),
                ))
            } else if line.starts_with('-') {
                Line::from(Span::styled(
                    line.to_string(),
                    Style::default().fg(Color::Red),
                ))
            } else {
                Line::from(line.to_string())
            }
        })
        .collect()
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
