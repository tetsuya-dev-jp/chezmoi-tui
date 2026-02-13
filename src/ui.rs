use crate::app::{App, ConfirmStep, DetailKind, InputKind, ModalState, PaneFocus};
use crate::domain::Action;
use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::prelude::{Alignment, Color, Line, Modifier, Span, Style};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap};

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
        app.detail_text.lines().map(Line::from).collect()
    };

    let paragraph = Paragraph::new(lines)
        .block(
            Block::default()
                .title(format!(" {} ", app.detail_title))
                .borders(Borders::ALL)
                .border_style(border_style),
        )
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
            "j/k move h/l collapse/expand tab focus d diff v preview a action e edit r refresh q quit",
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

fn colorized_diff_lines(diff: &str) -> Vec<Line<'_>> {
    diff.lines()
        .map(|line| {
            if line.starts_with("+++") || line.starts_with("---") {
                Line::from(Span::styled(line, Style::default().fg(Color::Cyan)))
            } else if line.starts_with("@@") {
                Line::from(Span::styled(
                    line,
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD),
                ))
            } else if line.starts_with('+') {
                Line::from(Span::styled(line, Style::default().fg(Color::Green)))
            } else if line.starts_with('-') {
                Line::from(Span::styled(line, Style::default().fg(Color::Red)))
            } else {
                Line::raw(line)
            }
        })
        .collect()
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
