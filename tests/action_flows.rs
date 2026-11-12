use std::fs;
use std::path::PathBuf;

use chezmoi_tui::AppConfig;
use chezmoi_tui::app::{
    App, ApplyPlan, ApplyPlanSnapshot, BackendEvent, BackendTask, DetailKind, ModalState,
    NoticeTone,
};
use chezmoi_tui::domain::{
    Action, ActionRequest, ChangeKind, CommandResult, ListView, StatusEntry,
};
use chezmoi_tui::handlers::{handle_backend_event, handle_key_event};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use tempfile::TempDir;
use tokio::sync::mpsc;

struct Fixture {
    _temp: TempDir,
    home: PathBuf,
    work: PathBuf,
    source: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().expect("temporary fixture");
        let home = temp.path().join("home");
        let work = temp.path().join("work");
        let source = temp.path().join("source");
        fs::create_dir_all(&home).expect("destination fixture");
        fs::create_dir_all(&work).expect("working fixture");
        fs::create_dir_all(&source).expect("source fixture");
        Self {
            _temp: temp,
            home,
            work,
            source,
        }
    }

    fn config(&self, view: ListView) -> AppConfig {
        AppConfig {
            initial_view: view,
            auto_preview: false,
            show_apply_plan: true,
            destination_dir: Some(self.home.clone()),
            source_dir: Some(self.source.clone()),
            working_dir: self.work.clone(),
            ..AppConfig::default()
        }
    }

    fn app(
        &self,
        view: ListView,
        status: Vec<StatusEntry>,
        managed: Vec<PathBuf>,
        unmanaged: Vec<PathBuf>,
        source: Vec<PathBuf>,
    ) -> App {
        let mut app = App::new(self.config(view));
        app.apply_refresh_entries(
            status,
            managed,
            unmanaged,
            Some(self.source.clone()),
            source,
        );
        app.rebuild_visible_entries();
        app
    }
}

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn press(app: &mut App, tx: &mpsc::Sender<BackendTask>, code: KeyCode) {
    handle_key_event(app, key(code), tx).expect("key event");
}

fn choose_action(app: &mut App, tx: &mpsc::Sender<BackendTask>, label: &str) {
    handle_key_event(
        app,
        KeyEvent::new(KeyCode::Char('p'), KeyModifiers::CONTROL),
        tx,
    )
    .expect("open action menu");
    for character in label.chars() {
        press(app, tx, KeyCode::Char(character));
    }
    press(app, tx, KeyCode::Enter);
}

fn queued_tasks(rx: &mut mpsc::Receiver<BackendTask>) -> Vec<BackendTask> {
    let mut tasks = Vec::new();
    while let Ok(task) = rx.try_recv() {
        tasks.push(task);
    }
    tasks
}

fn one_task(rx: &mut mpsc::Receiver<BackendTask>) -> BackendTask {
    let tasks = queued_tasks(rx);
    assert_eq!(tasks.len(), 1, "expected one queued task, got {tasks:?}");
    tasks.into_iter().next().expect("queued task")
}

fn assert_no_tasks(rx: &mut mpsc::Receiver<BackendTask>) {
    let tasks = queued_tasks(rx);
    assert!(tasks.is_empty(), "unexpected backend tasks: {tasks:?}");
}

fn status(path: &str, target: ChangeKind) -> StatusEntry {
    StatusEntry {
        path: PathBuf::from(path),
        actual_vs_state: ChangeKind::None,
        actual_vs_target: target,
    }
}

fn result(exit_code: i32, stdout: &str, stderr: &str) -> CommandResult {
    CommandResult {
        exit_code,
        stdout: stdout.to_string(),
        stderr: stderr.to_string(),
        duration_ms: 7,
        timed_out: false,
        output_limited: false,
        stdout_truncated: false,
        stderr_truncated: false,
    }
}

fn action_finished(request: ActionRequest, exit_code: i32, stdout: &str) -> BackendEvent {
    BackendEvent::ActionFinished {
        request,
        result: result(exit_code, stdout, ""),
    }
}

fn expect_action_task(
    task: BackendTask,
    expected_action: Action,
    expected_targets: &[PathBuf],
) -> (Option<u64>, ActionRequest) {
    match task {
        BackendTask::RunAction { request } => {
            assert_eq!(request.action, expected_action);
            assert_eq!(request.targets, expected_targets);
            (None, request)
        }
        BackendTask::Inspect {
            request_id,
            request,
        } => {
            assert_eq!(request.action, expected_action);
            assert_eq!(request.targets, expected_targets);
            (Some(request_id), request)
        }
        other => panic!("expected action task, got {other:?}"),
    }
}

#[test]
fn cat_uses_the_exact_managed_selection_and_renders_output_without_refresh_or_log_leak() {
    let fixture = Fixture::new();
    let target = fixture.home.join(".zshrc");
    fs::write(&target, "destination content").expect("managed destination fixture");
    let mut app = fixture.app(
        ListView::Status,
        vec![status(".zshrc", ChangeKind::Modified)],
        vec![PathBuf::from(".zshrc")],
        Vec::new(),
        Vec::new(),
    );
    let (task_tx, mut task_rx) = mpsc::channel(16);

    choose_action(&mut app, &task_tx, "cat");

    assert!(
        matches!(app.modal, ModalState::None),
        "cat must not ask for redundant confirmation"
    );
    let (request_id, request) = expect_action_task(
        one_task(&mut task_rx),
        Action::Cat,
        std::slice::from_ref(&target),
    );
    assert!(
        !app.has_mutating_work(),
        "cat must remain a read-only inspection"
    );

    let rendered = "synthetic rendered stdout: token=fixture-only";
    let request_id = request_id.expect("cat inspection request id");
    handle_backend_event(
        &mut app,
        &task_tx,
        BackendEvent::InspectionFinished {
            request_id,
            request,
            result: result(0, rendered, ""),
        },
    )
    .expect("action result");

    assert_eq!(app.detail_kind, DetailKind::Preview);
    assert_eq!(app.detail_text, rendered);
    assert!(app.detail_title.contains("cat"));
    assert!(
        app.logs.iter().all(|line| !line.contains(rendered)),
        "stdout payload must not be persisted in the log pane"
    );
    assert_no_tasks(&mut task_rx);
}

#[test]
fn cat_rejects_a_marked_unmanaged_status_entry() {
    let fixture = Fixture::new();
    fs::write(fixture.home.join(".zshrc"), "managed").expect("managed file");
    fs::write(fixture.home.join("scratch.txt"), "unmanaged").expect("unmanaged file");
    let mut app = fixture.app(
        ListView::Status,
        vec![
            status(".zshrc", ChangeKind::Modified),
            status("scratch.txt", ChangeKind::Modified),
        ],
        vec![PathBuf::from(".zshrc")],
        Vec::new(),
        Vec::new(),
    );
    let (task_tx, mut task_rx) = mpsc::channel(16);

    press(&mut app, &task_tx, KeyCode::Char('j'));
    press(&mut app, &task_tx, KeyCode::Char(' '));
    assert_eq!(app.marked_count(), 1);
    assert_eq!(
        app.selected_absolute_path(),
        Some(fixture.home.join("scratch.txt"))
    );

    choose_action(&mut app, &task_tx, "cat");

    assert!(matches!(app.modal, ModalState::None));
    assert_no_tasks(&mut task_rx);
    assert!(
        app.logs
            .iter()
            .any(|line| line.contains("exact managed entries")),
        "rejection should explain the managed-only target policy: {:?}",
        app.logs
    );
}

#[test]
fn verify_distinguishes_match_from_drift_without_failure_or_refresh() {
    let fixture = Fixture::new();
    fs::write(fixture.home.join(".zshrc"), "managed").expect("managed file");
    let mut app = fixture.app(
        ListView::Status,
        vec![status(".zshrc", ChangeKind::Modified)],
        vec![PathBuf::from(".zshrc")],
        Vec::new(),
        Vec::new(),
    );
    let (task_tx, mut task_rx) = mpsc::channel(16);
    choose_action(&mut app, &task_tx, "verify");
    let (request_id, request) = expect_action_task(one_task(&mut task_rx), Action::Verify, &[]);
    let request_id = request_id.expect("verify inspection id");
    assert!(!app.has_mutating_work());
    handle_backend_event(
        &mut app,
        &task_tx,
        BackendEvent::InspectionFinished {
            request_id,
            request,
            result: result(0, "verified: target state matches", ""),
        },
    )
    .expect("match result");

    let match_notice = app.latest_notice().cloned().expect("match notice");
    let match_detail = app.detail_text.clone();
    assert_no_tasks(&mut task_rx);

    choose_action(&mut app, &task_tx, "verify");
    let (request_id, request) = expect_action_task(one_task(&mut task_rx), Action::Verify, &[]);
    let request_id = request_id.expect("verify inspection id");
    handle_backend_event(
        &mut app,
        &task_tx,
        BackendEvent::InspectionFinished {
            request_id,
            request,
            result: result(1, "drift: .zshrc differs from target state", ""),
        },
    )
    .expect("drift result");

    let drift_notice = app.latest_notice().cloned().expect("drift notice");
    let drift_detail = app.detail_text.clone();
    let drift_logged_as_failure = app
        .logs
        .iter()
        .any(|line| line.contains("verify failed for"));
    assert_no_tasks(&mut task_rx);

    assert_eq!(match_notice.tone, NoticeTone::Success);
    assert!(match_notice.message.to_ascii_lowercase().contains("match"));
    assert!(match_detail.contains("target state matches"));
    assert_eq!(drift_notice.tone, NoticeTone::Info);
    assert!(
        drift_notice.message.to_ascii_lowercase().contains("drift"),
        "exit 1 must report drift rather than a generic command failure: {}",
        drift_notice.message
    );
    assert!(drift_detail.contains(".zshrc differs"));
    assert!(
        !drift_logged_as_failure,
        "exit 1 from verify must not be logged as a generic failure"
    );
}

#[test]
fn git_read_actions_render_their_primary_output_and_do_not_refresh() {
    let fixture = Fixture::new();
    let mut app = fixture.app(
        ListView::Status,
        Vec::new(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
    );
    let (task_tx, mut task_rx) = mpsc::channel(16);
    let mut failures = Vec::new();

    for (label, action, output, kind) in [
        (
            "git-status",
            Action::GitStatus,
            "## main\n M dot_zshrc",
            DetailKind::Preview,
        ),
        (
            "git-diff",
            Action::GitDiff,
            "diff --git a/dot_zshrc b/dot_zshrc\n-old\n+new\n",
            DetailKind::Diff,
        ),
        (
            "git-log",
            Action::GitLog,
            "abc123 fixture commit",
            DetailKind::Preview,
        ),
    ] {
        choose_action(&mut app, &task_tx, label);
        let (request_id, request) = expect_action_task(one_task(&mut task_rx), action, &[]);
        let request_id = request_id.expect("git inspection id");
        handle_backend_event(
            &mut app,
            &task_tx,
            BackendEvent::InspectionFinished {
                request_id,
                request,
                result: result(0, output, ""),
            },
        )
        .expect("git result");

        if app.detail_kind != kind {
            failures.push(format!(
                "{label}: detail kind {:?}, expected {kind:?}",
                app.detail_kind
            ));
        }
        if app.detail_text != output {
            failures.push(format!("{label}: detail output was {:?}", app.detail_text));
        }
        if !app.detail_title.contains(label) {
            failures.push(format!("{label}: detail title was {:?}", app.detail_title));
        }
        let tasks = queued_tasks(&mut task_rx);
        if !tasks.is_empty() {
            failures.push(format!("{label}: unexpected backend tasks {tasks:?}"));
        }
    }

    assert!(failures.is_empty(), "{failures:#?}");
}

#[test]
fn git_stage_is_confirmed_for_the_source_file_and_edit_source_stays_foreground_metadata() {
    let fixture = Fixture::new();
    let source_file = fixture.source.join("dot_file");
    fs::write(&source_file, "source content").expect("source file");

    let mut stage_app = fixture.app(
        ListView::Source,
        Vec::new(),
        Vec::new(),
        Vec::new(),
        vec![PathBuf::from("dot_file")],
    );
    let (stage_tx, mut stage_rx) = mpsc::channel(16);
    choose_action(&mut stage_app, &stage_tx, "git-stage");
    match &stage_app.modal {
        ModalState::Confirm { request, .. } => {
            assert_eq!(request.action, Action::GitStage);
            assert_eq!(request.targets, vec![source_file.clone()]);
        }
        other => panic!("git-stage must require confirmation, got {other:?}"),
    }
    assert_no_tasks(&mut stage_rx);

    press(&mut stage_app, &stage_tx, KeyCode::Enter);
    expect_action_task(
        one_task(&mut stage_rx),
        Action::GitStage,
        std::slice::from_ref(&source_file),
    );
    assert!(stage_app.has_mutating_work());

    let mut edit_app = fixture.app(
        ListView::Source,
        Vec::new(),
        Vec::new(),
        Vec::new(),
        vec![PathBuf::from("dot_file")],
    );
    let (edit_tx, mut edit_rx) = mpsc::channel(16);
    choose_action(&mut edit_app, &edit_tx, "edit-source");

    assert_no_tasks(&mut edit_rx);
    assert!(matches!(edit_app.modal, ModalState::None));
    let pending = edit_app
        .pending_foreground
        .as_ref()
        .expect("edit-source foreground metadata");
    assert_eq!(pending.action, Action::EditSource);
    assert_eq!(pending.targets, vec![source_file.clone()]);
    assert!(edit_app.has_mutating_work());
}

#[test]
fn directory_preview_is_read_only_but_add_requires_confirmation_and_stays_in_destination() {
    let fixture = Fixture::new();
    let import_target = fixture.home.join("imports");
    fs::create_dir(&import_target).expect("inside destination directory");
    let mut config = fixture.config(ListView::Unmanaged);
    config.working_dir = fixture.home.clone();
    let mut app = App::new(config);
    app.apply_refresh_entries(
        Vec::new(),
        Vec::new(),
        vec![PathBuf::from("imports")],
        Some(fixture.source.clone()),
        Vec::new(),
    );
    app.rebuild_visible_entries();
    let (task_tx, mut task_rx) = mpsc::channel(16);

    choose_action(&mut app, &task_tx, "preview-add-directory");
    let (preview_id, preview) = expect_action_task(
        one_task(&mut task_rx),
        Action::PreviewAddDirectory,
        std::slice::from_ref(&import_target),
    );
    assert!(
        !app.has_mutating_work(),
        "directory preview must be read-only"
    );
    handle_backend_event(
        &mut app,
        &task_tx,
        BackendEvent::InspectionFinished {
            request_id: preview_id.expect("preview inspection id"),
            request: preview,
            result: result(0, "would add imports", ""),
        },
    )
    .expect("directory preview");
    let preview_rendered = app.detail_text == "would add imports";
    assert_no_tasks(&mut task_rx);

    choose_action(&mut app, &task_tx, "add-directory");
    match &app.modal {
        ModalState::Confirm { request, .. } => {
            assert_eq!(request.action, Action::AddDirectory);
            assert_eq!(request.targets, vec![import_target.clone()]);
        }
        other => panic!("add-directory must require confirmation, got {other:?}"),
    }
    assert_no_tasks(&mut task_rx);
    press(&mut app, &task_tx, KeyCode::Enter);
    expect_action_task(
        one_task(&mut task_rx),
        Action::AddDirectory,
        &[import_target],
    );

    let outside = fixture._temp.path().join("outside");
    let outside_target = outside.join("imports");
    fs::create_dir_all(&outside_target).expect("outside directory");
    let mut outside_config = fixture.config(ListView::Unmanaged);
    outside_config.working_dir = outside;
    let mut outside_app = App::new(outside_config);
    outside_app.apply_refresh_entries(
        Vec::new(),
        Vec::new(),
        vec![PathBuf::from("imports")],
        Some(fixture.source.clone()),
        Vec::new(),
    );
    outside_app.rebuild_visible_entries();
    let (outside_tx, mut outside_rx) = mpsc::channel(16);

    choose_action(&mut outside_app, &outside_tx, "add-directory");
    assert!(matches!(outside_app.modal, ModalState::None));
    assert_no_tasks(&mut outside_rx);
    assert!(
        outside_app
            .logs
            .iter()
            .any(|line| line.contains("outside destination")),
        "outside destination rejection must explain the boundary: {:?}",
        outside_app.logs
    );
    assert!(
        preview_rendered,
        "directory preview output must be shown in detail"
    );
}

fn marked_apply_app(fixture: &Fixture) -> App {
    fs::write(fixture.home.join(".zshrc"), "first").expect("first target");
    fs::write(fixture.home.join(".vimrc"), "second").expect("second target");
    fixture.app(
        ListView::Status,
        vec![
            status(".zshrc", ChangeKind::Modified),
            status(".vimrc", ChangeKind::Modified),
        ],
        vec![PathBuf::from(".zshrc"), PathBuf::from(".vimrc")],
        Vec::new(),
        Vec::new(),
    )
}

fn mark_both_status_entries(app: &mut App, tx: &mpsc::Sender<BackendTask>) {
    press(app, tx, KeyCode::Char(' '));
    press(app, tx, KeyCode::Char('j'));
    press(app, tx, KeyCode::Char(' '));
    assert_eq!(app.marked_count(), 2);
}

#[test]
fn batch_apply_goes_directly_to_one_plan_for_all_targets() {
    let fixture = Fixture::new();
    let mut app = marked_apply_app(&fixture);
    let (task_tx, mut task_rx) = mpsc::channel(16);
    mark_both_status_entries(&mut app, &task_tx);

    choose_action(&mut app, &task_tx, "apply");
    assert!(
        matches!(app.modal, ModalState::None),
        "apply already has plan and final confirmations; it must not open a generic preflight"
    );
    let (request_id, request) = match one_task(&mut task_rx) {
        BackendTask::PrepareApplyPlan {
            request_id,
            request,
            ..
        } => (request_id, request),
        other => panic!("expected initial apply-plan review, got {other:?}"),
    };
    assert_eq!(request.action, Action::Apply);
    let mut targets = request.targets.clone();
    targets.sort();
    let mut expected_targets = vec![fixture.home.join(".zshrc"), fixture.home.join(".vimrc")];
    expected_targets.sort();
    assert_eq!(targets, expected_targets);
    assert_no_tasks(&mut task_rx);

    handle_backend_event(
        &mut app,
        &task_tx,
        BackendEvent::ApplyPlanPrepared {
            request_id,
            request,
            status: vec![
                status(".zshrc", ChangeKind::Modified),
                status(".vimrc", ChangeKind::Modified),
            ],
            snapshot: ApplyPlanSnapshot {
                targets: vec![fixture.home.join(".zshrc"), fixture.home.join(".vimrc")],
                plan: ApplyPlan {
                    modified: vec![PathBuf::from(".zshrc"), PathBuf::from(".vimrc")],
                    ..ApplyPlan::default()
                },
                diff_fingerprint: 41,
            },
        },
    )
    .expect("apply plan");

    let plan = match &app.modal {
        ModalState::ApplyPlan { snapshot, .. } => &snapshot.plan,
        other => panic!("expected apply plan modal, got {other:?}"),
    };
    let mut planned_targets = plan.modified.clone();
    planned_targets.sort();
    assert_eq!(
        planned_targets,
        vec![PathBuf::from(".vimrc"), PathBuf::from(".zshrc")],
        "the reviewed plan must include every target that the batch may execute"
    );
    assert_no_tasks(&mut task_rx);
}

#[test]
fn new_actions_are_rejected_while_mutation_batch_runs_without_clearing_it() {
    let fixture = Fixture::new();
    let mut app = marked_apply_app(&fixture);
    let (task_tx, mut task_rx) = mpsc::channel(16);
    let first = ActionRequest {
        action: Action::Forget,
        targets: vec![fixture.home.join(".zshrc")],
        chattr_attrs: None,
    };
    let second = ActionRequest {
        action: Action::Forget,
        targets: vec![fixture.home.join(".vimrc")],
        chattr_attrs: None,
    };
    app.start_batch(vec![first, second])
        .expect("existing batch starts");
    let batch_total = app.batch_total();
    let apply_request = ActionRequest {
        action: Action::Apply,
        targets: vec![fixture.home.join(".zshrc"), fixture.home.join(".vimrc")],
        chattr_attrs: None,
    };
    app.open_confirm_apply(
        apply_request.clone(),
        ApplyPlanSnapshot {
            targets: apply_request.targets.clone(),
            plan: ApplyPlan {
                modified: vec![PathBuf::from(".zshrc"), PathBuf::from(".vimrc")],
                ..ApplyPlan::default()
            },
            diff_fingerprint: 7,
        },
    );
    press(&mut app, &task_tx, KeyCode::Enter);
    assert!(matches!(
        one_task(&mut task_rx),
        BackendTask::ApplyReviewed { request, .. } if request == apply_request
    ));
    assert!(app.has_mutating_work());

    press(&mut app, &task_tx, KeyCode::Char('e'));
    assert!(matches!(app.modal, ModalState::None));
    assert!(app.pending_foreground.is_none());
    assert_no_tasks(&mut task_rx);

    choose_action(&mut app, &task_tx, "update");
    assert!(matches!(app.modal, ModalState::None));
    assert!(app.pending_foreground.is_none());
    assert_no_tasks(&mut task_rx);

    choose_action(&mut app, &task_tx, "doctor");
    assert!(matches!(app.modal, ModalState::None));
    assert!(app.pending_foreground.is_none());
    assert_no_tasks(&mut task_rx);

    assert_eq!(app.batch_action(), Some(Action::Forget));
    assert_eq!(app.batch_total(), batch_total);
    assert!(app.has_mutating_work());
    assert!(
        app.latest_notice()
            .is_some_and(|notice| notice.message.contains("doctor blocked")),
        "rejection must identify the attempted action: {:?}",
        app.latest_notice()
    );
}

#[test]
fn batch_apply_confirmation_enqueues_one_atomic_reviewed_job() {
    let fixture = Fixture::new();
    let mut app = marked_apply_app(&fixture);
    let (task_tx, mut task_rx) = mpsc::channel(16);
    mark_both_status_entries(&mut app, &task_tx);

    choose_action(&mut app, &task_tx, "apply");
    let (initial_id, initial_request) = match one_task(&mut task_rx) {
        BackendTask::PrepareApplyPlan {
            request_id,
            request,
            ..
        } => (request_id, request),
        other => panic!("expected initial apply plan, got {other:?}"),
    };

    let fresh_status = vec![
        status(".zshrc", ChangeKind::Modified),
        status(".vimrc", ChangeKind::Modified),
    ];
    handle_backend_event(
        &mut app,
        &task_tx,
        BackendEvent::ApplyPlanPrepared {
            request_id: initial_id,
            request: initial_request,
            status: fresh_status.clone(),
            snapshot: ApplyPlanSnapshot {
                targets: vec![fixture.home.join(".vimrc"), fixture.home.join(".zshrc")],
                plan: ApplyPlan {
                    modified: vec![PathBuf::from(".vimrc"), PathBuf::from(".zshrc")],
                    ..ApplyPlan::default()
                },
                diff_fingerprint: 10,
            },
        },
    )
    .expect("initial apply plan");

    press(&mut app, &task_tx, KeyCode::Enter);
    assert!(matches!(app.modal, ModalState::ConfirmApply { .. }));
    assert_no_tasks(&mut task_rx);
    press(&mut app, &task_tx, KeyCode::Enter);
    let (validation_id, validation_request, expected_snapshot) = match one_task(&mut task_rx) {
        BackendTask::ApplyReviewed {
            request_id,
            request,
            expected_snapshot,
            ..
        } => (request_id, request, expected_snapshot),
        other => panic!("expected serialized reviewed apply, got {other:?}"),
    };
    assert_eq!(
        validation_request.targets,
        vec![fixture.home.join(".vimrc"), fixture.home.join(".zshrc")]
    );
    assert!(app.has_mutating_work());

    handle_backend_event(
        &mut app,
        &task_tx,
        BackendEvent::ApplyPlanChanged {
            request_id: validation_id,
            request: validation_request,
            status: fresh_status,
            snapshot: ApplyPlanSnapshot {
                targets: expected_snapshot.targets.clone(),
                plan: ApplyPlan {
                    modified: vec![PathBuf::from(".vimrc"), PathBuf::from(".zshrc")],
                    ..ApplyPlan::default()
                },
                diff_fingerprint: 99,
            },
        },
    )
    .expect("changed apply plan");

    assert!(
        matches!(app.modal, ModalState::ApplyPlan { .. }),
        "a changed snapshot must return to plan review"
    );
    assert!(
        app.latest_notice()
            .is_some_and(|notice| notice.message.contains("changed")),
        "changed plan must produce a useful notice"
    );
    assert_no_tasks(&mut task_rx);

    press(&mut app, &task_tx, KeyCode::Enter);
    assert!(matches!(app.modal, ModalState::ConfirmApply { .. }));
    press(&mut app, &task_tx, KeyCode::Enter);
    let request = match one_task(&mut task_rx) {
        BackendTask::ApplyReviewed {
            request,
            expected_snapshot,
            ..
        } => {
            assert_eq!(expected_snapshot.diff_fingerprint, 99);
            request
        }
        other => panic!("expected reviewed apply, got {other:?}"),
    };
    assert_no_tasks(&mut task_rx);

    handle_backend_event(
        &mut app,
        &task_tx,
        action_finished(request, 0, "applied reviewed targets"),
    )
    .expect("apply result");
    match one_task(&mut task_rx) {
        BackendTask::RefreshAll => {
            assert!(!app.batch_in_progress());
        }
        other => panic!("one successful aggregate apply should refresh, got {other:?}"),
    }
    assert_no_tasks(&mut task_rx);
}
