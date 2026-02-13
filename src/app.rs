use crate::config::AppConfig;
use crate::domain::{Action, ActionRequest, CommandResult, DiffText, ListView, StatusEntry};
use std::collections::{BTreeSet, HashSet};
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
    ActionMenu {
        selected: usize,
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
    pub detail_kind: DetailKind,
    pub detail_title: String,
    pub detail_text: String,
    pub detail_target: Option<PathBuf>,
    pub logs: Vec<String>,
    pub modal: ModalState,
    pub busy: bool,
    pub pending_foreground: Option<ActionRequest>,
    pub should_quit: bool,
    destination_dir: PathBuf,
    expanded_dirs: BTreeSet<PathBuf>,
    visible_entries: Vec<VisibleEntry>,
}

impl App {
    pub fn new(config: AppConfig) -> Self {
        let destination_dir = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
        let mut app = Self {
            config,
            focus: PaneFocus::List,
            view: ListView::Status,
            status_entries: Vec::new(),
            managed_entries: Vec::new(),
            unmanaged_entries: Vec::new(),
            selected_index: 0,
            detail_kind: DetailKind::Diff,
            detail_title: "Diff / Preview".to_string(),
            detail_text: String::new(),
            detail_target: None,
            logs: Vec::new(),
            modal: ModalState::None,
            busy: false,
            pending_foreground: None,
            should_quit: false,
            destination_dir,
            expanded_dirs: BTreeSet::new(),
            visible_entries: Vec::new(),
        };

        app.rebuild_visible_entries_reset();
        app.log(
            "起動: r=refresh d=diff v=preview a=action e=edit h/l=collapse/expand tab=focus q=quit"
                .to_string(),
        );
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
        self.selected_path().map(|path| self.resolve_path(&path))
    }

    pub fn selected_is_directory(&self) -> bool {
        self.visible_entries
            .get(self.selected_index)
            .map(|entry| entry.is_dir)
            .unwrap_or(false)
    }

    pub fn expand_selected_directory(&mut self) -> bool {
        if self.view != ListView::Unmanaged {
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
        if self.view != ListView::Unmanaged {
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
        self.modal = ModalState::ActionMenu { selected: 0 };
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
        if self.logs.len() > MAX_LOG_LINES {
            let to_trim = self.logs.len() - MAX_LOG_LINES;
            self.logs.drain(0..to_trim);
        }
    }

    pub fn sync_selection_bounds(&mut self) {
        let len = self.current_len();
        if len == 0 {
            self.selected_index = 0;
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

    pub fn set_detail_diff(&mut self, target: Option<&Path>, text: String) {
        self.detail_kind = DetailKind::Diff;
        self.detail_title = match target {
            Some(path) => format!("Diff: {}", path.display()),
            None => "Diff: (all)".to_string(),
        };
        self.detail_text = text;
        self.detail_target = target.map(Path::to_path_buf);
    }

    pub fn set_detail_preview(&mut self, target: &Path, content: String) {
        self.detail_kind = DetailKind::Preview;
        self.detail_title = format!("Preview: {}", target.display());
        self.detail_text = content;
        self.detail_target = Some(target.to_path_buf());
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

    fn base_paths_for_view(&self) -> Vec<PathBuf> {
        match self.view {
            ListView::Status => self
                .status_entries
                .iter()
                .map(|entry| entry.path.clone())
                .collect(),
            ListView::Managed => self.managed_entries.clone(),
            ListView::Unmanaged => self.unmanaged_entries.clone(),
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

    fn read_children(&self, parent: &Path) -> Vec<PathBuf> {
        let abs_parent = self.resolve_path(parent);
        let Ok(read_dir) = fs::read_dir(abs_parent) else {
            return Vec::new();
        };

        let mut children: Vec<PathBuf> = read_dir
            .filter_map(Result::ok)
            .map(|entry| entry.file_name())
            .map(|name| {
                if parent.is_absolute() {
                    self.resolve_path(parent).join(name)
                } else {
                    PathBuf::from(parent).join(name)
                }
            })
            .collect();

        children.sort_by(|a, b| a.to_string_lossy().cmp(&b.to_string_lossy()));
        children
    }

    fn format_visible_entry(&self, entry: &VisibleEntry) -> String {
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
        let abs = self.resolve_path(path);
        fs::symlink_metadata(abs)
            .map(|meta| meta.file_type().is_dir())
            .unwrap_or(false)
    }

    fn resolve_path(&self, path: &Path) -> PathBuf {
        if path.is_absolute() {
            path.to_path_buf()
        } else {
            self.destination_dir.join(path)
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
        app.destination_dir = temp_root.clone();
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
}
