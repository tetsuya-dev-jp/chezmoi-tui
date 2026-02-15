use crate::config::AppConfig;
use crate::domain::{Action, ActionRequest, CommandResult, DiffText, ListView, StatusEntry};
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

const MAX_LOG_LINES: usize = 500;

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
    Help,
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
    pub busy: bool,
    pub pending_foreground: Option<ActionRequest>,
    pub should_quit: bool,
    home_dir: PathBuf,
    working_dir: PathBuf,
    expanded_dirs: BTreeSet<PathBuf>,
    visible_entries: Vec<VisibleEntry>,
}

impl App {
    pub fn new(config: AppConfig) -> Self {
        let working_dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let home_dir = dirs::home_dir().unwrap_or_else(|| working_dir.clone());
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
            busy: false,
            pending_foreground: None,
            should_quit: false,
            home_dir,
            working_dir,
            expanded_dirs: BTreeSet::new(),
            visible_entries: Vec::new(),
        };

        app.rebuild_visible_entries_reset();
        app
    }

    pub fn switch_view(&mut self, view: ListView) {
        self.view = view;
        self.rebuild_visible_entries_reset();
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
        let Some(selected) = self.selected_path() else {
            return false;
        };
        let selected_abs = self.resolve_path_for_view(&selected, self.view);

        self.managed_entries.iter().any(|managed| {
            managed == &selected
                || managed == &selected_abs
                || self.resolve_with_base(managed.as_path(), &self.home_dir) == selected_abs
        })
    }

    pub fn expand_selected_directory(&mut self) -> bool {
        if !self.view_supports_tree() {
            return false;
        }

        let Some(entry) = self.visible_entries.get(self.selected_index).cloned() else {
            return false;
        };
        if !entry.is_dir {
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

    pub fn open_help(&mut self) {
        self.modal = ModalState::Help;
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
        let mut matches: Vec<(usize, String)> = Action::ALL
            .iter()
            .enumerate()
            .filter(|(_, action)| {
                Self::action_visible_in_view(view, **action)
                    && (query.is_empty() || action.label().to_ascii_lowercase().contains(&query))
            })
            .map(|(index, action)| (index, action.label().to_ascii_lowercase()))
            .collect();

        matches.sort_by(|a, b| a.1.cmp(&b.1));
        matches.into_iter().map(|(index, _)| index).collect()
    }

    fn action_visible_in_view(view: ListView, action: Action) -> bool {
        match view {
            ListView::Status => matches!(
                action,
                Action::Apply
                    | Action::Update
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
                    | Action::Edit
                    | Action::Forget
                    | Action::Chattr
                    | Action::Destroy
                    | Action::Purge
            ),
            ListView::Unmanaged => {
                matches!(
                    action,
                    Action::Add | Action::Apply | Action::Update | Action::Purge
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
        let base_paths = self.base_paths_for_view();
        let mut seen = HashSet::new();
        let mut entries = Vec::new();

        if self.view == ListView::Unmanaged {
            for base in base_paths {
                self.push_visible_recursive(base, 0, &mut entries, &mut seen);
            }
        } else if self.view == ListView::Managed {
            self.push_managed_visible_entries(&mut entries);
        } else {
            for path in base_paths {
                if !seen.insert(path.clone()) {
                    continue;
                }
                let is_dir = self.path_is_directory(&path);
                entries.push(VisibleEntry {
                    path,
                    depth: 0,
                    is_dir,
                });
            }
        }

        self.visible_entries = entries;

        if let Some(target) = previous
            && let Some(idx) = self.visible_entries.iter().position(|e| e.path == target)
        {
            self.selected_index = idx;
            return;
        }

        self.sync_selection_bounds();
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
            ListView::Unmanaged => self
                .unmanaged_entries
                .iter()
                .filter(|path| {
                    let abs = self.resolve_with_base(path.as_path(), &self.working_dir);
                    !self.is_managed_absolute_path(&abs)
                })
                .cloned()
                .collect(),
        }
    }

    fn push_visible_recursive(
        &self,
        path: PathBuf,
        depth: usize,
        out: &mut Vec<VisibleEntry>,
        seen: &mut HashSet<PathBuf>,
    ) {
        if !seen.insert(path.clone()) {
            return;
        }

        let is_dir = self.path_is_directory(&path);
        out.push(VisibleEntry {
            path: path.clone(),
            depth,
            is_dir,
        });

        if !is_dir || !self.expanded_dirs.contains(&path) {
            return;
        }

        for child in self.read_children(&path) {
            self.push_visible_recursive(child, depth + 1, out, seen);
        }
    }

    fn push_managed_visible_entries(&self, out: &mut Vec<VisibleEntry>) {
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
            self.push_managed_visible_recursive(root, 0, out, &children, &mut seen);
        }
    }

    fn push_managed_visible_recursive(
        &self,
        path: PathBuf,
        depth: usize,
        out: &mut Vec<VisibleEntry>,
        children: &BTreeMap<PathBuf, Vec<PathBuf>>,
        seen: &mut HashSet<PathBuf>,
    ) {
        if !seen.insert(path.clone()) {
            return;
        }

        let has_children = children
            .get(&path)
            .map(|entries| !entries.is_empty())
            .unwrap_or(false);
        let is_dir = has_children || self.path_is_directory_with_base(&path, &self.home_dir);
        out.push(VisibleEntry {
            path: path.clone(),
            depth,
            is_dir,
        });

        if !is_dir || !self.expanded_dirs.contains(&path) {
            return;
        }

        if let Some(child_paths) = children.get(&path) {
            for child in child_paths {
                self.push_managed_visible_recursive(child.clone(), depth + 1, out, children, seen);
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
                if parent.is_absolute() {
                    parent.join(name)
                } else {
                    PathBuf::from(parent).join(name)
                }
            })
            .filter(|path| {
                if self.view != ListView::Unmanaged {
                    return true;
                }
                let abs = self.resolve_with_base(path.as_path(), &self.working_dir);
                !self.is_managed_absolute_path(&abs)
            })
            .collect();

        children.sort_by(|a, b| a.to_string_lossy().cmp(&b.to_string_lossy()));
        children
    }

    fn is_managed_absolute_path(&self, path: &Path) -> bool {
        self.managed_entries.iter().any(|managed| {
            let managed_abs = self.resolve_with_base(managed.as_path(), &self.home_dir);
            if !managed_abs.starts_with(&self.working_dir) {
                return false;
            }
            path == managed_abs
        })
    }

    fn format_visible_entry(&self, entry: &VisibleEntry) -> String {
        if self.view == ListView::Status {
            let mut label = String::new();
            if let Some(status) = self.status_entries.iter().find(|s| s.path == entry.path) {
                label.push(status.actual_vs_state.as_symbol());
                label.push(status.actual_vs_target.as_symbol());
            } else {
                label.push(' ');
                label.push(' ');
            }
            label.push(' ');
            label.push_str(&entry.path.display().to_string());
            if entry.is_dir {
                label.push('/');
            }
            return label;
        }

        let mut label = String::new();
        label.push_str(&"  ".repeat(entry.depth));

        let expanded = self.expanded_dirs.contains(&entry.path);
        let marker = if entry.is_dir {
            if expanded { "[-]" } else { "[+]" }
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
        if entry.is_dir {
            label.push('/');
        }

        label
    }

    fn path_is_directory(&self, path: &Path) -> bool {
        let abs = self.resolve_path_for_view(path, self.view);
        fs::symlink_metadata(abs)
            .map(|meta| meta.file_type().is_dir())
            .unwrap_or(false)
    }

    fn path_is_directory_with_base(&self, path: &Path, base: &Path) -> bool {
        let abs = self.resolve_with_base(path, base);
        fs::symlink_metadata(abs)
            .map(|meta| meta.file_type().is_dir())
            .unwrap_or(false)
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::ChangeKind;
    use std::path::Path;
    use std::time::{SystemTime, UNIX_EPOCH};

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
        assert_eq!(items[0], "MM .zshrc");
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
    fn action_menu_indices_are_filtered_by_view() {
        let unmanaged = App::action_menu_indices(ListView::Unmanaged, "");
        assert!(
            unmanaged
                .iter()
                .any(|i| App::action_by_index(*i) == Some(Action::Add))
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
