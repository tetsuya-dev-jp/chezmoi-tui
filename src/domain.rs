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
    Doctor,
    Data,
    OpenSourceDir,
    ExternalDiff,
    DebugContext,
    Update,
    EditConfig,
    EditConfigTemplate,
    EditIgnore,
    ReAdd,
    Merge,
    MergeAll,
    Add,
    Ignore,
    Edit,
    Forget,
    Chattr,
    Destroy,
    Purge,
    Verify,
    Cat,
    Ignored,
    DumpConfig,
    DiffAll,
    GitStatus,
    GitDiff,
    GitLog,
    GitPull,
    GitPush,
    GitCommit,
    GitStage,
    EditSource,
    ReAddAll,
    Init,
    AddDirectory,
    PreviewAddDirectory,
}

impl Action {
    pub const ALL: [Action; 37] = [
        Action::Apply,
        Action::Doctor,
        Action::Data,
        Action::OpenSourceDir,
        Action::ExternalDiff,
        Action::DebugContext,
        Action::Update,
        Action::EditConfig,
        Action::EditConfigTemplate,
        Action::EditIgnore,
        Action::ReAdd,
        Action::Merge,
        Action::MergeAll,
        Action::Add,
        Action::Ignore,
        Action::Edit,
        Action::Forget,
        Action::Chattr,
        Action::Destroy,
        Action::Purge,
        Action::Verify,
        Action::Cat,
        Action::Ignored,
        Action::DumpConfig,
        Action::DiffAll,
        Action::GitStatus,
        Action::GitDiff,
        Action::GitLog,
        Action::GitPull,
        Action::GitPush,
        Action::GitCommit,
        Action::GitStage,
        Action::EditSource,
        Action::ReAddAll,
        Action::Init,
        Action::AddDirectory,
        Action::PreviewAddDirectory,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Action::Apply => "apply",
            Action::Doctor => "doctor",
            Action::Data => "data",
            Action::OpenSourceDir => "open-source-dir",
            Action::ExternalDiff => "external-diff",
            Action::DebugContext => "debug-context",
            Action::Update => "update",
            Action::EditConfig => "edit-config",
            Action::EditConfigTemplate => "edit-config-template",
            Action::EditIgnore => "edit-ignore",
            Action::ReAdd => "re-add",
            Action::Merge => "merge",
            Action::MergeAll => "merge-all",
            Action::Add => "add",
            Action::Ignore => "ignore",
            Action::Edit => "edit",
            Action::Forget => "forget",
            Action::Chattr => "chattr",
            Action::Destroy => "destroy",
            Action::Purge => "purge",
            Action::Verify => "verify",
            Action::Cat => "cat",
            Action::Ignored => "ignored",
            Action::DumpConfig => "dump-config",
            Action::DiffAll => "diff-all",
            Action::GitStatus => "git-status",
            Action::GitDiff => "git-diff",
            Action::GitLog => "git-log",
            Action::GitPull => "git-pull",
            Action::GitPush => "git-push",
            Action::GitCommit => "git-commit",
            Action::GitStage => "git-stage",
            Action::EditSource => "edit-source",
            Action::ReAddAll => "re-add-all",
            Action::Init => "init",
            Action::AddDirectory => "add-directory",
            Action::PreviewAddDirectory => "preview-add-directory",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Action::Apply => "apply target state to destination",
            Action::Doctor => "run chezmoi diagnostics",
            Action::Data => "show template data as JSON",
            Action::OpenSourceDir => "open an interactive shell in the source directory",
            Action::ExternalDiff => "open full diff in configured external diff tool",
            Action::DebugContext => "show current app/view/debug context",
            Action::Update => "update source and apply changes",
            Action::EditConfig => "edit chezmoi config file",
            Action::EditConfigTemplate => "edit chezmoi config template",
            Action::EditIgnore => "edit .chezmoiignore",
            Action::ReAdd => "re-import selected modified file",
            Action::Merge => "run 3-way merge",
            Action::MergeAll => "run 3-way merge for all changes",
            Action::Add => "add existing file to managed set",
            Action::Ignore => "append target to .chezmoiignore",
            Action::Edit => "edit source state in external editor",
            Action::Forget => "remove from managed set",
            Action::Chattr => "change source attributes",
            Action::Destroy => "delete from source/destination/state",
            Action::Purge => "remove chezmoi config and data",
            Action::Verify => {
                "verify destination matches target state, or all when no target is selected"
            }
            Action::Cat => "render selected managed target contents on explicit request",
            Action::Ignored => "list targets ignored by chezmoi",
            Action::DumpConfig => "show effective chezmoi config as JSON on explicit request",
            Action::DiffAll => "show the complete destination diff",
            Action::GitStatus => "show source repository status",
            Action::GitDiff => "show source repository diff",
            Action::GitLog => "show source repository history",
            Action::GitPull => "pull source changes without applying them",
            Action::GitPush => "push existing commits from the source repository",
            Action::GitCommit => "commit already staged source repository changes",
            Action::GitStage => "stage selected source files",
            Action::EditSource => "edit selected source file in an external editor",
            Action::ReAddAll => "re-import all modified non-template files",
            Action::Init => "initialize a missing chezmoi source",
            Action::AddDirectory => "recursively add an unmanaged directory",
            Action::PreviewAddDirectory => "preview a recursive unmanaged directory import",
        }
    }

    pub fn is_dangerous(self) -> bool {
        crate::action_policy::policy_for(self).dangerous
    }

    pub fn is_read_only(self) -> bool {
        crate::action_policy::policy_for(self).read_only
    }

    pub fn runs_foreground(self) -> bool {
        crate::action_policy::policy_for(self).runs_foreground
    }

    pub fn shows_output(self) -> bool {
        crate::action_policy::policy_for(self).shows_output
    }

    pub fn group(self) -> crate::action_policy::ActionGroup {
        crate::action_policy::policy_for(self).group
    }

    pub fn requires_confirmation(self) -> bool {
        !matches!(
            crate::action_policy::policy_for(self).confirmation,
            crate::action_policy::ConfirmationPolicy::None
        )
    }

    pub fn confirm_phrase(self) -> Option<&'static str> {
        match crate::action_policy::policy_for(self).confirmation {
            crate::action_policy::ConfirmationPolicy::Strict(phrase) => Some(phrase),
            _ => None,
        }
    }

    pub fn needs_target(self) -> bool {
        !matches!(
            crate::action_policy::policy_for(self).target,
            crate::action_policy::TargetPolicy::None | crate::action_policy::TargetPolicy::Optional
        )
    }

    pub fn requires_exact_managed_target(self) -> bool {
        matches!(
            crate::action_policy::policy_for(self).target,
            crate::action_policy::TargetPolicy::ExactManaged
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActionRequest {
    pub action: Action,
    pub targets: Vec<PathBuf>,
    pub chattr_attrs: Option<String>,
}

impl ActionRequest {
    pub fn requires_strict_confirmation(&self) -> bool {
        matches!(
            crate::action_policy::policy_for(self.action).confirmation,
            crate::action_policy::ConfirmationPolicy::Strict(_)
        )
    }

    pub fn confirmation_phrase(&self) -> Option<String> {
        let base = self.action.confirm_phrase()?;
        match self.action {
            Action::Destroy => self
                .targets
                .first()
                .map(|target| format!("{base} {}", target.display())),
            Action::Purge => Some(format!("{base} ALL")),
            _ => Some(base.to_string()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandResult {
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
    pub duration_ms: u64,
    pub timed_out: bool,
    pub output_limited: bool,
    pub stdout_truncated: bool,
    pub stderr_truncated: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListView {
    Status,
    Managed,
    Unmanaged,
    Source,
}

impl ListView {
    pub fn title(self) -> &'static str {
        match self {
            ListView::Status => "Status",
            ListView::Managed => "Managed",
            ListView::Unmanaged => "Unmanaged",
            ListView::Source => "Source",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn action_request_confirmation_phrase_includes_target_for_destroy() {
        let req = ActionRequest {
            action: Action::Destroy,
            targets: vec![PathBuf::from("/tmp/demo.txt")],
            chattr_attrs: None,
        };
        assert_eq!(
            req.confirmation_phrase(),
            Some("DESTROY /tmp/demo.txt".to_string())
        );
    }

    #[test]
    fn action_request_confirmation_phrase_is_all_for_purge() {
        let req = ActionRequest {
            action: Action::Purge,
            targets: Vec::new(),
            chattr_attrs: None,
        };
        assert_eq!(req.confirmation_phrase(), Some("PURGE ALL".to_string()));
    }

    #[test]
    fn readd_action_requires_target() {
        assert!(Action::ReAdd.needs_target());
        assert_eq!(
            Action::ReAdd.description(),
            "re-import selected modified file"
        );
    }

    #[test]
    fn action_helpers_delegate_to_policy() {
        for action in Action::ALL {
            let policy = crate::action_policy::policy_for(action);
            assert_eq!(action.is_read_only(), policy.read_only);
            assert_eq!(action.runs_foreground(), policy.runs_foreground);
            assert_eq!(action.shows_output(), policy.shows_output);
        }
    }

    #[test]
    fn interactive_shell_is_mutating_and_read_only_helpers_are_specific() {
        assert!(!Action::OpenSourceDir.is_read_only());
        assert!(Action::OpenSourceDir.runs_foreground());
        assert!(Action::ExternalDiff.is_read_only());
        assert!(Action::ExternalDiff.runs_foreground());
        assert!(Action::GitStatus.is_read_only());
        assert!(!Action::GitPull.is_read_only());
    }
}
