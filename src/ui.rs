use crate::app::{
    App, ConfirmStep, DetailKind, InputKind, LayoutMode, ModalState, NoticeTone, PaneFocus,
    SearchScope,
};
use crate::domain::{Action, ActionRequest, ListView};
use crate::infra::action_to_args;
use crate::ui_diff::colorized_diff_lines;
use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::prelude::{Alignment, Color, Line, Modifier, Span, Style};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap};
use std::collections::HashSet;
use std::path::Path;

pub fn draw(frame: &mut Frame, app: &mut App) {
    let footer_height = if app.footer_help { 2 } else { 1 };
    let outer = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(footer_height)])
        .split(frame.area());

    match app.layout_mode {
        LayoutMode::Normal => {
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
        }
        LayoutMode::DetailMax => draw_detail(frame, app, outer[0]),
        LayoutMode::LogMax => draw_logs(frame, app, outer[0]),
    }
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

    let title = if app.list_filter().trim().is_empty() {
        format!(" {} ", app.view.title())
    } else {
        let summary = app.current_filter_summary().unwrap_or_default();
        format!(
            " {} /{} ({}) ",
            app.view.title(),
            app.list_filter(),
            summary
        )
    };

    let list = List::new(items)
        .block(
            Block::default()
                .title(title)
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
        if matches!(app.view, ListView::Unmanaged | ListView::Source) && app.selected_is_directory()
        {
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
        .scroll((
            clamp_to_u16(app.detail_scroll),
            clamp_to_u16(app.detail_hscroll),
        ))
        .wrap(Wrap { trim: false });

    frame.render_widget(paragraph, area);
}

fn draw_logs(frame: &mut Frame, app: &App, area: Rect) {
    let border_style = if app.focus == PaneFocus::Log {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default()
    };

    let lines: Vec<Line> = app.logs.iter().map(|line| styled_log_line(line)).collect();
    let scroll = log_scroll_offset(lines.len(), area.height, app.log_tail_offset);

    let paragraph = Paragraph::new(lines)
        .block(
            Block::default()
                .title(" Log ")
                .borders(Borders::ALL)
                .border_style(border_style),
        )
        .scroll((scroll, clamp_to_u16(app.log_hscroll)))
        .wrap(Wrap { trim: false });

    frame.render_widget(paragraph, area);
}

fn styled_log_line(line: &str) -> Line<'static> {
    let style = if line.starts_with("hint:") {
        Style::default().fg(Color::LightBlue)
    } else if line.contains("error") || line.starts_with("stderr:") {
        Style::default().fg(Color::LightRed)
    } else if line.contains("completed") || line.contains("exit=0") {
        Style::default().fg(Color::LightGreen)
    } else if line.starts_with("action") || line.starts_with("foreground") {
        Style::default().fg(Color::LightYellow)
    } else {
        Style::default().fg(Color::Gray)
    };
    Line::styled(line.to_string(), style)
}

fn log_scroll_offset(total_lines: usize, area_height: u16, tail_offset: usize) -> u16 {
    let visible_rows = area_height.saturating_sub(2) as usize;
    let max_offset = total_lines.saturating_sub(visible_rows.max(1));
    clamp_to_u16(max_offset.saturating_sub(tail_offset.min(max_offset)))
}

fn clamp_to_u16(value: usize) -> u16 {
    u16::try_from(value).unwrap_or(u16::MAX)
}

fn draw_status_bar(frame: &mut Frame, app: &App, area: Rect) {
    FooterBar::draw(frame, app, area);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HintTone {
    Primary,
    Secondary,
    Muted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Hint {
    key: &'static str,
    label: &'static str,
    group: Option<&'static str>,
    priority: u8,
    tone: HintTone,
    enabled: bool,
    mandatory: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LabelMode {
    Full,
    Truncated(usize),
    KeyOnly,
}

#[derive(Debug, Clone)]
struct LeftSegment {
    text: String,
    style: Style,
    essential: bool,
    badge: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct HintRendered {
    key: &'static str,
    label: String,
    tone: HintTone,
    mandatory: bool,
}

#[derive(Debug, Clone)]
struct CheatItem {
    key: &'static str,
    label: &'static str,
}

#[derive(Debug, Clone)]
struct CheatGroup {
    title: &'static str,
    items: Vec<CheatItem>,
    priority: u8,
}

struct FooterBar;

const MIN_RIGHT_HINT_WIDTH: usize = 34;
const TARGET_HINT_COUNT: usize = 7;
const TRUNCATED_HINT_LABEL_WIDTH: usize = 6;

impl FooterBar {
    fn draw(frame: &mut Frame, app: &App, area: Rect) {
        if area.height == 0 {
            return;
        }

        let rows = if app.footer_help && area.height >= 2 {
            Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Length(1), Constraint::Length(1)])
                .split(area)
        } else {
            Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Length(1)])
                .split(area)
        };

        Self::draw_main_row(frame, app, rows[0]);
        if rows.len() > 1 {
            Self::draw_cheat_row(frame, app, rows[1]);
        }
    }

    fn draw_main_row(frame: &mut Frame, app: &App, area: Rect) {
        let total_width = area.width as usize;
        if total_width == 0 {
            return;
        }

        let min_right = MIN_RIGHT_HINT_WIDTH.min(total_width.saturating_sub(1));
        let left_max = total_width.saturating_sub(min_right + 1);
        let (left_spans, left_width) = footer_left(app, left_max);

        let right_budget = total_width
            .saturating_sub(left_width)
            .saturating_sub(usize::from(left_width > 0));
        // Help ON でも 1行目は通常フッターの最重要ヒントだけを表示する。
        // 追加説明キーは 2行目の Help シートにのみ集約する。
        let rendered = layout_hints(right_budget, footer_hints(app));
        let (right_spans, right_width) = render_hints(&rendered);

        let gap = total_width.saturating_sub(left_width + right_width);

        let mut line = Vec::new();
        line.extend(left_spans);
        if gap > 0 {
            line.push(Span::raw(" ".repeat(gap)));
        }
        line.extend(right_spans);
        let line = clip_spans_to_width(line, total_width);

        let paragraph = Paragraph::new(Line::from(line))
            .alignment(Alignment::Left)
            .style(Style::default().bg(Color::Rgb(14, 16, 20)));
        frame.render_widget(paragraph, area);
    }

    fn draw_cheat_row(frame: &mut Frame, app: &App, area: Rect) {
        let max_width = area.width as usize;
        if max_width == 0 {
            return;
        }

        let mut spans = vec![
            Span::styled(
                "Help:",
                Style::default()
                    .fg(Color::LightBlue)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" "),
        ];
        spans.extend(render_help_groups(
            app,
            max_width.saturating_sub(text_width("Help: ")),
        ));
        let spans = clip_spans_to_width(spans, max_width);
        let line = Line::from(spans);
        let paragraph = Paragraph::new(line)
            .alignment(Alignment::Left)
            .style(Style::default().bg(Color::Rgb(14, 16, 20)));
        frame.render_widget(paragraph, area);
    }
}

fn footer_left(app: &App, max_width: usize) -> (Vec<Span<'static>>, usize) {
    if max_width == 0 {
        return (Vec::new(), 0);
    }

    let item_count = app.current_len();
    let selected_ordinal = if item_count == 0 {
        0
    } else {
        app.selected_index.min(item_count - 1) + 1
    };
    let marked_count = app.marked_count();
    let mut segments = vec![LeftSegment {
        text: app.view.title().to_string(),
        style: Style::default()
            .bg(Color::Rgb(35, 118, 210))
            .fg(Color::Black)
            .add_modifier(Modifier::BOLD),
        essential: true,
        badge: true,
    }];

    if app.config.show_base_context {
        segments.push(LeftSegment {
            text: compact_label(&app.view_context_text(), 42),
            style: Style::default().fg(Color::DarkGray),
            essential: false,
            badge: false,
        });
    }

    if app.is_busy() {
        let text = app
            .busy_message()
            .map_or_else(|| "Busy".to_string(), |message| format!("Busy: {message}"));
        segments.push(LeftSegment {
            text: compact_label(&text, 48),
            style: Style::default().fg(Color::LightYellow),
            essential: false,
            badge: false,
        });
    }

    if app.config.show_notices
        && let Some(notice) = app.latest_notice()
    {
        segments.push(LeftSegment {
            text: compact_label(&notice.message, 48),
            style: notice_style(notice.tone),
            essential: false,
            badge: false,
        });
    }

    segments.push(LeftSegment {
        text: format!("focus={}", focus_label(app.focus)),
        style: Style::default().fg(Color::Gray),
        essential: false,
        badge: false,
    });

    if app.view == ListView::Status {
        segments.push(LeftSegment {
            text: "status cols: state/target".to_string(),
            style: Style::default().fg(Color::DarkGray),
            essential: false,
            badge: false,
        });
    }

    if let Some(search) = app.active_search_label() {
        segments.push(LeftSegment {
            text: compact_label(&search, 24),
            style: Style::default().fg(Color::LightYellow),
            essential: false,
            badge: false,
        });
    }

    if app.layout_mode != LayoutMode::Normal {
        segments.push(LeftSegment {
            text: format!("layout={:?}", app.layout_mode),
            style: Style::default().fg(Color::LightMagenta),
            essential: false,
            badge: false,
        });
    }

    segments.extend([
        LeftSegment {
            text: format!(
                "{}/{} {}",
                selected_ordinal,
                item_count,
                item_word(item_count)
            ),
            style: Style::default().fg(Color::Gray),
            essential: true,
            badge: false,
        },
        LeftSegment {
            text: format!("{marked_count} marked"),
            style: Style::default().fg(Color::Gray),
            essential: true,
            badge: false,
        },
    ]);

    if !app.list_filter().trim().is_empty() {
        segments.push(LeftSegment {
            text: format!("/{}", compact_label(app.list_filter(), 18)),
            style: Style::default().fg(Color::LightYellow),
            essential: false,
            badge: false,
        });
    }

    fit_left_segments(&mut segments, max_width);

    let mut spans = Vec::new();
    let mut width = 0usize;
    for (idx, seg) in segments.iter().enumerate() {
        if idx > 0 {
            let sep = " • ";
            spans.push(Span::styled(sep, Style::default().fg(Color::DarkGray)));
            width += text_width(sep);
        }

        if seg.badge {
            spans.push(Span::styled(format!(" {} ", seg.text), seg.style));
            width += text_width(&seg.text) + 2;
        } else {
            spans.push(Span::styled(seg.text.clone(), seg.style));
            width += text_width(&seg.text);
        }
    }

    (spans, width)
}

fn fit_left_segments(segments: &mut Vec<LeftSegment>, max_width: usize) {
    while left_segments_width(segments) > max_width {
        if let Some(index) = (0..segments.len())
            .rev()
            .find(|&index| !segments[index].essential)
        {
            segments.remove(index);
        } else {
            break;
        }
    }

    while left_segments_width(segments) > max_width && segments.len() > 2 {
        segments.pop();
    }

    if left_segments_width(segments) > max_width && segments.len() > 1 {
        let last = segments.len() - 1;
        let current = left_segments_width(segments);
        let over = current.saturating_sub(max_width);
        let keep = text_width(&segments[last].text).saturating_sub(over + 1);
        segments[last].text = compact_label(&segments[last].text, keep.max(1));
    }
}

fn left_segments_width(segments: &[LeftSegment]) -> usize {
    if segments.is_empty() {
        return 0;
    }

    segments
        .iter()
        .enumerate()
        .map(|(idx, seg)| {
            let text_len = if seg.badge {
                text_width(&seg.text) + 2
            } else {
                text_width(&seg.text)
            };
            let sep_len = if idx == 0 { 0 } else { text_width(" • ") };
            text_len + sep_len
        })
        .sum()
}

fn footer_hints(app: &App) -> Vec<Hint> {
    let mut hints = match app.focus {
        PaneFocus::List => list_focus_hints(app),
        PaneFocus::Detail | PaneFocus::Log => detail_focus_hints(),
    };

    if app.footer_help {
        hints.extend(help_only_global_hints());
    }
    hints.extend(primary_global_hints());

    hints
}

fn list_focus_hints(app: &App) -> Vec<Hint> {
    vec![
        hint(
            "/",
            "Find",
            Some("list"),
            100,
            HintTone::Secondary,
            true,
            false,
        ),
        hint(
            "Space",
            "Mark",
            Some("list"),
            95,
            HintTone::Secondary,
            true,
            false,
        ),
        hint(
            "j/k",
            "Move",
            Some("list"),
            90,
            HintTone::Secondary,
            true,
            false,
        ),
        hint(
            "d",
            "Diff",
            Some("detail"),
            88,
            HintTone::Secondary,
            app.view == ListView::Status,
            false,
        ),
        hint(
            "v",
            "View",
            Some("detail"),
            88,
            HintTone::Secondary,
            app.view != ListView::Status && !app.selected_is_directory(),
            false,
        ),
        hint(
            "c",
            "Clear",
            Some("list"),
            70,
            HintTone::Muted,
            app.marked_count() > 0,
            false,
        ),
        hint(
            "h/l",
            "Fold",
            Some("tree"),
            62,
            HintTone::Muted,
            app.footer_help
                && matches!(
                    app.view,
                    ListView::Managed | ListView::Unmanaged | ListView::Source
                ),
            false,
        ),
    ]
}

fn detail_focus_hints() -> Vec<Hint> {
    vec![
        hint(
            "j/k",
            "Scroll",
            Some("scroll"),
            100,
            HintTone::Secondary,
            true,
            false,
        ),
        hint(
            "PgUp/PgDn",
            "Page",
            Some("scroll"),
            95,
            HintTone::Secondary,
            true,
            false,
        ),
        hint(
            "C-u/d",
            "Jump",
            Some("scroll"),
            90,
            HintTone::Secondary,
            true,
            false,
        ),
    ]
}

fn help_only_global_hints() -> [Hint; 3] {
    [
        hint(
            "Tab",
            "Pane",
            Some("global"),
            60,
            HintTone::Muted,
            true,
            false,
        ),
        hint(
            "1-4",
            "Switch",
            Some("global"),
            58,
            HintTone::Muted,
            true,
            false,
        ),
        hint(
            "r",
            "Refresh",
            Some("global"),
            55,
            HintTone::Muted,
            true,
            false,
        ),
    ]
}

fn primary_global_hints() -> [Hint; 3] {
    [
        hint(
            "a",
            "Actions",
            Some("global"),
            89,
            HintTone::Primary,
            true,
            true,
        ),
        hint(
            "?",
            "Help",
            Some("global"),
            88,
            HintTone::Primary,
            true,
            true,
        ),
        hint(
            "q",
            "Quit",
            Some("global"),
            87,
            HintTone::Primary,
            true,
            true,
        ),
    ]
}

fn hint(
    key: &'static str,
    label: &'static str,
    group: Option<&'static str>,
    priority: u8,
    tone: HintTone,
    enabled: bool,
    mandatory: bool,
) -> Hint {
    Hint {
        key,
        label,
        group,
        priority,
        tone,
        enabled,
        mandatory,
    }
}

fn layout_hints(max_width: usize, hints: Vec<Hint>) -> Vec<HintRendered> {
    if max_width == 0 {
        return Vec::new();
    }

    let mut active: Vec<Hint> = hints
        .into_iter()
        .filter(|hint| hint.enabled && hint.tone != HintTone::Muted)
        .collect();
    active.sort_by(|a, b| {
        b.priority
            .cmp(&a.priority)
            .then_with(|| a.group.cmp(&b.group))
            .then_with(|| a.label.cmp(b.label))
    });

    let mut rendered = render_hints_for_mode(&active, LabelMode::Full);
    trim_optional_hints(&mut rendered, max_width, Some(TARGET_HINT_COUNT));

    if hints_width(&rendered) > max_width {
        rendered = rerender_selected_hints(
            &active,
            &rendered,
            LabelMode::Truncated(TRUNCATED_HINT_LABEL_WIDTH),
        );
    }

    trim_optional_hints(&mut rendered, max_width, None);

    if hints_width(&rendered) > max_width {
        rendered = rerender_selected_hints(&active, &rendered, LabelMode::KeyOnly);
    }

    trim_optional_hints(&mut rendered, max_width, None);

    rendered
}

fn trim_optional_hints(
    rendered: &mut Vec<HintRendered>,
    max_width: usize,
    max_count: Option<usize>,
) {
    loop {
        let over_count = max_count.is_some_and(|limit| rendered.len() > limit);
        let over_width = hints_width(rendered) > max_width;
        if !over_count && !over_width {
            break;
        }

        if let Some(index) = rendered.iter().rposition(|hint| !hint.mandatory) {
            rendered.remove(index);
        } else {
            break;
        }
    }
}

fn rerender_selected_hints(
    active: &[Hint],
    rendered: &[HintRendered],
    mode: LabelMode,
) -> Vec<HintRendered> {
    let selected_keys: HashSet<&'static str> = rendered.iter().map(|hint| hint.key).collect();
    let selected: Vec<Hint> = active
        .iter()
        .filter(|candidate| selected_keys.contains(candidate.key))
        .copied()
        .collect();
    render_hints_for_mode(&selected, mode)
}

fn render_hints_for_mode(hints: &[Hint], mode: LabelMode) -> Vec<HintRendered> {
    hints
        .iter()
        .map(|hint| HintRendered {
            key: hint.key,
            label: render_hint_label(hint.label, mode),
            tone: hint.tone,
            mandatory: hint.mandatory,
        })
        .collect()
}

fn render_hint_label(label: &str, mode: LabelMode) -> String {
    match mode {
        LabelMode::Full => label.to_string(),
        LabelMode::Truncated(max) => compact_label(label, max),
        LabelMode::KeyOnly => String::new(),
    }
}

fn hints_width(hints: &[HintRendered]) -> usize {
    if hints.is_empty() {
        return 0;
    }

    hints
        .iter()
        .enumerate()
        .map(|(index, hint)| {
            let mut width = keycap_width(hint.key);
            if !hint.label.is_empty() {
                width += 1 + text_width(&hint.label);
            }
            if index > 0 {
                width += 2;
            }
            width
        })
        .sum()
}

fn render_hints(hints: &[HintRendered]) -> (Vec<Span<'static>>, usize) {
    let mut spans = Vec::new();
    let mut width = 0usize;

    for (index, hint) in hints.iter().enumerate() {
        if index > 0 {
            spans.push(Span::raw("  "));
            width += 2;
        }

        let keycap_style = keycap_style(hint.tone);
        let label_style = hint_label_style(hint.tone);
        spans.push(Span::styled(format!(" {} ", hint.key), keycap_style));
        width += keycap_width(hint.key);

        if !hint.label.is_empty() {
            spans.push(Span::raw(" "));
            spans.push(Span::styled(hint.label.clone(), label_style));
            width += 1 + text_width(&hint.label);
        }
    }

    (spans, width)
}

fn notice_tone_label(tone: NoticeTone) -> &'static str {
    match tone {
        NoticeTone::Info => "INFO",
        NoticeTone::Success => "OK  ",
        NoticeTone::Error => "ERR ",
    }
}

fn notice_style(tone: NoticeTone) -> Style {
    match tone {
        NoticeTone::Info => Style::default().fg(Color::LightBlue),
        NoticeTone::Success => Style::default().fg(Color::LightGreen),
        NoticeTone::Error => Style::default()
            .fg(Color::LightRed)
            .add_modifier(Modifier::BOLD),
    }
}

fn keycap_style(tone: HintTone) -> Style {
    match tone {
        HintTone::Primary => Style::default()
            .bg(Color::Rgb(70, 160, 250))
            .fg(Color::Black)
            .add_modifier(Modifier::BOLD),
        HintTone::Secondary => Style::default().bg(Color::Rgb(34, 38, 46)).fg(Color::White),
        HintTone::Muted => Style::default().fg(Color::DarkGray),
    }
}

fn hint_label_style(tone: HintTone) -> Style {
    match tone {
        HintTone::Primary => Style::default()
            .fg(Color::LightCyan)
            .add_modifier(Modifier::BOLD),
        HintTone::Secondary => Style::default().fg(Color::Gray),
        HintTone::Muted => Style::default().fg(Color::DarkGray),
    }
}

fn cheat_groups(app: &App) -> Vec<CheatGroup> {
    let mut nav_items = Vec::new();
    let mut view_items = Vec::new();
    let mut global_items = vec![CheatItem {
        key: "a",
        label: "Actions",
    }];
    if !app.is_busy() {
        global_items.push(CheatItem {
            key: "r",
            label: "Refresh",
        });
    }
    global_items.extend([
        CheatItem {
            key: "?",
            label: "Help",
        },
        CheatItem {
            key: "q",
            label: "Quit",
        },
    ]);

    match app.focus {
        PaneFocus::List => {
            nav_items.extend([
                CheatItem {
                    key: "j/k",
                    label: "Move",
                },
                CheatItem {
                    key: "/",
                    label: "Find",
                },
                CheatItem {
                    key: "Space",
                    label: "Mark",
                },
            ]);
            if app.view == ListView::Status {
                nav_items.push(CheatItem {
                    key: "d",
                    label: "Diff",
                });
            } else if !app.selected_is_directory() {
                nav_items.push(CheatItem {
                    key: "v",
                    label: "View",
                });
            }
            if app.marked_count() > 0 {
                nav_items.push(CheatItem {
                    key: "c",
                    label: "Clear",
                });
            }

            if matches!(
                app.view,
                ListView::Managed | ListView::Unmanaged | ListView::Source
            ) {
                view_items.push(CheatItem {
                    key: "h/l",
                    label: "Fold",
                });
            }
        }
        PaneFocus::Detail | PaneFocus::Log => {
            nav_items.extend([
                CheatItem {
                    key: "j/k",
                    label: "Scroll",
                },
                CheatItem {
                    key: "PgUp/PgDn",
                    label: "Page",
                },
                CheatItem {
                    key: "C-u/d",
                    label: "Jump",
                },
                CheatItem {
                    key: "/ n/N",
                    label: "Search",
                },
                CheatItem {
                    key: "H/L",
                    label: "Wide",
                },
            ]);
        }
    }

    view_items.extend([
        CheatItem {
            key: "Tab",
            label: "Pane",
        },
        CheatItem {
            key: "m",
            label: "Max",
        },
        CheatItem {
            key: "1-4",
            label: "Switch",
        },
    ]);

    vec![
        CheatGroup {
            title: "Nav",
            items: nav_items,
            priority: 2,
        },
        CheatGroup {
            title: "View",
            items: view_items,
            priority: 1,
        },
        CheatGroup {
            title: "Global",
            items: global_items,
            priority: 3,
        },
    ]
}

fn render_help_groups(app: &App, max_width: usize) -> Vec<Span<'static>> {
    if max_width == 0 {
        return Vec::new();
    }

    let groups = cheat_groups(app);
    let (selected, omitted) = fit_cheat_groups(&groups, max_width);
    let mut spans = Vec::new();

    for (group, keep) in groups.iter().zip(selected.iter()) {
        if !*keep {
            continue;
        }

        if !spans.is_empty() {
            spans.push(Span::raw("   "));
        }

        spans.push(Span::styled(
            format!("{}:", group.title),
            Style::default()
                .fg(Color::LightBlue)
                .add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::raw(" "));

        for (item_idx, item) in group.items.iter().enumerate() {
            if item_idx > 0 {
                spans.push(Span::raw("  "));
            }
            spans.push(Span::styled(
                item.key.to_string(),
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ));
            spans.push(Span::raw(" "));
            spans.push(Span::styled(
                item.label.to_string(),
                Style::default().fg(Color::Gray),
            ));
        }
    }

    if omitted {
        if !spans.is_empty() {
            spans.push(Span::raw("  "));
        }
        spans.push(Span::styled("…", Style::default().fg(Color::DarkGray)));
    }
    spans
}

fn fit_cheat_groups(groups: &[CheatGroup], max_width: usize) -> (Vec<bool>, bool) {
    if groups.is_empty() {
        return (Vec::new(), false);
    }

    let mut selected = vec![true; groups.len()];
    let mut omitted = false;

    while cheat_groups_width(groups, &selected, omitted) > max_width {
        let next_drop = (0..groups.len())
            .filter(|&idx| selected[idx] && groups[idx].priority < 3)
            .min_by_key(|&idx| groups[idx].priority);
        if let Some(idx) = next_drop {
            selected[idx] = false;
            omitted = true;
        } else {
            break;
        }
    }

    (selected, omitted)
}

fn cheat_groups_width(groups: &[CheatGroup], selected: &[bool], omitted: bool) -> usize {
    let mut width = 0usize;
    let mut kept = 0usize;

    for (group, keep) in groups.iter().zip(selected.iter()) {
        if !*keep {
            continue;
        }
        if kept > 0 {
            width += text_width("   ");
        }
        width += cheat_group_width(group);
        kept += 1;
    }

    if omitted {
        if kept > 0 {
            width += text_width("  ");
        }
        width += text_width("…");
    }

    width
}

fn cheat_group_width(group: &CheatGroup) -> usize {
    let mut width = text_width(group.title) + text_width(": ");
    for (idx, item) in group.items.iter().enumerate() {
        if idx > 0 {
            width += text_width("  ");
        }
        width += text_width(item.key) + text_width(" ") + text_width(item.label);
    }
    width
}

fn clip_spans_to_width(spans: Vec<Span<'static>>, max_width: usize) -> Vec<Span<'static>> {
    if max_width == 0 {
        return Vec::new();
    }

    let mut clipped = Vec::new();
    let mut used = 0usize;

    for span in spans {
        if used >= max_width {
            break;
        }

        let content = span.content.to_string();
        let width = text_width(&content);
        if used + width <= max_width {
            clipped.push(span);
            used += width;
            continue;
        }

        let remain = max_width.saturating_sub(used);
        if remain == 0 {
            break;
        }

        let truncated: String = content.chars().take(remain).collect();
        if !truncated.is_empty() {
            clipped.push(Span::styled(truncated, span.style));
        }
        break;
    }

    clipped
}

fn text_width(text: &str) -> usize {
    text.chars().count()
}

fn keycap_width(key: &str) -> usize {
    text_width(key) + 2
}

fn item_word(count: usize) -> &'static str {
    if count == 1 { "item" } else { "items" }
}

fn focus_label(focus: PaneFocus) -> &'static str {
    match focus {
        PaneFocus::List => "List",
        PaneFocus::Detail => "Detail",
        PaneFocus::Log => "Log",
    }
}

fn compact_label(value: &str, max_chars: usize) -> String {
    let chars: Vec<char> = value.chars().collect();
    if chars.len() <= max_chars {
        return value.to_string();
    }
    if max_chars == 0 {
        return String::new();
    }
    let keep = max_chars.saturating_sub(1);
    let mut out: String = chars.into_iter().take(keep).collect();
    out.push('~');
    out
}

fn draw_modal(frame: &mut Frame, app: &App) {
    match &app.modal {
        ModalState::None => {}
        ModalState::ListFilter { value, .. } => {
            let area = centered_rect(62, 22, frame.area());
            frame.render_widget(Clear, area);

            let shown = if value.is_empty() {
                "<empty: no filter>".to_string()
            } else {
                value.clone()
            };

            let lines = vec![
                Line::from("Type to filter visible list items by path."),
                Line::from(""),
                Line::from(vec![
                    Span::styled("query: ", Style::default().fg(Color::Gray)),
                    Span::styled(shown, Style::default().fg(Color::Yellow)),
                ]),
                Line::from(""),
                Line::from("Enter: apply and close  Esc: cancel  Backspace: delete"),
            ];

            let p = Paragraph::new(lines)
                .block(
                    Block::default()
                        .title(" List Filter ")
                        .borders(Borders::ALL)
                        .border_style(Style::default().fg(Color::LightBlue)),
                )
                .wrap(Wrap { trim: false });
            frame.render_widget(p, area);
        }
        ModalState::Ignore { requests, selected } => {
            let area = centered_rect(70, 42, frame.area());
            frame.render_widget(Clear, area);

            let target_text = requests
                .first()
                .and_then(|request| request.target.as_ref())
                .map_or_else(|| "(none)".to_string(), |path| path.display().to_string());
            let count = requests.len();
            let options = [
                ("Auto (recommended)", "file => exact, directory => /**"),
                ("Exact path", "Use exact path only"),
                ("Direct children", "Directory children only: /*"),
                ("Recursive", "Directory and all descendants: /**"),
                ("Global by name", "Any depth by name: **/<name>/**"),
            ];

            let mut lines = vec![
                Line::from(format!("targets: {count}")),
                Line::from(format!("sample target: {target_text}")),
                Line::from("scope: home-relative + global-by-name"),
                Line::from(""),
                Line::from("Select ignore rule mode:"),
            ];

            for (index, (label, description)) in options.into_iter().enumerate() {
                let prefix = if index == *selected { "▶" } else { " " };
                lines.push(Line::from(vec![
                    Span::styled(
                        format!("{prefix} {label}"),
                        if index == *selected {
                            Style::default()
                                .fg(Color::Black)
                                .bg(Color::LightYellow)
                                .add_modifier(Modifier::BOLD)
                        } else {
                            Style::default().fg(Color::White)
                        },
                    ),
                    Span::raw("  "),
                    Span::styled(
                        description.to_string(),
                        Style::default().fg(Color::DarkGray),
                    ),
                ]));
            }

            lines.push(Line::from(""));
            lines.push(Line::from(format!(
                "preview pattern: {}",
                ignore_pattern_preview(&target_text, *selected)
            )));
            lines.push(Line::from(""));
            lines.push(Line::from(
                "Up/Down or j/k: select  Enter: apply  Esc: cancel",
            ));

            let p = Paragraph::new(lines)
                .block(
                    Block::default()
                        .title(" Ignore Rule ")
                        .borders(Borders::ALL)
                        .border_style(Style::default().fg(Color::LightBlue)),
                )
                .wrap(Wrap { trim: false });
            frame.render_widget(p, area);
        }
        ModalState::AddOptions {
            requests,
            selected,
            template,
            private,
            executable,
            encrypted,
        } => {
            let area = centered_rect(70, 42, frame.area());
            frame.render_widget(Clear, area);

            let target_text = requests
                .first()
                .and_then(|request| request.target.as_ref())
                .map_or_else(|| "(none)".to_string(), |path| path.display().to_string());
            let count = requests.len();
            let options = [
                ("template", *template, "mark source as a template"),
                ("private", *private, "remove group/world permissions"),
                ("executable", *executable, "make destination executable"),
                ("encrypted", *encrypted, "encrypt source state"),
            ];

            let mut lines = vec![
                Line::from(format!("targets: {count}")),
                Line::from(format!("sample target: {target_text}")),
                Line::from(""),
                Line::from("Select attributes to apply after add:"),
            ];

            for (index, (label, enabled, description)) in options.into_iter().enumerate() {
                let prefix = if index == *selected { "▶" } else { " " };
                let checkbox = if enabled { "[x]" } else { "[ ]" };
                lines.push(Line::from(vec![
                    Span::styled(
                        format!("{prefix} {checkbox} {label}"),
                        if index == *selected {
                            Style::default()
                                .fg(Color::Black)
                                .bg(Color::LightYellow)
                                .add_modifier(Modifier::BOLD)
                        } else {
                            Style::default().fg(Color::White)
                        },
                    ),
                    Span::raw("  "),
                    Span::styled(
                        description.to_string(),
                        Style::default().fg(Color::DarkGray),
                    ),
                ]));
            }

            let attrs_preview = add_attrs_preview(*template, *private, *executable, *encrypted);
            lines.push(Line::from(""));
            lines.push(Line::from(format!("attrs preview: {attrs_preview}")));
            lines.push(Line::from(""));
            lines.push(Line::from(
                "Space: toggle  Up/Down or j/k: select  Enter: add  Esc: cancel",
            ));

            let p = Paragraph::new(lines)
                .block(
                    Block::default()
                        .title(" Add Attributes ")
                        .borders(Borders::ALL)
                        .border_style(Style::default().fg(Color::LightBlue)),
                )
                .wrap(Wrap { trim: false });
            frame.render_widget(p, area);
        }
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
                filter.clone()
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
                Line::from(
                    "Backspace: delete  danger:<name>: reveal danger  Enter: run  Esc: close",
                ),
            ])
            .block(
                Block::default()
                    .title(" Action Filter ")
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(Color::LightBlue)),
            )
            .wrap(Wrap { trim: false });
            frame.render_widget(filter_widget, sections[0]);

            let indices = app.action_menu_indices(filter);
            let actions: Vec<Action> = indices
                .iter()
                .filter_map(|index| App::action_by_index(*index))
                .collect();
            let filtering = !filter.trim().is_empty();

            let rows = action_menu_rows_for_app(app, &actions, filtering, filter);
            let mut selectable = Vec::new();
            let items: Vec<ListItem> = if rows.is_empty() {
                let message = if filter.trim().starts_with("danger:") {
                    "No dangerous actions match. Try danger:destroy or danger:purge"
                } else {
                    "No actions match. Dangerous actions require danger:<name>."
                };
                vec![ListItem::new(message)]
            } else {
                rows.into_iter()
                    .enumerate()
                    .map(|(row_index, row)| {
                        if matches!(row, ActionMenuRow::Action(_)) {
                            selectable.push(row_index);
                        }
                        action_menu_row_item(row)
                    })
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
                let action_index = (*selected).min(indices.len().saturating_sub(1));
                let row_index = selectable.get(action_index).copied().unwrap_or(0);
                state.select(Some(row_index));
            }
            frame.render_stateful_widget(list, sections[1], &mut state);
        }
        ModalState::ActionPreflight { requests, scroll } => {
            let area = centered_rect(76, 64, frame.area());
            frame.render_widget(Clear, area);
            let action = requests.first().map(|request| request.action);
            let mut lines = Vec::new();
            lines.push(Line::from("Review before running."));
            lines.push(Line::from(""));
            if let Some(action) = action {
                lines.push(Line::from(format!("action: {}", action.label())));
                lines.push(Line::from(format!(
                    "impact: {}",
                    action_preflight_impact(action)
                )));
                lines.push(Line::from(format!(
                    "mode: {}",
                    action_preflight_mode(action)
                )));
                lines.push(Line::from(format!(
                    "targets: {}",
                    requests.iter().filter(|r| r.target.is_some()).count()
                )));
                lines.push(Line::from(""));
                lines.push(Line::from("command preview:"));
                for command in action_preflight_commands(requests, 3) {
                    lines.push(Line::from(format!("  {command}")));
                }
                if requests.len() > 3 && action.needs_target() {
                    lines.push(Line::from(format!(
                        "  ... and {} more target commands",
                        requests.len() - 3
                    )));
                }
                lines.push(Line::from(""));
                lines.push(Line::from("targets:"));
                for request in requests {
                    let target = request
                        .target
                        .as_ref()
                        .map_or_else(|| "(none)".to_string(), |path| path.display().to_string());
                    lines.push(Line::from(format!("  {target}")));
                }
            }
            lines.push(Line::from(""));
            lines.push(Line::from(
                "Enter: continue  Esc: cancel  j/k/PgUp/PgDn: scroll",
            ));

            let visible_rows = area.height.saturating_sub(2) as usize;
            let max_scroll = lines.len().saturating_sub(visible_rows.max(1));
            let p = Paragraph::new(lines)
                .block(
                    Block::default()
                        .title(" Action Preflight ")
                        .borders(Borders::ALL)
                        .border_style(Style::default().fg(Color::LightYellow)),
                )
                .scroll((clamp_to_u16((*scroll).min(max_scroll)), 0))
                .wrap(Wrap { trim: false });
            frame.render_widget(p, area);
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
                        .map_or_else(|| "(none)".to_string(), |p| p.display().to_string())
                )),
            ];

            if let Some(attrs) = &request.chattr_attrs {
                lines.push(Line::from(format!("attributes: {attrs}")));
            }
            if app.batch_in_progress() {
                lines.push(Line::from(format!(
                    "batch: action={} total={} targets",
                    app.batch_action().map_or("unknown", Action::label),
                    app.batch_total()
                )));
                if request.action.is_dangerous() {
                    lines.push(Line::from(
                        "Dangerous batch items require confirmation for each target.",
                    ));
                } else {
                    lines.push(Line::from(
                        "Confirmation applies to the remaining queued batch items.",
                    ));
                }
            }

            let impact_lines = app.confirmation_impact_lines(request);
            if !impact_lines.is_empty() {
                lines.push(Line::from(""));
                for line in impact_lines {
                    lines.push(Line::from(line).style(Style::default().fg(Color::LightRed)));
                }
            }

            lines.push(Line::from(""));
            match step {
                ConfirmStep::Primary => {
                    if request.requires_strict_confirmation() {
                        lines.push(Line::from("Enter: Continue  Esc: Cancel"));
                        lines.push(Line::from(
                            "This is a dangerous action. A confirmation phrase is always required.",
                        ));
                    } else {
                        lines.push(Line::from("Enter: Run  Esc: Cancel"));
                    }
                    if request.action.is_dangerous() && !request.requires_strict_confirmation() {
                        lines.push(Line::from(
                            "This is a dangerous action. A confirmation phrase is required next.",
                        ));
                    }
                }
                ConfirmStep::DangerPhrase => {
                    lines.push(Line::from(
                        "Type the confirmation phrase and press Enter to run, Esc to cancel.",
                    ));
                    if let Some(phrase) = request.confirmation_phrase() {
                        lines.push(
                            Line::from(format!("required: {phrase}")).style(
                                Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
                            ),
                        );
                    }
                    lines.push(
                        Line::from(format!("input: {typed}"))
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
        ModalState::ApplyPlan {
            request,
            plan,
            scroll,
        } => {
            let area = centered_rect(76, 64, frame.area());
            frame.render_widget(Clear, area);

            let mut lines = vec![
                Line::from(format!("action: {}", request.action.label())),
                Line::from(format!("total changes: {}", plan.total())),
                Line::from(""),
                Line::from(format!("Added: {}", plan.added.len())),
                Line::from(format!("Modified: {}", plan.modified.len())),
                Line::from(format!("Deleted: {}", plan.deleted.len())),
                Line::from(format!("Run scripts: {}", plan.run.len())),
                Line::from(format!("Unknown: {}", plan.unknown.len())),
            ];

            if plan.total() == 0 {
                lines.push(Line::from(""));
                lines.push(Line::from("No pending status entries were loaded."));
            }
            if !plan.run.is_empty() {
                lines.push(Line::from(""));
                lines.push(
                    Line::from("Warning: apply may run scripts.")
                        .style(Style::default().fg(Color::LightYellow)),
                );
            }
            if !plan.unknown.is_empty() {
                lines.push(
                    Line::from("Warning: unknown status entries need manual review.")
                        .style(Style::default().fg(Color::LightYellow)),
                );
            }

            push_apply_plan_group(
                &mut lines,
                "Added",
                &plan.added,
                Style::default().fg(Color::LightGreen),
            );
            push_apply_plan_group(
                &mut lines,
                "Modified",
                &plan.modified,
                Style::default().fg(Color::LightBlue),
            );
            push_apply_plan_group(
                &mut lines,
                "Deleted",
                &plan.deleted,
                Style::default().fg(Color::LightRed),
            );
            push_apply_plan_group(
                &mut lines,
                "Run scripts",
                &plan.run,
                Style::default()
                    .fg(Color::LightYellow)
                    .add_modifier(Modifier::BOLD),
            );
            push_apply_plan_group(
                &mut lines,
                "Unknown",
                &plan.unknown,
                Style::default()
                    .fg(Color::LightMagenta)
                    .add_modifier(Modifier::BOLD),
            );
            lines.push(Line::from(""));
            lines.push(Line::from(
                "Enter: continue to confirmation  d: diff  Esc: cancel  j/k/PgUp/PgDn: scroll",
            ));

            let visible_rows = area.height.saturating_sub(2) as usize;
            let max_scroll = lines.len().saturating_sub(visible_rows.max(1));
            let p = Paragraph::new(lines)
                .block(
                    Block::default()
                        .title(" Apply Plan ")
                        .borders(Borders::ALL)
                        .border_style(Style::default().fg(Color::LightBlue)),
                )
                .scroll((clamp_to_u16((*scroll).min(max_scroll)), 0))
                .wrap(Wrap { trim: false });
            frame.render_widget(p, area);
        }
        ModalState::Help { scroll } => {
            let area = centered_rect(78, 78, frame.area());
            frame.render_widget(Clear, area);
            let lines = help_lines();
            let visible_rows = area.height.saturating_sub(2) as usize;
            let max_scroll = lines.len().saturating_sub(visible_rows.max(1));
            let p = Paragraph::new(lines)
                .block(
                    Block::default()
                        .title(" Help / Legends ")
                        .borders(Borders::ALL)
                        .border_style(Style::default().fg(Color::LightBlue)),
                )
                .scroll((clamp_to_u16((*scroll).min(max_scroll)), 0))
                .wrap(Wrap { trim: false });
            frame.render_widget(p, area);
        }
        ModalState::NoticeHistory { scroll } => {
            let area = centered_rect(76, 64, frame.area());
            frame.render_widget(Clear, area);
            let mut lines = vec![Line::from("Recent notices and errors:"), Line::from("")];
            if app.notice_history().is_empty() {
                lines.push(Line::from("No notices yet."));
            } else {
                for (index, notice) in app.notice_history().iter().enumerate() {
                    lines.push(Line::from(vec![
                        Span::styled(
                            format!("{:>2} ", index + 1),
                            Style::default().fg(Color::DarkGray),
                        ),
                        Span::styled(notice_tone_label(notice.tone), notice_style(notice.tone)),
                        Span::raw(" "),
                        Span::styled(notice.message.clone(), Style::default().fg(Color::White)),
                    ]));
                }
            }
            lines.push(Line::from(""));
            lines.push(Line::from("Esc: close  j/k/PgUp/PgDn: scroll"));
            let visible_rows = area.height.saturating_sub(2) as usize;
            let max_scroll = lines.len().saturating_sub(visible_rows.max(1));
            let p = Paragraph::new(lines)
                .block(
                    Block::default()
                        .title(" Notice History ")
                        .borders(Borders::ALL)
                        .border_style(Style::default().fg(Color::LightBlue)),
                )
                .scroll((clamp_to_u16((*scroll).min(max_scroll)), 0))
                .wrap(Wrap { trim: false });
            frame.render_widget(p, area);
        }
        ModalState::Search { scope, value } => {
            let area = centered_rect(62, 22, frame.area());
            frame.render_widget(Clear, area);
            let scope_label = match scope {
                SearchScope::Detail => "Detail",
                SearchScope::Log => "Log",
            };
            let shown = if value.is_empty() { "<empty>" } else { value };
            let lines = vec![
                Line::from(format!("Search in {scope_label} pane.")),
                Line::from(""),
                Line::from(vec![
                    Span::styled("query: ", Style::default().fg(Color::Gray)),
                    Span::styled(shown.to_string(), Style::default().fg(Color::Yellow)),
                ]),
                Line::from(""),
                Line::from("Enter: search/jump  Esc: cancel  Backspace: delete"),
            ];
            let p = Paragraph::new(lines)
                .block(
                    Block::default()
                        .title(" Pane Search ")
                        .borders(Borders::ALL)
                        .border_style(Style::default().fg(Color::LightBlue)),
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
                        .map_or_else(|| "(none)".to_string(), |p| p.display().to_string())
                )),
                Line::from(""),
                Line::from(prompt),
                Line::from(format!("> {value}")).style(Style::default().fg(Color::Yellow)),
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ActionMenuSection {
    Global,
    SelectedItem,
    Danger,
    Unavailable,
}

impl ActionMenuSection {
    fn title(self) -> &'static str {
        match self {
            Self::Global => "Global",
            Self::SelectedItem => "Selected item",
            Self::Danger => "Danger",
            Self::Unavailable => "Unavailable",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ActionMenuRow {
    Header(ActionMenuSection),
    Spacer,
    Note(String),
    Action(Action),
    Disabled(Action, String),
}

fn action_menu_section(action: Action) -> ActionMenuSection {
    if action.is_dangerous() {
        ActionMenuSection::Danger
    } else if action.needs_target() {
        ActionMenuSection::SelectedItem
    } else {
        ActionMenuSection::Global
    }
}

fn build_action_menu_rows(actions: &[Action]) -> Vec<ActionMenuRow> {
    let mut global = Vec::new();
    let mut selected = Vec::new();
    let mut danger = Vec::new();

    for action in actions {
        match action_menu_section(*action) {
            ActionMenuSection::Global => global.push(*action),
            ActionMenuSection::SelectedItem => selected.push(*action),
            ActionMenuSection::Danger => danger.push(*action),
            ActionMenuSection::Unavailable => {}
        }
    }

    let sections = [
        (ActionMenuSection::Global, global),
        (ActionMenuSection::SelectedItem, selected),
        (ActionMenuSection::Danger, danger),
    ];

    let mut rows = Vec::new();
    for (section, actions) in sections {
        if actions.is_empty() {
            continue;
        }
        if !rows.is_empty() {
            rows.push(ActionMenuRow::Spacer);
        }
        rows.push(ActionMenuRow::Header(section));
        rows.extend(actions.into_iter().map(ActionMenuRow::Action));
    }

    rows
}

fn action_menu_rows(actions: &[Action], filtering: bool) -> Vec<ActionMenuRow> {
    if filtering {
        return actions.iter().copied().map(ActionMenuRow::Action).collect();
    }

    build_action_menu_rows(actions)
}

fn action_menu_rows_for_app(
    app: &App,
    actions: &[Action],
    filtering: bool,
    filter: &str,
) -> Vec<ActionMenuRow> {
    let mut rows = action_menu_rows(actions, filtering);
    if filter.trim().is_empty() {
        rows.push(ActionMenuRow::Spacer);
        rows.push(ActionMenuRow::Note(
            "Danger actions are hidden from plain filtering; type danger:<name>.".to_string(),
        ));
        let disabled: Vec<_> = Action::ALL
            .iter()
            .copied()
            .filter_map(|action| {
                app.action_disabled_reason(action)
                    .map(|reason| (action, reason))
            })
            .collect();
        if !disabled.is_empty() {
            rows.push(ActionMenuRow::Spacer);
            rows.push(ActionMenuRow::Header(ActionMenuSection::Unavailable));
            rows.extend(
                disabled
                    .into_iter()
                    .map(|(action, reason)| ActionMenuRow::Disabled(action, reason)),
            );
        }
    } else if filter.trim() == "danger:" {
        rows.push(ActionMenuRow::Note(
            "Examples: danger:destroy, danger:purge".to_string(),
        ));
    }
    rows
}

fn action_menu_text(action: Action) -> String {
    if action.is_dangerous() {
        format!(
            "  {:<10} !! {} [danger]",
            action.label(),
            action.description()
        )
    } else {
        format!("  {:<10}    {}", action.label(), action.description())
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

fn help_lines() -> Vec<Line<'static>> {
    vec![
        Line::from("Views"),
        Line::from("  1 Status    pending chezmoi changes at the destination/home"),
        Line::from("  2 Managed   files managed by chezmoi under destination/home"),
        Line::from("  3 Unmanaged files in current working directory not managed by chezmoi"),
        Line::from("  4 Source    chezmoi source directory"),
        Line::from(""),
        Line::from("Status symbols"),
        Line::from("  Two columns are shown before each status path."),
        Line::from("  Column 1: actual file vs chezmoi source/state."),
        Line::from("  Column 2: actual file vs target state to apply."),
        Line::from("  A added   M modified   D deleted   R run script   other = unknown"),
        Line::from(""),
        Line::from("Tree symbols"),
        Line::from("  [+] collapsed directory   [-] expanded directory   [ ] directory"),
        Line::from("  [L] symlink directory     L symlink file           @ symlink suffix"),
        Line::from("  / directory suffix"),
        Line::from(""),
        Line::from("Actions and safety"),
        Line::from("  a opens actions. Broad/risky actions show a preflight review first."),
        Line::from("  Dangerous actions are hidden from normal filtering."),
        Line::from("  Type danger:destroy or danger:purge to reveal dangerous actions."),
        Line::from("  Destroy/purge still require typed confirmation phrases."),
        Line::from("  Disabled action rows explain why an action is unavailable."),
        Line::from(""),
        Line::from("Search and history"),
        Line::from("  / in List filters paths. / in Detail or Log searches that pane."),
        Line::from("  n/N jumps search matches; in diff without search, n/N jumps hunks."),
        Line::from("  H/L horizontally scroll Detail or Log. m maximizes/restores focused pane."),
        Line::from("  ! opens notice history."),
        Line::from(""),
        Line::from("Keys"),
        Line::from("  Tab focus panes   j/k or arrows move/scroll   r refresh   q quit"),
        Line::from("  Esc closes modals. This help scrolls with j/k/PgUp/PgDn."),
    ]
}

fn ignore_pattern_preview(target: &str, selected: usize) -> String {
    let name = Path::new(target)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(target);
    match selected {
        0 => format!("auto for {target}"),
        1 => target.to_string(),
        2 => format!("{target}/*"),
        3 => format!("{target}/**"),
        4 => format!("**/{name}/**"),
        _ => "unknown".to_string(),
    }
}

fn add_attrs_preview(template: bool, private: bool, executable: bool, encrypted: bool) -> String {
    let attrs = [
        (template, "template"),
        (private, "private"),
        (executable, "executable"),
        (encrypted, "encrypted"),
    ]
    .into_iter()
    .filter_map(|(enabled, label)| enabled.then_some(label))
    .collect::<Vec<_>>();
    if attrs.is_empty() {
        "none".to_string()
    } else {
        attrs.join(",")
    }
}

fn action_preflight_impact(action: Action) -> &'static str {
    match action {
        Action::Apply => "may update files in the destination",
        Action::Update => "updates source then applies changes",
        Action::MergeAll => "runs merge workflow for all changes",
        Action::Add => "imports selected files into chezmoi source state",
        Action::Ignore => "appends ignore rules to .chezmoiignore",
        Action::ReAdd => "re-imports selected modified files",
        Action::Forget => "removes selected targets from chezmoi state",
        Action::Chattr => "changes source attributes for selected targets",
        Action::Destroy => "deletes selected target from source/destination/state",
        Action::Purge => "removes chezmoi config and data for this environment",
        _ => action.description(),
    }
}

fn action_preflight_mode(action: Action) -> &'static str {
    match action {
        Action::Edit | Action::Update | Action::Merge | Action::MergeAll => "foreground command",
        Action::Ignore => "internal file update",
        _ => "background command",
    }
}

fn action_preflight_commands(requests: &[ActionRequest], limit: usize) -> Vec<String> {
    requests
        .iter()
        .take(limit)
        .map(|request| match action_to_args(request) {
            Ok(args) => {
                let args = args
                    .into_iter()
                    .map(|arg| arg.to_string_lossy().into_owned())
                    .collect::<Vec<_>>()
                    .join(" ");
                format!("chezmoi {args}")
            }
            Err(_) if request.action == Action::Ignore => {
                let mode = request.chattr_attrs.as_deref().unwrap_or("auto");
                format!("internal ignore rule update ({mode})")
            }
            Err(_) => format!("internal/foreground action: {}", request.action.label()),
        })
        .collect()
}

fn push_apply_plan_group(
    lines: &mut Vec<Line<'static>>,
    title: &'static str,
    paths: &[std::path::PathBuf],
    style: Style,
) {
    if paths.is_empty() {
        return;
    }
    lines.push(Line::from(""));
    lines.push(Line::from(format!("{title} ({})", paths.len())).style(style));
    for path in paths {
        lines.push(Line::from(format!("  {}", path.display())));
    }
}

fn action_menu_row_item(row: ActionMenuRow) -> ListItem<'static> {
    match row {
        ActionMenuRow::Header(section) => ListItem::new(Line::styled(
            format!(" {} ", section.title()),
            Style::default()
                .fg(Color::LightBlue)
                .add_modifier(Modifier::BOLD),
        )),
        ActionMenuRow::Spacer => ListItem::new(Line::from("")),
        ActionMenuRow::Note(text) => ListItem::new(Line::styled(
            format!("  {text}"),
            Style::default().fg(Color::DarkGray),
        )),
        ActionMenuRow::Action(action) => action_menu_item(action),
        ActionMenuRow::Disabled(action, reason) => ListItem::new(Line::styled(
            format!("  {:<10} -- disabled: {}", action.label(), reason),
            Style::default().fg(Color::DarkGray),
        )),
    }
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
        .map(str::to_ascii_lowercase);
    match ext.as_deref() {
        Some("rs") => PreviewLanguage::Rust,
        Some("sh" | "bash" | "zsh" | "fish") => PreviewLanguage::Shell,
        Some("lua") => PreviewLanguage::Lua,
        Some("py") => PreviewLanguage::Python,
        Some("js" | "mjs" | "cjs" | "ts" | "tsx" | "jsx") => PreviewLanguage::JsTs,
        Some("json") => PreviewLanguage::Json,
        Some("toml") => PreviewLanguage::Toml,
        Some("yaml" | "yml") => PreviewLanguage::Yaml,
        _ => {
            let name = path
                .and_then(|p| p.file_name().and_then(|n| n.to_str()))
                .unwrap_or_default()
                .to_ascii_lowercase();
            match name.as_str() {
                ".zshrc" | ".bashrc" | ".bash_profile" => PreviewLanguage::Shell,
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
        PreviewLanguage::Json | PreviewLanguage::Yaml => &["true", "false", "null"],
        PreviewLanguage::Toml => &["true", "false"],
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
    use super::{
        ActionMenuRow, ActionMenuSection, action_menu_rows, action_menu_text,
        build_action_menu_rows, cheat_groups, cheat_groups_width, fit_cheat_groups, footer_hints,
        footer_left, hints_width, layout_hints, log_scroll_offset,
    };
    use crate::app::{App, PaneFocus};
    use crate::config::AppConfig;
    use crate::domain::Action;
    use crate::domain::ListView;
    use crate::ui_diff::colorized_diff_lines;
    use ratatui::style::Color;
    use ratatui::text::Line;

    fn render_line_text(line: &Line<'_>) -> String {
        line.spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect::<String>()
    }

    #[test]
    fn colorized_diff_lines_normalize_ansi_wrapped_diff_output() {
        let diff = concat!(
            "\u{1b}[34mdiff --git a/.zshrc b/.zshrc\u{1b}[0m\n",
            "\u{1b}[31m--- a/.zshrc\u{1b}[0m\n",
            "\u{1b}[32m+++ b/.zshrc\u{1b}[0m\n",
            "\u{1b}[33m@@ -1,1 +1,1 @@\u{1b}[0m\n",
            "\u{1b}[31m-old\u{1b}[0m\n",
            "\u{1b}[32m+new\u{1b}[0m\n",
        );

        let lines = colorized_diff_lines(diff);
        let rendered: Vec<String> = lines.iter().map(render_line_text).collect();

        assert!(rendered.iter().all(|line| !line.contains('\u{1b}')));
        assert_eq!(rendered[0], "diff --git a/.zshrc b/.zshrc");
        assert_eq!(rendered[1], "--- a/.zshrc");
        assert_eq!(rendered[2], "+++ b/.zshrc");
        assert_eq!(rendered[3], "@@ -1,1 +1,1 @@");
        assert_eq!(rendered[4], "-old");
        assert_eq!(rendered[5], "+new");
    }

    #[test]
    fn colorized_diff_lines_preserve_ansi_colors_when_present() {
        let diff = "\u{1b}[38;5;81mdiff --git a/.zshrc b/.zshrc\u{1b}[0m\n\u{1b}[38;5;203m-old\u{1b}[0m\n\u{1b}[38;5;149m+new\u{1b}[0m\n";

        let lines = colorized_diff_lines(diff);

        assert_eq!(render_line_text(&lines[0]), "diff --git a/.zshrc b/.zshrc");
        assert_eq!(lines[0].spans[0].style.fg, Some(Color::Indexed(81)));

        assert_eq!(render_line_text(&lines[1]), "-old");
        assert_eq!(lines[1].spans[0].style.fg, Some(Color::Indexed(203)));

        assert_eq!(render_line_text(&lines[2]), "+new");
        assert_eq!(lines[2].spans[0].style.fg, Some(Color::Indexed(149)));
    }

    #[test]
    fn log_scroll_offset_keeps_latest_visible() {
        assert_eq!(log_scroll_offset(10, 6, 0), 6);
        assert_eq!(log_scroll_offset(10, 6, 2), 4);
        assert_eq!(log_scroll_offset(3, 10, 0), 0);
    }

    #[test]
    fn footer_left_shows_selected_ordinal_with_total_items() {
        let mut app = App::new(AppConfig::default());
        app.managed_entries = vec![
            std::path::PathBuf::from("a"),
            std::path::PathBuf::from("b"),
            std::path::PathBuf::from("c"),
        ];
        app.switch_view(ListView::Managed);
        app.selected_index = 1;

        let (spans, _) = footer_left(&app, 120);
        let rendered = spans
            .into_iter()
            .map(|span| span.content.to_string())
            .collect::<String>();

        assert!(rendered.contains("2/3 items"));
    }

    #[test]
    fn footer_left_shows_latest_notice() {
        let mut app = App::new(AppConfig::default());
        app.set_error_notice("apply failed for dotfile");

        let (spans, _) = footer_left(&app, 120);
        let rendered = spans
            .into_iter()
            .map(|span| span.content.to_string())
            .collect::<String>();

        assert!(rendered.contains("apply failed for dotfile"));
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

    #[test]
    fn action_menu_rows_are_grouped_into_sections() {
        let rows = build_action_menu_rows(&[
            Action::Apply,
            Action::Update,
            Action::Add,
            Action::Edit,
            Action::Destroy,
            Action::Purge,
        ]);

        let global_idx = rows
            .iter()
            .position(|row| matches!(row, ActionMenuRow::Header(ActionMenuSection::Global)))
            .expect("global header");
        let selected_idx = rows
            .iter()
            .position(|row| matches!(row, ActionMenuRow::Header(ActionMenuSection::SelectedItem)))
            .expect("selected header");
        let danger_idx = rows
            .iter()
            .position(|row| matches!(row, ActionMenuRow::Header(ActionMenuSection::Danger)))
            .expect("danger header");

        assert!(global_idx < selected_idx);
        assert!(selected_idx < danger_idx);
        assert!(rows.contains(&ActionMenuRow::Action(Action::Apply)));
        assert!(rows.contains(&ActionMenuRow::Action(Action::Edit)));
        assert!(rows.contains(&ActionMenuRow::Action(Action::Purge)));
    }

    #[test]
    fn action_menu_rows_are_flat_when_filtering() {
        let rows = action_menu_rows(&[Action::EditIgnore, Action::Ignore], true);
        assert_eq!(
            rows,
            vec![
                ActionMenuRow::Action(Action::EditIgnore),
                ActionMenuRow::Action(Action::Ignore),
            ]
        );
        assert!(
            !rows
                .iter()
                .any(|row| matches!(row, ActionMenuRow::Header(_)))
        );
        assert!(!rows.iter().any(|row| matches!(row, ActionMenuRow::Spacer)));
    }

    #[test]
    fn footer_hints_hide_diff_in_unmanaged_list_view() {
        let mut app = App::new(AppConfig::default());
        app.focus = PaneFocus::List;
        app.view = ListView::Unmanaged;

        let hints = footer_hints(&app);
        let labels: Vec<&str> = hints
            .iter()
            .filter(|hint| hint.enabled)
            .map(|hint| hint.label)
            .collect();

        assert!(!labels.contains(&"Diff"));
        assert!(labels.contains(&"View"));
        assert!(!labels.contains(&"Fold"));
        assert!(!labels.contains(&"Pane"));
    }

    #[test]
    fn footer_hints_show_scroll_only_for_detail_focus() {
        let mut app = App::new(AppConfig::default());
        app.focus = PaneFocus::Detail;
        app.view = ListView::Managed;

        let hints = footer_hints(&app);
        let labels: Vec<&str> = hints
            .iter()
            .filter(|hint| hint.enabled)
            .map(|hint| hint.label)
            .collect();

        assert!(labels.contains(&"Scroll"));
        assert!(labels.contains(&"Page"));
        assert!(labels.contains(&"Jump"));
        assert!(!labels.contains(&"Diff"));
        assert!(!labels.contains(&"Fold"));
    }

    #[test]
    fn footer_hints_include_help_globally() {
        let app = App::new(AppConfig::default());
        let hints = footer_hints(&app);
        let labels: Vec<&str> = hints
            .iter()
            .filter(|hint| hint.enabled)
            .map(|hint| hint.label)
            .collect();
        assert!(labels.contains(&"Help"));
        assert!(labels.contains(&"Actions"));
        assert!(labels.contains(&"Quit"));
    }

    #[test]
    fn footer_hints_fit_keeps_mandatory_on_narrow_width() {
        let app = App::new(AppConfig::default());
        let rendered = layout_hints(18, footer_hints(&app));
        let keys: Vec<&str> = rendered.iter().map(|hint| hint.key).collect();
        assert!(keys.contains(&"a"));
        assert!(keys.contains(&"?"));
        assert!(keys.contains(&"q"));
        assert!(hints_width(&rendered) <= 18);
    }

    #[test]
    fn footer_hints_fit_prefers_more_hints_on_wider_terminal() {
        let app = App::new(AppConfig::default());
        let narrow = layout_hints(40, footer_hints(&app));
        let wide = layout_hints(80, footer_hints(&app));

        assert!(hints_width(&wide) <= 80);
        assert!(hints_width(&narrow) <= 40);
        assert!(wide.len() >= narrow.len());
    }

    #[test]
    fn layout_hints_never_shows_muted_entries() {
        let mut app = App::new(AppConfig::default());
        app.focus = PaneFocus::List;
        let normal = layout_hints(120, footer_hints(&app));
        let normal_keys: Vec<&str> = normal.iter().map(|hint| hint.key).collect();
        assert!(!normal_keys.contains(&"Tab"));
        assert!(!normal_keys.contains(&"1-4"));
        assert!(!normal_keys.contains(&"h/l"));
    }

    #[test]
    fn cheat_groups_are_ordered_nav_view_global() {
        let app = App::new(AppConfig::default());
        let groups = cheat_groups(&app);
        let titles: Vec<&str> = groups.iter().map(|group| group.title).collect();
        assert_eq!(titles, vec!["Nav", "View", "Global"]);
    }

    #[test]
    fn cheat_groups_drop_view_first_when_narrow() {
        let mut app = App::new(AppConfig::default());
        app.view = ListView::Managed;
        let groups = cheat_groups(&app);
        let (selected, omitted) = fit_cheat_groups(&groups, 56);
        let mut kept_titles = Vec::new();
        for (group, keep) in groups.iter().zip(selected.iter()) {
            if *keep {
                kept_titles.push(group.title);
            }
        }
        assert!(omitted);
        assert!(kept_titles.contains(&"Global"));
        assert!(cheat_groups_width(&groups, &selected, omitted) <= 56);
    }
}
