//! Centralized action policy: visibility, target requirements, confirmation,
//! execution mode, output handling, and danger.
//!
//! Every action's behavioural specification lives here as a single source of
//! truth.  `domain.rs` methods (`is_dangerous`, `requires_confirmation`, …)
//! and `app.rs` helpers (`action_visible_in_view`, `action_disabled_reason`)
//! delegate to [`policy_for`] so that adding or changing an action only
//! requires updating this module.

use crate::domain::{Action, ListView};

// ---------------------------------------------------------------------------
// Policy types
// ---------------------------------------------------------------------------

/// Whether an action needs a target and, if so, what kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetPolicy {
    /// No target needed (e.g. `Doctor`, `Purge`).
    None,
    /// Target is accepted but optional (e.g. `Apply`).
    Optional,
    /// Target is required (e.g. `Merge`).
    Required,
    /// Target must be an exact managed entry (e.g. `Edit`, `Forget`).
    ExactManaged,
    /// Target must be a modified status file (e.g. `ReAdd`).
    ModifiedStatusFile,
    /// Target must be an existing non-directory file (e.g. `Add`).
    ExistingNonDirectory,
    /// Target must be an existing directory (e.g. `AddDirectory`).
    ExistingDirectory,
    /// Target must be an existing non-directory file in the source view.
    ExistingSourceFile,
    /// Target must be eligible for ignore (e.g. `Ignore`).
    IgnoreEligible,
}

/// Whether an action requires user confirmation before execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfirmationPolicy {
    /// No confirmation needed.
    None,
    /// Standard yes/no confirmation.
    Standard,
    /// Strict confirmation requiring the user to type a specific phrase.
    Strict(&'static str),
}

/// Workflow category used by both action ordering and action-menu headers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ActionGroup {
    ApplyAndSync,
    Files,
    Git,
    Inspect,
    Settings,
    Danger,
}

impl ActionGroup {
    pub const ALL: [Self; 6] = [
        Self::ApplyAndSync,
        Self::Files,
        Self::Git,
        Self::Inspect,
        Self::Settings,
        Self::Danger,
    ];

    pub fn title(self) -> &'static str {
        match self {
            Self::ApplyAndSync => "Apply & sync",
            Self::Files => "Files",
            Self::Git => "Git",
            Self::Inspect => "Inspect",
            Self::Settings => "Settings",
            Self::Danger => "Danger",
        }
    }
}

/// Aggregated policy for a single action.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ActionPolicy {
    /// Which list views this action is visible in.
    pub visible_in: &'static [ListView],
    /// Workflow category for deterministic ordering and menu headers.
    pub group: ActionGroup,
    /// Target requirement.
    pub target: TargetPolicy,
    /// Confirmation requirement.
    pub confirmation: ConfirmationPolicy,
    /// Whether the action avoids intentional source/destination changes.
    pub read_only: bool,
    /// Whether execution needs the restored terminal and inherited stdin/stdout.
    pub runs_foreground: bool,
    /// Whether command output should be shown as the action's primary result.
    pub shows_output: bool,
    /// Whether this action is destructive / dangerous.
    pub dangerous: bool,
}

// ---------------------------------------------------------------------------
// View-set constants
// ---------------------------------------------------------------------------

const STATUS_ONLY: &[ListView] = &[ListView::Status];
const MANAGED_ONLY: &[ListView] = &[ListView::Managed];
const UNMANAGED_ONLY: &[ListView] = &[ListView::Unmanaged];
const SOURCE_ONLY: &[ListView] = &[ListView::Source];

const STATUS_MANAGED: &[ListView] = &[ListView::Status, ListView::Managed];
const ALL_VIEWS: &[ListView] = &[
    ListView::Status,
    ListView::Managed,
    ListView::Unmanaged,
    ListView::Source,
];

// ---------------------------------------------------------------------------
// Policy lookup
// ---------------------------------------------------------------------------

/// Return the policy for the given action.
pub fn policy_for(action: Action) -> ActionPolicy {
    match action {
        Action::Apply => ActionPolicy {
            visible_in: ALL_VIEWS,
            group: ActionGroup::ApplyAndSync,
            target: TargetPolicy::Optional,
            confirmation: ConfirmationPolicy::Standard,
            read_only: false,
            runs_foreground: false,
            shows_output: false,
            dangerous: false,
        },
        Action::Doctor => ActionPolicy {
            visible_in: ALL_VIEWS,
            group: ActionGroup::Inspect,
            target: TargetPolicy::None,
            confirmation: ConfirmationPolicy::None,
            read_only: true,
            runs_foreground: false,
            shows_output: true,
            dangerous: false,
        },
        Action::Data => ActionPolicy {
            visible_in: ALL_VIEWS,
            group: ActionGroup::Inspect,
            target: TargetPolicy::None,
            confirmation: ConfirmationPolicy::None,
            read_only: true,
            runs_foreground: false,
            shows_output: true,
            dangerous: false,
        },
        Action::OpenSourceDir => ActionPolicy {
            visible_in: ALL_VIEWS,
            group: ActionGroup::Settings,
            target: TargetPolicy::None,
            confirmation: ConfirmationPolicy::None,
            read_only: false,
            runs_foreground: true,
            shows_output: false,
            dangerous: false,
        },
        Action::ExternalDiff => ActionPolicy {
            visible_in: ALL_VIEWS,
            group: ActionGroup::Inspect,
            target: TargetPolicy::None,
            confirmation: ConfirmationPolicy::None,
            read_only: true,
            runs_foreground: true,
            shows_output: false,
            dangerous: false,
        },
        Action::DebugContext => ActionPolicy {
            visible_in: ALL_VIEWS,
            group: ActionGroup::Inspect,
            target: TargetPolicy::None,
            confirmation: ConfirmationPolicy::None,
            read_only: true,
            runs_foreground: false,
            shows_output: true,
            dangerous: false,
        },
        Action::Update => ActionPolicy {
            visible_in: ALL_VIEWS,
            group: ActionGroup::ApplyAndSync,
            target: TargetPolicy::None,
            confirmation: ConfirmationPolicy::Standard,
            read_only: false,
            runs_foreground: true,
            shows_output: false,
            dangerous: false,
        },
        Action::EditConfig | Action::EditConfigTemplate | Action::EditIgnore => ActionPolicy {
            visible_in: ALL_VIEWS,
            group: ActionGroup::Settings,
            target: TargetPolicy::None,
            confirmation: ConfirmationPolicy::None,
            read_only: false,
            runs_foreground: true,
            shows_output: false,
            dangerous: false,
        },
        Action::ReAdd => ActionPolicy {
            visible_in: STATUS_ONLY,
            group: ActionGroup::Files,
            target: TargetPolicy::ModifiedStatusFile,
            confirmation: ConfirmationPolicy::None,
            read_only: false,
            runs_foreground: false,
            shows_output: false,
            dangerous: false,
        },
        Action::Merge => ActionPolicy {
            visible_in: STATUS_ONLY,
            group: ActionGroup::ApplyAndSync,
            target: TargetPolicy::Required,
            confirmation: ConfirmationPolicy::None,
            read_only: false,
            runs_foreground: true,
            shows_output: false,
            dangerous: false,
        },
        Action::MergeAll => ActionPolicy {
            visible_in: STATUS_ONLY,
            group: ActionGroup::ApplyAndSync,
            target: TargetPolicy::None,
            confirmation: ConfirmationPolicy::Standard,
            read_only: false,
            runs_foreground: true,
            shows_output: false,
            dangerous: false,
        },
        Action::Add => ActionPolicy {
            visible_in: UNMANAGED_ONLY,
            group: ActionGroup::Files,
            target: TargetPolicy::ExistingNonDirectory,
            confirmation: ConfirmationPolicy::None,
            read_only: false,
            runs_foreground: false,
            shows_output: false,
            dangerous: false,
        },
        Action::Ignore => ActionPolicy {
            visible_in: UNMANAGED_ONLY,
            group: ActionGroup::Files,
            target: TargetPolicy::IgnoreEligible,
            confirmation: ConfirmationPolicy::None,
            read_only: false,
            runs_foreground: false,
            shows_output: false,
            dangerous: false,
        },
        Action::Edit => ActionPolicy {
            visible_in: STATUS_MANAGED,
            group: ActionGroup::Files,
            target: TargetPolicy::ExactManaged,
            confirmation: ConfirmationPolicy::None,
            read_only: false,
            runs_foreground: true,
            shows_output: false,
            dangerous: false,
        },
        Action::Forget => ActionPolicy {
            visible_in: STATUS_MANAGED,
            group: ActionGroup::Files,
            target: TargetPolicy::ExactManaged,
            confirmation: ConfirmationPolicy::Standard,
            read_only: false,
            runs_foreground: false,
            shows_output: false,
            dangerous: false,
        },
        Action::Chattr => ActionPolicy {
            visible_in: STATUS_MANAGED,
            group: ActionGroup::Settings,
            target: TargetPolicy::ExactManaged,
            confirmation: ConfirmationPolicy::Standard,
            read_only: false,
            runs_foreground: false,
            shows_output: false,
            dangerous: false,
        },
        Action::Destroy => ActionPolicy {
            visible_in: MANAGED_ONLY,
            group: ActionGroup::Danger,
            target: TargetPolicy::ExactManaged,
            confirmation: ConfirmationPolicy::Strict("DESTROY"),
            read_only: false,
            runs_foreground: false,
            shows_output: false,
            dangerous: true,
        },
        Action::Purge => ActionPolicy {
            visible_in: ALL_VIEWS,
            group: ActionGroup::Danger,
            target: TargetPolicy::None,
            confirmation: ConfirmationPolicy::Strict("PURGE"),
            read_only: false,
            runs_foreground: false,
            shows_output: false,
            dangerous: true,
        },
        Action::Verify => ActionPolicy {
            visible_in: ALL_VIEWS,
            group: ActionGroup::Inspect,
            target: TargetPolicy::Optional,
            confirmation: ConfirmationPolicy::None,
            read_only: true,
            runs_foreground: false,
            shows_output: true,
            dangerous: false,
        },
        Action::Cat => ActionPolicy {
            visible_in: STATUS_MANAGED,
            group: ActionGroup::Inspect,
            target: TargetPolicy::ExactManaged,
            confirmation: ConfirmationPolicy::None,
            read_only: true,
            runs_foreground: false,
            shows_output: true,
            dangerous: false,
        },
        Action::Ignored => ActionPolicy {
            visible_in: ALL_VIEWS,
            group: ActionGroup::Inspect,
            target: TargetPolicy::None,
            confirmation: ConfirmationPolicy::None,
            read_only: true,
            runs_foreground: false,
            shows_output: true,
            dangerous: false,
        },
        Action::DumpConfig => ActionPolicy {
            visible_in: ALL_VIEWS,
            group: ActionGroup::Inspect,
            target: TargetPolicy::None,
            confirmation: ConfirmationPolicy::None,
            read_only: true,
            runs_foreground: false,
            shows_output: true,
            dangerous: false,
        },
        Action::DiffAll => ActionPolicy {
            visible_in: ALL_VIEWS,
            group: ActionGroup::Inspect,
            target: TargetPolicy::None,
            confirmation: ConfirmationPolicy::None,
            read_only: true,
            runs_foreground: false,
            shows_output: true,
            dangerous: false,
        },
        Action::GitStatus => ActionPolicy {
            visible_in: ALL_VIEWS,
            group: ActionGroup::Git,
            target: TargetPolicy::None,
            confirmation: ConfirmationPolicy::None,
            read_only: true,
            runs_foreground: false,
            shows_output: true,
            dangerous: false,
        },
        Action::GitDiff => ActionPolicy {
            visible_in: ALL_VIEWS,
            group: ActionGroup::Git,
            target: TargetPolicy::None,
            confirmation: ConfirmationPolicy::None,
            read_only: true,
            runs_foreground: false,
            shows_output: true,
            dangerous: false,
        },
        Action::GitLog => ActionPolicy {
            visible_in: ALL_VIEWS,
            group: ActionGroup::Git,
            target: TargetPolicy::None,
            confirmation: ConfirmationPolicy::None,
            read_only: true,
            runs_foreground: false,
            shows_output: true,
            dangerous: false,
        },
        Action::GitPull => ActionPolicy {
            visible_in: ALL_VIEWS,
            group: ActionGroup::Git,
            target: TargetPolicy::None,
            confirmation: ConfirmationPolicy::Standard,
            read_only: false,
            runs_foreground: true,
            shows_output: true,
            dangerous: false,
        },
        Action::GitPush => ActionPolicy {
            visible_in: SOURCE_ONLY,
            group: ActionGroup::Git,
            target: TargetPolicy::None,
            confirmation: ConfirmationPolicy::Standard,
            read_only: false,
            runs_foreground: true,
            shows_output: true,
            dangerous: false,
        },
        Action::GitCommit => ActionPolicy {
            visible_in: SOURCE_ONLY,
            group: ActionGroup::Git,
            target: TargetPolicy::None,
            confirmation: ConfirmationPolicy::Standard,
            read_only: false,
            runs_foreground: true,
            shows_output: true,
            dangerous: false,
        },
        Action::GitStage => ActionPolicy {
            visible_in: SOURCE_ONLY,
            group: ActionGroup::Git,
            target: TargetPolicy::ExistingSourceFile,
            confirmation: ConfirmationPolicy::Standard,
            read_only: false,
            runs_foreground: false,
            shows_output: false,
            dangerous: false,
        },
        Action::EditSource => ActionPolicy {
            visible_in: SOURCE_ONLY,
            group: ActionGroup::Files,
            target: TargetPolicy::ExistingSourceFile,
            confirmation: ConfirmationPolicy::None,
            read_only: false,
            runs_foreground: true,
            shows_output: false,
            dangerous: false,
        },
        Action::ReAddAll => ActionPolicy {
            visible_in: STATUS_ONLY,
            group: ActionGroup::Files,
            target: TargetPolicy::None,
            confirmation: ConfirmationPolicy::Standard,
            read_only: false,
            runs_foreground: false,
            shows_output: false,
            dangerous: false,
        },
        Action::Init => ActionPolicy {
            visible_in: ALL_VIEWS,
            group: ActionGroup::ApplyAndSync,
            target: TargetPolicy::None,
            confirmation: ConfirmationPolicy::Standard,
            read_only: false,
            runs_foreground: true,
            shows_output: true,
            dangerous: false,
        },
        Action::AddDirectory => ActionPolicy {
            visible_in: UNMANAGED_ONLY,
            group: ActionGroup::Files,
            target: TargetPolicy::ExistingDirectory,
            confirmation: ConfirmationPolicy::Standard,
            read_only: false,
            runs_foreground: false,
            shows_output: false,
            dangerous: false,
        },
        Action::PreviewAddDirectory => ActionPolicy {
            visible_in: UNMANAGED_ONLY,
            group: ActionGroup::Files,
            target: TargetPolicy::ExistingDirectory,
            confirmation: ConfirmationPolicy::None,
            read_only: true,
            runs_foreground: false,
            shows_output: true,
            dangerous: false,
        },
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn destructive_actions_are_marked_dangerous() {
        assert!(policy_for(Action::Destroy).dangerous);
        assert!(policy_for(Action::Purge).dangerous);
        assert!(!policy_for(Action::Apply).dangerous);
    }

    #[test]
    fn action_methods_delegate_to_policy() {
        for action in Action::ALL {
            let policy = policy_for(action);

            assert_eq!(
                action.is_dangerous(),
                policy.dangerous,
                "is_dangerous mismatch for {action:?}"
            );
            assert_eq!(
                action.requires_confirmation(),
                !matches!(policy.confirmation, ConfirmationPolicy::None),
                "requires_confirmation mismatch for {action:?}"
            );
            assert_eq!(
                action.requires_exact_managed_target(),
                matches!(policy.target, TargetPolicy::ExactManaged),
                "requires_exact_managed_target mismatch for {action:?}"
            );
        }
    }

    #[test]
    fn current_visibility_is_preserved_for_representative_actions() {
        assert_eq!(policy_for(Action::Apply).visible_in, ALL_VIEWS);
        assert_eq!(policy_for(Action::Chattr).visible_in, STATUS_MANAGED);
        assert_eq!(policy_for(Action::Destroy).visible_in, MANAGED_ONLY);
        assert_eq!(policy_for(Action::ReAdd).visible_in, STATUS_ONLY);
        assert_eq!(policy_for(Action::Merge).visible_in, STATUS_ONLY);
        assert_eq!(policy_for(Action::Add).visible_in, UNMANAGED_ONLY);
        assert_eq!(policy_for(Action::Ignore).visible_in, UNMANAGED_ONLY);
    }

    #[test]
    fn new_inspection_actions_have_typed_policies() {
        let verify = policy_for(Action::Verify);
        assert_eq!(verify.visible_in, ALL_VIEWS);
        assert_eq!(verify.target, TargetPolicy::Optional);
        assert_eq!(verify.confirmation, ConfirmationPolicy::None);
        assert!(verify.read_only && verify.shows_output);
        assert!(!verify.runs_foreground);

        let cat = policy_for(Action::Cat);
        assert_eq!(cat.visible_in, STATUS_MANAGED);
        assert_eq!(cat.target, TargetPolicy::ExactManaged);
        assert_eq!(cat.confirmation, ConfirmationPolicy::None);
        assert!(cat.read_only && cat.shows_output);

        let dump_config = policy_for(Action::DumpConfig);
        assert_eq!(dump_config.confirmation, ConfirmationPolicy::None);
        assert!(dump_config.read_only && dump_config.shows_output);

        for action in [
            Action::Ignored,
            Action::DiffAll,
            Action::GitStatus,
            Action::GitDiff,
            Action::GitLog,
        ] {
            let policy = policy_for(action);
            assert!(policy.read_only, "{action:?}");
            assert!(policy.shows_output, "{action:?}");
            assert_eq!(policy.confirmation, ConfirmationPolicy::None, "{action:?}");
        }
    }

    #[test]
    fn new_source_and_mutation_actions_have_typed_policies() {
        let git_pull = policy_for(Action::GitPull);
        assert!(!git_pull.read_only && git_pull.runs_foreground && git_pull.shows_output);
        assert_eq!(git_pull.confirmation, ConfirmationPolicy::Standard);

        for action in [Action::GitPush, Action::GitCommit] {
            let policy = policy_for(action);
            assert_eq!(policy.visible_in, SOURCE_ONLY, "{action:?}");
            assert!(!policy.read_only && policy.runs_foreground, "{action:?}");
            assert_eq!(
                policy.confirmation,
                ConfirmationPolicy::Standard,
                "{action:?}"
            );
        }

        for action in [Action::GitStage, Action::EditSource] {
            let policy = policy_for(action);
            assert_eq!(policy.visible_in, SOURCE_ONLY, "{action:?}");
            assert_eq!(
                policy.target,
                TargetPolicy::ExistingSourceFile,
                "{action:?}"
            );
        }

        let git_stage = policy_for(Action::GitStage);
        assert!(!git_stage.read_only && !git_stage.runs_foreground);
        assert_eq!(git_stage.confirmation, ConfirmationPolicy::Standard);

        let edit_source = policy_for(Action::EditSource);
        assert!(!edit_source.read_only && edit_source.runs_foreground);
        assert_eq!(edit_source.confirmation, ConfirmationPolicy::None);

        for action in [Action::ReAddAll, Action::Init, Action::AddDirectory] {
            let policy = policy_for(action);
            assert!(!policy.read_only, "{action:?}");
            assert_eq!(
                policy.confirmation,
                ConfirmationPolicy::Standard,
                "{action:?}"
            );
        }

        assert_eq!(
            policy_for(Action::AddDirectory).target,
            TargetPolicy::ExistingDirectory
        );
        assert!(policy_for(Action::Init).runs_foreground);
        assert_eq!(policy_for(Action::ReAddAll).visible_in, STATUS_ONLY);

        let preview = policy_for(Action::PreviewAddDirectory);
        assert_eq!(preview.visible_in, UNMANAGED_ONLY);
        assert_eq!(preview.target, TargetPolicy::ExistingDirectory);
        assert!(preview.read_only && preview.shows_output);
        assert_eq!(preview.confirmation, ConfirmationPolicy::None);
    }

    #[test]
    fn execution_metadata_distinguishes_shell_diff_and_inspection() {
        assert!(!Action::OpenSourceDir.is_read_only());
        assert!(Action::OpenSourceDir.runs_foreground());

        let external_diff = policy_for(Action::ExternalDiff);
        assert!(external_diff.read_only);
        assert!(external_diff.runs_foreground);
        assert!(!external_diff.shows_output);

        assert!(Action::PreviewAddDirectory.shows_output());
        assert!(!Action::GitStage.shows_output());
    }
}
