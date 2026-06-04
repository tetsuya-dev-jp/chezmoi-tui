use crate::config::AppConfig;
use crate::domain::{
    Action, ActionRequest, ChangeKind, CommandResult, DiffText, ListView, StatusEntry,
};
use std::collections::{BTreeMap, BTreeSet, HashSet, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const MAX_LOG_LINES: usize = 500;
const MAX_NOTICE_HISTORY: usize = 50;
const LIST_FILTER_DEBOUNCE_MS: u64 = 120;
const INITIAL_UNMANAGED_FILTER_INDEX_ENTRIES: usize = 50_000;
const UNMANAGED_FILTER_INDEX_STEP: usize = 50_000;
const MAX_UNMANAGED_FILTER_INDEX_ENTRIES: usize = 200_000;
const LIVE_FILTER_INITIAL_UNMANAGED_INDEX_ENTRIES: usize = 2_000;
const LIVE_FILTER_UNMANAGED_INDEX_STEP: usize = 2_000;
const LIVE_FILTER_MAX_UNMANAGED_INDEX_ENTRIES: usize = 20_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaneFocus {
    List,
    Detail,
    Log,
}

impl PaneFocus {
    pub fn next(self) -> Self {
        match self {
            Self::List => Self::Detail,
            Self::Detail => Self::Log,
            Self::Log => Self::List,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DetailKind {
    Diff,
    Preview,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PreviewOrigin {
    Destination,
    WorkingDirectory,
    Source,
}

impl PreviewOrigin {
    pub fn label(self) -> &'static str {
        match self {
            Self::Destination => "destination",
            Self::WorkingDirectory => "cwd",
            Self::Source => "source",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ApplyPlan {
    pub added: Vec<PathBuf>,
    pub modified: Vec<PathBuf>,
    pub deleted: Vec<PathBuf>,
    pub run: Vec<PathBuf>,
    pub unknown: Vec<PathBuf>,
}

impl ApplyPlan {
    pub fn total(&self) -> usize {
        self.added.len()
            + self.modified.len()
            + self.deleted.len()
            + self.run.len()
            + self.unknown.len()
    }

    pub fn sort_groups(&mut self) {
        self.added.sort();
        self.modified.sort();
        self.deleted.sort();
        self.run.sort();
        self.unknown.sort();
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApplyPlanMode {
    OpenPlan,
    ValidateBeforeExecute,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApplyPlanSnapshot {
    pub plan: ApplyPlan,
    pub diff_fingerprint: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoticeTone {
    Info,
    Success,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notice {
    pub tone: NoticeTone,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfirmStep {
    Primary,
    DangerPhrase,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InputKind {
    ChattrAttrs,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchScope {
    Detail,
    Log,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LayoutMode {
    #[default]
    Normal,
    DetailMax,
    LogMax,
}

impl LayoutMode {
    pub fn next_for_focus(self, focus: PaneFocus) -> Self {
        match (self, focus) {
            (Self::Normal, PaneFocus::Detail) => Self::DetailMax,
            (Self::Normal, PaneFocus::Log) => Self::LogMax,
            (Self::Normal, _) => Self::DetailMax,
            _ => Self::Normal,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModalState {
    None,
    ListFilter {
        value: String,
        original: String,
    },
    Ignore {
        requests: Vec<ActionRequest>,
        selected: usize,
    },
    AddOptions {
        requests: Vec<ActionRequest>,
        selected: usize,
        template: bool,
        private: bool,
        executable: bool,
        encrypted: bool,
    },
    ActionMenu {
        selected: usize,
        filter: String,
    },
    Help {
        scroll: usize,
    },
    NoticeHistory {
        scroll: usize,
    },
    Search {
        scope: SearchScope,
        value: String,
    },
    ActionPreflight {
        requests: Vec<ActionRequest>,
        scroll: usize,
    },
    Confirm {
        request: ActionRequest,
        step: ConfirmStep,
        typed: String,
    },
    ApplyPlan {
        request: ActionRequest,
        snapshot: ApplyPlanSnapshot,
        scroll: usize,
    },
    Input {
        kind: InputKind,
        request: ActionRequest,
        value: String,
    },
}

#[derive(Debug, Clone)]
pub enum BackendTask {
    RefreshAll,
    LoadDiff {
        request_id: u64,
        target: Option<PathBuf>,
    },
    LoadPreview {
        request_id: u64,
        target: PathBuf,
        absolute: PathBuf,
        origin: PreviewOrigin,
    },
    RunAction {
        request: ActionRequest,
    },
    PrepareApplyPlan {
        request: ActionRequest,
        expected_snapshot: Option<ApplyPlanSnapshot>,
        mode: ApplyPlanMode,
    },
}

#[derive(Debug, Clone)]
pub enum BackendEvent {
    Refreshed {
        status: Vec<StatusEntry>,
        managed: Vec<PathBuf>,
        unmanaged: Vec<PathBuf>,
        source_dir: Option<PathBuf>,
        source: Vec<PathBuf>,
    },
    DiffLoaded {
        request_id: u64,
        target: Option<PathBuf>,
        diff: DiffText,
    },
    PreviewLoaded {
        request_id: u64,
        target: PathBuf,
        origin: PreviewOrigin,
        content: String,
    },
    ActionFinished {
        request: ActionRequest,
        result: CommandResult,
    },
    ApplyPlanPrepared {
        request: ActionRequest,
        status: Vec<StatusEntry>,
        diff_fingerprint: u64,
        expected_snapshot: Option<ApplyPlanSnapshot>,
        mode: ApplyPlanMode,
    },
    Error {
        context: String,
        message: String,
    },
}

#[derive(Debug, Clone)]
struct VisibleEntry {
    path: PathBuf,
    depth: usize,
    is_dir: bool,
    can_expand: bool,
    is_symlink: bool,
}

#[derive(Debug, Clone, Copy, Default)]
struct DirectoryState {
    is_dir: bool,
    can_expand: bool,
    is_symlink: bool,
}

#[derive(Debug, Clone, Default)]
struct UnmanagedFilterCache {
    entries: Vec<PathBuf>,
    seen: BTreeSet<PathBuf>,
    frontier: VecDeque<PathBuf>,
    initialized: bool,
    scan_complete: bool,
}

pub struct App {
    pub config: AppConfig,
    pub focus: PaneFocus,
    pub view: ListView,
    pub status_entries: Vec<StatusEntry>,
    pub managed_entries: Vec<PathBuf>,
    pub unmanaged_entries: Vec<PathBuf>,
    pub source_entries: Vec<PathBuf>,
    pub selected_index: usize,
    list_scroll: usize,
    pub detail_kind: DetailKind,
    pub detail_title: String,
    pub detail_text: String,
    pub detail_target: Option<PathBuf>,
    pub detail_scroll: usize,
    pub logs: Vec<String>,
    pub log_tail_offset: usize,
    pub detail_hscroll: usize,
    pub log_hscroll: usize,
    pub layout_mode: LayoutMode,
    pub modal: ModalState,
    list_filter: String,
    staged_list_filter: Option<String>,
    staged_filter_updated_at: Option<Instant>,
    busy_tasks: usize,
    latest_notice: Option<Notice>,
    notice_history: Vec<Notice>,
    busy_message: Option<String>,
    detail_search: Option<String>,
    log_search: Option<String>,
    detail_search_index: usize,
    log_search_index: usize,
    pub footer_help: bool,
    pub pending_foreground: Option<ActionRequest>,
    pub should_quit: bool,
    detail_request_seq: u64,
    latest_detail_request_id: Option<u64>,
    pub home_dir: PathBuf,
    working_dir: PathBuf,
    expanded_dirs: BTreeSet<PathBuf>,
    marked_entries: BTreeSet<PathBuf>,
    batch_action: Option<Action>,
    batch_total: usize,
    batch_confirmed: bool,
    batch_queue: VecDeque<ActionRequest>,
    visible_entries: Vec<VisibleEntry>,
    unmanaged_filter_cache: UnmanagedFilterCache,
}

impl App {
    pub fn new(config: AppConfig) -> Self {
        let working_dir = config.working_dir.clone();
        let home_dir = config
            .destination_dir
            .clone()
            .or_else(dirs::home_dir)
            .unwrap_or_else(|| working_dir.clone());
        let initial_view = config.initial_view;
        let initial_layout = config.default_layout;
        let initial_footer_help = config.footer_help;
        let mut app = Self {
            config,
            focus: PaneFocus::List,
            view: initial_view,
            status_entries: Vec::new(),
            managed_entries: Vec::new(),
            unmanaged_entries: Vec::new(),
            source_entries: Vec::new(),
            selected_index: 0,
            list_scroll: 0,
            detail_kind: DetailKind::Diff,
            detail_title: "Diff / Preview".to_string(),
            detail_text: String::new(),
            detail_target: None,
            detail_scroll: 0,
            logs: Vec::new(),
            log_tail_offset: 0,
            detail_hscroll: 0,
            log_hscroll: 0,
            layout_mode: initial_layout,
            modal: ModalState::None,
            list_filter: String::new(),
            staged_list_filter: None,
            staged_filter_updated_at: None,
            busy_tasks: 0,
            latest_notice: None,
            notice_history: Vec::new(),
            busy_message: None,
            detail_search: None,
            log_search: None,
            detail_search_index: 0,
            log_search_index: 0,
            footer_help: initial_footer_help,
            pending_foreground: None,
            should_quit: false,
            detail_request_seq: 0,
            latest_detail_request_id: None,
            home_dir,
            working_dir,
            expanded_dirs: BTreeSet::new(),
            marked_entries: BTreeSet::new(),
            batch_action: None,
            batch_total: 0,
            batch_confirmed: false,
            batch_queue: VecDeque::new(),
            visible_entries: Vec::new(),
            unmanaged_filter_cache: UnmanagedFilterCache::default(),
        };

        app.rebuild_visible_entries_reset();
        app
    }

    pub fn switch_view(&mut self, view: ListView) {
        self.view = view;
        self.list_filter.clear();
        self.clear_staged_list_filter();
        self.invalidate_unmanaged_filter_index();
        self.clear_marked_entries();
        self.rebuild_visible_entries_reset();
    }

    pub fn begin_busy_task(&mut self) {
        self.busy_tasks = self.busy_tasks.saturating_add(1);
    }

    pub fn begin_busy_task_with_message(&mut self, message: impl Into<String>) {
        self.busy_message = Some(message.into());
        self.begin_busy_task();
    }

    pub fn finish_busy_task(&mut self) {
        self.busy_tasks = self.busy_tasks.saturating_sub(1);
        if self.busy_tasks == 0 {
            self.busy_message = None;
        }
    }

    pub fn is_busy(&self) -> bool {
        self.busy_tasks > 0
    }

    #[cfg(test)]
    pub fn busy_task_count(&self) -> usize {
        self.busy_tasks
    }

    pub fn busy_message(&self) -> Option<&str> {
        self.busy_message.as_deref()
    }

    pub fn set_notice(&mut self, tone: NoticeTone, message: impl Into<String>) {
        let notice = Notice {
            tone,
            message: message.into(),
        };
        self.latest_notice = Some(notice.clone());
        self.notice_history.push(notice);
        if self.notice_history.len() > MAX_NOTICE_HISTORY {
            let trim = self.notice_history.len() - MAX_NOTICE_HISTORY;
            self.notice_history.drain(0..trim);
        }
    }

    pub fn set_info_notice(&mut self, message: impl Into<String>) {
        self.set_notice(NoticeTone::Info, message);
    }

    pub fn set_success_notice(&mut self, message: impl Into<String>) {
        self.set_notice(NoticeTone::Success, message);
    }

    pub fn set_error_notice(&mut self, message: impl Into<String>) {
        self.set_notice(NoticeTone::Error, message);
    }

    pub fn latest_notice(&self) -> Option<&Notice> {
        self.latest_notice.as_ref()
    }

    pub fn notice_history(&self) -> &[Notice] {
        &self.notice_history
    }

    #[cfg(test)]
    pub fn clear_notice(&mut self) {
        self.latest_notice = None;
    }

    pub fn apply_refresh_entries(
        &mut self,
        status: Vec<StatusEntry>,
        managed: Vec<PathBuf>,
        unmanaged: Vec<PathBuf>,
        source_dir: Option<PathBuf>,
        source: Vec<PathBuf>,
    ) {
        if self.config.source_dir.is_none() {
            self.config.source_dir = source_dir;
        }
        self.status_entries = status;
        self.managed_entries = managed;
        self.unmanaged_entries = unmanaged;
        self.source_entries = source;
        self.invalidate_unmanaged_filter_index();
    }

    pub fn select_next(&mut self) {
        let len = self.current_len();
        if len == 0 {
            self.selected_index = 0;
            return;
        }
        self.selected_index = (self.selected_index + 1) % len;
    }

    pub fn select_prev(&mut self) {
        let len = self.current_len();
        if len == 0 {
            self.selected_index = 0;
            return;
        }
        if self.selected_index == 0 {
            self.selected_index = len - 1;
        } else {
            self.selected_index -= 1;
        }
    }

    pub fn current_len(&self) -> usize {
        self.visible_entries.len()
    }

    pub fn list_scroll(&self) -> usize {
        self.list_scroll
    }

    pub fn sync_list_scroll(&mut self, viewport_rows: usize) {
        let len = self.current_len();
        if len == 0 {
            self.list_scroll = 0;
            return;
        }

        let rows = viewport_rows.max(1);
        if self.selected_index < self.list_scroll {
            self.list_scroll = self.selected_index;
        } else if self.selected_index >= self.list_scroll + rows {
            self.list_scroll = self.selected_index + 1 - rows;
        }

        let max_offset = len.saturating_sub(rows);
        if self.list_scroll > max_offset {
            self.list_scroll = max_offset;
        }
    }

    pub fn current_items(&self) -> Vec<String> {
        self.visible_entries
            .iter()
            .map(|entry| self.format_visible_entry(entry))
            .collect()
    }

    pub fn selected_path(&self) -> Option<PathBuf> {
        self.visible_entries
            .get(self.selected_index)
            .map(|entry| entry.path.clone())
    }

    pub fn selected_absolute_path(&self) -> Option<PathBuf> {
        self.selected_path()
            .map(|path| self.resolve_path_for_view(&path, self.view))
    }

    pub fn selected_is_directory(&self) -> bool {
        self.visible_entries
            .get(self.selected_index)
            .is_some_and(|entry| entry.is_dir)
    }

    pub fn selected_is_managed(&self) -> bool {
        let Some(selected_abs) = self.selected_absolute_path() else {
            return false;
        };

        self.is_absolute_path_managed(&selected_abs)
    }

    pub fn is_absolute_path_managed(&self, path: &Path) -> bool {
        self.managed_entries
            .iter()
            .any(|managed| self.managed_absolute_path(managed.as_path()) == path)
    }

    pub fn toggle_selected_mark(&mut self) -> bool {
        let Some(path) = self.selected_path() else {
            return false;
        };
        if self.marked_entries.contains(&path) {
            self.marked_entries.remove(&path)
        } else {
            self.marked_entries.insert(path)
        }
    }

    pub fn clear_marked_entries(&mut self) -> bool {
        if self.marked_entries.is_empty() {
            return false;
        }
        self.marked_entries.clear();
        true
    }

    pub fn marked_count(&self) -> usize {
        self.marked_entries.len()
    }

    pub fn selected_action_targets_absolute(&self) -> Vec<PathBuf> {
        if self.marked_entries.is_empty() {
            return self.selected_absolute_path().into_iter().collect();
        }

        self.visible_entries
            .iter()
            .filter(|entry| self.marked_entries.contains(&entry.path))
            .map(|entry| self.resolve_path_for_view(&entry.path, self.view))
            .collect()
    }

    pub fn start_batch(&mut self, requests: Vec<ActionRequest>) -> Option<ActionRequest> {
        if requests.is_empty() {
            return None;
        }

        if requests.len() == 1 {
            self.clear_batch();
            return requests.into_iter().next();
        }

        let mut queue = VecDeque::from(requests);
        let first = queue.pop_front()?;
        self.batch_action = Some(first.action);
        self.batch_total = queue.len() + 1;
        self.batch_confirmed = false;
        self.batch_queue = queue;
        Some(first)
    }

    pub fn pop_next_batch_request(&mut self) -> Option<ActionRequest> {
        self.batch_queue.pop_front()
    }

    pub fn batch_in_progress(&self) -> bool {
        self.batch_action.is_some()
    }

    pub fn batch_total(&self) -> usize {
        self.batch_total
    }

    pub fn batch_action(&self) -> Option<Action> {
        self.batch_action
    }

    pub fn batch_confirmed(&self) -> bool {
        self.batch_confirmed
    }

    pub fn mark_batch_confirmed(&mut self) {
        self.batch_confirmed = true;
    }

    pub fn apply_chattr_attrs_to_batch(&mut self, attrs: &str) {
        for request in &mut self.batch_queue {
            if request.action == Action::Chattr {
                request.chattr_attrs = Some(attrs.to_string());
            }
        }
    }

    pub fn clear_batch(&mut self) {
        self.batch_action = None;
        self.batch_total = 0;
        self.batch_confirmed = false;
        self.batch_queue.clear();
    }

    pub fn expand_selected_directory(&mut self) -> bool {
        if !self.view_supports_tree() {
            return false;
        }

        let Some(entry) = self.visible_entries.get(self.selected_index).cloned() else {
            return false;
        };
        if !entry.can_expand {
            return false;
        }

        let path = entry.path;
        let changed = self.expanded_dirs.insert(path.clone());
        if changed {
            self.rebuild_visible_entries_with_selection(Some(path));
        }
        changed
    }

    pub fn collapse_selected_directory_or_parent(&mut self) -> bool {
        if !self.view_supports_tree() {
            return false;
        }

        let Some(selected_path) = self.selected_path() else {
            return false;
        };

        let mut current: Option<&Path> = Some(selected_path.as_path());
        while let Some(path) = current {
            let candidate = path.to_path_buf();
            if self.expanded_dirs.contains(&candidate) {
                self.collapse_tree(&candidate);
                self.rebuild_visible_entries_with_selection(Some(candidate));
                return true;
            }
            current = path.parent();
        }

        false
    }

    pub fn open_action_menu(&mut self) {
        self.modal = ModalState::ActionMenu {
            selected: 0,
            filter: String::new(),
        };
    }

    pub fn open_ignore_menu(&mut self, requests: Vec<ActionRequest>) {
        self.modal = ModalState::Ignore {
            requests,
            selected: 0,
        };
    }

    pub fn open_add_options_menu(&mut self, requests: Vec<ActionRequest>) {
        self.modal = ModalState::AddOptions {
            requests,
            selected: 0,
            template: false,
            private: false,
            executable: false,
            encrypted: false,
        };
    }

    pub fn open_list_filter(&mut self) {
        self.clear_staged_list_filter();
        self.modal = ModalState::ListFilter {
            value: self.list_filter.clone(),
            original: self.list_filter.clone(),
        };
    }

    pub fn open_help(&mut self) {
        self.modal = ModalState::Help { scroll: 0 };
    }

    pub fn open_notice_history(&mut self) {
        self.modal = ModalState::NoticeHistory { scroll: 0 };
    }

    pub fn open_search(&mut self, scope: SearchScope) {
        let value = match scope {
            SearchScope::Detail => self.detail_search.clone().unwrap_or_default(),
            SearchScope::Log => self.log_search.clone().unwrap_or_default(),
        };
        self.modal = ModalState::Search { scope, value };
    }

    pub fn apply_search(&mut self, scope: SearchScope, value: String) -> bool {
        let query = value.trim().to_string();
        if query.is_empty() {
            match scope {
                SearchScope::Detail => self.detail_search = None,
                SearchScope::Log => self.log_search = None,
            }
            return false;
        }

        match scope {
            SearchScope::Detail => {
                self.detail_search = Some(query);
                self.detail_search_index = 0;
                self.jump_to_search_match(SearchScope::Detail, 0)
            }
            SearchScope::Log => {
                self.log_search = Some(query);
                self.log_search_index = 0;
                self.jump_to_search_match(SearchScope::Log, 0)
            }
        }
    }

    pub fn search_match_count(&self, scope: SearchScope) -> usize {
        let Some(query) = self.search_query(scope) else {
            return 0;
        };
        let query = query.to_ascii_lowercase();
        match scope {
            SearchScope::Detail => self
                .detail_text
                .lines()
                .filter(|line| line.to_ascii_lowercase().contains(&query))
                .count(),
            SearchScope::Log => self
                .logs
                .iter()
                .filter(|line| line.to_ascii_lowercase().contains(&query))
                .count(),
        }
    }

    pub fn next_search_match(&mut self, scope: SearchScope) -> bool {
        let count = self.search_match_count(scope);
        if count == 0 {
            return false;
        }
        let next = match scope {
            SearchScope::Detail => (self.detail_search_index + 1) % count,
            SearchScope::Log => (self.log_search_index + 1) % count,
        };
        self.jump_to_search_match(scope, next)
    }

    pub fn prev_search_match(&mut self, scope: SearchScope) -> bool {
        let count = self.search_match_count(scope);
        if count == 0 {
            return false;
        }
        let next = match scope {
            SearchScope::Detail => self.detail_search_index.checked_sub(1).unwrap_or(count - 1),
            SearchScope::Log => self.log_search_index.checked_sub(1).unwrap_or(count - 1),
        };
        self.jump_to_search_match(scope, next)
    }

    pub fn jump_next_diff_hunk(&mut self) -> bool {
        if self.detail_kind != DetailKind::Diff {
            return false;
        }
        let hunks = self.diff_hunk_lines();
        let Some(next) = hunks.into_iter().find(|line| *line > self.detail_scroll) else {
            return self.diff_hunk_lines().first().copied().is_some_and(|line| {
                self.detail_scroll = line;
                true
            });
        };
        self.detail_scroll = next;
        true
    }

    pub fn jump_prev_diff_hunk(&mut self) -> bool {
        if self.detail_kind != DetailKind::Diff {
            return false;
        }
        let hunks = self.diff_hunk_lines();
        let Some(prev) = hunks.iter().rev().find(|line| **line < self.detail_scroll) else {
            return hunks.last().copied().is_some_and(|line| {
                self.detail_scroll = line;
                true
            });
        };
        self.detail_scroll = *prev;
        true
    }

    fn diff_hunk_lines(&self) -> Vec<usize> {
        self.detail_text
            .lines()
            .enumerate()
            .filter_map(|(index, line)| line.starts_with("@@").then_some(index))
            .collect()
    }

    fn search_query(&self, scope: SearchScope) -> Option<&str> {
        match scope {
            SearchScope::Detail => self.detail_search.as_deref(),
            SearchScope::Log => self.log_search.as_deref(),
        }
    }

    fn jump_to_search_match(&mut self, scope: SearchScope, match_index: usize) -> bool {
        let Some(query) = self.search_query(scope) else {
            return false;
        };
        let query = query.to_ascii_lowercase();
        let line_index = match scope {
            SearchScope::Detail => self
                .detail_text
                .lines()
                .enumerate()
                .filter(|(_, line)| line.to_ascii_lowercase().contains(&query))
                .nth(match_index)
                .map(|(index, _)| index),
            SearchScope::Log => self
                .logs
                .iter()
                .enumerate()
                .filter(|(_, line)| line.to_ascii_lowercase().contains(&query))
                .nth(match_index)
                .map(|(index, _)| index),
        };
        let Some(line_index) = line_index else {
            return false;
        };
        match scope {
            SearchScope::Detail => {
                self.detail_search_index = match_index;
                self.detail_scroll = line_index;
            }
            SearchScope::Log => {
                self.log_search_index = match_index;
                self.log_tail_offset = self.logs.len().saturating_sub(line_index + 1);
            }
        }
        true
    }

    pub fn active_search_label(&self) -> Option<String> {
        match self.focus {
            PaneFocus::Detail => self.detail_search.as_ref().map(|query| {
                format!(
                    "detail /{} {}/{}",
                    query,
                    self.detail_search_index
                        .saturating_add(1)
                        .min(self.search_match_count(SearchScope::Detail).max(1)),
                    self.search_match_count(SearchScope::Detail)
                )
            }),
            PaneFocus::Log => self.log_search.as_ref().map(|query| {
                format!(
                    "log /{} {}/{}",
                    query,
                    self.log_search_index
                        .saturating_add(1)
                        .min(self.search_match_count(SearchScope::Log).max(1)),
                    self.search_match_count(SearchScope::Log)
                )
            }),
            PaneFocus::List => None,
        }
    }

    pub fn open_confirm(&mut self, request: ActionRequest) {
        self.modal = ModalState::Confirm {
            request,
            step: ConfirmStep::Primary,
            typed: String::new(),
        };
    }

    pub fn open_action_preflight(&mut self, requests: Vec<ActionRequest>) {
        self.modal = ModalState::ActionPreflight {
            requests,
            scroll: 0,
        };
    }

    pub fn scroll_modal_down(&mut self, lines: usize) -> bool {
        match &mut self.modal {
            ModalState::ActionPreflight { scroll, .. }
            | ModalState::ApplyPlan { scroll, .. }
            | ModalState::Help { scroll }
            | ModalState::NoticeHistory { scroll } => {
                *scroll = scroll.saturating_add(lines);
                true
            }
            _ => false,
        }
    }

    pub fn scroll_modal_up(&mut self, lines: usize) -> bool {
        match &mut self.modal {
            ModalState::ActionPreflight { scroll, .. }
            | ModalState::ApplyPlan { scroll, .. }
            | ModalState::Help { scroll }
            | ModalState::NoticeHistory { scroll } => {
                let before = *scroll;
                *scroll = scroll.saturating_sub(lines);
                *scroll != before
            }
            _ => false,
        }
    }

    pub fn open_input(&mut self, kind: InputKind, request: ActionRequest) {
        self.modal = ModalState::Input {
            kind,
            request,
            value: String::new(),
        };
    }

    pub fn close_modal(&mut self) {
        self.modal = ModalState::None;
    }

    pub fn list_filter(&self) -> &str {
        &self.list_filter
    }

    pub fn flush_staged_filter(&mut self, now: Instant) -> bool {
        let Some(updated_at) = self.staged_filter_updated_at else {
            return false;
        };
        if now.duration_since(updated_at) < Duration::from_millis(LIST_FILTER_DEBOUNCE_MS) {
            return false;
        }

        let Some(value) = self.staged_list_filter.take() else {
            self.staged_filter_updated_at = None;
            return false;
        };
        self.staged_filter_updated_at = None;
        if self.list_filter == value {
            return false;
        }

        self.list_filter = value;
        self.rebuild_visible_entries();
        true
    }

    pub fn apply_list_filter_immediately(&mut self, value: String) {
        self.clear_staged_list_filter();
        if self.list_filter == value {
            return;
        }
        self.list_filter = value;
        self.rebuild_visible_entries();
    }

    pub fn log(&mut self, line: String) {
        self.logs.push(line);
        if self.log_tail_offset > 0 {
            self.log_tail_offset = self.log_tail_offset.saturating_add(1);
        }
        if self.logs.len() > MAX_LOG_LINES {
            let to_trim = self.logs.len() - MAX_LOG_LINES;
            self.logs.drain(0..to_trim);
        }
    }

    pub fn scroll_log_up(&mut self, lines: usize) -> bool {
        let before = self.log_tail_offset;
        self.log_tail_offset = self.log_tail_offset.saturating_add(lines);
        self.log_tail_offset != before
    }

    pub fn scroll_log_down(&mut self, lines: usize) -> bool {
        let before = self.log_tail_offset;
        self.log_tail_offset = self.log_tail_offset.saturating_sub(lines);
        self.log_tail_offset != before
    }

    pub fn sync_selection_bounds(&mut self) {
        let len = self.current_len();
        if len == 0 {
            self.selected_index = 0;
            self.list_scroll = 0;
        } else if self.selected_index >= len {
            self.selected_index = len - 1;
        }
    }

    pub fn rebuild_visible_entries(&mut self) {
        let selected = self.selected_path();
        self.rebuild_visible_entries_with_selection(selected);
    }

    pub fn action_by_index(index: usize) -> Option<Action> {
        Action::ALL.get(index).copied()
    }

    pub fn action_disabled_reason(&self, action: Action) -> Option<String> {
        if action == Action::ReAdd && !self.readd_selection_is_eligible() {
            return Some("only for modified status files".to_string());
        }
        if !Self::action_visible_in_view(self.view, action) {
            return Some(format!("not available in {} view", self.view.title()));
        }
        if action.needs_target() && self.selected_action_targets_absolute().is_empty() {
            return Some("requires a selected target".to_string());
        }
        if action.requires_exact_managed_target()
            && self
                .selected_action_targets_absolute()
                .iter()
                .any(|path| !self.is_absolute_path_managed(path))
        {
            return Some(format!(
                "{} is only for exact managed entries",
                action.label()
            ));
        }
        if action == Action::Add
            && self
                .selected_action_targets_absolute()
                .iter()
                .any(|path| path.is_dir())
        {
            return Some("directory add is disabled; expand and select files".to_string());
        }
        None
    }

    pub fn action_menu_indices(&self, filter: &str) -> Vec<usize> {
        let query = filter.trim().to_ascii_lowercase();
        let danger_filter = query.strip_prefix("danger:");
        let effective_query = danger_filter.unwrap_or(&query);
        let mut matches: Vec<(usize, u8, u8, String)> = Action::ALL
            .iter()
            .enumerate()
            .filter_map(|(index, action)| {
                if !self.action_visible(*action) {
                    return None;
                }
                if danger_filter.is_some() && !action.is_dangerous() {
                    return None;
                }
                if !query.is_empty() && danger_filter.is_none() && action.is_dangerous() {
                    return None;
                }

                let label = action.label().to_ascii_lowercase();
                let match_rank = if effective_query.is_empty() {
                    0
                } else {
                    Self::action_filter_match_rank(&label, effective_query)?
                };

                Some((
                    index,
                    match_rank,
                    Self::action_section_order(*action),
                    label,
                ))
            })
            .collect();

        if query.is_empty() {
            matches.sort_by(|a, b| a.2.cmp(&b.2).then(a.3.cmp(&b.3)));
        } else {
            matches.sort_by(|a, b| a.1.cmp(&b.1).then(a.3.cmp(&b.3)).then(a.2.cmp(&b.2)));
        }
        matches.into_iter().map(|(index, _, _, _)| index).collect()
    }

    pub(crate) fn readd_selection_is_eligible(&self) -> bool {
        if self.view != ListView::Status {
            return false;
        }

        let entries = self.selected_status_entries_for_actions();
        !entries.is_empty()
            && entries
                .iter()
                .all(|entry| entry.actual_vs_target == ChangeKind::Modified)
            && entries.iter().all(|entry| {
                !self
                    .resolve_path_for_view(&entry.path, ListView::Status)
                    .is_dir()
            })
    }

    fn selected_status_entries_for_actions(&self) -> Vec<&StatusEntry> {
        if self.view != ListView::Status {
            return Vec::new();
        }

        if self.marked_entries.is_empty() {
            return self
                .selected_path()
                .into_iter()
                .filter_map(|path| self.status_entries.iter().find(|entry| entry.path == path))
                .collect();
        }

        self.visible_entries
            .iter()
            .filter(|entry| self.marked_entries.contains(&entry.path))
            .filter_map(|entry| {
                self.status_entries
                    .iter()
                    .find(|status| status.path == entry.path)
            })
            .collect()
    }

    fn action_visible(&self, action: Action) -> bool {
        if action == Action::ReAdd {
            return self.readd_selection_is_eligible();
        }

        Self::action_visible_in_view(self.view, action)
    }

    fn action_section_order(action: Action) -> u8 {
        if action.is_dangerous() {
            2
        } else {
            u8::from(action.needs_target())
        }
    }

    fn action_visible_in_view(view: ListView, action: Action) -> bool {
        match view {
            ListView::Status => matches!(
                action,
                Action::Apply
                    | Action::Doctor
                    | Action::Data
                    | Action::OpenSourceDir
                    | Action::ExternalDiff
                    | Action::DebugContext
                    | Action::Update
                    | Action::EditConfig
                    | Action::EditConfigTemplate
                    | Action::EditIgnore
                    | Action::Merge
                    | Action::MergeAll
                    | Action::Edit
                    | Action::Forget
                    | Action::Chattr
                    | Action::Purge
            ),
            ListView::Managed => matches!(
                action,
                Action::Apply
                    | Action::Doctor
                    | Action::Data
                    | Action::OpenSourceDir
                    | Action::ExternalDiff
                    | Action::DebugContext
                    | Action::Update
                    | Action::EditConfig
                    | Action::EditConfigTemplate
                    | Action::EditIgnore
                    | Action::Edit
                    | Action::Forget
                    | Action::Chattr
                    | Action::Destroy
                    | Action::Purge
            ),
            ListView::Unmanaged => {
                matches!(
                    action,
                    Action::Add
                        | Action::Ignore
                        | Action::Apply
                        | Action::Doctor
                        | Action::Data
                        | Action::OpenSourceDir
                        | Action::ExternalDiff
                        | Action::DebugContext
                        | Action::Update
                        | Action::EditConfig
                        | Action::EditConfigTemplate
                        | Action::EditIgnore
                        | Action::Purge
                )
            }
            ListView::Source => matches!(
                action,
                Action::Apply
                    | Action::Doctor
                    | Action::Data
                    | Action::OpenSourceDir
                    | Action::ExternalDiff
                    | Action::DebugContext
                    | Action::Update
                    | Action::EditConfig
                    | Action::EditConfigTemplate
                    | Action::EditIgnore
                    | Action::Purge
            ),
        }
    }

    fn action_filter_match_rank(label: &str, query: &str) -> Option<u8> {
        if label == query {
            return Some(0);
        }
        if label
            .split(|c: char| !c.is_ascii_alphanumeric())
            .any(|token| token == query)
        {
            return Some(1);
        }
        if label.starts_with(query) {
            return Some(2);
        }
        if label.contains(query) {
            return Some(3);
        }
        None
    }

    pub fn confirmation_impact_lines(&self, request: &ActionRequest) -> Vec<String> {
        match request.action {
            Action::Destroy => vec![
                "Impact: removes the selected target from chezmoi source/destination/state."
                    .to_string(),
                "This cannot be undone from this TUI.".to_string(),
            ],
            Action::Purge => vec![
                "Impact: removes chezmoi configuration and data for this environment.".to_string(),
                format!("destination: {}", self.home_dir.display()),
                format!(
                    "source: {}",
                    self.config.source_dir.as_ref().map_or_else(
                        || "(resolved by chezmoi at runtime)".to_string(),
                        |path| path.display().to_string()
                    )
                ),
                "This cannot be undone from this TUI.".to_string(),
            ],
            _ => Vec::new(),
        }
    }

    pub fn scroll_detail_up(&mut self, lines: usize) -> bool {
        if self.detail_scroll == 0 {
            return false;
        }
        self.detail_scroll = self.detail_scroll.saturating_sub(lines);
        true
    }

    pub fn scroll_detail_down(&mut self, lines: usize) -> bool {
        let max = self.detail_max_scroll();
        if self.detail_scroll >= max {
            return false;
        }
        self.detail_scroll = (self.detail_scroll + lines).min(max);
        true
    }

    pub fn scroll_detail_right(&mut self, cols: usize) -> bool {
        let before = self.detail_hscroll;
        self.detail_hscroll = self.detail_hscroll.saturating_add(cols);
        self.detail_hscroll != before
    }

    pub fn scroll_detail_left(&mut self, cols: usize) -> bool {
        let before = self.detail_hscroll;
        self.detail_hscroll = self.detail_hscroll.saturating_sub(cols);
        self.detail_hscroll != before
    }

    pub fn scroll_log_right(&mut self, cols: usize) -> bool {
        let before = self.log_hscroll;
        self.log_hscroll = self.log_hscroll.saturating_add(cols);
        self.log_hscroll != before
    }

    pub fn scroll_log_left(&mut self, cols: usize) -> bool {
        let before = self.log_hscroll;
        self.log_hscroll = self.log_hscroll.saturating_sub(cols);
        self.log_hscroll != before
    }

    pub fn toggle_layout_mode_for_focus(&mut self) {
        self.layout_mode = self.layout_mode.next_for_focus(self.focus);
    }

    fn detail_max_scroll(&self) -> usize {
        self.detail_text.lines().count().saturating_sub(1)
    }

    pub fn set_detail_diff(&mut self, target: Option<&Path>, text: String) {
        self.detail_kind = DetailKind::Diff;
        self.detail_title = match target {
            Some(path) => format!("Diff: {}", path.display()),
            None => "Diff: (all)".to_string(),
        };
        self.detail_text = text;
        self.detail_target = target.map(Path::to_path_buf);
        self.detail_scroll = 0;
        self.detail_hscroll = 0;
    }

    pub fn set_detail_preview(&mut self, target: &Path, origin: PreviewOrigin, content: String) {
        self.detail_kind = DetailKind::Preview;
        self.detail_title = format!("Preview({}): {}", origin.label(), target.display());
        self.detail_text = content;
        self.detail_target = Some(target.to_path_buf());
        self.detail_scroll = 0;
        self.detail_hscroll = 0;
    }

    pub fn view_context_text(&self) -> String {
        let (label, path) = match self.view {
            ListView::Status | ListView::Managed => ("dest", &self.home_dir),
            ListView::Unmanaged => ("cwd", &self.working_dir),
            ListView::Source => (
                "source",
                self.config.source_dir.as_ref().unwrap_or(&self.working_dir),
            ),
        };
        format!("{label}={}", path.display())
    }

    pub fn preview_origin_for_view(&self, view: ListView) -> PreviewOrigin {
        match view {
            ListView::Status | ListView::Managed => PreviewOrigin::Destination,
            ListView::Unmanaged => PreviewOrigin::WorkingDirectory,
            ListView::Source => PreviewOrigin::Source,
        }
    }

    pub(crate) fn build_apply_plan_from_status(
        &self,
        status: &[StatusEntry],
        target: Option<&Path>,
    ) -> ApplyPlan {
        let mut plan = ApplyPlan::default();
        for entry in status {
            if let Some(target) = target
                && !self.status_entry_matches_apply_target(entry.path.as_path(), target)
            {
                continue;
            }

            match entry.actual_vs_target {
                ChangeKind::Added => plan.added.push(entry.path.clone()),
                ChangeKind::Modified => plan.modified.push(entry.path.clone()),
                ChangeKind::Deleted => plan.deleted.push(entry.path.clone()),
                ChangeKind::Run => plan.run.push(entry.path.clone()),
                ChangeKind::Unknown(_) => plan.unknown.push(entry.path.clone()),
                ChangeKind::None => {}
            }
        }
        plan.sort_groups();
        plan
    }

    #[allow(dead_code)]
    fn build_apply_plan_for_target(&self, target: Option<&Path>) -> ApplyPlan {
        self.build_apply_plan_from_status(&self.status_entries, target)
    }

    pub(crate) fn build_apply_plan_snapshot_from_status(
        &self,
        status: &[StatusEntry],
        target: Option<&Path>,
        diff_fingerprint: u64,
    ) -> ApplyPlanSnapshot {
        ApplyPlanSnapshot {
            plan: self.build_apply_plan_from_status(status, target),
            diff_fingerprint,
        }
    }

    fn status_entry_matches_apply_target(&self, entry_path: &Path, target: &Path) -> bool {
        let absolute = self.resolve_path_for_view(entry_path, ListView::Status);
        target == entry_path || target == absolute || absolute.starts_with(target)
    }

    #[allow(dead_code)]
    pub fn open_apply_plan(&mut self, request: ActionRequest) {
        let plan = self.build_apply_plan_for_target(request.target.as_deref());
        let snapshot = ApplyPlanSnapshot {
            plan,
            diff_fingerprint: 0,
        };
        self.modal = ModalState::ApplyPlan {
            request,
            snapshot,
            scroll: 0,
        };
    }

    pub fn open_apply_plan_with_snapshot(
        &mut self,
        request: ActionRequest,
        snapshot: ApplyPlanSnapshot,
    ) {
        self.modal = ModalState::ApplyPlan {
            request,
            snapshot,
            scroll: 0,
        };
    }

    pub fn clear_detail(&mut self) {
        self.detail_title = "Diff / Preview".to_string();
        self.detail_text.clear();
        self.detail_target = None;
        self.detail_scroll = 0;
    }

    pub fn begin_detail_request(&mut self) -> u64 {
        self.detail_request_seq = self.detail_request_seq.wrapping_add(1).max(1);
        self.latest_detail_request_id = Some(self.detail_request_seq);
        self.detail_request_seq
    }

    pub fn accepts_detail_request(&self, request_id: u64) -> bool {
        self.latest_detail_request_id == Some(request_id)
    }

    fn rebuild_visible_entries_reset(&mut self) {
        self.rebuild_visible_entries_with_selection(None);
    }

    fn rebuild_visible_entries_with_selection(&mut self, preferred: Option<PathBuf>) {
        let previous = preferred.or_else(|| self.selected_path());
        let filtering = !self.list_filter.trim().is_empty();

        let entries = if filtering {
            self.build_filtered_visible_entries()
        } else {
            self.build_unfiltered_visible_entries()
        };

        self.visible_entries = entries;
        let visible_paths: HashSet<PathBuf> = self
            .visible_entries
            .iter()
            .map(|entry| entry.path.clone())
            .collect();
        self.marked_entries
            .retain(|path| visible_paths.contains(path));

        if let Some(target) = previous
            && let Some(idx) = self.visible_entries.iter().position(|e| e.path == target)
        {
            self.selected_index = idx;
            return;
        }

        self.sync_selection_bounds();
    }

    fn build_unfiltered_visible_entries(&self) -> Vec<VisibleEntry> {
        let base_paths = self.base_paths_for_view();
        let mut seen = HashSet::new();
        let mut entries = Vec::new();

        if self.view == ListView::Unmanaged {
            if self
                .unmanaged_entries
                .iter()
                .any(|path| path == Path::new("."))
            {
                for base in base_paths {
                    self.push_visible_recursive(&base, 0, &mut entries, &mut seen, false);
                }
                return entries;
            }

            self.push_unmanaged_visible_entries(&mut entries, false);
            return entries;
        }

        if self.view == ListView::Managed {
            self.push_managed_visible_entries(&mut entries, false);
            return entries;
        }

        if self.view == ListView::Source {
            self.push_source_visible_entries(&mut entries, false);
            return entries;
        }

        for path in base_paths {
            if !seen.insert(path.clone()) {
                continue;
            }
            let is_dir = self.path_is_directory(&path);
            entries.push(VisibleEntry {
                path,
                depth: 0,
                is_dir,
                can_expand: false,
                is_symlink: false,
            });
        }
        entries
    }

    fn build_filtered_visible_entries(&mut self) -> Vec<VisibleEntry> {
        let query = self.list_filter.trim().to_ascii_lowercase();
        let view = self.view;
        match view {
            ListView::Status => self.build_filtered_status_entries(&query),
            ListView::Managed => self.build_filtered_tree_entries(
                self.managed_tree_nodes().into_iter().collect(),
                &query,
            ),
            ListView::Unmanaged => {
                let source_paths = if matches!(self.modal, ModalState::ListFilter { .. }) {
                    self.unmanaged_filter_source_paths_live(&query)
                } else {
                    self.unmanaged_filter_source_paths(&query)
                };
                self.build_filtered_tree_entries(source_paths, &query)
            }
            ListView::Source => self.build_filtered_tree_entries(
                self.source_tree_nodes().into_iter().collect(),
                &query,
            ),
        }
    }

    fn build_filtered_status_entries(&self, query: &str) -> Vec<VisibleEntry> {
        self.status_entries
            .iter()
            .filter_map(|entry| {
                let path = entry.path.clone();
                let matched =
                    query.is_empty() || path.to_string_lossy().to_ascii_lowercase().contains(query);
                if !matched {
                    return None;
                }
                Some(VisibleEntry {
                    is_dir: self.path_is_directory(&path),
                    path,
                    depth: 0,
                    can_expand: false,
                    is_symlink: false,
                })
            })
            .collect()
    }

    fn build_filtered_tree_entries(
        &self,
        source_paths: Vec<PathBuf>,
        query: &str,
    ) -> Vec<VisibleEntry> {
        if query.is_empty() {
            return Vec::new();
        }

        let matched: BTreeSet<PathBuf> = source_paths
            .into_iter()
            .filter(|path| Self::tree_entry_name_matches_query(path, query))
            .collect();
        if matched.is_empty() {
            return Vec::new();
        }

        let mut keep = matched;
        let mut ancestors = Vec::new();
        for path in &keep {
            let mut current = path.parent();
            while let Some(parent) = current {
                if parent.as_os_str().is_empty() {
                    break;
                }
                ancestors.push(parent.to_path_buf());
                current = parent.parent();
            }
        }
        keep.extend(ancestors);

        self.build_tree_entries_from_paths(&keep)
    }

    fn tree_entry_name_matches_query(path: &Path, query: &str) -> bool {
        path.file_name().and_then(|name| name.to_str()).map_or_else(
            || path.to_string_lossy().to_ascii_lowercase().contains(query),
            |name| name.to_ascii_lowercase().contains(query),
        )
    }

    fn build_tree_entries_from_paths(&self, nodes: &BTreeSet<PathBuf>) -> Vec<VisibleEntry> {
        if nodes.is_empty() {
            return Vec::new();
        }

        let mut children: BTreeMap<PathBuf, Vec<PathBuf>> = BTreeMap::new();
        let mut roots = Vec::new();

        for node in nodes {
            let parent = node.parent().map(Path::to_path_buf);
            if let Some(parent) = parent
                && !parent.as_os_str().is_empty()
                && nodes.contains(&parent)
            {
                children.entry(parent).or_default().push(node.clone());
            } else {
                roots.push(node.clone());
            }
        }

        for siblings in children.values_mut() {
            siblings.sort_by(|a, b| a.to_string_lossy().cmp(&b.to_string_lossy()));
        }
        roots.sort_by(|a, b| a.to_string_lossy().cmp(&b.to_string_lossy()));

        let mut entries = Vec::new();
        for root in &roots {
            self.push_filtered_tree_entry_recursive(root, 0, &children, &mut entries);
        }
        entries
    }

    fn push_filtered_tree_entry_recursive(
        &self,
        path: &Path,
        depth: usize,
        children: &BTreeMap<PathBuf, Vec<PathBuf>>,
        out: &mut Vec<VisibleEntry>,
    ) {
        let has_children = children
            .get(path)
            .is_some_and(|entries| !entries.is_empty());
        let managed_has_descendants =
            self.view == ListView::Managed && self.path_has_managed_descendants(path);
        let directory = self.path_directory_state_for_view(path, self.view);
        out.push(VisibleEntry {
            path: path.to_path_buf(),
            depth,
            is_dir: has_children || managed_has_descendants || directory.is_dir,
            can_expand: has_children || directory.can_expand,
            is_symlink: directory.is_symlink,
        });

        if let Some(child_paths) = children.get(path) {
            for child in child_paths {
                self.push_filtered_tree_entry_recursive(child, depth + 1, children, out);
            }
        }
    }

    fn path_has_managed_descendants(&self, path: &Path) -> bool {
        self.managed_entries
            .iter()
            .any(|managed| managed != path && managed.starts_with(path))
    }

    fn view_supports_tree(&self) -> bool {
        matches!(
            self.view,
            ListView::Managed | ListView::Unmanaged | ListView::Source
        )
    }

    fn base_paths_for_view(&self) -> Vec<PathBuf> {
        match self.view {
            ListView::Status => self
                .status_entries
                .iter()
                .map(|entry| entry.path.clone())
                .collect(),
            ListView::Managed => self.managed_entries.clone(),
            ListView::Unmanaged => {
                let base_paths: Vec<PathBuf> = self
                    .unmanaged_entries
                    .iter()
                    .filter(|path| self.is_visible_in_unmanaged_view(path.as_path()))
                    .cloned()
                    .collect();

                // When infra collapses to ".", treat it as "working dir root" and
                // show direct children instead of rendering "./" as a tree node.
                if base_paths.iter().any(|path| path == Path::new(".")) {
                    return self.read_children(Path::new("."));
                }

                base_paths
            }
            ListView::Source => self.source_entries.clone(),
        }
    }

    fn unmanaged_filter_source_paths(&mut self, query: &str) -> Vec<PathBuf> {
        self.unmanaged_filter_source_paths_with_limits(
            query,
            INITIAL_UNMANAGED_FILTER_INDEX_ENTRIES,
            UNMANAGED_FILTER_INDEX_STEP,
            MAX_UNMANAGED_FILTER_INDEX_ENTRIES,
        )
    }

    fn unmanaged_filter_source_paths_live(&mut self, query: &str) -> Vec<PathBuf> {
        self.unmanaged_filter_source_paths_with_limits_and_options(
            query,
            LIVE_FILTER_INITIAL_UNMANAGED_INDEX_ENTRIES,
            LIVE_FILTER_UNMANAGED_INDEX_STEP,
            LIVE_FILTER_MAX_UNMANAGED_INDEX_ENTRIES,
            1,
            false,
        )
    }

    fn unmanaged_filter_source_paths_with_limits(
        &mut self,
        query: &str,
        initial_limit: usize,
        step: usize,
        max_limit: usize,
    ) -> Vec<PathBuf> {
        self.unmanaged_filter_source_paths_with_limits_and_options(
            query,
            initial_limit,
            step,
            max_limit,
            usize::MAX,
            true,
        )
    }

    fn unmanaged_filter_source_paths_with_limits_and_options(
        &mut self,
        query: &str,
        initial_limit: usize,
        step: usize,
        max_limit: usize,
        scan_rounds: usize,
        allow_fallback_scan: bool,
    ) -> Vec<PathBuf> {
        let initial = initial_limit.min(max_limit).max(1);
        self.scan_unmanaged_filter_index_to(initial);

        let normalized_query = query.trim().to_ascii_lowercase();
        if normalized_query.is_empty() {
            return self.unmanaged_filter_cache.entries.clone();
        }

        let mut current_limit = self.unmanaged_filter_cache.entries.len().max(initial);
        let mut rounds = 0usize;
        while !self.unmanaged_filter_cache.scan_complete
            && !self.unmanaged_index_has_match(&normalized_query)
            && current_limit < max_limit
            && rounds < scan_rounds
        {
            current_limit = (current_limit + step).min(max_limit);
            self.scan_unmanaged_filter_index_to(current_limit);
            rounds += 1;
        }

        if self.unmanaged_filter_cache.scan_complete
            || self.unmanaged_index_has_match(&normalized_query)
        {
            return self.unmanaged_filter_cache.entries.clone();
        }

        if allow_fallback_scan && current_limit >= max_limit {
            let mut source_paths = self.unmanaged_filter_cache.entries.clone();
            source_paths.extend(self.fallback_unmanaged_matches_outside_index(&normalized_query));
            return source_paths;
        }

        self.unmanaged_filter_cache.entries.clone()
    }

    fn unmanaged_index_has_match(&self, query: &str) -> bool {
        self.unmanaged_filter_cache
            .entries
            .iter()
            .any(|path| path.to_string_lossy().to_ascii_lowercase().contains(query))
    }

    fn fallback_unmanaged_matches_outside_index(&self, query: &str) -> Vec<PathBuf> {
        if query.is_empty() || self.unmanaged_filter_cache.scan_complete {
            return Vec::new();
        }

        let mut frontier = self.unmanaged_filter_cache.frontier.clone();
        let mut seen = self.unmanaged_filter_cache.seen.clone();
        let mut matches = Vec::new();

        while let Some(path) = frontier.pop_front() {
            if !seen.insert(path.clone()) {
                continue;
            }

            if Self::tree_entry_name_matches_query(path.as_path(), query) {
                matches.push(path.clone());
            }

            let directory = self.path_directory_state_for_view(&path, ListView::Unmanaged);
            if directory.can_expand {
                frontier.extend(self.read_children(&path));
            }
        }

        matches
    }

    fn scan_unmanaged_filter_index_to(&mut self, limit: usize) {
        self.ensure_unmanaged_filter_index_seeded();
        if self.unmanaged_filter_cache.scan_complete {
            return;
        }

        while self.unmanaged_filter_cache.entries.len() < limit {
            let Some(path) = self.unmanaged_filter_cache.frontier.pop_front() else {
                self.unmanaged_filter_cache.scan_complete = true;
                break;
            };
            if !self.unmanaged_filter_cache.seen.insert(path.clone()) {
                continue;
            }

            self.unmanaged_filter_cache.entries.push(path.clone());
            let directory = self.path_directory_state_for_view(&path, ListView::Unmanaged);
            if directory.can_expand {
                self.unmanaged_filter_cache
                    .frontier
                    .extend(self.read_children(&path));
            }
        }

        if self.unmanaged_filter_cache.frontier.is_empty() {
            self.unmanaged_filter_cache.scan_complete = true;
        }
    }

    fn ensure_unmanaged_filter_index_seeded(&mut self) {
        if self.unmanaged_filter_cache.initialized {
            return;
        }

        let mut roots: Vec<PathBuf> = self
            .unmanaged_entries
            .iter()
            .filter(|path| self.is_visible_in_unmanaged_view(path.as_path()))
            .cloned()
            .collect();

        if roots.iter().any(|path| path == Path::new(".")) {
            roots.retain(|path| path != Path::new("."));
            roots.extend(self.read_children(Path::new(".")));
        }
        roots.sort_by(|a, b| a.to_string_lossy().cmp(&b.to_string_lossy()));

        self.unmanaged_filter_cache.frontier = roots.into_iter().collect();
        self.unmanaged_filter_cache.initialized = true;
        if self.unmanaged_filter_cache.frontier.is_empty() {
            self.unmanaged_filter_cache.scan_complete = true;
        }
    }

    fn push_visible_recursive(
        &self,
        path: &Path,
        depth: usize,
        out: &mut Vec<VisibleEntry>,
        seen: &mut HashSet<PathBuf>,
        force_expand: bool,
    ) {
        if !seen.insert(path.to_path_buf()) {
            return;
        }

        let directory = self.path_directory_state_for_view(path, self.view);
        let is_dir = directory.is_dir;
        out.push(VisibleEntry {
            path: path.to_path_buf(),
            depth,
            is_dir,
            can_expand: directory.can_expand,
            is_symlink: directory.is_symlink,
        });

        if !directory.can_expand || (!force_expand && !self.expanded_dirs.contains(path)) {
            return;
        }

        for child in self.read_children(path) {
            self.push_visible_recursive(&child, depth + 1, out, seen, force_expand);
        }
    }

    fn push_unmanaged_visible_entries(&self, out: &mut Vec<VisibleEntry>, force_expand: bool) {
        let nodes = self.unmanaged_tree_nodes();

        if nodes.is_empty() {
            return;
        }

        let mut children: BTreeMap<PathBuf, Vec<PathBuf>> = BTreeMap::new();
        let mut roots = Vec::new();

        for node in &nodes {
            let parent = node.parent().map(Path::to_path_buf);
            if let Some(parent) = parent
                && !parent.as_os_str().is_empty()
                && nodes.contains(&parent)
            {
                children.entry(parent).or_default().push(node.clone());
            } else {
                roots.push(node.clone());
            }
        }

        for siblings in children.values_mut() {
            siblings.sort_by(|a, b| a.to_string_lossy().cmp(&b.to_string_lossy()));
        }
        roots.sort_by(|a, b| a.to_string_lossy().cmp(&b.to_string_lossy()));

        let mut seen = HashSet::new();
        for root in &roots {
            self.push_unmanaged_visible_recursive(root, 0, out, &children, &mut seen, force_expand);
        }
    }

    fn unmanaged_tree_nodes(&self) -> BTreeSet<PathBuf> {
        let mut nodes = BTreeSet::new();

        for unmanaged in &self.unmanaged_entries {
            if unmanaged.as_os_str().is_empty() || unmanaged == Path::new(".") {
                continue;
            }

            let mut current = unmanaged.clone();
            loop {
                if current.as_os_str().is_empty() {
                    break;
                }
                nodes.insert(current.clone());

                let Some(parent) = current.parent() else {
                    break;
                };
                if parent.as_os_str().is_empty() {
                    break;
                }
                current = parent.to_path_buf();
            }
        }

        nodes
    }

    fn push_unmanaged_visible_recursive(
        &self,
        path: &Path,
        depth: usize,
        out: &mut Vec<VisibleEntry>,
        children: &BTreeMap<PathBuf, Vec<PathBuf>>,
        seen: &mut HashSet<PathBuf>,
        force_expand: bool,
    ) {
        if !seen.insert(path.to_path_buf()) {
            return;
        }

        let virtual_children = children.get(path).cloned().unwrap_or_default();
        let directory = Self::directory_state_with_base(path, &self.working_dir);
        let is_dir = !virtual_children.is_empty() || directory.is_dir;
        let can_expand = !virtual_children.is_empty() || directory.can_expand;
        out.push(VisibleEntry {
            path: path.to_path_buf(),
            depth,
            is_dir,
            can_expand,
            is_symlink: directory.is_symlink,
        });

        if !can_expand || (!force_expand && !self.expanded_dirs.contains(path)) {
            return;
        }

        let mut child_paths = virtual_children;
        if directory.can_expand {
            child_paths.extend(self.read_children(path));
        }
        child_paths.sort_by(|a, b| a.to_string_lossy().cmp(&b.to_string_lossy()));
        child_paths.dedup();

        for child in child_paths {
            self.push_unmanaged_visible_recursive(
                &child,
                depth + 1,
                out,
                children,
                seen,
                force_expand,
            );
        }
    }

    fn push_managed_visible_entries(&self, out: &mut Vec<VisibleEntry>, force_expand: bool) {
        let nodes = self.managed_tree_nodes();

        if nodes.is_empty() {
            return;
        }

        let mut children: BTreeMap<PathBuf, Vec<PathBuf>> = BTreeMap::new();
        let mut roots = Vec::new();

        for node in &nodes {
            let parent = node.parent().map(Path::to_path_buf);
            if let Some(parent) = parent
                && !parent.as_os_str().is_empty()
                && nodes.contains(&parent)
            {
                children.entry(parent).or_default().push(node.clone());
            } else {
                roots.push(node.clone());
            }
        }

        for siblings in children.values_mut() {
            siblings.sort_by(|a, b| a.to_string_lossy().cmp(&b.to_string_lossy()));
        }
        roots.sort_by(|a, b| a.to_string_lossy().cmp(&b.to_string_lossy()));

        let mut seen = HashSet::new();
        for root in &roots {
            self.push_managed_visible_recursive(root, 0, out, &children, &mut seen, force_expand);
        }
    }

    fn managed_tree_nodes(&self) -> BTreeSet<PathBuf> {
        let mut nodes = BTreeSet::new();

        for managed in &self.managed_entries {
            if managed.as_os_str().is_empty() {
                continue;
            }

            let mut current = managed.clone();
            loop {
                if current.as_os_str().is_empty() {
                    break;
                }
                nodes.insert(current.clone());

                let Some(parent) = current.parent() else {
                    break;
                };
                if parent.as_os_str().is_empty() {
                    break;
                }
                current = parent.to_path_buf();
            }
        }

        nodes
    }

    fn push_managed_visible_recursive(
        &self,
        path: &Path,
        depth: usize,
        out: &mut Vec<VisibleEntry>,
        children: &BTreeMap<PathBuf, Vec<PathBuf>>,
        seen: &mut HashSet<PathBuf>,
        force_expand: bool,
    ) {
        if !seen.insert(path.to_path_buf()) {
            return;
        }

        let has_children = children
            .get(path)
            .is_some_and(|entries| !entries.is_empty());
        let directory = Self::directory_state_with_base(path, &self.home_dir);
        let is_dir = has_children || directory.is_dir;
        let can_expand = has_children || directory.can_expand;
        out.push(VisibleEntry {
            path: path.to_path_buf(),
            depth,
            is_dir,
            can_expand,
            is_symlink: directory.is_symlink,
        });

        if !can_expand || (!force_expand && !self.expanded_dirs.contains(path)) {
            return;
        }

        if let Some(child_paths) = children.get(path) {
            for child in child_paths {
                self.push_managed_visible_recursive(
                    child,
                    depth + 1,
                    out,
                    children,
                    seen,
                    force_expand,
                );
            }
        }
    }

    fn push_source_visible_entries(&self, out: &mut Vec<VisibleEntry>, force_expand: bool) {
        let nodes = self.source_tree_nodes();
        if nodes.is_empty() {
            return;
        }

        let mut children: BTreeMap<PathBuf, Vec<PathBuf>> = BTreeMap::new();
        let mut roots = Vec::new();
        for node in &nodes {
            let parent = node.parent().map(Path::to_path_buf);
            if let Some(parent) = parent
                && !parent.as_os_str().is_empty()
                && nodes.contains(&parent)
            {
                children.entry(parent).or_default().push(node.clone());
            } else {
                roots.push(node.clone());
            }
        }
        for siblings in children.values_mut() {
            siblings.sort_by(|a, b| a.to_string_lossy().cmp(&b.to_string_lossy()));
        }
        roots.sort_by(|a, b| a.to_string_lossy().cmp(&b.to_string_lossy()));

        let mut seen = HashSet::new();
        for root in &roots {
            self.push_source_visible_recursive(root, 0, out, &children, &mut seen, force_expand);
        }
    }

    fn source_tree_nodes(&self) -> BTreeSet<PathBuf> {
        let mut nodes = BTreeSet::new();
        for source in &self.source_entries {
            if source.as_os_str().is_empty() {
                continue;
            }
            let mut current = source.clone();
            loop {
                if current.as_os_str().is_empty() {
                    break;
                }
                nodes.insert(current.clone());
                let Some(parent) = current.parent() else {
                    break;
                };
                if parent.as_os_str().is_empty() {
                    break;
                }
                current = parent.to_path_buf();
            }
        }
        nodes
    }

    fn push_source_visible_recursive(
        &self,
        path: &Path,
        depth: usize,
        out: &mut Vec<VisibleEntry>,
        children: &BTreeMap<PathBuf, Vec<PathBuf>>,
        seen: &mut HashSet<PathBuf>,
        force_expand: bool,
    ) {
        if !seen.insert(path.to_path_buf()) {
            return;
        }
        let has_children = children
            .get(path)
            .is_some_and(|entries| !entries.is_empty());
        let directory = self.path_directory_state_for_view(path, ListView::Source);
        let is_dir = has_children || directory.is_dir;
        let can_expand = has_children || directory.can_expand;
        out.push(VisibleEntry {
            path: path.to_path_buf(),
            depth,
            is_dir,
            can_expand,
            is_symlink: directory.is_symlink,
        });
        if !can_expand || (!force_expand && !self.expanded_dirs.contains(path)) {
            return;
        }
        if let Some(child_paths) = children.get(path) {
            for child in child_paths {
                self.push_source_visible_recursive(
                    child,
                    depth + 1,
                    out,
                    children,
                    seen,
                    force_expand,
                );
            }
        }
    }

    fn read_children(&self, parent: &Path) -> Vec<PathBuf> {
        let abs_parent = Self::resolve_with_base(parent, &self.working_dir);
        let Ok(read_dir) = fs::read_dir(abs_parent) else {
            return Vec::new();
        };

        let mut children: Vec<PathBuf> = read_dir
            .filter_map(Result::ok)
            .map(|entry| entry.file_name())
            .map(|name| {
                if parent == Path::new(".") {
                    PathBuf::from(name)
                } else {
                    PathBuf::from(parent).join(name)
                }
            })
            .filter(|path| self.is_visible_in_unmanaged_view(path.as_path()))
            .collect();

        children.sort_by(|a, b| a.to_string_lossy().cmp(&b.to_string_lossy()));
        children
    }

    fn managed_absolute_path(&self, managed: &Path) -> PathBuf {
        Self::resolve_with_base(managed, &self.home_dir)
    }

    fn is_exact_managed_path_in_working_dir(&self, path: &Path) -> bool {
        self.managed_entries.iter().any(|managed| {
            let managed_abs = self.managed_absolute_path(managed.as_path());
            managed_abs.starts_with(&self.working_dir) && managed_abs == path
        })
    }

    fn is_visible_in_unmanaged_view(&self, path: &Path) -> bool {
        !self.is_excluded_unmanaged_path(path)
    }

    fn is_excluded_unmanaged_path(&self, path: &Path) -> bool {
        let abs = Self::resolve_with_base(path, &self.working_dir);
        self.is_exact_managed_path_in_working_dir(&abs)
    }

    fn format_visible_entry(&self, entry: &VisibleEntry) -> String {
        let marked = self.marked_entries.contains(&entry.path);
        if self.view == ListView::Status {
            let mut label = String::new();
            label.push_str(if marked { "* " } else { "  " });
            if let Some(status) = self.status_entries.iter().find(|s| s.path == entry.path) {
                label.push(status.actual_vs_state.as_symbol());
                label.push(status.actual_vs_target.as_symbol());
            } else {
                label.push(' ');
                label.push(' ');
            }
            label.push(' ');
            label.push_str(&compact_path_for_list(&entry.path));
            if entry.is_symlink {
                label.push('@');
            }
            if entry.is_dir {
                label.push('/');
            }
            return label;
        }

        let mut label = String::new();
        label.push_str(&"  ".repeat(entry.depth));
        label.push_str(if marked { "* " } else { "  " });

        let expanded = self.expanded_dirs.contains(&entry.path);
        let node_marker = if entry.is_symlink && entry.is_dir {
            "[L]"
        } else if entry.is_symlink {
            " L "
        } else if entry.can_expand {
            if expanded { "[-]" } else { "[+]" }
        } else if entry.is_dir {
            "[ ]"
        } else {
            "   "
        };
        label.push_str(node_marker);
        label.push(' ');

        let name = if entry.depth == 0 {
            compact_path_for_list(&entry.path)
        } else {
            entry.path.file_name().and_then(|n| n.to_str()).map_or_else(
                || entry.path.display().to_string(),
                std::string::ToString::to_string,
            )
        };

        label.push_str(&name);
        if entry.is_symlink {
            label.push('@');
        }
        if entry.is_dir {
            label.push('/');
        }
        if self.view == ListView::Source {
            let attrs = source_attr_markers(&entry.path);
            if !attrs.is_empty() {
                label.push(' ');
                label.push_str(&attrs);
            }
        }

        label
    }

    fn path_is_directory(&self, path: &Path) -> bool {
        self.path_directory_state_for_view(path, self.view).is_dir
    }

    fn path_directory_state_for_view(&self, path: &Path, view: ListView) -> DirectoryState {
        let abs = self.resolve_path_for_view(path, view);
        Self::directory_state_for_absolute(&abs)
    }

    fn directory_state_with_base(path: &Path, base: &Path) -> DirectoryState {
        let abs = Self::resolve_with_base(path, base);
        Self::directory_state_for_absolute(&abs)
    }

    fn directory_state_for_absolute(abs: &Path) -> DirectoryState {
        let Ok(meta) = fs::symlink_metadata(abs) else {
            return DirectoryState::default();
        };
        let kind = meta.file_type();
        if kind.is_dir() {
            return DirectoryState {
                is_dir: true,
                can_expand: true,
                is_symlink: false,
            };
        }

        if kind.is_symlink() && fs::metadata(abs).is_ok_and(|target| target.is_dir()) {
            return DirectoryState {
                is_dir: true,
                can_expand: false,
                is_symlink: true,
            };
        }

        if kind.is_symlink() {
            return DirectoryState {
                is_dir: false,
                can_expand: false,
                is_symlink: true,
            };
        }

        DirectoryState::default()
    }

    fn resolve_path_for_view(&self, path: &Path, view: ListView) -> PathBuf {
        let base = match view {
            ListView::Status | ListView::Managed => &self.home_dir,
            ListView::Unmanaged => &self.working_dir,
            ListView::Source => self.config.source_dir.as_ref().unwrap_or(&self.working_dir),
        };
        Self::resolve_with_base(path, base)
    }

    fn resolve_with_base(path: &Path, base: &Path) -> PathBuf {
        if path.is_absolute() {
            path.to_path_buf()
        } else {
            base.join(path)
        }
    }

    pub fn current_filter_summary(&self) -> Option<String> {
        if self.list_filter.trim().is_empty() {
            return None;
        }
        Some(format!("{} matched", self.current_len()))
    }

    pub fn debug_context_text(&self) -> String {
        let selected = self
            .selected_absolute_path()
            .map_or_else(|| "(none)".to_string(), |path| path.display().to_string());
        let notice = self
            .latest_notice()
            .map_or_else(|| "(none)".to_string(), |notice| notice.message.clone());
        format!(
            "chezmoi-tui debug context\n\nview: {}\nfocus: {:?}\nlayout: {:?}\nbase: {}\nselected: {}\nitems: {}\nmarked: {}\nbusy: {}\nlatest notice: {}\nconfig file: {}\nsource: {}\ndestination: {}\nworking dir: {}\n",
            self.view.title(),
            self.focus,
            self.layout_mode,
            self.view_context_text(),
            selected,
            self.current_len(),
            self.marked_count(),
            self.busy_message().unwrap_or("no"),
            notice,
            self.config
                .config_file
                .as_ref()
                .map_or_else(|| "(none)".to_string(), |p| p.display().to_string()),
            self.config
                .source_dir
                .as_ref()
                .map_or_else(|| "(runtime)".to_string(), |p| p.display().to_string()),
            self.config.destination_dir.as_ref().map_or_else(
                || self.home_dir.display().to_string(),
                |p| p.display().to_string()
            ),
            self.working_dir.display(),
        )
    }

    fn collapse_tree(&mut self, dir: &Path) {
        let targets: Vec<PathBuf> = self
            .expanded_dirs
            .iter()
            .filter(|p| p.starts_with(dir))
            .cloned()
            .collect();
        for target in targets {
            self.expanded_dirs.remove(&target);
        }
    }

    fn clear_staged_list_filter(&mut self) {
        self.staged_list_filter = None;
        self.staged_filter_updated_at = None;
    }

    fn invalidate_unmanaged_filter_index(&mut self) {
        self.unmanaged_filter_cache = UnmanagedFilterCache::default();
    }
}

fn source_attr_markers(path: &Path) -> String {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("");
    let mut attrs = Vec::new();
    if name.ends_with(".tmpl") {
        attrs.push("tmpl");
    }
    for component in path
        .components()
        .filter_map(|component| component.as_os_str().to_str())
    {
        for part in component.split('_') {
            match part {
                "private" => attrs.push("priv"),
                "executable" => attrs.push("exec"),
                "encrypted" => attrs.push("enc"),
                _ => {}
            }
        }
    }
    attrs.sort_unstable();
    attrs.dedup();
    if attrs.is_empty() {
        String::new()
    } else {
        format!("{{{}}}", attrs.join(","))
    }
}

fn compact_path_for_list(path: &Path) -> String {
    const MAX: usize = 96;
    let value = path.display().to_string();
    if value.chars().count() <= MAX {
        return value;
    }
    let file = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    if !file.is_empty() && file.chars().count() + 4 < MAX {
        let keep_prefix = MAX - file.chars().count() - 4;
        let prefix: String = value.chars().take(keep_prefix).collect();
        return format!("{prefix}.../{file}");
    }
    let head = MAX / 2;
    let tail = MAX.saturating_sub(head + 3);
    let prefix: String = value.chars().take(head).collect();
    let suffix: String = value
        .chars()
        .rev()
        .take(tail)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    format!("{prefix}...{suffix}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::ChangeKind;
    use std::path::Path;
    use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

    #[test]
    fn busy_task_counter_tracks_multiple_in_flight_tasks() {
        let mut app = App::new(AppConfig::default());

        assert!(!app.is_busy());
        app.begin_busy_task();
        app.begin_busy_task();
        assert!(app.is_busy());
        assert_eq!(app.busy_task_count(), 2);

        app.finish_busy_task();
        assert!(app.is_busy());
        assert_eq!(app.busy_task_count(), 1);

        app.finish_busy_task();
        assert!(!app.is_busy());
        assert_eq!(app.busy_task_count(), 0);

        app.finish_busy_task();
        assert!(!app.is_busy());
        assert_eq!(app.busy_task_count(), 0);
    }

    #[test]
    fn latest_notice_can_be_set_and_cleared() {
        let mut app = App::new(AppConfig::default());

        app.set_error_notice("boom");
        let notice = app.latest_notice().expect("notice");
        assert_eq!(notice.tone, NoticeTone::Error);
        assert_eq!(notice.message, "boom");

        app.clear_notice();
        assert!(app.latest_notice().is_none());
    }

    #[test]
    fn status_selection_returns_path() {
        let mut app = App::new(AppConfig::default());
        app.status_entries = vec![StatusEntry {
            path: PathBuf::from(".zshrc"),
            actual_vs_state: ChangeKind::None,
            actual_vs_target: ChangeKind::Modified,
        }];
        app.rebuild_visible_entries();
        assert_eq!(app.selected_path(), Some(PathBuf::from(".zshrc")));
    }

    #[test]
    fn status_items_include_change_symbols() {
        let mut app = App::new(AppConfig::default());
        app.status_entries = vec![StatusEntry {
            path: PathBuf::from(".zshrc"),
            actual_vs_state: ChangeKind::Modified,
            actual_vs_target: ChangeKind::Modified,
        }];
        app.rebuild_visible_entries();
        let items = app.current_items();
        assert_eq!(items[0], "  MM .zshrc");
    }

    #[test]
    fn selected_absolute_path_uses_home_dir_for_status() {
        let temp_root = std::env::temp_dir().join(format!(
            "chezmoi_tui_abs_{}_{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("time")
                .as_nanos()
        ));
        let mut app = App::new(AppConfig::default());
        app.home_dir = temp_root.clone();
        app.status_entries = vec![StatusEntry {
            path: PathBuf::from(".zshrc"),
            actual_vs_state: ChangeKind::None,
            actual_vs_target: ChangeKind::Modified,
        }];
        app.rebuild_visible_entries();

        assert_eq!(app.selected_absolute_path(), Some(temp_root.join(".zshrc")));
    }

    #[test]
    fn selected_absolute_path_uses_working_dir_for_unmanaged() {
        let temp_root = std::env::temp_dir().join(format!(
            "chezmoi_tui_wd_{}_{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("time")
                .as_nanos()
        ));
        fs::create_dir_all(&temp_root).expect("create temp root");
        let mut app = App::new(AppConfig::default());
        app.working_dir = temp_root.clone();
        app.unmanaged_entries = vec![PathBuf::from(".zshrc")];
        app.switch_view(ListView::Unmanaged);
        app.rebuild_visible_entries();
        assert_eq!(app.selected_absolute_path(), Some(temp_root.join(".zshrc")));
        let _ = fs::remove_dir_all(temp_root);
    }

    #[test]
    fn selection_is_bounded() {
        let mut app = App::new(AppConfig::default());
        app.managed_entries = vec![PathBuf::from("a"), PathBuf::from("b")];
        app.switch_view(ListView::Managed);
        app.selected_index = 5;
        app.sync_selection_bounds();
        assert_eq!(app.selected_index, 1);
    }

    #[test]
    fn unmanaged_directory_can_expand_and_show_children() {
        let temp_root = std::env::temp_dir().join(format!(
            "chezmoi_tui_test_{}_{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("time")
                .as_nanos()
        ));
        let dir = temp_root.join(".config/nvim");
        fs::create_dir_all(&dir).expect("create dir");
        fs::write(dir.join("init.lua"), "set number").expect("write file");

        let mut app = App::new(AppConfig::default());
        app.working_dir = temp_root.clone();
        app.unmanaged_entries = vec![PathBuf::from(".config")];
        app.switch_view(ListView::Unmanaged);

        assert!(
            app.current_items()
                .iter()
                .any(|line| line.contains(".config/"))
        );

        let expanded = app.expand_selected_directory();
        assert!(expanded);
        assert!(
            app.current_items()
                .iter()
                .any(|line| line.contains("nvim/"))
        );

        let _ = fs::remove_dir_all(temp_root);
    }

    #[cfg(unix)]
    #[test]
    fn unmanaged_symlink_directory_is_shown_but_not_expandable() {
        use std::os::unix::fs::symlink;

        let temp_root = std::env::temp_dir().join(format!(
            "chezmoi_tui_symlink_dir_{}_{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("time")
                .as_nanos()
        ));
        let real_dir = temp_root.join("real");
        fs::create_dir_all(&real_dir).expect("create real dir");
        fs::write(real_dir.join("inside.txt"), "inside").expect("write inner file");
        symlink(&real_dir, temp_root.join("linkdir")).expect("create symlink dir");

        let mut app = App::new(AppConfig::default());
        app.working_dir = temp_root.clone();
        app.unmanaged_entries = vec![PathBuf::from("linkdir")];
        app.switch_view(ListView::Unmanaged);

        assert!(app.selected_is_directory());
        assert!(!app.expand_selected_directory());
        let items = app.current_items();
        assert!(
            items
                .iter()
                .any(|line| line.contains("[L]") && line.contains("linkdir@/"))
        );
        assert!(!items.iter().any(|line| line.contains("inside.txt")));

        let _ = fs::remove_dir_all(temp_root);
    }

    #[cfg(unix)]
    #[test]
    fn unmanaged_symlink_file_shows_link_marker_and_suffix() {
        use std::os::unix::fs::symlink;

        let temp_root = std::env::temp_dir().join(format!(
            "chezmoi_tui_symlink_file_{}_{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("time")
                .as_nanos()
        ));
        fs::create_dir_all(&temp_root).expect("create root");
        fs::write(temp_root.join("real.txt"), "hello").expect("write real file");
        symlink(temp_root.join("real.txt"), temp_root.join("link.txt"))
            .expect("create symlink file");

        let mut app = App::new(AppConfig::default());
        app.working_dir = temp_root.clone();
        app.unmanaged_entries = vec![PathBuf::from("link.txt")];
        app.switch_view(ListView::Unmanaged);

        let items = app.current_items();
        assert!(
            items
                .iter()
                .any(|line| line.contains(" L ") && line.contains("link.txt@"))
        );

        let _ = fs::remove_dir_all(temp_root);
    }

    #[test]
    fn unmanaged_tree_excludes_managed_children() {
        let temp_root = std::env::temp_dir().join(format!(
            "chezmoi_tui_unmanaged_filter_{}_{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("time")
                .as_nanos()
        ));
        let dir = temp_root.join(".config");
        fs::create_dir_all(&dir).expect("create dir");
        fs::write(dir.join("managed.lua"), "managed").expect("write managed");
        fs::write(dir.join("local.lua"), "local").expect("write unmanaged");

        let mut app = App::new(AppConfig::default());
        app.home_dir = temp_root.clone();
        app.working_dir = temp_root.clone();
        app.managed_entries = vec![PathBuf::from(".config/managed.lua")];
        app.unmanaged_entries = vec![PathBuf::from(".config")];
        app.switch_view(ListView::Unmanaged);

        assert!(app.expand_selected_directory());
        let items = app.current_items();
        assert!(items.iter().any(|line| line.contains("local.lua")));
        assert!(!items.iter().any(|line| line.contains("managed.lua")));

        let _ = fs::remove_dir_all(temp_root);
    }

    #[test]
    fn unmanaged_filter_ignores_managed_paths_outside_working_dir() {
        let temp_root = std::env::temp_dir().join(format!(
            "chezmoi_tui_unmanaged_scope_{}_{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("time")
                .as_nanos()
        ));
        let work = temp_root.join("dev/chezmoi-tui");
        fs::create_dir_all(&work).expect("create work dir");
        fs::write(work.join("local.txt"), "local").expect("write local file");

        let mut app = App::new(AppConfig::default());
        app.home_dir = temp_root.clone();
        app.working_dir = work.clone();
        app.managed_entries = vec![PathBuf::from("dev")];
        app.unmanaged_entries = vec![PathBuf::from("local.txt")];
        app.switch_view(ListView::Unmanaged);

        let items = app.current_items();
        assert!(items.iter().any(|line| line.contains("local.txt")));

        let _ = fs::remove_dir_all(temp_root);
    }

    #[test]
    fn unmanaged_filter_does_not_hide_all_when_working_dir_itself_is_managed() {
        let temp_root = std::env::temp_dir().join(format!(
            "chezmoi_tui_unmanaged_root_{}_{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("time")
                .as_nanos()
        ));
        let work = temp_root.join("dev/chezmoi-tui");
        fs::create_dir_all(&work).expect("create work dir");
        fs::write(work.join("Cargo.lock"), "lock").expect("write file");

        let mut app = App::new(AppConfig::default());
        app.home_dir = temp_root.clone();
        app.working_dir = work.clone();
        app.managed_entries = vec![PathBuf::from("dev/chezmoi-tui")];
        app.unmanaged_entries = vec![PathBuf::from("Cargo.lock")];
        app.switch_view(ListView::Unmanaged);

        let items = app.current_items();
        assert!(items.iter().any(|line| line.contains("Cargo.lock")));

        let _ = fs::remove_dir_all(temp_root);
    }

    #[test]
    fn unmanaged_root_placeholder_is_expanded_without_dot_prefix() {
        let temp_root = std::env::temp_dir().join(format!(
            "chezmoi_tui_unmanaged_dot_root_{}_{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("time")
                .as_nanos()
        ));
        fs::create_dir_all(temp_root.join(".config")).expect("create child dir");
        fs::write(temp_root.join("alpha.txt"), "alpha").expect("write child file");

        let mut app = App::new(AppConfig::default());
        app.working_dir = temp_root.clone();
        app.unmanaged_entries = vec![PathBuf::from(".")];
        app.switch_view(ListView::Unmanaged);

        let items = app.current_items();
        assert!(items.iter().any(|line| line.contains("alpha.txt")));
        assert!(items.iter().any(|line| line.contains(".config/")));
        assert!(!items.iter().any(|line| line.contains("./")));

        let _ = fs::remove_dir_all(temp_root);
    }

    #[test]
    fn unmanaged_view_does_not_exclude_without_configured_paths() {
        let temp_root = std::env::temp_dir().join(format!(
            "chezmoi_tui_unmanaged_no_default_exclude_{}_{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("time")
                .as_nanos()
        ));
        fs::create_dir_all(temp_root.join(".cache")).expect("create cache dir");
        fs::create_dir_all(temp_root.join(".codex/skills")).expect("create codex dir");
        fs::write(temp_root.join(".codex/skills/SKILL.md"), "skill").expect("write skill");

        let mut app = App::new(AppConfig::default());
        app.working_dir = temp_root.clone();
        app.unmanaged_entries = vec![PathBuf::from(".cache"), PathBuf::from(".codex")];
        app.switch_view(ListView::Unmanaged);

        let items = app.current_items();
        assert!(items.iter().any(|line| line.contains(".cache/")));
        assert!(items.iter().any(|line| line.contains(".codex/")));

        app.apply_list_filter_immediately("skill".to_string());
        let filtered = app.current_items();
        assert!(filtered.iter().any(|line| line.contains("SKILL.md")));

        let _ = fs::remove_dir_all(temp_root);
    }

    #[test]
    fn unmanaged_filter_index_uses_breadth_first_scan_order() {
        let temp_root = std::env::temp_dir().join(format!(
            "chezmoi_tui_unmanaged_bfs_{}_{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("time")
                .as_nanos()
        ));
        fs::create_dir_all(temp_root.join("a/sub")).expect("create a/sub");
        fs::create_dir_all(temp_root.join("b")).expect("create b");
        fs::write(temp_root.join("a/sub/deep.txt"), "deep").expect("write deep");
        fs::write(temp_root.join("b/root.txt"), "root").expect("write root");

        let mut app = App::new(AppConfig::default());
        app.working_dir = temp_root.clone();
        app.unmanaged_entries = vec![PathBuf::from(".")];
        app.switch_view(ListView::Unmanaged);

        let indexed = app.unmanaged_filter_source_paths_with_limits("", 3, 3, 9);
        assert_eq!(indexed[0], PathBuf::from("a"));
        assert_eq!(indexed[1], PathBuf::from("b"));
        assert_eq!(indexed[2], PathBuf::from("a/sub"));

        let _ = fs::remove_dir_all(temp_root);
    }

    #[test]
    fn unmanaged_filter_index_expands_limit_until_query_match() {
        let temp_root = std::env::temp_dir().join(format!(
            "chezmoi_tui_unmanaged_expand_{}_{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("time")
                .as_nanos()
        ));
        fs::create_dir_all(temp_root.join("a")).expect("create a");
        fs::create_dir_all(temp_root.join("b")).expect("create b");
        fs::create_dir_all(temp_root.join("c")).expect("create c");
        fs::write(temp_root.join("a/one.txt"), "one").expect("write one");
        fs::write(temp_root.join("b/two.txt"), "two").expect("write two");
        fs::write(temp_root.join("c/target-skill.md"), "target").expect("write target");

        let mut app = App::new(AppConfig::default());
        app.working_dir = temp_root.clone();
        app.unmanaged_entries = vec![PathBuf::from(".")];
        app.switch_view(ListView::Unmanaged);

        let indexed = app.unmanaged_filter_source_paths_with_limits("target-skill", 2, 2, 8);
        assert!(
            indexed
                .iter()
                .any(|path| path.ends_with(Path::new("c/target-skill.md")))
        );

        let _ = fs::remove_dir_all(temp_root);
    }

    #[test]
    fn unmanaged_filter_index_falls_back_when_max_limit_is_reached() {
        let temp_root = std::env::temp_dir().join(format!(
            "chezmoi_tui_unmanaged_fallback_{}_{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("time")
                .as_nanos()
        ));
        fs::create_dir_all(temp_root.join("a")).expect("create a");
        fs::create_dir_all(temp_root.join("b")).expect("create b");
        fs::create_dir_all(temp_root.join("c")).expect("create c");
        fs::write(temp_root.join("a/one.txt"), "one").expect("write one");
        fs::write(temp_root.join("b/two.txt"), "two").expect("write two");
        fs::write(temp_root.join("c/target-skill.md"), "target").expect("write target");

        let mut app = App::new(AppConfig::default());
        app.working_dir = temp_root.clone();
        app.unmanaged_entries = vec![PathBuf::from(".")];
        app.switch_view(ListView::Unmanaged);

        let indexed = app.unmanaged_filter_source_paths_with_limits("target-skill", 1, 1, 2);
        assert!(
            indexed
                .iter()
                .any(|path| path.ends_with(Path::new("c/target-skill.md")))
        );

        let _ = fs::remove_dir_all(temp_root);
    }

    #[test]
    fn list_filter_directory_name_match_does_not_expand_children_without_descendant_match() {
        let temp_root = std::env::temp_dir().join(format!(
            "chezmoi_tui_filter_dir_name_only_{}_{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("time")
                .as_nanos()
        ));
        fs::create_dir_all(temp_root.join("skills")).expect("create skills dir");
        fs::write(temp_root.join("skills/guide.md"), "guide").expect("write file");

        let mut app = App::new(AppConfig::default());
        app.working_dir = temp_root.clone();
        app.unmanaged_entries = vec![PathBuf::from("skills")];
        app.switch_view(ListView::Unmanaged);

        app.apply_list_filter_immediately("skills".to_string());
        let items = app.current_items();
        assert!(items.iter().any(|line| line.contains("skills/")));
        assert!(!items.iter().any(|line| line.contains("guide.md")));

        let _ = fs::remove_dir_all(temp_root);
    }

    #[test]
    fn list_filter_file_name_match_expands_ancestor_directories() {
        let temp_root = std::env::temp_dir().join(format!(
            "chezmoi_tui_filter_file_name_{}_{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("time")
                .as_nanos()
        ));
        fs::create_dir_all(temp_root.join("skills")).expect("create skills dir");
        fs::write(temp_root.join("skills/SKILL.md"), "skill").expect("write file");

        let mut app = App::new(AppConfig::default());
        app.working_dir = temp_root.clone();
        app.unmanaged_entries = vec![PathBuf::from("skills")];
        app.switch_view(ListView::Unmanaged);

        app.apply_list_filter_immediately("skill.md".to_string());
        let items = app.current_items();
        assert!(items.iter().any(|line| line.contains("skills/")));
        assert!(items.iter().any(|line| line.contains("SKILL.md")));

        let _ = fs::remove_dir_all(temp_root);
    }

    #[test]
    fn managed_view_is_hierarchical_and_expandable() {
        let mut app = App::new(AppConfig::default());
        app.managed_entries = vec![
            PathBuf::from("dev"),
            PathBuf::from("dev/chezmoi-tui"),
            PathBuf::from("dev/chezmoi-tui/Cargo.toml"),
        ];
        app.switch_view(ListView::Managed);

        let first = app.current_items();
        assert!(first.iter().any(|line| line.contains("dev/")));
        assert!(!first.iter().any(|line| line.contains("Cargo.toml")));

        assert!(app.expand_selected_directory());
        let second = app.current_items();
        assert!(second.iter().any(|line| line.contains("chezmoi-tui/")));
        assert!(!second.iter().any(|line| line.contains("Cargo.toml")));

        app.select_next();
        assert!(app.expand_selected_directory());
        let third = app.current_items();
        assert!(third.iter().any(|line| line.contains("Cargo.toml")));
    }

    #[test]
    fn managed_view_keeps_directory_only_branch_without_managed_files() {
        let temp_root = std::env::temp_dir().join(format!(
            "chezmoi_tui_managed_empty_branch_{}_{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("time")
                .as_nanos()
        ));
        fs::create_dir_all(temp_root.join("dev/project/.worktrees/diff-view-ansi"))
            .expect("create dir");

        let mut app = App::new(AppConfig::default());
        app.home_dir = temp_root.clone();
        app.managed_entries = vec![
            PathBuf::from("dev"),
            PathBuf::from("dev/project"),
            PathBuf::from("dev/project/.worktrees"),
            PathBuf::from("dev/project/.worktrees/diff-view-ansi"),
        ];
        app.switch_view(ListView::Managed);

        let first = app.current_items();
        assert!(first.iter().any(|line| line.contains("dev/")));

        assert!(app.expand_selected_directory());
        let second = app.current_items();
        assert!(second.iter().any(|line| line.contains("project/")));

        app.select_next();
        assert!(app.expand_selected_directory());
        let third = app.current_items();
        assert!(third.iter().any(|line| line.contains(".worktrees/")));

        let _ = fs::remove_dir_all(temp_root);
    }

    #[test]
    fn managed_view_keeps_directory_only_and_file_backed_branches() {
        let temp_root = std::env::temp_dir().join(format!(
            "chezmoi_tui_managed_keep_branch_{}_{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("time")
                .as_nanos()
        ));
        fs::create_dir_all(temp_root.join("dev/project/.worktrees/diff-view-ansi"))
            .expect("create empty branch");
        fs::create_dir_all(temp_root.join("dev/project/keep")).expect("create keep dir");
        fs::write(temp_root.join("dev/project/keep/Cargo.toml"), "package")
            .expect("write managed file");

        let mut app = App::new(AppConfig::default());
        app.home_dir = temp_root.clone();
        app.managed_entries = vec![
            PathBuf::from("dev"),
            PathBuf::from("dev/project"),
            PathBuf::from("dev/project/.worktrees"),
            PathBuf::from("dev/project/.worktrees/diff-view-ansi"),
            PathBuf::from("dev/project/keep"),
            PathBuf::from("dev/project/keep/Cargo.toml"),
        ];
        app.switch_view(ListView::Managed);

        let first = app.current_items();
        assert!(first.iter().any(|line| line.contains("dev/")));

        assert!(app.expand_selected_directory());
        let second = app.current_items();
        assert!(second.iter().any(|line| line.contains("project/")));

        app.select_next();
        assert!(app.expand_selected_directory());
        let third = app.current_items();
        assert!(third.iter().any(|line| line.contains("keep/")));
        assert!(third.iter().any(|line| line.contains(".worktrees/")));

        let _ = fs::remove_dir_all(temp_root);
    }

    #[test]
    fn detail_scroll_is_clamped() {
        let mut app = App::new(AppConfig::default());
        app.set_detail_preview(
            Path::new(".config/test.txt"),
            PreviewOrigin::Destination,
            "a\nb\nc\nd\ne".to_string(),
        );
        assert!(app.scroll_detail_down(2));
        assert_eq!(app.detail_scroll, 2);
        assert!(app.scroll_detail_down(100));
        assert_eq!(app.detail_scroll, 4);
        assert!(!app.scroll_detail_down(1));
        assert!(app.scroll_detail_up(3));
        assert_eq!(app.detail_scroll, 1);
        assert!(app.scroll_detail_up(10));
        assert_eq!(app.detail_scroll, 0);
        assert!(!app.scroll_detail_up(1));
    }

    #[test]
    fn log_scroll_moves_with_up_and_down() {
        let mut app = App::new(AppConfig::default());
        assert!(!app.scroll_log_down(1));
        assert!(app.scroll_log_up(5));
        assert_eq!(app.log_tail_offset, 5);
        assert!(app.scroll_log_down(2));
        assert_eq!(app.log_tail_offset, 3);
        assert!(app.scroll_log_down(10));
        assert_eq!(app.log_tail_offset, 0);
        assert!(!app.scroll_log_down(1));
    }

    #[test]
    fn log_preserves_manual_scroll_position_when_new_entries_arrive() {
        let mut app = App::new(AppConfig::default());
        app.scroll_log_up(4);
        app.log("line-1".to_string());
        app.log("line-2".to_string());
        assert_eq!(app.log_tail_offset, 6);
    }

    #[test]
    fn clear_detail_resets_preview_state() {
        let mut app = App::new(AppConfig::default());
        app.set_detail_preview(
            Path::new(".config/test.txt"),
            PreviewOrigin::Destination,
            "hello".to_string(),
        );
        assert!(!app.detail_text.is_empty());
        assert!(app.detail_target.is_some());
        app.clear_detail();
        assert!(app.detail_text.is_empty());
        assert!(app.detail_target.is_none());
        assert_eq!(app.detail_scroll, 0);
    }

    #[test]
    fn selected_is_managed_checks_against_managed_entries() {
        let mut app = App::new(AppConfig::default());
        app.status_entries = vec![StatusEntry {
            path: PathBuf::from(".zshrc"),
            actual_vs_state: ChangeKind::None,
            actual_vs_target: ChangeKind::Modified,
        }];
        app.managed_entries = vec![PathBuf::from(".zshrc")];
        app.rebuild_visible_entries();
        assert!(app.selected_is_managed());

        app.managed_entries = vec![PathBuf::from(".gitconfig")];
        assert!(!app.selected_is_managed());
    }

    #[test]
    fn view_context_text_uses_view_base_paths() {
        let config = AppConfig {
            destination_dir: Some(PathBuf::from("/tmp/home")),
            source_dir: Some(PathBuf::from("/tmp/source")),
            working_dir: PathBuf::from("/tmp/work"),
            ..AppConfig::default()
        };
        let mut app = App::new(config);

        app.switch_view(ListView::Status);
        assert_eq!(app.view_context_text(), "dest=/tmp/home");
        app.switch_view(ListView::Managed);
        assert_eq!(app.view_context_text(), "dest=/tmp/home");
        app.switch_view(ListView::Unmanaged);
        assert_eq!(app.view_context_text(), "cwd=/tmp/work");
        app.switch_view(ListView::Source);
        assert_eq!(app.view_context_text(), "source=/tmp/source");
    }

    #[test]
    fn preview_title_includes_origin() {
        let mut app = App::new(AppConfig::default());
        app.set_detail_preview(
            Path::new(".zshrc"),
            PreviewOrigin::Destination,
            "content".to_string(),
        );
        assert_eq!(app.detail_title, "Preview(destination): .zshrc");
    }

    #[test]
    fn apply_plan_groups_status_entries_by_target_change_kind() {
        let mut app = App::new(AppConfig::default());
        app.status_entries = vec![
            StatusEntry {
                path: PathBuf::from("added"),
                actual_vs_state: ChangeKind::None,
                actual_vs_target: ChangeKind::Added,
            },
            StatusEntry {
                path: PathBuf::from("modified"),
                actual_vs_state: ChangeKind::None,
                actual_vs_target: ChangeKind::Modified,
            },
            StatusEntry {
                path: PathBuf::from("deleted"),
                actual_vs_state: ChangeKind::None,
                actual_vs_target: ChangeKind::Deleted,
            },
            StatusEntry {
                path: PathBuf::from("script"),
                actual_vs_state: ChangeKind::None,
                actual_vs_target: ChangeKind::Run,
            },
            StatusEntry {
                path: PathBuf::from("clean"),
                actual_vs_state: ChangeKind::None,
                actual_vs_target: ChangeKind::None,
            },
        ];

        app.open_apply_plan(ActionRequest {
            action: Action::Apply,
            target: None,
            chattr_attrs: None,
        });

        let ModalState::ApplyPlan { snapshot, .. } = &app.modal else {
            panic!("expected apply plan modal");
        };
        assert_eq!(snapshot.plan.total(), 4);
        assert_eq!(snapshot.plan.added, vec![PathBuf::from("added")]);
        assert_eq!(snapshot.plan.modified, vec![PathBuf::from("modified")]);
        assert_eq!(snapshot.plan.deleted, vec![PathBuf::from("deleted")]);
        assert_eq!(snapshot.plan.run, vec![PathBuf::from("script")]);
    }

    #[test]
    fn open_apply_plan_filters_status_entries_for_targeted_apply() {
        let temp_root = std::env::temp_dir().join(format!(
            "chezmoi_tui_apply_plan_{}_{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("time")
                .as_nanos()
        ));
        std::fs::create_dir_all(&temp_root).expect("create temp root");

        let mut app = App::new(AppConfig::default());
        app.home_dir = temp_root.clone();
        app.status_entries = vec![
            StatusEntry {
                path: PathBuf::from(".a"),
                actual_vs_state: ChangeKind::None,
                actual_vs_target: ChangeKind::Modified,
            },
            StatusEntry {
                path: PathBuf::from(".b"),
                actual_vs_state: ChangeKind::None,
                actual_vs_target: ChangeKind::Added,
            },
        ];

        app.open_apply_plan(ActionRequest {
            action: Action::Apply,
            target: Some(temp_root.join(".a")),
            chattr_attrs: None,
        });

        let ModalState::ApplyPlan { snapshot, .. } = &app.modal else {
            panic!("expected apply plan modal");
        };
        assert_eq!(snapshot.plan.total(), 1);
        assert_eq!(snapshot.plan.modified, vec![PathBuf::from(".a")]);

        let _ = std::fs::remove_dir_all(temp_root);
    }

    #[test]
    fn action_menu_indices_filters_by_label_only() {
        let mut app = App::new(AppConfig::default());
        app.switch_view(ListView::Status);

        let merge = app.action_menu_indices("merge");
        assert!(
            merge
                .iter()
                .any(|i| App::action_by_index(*i) == Some(Action::Merge))
        );
        assert!(
            merge
                .iter()
                .any(|i| App::action_by_index(*i) == Some(Action::MergeAll))
        );
        assert!(!merge.is_empty());

        let attrs = app.action_menu_indices("chattr");
        assert_eq!(attrs.len(), 1);
        assert_eq!(App::action_by_index(attrs[0]), Some(Action::Chattr));

        let by_description_only = app.action_menu_indices("attributes");
        assert!(by_description_only.is_empty());
    }

    #[test]
    fn action_menu_indices_hide_danger_actions_from_plain_filter() {
        let mut app = App::new(AppConfig::default());
        app.switch_view(ListView::Managed);

        let plain: Vec<Action> = app
            .action_menu_indices("purge")
            .into_iter()
            .filter_map(App::action_by_index)
            .collect();
        assert!(!plain.contains(&Action::Purge));

        let danger: Vec<Action> = app
            .action_menu_indices("danger:purge")
            .into_iter()
            .filter_map(App::action_by_index)
            .collect();
        assert_eq!(danger, vec![Action::Purge]);
    }

    #[test]
    fn confirmation_impact_lines_show_purge_context() {
        let source = PathBuf::from("/tmp/chezmoi-source");
        let config = AppConfig {
            destination_dir: Some(PathBuf::from("/tmp/chezmoi-home")),
            source_dir: Some(source.clone()),
            ..AppConfig::default()
        };
        let app = App::new(config);
        let request = ActionRequest {
            action: Action::Purge,
            target: None,
            chattr_attrs: None,
        };

        let lines = app.confirmation_impact_lines(&request).join("\n");
        assert!(lines.contains("configuration and data"));
        assert!(lines.contains("/tmp/chezmoi-home"));
        assert!(lines.contains(&source.display().to_string()));
        assert!(lines.contains("cannot be undone"));
    }

    #[test]
    fn action_menu_indices_are_sorted_alphabetically_by_label() {
        let mut app = App::new(AppConfig::default());
        app.switch_view(ListView::Unmanaged);

        let got = app.action_menu_indices("ad");
        assert_eq!(App::action_by_index(got[0]), Some(Action::Add));
    }

    #[test]
    fn action_menu_indices_prioritize_exact_match_over_partial_match() {
        let mut app = App::new(AppConfig::default());
        app.switch_view(ListView::Unmanaged);

        let got: Vec<Action> = app
            .action_menu_indices("ignore")
            .into_iter()
            .filter_map(App::action_by_index)
            .collect();

        let ignore_index = got
            .iter()
            .position(|action| *action == Action::Ignore)
            .expect("ignore should be matched");
        let edit_ignore_index = got
            .iter()
            .position(|action| *action == Action::EditIgnore)
            .expect("edit-ignore should be matched");

        assert!(ignore_index < edit_ignore_index);
    }

    #[test]
    fn action_menu_indices_follow_section_order_for_display_and_execution() {
        let mut app = App::new(AppConfig::default());
        app.switch_view(ListView::Managed);

        let got: Vec<Action> = app
            .action_menu_indices("")
            .into_iter()
            .filter_map(App::action_by_index)
            .collect();

        assert_eq!(
            got,
            vec![
                Action::Apply,
                Action::Data,
                Action::DebugContext,
                Action::Doctor,
                Action::EditConfig,
                Action::EditConfigTemplate,
                Action::EditIgnore,
                Action::ExternalDiff,
                Action::OpenSourceDir,
                Action::Update,
                Action::Chattr,
                Action::Edit,
                Action::Forget,
                Action::Destroy,
                Action::Purge,
            ]
        );
    }

    #[test]
    fn action_menu_indices_are_filtered_by_view() {
        let mut app = App::new(AppConfig::default());
        app.switch_view(ListView::Unmanaged);

        let unmanaged = app.action_menu_indices("");
        assert!(
            unmanaged
                .iter()
                .any(|i| App::action_by_index(*i) == Some(Action::Add))
        );
        assert!(
            unmanaged
                .iter()
                .any(|i| App::action_by_index(*i) == Some(Action::Ignore))
        );
        assert!(
            unmanaged
                .iter()
                .any(|i| App::action_by_index(*i) == Some(Action::EditConfig))
        );
        assert!(
            unmanaged
                .iter()
                .any(|i| App::action_by_index(*i) == Some(Action::EditConfigTemplate))
        );
        assert!(
            unmanaged
                .iter()
                .any(|i| App::action_by_index(*i) == Some(Action::EditIgnore))
        );
        assert!(
            !unmanaged
                .iter()
                .any(|i| App::action_by_index(*i) == Some(Action::Edit))
        );

        let mut app = App::new(AppConfig::default());
        app.switch_view(ListView::Managed);

        let managed = app.action_menu_indices("");
        assert!(
            managed
                .iter()
                .any(|i| App::action_by_index(*i) == Some(Action::Destroy))
        );
        assert!(
            !managed
                .iter()
                .any(|i| App::action_by_index(*i) == Some(Action::Add))
        );
    }

    #[test]
    fn readd_action_menu_indices_show_readd_for_modified_status_selection() {
        let temp_root = std::env::temp_dir().join(format!(
            "chezmoi_tui_readd_visible_{}_{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("time")
                .as_nanos()
        ));
        fs::create_dir_all(&temp_root).expect("create temp root");
        fs::write(temp_root.join(".zshrc"), "export ZDOTDIR=$HOME").expect("write file");

        let mut app = App::new(AppConfig::default());
        app.home_dir = temp_root.clone();
        app.status_entries = vec![StatusEntry {
            path: PathBuf::from(".zshrc"),
            actual_vs_state: ChangeKind::None,
            actual_vs_target: ChangeKind::Modified,
        }];
        app.switch_view(ListView::Status);

        let actions: Vec<Action> = app
            .action_menu_indices("")
            .into_iter()
            .filter_map(App::action_by_index)
            .collect();

        assert!(actions.contains(&Action::ReAdd));
        let _ = fs::remove_dir_all(temp_root);
    }

    #[test]
    fn readd_action_menu_indices_hide_readd_for_mixed_marked_status_entries() {
        let temp_root = std::env::temp_dir().join(format!(
            "chezmoi_tui_readd_hidden_{}_{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("time")
                .as_nanos()
        ));
        fs::create_dir_all(&temp_root).expect("create temp root");
        fs::write(temp_root.join(".zshrc"), "export ZDOTDIR=$HOME").expect("write file");
        fs::write(temp_root.join(".gitconfig"), "[user]").expect("write file");

        let mut app = App::new(AppConfig::default());
        app.home_dir = temp_root.clone();
        app.status_entries = vec![
            StatusEntry {
                path: PathBuf::from(".zshrc"),
                actual_vs_state: ChangeKind::None,
                actual_vs_target: ChangeKind::Modified,
            },
            StatusEntry {
                path: PathBuf::from(".gitconfig"),
                actual_vs_state: ChangeKind::None,
                actual_vs_target: ChangeKind::Added,
            },
        ];
        app.switch_view(ListView::Status);
        app.toggle_selected_mark();
        app.select_next();
        app.toggle_selected_mark();

        let actions: Vec<Action> = app
            .action_menu_indices("")
            .into_iter()
            .filter_map(App::action_by_index)
            .collect();

        assert!(!actions.contains(&Action::ReAdd));
        let _ = fs::remove_dir_all(temp_root);
    }

    #[test]
    fn selected_action_targets_use_marked_entries_in_visible_order() {
        let temp_root = std::env::temp_dir().join(format!(
            "chezmoi_tui_marked_targets_{}_{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("time")
                .as_nanos()
        ));
        fs::create_dir_all(&temp_root).expect("create temp root");
        fs::write(temp_root.join("a"), "a").expect("write a");
        fs::write(temp_root.join("b"), "b").expect("write b");

        let mut app = App::new(AppConfig::default());
        app.home_dir = temp_root.clone();
        app.managed_entries = vec![PathBuf::from("b"), PathBuf::from("a")];
        app.switch_view(ListView::Managed);

        // visible order is alphabetical (a, b)
        app.toggle_selected_mark();
        app.select_next();
        app.toggle_selected_mark();

        let targets = app.selected_action_targets_absolute();
        assert_eq!(targets, vec![temp_root.join("a"), temp_root.join("b")]);

        let _ = fs::remove_dir_all(temp_root);
    }

    #[test]
    fn switching_view_clears_multi_selection_marks() {
        let mut app = App::new(AppConfig::default());
        app.status_entries = vec![StatusEntry {
            path: PathBuf::from(".zshrc"),
            actual_vs_state: ChangeKind::Modified,
            actual_vs_target: ChangeKind::Modified,
        }];
        app.rebuild_visible_entries();
        assert!(app.toggle_selected_mark());
        assert_eq!(app.marked_count(), 1);
        app.switch_view(ListView::Managed);
        assert_eq!(app.marked_count(), 0);
    }

    #[test]
    fn switching_view_resets_list_filter() {
        let mut app = App::new(AppConfig::default());
        app.status_entries = vec![
            StatusEntry {
                path: PathBuf::from(".zshrc"),
                actual_vs_state: ChangeKind::Modified,
                actual_vs_target: ChangeKind::Modified,
            },
            StatusEntry {
                path: PathBuf::from(".gitconfig"),
                actual_vs_state: ChangeKind::Modified,
                actual_vs_target: ChangeKind::Modified,
            },
        ];
        app.switch_view(ListView::Status);
        app.apply_list_filter_immediately("zsh".to_string());
        assert_eq!(app.current_items().len(), 1);

        app.switch_view(ListView::Managed);
        assert!(app.list_filter().is_empty());
    }

    #[test]
    fn list_filter_matches_status_entries_case_insensitively() {
        let mut app = App::new(AppConfig::default());
        app.status_entries = vec![
            StatusEntry {
                path: PathBuf::from(".zshrc"),
                actual_vs_state: ChangeKind::Modified,
                actual_vs_target: ChangeKind::Modified,
            },
            StatusEntry {
                path: PathBuf::from(".gitconfig"),
                actual_vs_state: ChangeKind::Modified,
                actual_vs_target: ChangeKind::Modified,
            },
        ];
        app.switch_view(ListView::Status);
        app.apply_list_filter_immediately("ZSH".to_string());
        let items = app.current_items();
        assert_eq!(items.len(), 1);
        assert!(items[0].contains(".zshrc"));
    }

    #[test]
    fn list_filter_keeps_tree_parents_for_matching_children() {
        let mut app = App::new(AppConfig::default());
        app.managed_entries = vec![
            PathBuf::from("dev"),
            PathBuf::from("dev/chezmoi-tui"),
            PathBuf::from("dev/chezmoi-tui/Cargo.toml"),
        ];
        app.switch_view(ListView::Managed);

        app.apply_list_filter_immediately("cargo".to_string());
        let items = app.current_items();
        assert!(items.iter().any(|line| line.contains("dev/")));
        assert!(items.iter().any(|line| line.contains("chezmoi-tui/")));
        assert!(items.iter().any(|line| line.contains("Cargo.toml")));
    }

    #[test]
    fn list_filter_matches_managed_parent_directory_from_file_only_entries() {
        let mut app = App::new(AppConfig::default());
        app.managed_entries = vec![PathBuf::from("dev/chezmoi-tui/Cargo.toml")];
        app.switch_view(ListView::Managed);

        app.apply_list_filter_immediately("chezmoi-tui".to_string());
        let items = app.current_items();
        assert!(items.iter().any(|line| line.contains("dev/")));
        assert!(items.iter().any(|line| line.contains("chezmoi-tui/")));
        assert!(!items.iter().any(|line| line.contains("Cargo.toml")));
    }

    #[test]
    fn list_filter_finds_unmanaged_child_without_manual_expand() {
        let temp_root = std::env::temp_dir().join(format!(
            "chezmoi_tui_filter_unmanaged_{}_{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("time")
                .as_nanos()
        ));
        let dir = temp_root.join(".config/nvim");
        fs::create_dir_all(&dir).expect("create dir");
        fs::write(dir.join("init.lua"), "set number").expect("write file");

        let mut app = App::new(AppConfig::default());
        app.working_dir = temp_root.clone();
        app.unmanaged_entries = vec![PathBuf::from(".config")];
        app.switch_view(ListView::Unmanaged);

        app.apply_list_filter_immediately("init.lua".to_string());
        let items = app.current_items();
        assert!(items.iter().any(|line| line.contains(".config/")));
        assert!(items.iter().any(|line| line.contains("nvim/")));
        assert!(items.iter().any(|line| line.contains("init.lua")));

        let _ = fs::remove_dir_all(temp_root);
    }

    #[test]
    fn list_filter_unmanaged_keeps_ancestors_without_fs_walk() {
        let mut app = App::new(AppConfig::default());
        app.unmanaged_entries = vec![PathBuf::from("dev/chezmoi-tui/src/main.rs")];
        app.switch_view(ListView::Unmanaged);

        app.apply_list_filter_immediately("main.rs".to_string());
        let items = app.current_items();
        assert!(items.iter().any(|line| line.contains("dev/")));
        assert!(items.iter().any(|line| line.contains("chezmoi-tui/")));
        assert!(items.iter().any(|line| line.contains("src/")));
        assert!(items.iter().any(|line| line.contains("main.rs")));
    }

    #[test]
    fn unmanaged_view_keeps_ancestors_for_deep_entries_without_root_placeholder() {
        let mut app = App::new(AppConfig::default());
        app.unmanaged_entries = vec![
            PathBuf::from("dev/agent/.claude"),
            PathBuf::from("dev/agent/src/main.rs"),
        ];
        app.switch_view(ListView::Unmanaged);

        let initial = app.current_items();
        assert!(initial.iter().any(|line| line.contains("dev/")));
        assert!(!initial.iter().any(|line| line.contains("agent/")));
        assert!(!initial.iter().any(|line| line.contains("main.rs")));

        assert!(app.expand_selected_directory());
        let second = app.current_items();
        assert!(second.iter().any(|line| line.contains("agent/")));
        assert!(!second.iter().any(|line| line.contains(".claude")));
        assert!(!second.iter().any(|line| line.contains("main.rs")));

        app.select_next();
        assert!(app.expand_selected_directory());
        let third = app.current_items();
        assert!(third.iter().any(|line| line.contains(".claude")));
        assert!(third.iter().any(|line| line.contains("src/")));
        assert!(!third.iter().any(|line| line.contains("main.rs")));

        app.select_next();
        app.select_next();
        assert!(app.expand_selected_directory());
        let fourth = app.current_items();
        assert!(fourth.iter().any(|line| line.contains("main.rs")));
    }

    #[test]
    fn unmanaged_view_keeps_managed_ancestors_as_context_for_unmanaged_descendants() {
        let temp_root = std::env::temp_dir().join(format!(
            "chezmoi_tui_unmanaged_managed_ancestors_{}_{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("time")
                .as_nanos()
        ));
        fs::create_dir_all(temp_root.join(".worktrees/diff-view-ansi/src"))
            .expect("create source dir");
        fs::write(
            temp_root.join(".worktrees/diff-view-ansi/Cargo.lock"),
            "lock",
        )
        .expect("write lock file");

        let mut app = App::new(AppConfig::default());
        app.home_dir = temp_root.clone();
        app.working_dir = temp_root.clone();
        app.managed_entries = vec![
            PathBuf::from(".worktrees"),
            PathBuf::from(".worktrees/diff-view-ansi"),
        ];
        app.unmanaged_entries = vec![
            PathBuf::from(".worktrees/diff-view-ansi/src"),
            PathBuf::from(".worktrees/diff-view-ansi/Cargo.lock"),
        ];
        app.switch_view(ListView::Unmanaged);

        let initial = app.current_items();
        assert!(initial.iter().any(|line| line.contains(".worktrees/")));
        assert!(
            !initial
                .iter()
                .any(|line| line.contains("diff-view-ansi/src/"))
        );
        assert!(!initial.iter().any(|line| line.contains("Cargo.lock")));

        assert!(app.expand_selected_directory());
        let second = app.current_items();
        assert!(second.iter().any(|line| line.contains("diff-view-ansi/")));
        assert!(
            !second
                .iter()
                .any(|line| line.contains("diff-view-ansi/src/"))
        );
        assert!(!second.iter().any(|line| line.contains("Cargo.lock")));

        app.select_next();
        assert!(app.expand_selected_directory());
        let third = app.current_items();
        assert!(third.iter().any(|line| line.contains("src/")));
        assert!(third.iter().any(|line| line.contains("Cargo.lock")));

        let _ = fs::remove_dir_all(temp_root);
    }

    #[test]
    fn staged_filter_is_flushed_only_after_debounce_interval() {
        let mut app = App::new(AppConfig::default());
        app.status_entries = vec![
            StatusEntry {
                path: PathBuf::from(".zshrc"),
                actual_vs_state: ChangeKind::Modified,
                actual_vs_target: ChangeKind::Modified,
            },
            StatusEntry {
                path: PathBuf::from(".gitconfig"),
                actual_vs_state: ChangeKind::Modified,
                actual_vs_target: ChangeKind::Modified,
            },
        ];
        app.switch_view(ListView::Status);
        assert_eq!(app.current_items().len(), 2);

        app.staged_list_filter = Some("zsh".to_string());
        app.staged_filter_updated_at = Some(Instant::now());
        assert_eq!(app.current_items().len(), 2);

        let just_now = app
            .staged_filter_updated_at
            .expect("staged filter timestamp should exist");
        assert!(!app.flush_staged_filter(just_now + Duration::from_millis(10)));
        assert!(app.list_filter().is_empty());

        app.staged_filter_updated_at = Some(
            Instant::now()
                .checked_sub(Duration::from_millis(200))
                .unwrap(),
        );
        assert!(app.flush_staged_filter(Instant::now()));
        assert_eq!(app.list_filter(), "zsh");
        assert_eq!(app.current_items().len(), 1);
    }

    #[test]
    fn source_view_is_hierarchical_and_resolves_to_source_dir() {
        let temp_root = std::env::temp_dir().join(format!(
            "chezmoi_tui_source_view_{}_{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("time")
                .as_nanos()
        ));
        fs::create_dir_all(temp_root.join("dot_config/nvim")).expect("create source dir");
        fs::write(
            temp_root.join("dot_config/nvim/init.lua"),
            "vim.opt.number = true",
        )
        .expect("write source file");

        let config = AppConfig {
            source_dir: Some(temp_root.clone()),
            ..AppConfig::default()
        };
        let mut app = App::new(config);
        app.source_entries = vec![PathBuf::from("dot_config/nvim/init.lua")];
        app.switch_view(ListView::Source);

        let items = app.current_items();
        assert!(items.iter().any(|line| line.contains("dot_config/")));
        assert!(!items.iter().any(|line| line.contains("init.lua")));
        assert!(app.expand_selected_directory());
        app.select_next();
        assert!(app.expand_selected_directory());
        let expanded = app.current_items();
        assert!(expanded.iter().any(|line| line.contains("init.lua")));

        app.select_next();
        assert_eq!(
            app.selected_absolute_path(),
            Some(temp_root.join("dot_config/nvim/init.lua"))
        );

        let _ = fs::remove_dir_all(temp_root);
    }

    #[test]
    fn list_scroll_moves_only_at_view_edges() {
        let mut app = App::new(AppConfig::default());
        app.managed_entries = (0..20)
            .map(|i| PathBuf::from(format!("file-{i}")))
            .collect();
        app.switch_view(ListView::Managed);

        app.selected_index = 10;
        app.sync_list_scroll(5);
        assert_eq!(app.list_scroll(), 6);

        app.selected_index = 9;
        app.sync_list_scroll(5);
        assert_eq!(app.list_scroll(), 6);

        app.selected_index = 6;
        app.sync_list_scroll(5);
        assert_eq!(app.list_scroll(), 6);

        app.selected_index = 5;
        app.sync_list_scroll(5);
        assert_eq!(app.list_scroll(), 5);
    }
}
