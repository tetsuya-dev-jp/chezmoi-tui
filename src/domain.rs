use std::fmt;
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeKind {
    None,
    Added,
    Deleted,
    Modified,
    Run,
    Unknown(char),
}

impl ChangeKind {
    pub fn from_status_char(c: char) -> Self {
        match c {
            ' ' => Self::None,
            'A' => Self::Added,
            'D' => Self::Deleted,
            'M' => Self::Modified,
            'R' => Self::Run,
            other => Self::Unknown(other),
        }
    }

    pub fn as_symbol(self) -> char {
        match self {
            Self::None => ' ',
            Self::Added => 'A',
            Self::Deleted => 'D',
            Self::Modified => 'M',
            Self::Run => 'R',
            Self::Unknown(c) => c,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusEntry {
    pub path: PathBuf,
    pub actual_vs_state: ChangeKind,
    pub actual_vs_target: ChangeKind,
}

impl fmt::Display for StatusEntry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}{} {}",
            self.actual_vs_state.as_symbol(),
            self.actual_vs_target.as_symbol(),
            self.path.display()
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffText {
    pub text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Apply,
    Update,
    ReAdd,
    Merge,
    MergeAll,
    Add,
    Edit,
    Forget,
    Chattr,
    Destroy,
    Purge,
}

impl Action {
    pub const ALL: [Action; 11] = [
        Action::Apply,
        Action::Update,
        Action::ReAdd,
        Action::Merge,
        Action::MergeAll,
        Action::Add,
        Action::Edit,
        Action::Forget,
        Action::Chattr,
        Action::Destroy,
        Action::Purge,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Action::Apply => "apply",
            Action::Update => "update",
            Action::ReAdd => "re-add",
            Action::Merge => "merge",
            Action::MergeAll => "merge-all",
            Action::Add => "add",
            Action::Edit => "edit",
            Action::Forget => "forget",
            Action::Chattr => "chattr",
            Action::Destroy => "destroy",
            Action::Purge => "purge",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Action::Apply => "apply target state to destination",
            Action::Update => "update source and apply changes",
            Action::ReAdd => "re-import modified files",
            Action::Merge => "run 3-way merge",
            Action::MergeAll => "run 3-way merge for all changes",
            Action::Add => "add existing file to managed set",
            Action::Edit => "edit source state in external editor",
            Action::Forget => "remove from managed set",
            Action::Chattr => "change source attributes",
            Action::Destroy => "delete from source/destination/state",
            Action::Purge => "remove chezmoi config and data",
        }
    }

    pub fn is_dangerous(self) -> bool {
        matches!(self, Action::Destroy | Action::Purge)
    }

    pub fn confirm_phrase(self) -> Option<&'static str> {
        match self {
            Action::Destroy => Some("DESTROY"),
            Action::Purge => Some("PURGE"),
            _ => None,
        }
    }

    pub fn needs_target(self) -> bool {
        matches!(
            self,
            Action::Merge
                | Action::Add
                | Action::Edit
                | Action::Forget
                | Action::Chattr
                | Action::Destroy
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActionRequest {
    pub action: Action,
    pub target: Option<PathBuf>,
    pub chattr_attrs: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandResult {
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
    pub duration_ms: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListView {
    Status,
    Managed,
    Unmanaged,
}

impl ListView {
    pub fn title(self) -> &'static str {
        match self {
            ListView::Status => "Status",
            ListView::Managed => "Managed",
            ListView::Unmanaged => "Unmanaged",
        }
    }
}
