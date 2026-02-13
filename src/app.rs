use crate::config::AppConfig;
use crate::domain::{Action, ActionRequest, CommandResult, DiffText, ListView, StatusEntry};
use std::path::PathBuf;

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
    ActionFinished {
        request: ActionRequest,
        result: CommandResult,
    },
    Error {
        context: String,
        message: String,
    },
}

pub struct App {
    pub config: AppConfig,
    pub focus: PaneFocus,
    pub view: ListView,
    pub status_entries: Vec<StatusEntry>,
    pub managed_entries: Vec<PathBuf>,
    pub unmanaged_entries: Vec<PathBuf>,
    pub selected_index: usize,
    pub diff_text: String,
    pub logs: Vec<String>,
    pub modal: ModalState,
    pub busy: bool,
    pub pending_foreground: Option<ActionRequest>,
    pub should_quit: bool,
}

impl App {
    pub fn new(config: AppConfig) -> Self {
        let mut app = Self {
            config,
            focus: PaneFocus::List,
            view: ListView::Status,
            status_entries: Vec::new(),
            managed_entries: Vec::new(),
            unmanaged_entries: Vec::new(),
            selected_index: 0,
            diff_text: String::new(),
            logs: Vec::new(),
            modal: ModalState::None,
            busy: false,
            pending_foreground: None,
            should_quit: false,
        };

        app.log("起動: r=refresh d=diff a=action e=edit tab=focus q=quit".to_string());
        app
    }

    pub fn switch_view(&mut self, view: ListView) {
        self.view = view;
        self.selected_index = 0;
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
        match self.view {
            ListView::Status => self.status_entries.len(),
            ListView::Managed => self.managed_entries.len(),
            ListView::Unmanaged => self.unmanaged_entries.len(),
        }
    }

    pub fn current_items(&self) -> Vec<String> {
        match self.view {
            ListView::Status => self
                .status_entries
                .iter()
                .map(ToString::to_string)
                .collect(),
            ListView::Managed => self
                .managed_entries
                .iter()
                .map(|p| p.display().to_string())
                .collect(),
            ListView::Unmanaged => self
                .unmanaged_entries
                .iter()
                .map(|p| p.display().to_string())
                .collect(),
        }
    }

    pub fn selected_path(&self) -> Option<PathBuf> {
        match self.view {
            ListView::Status => self
                .status_entries
                .get(self.selected_index)
                .map(|entry| entry.path.clone()),
            ListView::Managed => self.managed_entries.get(self.selected_index).cloned(),
            ListView::Unmanaged => self.unmanaged_entries.get(self.selected_index).cloned(),
        }
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

    pub fn action_by_index(index: usize) -> Option<Action> {
        Action::ALL.get(index).copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::ChangeKind;

    #[test]
    fn status_selection_returns_path() {
        let mut app = App::new(AppConfig::default());
        app.status_entries = vec![StatusEntry {
            path: PathBuf::from(".zshrc"),
            actual_vs_state: ChangeKind::None,
            actual_vs_target: ChangeKind::Modified,
        }];
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
}
