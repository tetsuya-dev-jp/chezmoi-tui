//! Centralized action policy: visibility, target requirements, confirmation, danger.
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

/// Aggregated policy for a single action.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ActionPolicy {
    /// Which list views this action is visible in.
    pub visible_in: &'static [ListView],
    /// Target requirement.
    pub target: TargetPolicy,
    /// Confirmation requirement.
    pub confirmation: ConfirmationPolicy,
    /// Whether this action is destructive / dangerous.
    pub dangerous: bool,
}

// ---------------------------------------------------------------------------
// View-set constants
// ---------------------------------------------------------------------------

const STATUS_ONLY: &[ListView] = &[ListView::Status];
const MANAGED_ONLY: &[ListView] = &[ListView::Managed];
const UNMANAGED_ONLY: &[ListView] = &[ListView::Unmanaged];

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
            target: TargetPolicy::Optional,
            confirmation: ConfirmationPolicy::Standard,
            dangerous: false,
        },
        Action::Doctor => ActionPolicy {
            visible_in: ALL_VIEWS,
            target: TargetPolicy::None,
            confirmation: ConfirmationPolicy::None,
            dangerous: false,
        },
        Action::Data => ActionPolicy {
            visible_in: ALL_VIEWS,
            target: TargetPolicy::None,
            confirmation: ConfirmationPolicy::None,
            dangerous: false,
        },
        Action::OpenSourceDir => ActionPolicy {
            visible_in: ALL_VIEWS,
            target: TargetPolicy::None,
            confirmation: ConfirmationPolicy::None,
            dangerous: false,
        },
        Action::ExternalDiff => ActionPolicy {
            visible_in: ALL_VIEWS,
            target: TargetPolicy::None,
            confirmation: ConfirmationPolicy::None,
            dangerous: false,
        },
        Action::DebugContext => ActionPolicy {
            visible_in: ALL_VIEWS,
            target: TargetPolicy::None,
            confirmation: ConfirmationPolicy::None,
            dangerous: false,
        },
        Action::Update => ActionPolicy {
            visible_in: ALL_VIEWS,
            target: TargetPolicy::None,
            confirmation: ConfirmationPolicy::Standard,
            dangerous: false,
        },
        Action::EditConfig | Action::EditConfigTemplate | Action::EditIgnore => ActionPolicy {
            visible_in: ALL_VIEWS,
            target: TargetPolicy::None,
            confirmation: ConfirmationPolicy::None,
            dangerous: false,
        },
        Action::ReAdd => ActionPolicy {
            visible_in: STATUS_ONLY,
            target: TargetPolicy::ModifiedStatusFile,
            confirmation: ConfirmationPolicy::None,
            dangerous: false,
        },
        Action::Merge => ActionPolicy {
            visible_in: STATUS_ONLY,
            target: TargetPolicy::Required,
            confirmation: ConfirmationPolicy::None,
            dangerous: false,
        },
        Action::MergeAll => ActionPolicy {
            visible_in: STATUS_ONLY,
            target: TargetPolicy::None,
            confirmation: ConfirmationPolicy::Standard,
            dangerous: false,
        },
        Action::Add => ActionPolicy {
            visible_in: UNMANAGED_ONLY,
            target: TargetPolicy::ExistingNonDirectory,
            confirmation: ConfirmationPolicy::None,
            dangerous: false,
        },
        Action::Ignore => ActionPolicy {
            visible_in: UNMANAGED_ONLY,
            target: TargetPolicy::IgnoreEligible,
            confirmation: ConfirmationPolicy::None,
            dangerous: false,
        },
        Action::Edit => ActionPolicy {
            visible_in: STATUS_MANAGED,
            target: TargetPolicy::ExactManaged,
            confirmation: ConfirmationPolicy::None,
            dangerous: false,
        },
        Action::Forget => ActionPolicy {
            visible_in: STATUS_MANAGED,
            target: TargetPolicy::ExactManaged,
            confirmation: ConfirmationPolicy::Standard,
            dangerous: false,
        },
        Action::Chattr => ActionPolicy {
            visible_in: STATUS_MANAGED,
            target: TargetPolicy::ExactManaged,
            confirmation: ConfirmationPolicy::Standard,
            dangerous: false,
        },
        Action::Destroy => ActionPolicy {
            visible_in: MANAGED_ONLY,
            target: TargetPolicy::ExactManaged,
            confirmation: ConfirmationPolicy::Strict("DESTROY"),
            dangerous: true,
        },
        Action::Purge => ActionPolicy {
            visible_in: ALL_VIEWS,
            target: TargetPolicy::None,
            confirmation: ConfirmationPolicy::Strict("PURGE"),
            dangerous: true,
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
}
