use crate::app::{App, ConfirmStep, DetailKind, InputKind, ModalState, PaneFocus};
use crate::domain::{Action, ListView};
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

    let title = if app.list_filter().trim().is_empty() {
        format!(" {} ", app.view.title())
    } else {
        format!(" {} /{} ", app.view.title(), app.list_filter())
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
        .scroll((clamp_to_u16(app.detail_scroll), 0))
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
    let scroll = log_scroll_offset(lines.len(), area.height, app.log_tail_offset);

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

    if app.busy {
        segments.push(LeftSegment {
            text: "Busy".to_string(),
            style: Style::default().fg(Color::LightYellow),
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
            app.footer_help && matches!(app.view, ListView::Managed | ListView::Unmanaged),
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
            "1-3",
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
    if !app.busy {
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

            if matches!(app.view, ListView::Managed | ListView::Unmanaged) {
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
            ]);
        }
    }

    view_items.extend([
        CheatItem {
            key: "Tab",
            label: "Pane",
        },
        CheatItem {
            key: "1-3",
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
            let actions: Vec<Action> = indices
                .iter()
                .filter_map(|index| App::action_by_index(*index))
                .collect();
            let filtering = !filter.trim().is_empty();

            let (items, selectable_rows): (Vec<ListItem>, Vec<usize>) = if actions.is_empty() {
                (
                    vec![ListItem::new("No actions match the current filter")],
                    Vec::new(),
                )
            } else {
                let rows = action_menu_rows(&actions, filtering);
                let mut selectable = Vec::new();
                let items = rows
                    .into_iter()
                    .enumerate()
                    .map(|(row_index, row)| {
                        if matches!(row, ActionMenuRow::Action(_)) {
                            selectable.push(row_index);
                        }
                        action_menu_row_item(row)
                    })
                    .collect();
                (items, selectable)
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
                let row_index = selectable_rows.get(action_index).copied().unwrap_or(0);
                state.select(Some(row_index));
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
                        .map_or_else(|| "(none)".to_string(), |p| p.display().to_string())
                )),
            ];

            if let Some(attrs) = &request.chattr_attrs {
                lines.push(Line::from(format!("attributes: {attrs}")));
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
}

impl ActionMenuSection {
    fn title(self) -> &'static str {
        match self {
            Self::Global => "Global",
            Self::SelectedItem => "Selected item",
            Self::Danger => "Danger",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ActionMenuRow {
    Header(ActionMenuSection),
    Spacer,
    Action(Action),
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

fn action_menu_row_item(row: ActionMenuRow) -> ListItem<'static> {
    match row {
        ActionMenuRow::Header(section) => ListItem::new(Line::styled(
            format!(" {} ", section.title()),
            Style::default()
                .fg(Color::LightBlue)
                .add_modifier(Modifier::BOLD),
        )),
        ActionMenuRow::Spacer => ListItem::new(Line::from("")),
        ActionMenuRow::Action(action) => action_menu_item(action),
    }
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
    let old_num = old.map_or_else(String::new, |n| n.to_string());
    let new_num = new.map_or_else(String::new, |n| n.to_string());

    let marker_style = match marker {
        '+' => Style::default()
            .fg(Color::Green)
            .add_modifier(Modifier::BOLD),
        '-' => Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
        _ => Style::default().fg(Color::DarkGray),
    };

    Line::from(vec![
        Span::styled(
            format!("{old_num:>5}"),
            Style::default().fg(Color::DarkGray),
        ),
        Span::raw(" "),
        Span::styled(
            format!("{new_num:>5}"),
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
        footer_left, hints_width, layout_hints, log_scroll_offset, parse_hunk_header,
    };
    use crate::app::{App, PaneFocus};
    use crate::config::AppConfig;
    use crate::domain::Action;
    use crate::domain::ListView;

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
        assert!(!normal_keys.contains(&"1-3"));
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
