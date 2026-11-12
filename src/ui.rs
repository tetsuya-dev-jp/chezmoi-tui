use crate::action_policy::ActionGroup;
use crate::app::{
    App, ConfirmStep, DetailKind, InputKind, LayoutMode, ModalState, NoticeTone, PaneFocus,
    SearchScope,
};
use crate::domain::{Action, ActionRequest, ListView};
use crate::infra::action_to_args;
use crate::ui_diff::colorized_diff_lines;
use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::prelude::{Color, Line, Modifier, Span, Style};
use ratatui::widgets::{
    Block, BorderType, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap,
};
use std::collections::HashSet;
use std::path::Path;

pub fn draw(frame: &mut Frame, app: &mut App) {
    draw_styled(frame, app);
    apply_no_color(frame);
}

fn draw_styled(frame: &mut Frame, app: &mut App) {
    let area = frame.area();
    if area.width < 24 || area.height < 6 {
        draw_tiny(frame, app, area);
        draw_modal(frame, app);
        ensure_dark_canvas(frame.buffer_mut());
        return;
    }

    let footer_height = if area.height >= 10 {
        if app.footer_help { 3 } else { 2 }
    } else {
        1
    };
    let outer = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2),
            Constraint::Min(1),
            Constraint::Length(footer_height),
        ])
        .split(area);
    draw_header(frame, app, outer[0]);

    let single_pane = area.width < 90 || area.height < 18;

    match (app.layout_mode, single_pane) {
        (LayoutMode::DetailMax, _) => draw_detail(frame, app, outer[1]),
        (LayoutMode::LogMax, _) => draw_logs(frame, app, outer[1]),
        (LayoutMode::Normal, true) => match app.focus {
            PaneFocus::List => draw_list(frame, app, outer[1]),
            PaneFocus::Detail => draw_detail(frame, app, outer[1]),
            PaneFocus::Log => draw_logs(frame, app, outer[1]),
        },
        (LayoutMode::Normal, false) => {
            let list_ratio = app.config.list_ratio;
            let main = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([
                    Constraint::Percentage(list_ratio),
                    Constraint::Percentage(100 - list_ratio),
                ])
                .split(outer[1]);

            let right = if app.focus == PaneFocus::Log {
                let detail_ratio = app.config.detail_ratio.min(90);
                Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([
                        Constraint::Percentage(detail_ratio),
                        Constraint::Percentage(100 - detail_ratio),
                    ])
                    .split(main[1])
            } else {
                Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([Constraint::Min(1), Constraint::Length(4)])
                    .split(main[1])
            };

            draw_list(frame, app, main[0]);
            draw_detail(frame, app, right[0]);
            draw_logs(frame, app, right[1]);
        }
    }
    draw_status_bar(frame, app, outer[2]);
    draw_modal(frame, app);
    ensure_dark_canvas(frame.buffer_mut());
}

fn draw_list(frame: &mut Frame, app: &mut App, area: Rect) {
    let viewport_rows = area.height.saturating_sub(2) as usize;
    app.sync_list_scroll(viewport_rows);
    let len = app.current_len();
    let selected_ordinal = if len == 0 {
        0
    } else {
        app.selected_index.min(len - 1) + 1
    };
    let list_title = match app.view {
        ListView::Source => "Source (raw)",
        _ => app.view.title(),
    };
    let title = format!(
        "{} {} · {selected_ordinal}/{len}{}",
        focus_marker(app.focus == PaneFocus::List),
        list_title,
        filter_title_suffix(app)
    );

    let border_style = if app.focus == PaneFocus::List {
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default()
    };

    let row_width = area.width.saturating_sub(4) as usize;
    let items: Vec<ListItem> = if len == 0 {
        let message = empty_list_message(app);
        vec![ListItem::new(Line::styled(
            clip_text(&message, row_width),
            Style::default().fg(Color::DarkGray),
        ))]
    } else {
        let offset = app.list_scroll();
        app.current_items_window(offset, viewport_rows)
            .into_iter()
            .map(|item| ListItem::new(Line::from(compact_path_row(&item, row_width))))
            .collect()
    };
    let has_rendered_items = !items.is_empty();

    let list = List::new(items)
        .block(
            Block::default()
                .title(format!(
                    " {} ",
                    clip_title(&title, area.width.saturating_sub(4) as usize)
                ))
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(border_style),
        )
        .highlight_style(selection_style())
        .highlight_symbol("› ");

    let mut state = ListState::default();
    if len > 0 && has_rendered_items {
        state.select(Some(app.selected_index.saturating_sub(app.list_scroll())));
    }

    frame.render_stateful_widget(list, area, &mut state);
}

fn draw_detail(frame: &mut Frame, app: &App, area: Rect) {
    let border_style = if app.focus == PaneFocus::Detail {
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default()
    };

    let lines = if let Some(lines) = empty_detail_lines(app) {
        lines
    } else if app.detail_kind == DetailKind::Diff {
        colorized_diff_lines(&app.detail_text)
    } else {
        colorized_preview_lines(app.detail_target.as_deref(), &app.detail_text)
    };
    let title = format!(
        "{} {} ",
        focus_marker(app.focus == PaneFocus::Detail),
        compact_middle(&app.detail_title, area.width.saturating_sub(6) as usize)
    );

    let paragraph = Paragraph::new(lines)
        .block(
            Block::default()
                .title(title)
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(border_style),
        )
        .scroll((
            clamp_to_u16(app.detail_scroll),
            clamp_to_u16(app.detail_hscroll),
        ));

    frame.render_widget(paragraph, area);
}

fn draw_logs(frame: &mut Frame, app: &App, area: Rect) {
    let border_style = if app.focus == PaneFocus::Log {
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default()
    };

    let lines: Vec<Line> = app.logs.iter().map(|line| styled_log_line(line)).collect();
    let scroll = log_scroll_offset(lines.len(), area.height, app.log_tail_offset);
    let title = format!(
        "{} Log · {}{} ",
        focus_marker(app.focus == PaneFocus::Log),
        app.logs.len(),
        if app.log_tail_offset == 0 {
            " · follow"
        } else {
            " · paused"
        }
    );

    let paragraph = Paragraph::new(lines)
        .block(
            Block::default()
                .title(clip_title(&title, area.width.saturating_sub(4) as usize))
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(border_style),
        )
        .scroll((scroll, clamp_to_u16(app.log_hscroll)));

    frame.render_widget(paragraph, area);
}

fn styled_log_line(line: &str) -> Line<'static> {
    let sanitized = sanitize_terminal_text(line);
    let trimmed = sanitized.trim_start();
    let style = if trimmed.starts_with("hint:") {
        Style::default().fg(Color::LightBlue)
    } else if trimmed.starts_with("error:")
        || trimmed.starts_with("Error:")
        || trimmed.starts_with("failed:")
        || trimmed.starts_with("stderr:")
        || trimmed.starts_with("ERR ")
    {
        Style::default().fg(Color::LightRed)
    } else if trimmed.starts_with("completed:")
        || trimmed.starts_with("ok:")
        || trimmed.starts_with("exit=0")
    {
        Style::default().fg(Color::LightGreen)
    } else if trimmed.starts_with("action:") || trimmed.starts_with("foreground:") {
        Style::default().fg(Color::LightCyan)
    } else {
        Style::default().fg(Color::Gray)
    };
    Line::styled(sanitized, style)
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

fn draw_header(frame: &mut Frame, app: &App, area: Rect) {
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Length(1)])
        .split(area);
    let width = area.width as usize;

    let identity = Line::from(vec![
        Span::styled(
            " chezmoi-tui ",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("v{}", env!("CARGO_PKG_VERSION")),
            Style::default().fg(Color::Gray),
        ),
    ]);
    let summary = Span::styled(
        compact_middle(&header_summary(app), width.saturating_sub(30)),
        Style::default().fg(Color::DarkGray),
    );
    let top = join_line_to_width(identity, summary, width);
    frame.render_widget(
        Paragraph::new(top).style(Style::default().fg(Color::Reset)),
        rows[0],
    );

    let tabs = Line::from(tab_spans(app, app.view, width));
    frame.render_widget(Paragraph::new(tabs), rows[1]);
}

fn tab_spans(app: &App, active: ListView, width: usize) -> Vec<Span<'static>> {
    let tabs = [
        ('1', ListView::Status, app.status_entries.len()),
        ('2', ListView::Managed, app.managed_entries.len()),
        ('3', ListView::Unmanaged, app.unmanaged_entries.len()),
        ('4', ListView::Source, app.source_entries.len()),
    ];
    let mut spans = Vec::new();
    for (index, (number, view, count)) in tabs.into_iter().enumerate() {
        if index > 0 {
            spans.push(Span::raw("  "));
        }
        let marker = if view == active { "▶ " } else { "  " };
        let text = format!("{marker}{number} {} {count}", view.title());
        let style = if view == active {
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::Gray)
        };
        spans.push(Span::styled(text, style));
    }
    clip_spans_to_width(spans, width)
}

fn header_summary(app: &App) -> String {
    if let Some(notice) = display_notice(app)
        && notice.tone == NoticeTone::Error
    {
        return format!("ERR {}", notice.message);
    }
    if app.is_busy() {
        return app
            .busy_message()
            .map_or_else(|| "busy".to_string(), |message| format!("busy · {message}"));
    }

    let len = app.current_len();
    let position = if len == 0 {
        0
    } else {
        app.selected_index.min(len - 1) + 1
    };
    let mut parts = vec![format!("{position}/{len}")];
    if let Some(path) = app.selected_path() {
        parts.push(path.display().to_string());
    } else {
        parts.push(app.view_context_text());
    }
    if app.marked_count() > 0 {
        parts.push(format!("{} marked", app.marked_count()));
    }
    if !app.list_filter().trim().is_empty() {
        parts.push(format!("/{}", app.list_filter()));
    }
    parts.join(" · ")
}

fn display_notice(app: &App) -> Option<&crate::app::Notice> {
    app.latest_notice()
        .filter(|notice| notice.tone == NoticeTone::Error || app.config.show_notices)
}

fn filter_title_suffix(app: &App) -> String {
    if app.list_filter().trim().is_empty() {
        String::new()
    } else {
        format!(" /{}", compact_label(app.list_filter(), 18))
    }
}

fn empty_list_message(app: &App) -> String {
    if app.is_busy() {
        return format!("Loading {}…", app.view.title().to_lowercase());
    }
    if !app.list_filter().trim().is_empty() {
        return format!(
            "No matches for /{}. Esc clears the filter.",
            app.list_filter()
        );
    }
    match app.view {
        ListView::Status => "No pending changes.".to_string(),
        ListView::Managed => "No managed files loaded. Press r to refresh.".to_string(),
        ListView::Unmanaged => "No unmanaged files in the current directory.".to_string(),
        ListView::Source => "No source files loaded. Press r to refresh.".to_string(),
    }
}

fn empty_detail_lines(app: &App) -> Option<Vec<Line<'static>>> {
    if !app.detail_text.trim().is_empty() {
        return None;
    }

    let muted = Style::default().fg(Color::DarkGray);
    let lines = if app.is_busy() {
        vec![
            Line::styled("Loading detail…", Style::default().fg(Color::Cyan)),
            Line::styled("The selected item is being prepared.", muted),
        ]
    } else if app.selected_is_directory() {
        vec![
            Line::styled(
                "Directory selected.",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Line::styled("Enter expands it; h/l folds and unfolds.", muted),
        ]
    } else if app.current_len() == 0 {
        vec![Line::styled(empty_list_message(app), muted)]
    } else {
        vec![
            Line::styled(
                "No detail loaded for this selection.",
                Style::default().fg(Color::Gray),
            ),
            Line::styled("d: diff   v: preview   /: filter list", muted),
        ]
    };
    Some(lines)
}

fn draw_tiny(frame: &mut Frame, app: &App, area: Rect) {
    let summary = if area.width < 24 {
        "chezmoi-tui"
    } else {
        "chezmoi-tui · terminal too small"
    };
    let mut lines = vec![
        Line::styled(
            clip_text(summary, area.width as usize),
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Line::styled(
            clip_text(
                &format!("{}x{} · {}", area.width, area.height, app.view.title()),
                area.width as usize,
            ),
            Style::default().fg(Color::DarkGray),
        ),
    ];
    if let Some(notice) = display_notice(app) {
        lines.push(Line::styled(
            clip_text(
                &format!("{} {}", notice_tone_label(notice.tone), notice.message),
                area.width as usize,
            ),
            notice_style(notice.tone),
        ));
    }
    frame.render_widget(Paragraph::new(lines), area);
}

fn apply_no_color(frame: &mut Frame) {
    if std::env::var_os("NO_COLOR").is_some() {
        clear_buffer_styles(frame.buffer_mut());
    }
}

fn ensure_dark_canvas(buffer: &mut ratatui::buffer::Buffer) {
    for cell in &mut buffer.content {
        if cell.fg == Color::Reset {
            cell.fg = Color::Gray;
        }
        if cell.bg == Color::Reset {
            cell.bg = Color::Black;
        }
    }
}

fn clear_buffer_styles(buffer: &mut ratatui::buffer::Buffer) {
    let area = *buffer.area();
    buffer.set_style(area, Style::reset());
}

fn focus_marker(focused: bool) -> &'static str {
    if focused { "▶" } else { " " }
}

fn selection_style() -> Style {
    Style::default()
        .fg(Color::White)
        .bg(SELECTION_BG)
        .add_modifier(Modifier::BOLD)
}

fn clip_title(value: &str, width: usize) -> String {
    compact_middle(value, width)
}

fn join_line_to_width(left: Line<'static>, right: Span<'static>, width: usize) -> Line<'static> {
    let right_width = right.width();
    let left_width = left.width();
    let mut spans = left.spans;
    if left_width + right_width < width {
        spans.push(Span::raw(" ".repeat(width - left_width - right_width)));
        spans.push(right);
    } else {
        spans.push(Span::raw(" "));
        spans.push(right);
    }
    Line::from(clip_spans_to_width(spans, width))
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

struct FooterBar;

const SELECTION_BG: Color = Color::Rgb(42, 78, 102);
const KEYCAP_BG: Color = Color::Rgb(32, 43, 51);
const TARGET_HINT_COUNT: usize = 7;
const TRUNCATED_HINT_LABEL_WIDTH: usize = 6;

impl FooterBar {
    fn draw(frame: &mut Frame, app: &App, area: Rect) {
        if area.height == 0 {
            return;
        }

        let rows = if area.height >= 2 {
            Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Length(1),
                    Constraint::Length(1),
                    Constraint::Length(area.height.saturating_sub(2)),
                ])
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
        if rows.len() > 2 && rows[2].height > 0 && app.footer_help {
            draw_footer_line(frame, rows[2], footer_help_sheet());
        }
    }

    fn draw_main_row(frame: &mut Frame, app: &App, area: Rect) {
        if area.width == 0 || area.height == 0 {
            return;
        }

        let (left, _) = footer_left(app, area.width as usize);
        draw_footer_line(frame, area, Line::from(left));
    }

    fn draw_cheat_row(frame: &mut Frame, app: &App, area: Rect) {
        let hints = layout_hints(area.width as usize, footer_hints(app));
        let (spans, _) = render_hints(&hints);
        draw_footer_line(frame, area, Line::from(spans));
    }
}

fn footer_help_sheet() -> Line<'static> {
    Line::from(vec![
        Span::styled("More: ", Style::default().fg(Color::Cyan)),
        Span::styled(
            "m maximize   e edit   ! notices   n/N next   H/L horizontal   C-u/d half-page",
            Style::default().fg(Color::DarkGray),
        ),
    ])
}

fn draw_footer_line(frame: &mut Frame, area: Rect, line: Line<'static>) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let clipped = Line::from(clip_spans_to_width(line.spans, area.width as usize));
    frame.render_widget(Paragraph::new(clipped), area);
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
            .bg(SELECTION_BG)
            .fg(Color::White)
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
            style: Style::default().fg(Color::LightCyan),
            essential: false,
            badge: false,
        });
    }

    if let Some(notice) = display_notice(app) {
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
            style: Style::default().fg(Color::LightCyan),
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
            style: Style::default().fg(Color::LightCyan),
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

    hints.extend(primary_global_hints());

    hints
}

fn list_focus_hints(app: &App) -> Vec<Hint> {
    let enter_label = if app.selected_is_directory() {
        "expand"
    } else {
        "open"
    };
    vec![
        hint(
            "Enter",
            enter_label,
            Some("list"),
            96,
            HintTone::Secondary,
            true,
            false,
        ),
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
            true,
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
            true,
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

fn primary_global_hints() -> [Hint; 3] {
    [
        hint(
            "a",
            "Actions (C-p)",
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
            .bg(SELECTION_BG)
            .fg(Color::White)
            .add_modifier(Modifier::BOLD),
        HintTone::Secondary => Style::default().bg(KEYCAP_BG).fg(Color::Gray),
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

        let mut truncated = String::new();
        let mut truncated_width = 0usize;
        for ch in content.chars() {
            let char_width = Span::raw(ch.to_string()).width();
            if truncated_width + char_width > remain {
                break;
            }
            truncated.push(ch);
            truncated_width += char_width;
        }
        if !truncated.is_empty() {
            clipped.push(Span::styled(truncated, span.style));
        }
        break;
    }

    clipped
}

fn text_width(text: &str) -> usize {
    Span::raw(text).width()
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
    if text_width(value) <= max_chars {
        return value.to_string();
    }
    if max_chars == 0 {
        return String::new();
    }
    let keep = max_chars.saturating_sub(Span::raw("…").width());
    format!("{}…", clip_text(value, keep))
}

fn compact_middle(value: &str, max_width: usize) -> String {
    if text_width(value) <= max_width {
        return value.to_string();
    }
    if max_width <= Span::raw("…").width() {
        return clip_text("…", max_width);
    }

    let ellipsis_width = Span::raw("…").width();
    let keep = max_width.saturating_sub(ellipsis_width);
    let left = keep / 2;
    let right = keep.saturating_sub(left);
    format!("{}…{}", clip_text(value, left), tail_text(value, right))
}

fn compact_path_row(value: &str, max_width: usize) -> String {
    if text_width(value) <= max_width {
        return value.to_string();
    }
    if max_width <= Span::raw("…").width() + 1 {
        return tail_text(value, max_width);
    }

    let prefix_width = max_width.min(4);
    let prefix = clip_text(value, prefix_width);
    let prefix_width = text_width(&prefix);
    let remaining = max_width
        .saturating_sub(prefix_width)
        .saturating_sub(Span::raw("…").width());
    format!("{prefix}…{}", tail_text(value, remaining))
}

fn clip_text(value: &str, max_width: usize) -> String {
    let mut out = String::new();
    let mut used = 0usize;
    for ch in value.chars() {
        let width = Span::raw(ch.to_string()).width();
        if used + width > max_width {
            break;
        }
        out.push(ch);
        used += width;
    }
    out
}

fn tail_text(value: &str, max_width: usize) -> String {
    let mut chars = Vec::new();
    let mut used = 0usize;
    for ch in value.chars().rev() {
        let width = Span::raw(ch.to_string()).width();
        if used + width > max_width {
            break;
        }
        chars.push(ch);
        used += width;
    }
    chars.into_iter().rev().collect()
}

fn sanitize_terminal_text(value: &str) -> String {
    let mut out = String::new();
    let mut chars = value.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\u{1b}' {
            match chars.peek().copied() {
                Some('[') => {
                    chars.next();
                    for next in chars.by_ref() {
                        if ('@'..='~').contains(&next) {
                            break;
                        }
                    }
                }
                Some(']') => {
                    chars.next();
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
                Some(_) => {
                    chars.next();
                }
                None => {}
            }
        } else if ch == '\t' || !ch.is_control() {
            out.push(ch);
        }
    }
    out
}

fn draw_modal(frame: &mut Frame, app: &App) {
    match &app.modal {
        ModalState::None => {}
        ModalState::ListFilter { value, .. } => {
            let area = modal_rect(frame.area(), 76, 12);
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
                    Span::styled(shown, Style::default().fg(Color::LightCyan)),
                ]),
                Line::from(""),
                Line::from("Enter: apply and close  Esc: cancel  Backspace: delete"),
            ];

            let p = Paragraph::new(lines)
                .block(
                    Block::default()
                        .title(" List Filter ")
                        .borders(Borders::ALL)
                        .border_type(BorderType::Rounded)
                        .border_style(Style::default().fg(Color::LightBlue)),
                )
                .wrap(Wrap { trim: false });
            frame.render_widget(p, area);
            set_input_cursor(frame, area, 2, 7, value);
        }
        ModalState::Ignore {
            requests,
            selected,
            global_only,
        } => {
            let area = modal_rect(frame.area(), 76, 20);
            frame.render_widget(Clear, area);

            let raw_target = requests
                .first()
                .and_then(|request| request.targets.first())
                .map_or_else(|| "(none)".to_string(), |path| path.display().to_string());
            let target_text = compact_middle(&raw_target, area.width.saturating_sub(30) as usize);
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
                Line::from(if *global_only {
                    "scope: outside destination; global-by-name only"
                } else {
                    "scope: home-relative + global-by-name"
                }),
                Line::from(""),
                Line::from("Select ignore rule mode:"),
            ];

            for (index, (label, description)) in options.into_iter().enumerate() {
                let disabled = *global_only && index != 4;
                let prefix = if index == *selected { "▶" } else { " " };
                lines.push(Line::from(vec![
                    Span::styled(
                        format!("{prefix} {label}"),
                        if disabled {
                            Style::default().fg(Color::DarkGray)
                        } else if index == *selected {
                            selection_style()
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
                ignore_pattern_preview(&raw_target, *selected)
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
                        .border_type(BorderType::Rounded)
                        .border_style(Style::default().fg(Color::LightBlue)),
                )
                .wrap(Wrap { trim: true });
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
            let area = modal_rect(frame.area(), 76, 20);
            frame.render_widget(Clear, area);

            let target_text = requests
                .first()
                .and_then(|request| request.targets.first())
                .map_or_else(|| "(none)".to_string(), |path| path.display().to_string());
            let target_text = compact_middle(&target_text, area.width.saturating_sub(30) as usize);
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
                            selection_style()
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
                        .border_type(BorderType::Rounded)
                        .border_style(Style::default().fg(Color::LightBlue)),
                )
                .wrap(Wrap { trim: true });
            frame.render_widget(p, area);
        }
        ModalState::ActionMenu { selected, filter } => {
            let area = modal_rect(frame.area(), 76, 22);
            frame.render_widget(Clear, area);
            let sections = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Length(4), Constraint::Min(1)])
                .split(area);

            let query = if filter.is_empty() {
                "<type to filter>".to_string()
            } else {
                filter.clone()
            };
            let query_style = if filter.is_empty() {
                Style::default().fg(Color::DarkGray)
            } else {
                Style::default().fg(Color::LightCyan)
            };
            let filter_widget = Paragraph::new(vec![
                Line::from(vec![
                    Span::styled("query: ", Style::default().fg(Color::Gray)),
                    Span::styled(query, query_style),
                ]),
                Line::from("Backspace del  danger:<name> reveal  Enter run  Esc close"),
            ])
            .block(
                Block::default()
                    .title(" Action Filter ")
                    .borders(Borders::ALL)
                    .border_type(BorderType::Rounded)
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
                        .border_type(BorderType::Rounded)
                        .border_style(Style::default().fg(Color::Cyan)),
                )
                .highlight_style(selection_style())
                .highlight_symbol("▶ ");

            let mut state = ListState::default();
            if !indices.is_empty() {
                let action_index = (*selected).min(indices.len().saturating_sub(1));
                let row_index = selectable.get(action_index).copied().unwrap_or(0);
                state.select(Some(row_index));
            }
            frame.render_stateful_widget(list, sections[1], &mut state);
            set_input_cursor(frame, sections[0], 0, 7, filter);
        }
        ModalState::ActionPreflight { requests, scroll } => {
            let area = modal_rect(frame.area(), 80, 22);
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
                    requests
                        .iter()
                        .map(|request| request.targets.len())
                        .sum::<usize>()
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
                    if request.targets.is_empty() {
                        lines.push(Line::from("  (none)"));
                    } else {
                        lines.extend(
                            request
                                .targets
                                .iter()
                                .map(|path| Line::from(format!("  {}", path.display()))),
                        );
                    }
                }
            }
            lines.push(Line::from(""));
            lines.push(Line::from(
                "Enter: continue  Esc: cancel  j/k/PgUp/PgDn: scroll",
            ));

            let max_scroll = modal_max_scroll(&lines, area);
            let p = Paragraph::new(lines)
                .block(
                    Block::default()
                        .title(" Action Preflight ")
                        .borders(Borders::ALL)
                        .border_type(BorderType::Rounded)
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
            let area = modal_rect(frame.area(), 76, 22);
            frame.render_widget(Clear, area);
            let title = match step {
                ConfirmStep::Primary => " Confirm Action ",
                ConfirmStep::DangerPhrase => " Dangerous Action ",
            };

            let mut lines = vec![
                Line::from(format!("action: {}", request.action.label())),
                Line::from(format!(
                    "target: {}",
                    compact_middle(
                        &request_targets_text(request),
                        area.width.saturating_sub(10) as usize
                    )
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
            let mut input_line = None;
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
                            Line::from(format!(
                                "required: {}",
                                compact_middle(&phrase, area.width.saturating_sub(18) as usize)
                            ))
                            .style(Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)),
                        );
                    }
                    lines.push(
                        Line::from(format!(
                            "input: {}",
                            compact_middle(typed, area.width.saturating_sub(15) as usize)
                        ))
                        .style(Style::default().fg(Color::Yellow)),
                    );
                    input_line = Some(lines.len().saturating_sub(1));
                }
            }

            let p = Paragraph::new(lines)
                .block(
                    Block::default()
                        .title(title)
                        .borders(Borders::ALL)
                        .border_type(BorderType::Rounded)
                        .border_style(Style::default().fg(Color::LightRed)),
                )
                .wrap(Wrap { trim: true });

            frame.render_widget(p, area);
            if let Some(line) = input_line {
                set_input_cursor(frame, area, clamp_to_u16(line), 7, typed);
            }
        }
        ModalState::ConfirmApply { request, snapshot } => {
            let area = modal_rect(frame.area(), 80, 22);
            frame.render_widget(Clear, area);
            let plan = &snapshot.plan;
            let has_directory = apply_has_recursive_scope(&request.targets, plan, &app.home_dir);
            let mut lines = vec![
                Line::from(apply_scope_summary(&request.targets)),
                Line::from(apply_plan_count_summary(plan)),
                Line::from(apply_plan_secondary_summary(plan)),
            ];

            if request.targets.is_empty() {
                lines.push(Line::from(
                    "target roots: ALL pending changes (no target filter)",
                ));
            } else if has_directory {
                lines.push(Line::from(
                    "directory roots include every affected descendant recursively",
                ));
            }

            let preview_count = request.targets.len().min(3);
            if preview_count > 0 {
                lines.push(Line::from(""));
                lines.push(Line::from(format!(
                    "target preview ({preview_count} of {}):",
                    request.targets.len()
                )));
            }
            for target in request.targets.iter().take(preview_count) {
                lines.push(Line::from(format!(
                    "  {}",
                    compact_middle(
                        &target.display().to_string(),
                        area.width.saturating_sub(8) as usize
                    )
                )));
            }
            if request.targets.len() > preview_count {
                lines.push(Line::from(format!(
                    "  ... {} more targets - reviewed in the full Apply Plan",
                    request.targets.len() - preview_count
                )));
            }
            push_apply_warnings(&mut lines, plan);
            lines.push(Line::from(
                "Full roots and affected paths were shown in Apply Plan.",
            ));
            lines.push(Line::from(""));
            lines.push(Line::from("Enter: validate and apply  Esc: cancel"));

            let p = Paragraph::new(lines)
                .block(
                    Block::default()
                        .title(" Confirm Apply ")
                        .borders(Borders::ALL)
                        .border_type(BorderType::Rounded)
                        .border_style(Style::default().fg(Color::LightYellow)),
                )
                .wrap(Wrap { trim: true });
            frame.render_widget(p, area);
        }
        ModalState::ApplyPlan {
            request,
            snapshot,
            scroll,
        } => {
            let area = modal_rect(frame.area(), 80, 22);
            frame.render_widget(Clear, area);

            let plan = &snapshot.plan;
            let has_directory = apply_has_recursive_scope(&request.targets, plan, &app.home_dir);

            let mut lines = vec![
                Line::from(format!("action: {}", request.action.label())),
                Line::from(apply_scope_summary(&request.targets)),
                Line::from(apply_plan_count_summary(plan)),
                Line::from(apply_plan_secondary_summary(plan)),
                Line::from(""),
                Line::from("This plan was generated from a fresh chezmoi status and diff."),
                Line::from("It will be validated again before apply."),
            ];

            if request.targets.is_empty() {
                lines.push(Line::from(
                    "target roots: ALL pending changes (no target filter)",
                ));
            } else {
                lines.push(Line::from(format!(
                    "target roots: {} selected",
                    request.targets.len()
                )));
                if has_directory {
                    lines.push(Line::from(
                        "directory roots include every affected descendant recursively",
                    ));
                }
                lines.extend(
                    request
                        .targets
                        .iter()
                        .map(|target| Line::from(format!("  {}", target.display()))),
                );
            }

            if plan.total() == 0 {
                lines.push(Line::from(""));
                lines.push(Line::from("No pending status entries were loaded."));
            }
            push_apply_warnings(&mut lines, plan);
            lines.push(Line::from(""));
            lines.push(Line::from("affected paths:"));

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

            let max_scroll = modal_max_scroll(&lines, area);
            let p = Paragraph::new(lines)
                .block(
                    Block::default()
                        .title(" Apply Plan ")
                        .borders(Borders::ALL)
                        .border_type(BorderType::Rounded)
                        .border_style(Style::default().fg(Color::LightBlue)),
                )
                .scroll((clamp_to_u16((*scroll).min(max_scroll)), 0))
                .wrap(Wrap { trim: false });
            frame.render_widget(p, area);
        }
        ModalState::Help { scroll } => {
            let area = modal_rect(frame.area(), 80, 22);
            frame.render_widget(Clear, area);
            let lines = help_lines();
            let max_scroll = modal_max_scroll(&lines, area);
            let p = Paragraph::new(lines)
                .block(
                    Block::default()
                        .title(" Help / Legends ")
                        .borders(Borders::ALL)
                        .border_type(BorderType::Rounded)
                        .border_style(Style::default().fg(Color::LightBlue)),
                )
                .scroll((clamp_to_u16((*scroll).min(max_scroll)), 0))
                .wrap(Wrap { trim: false });
            frame.render_widget(p, area);
        }
        ModalState::NoticeHistory { scroll } => {
            let area = modal_rect(frame.area(), 80, 22);
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
            let max_scroll = modal_max_scroll(&lines, area);
            let p = Paragraph::new(lines)
                .block(
                    Block::default()
                        .title(" Notice History ")
                        .borders(Borders::ALL)
                        .border_type(BorderType::Rounded)
                        .border_style(Style::default().fg(Color::LightBlue)),
                )
                .scroll((clamp_to_u16((*scroll).min(max_scroll)), 0))
                .wrap(Wrap { trim: false });
            frame.render_widget(p, area);
        }
        ModalState::Search { scope, value } => {
            let area = modal_rect(frame.area(), 76, 12);
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
                    Span::styled(shown.to_string(), Style::default().fg(Color::LightCyan)),
                ]),
                Line::from(""),
                Line::from("Enter: search/jump  Esc: cancel  Backspace: delete"),
            ];
            let p = Paragraph::new(lines)
                .block(
                    Block::default()
                        .title(" Pane Search ")
                        .borders(Borders::ALL)
                        .border_type(BorderType::Rounded)
                        .border_style(Style::default().fg(Color::LightBlue)),
                )
                .wrap(Wrap { trim: false });
            frame.render_widget(p, area);
            set_input_cursor(frame, area, 2, 7, value);
        }
        ModalState::Input {
            kind,
            request,
            value,
        } => {
            let area = modal_rect(frame.area(), 76, 12);
            frame.render_widget(Clear, area);

            let prompt = match kind {
                InputKind::ChattrAttrs => "chattr attributes (e.g. private,template)",
            };

            let lines = vec![
                Line::from(format!("action: {}", request.action.label())),
                Line::from(format!(
                    "target: {}",
                    compact_middle(
                        &request_targets_text(request),
                        area.width.saturating_sub(10) as usize
                    )
                )),
                Line::from(""),
                Line::from(prompt),
                Line::from(format!("> {value}")).style(Style::default().fg(Color::LightCyan)),
                Line::from("Enter: Confirm  Esc: Cancel  Ctrl+u: clear"),
            ];

            let p = Paragraph::new(lines)
                .block(
                    Block::default()
                        .title(" Input ")
                        .borders(Borders::ALL)
                        .border_type(BorderType::Rounded)
                        .border_style(Style::default().fg(Color::LightBlue)),
                )
                .wrap(Wrap { trim: false });
            frame.render_widget(p, area);
            set_input_cursor(frame, area, 4, 2, value);
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ActionMenuSection {
    Group(ActionGroup),
    Unavailable,
}

impl ActionMenuSection {
    fn title(self) -> &'static str {
        match self {
            Self::Group(group) => group.title(),
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

fn build_action_menu_rows(actions: &[Action]) -> Vec<ActionMenuRow> {
    let mut rows = Vec::new();
    for group in ActionGroup::ALL {
        let grouped = actions
            .iter()
            .copied()
            .filter(|action| action.group() == group)
            .collect::<Vec<_>>();
        if grouped.is_empty() {
            continue;
        }
        if !rows.is_empty() {
            rows.push(ActionMenuRow::Spacer);
        }
        rows.push(ActionMenuRow::Header(ActionMenuSection::Group(group)));
        rows.extend(grouped.into_iter().map(ActionMenuRow::Action));
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
            "  {:<20} !! {} [danger]",
            action.label(),
            action.description()
        )
    } else {
        format!("  {:<20}    {}", action.label(), action.description())
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
        Line::from("     Source lists raw source files; use cat for rendered target content"),
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
        Line::from("  Normal layout keeps Log in a 4-row drawer; focused Log uses detail_ratio."),
        Line::from("  footer_help adds the More key sheet below the persistent footer rows."),
        Line::from("  show_notices suppresses info/success text; errors always remain visible."),
        Line::from("  Source attributes: {tmpl,priv,exec,enc} when encoded in source names."),
        Line::from("  external-diff opens chezmoi diff in tools.external_diff (default delta)."),
        Line::from("  debug-context shows current view/selection/config summary."),
        Line::from("  ! opens notice history."),
        Line::from(""),
        Line::from("Keys"),
        Line::from("  Tab / Back Tab: next / previous pane"),
        Line::from("  j/k: move or scroll   Home/End or g/G: list top/bottom"),
        Line::from("  PgUp/PgDn: page   Enter: expand directory   /: filter or search"),
        Line::from("  Esc: close modal or clear the active list filter"),
        Line::from("  Ctrl+P: action palette   a: actions   r: refresh   q: quit"),
        Line::from("  This help scrolls with j/k, PgUp/PgDn, Home/End."),
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
        Action::ExternalDiff => "opens full diff in configured external diff tool",
        Action::DebugContext => "shows current app context without changing files",
        Action::Update => "updates source then applies changes",
        Action::MergeAll => "runs merge workflow for all changes",
        Action::Add => "imports selected files into chezmoi source state",
        Action::AddDirectory => "imports the entire selected directory tree recursively",
        Action::Ignore => "appends ignore rules to .chezmoiignore",
        Action::ReAdd => "re-imports selected modified files",
        Action::ReAddAll => "re-imports every eligible modified file",
        Action::Forget => "removes selected targets from chezmoi state",
        Action::Chattr => "changes source attributes for selected targets",
        Action::GitPull => "updates the source repository without applying changes",
        Action::GitPush => "pushes existing source repository commits",
        Action::GitCommit => "commits only changes already staged in the source repository",
        Action::Init => "initializes chezmoi source state for this destination",
        Action::Destroy => "deletes selected target from source/destination/state",
        Action::Purge => "removes chezmoi config and data for this environment",
        _ => action.description(),
    }
}

fn action_preflight_mode(action: Action) -> &'static str {
    match action {
        Action::DebugContext => "internal detail view",
        Action::Ignore => "internal file update",
        _ if action.runs_foreground() => "foreground command",
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
                    .map(|arg| quote_preview_arg(&arg.to_string_lossy()))
                    .collect::<Vec<_>>()
                    .join(" ");
                format!("chezmoi {args}")
            }
            Err(_) if request.action == Action::Ignore => {
                let mode = request.chattr_attrs.as_deref().unwrap_or("auto");
                format!("internal ignore rule update ({mode})")
            }
            Err(_) if request.action == Action::DebugContext => {
                "internal debug-context detail".to_string()
            }
            Err(_) if request.action == Action::OpenSourceDir => {
                "interactive shell: source directory".to_string()
            }
            Err(_) if request.action == Action::ExternalDiff => {
                "external diff tool: chezmoi diff".to_string()
            }
            Err(_) if request.action == Action::EditIgnore => {
                "external editor: .chezmoiignore".to_string()
            }
            Err(_) if request.action == Action::EditSource => format!(
                "external editor: {}",
                request.targets.first().map_or_else(
                    || "(selected source file)".to_string(),
                    |path| quote_preview_arg(&path.display().to_string())
                )
            ),
            Err(_) => format!("internal/foreground action: {}", request.action.label()),
        })
        .collect()
}

fn quote_preview_arg(value: &str) -> String {
    if !value.is_empty()
        && value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || "_./:=@+-".contains(ch))
    {
        return value.to_string();
    }
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn request_targets_text(request: &ActionRequest) -> String {
    match request.targets.as_slice() {
        [] => "(none)".to_string(),
        [target] => target.display().to_string(),
        [first, rest @ ..] => format!("{} (+{} more)", first.display(), rest.len()),
    }
}

fn apply_has_recursive_scope(
    targets: &[std::path::PathBuf],
    plan: &crate::app::ApplyPlan,
    home_dir: &Path,
) -> bool {
    targets.iter().any(|target| {
        let target = if target.is_absolute() {
            target.clone()
        } else {
            home_dir.join(target)
        };
        target.is_dir()
            || plan
                .added
                .iter()
                .chain(&plan.modified)
                .chain(&plan.deleted)
                .chain(&plan.run)
                .chain(&plan.unknown)
                .any(|path| {
                    let path = if path.is_absolute() {
                        path.clone()
                    } else {
                        home_dir.join(path)
                    };
                    path != target && path.starts_with(&target)
                })
    })
}

fn apply_scope_summary(targets: &[std::path::PathBuf]) -> String {
    if targets.is_empty() {
        "scope: ALL pending changes - no target filter".to_string()
    } else {
        format!("scope: selected targets - {} target root(s)", targets.len())
    }
}

fn apply_plan_count_summary(plan: &crate::app::ApplyPlan) -> String {
    format!(
        "changes: {} total | Added {} | Modified {} | Deleted {}",
        plan.total(),
        plan.added.len(),
        plan.modified.len(),
        plan.deleted.len()
    )
}

fn apply_plan_secondary_summary(plan: &crate::app::ApplyPlan) -> String {
    format!(
        "plan groups: scripts {} | unknown {}",
        plan.run.len(),
        plan.unknown.len()
    )
}

fn push_apply_warnings(lines: &mut Vec<Line<'static>>, plan: &crate::app::ApplyPlan) {
    if plan.run.is_empty() && plan.deleted.is_empty() && plan.unknown.is_empty() {
        return;
    }

    lines.push(Line::from(""));
    if !plan.run.is_empty() {
        lines.push(
            Line::from(format!(
                "Warning: apply may run {} script(s).",
                plan.run.len()
            ))
            .style(Style::default().fg(Color::LightYellow)),
        );
    }
    if !plan.deleted.is_empty() {
        lines.push(
            Line::from(format!(
                "Warning: {} path(s) will be deleted.",
                plan.deleted.len()
            ))
            .style(Style::default().fg(Color::LightRed)),
        );
    }
    if !plan.unknown.is_empty() {
        lines.push(
            Line::from("Warning: unknown status entries need manual review.")
                .style(Style::default().fg(Color::LightYellow)),
        );
    }
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
            format!("  {:<20} -- disabled: {}", action.label(), reason),
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
    let sanitized = sanitize_terminal_text(line);
    let (code, comment) = split_comment(&sanitized, language);
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
            spans.push(Span::styled(token, Style::default().fg(Color::LightCyan)));
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

fn modal_rect(area: Rect, preferred_width: u16, preferred_height: u16) -> Rect {
    let max_width = area.width.saturating_sub(2);
    let max_height = area.height.saturating_sub(2);
    let width = preferred_width.min(max_width).max(max_width.min(1));
    let height = preferred_height.min(max_height).max(max_height.min(1));
    let x = area.x + area.width.saturating_sub(width) / 2;
    let y = area.y + area.height.saturating_sub(height) / 2;
    Rect::new(x, y, width, height)
}

fn modal_max_scroll(lines: &[Line<'static>], area: Rect) -> usize {
    let width = area.width.saturating_sub(2).max(1);
    let rendered_lines = lines
        .iter()
        .map(|line| wrapped_line_count(line, width as usize))
        .sum::<usize>();
    rendered_lines.saturating_sub(area.height.saturating_sub(2) as usize)
}

fn wrapped_line_count(line: &Line<'static>, width: usize) -> usize {
    let text = line
        .spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect::<String>();
    if text.is_empty() {
        return 1;
    }

    let mut rows = 1usize;
    let mut used = 0usize;
    for word in text.split_whitespace() {
        let word_width = Span::raw(word).width();
        if word_width > width {
            if used > 0 {
                rows += 1;
            }
            rows += word_width.div_ceil(width).saturating_sub(1);
            used = word_width % width;
            if used == 0 {
                used = width;
            }
            continue;
        }

        let gap = usize::from(used > 0);
        if used > 0 && used + gap + word_width > width {
            rows += 1;
            used = word_width;
        } else {
            used += gap + word_width;
        }
    }
    rows
}

fn set_input_cursor(
    frame: &mut Frame,
    area: Rect,
    line_index: u16,
    prefix_width: usize,
    value: &str,
) {
    if area.width <= 2 || area.height <= 2 {
        return;
    }
    let y = area
        .y
        .saturating_add(1)
        .saturating_add(line_index)
        .min(area.bottom().saturating_sub(2));
    let x = area
        .x
        .saturating_add(1)
        .saturating_add(clamp_to_u16(prefix_width.saturating_add(text_width(value))))
        .min(area.right().saturating_sub(2));
    frame.set_cursor_position((x, y));
}

#[cfg(test)]
mod tests {
    use super::{
        ActionMenuRow, ActionMenuSection, SELECTION_BG, action_menu_rows, action_menu_text,
        action_preflight_commands, action_preflight_impact, action_preflight_mode,
        build_action_menu_rows, clear_buffer_styles, draw, draw_styled, ensure_dark_canvas,
        log_scroll_offset, modal_rect,
    };
    use crate::app::{
        App, ApplyPlan, ApplyPlanSnapshot, ConfirmStep, InputKind, ModalState, PaneFocus,
    };
    use crate::config::AppConfig;
    use crate::domain::{Action, ActionRequest, ChangeKind, ListView, StatusEntry};
    use crate::ui_diff::colorized_diff_lines;
    use ratatui::Terminal;
    use ratatui::backend::{Backend, TestBackend};
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect;
    use ratatui::style::{Color, Modifier};
    use ratatui::text::Line;
    use std::path::PathBuf;

    fn render_line_text(line: &Line<'_>) -> String {
        line.spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect::<String>()
    }

    fn render_text(app: &mut App, width: u16, height: u16) -> String {
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).expect("test terminal");
        terminal
            .draw(|frame| draw(frame, app))
            .expect("draw succeeds");
        buffer_text(terminal.backend().buffer())
    }

    fn render_buffer(app: &mut App, width: u16, height: u16) -> Buffer {
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).expect("test terminal");
        terminal
            .draw(|frame| draw_styled(frame, app))
            .expect("draw succeeds");
        terminal.backend().buffer().clone()
    }

    fn has_quiet_selection(buffer: &Buffer) -> bool {
        buffer.content().iter().any(|cell| {
            cell.bg == SELECTION_BG
                && cell.fg == Color::White
                && cell.modifier.contains(Modifier::BOLD)
        })
    }

    fn buffer_text(buffer: &Buffer) -> String {
        buffer_rect_text(buffer, *buffer.area())
    }

    fn buffer_rect_text(buffer: &Buffer, area: Rect) -> String {
        (area.top()..area.bottom())
            .map(|y| {
                let mut text = String::new();
                let mut hidden_cells = 0usize;
                for x in area.left()..area.right() {
                    if hidden_cells > 0 {
                        hidden_cells -= 1;
                        continue;
                    }
                    let symbol = buffer[(x, y)].symbol();
                    text.push_str(symbol);
                    hidden_cells = ratatui::text::Span::raw(symbol).width().saturating_sub(1);
                }
                text
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn apply_plan_content_text(app: &mut App, width: u16, height: u16) -> String {
        let buffer = render_buffer(app, width, height);
        let modal = modal_rect(*buffer.area(), 80, 22);
        let content = Rect::new(
            modal.x.saturating_add(1),
            modal.y.saturating_add(1),
            modal.width.saturating_sub(2),
            modal.height.saturating_sub(2),
        );
        buffer_rect_text(&buffer, content)
    }

    fn request(action: Action) -> ActionRequest {
        ActionRequest {
            action,
            targets: vec![PathBuf::from("日本語/設定/very-long-target-name")],
            chattr_attrs: None,
        }
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
        assert!(rendered[4].contains("-old"));
        assert!(rendered[4].starts_with("   1      │"));
        assert!(rendered[5].contains("+new"));
        assert!(rendered[5].starts_with("        1 │"));
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
    fn header_keeps_identity_tabs_and_actual_counts_visible() {
        let mut app = App::new(AppConfig::default());
        app.status_entries = vec![StatusEntry {
            path: PathBuf::from("changed"),
            actual_vs_state: ChangeKind::Modified,
            actual_vs_target: ChangeKind::Modified,
        }];
        app.managed_entries = vec![PathBuf::from("a"), PathBuf::from("b")];
        app.unmanaged_entries = vec![PathBuf::from("loose")];
        app.source_entries = vec![PathBuf::from("x"), PathBuf::from("y"), PathBuf::from("z")];

        let rendered = render_text(&mut app, 100, 24);

        assert!(rendered.contains("chezmoi-tui"));
        assert!(rendered.contains(&format!("v{}", env!("CARGO_PKG_VERSION"))));
        assert!(rendered.contains("▶ 1 Status 1"));
        assert!(rendered.contains("2 Managed 2"));
        assert!(rendered.contains("3 Unmanaged 1"));
        assert!(rendered.contains("4 Source 3"));
    }

    #[test]
    fn source_view_list_title_marks_raw_source_content() {
        let mut app = App::new(AppConfig::default());
        app.source_entries = vec![PathBuf::from("dot_file")];
        app.switch_view(ListView::Source);
        app.focus = PaneFocus::List;

        let rendered = render_text(&mut app, 80, 24);

        assert!(rendered.contains("Source (raw)"), "{rendered}");
        assert!(rendered.contains("4 Source 1"), "{rendered}");
    }

    #[test]
    fn narrow_layout_shows_only_focused_pane_and_footer() {
        let mut app = App::new(AppConfig::default());
        app.status_entries = vec![StatusEntry {
            path: PathBuf::from(".bashrc"),
            actual_vs_state: ChangeKind::Modified,
            actual_vs_target: ChangeKind::Modified,
        }];
        app.switch_view(ListView::Status);
        app.focus = PaneFocus::List;

        let rendered = render_text(&mut app, 64, 24);
        let footer = rendered.lines().rev().take(2).collect::<Vec<_>>();
        let key_row = footer[0].split_whitespace().collect::<Vec<_>>().join(" ");

        assert!(rendered.contains("▶ Status"));
        assert!(footer[1].contains("Status"), "{rendered}");
        assert!(key_row.contains("j/k Move"), "{rendered}");
        assert!(key_row.contains("a Actions"), "{rendered}");
        assert!(key_row.contains("? Help"), "{rendered}");
        assert!(key_row.contains("q Quit"), "{rendered}");
        assert_eq!(key_row.matches("/ Find").count(), 1, "{rendered}");
        assert!(!rendered.contains("▶ Detail"));

        let wider = render_text(&mut app, 100, 24);
        assert!(wider.contains("Enter  open"), "{wider}");
    }

    #[test]
    fn normal_and_wizard_selections_use_one_quiet_style() {
        let mut list = App::new(AppConfig::default());
        list.managed_entries = vec![PathBuf::from("alpha"), PathBuf::from("beta")];
        list.switch_view(ListView::Managed);
        assert!(has_quiet_selection(&render_buffer(&mut list, 64, 24)));

        let mut menu = App::new(AppConfig::default());
        menu.open_action_menu();
        assert!(has_quiet_selection(&render_buffer(&mut menu, 80, 24)));

        let mut wizard = App::new(AppConfig::default());
        wizard.modal = ModalState::Ignore {
            requests: vec![request(Action::Ignore)],
            selected: 0,
            global_only: false,
        };
        assert!(has_quiet_selection(&render_buffer(&mut wizard, 80, 24)));
    }

    #[test]
    fn tiny_terminal_degrades_without_panicking() {
        let mut app = App::new(AppConfig::default());
        let rendered = render_text(&mut app, 16, 4);
        assert!(rendered.contains("chezmoi-tui"));
    }

    #[test]
    fn long_unicode_path_preserves_the_visible_tail() {
        let mut app = App::new(AppConfig::default());
        app.managed_entries = vec![PathBuf::from("日本語のとても長い最終設定ファイル.txt")];
        app.switch_view(ListView::Managed);

        let rendered = render_text(&mut app, 64, 24);

        assert!(rendered.contains("最終設定ファイル.txt"), "{rendered}");
    }

    #[test]
    fn list_viewport_keeps_the_selected_row_visible() {
        let mut app = App::new(AppConfig::default());
        app.managed_entries = (0..40)
            .map(|index| PathBuf::from(format!("item-{index:02}")))
            .collect();
        app.switch_view(ListView::Managed);
        app.selected_index = 30;

        let rendered = render_text(&mut app, 64, 14);

        assert!(rendered.contains("item-30"));
        assert!(!rendered.contains("item-00"));
    }

    #[test]
    fn empty_states_distinguish_loading_and_no_filter_matches() {
        let mut app = App::new(AppConfig::default());
        app.begin_busy_task_with_message("refreshing");
        let rendered = render_text(&mut app, 64, 24);
        assert!(rendered.contains("Loading status"));
        assert!(rendered.contains("refreshing"));

        let mut app = App::new(AppConfig::default());
        app.apply_list_filter_immediately("missing".to_string());
        let rendered = render_text(&mut app, 64, 24);
        assert!(rendered.contains("No matches for /missing"));
        assert!(rendered.contains("Esc clears the filter"));
    }

    #[test]
    fn footer_help_adds_a_third_key_sheet() {
        let mut app = App::new(AppConfig::default());
        let without_help = render_text(&mut app, 80, 24);
        assert!(!without_help.contains("More:"));

        app.footer_help = true;
        let with_help = render_text(&mut app, 80, 24);
        assert!(with_help.contains("More:"));
    }

    #[test]
    fn errors_stay_visible_when_information_notices_are_disabled() {
        let mut app = App::new(AppConfig::default());
        app.config.show_notices = false;
        app.set_error_notice("critical apply failure");
        let rendered = render_text(&mut app, 80, 24);
        assert!(rendered.contains("critical apply failure"));

        let mut app = App::new(AppConfig::default());
        app.config.show_notices = false;
        app.set_info_notice("informational refresh complete");
        let rendered = render_text(&mut app, 80, 24);
        assert!(!rendered.contains("informational refresh complete"));

        let mut app = App::new(AppConfig::default());
        app.config.show_notices = false;
        app.set_error_notice("critical apply failure");
        let rendered = render_text(&mut app, 23, 4);
        let compact = rendered.split_whitespace().collect::<Vec<_>>().join(" ");
        assert!(compact.contains("ERR critical"), "{rendered}");
    }

    #[test]
    fn preflight_mode_uses_action_metadata() {
        for action in Action::ALL {
            let expected = match action {
                Action::DebugContext => "internal detail view",
                Action::Ignore => "internal file update",
                _ if action.runs_foreground() => "foreground command",
                _ => "background command",
            };
            assert_eq!(action_preflight_mode(action), expected);
        }
    }

    #[test]
    fn preflight_quotes_arguments_with_spaces() {
        let request = ActionRequest {
            action: Action::Cat,
            targets: vec![PathBuf::from("/tmp/設定 files/profile")],
            chattr_attrs: None,
        };
        let commands = action_preflight_commands(&[request], 1);
        assert_eq!(commands[0], "chezmoi cat -- '/tmp/設定 files/profile'");
    }

    #[test]
    fn preflight_calls_out_broad_action_scopes() {
        for action in [
            Action::AddDirectory,
            Action::ReAddAll,
            Action::Init,
            Action::GitPull,
            Action::GitPush,
            Action::GitCommit,
        ] {
            let impact = action_preflight_impact(action);
            assert!(
                impact.contains("entire")
                    || impact.contains("every")
                    || impact.contains("source")
                    || impact.contains("destination")
                    || impact.contains("only"),
                "{action:?}: {impact}"
            );
        }
    }

    #[test]
    fn compact_modals_keep_required_content_visible() {
        let cases = vec![
            (
                ModalState::Ignore {
                    requests: vec![request(Action::Ignore)],
                    selected: 4,
                    global_only: false,
                },
                vec!["preview pattern:", "Global by name"],
            ),
            (
                ModalState::AddOptions {
                    requests: vec![request(Action::Add)],
                    selected: 3,
                    template: true,
                    private: false,
                    executable: false,
                    encrypted: true,
                },
                vec!["attrs preview:", "encrypted"],
            ),
            (
                ModalState::ActionMenu {
                    selected: 0,
                    filter: String::new(),
                },
                vec!["danger:<name>", "Enter run", "Esc close"],
            ),
            (
                ModalState::ActionPreflight {
                    requests: vec![request(Action::OpenSourceDir)],
                    scroll: 0,
                },
                vec![
                    "foreground command",
                    "command preview:",
                    "very-long-target-name",
                ],
            ),
            (
                ModalState::Input {
                    kind: InputKind::ChattrAttrs,
                    request: request(Action::Chattr),
                    value: "private".to_string(),
                },
                vec!["chattr attributes", "Ctrl+u: clear"],
            ),
            (
                ModalState::Confirm {
                    request: request(Action::Purge),
                    step: ConfirmStep::DangerPhrase,
                    typed: "purge ALL".to_string(),
                },
                vec![
                    "Dangerous Action",
                    "required: PURGE ALL",
                    "input: purge ALL",
                ],
            ),
        ];

        for width in [64, 80] {
            for (modal, expected) in &cases {
                let mut app = App::new(AppConfig::default());
                app.modal = modal.clone();
                let rendered = render_text(&mut app, width, 24);
                for text in expected {
                    assert!(
                        rendered.contains(text),
                        "width={width}, modal={modal:?}, missing={text:?}\n{rendered}"
                    );
                }
            }
        }
    }

    #[test]
    fn confirm_apply_compacts_targets_but_keeps_safety_actions_visible() {
        let targets = (0..20)
            .map(|index| PathBuf::from(format!("/home/demo/日本語/target-{index:02}.txt")))
            .collect::<Vec<_>>();
        let plan = ApplyPlan {
            modified: targets.clone(),
            deleted: vec![PathBuf::from("/home/demo/old.txt")],
            run: vec![PathBuf::from("/home/demo/run_once.sh")],
            ..ApplyPlan::default()
        };
        let request = ActionRequest {
            action: Action::Apply,
            targets: targets.clone(),
            chattr_attrs: None,
        };
        let snapshot = ApplyPlanSnapshot {
            targets,
            plan,
            diff_fingerprint: 0,
        };

        for width in [64, 80] {
            let mut app = App::new(AppConfig::default());
            app.modal = ModalState::ConfirmApply {
                request: request.clone(),
                snapshot: snapshot.clone(),
            };
            let rendered = render_text(&mut app, width, 24);
            let compact = rendered.split_whitespace().collect::<Vec<_>>().join(" ");

            assert!(compact.contains("scope: selected targets - 20 target root(s)"));
            assert!(compact.contains("target preview (3 of 20)"));
            assert!(compact.contains("17 more targets - reviewed in the full Apply Plan"));
            assert!(compact.contains("Warning: apply may run 1 script(s)."));
            assert!(compact.contains("Warning: 1 path(s) will be deleted."));
            assert!(compact.contains("Enter: validate and apply"));
            assert!(compact.contains("Esc: cancel"));
        }
    }

    #[test]
    fn confirm_apply_empty_target_set_means_all_pending_changes() {
        let request = ActionRequest {
            action: Action::Apply,
            targets: Vec::new(),
            chattr_attrs: None,
        };
        let snapshot = ApplyPlanSnapshot {
            targets: Vec::new(),
            plan: ApplyPlan {
                added: vec![PathBuf::from("/home/demo/new.txt")],
                ..ApplyPlan::default()
            },
            diff_fingerprint: 0,
        };
        let mut app = App::new(AppConfig::default());
        app.modal = ModalState::ConfirmApply { request, snapshot };

        for width in [64, 80] {
            let rendered = render_text(&mut app, width, 24);
            let compact = rendered.split_whitespace().collect::<Vec<_>>().join(" ");
            assert!(compact.contains("scope: ALL pending changes - no target filter"));
            assert!(compact.contains("target roots: ALL pending changes"));
            assert!(compact.contains("Enter: validate and apply"));
            assert!(compact.contains("Esc: cancel"));
        }
    }

    #[test]
    fn virtual_absent_directory_child_marks_recursive_scope_in_both_modals() {
        let mut app = App::new(AppConfig::default());
        let name = format!("chezmoi-virtual-scope-{}", std::process::id());
        let relative_root = PathBuf::from(format!(".config/{name}"));
        let relative_plan = relative_root.join("nested/settings.toml");
        let target = app.home_dir.join(&relative_root);
        assert!(!target.exists(), "fixture directory must be absent");

        let request = ActionRequest {
            action: Action::Apply,
            targets: vec![target.clone()],
            chattr_attrs: None,
        };
        let snapshot = ApplyPlanSnapshot {
            targets: vec![target],
            plan: ApplyPlan {
                modified: vec![relative_plan],
                ..ApplyPlan::default()
            },
            diff_fingerprint: 1,
        };

        for modal in [
            ModalState::ApplyPlan {
                request: request.clone(),
                snapshot: snapshot.clone(),
                scroll: 0,
            },
            ModalState::ConfirmApply { request, snapshot },
        ] {
            app.modal = modal;
            let rendered = render_text(&mut app, 64, 24);
            let compact = rendered.split_whitespace().collect::<Vec<_>>().join(" ");
            assert!(
                compact.contains("directory roots include every affected descendant"),
                "{rendered}"
            );
            assert!(compact.contains("recursively"), "{rendered}");
        }
    }

    #[test]
    fn apply_plan_shows_targets_and_all_affected_path_groups() {
        let root = tempfile::tempdir().expect("temp root");
        let target = root.path().join(
            "deliberately-long-target-root/for-wrapped-path-rendering-across-terminals/日本語",
        );
        std::fs::create_dir_all(&target).expect("target root");
        let affected = (0..20)
            .map(|index| target.join(format!("nested/日本語/file-{index:02}.txt")))
            .collect::<Vec<_>>();
        let request = ActionRequest {
            action: Action::Apply,
            targets: vec![target.clone()],
            chattr_attrs: None,
        };
        let plan = ApplyPlan {
            added: vec![affected[0].clone()],
            modified: affected[1..].to_vec(),
            deleted: vec![target.join("removed.txt")],
            run: vec![target.join("run_once.sh")],
            unknown: vec![target.join("unknown.txt")],
        };
        let snapshot = ApplyPlanSnapshot {
            targets: vec![target.clone()],
            plan,
            diff_fingerprint: 0,
        };
        let mut app = App::new(AppConfig::default());
        app.modal = ModalState::ApplyPlan {
            request,
            snapshot,
            scroll: 0,
        };

        let initial = apply_plan_content_text(&mut app, 64, 24);
        let compact = initial.split_whitespace().collect::<Vec<_>>().join(" ");
        assert!(compact.contains("scope: selected targets - 1 target root(s)"));
        assert!(compact.contains("target roots: 1 selected"));
        assert!(
            compact.contains("directory roots include every affected descendant"),
            "{initial}"
        );
        assert!(compact.contains("recursively"), "{initial}");
        let target_text = target.display().to_string();
        assert!(
            initial
                .lines()
                .all(|line| !line.contains(target_text.as_str())),
            "fixture target did not wrap:\n{initial}"
        );
        assert!(
            initial
                .split_whitespace()
                .collect::<String>()
                .contains(&target_text),
            "{initial}"
        );
        assert!(compact.contains("changes: 23 total"));

        let required = [
            "Added (1)",
            "Modified (19)",
            "Deleted (1)",
            "Run scripts (1)",
            "Unknown (1)",
            "Warning: apply may run 1 script(s).",
            "Warning: 1 path(s) will be deleted.",
            "Warning: unknown status entries need manual review.",
        ];
        let mut reached = [false; 8];
        let mut reached_footer = false;
        for _ in 0..100 {
            let visible = apply_plan_content_text(&mut app, 64, 24);
            let compact = visible.split_whitespace().collect::<Vec<_>>().join(" ");
            for (found, text) in reached.iter_mut().zip(required) {
                *found |= compact.contains(text);
            }
            if compact.contains("Enter: continue to confirmation")
                && compact.contains("Esc: cancel")
            {
                reached_footer = true;
                break;
            }

            let ModalState::ApplyPlan { scroll, .. } = &mut app.modal else {
                panic!("apply plan modal");
            };
            *scroll = scroll.saturating_add(10);
        }

        assert_eq!(reached, [true; 8]);
        assert!(reached_footer, "apply plan footer was not reachable");
    }

    #[test]
    fn input_modal_places_cursor_after_the_value() {
        let mut app = App::new(AppConfig::default());
        app.modal = ModalState::Input {
            kind: InputKind::ChattrAttrs,
            request: request(Action::Chattr),
            value: "private".to_string(),
        };
        let backend = TestBackend::new(64, 24);
        let mut terminal = Terminal::new(backend).expect("test terminal");
        terminal
            .draw(|frame| draw(frame, &mut app))
            .expect("draw succeeds");
        let cursor = terminal
            .backend_mut()
            .get_cursor_position()
            .expect("cursor position");
        assert_eq!(cursor.y, 11);
        assert_eq!(cursor.x, 11);
    }

    #[test]
    fn clearing_styles_preserves_no_color_text() {
        let mut buffer = Buffer::empty(Rect::new(0, 0, 8, 1));
        buffer[(0, 0)]
            .set_symbol("色")
            .set_fg(Color::Red)
            .set_bg(Color::Blue);
        clear_buffer_styles(&mut buffer);
        assert_eq!(buffer[(0, 0)].symbol(), "色");
        assert_eq!(buffer[(0, 0)].style().fg, Some(Color::Reset));
        assert_eq!(buffer[(0, 0)].style().bg, Some(Color::Reset));
        assert_eq!(
            buffer[(0, 0)].style().add_modifier,
            ratatui::style::Modifier::empty()
        );
    }

    #[test]
    fn dark_canvas_sets_readable_default_colors() {
        let mut buffer = Buffer::empty(Rect::new(0, 0, 4, 1));
        ensure_dark_canvas(&mut buffer);
        assert_eq!(buffer[(0, 0)].fg, Color::Gray);
        assert_eq!(buffer[(0, 0)].bg, Color::Black);
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

        let apply_idx = rows
            .iter()
            .position(|row| {
                matches!(
                    row,
                    ActionMenuRow::Header(ActionMenuSection::Group(
                        crate::action_policy::ActionGroup::ApplyAndSync
                    ))
                )
            })
            .expect("apply and sync header");
        let files_idx = rows
            .iter()
            .position(|row| {
                matches!(
                    row,
                    ActionMenuRow::Header(ActionMenuSection::Group(
                        crate::action_policy::ActionGroup::Files
                    ))
                )
            })
            .expect("files header");
        let danger_idx = rows
            .iter()
            .position(|row| {
                matches!(
                    row,
                    ActionMenuRow::Header(ActionMenuSection::Group(
                        crate::action_policy::ActionGroup::Danger
                    ))
                )
            })
            .expect("danger header");

        assert!(apply_idx < files_idx);
        assert!(files_idx < danger_idx);
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
}
