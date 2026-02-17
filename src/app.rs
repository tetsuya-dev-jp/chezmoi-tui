use crate::config::AppConfig;
use crate::domain::{Action, ActionRequest, CommandResult, DiffText, ListView, StatusEntry};
use std::collections::{BTreeMap, BTreeSet, HashSet, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const MAX_LOG_LINES: usize = 500;
const LIST_FILTER_DEBOUNCE_MS: u64 = 120;
const INITIAL_UNMANAGED_FILTER_INDEX_ENTRIES: usize = 50_000;
const UNMANAGED_FILTER_INDEX_STEP: usize = 50_000;
const MAX_UNMANAGED_FILTER_INDEX_ENTRIES: usize = 200_000;
const DEFAULT_UNMANAGED_EXCLUDES: &[&str] = &[
    ".cache",
    ".vscode-server",
    ".npm",
    ".cargo/registry",
    ".cargo/git",
    "tmp",
];

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
pub enum ConfirmStep {
    Primary,
    DangerPhrase,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InputKind {
    ChattrAttrs,
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
    ActionMenu {
        selected: usize,
        filter: String,
    },
    Confirm {
        request: ActionRequest,
        step: ConfirmStep,
        typed: String,
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
    LoadDiff { target: Option<PathBuf> },
    LoadPreview { target: PathBuf, absolute: PathBuf },
    RunAction { request: ActionRequest },
}

#[derive(Debug, Clone)]
pub enum BackendEvent {
    Refreshed {
        status: Vec<StatusEntry>,
        managed: Vec<PathBuf>,
        unmanaged: Vec<PathBuf>,
    },
    DiffLoaded {
        target: Option<PathBuf>,
        diff: DiffText,
    },
    PreviewLoaded {
        target: PathBuf,
        content: String,
    },
    ActionFinished {
        request: ActionRequest,
        result: CommandResult,
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
    pub selected_index: usize,
    list_scroll: usize,
    pub detail_kind: DetailKind,
    pub detail_title: String,
    pub detail_text: String,
    pub detail_target: Option<PathBuf>,
    pub detail_scroll: usize,
    pub logs: Vec<String>,
    pub log_tail_offset: usize,
    pub modal: ModalState,
    list_filter: String,
    staged_list_filter: Option<String>,
    staged_filter_updated_at: Option<Instant>,
    pub busy: bool,
    pub footer_help: bool,
    pub pending_foreground: Option<ActionRequest>,
    pub should_quit: bool,
    home_dir: PathBuf,
    working_dir: PathBuf,
    expanded_dirs: BTreeSet<PathBuf>,
    marked_entries: BTreeSet<PathBuf>,
    batch_action: Option<Action>,
    batch_total: usize,
    batch_queue: VecDeque<ActionRequest>,
    visible_entries: Vec<VisibleEntry>,
    unmanaged_filter_cache: UnmanagedFilterCache,
    unmanaged_exclude_prefixes: Vec<PathBuf>,
}

impl App {
    pub fn new(config: AppConfig) -> Self {
        let working_dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let home_dir = dirs::home_dir().unwrap_or_else(|| working_dir.clone());
        let unmanaged_exclude_prefixes = Self::build_unmanaged_exclude_prefixes(&config);
        let mut app = Self {
            config,
            focus: PaneFocus::List,
            view: ListView::Status,
            status_entries: Vec::new(),
            managed_entries: Vec::new(),
            unmanaged_entries: Vec::new(),
            selected_index: 0,
            list_scroll: 0,
            detail_kind: DetailKind::Diff,
            detail_title: "Diff / Preview".to_string(),
            detail_text: String::new(),
            detail_target: None,
            detail_scroll: 0,
            logs: Vec::new(),
            log_tail_offset: 0,
            modal: ModalState::None,
            list_filter: String::new(),
            staged_list_filter: None,
            staged_filter_updated_at: None,
            busy: false,
            footer_help: false,
            pending_foreground: None,
            should_quit: false,
            home_dir,
            working_dir,
            expanded_dirs: BTreeSet::new(),
            marked_entries: BTreeSet::new(),
            batch_action: None,
            batch_total: 0,
            batch_queue: VecDeque::new(),
            visible_entries: Vec::new(),
            unmanaged_filter_cache: UnmanagedFilterCache::default(),
            unmanaged_exclude_prefixes,
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

    pub fn apply_refresh_entries(
        &mut self,
        status: Vec<StatusEntry>,
        managed: Vec<PathBuf>,
        unmanaged: Vec<PathBuf>,
    ) {
        self.status_entries = status;
        self.managed_entries = managed;
        self.unmanaged_entries = unmanaged;
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
            .map(|entry| entry.is_dir)
            .unwrap_or(false)
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

    pub fn open_list_filter(&mut self) {
        self.clear_staged_list_filter();
        self.modal = ModalState::ListFilter {
            value: self.list_filter.clone(),
            original: self.list_filter.clone(),
        };
    }

    pub fn toggle_footer_help(&mut self) {
        self.footer_help = !self.footer_help;
    }

    pub fn open_confirm(&mut self, request: ActionRequest) {
        self.modal = ModalState::Confirm {
            request,
            step: ConfirmStep::Primary,
            typed: String::new(),
        };
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

    pub fn stage_list_filter(&mut self, value: String) {
        self.staged_list_filter = Some(value);
        self.staged_filter_updated_at = Some(Instant::now());
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

    pub fn action_menu_indices(view: ListView, filter: &str) -> Vec<usize> {
        let query = filter.trim().to_ascii_lowercase();
        let mut matches: Vec<(usize, u8, String)> = Action::ALL
            .iter()
            .enumerate()
            .filter(|(_, action)| {
                Self::action_visible_in_view(view, **action)
                    && (query.is_empty() || action.label().to_ascii_lowercase().contains(&query))
            })
            .map(|(index, action)| {
                (
                    index,
                    Self::action_section_order(*action),
                    action.label().to_ascii_lowercase(),
                )
            })
            .collect();

        matches.sort_by(|a, b| a.1.cmp(&b.1).then(a.2.cmp(&b.2)));
        matches.into_iter().map(|(index, _, _)| index).collect()
    }

    fn action_section_order(action: Action) -> u8 {
        if action.is_dangerous() {
            2
        } else if action.needs_target() {
            1
        } else {
            0
        }
    }

    fn action_visible_in_view(view: ListView, action: Action) -> bool {
        match view {
            ListView::Status => matches!(
                action,
                Action::Apply
                    | Action::Update
                    | Action::EditConfig
                    | Action::EditConfigTemplate
                    | Action::EditIgnore
                    | Action::ReAdd
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
                        | Action::Update
                        | Action::EditConfig
                        | Action::EditConfigTemplate
                        | Action::EditIgnore
                        | Action::Purge
                )
            }
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
    }

    pub fn set_detail_preview(&mut self, target: &Path, content: String) {
        self.detail_kind = DetailKind::Preview;
        self.detail_title = format!("Preview: {}", target.display());
        self.detail_text = content;
        self.detail_target = Some(target.to_path_buf());
        self.detail_scroll = 0;
    }

    pub fn clear_detail(&mut self) {
        self.detail_title = "Diff / Preview".to_string();
        self.detail_text.clear();
        self.detail_target = None;
        self.detail_scroll = 0;
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
            for base in base_paths {
                self.push_visible_recursive(base, 0, &mut entries, &mut seen, false);
            }
            return entries;
        }

        if self.view == ListView::Managed {
            self.push_managed_visible_entries(&mut entries, false);
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
                self.managed_entries
                    .iter()
                    .filter(|path| !path.as_os_str().is_empty())
                    .cloned()
                    .collect(),
                &query,
            ),
            ListView::Unmanaged => {
                let source_paths = self.unmanaged_filter_source_paths(&query);
                self.build_filtered_tree_entries(source_paths, &query)
            }
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

        self.build_tree_entries_from_paths(keep)
    }

    fn tree_entry_name_matches_query(path: &Path, query: &str) -> bool {
        path.file_name()
            .and_then(|name| name.to_str())
            .map(|name| name.to_ascii_lowercase().contains(query))
            .unwrap_or_else(|| path.to_string_lossy().to_ascii_lowercase().contains(query))
    }

    fn build_tree_entries_from_paths(&self, nodes: BTreeSet<PathBuf>) -> Vec<VisibleEntry> {
        if nodes.is_empty() {
            return Vec::new();
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

        let mut entries = Vec::new();
        for root in roots {
            self.push_filtered_tree_entry_recursive(root, 0, &children, &mut entries);
        }
        entries
    }

    fn push_filtered_tree_entry_recursive(
        &self,
        path: PathBuf,
        depth: usize,
        children: &BTreeMap<PathBuf, Vec<PathBuf>>,
        out: &mut Vec<VisibleEntry>,
    ) {
        let has_children = children
            .get(&path)
            .map(|entries| !entries.is_empty())
            .unwrap_or(false);
        let directory = self.path_directory_state_for_view(&path, self.view);
        out.push(VisibleEntry {
            path: path.clone(),
            depth,
            is_dir: has_children || directory.is_dir,
            can_expand: has_children || directory.can_expand,
            is_symlink: directory.is_symlink,
        });

        if let Some(child_paths) = children.get(&path) {
            for child in child_paths {
                self.push_filtered_tree_entry_recursive(child.clone(), depth + 1, children, out);
            }
        }
    }

    fn view_supports_tree(&self) -> bool {
        matches!(self.view, ListView::Managed | ListView::Unmanaged)
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

    fn unmanaged_filter_source_paths_with_limits(
        &mut self,
        query: &str,
        initial_limit: usize,
        step: usize,
        max_limit: usize,
    ) -> Vec<PathBuf> {
        let initial = initial_limit.min(max_limit).max(1);
        self.scan_unmanaged_filter_index_to(initial);

        let normalized_query = query.trim().to_ascii_lowercase();
        if normalized_query.is_empty() {
            return self.unmanaged_filter_cache.entries.clone();
        }

        let mut current_limit = initial;
        while !self.unmanaged_filter_cache.scan_complete
            && !self.unmanaged_index_has_match(&normalized_query)
            && current_limit < max_limit
        {
            current_limit = (current_limit + step).min(max_limit);
            self.scan_unmanaged_filter_index_to(current_limit);
        }

        self.unmanaged_filter_cache.entries.clone()
    }

    fn unmanaged_index_has_match(&self, query: &str) -> bool {
        self.unmanaged_filter_cache
            .entries
            .iter()
            .any(|path| path.to_string_lossy().to_ascii_lowercase().contains(query))
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
        path: PathBuf,
        depth: usize,
        out: &mut Vec<VisibleEntry>,
        seen: &mut HashSet<PathBuf>,
        force_expand: bool,
    ) {
        if !seen.insert(path.clone()) {
            return;
        }

        let directory = self.path_directory_state_for_view(&path, self.view);
        let is_dir = directory.is_dir;
        out.push(VisibleEntry {
            path: path.clone(),
            depth,
            is_dir,
            can_expand: directory.can_expand,
            is_symlink: directory.is_symlink,
        });

        if !directory.can_expand || (!force_expand && !self.expanded_dirs.contains(&path)) {
            return;
        }

        for child in self.read_children(&path) {
            self.push_visible_recursive(child, depth + 1, out, seen, force_expand);
        }
    }

    fn push_managed_visible_entries(&self, out: &mut Vec<VisibleEntry>, force_expand: bool) {
        let mut nodes = BTreeSet::new();

        for managed in &self.managed_entries {
            if managed.as_os_str().is_empty() {
                continue;
            }

            let mut current = managed.clone();
            loop {
                if !current.as_os_str().is_empty() {
                    nodes.insert(current.clone());
                }

                let Some(parent) = current.parent() else {
                    break;
                };
                if parent.as_os_str().is_empty() {
                    break;
                }
                current = parent.to_path_buf();
            }
        }

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
        for root in roots {
            self.push_managed_visible_recursive(root, 0, out, &children, &mut seen, force_expand);
        }
    }

    fn push_managed_visible_recursive(
        &self,
        path: PathBuf,
        depth: usize,
        out: &mut Vec<VisibleEntry>,
        children: &BTreeMap<PathBuf, Vec<PathBuf>>,
        seen: &mut HashSet<PathBuf>,
        force_expand: bool,
    ) {
        if !seen.insert(path.clone()) {
            return;
        }

        let has_children = children
            .get(&path)
            .map(|entries| !entries.is_empty())
            .unwrap_or(false);
        let directory = self.directory_state_with_base(&path, &self.home_dir);
        let is_dir = has_children || directory.is_dir;
        let can_expand = has_children || directory.can_expand;
        out.push(VisibleEntry {
            path: path.clone(),
            depth,
            is_dir,
            can_expand,
            is_symlink: directory.is_symlink,
        });

        if !can_expand || (!force_expand && !self.expanded_dirs.contains(&path)) {
            return;
        }

        if let Some(child_paths) = children.get(&path) {
            for child in child_paths {
                self.push_managed_visible_recursive(
                    child.clone(),
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
        let abs_parent = self.resolve_with_base(parent, &self.working_dir);
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
        self.resolve_with_base(managed, &self.home_dir)
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
        let abs = self.resolve_with_base(path, &self.working_dir);
        if self.is_exact_managed_path_in_working_dir(&abs) {
            return true;
        }

        let normalized = self.normalize_unmanaged_relative_path(path);
        self.unmanaged_exclude_prefixes
            .iter()
            .any(|exclude| normalized.starts_with(exclude))
    }

    fn build_unmanaged_exclude_prefixes(config: &AppConfig) -> Vec<PathBuf> {
        let mut excludes = Vec::new();

        for entry in DEFAULT_UNMANAGED_EXCLUDES
            .iter()
            .copied()
            .chain(config.unmanaged_exclude_paths.iter().map(String::as_str))
        {
            if let Some(normalized) = Self::normalize_exclude_entry(entry) {
                excludes.push(normalized);
            }
        }

        excludes
    }

    fn normalize_unmanaged_relative_path(&self, path: &Path) -> PathBuf {
        let relative = if path.is_absolute() {
            path.strip_prefix(&self.working_dir).unwrap_or(path)
        } else {
            path
        };
        Self::normalize_match_path(relative)
    }

    fn normalize_exclude_entry(entry: &str) -> Option<PathBuf> {
        let normalized = Self::normalize_match_path(Path::new(entry));
        if normalized == Path::new(".") {
            return None;
        }
        Some(normalized)
    }

    fn normalize_match_path(path: &Path) -> PathBuf {
        let normalized = path
            .to_string_lossy()
            .replace('\\', "/")
            .trim_start_matches("./")
            .trim_start_matches('/')
            .trim_end_matches('/')
            .to_string();
        if normalized.is_empty() {
            PathBuf::from(".")
        } else {
            PathBuf::from(normalized)
        }
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
            label.push_str(&entry.path.display().to_string());
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
        let marker = if entry.is_symlink && entry.is_dir {
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
        label.push_str(marker);
        label.push(' ');

        let name = if entry.depth == 0 {
            entry.path.display().to_string()
        } else {
            entry
                .path
                .file_name()
                .and_then(|n| n.to_str())
                .map(|s| s.to_string())
                .unwrap_or_else(|| entry.path.display().to_string())
        };

        label.push_str(&name);
        if entry.is_symlink {
            label.push('@');
        }
        if entry.is_dir {
            label.push('/');
        }

        label
    }

    fn path_is_directory(&self, path: &Path) -> bool {
        self.path_directory_state_for_view(path, self.view).is_dir
    }

    fn path_directory_state_for_view(&self, path: &Path, view: ListView) -> DirectoryState {
        let abs = self.resolve_path_for_view(path, view);
        self.directory_state_for_absolute(&abs)
    }

    fn directory_state_with_base(&self, path: &Path, base: &Path) -> DirectoryState {
        let abs = self.resolve_with_base(path, base);
        self.directory_state_for_absolute(&abs)
    }

    fn directory_state_for_absolute(&self, abs: &Path) -> DirectoryState {
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
        };
        self.resolve_with_base(path, base)
    }

    fn resolve_with_base(&self, path: &Path, base: &Path) -> PathBuf {
        if path.is_absolute() {
            path.to_path_buf()
        } else {
            base.join(path)
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::ChangeKind;
    use std::path::Path;
    use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

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
    fn unmanaged_view_excludes_default_temporary_directories() {
        let temp_root = std::env::temp_dir().join(format!(
            "chezmoi_tui_unmanaged_default_exclude_{}_{}",
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
        assert!(!items.iter().any(|line| line.contains(".cache")));
        assert!(items.iter().any(|line| line.contains(".codex/")));

        app.apply_list_filter_immediately("skill".to_string());
        let filtered = app.current_items();
        assert!(filtered.iter().any(|line| line.contains("SKILL.md")));

        let _ = fs::remove_dir_all(temp_root);
    }

    #[test]
    fn unmanaged_default_excludes_match_prefix_not_partial_name() {
        let mut app = App::new(AppConfig::default());
        app.unmanaged_entries = vec![
            PathBuf::from("tmp/file.txt"),
            PathBuf::from("template/file.txt"),
            PathBuf::from(".cargo/config.toml"),
            PathBuf::from(".cargo/registry/index.txt"),
        ];
        app.switch_view(ListView::Unmanaged);

        let items = app.current_items();
        assert!(!items.iter().any(|line| line.contains("tmp/file.txt")));
        assert!(items.iter().any(|line| line.contains("template/file.txt")));
        assert!(items.iter().any(|line| line.contains(".cargo/config.toml")));
        assert!(
            !items
                .iter()
                .any(|line| line.contains(".cargo/registry/index.txt"))
        );
    }

    #[test]
    fn unmanaged_view_supports_custom_excludes_from_config() {
        let temp_root = std::env::temp_dir().join(format!(
            "chezmoi_tui_unmanaged_custom_exclude_{}_{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("time")
                .as_nanos()
        ));
        fs::create_dir_all(temp_root.join(".codex/skills")).expect("create codex dir");
        fs::write(temp_root.join(".codex/skills/SKILL.md"), "skill").expect("write skill");
        fs::create_dir_all(temp_root.join("notes")).expect("create notes dir");
        fs::write(temp_root.join("notes/todo.md"), "todo").expect("write note");

        let mut app = App::new(AppConfig {
            unmanaged_exclude_paths: vec![".codex".to_string()],
            ..AppConfig::default()
        });
        app.working_dir = temp_root.clone();
        app.unmanaged_entries = vec![PathBuf::from(".codex"), PathBuf::from("notes")];
        app.switch_view(ListView::Unmanaged);

        let items = app.current_items();
        assert!(!items.iter().any(|line| line.contains(".codex")));
        assert!(items.iter().any(|line| line.contains("notes/")));

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
    fn detail_scroll_is_clamped() {
        let mut app = App::new(AppConfig::default());
        app.set_detail_preview(Path::new(".config/test.txt"), "a\nb\nc\nd\ne".to_string());
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
        app.set_detail_preview(Path::new(".config/test.txt"), "hello".to_string());
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
    fn action_menu_indices_filters_by_label_only() {
        let merge = App::action_menu_indices(ListView::Status, "merge");
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

        let attrs = App::action_menu_indices(ListView::Status, "chattr");
        assert_eq!(attrs.len(), 1);
        assert_eq!(App::action_by_index(attrs[0]), Some(Action::Chattr));

        let by_description_only = App::action_menu_indices(ListView::Status, "attributes");
        assert!(by_description_only.is_empty());
    }

    #[test]
    fn action_menu_indices_are_sorted_alphabetically_by_label() {
        let got = App::action_menu_indices(ListView::Unmanaged, "ad");
        assert_eq!(App::action_by_index(got[0]), Some(Action::Add));
    }

    #[test]
    fn action_menu_indices_follow_section_order_for_display_and_execution() {
        let got: Vec<Action> = App::action_menu_indices(ListView::Managed, "")
            .into_iter()
            .filter_map(App::action_by_index)
            .collect();

        assert_eq!(
            got,
            vec![
                Action::Apply,
                Action::EditConfig,
                Action::EditConfigTemplate,
                Action::EditIgnore,
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
        let unmanaged = App::action_menu_indices(ListView::Unmanaged, "");
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

        let managed = App::action_menu_indices(ListView::Managed, "");
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

        app.stage_list_filter("zsh".to_string());
        assert_eq!(app.current_items().len(), 2);

        let just_now = app
            .staged_filter_updated_at
            .expect("staged filter timestamp should exist");
        assert!(!app.flush_staged_filter(just_now + Duration::from_millis(10)));
        assert!(app.list_filter().is_empty());

        app.staged_filter_updated_at = Some(Instant::now() - Duration::from_millis(200));
        assert!(app.flush_staged_filter(Instant::now()));
        assert_eq!(app.list_filter(), "zsh");
        assert_eq!(app.current_items().len(), 1);
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
