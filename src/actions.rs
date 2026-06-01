use crate::app::{App, BackendTask, InputKind, NoticeTone};
use crate::config::AppConfig;
use crate::domain::{Action, ActionRequest, ListView};
use crate::ignore::{chezmoi_ignore_path_with_source, run_internal_ignore_action};
use crate::infra::action_to_args;
use crate::terminal::{restore_terminal, setup_terminal};
use anyhow::{Context, Result};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use std::fs::{self, OpenOptions};
use std::io;
use std::path::Path;
use std::process::Command;
use std::time::Instant;
use tokio::sync::mpsc::UnboundedSender;

pub(crate) fn send_task(
    app: &mut App,
    task_tx: &UnboundedSender<BackendTask>,
    task: BackendTask,
) -> Result<()> {
    let label = backend_task_label(&task);
    task_tx
        .send(task)
        .map_err(|err| anyhow::anyhow!("failed to dispatch task: {err}"))?;
    app.begin_busy_task_with_message(label);
    Ok(())
}

fn backend_task_label(task: &BackendTask) -> String {
    match task {
        BackendTask::RefreshAll => "refresh".to_string(),
        BackendTask::LoadDiff { target, .. } => target.as_ref().map_or_else(
            || "diff all".to_string(),
            |path| format!("diff {}", path.display()),
        ),
        BackendTask::LoadPreview { target, .. } => format!("preview {}", target.display()),
        BackendTask::RunAction { request } => format!(
            "{} {}",
            request.action.label(),
            request
                .target
                .as_ref()
                .map_or_else(|| "(none)".to_string(), |path| path.display().to_string())
        ),
    }
}

pub(crate) fn run_foreground_action(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut App,
    task_tx: &UnboundedSender<BackendTask>,
    request: &ActionRequest,
) -> Result<()> {
    restore_terminal(terminal)?;

    let result = run_action_foreground(request, &app.config);

    setup_terminal()?;
    terminal.clear()?;

    app.finish_busy_task();

    match result {
        Ok((code, elapsed)) => {
            let target = request
                .target
                .as_ref()
                .map_or_else(|| "(none)".to_string(), |p| p.display().to_string());
            let message = format!(
                "foreground action done: {} {} exit={} duration={}ms",
                request.action.label(),
                target,
                code,
                elapsed
            );
            app.log(message.clone());
            if code == 0 {
                app.set_success_notice(format!(
                    "{} completed for {}",
                    request.action.label(),
                    target
                ));
            } else {
                app.set_error_notice(format!(
                    "{} failed for {} (exit={})",
                    request.action.label(),
                    target,
                    code
                ));
            }

            if app.batch_in_progress() {
                maybe_continue_batch(app, task_tx)?;
            } else if code == 0 {
                send_task(app, task_tx, BackendTask::RefreshAll)?;
            }
        }
        Err(err) => {
            app.log(format!("foreground action error: {err:#}"));
            app.set_notice(
                NoticeTone::Error,
                format!("foreground action error: {err:#}"),
            );
            if app.batch_in_progress() {
                maybe_continue_batch(app, task_tx)?;
            }
        }
    }

    Ok(())
}

fn run_action_foreground(request: &ActionRequest, config: &AppConfig) -> Result<(i32, u64)> {
    match request.action {
        Action::EditIgnore => run_edit_ignore_foreground(config),
        Action::OpenSourceDir => run_open_source_dir_foreground(config),
        Action::ExternalDiff => run_external_diff_foreground(config),
        _ => run_chezmoi_foreground(request, config),
    }
}

pub(crate) fn dispatch_action_request(
    app: &mut App,
    task_tx: &UnboundedSender<BackendTask>,
    request: ActionRequest,
) -> Result<()> {
    if request.action == Action::Chattr && request.chattr_attrs.is_none() {
        app.open_input(InputKind::ChattrAttrs, request);
        return Ok(());
    }
    if request.action == Action::Apply && app.config.show_apply_plan && !app.batch_in_progress() {
        app.open_apply_plan(request);
        return Ok(());
    }
    if request.action.requires_confirmation() && !app.batch_confirmed() {
        app.open_confirm(request);
        return Ok(());
    }
    execute_action_request(app, task_tx, request)
}

pub(crate) fn execute_action_request(
    app: &mut App,
    task_tx: &UnboundedSender<BackendTask>,
    request: ActionRequest,
) -> Result<()> {
    if request.action == Action::DebugContext {
        app.set_detail_preview(
            std::path::Path::new("debug-context"),
            crate::app::PreviewOrigin::Source,
            app.debug_context_text(),
        );
        app.set_info_notice("debug context generated");
        return Ok(());
    }

    if request.action == Action::Ignore {
        match run_internal_ignore_action(app, &request) {
            Ok(()) => {
                if app.batch_in_progress() {
                    maybe_continue_batch(app, task_tx)?;
                } else {
                    send_task(app, task_tx, BackendTask::RefreshAll)?;
                }
            }
            Err(err) => {
                app.log(format!("ignore action error: {err:#}"));
                if app.batch_in_progress() {
                    maybe_continue_batch(app, task_tx)?;
                }
            }
        }
        return Ok(());
    }

    if matches!(
        request.action,
        Action::Edit
            | Action::Update
            | Action::Merge
            | Action::MergeAll
            | Action::EditConfig
            | Action::EditConfigTemplate
            | Action::EditIgnore
            | Action::OpenSourceDir
            | Action::ExternalDiff
    ) {
        let label = format!(
            "{} {}",
            request.action.label(),
            request
                .target
                .as_ref()
                .map_or_else(|| "(none)".to_string(), |path| path.display().to_string())
        );
        app.pending_foreground = Some(request);
        app.begin_busy_task_with_message(label);
    } else {
        send_task(app, task_tx, BackendTask::RunAction { request })?;
    }
    Ok(())
}

pub(crate) fn maybe_continue_batch(
    app: &mut App,
    task_tx: &UnboundedSender<BackendTask>,
) -> Result<()> {
    if !app.batch_in_progress() {
        return Ok(());
    }

    if let Some(next) = app.pop_next_batch_request() {
        dispatch_action_request(app, task_tx, next)?;
        return Ok(());
    }

    let action = app
        .batch_action()
        .map_or("unknown", super::domain::Action::label);
    let total = app.batch_total();
    app.log(format!("batch completed: action={action} total={total}"));
    app.clear_batch();
    send_task(app, task_tx, BackendTask::RefreshAll)
}

pub(crate) fn build_action_requests(app: &App, action: Action) -> Vec<ActionRequest> {
    if action == Action::Apply
        && app.marked_count() > 0
        && matches!(app.view, ListView::Status | ListView::Managed)
    {
        let targets = app.selected_action_targets_absolute();
        if !targets.is_empty() {
            return targets
                .into_iter()
                .map(|target| ActionRequest {
                    action,
                    target: Some(target),
                    chattr_attrs: None,
                })
                .collect();
        }
    }

    if !action.needs_target() {
        return vec![ActionRequest {
            action,
            target: None,
            chattr_attrs: None,
        }];
    }

    app.selected_action_targets_absolute()
        .into_iter()
        .map(|target| ActionRequest {
            action,
            target: Some(target),
            chattr_attrs: None,
        })
        .collect()
}

pub(crate) fn validate_action_requests(
    app: &App,
    action: Action,
    requests: &[ActionRequest],
) -> Option<String> {
    if requests.is_empty() {
        return Some(format!("{} requires a target file", action.label()));
    }

    let targets: Vec<&Path> = requests
        .iter()
        .filter_map(|req| req.target.as_deref())
        .collect();

    if action == Action::Add && targets.iter().any(|path| path.is_dir()) {
        return Some(
            "Adding a whole directory is disabled. Expand it and select only required files."
                .to_string(),
        );
    }

    if action == Action::ReAdd {
        if targets.iter().any(|path| path.is_dir()) {
            return Some("re-add is available only for files".to_string());
        }

        if !app.readd_selection_is_eligible() {
            return Some("re-add is available only for modified files in status view".to_string());
        }
    }

    if action == Action::Edit
        && targets
            .iter()
            .any(|path| !app.is_absolute_path_managed(path))
    {
        return Some("edit is available only for managed files".to_string());
    }

    None
}

fn run_chezmoi_foreground(request: &ActionRequest, config: &AppConfig) -> Result<(i32, u64)> {
    let args = action_to_args(request)?;
    let destination_dir =
        infer_destination_for_target_with_config(request.target.as_deref(), config);
    let started = Instant::now();
    let mut command = Command::new("chezmoi");
    command.arg("--destination").arg(destination_dir);
    if let Some(source_dir) = &config.source_dir {
        command.arg("--source").arg(source_dir);
    }
    let status = command
        .args(args)
        .status()
        .context("failed to start foreground chezmoi command")?;
    let elapsed = elapsed_millis_u64(started);

    Ok((status.code().unwrap_or(-1), elapsed))
}

fn run_open_source_dir_foreground(config: &AppConfig) -> Result<(i32, u64)> {
    let source_dir = resolve_source_dir_for_foreground(config)?;
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "sh".to_string());
    let started = Instant::now();
    let status = Command::new(shell)
        .current_dir(&source_dir)
        .status()
        .with_context(|| format!("failed to open shell in {}", source_dir.display()))?;
    let elapsed = elapsed_millis_u64(started);
    Ok((status.code().unwrap_or(-1), elapsed))
}

fn run_external_diff_foreground(config: &AppConfig) -> Result<(i32, u64)> {
    let tool = config
        .external_diff
        .clone()
        .or_else(|| std::env::var("CHEZMOI_TUI_EXTERNAL_DIFF").ok())
        .unwrap_or_else(|| "delta".to_string());
    let destination_dir = config
        .destination_dir
        .clone()
        .or_else(dirs::home_dir)
        .unwrap_or_else(|| config.working_dir.clone());
    let mut command = format!(
        "chezmoi --destination {}",
        shell_quote(&destination_dir.display().to_string())
    );
    if let Some(source_dir) = &config.source_dir {
        command.push_str(" --source ");
        command.push_str(&shell_quote(&source_dir.display().to_string()));
    }
    command.push_str(" diff --no-pager --use-builtin-diff --color=true | ");
    command.push_str(&tool);

    let started = Instant::now();
    let status = Command::new("sh")
        .arg("-c")
        .arg(&command)
        .status()
        .context("failed to launch external diff command")?;
    let elapsed = elapsed_millis_u64(started);
    Ok((status.code().unwrap_or(-1), elapsed))
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn resolve_source_dir_for_foreground(config: &AppConfig) -> Result<std::path::PathBuf> {
    if let Some(source_dir) = &config.source_dir {
        return Ok(source_dir.clone());
    }

    let output = Command::new("chezmoi")
        .arg("source-path")
        .output()
        .context("failed to execute chezmoi source-path")?;
    if !output.status.success() {
        anyhow::bail!(
            "chezmoi source-path failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    let source_dir = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if source_dir.is_empty() {
        anyhow::bail!("chezmoi source-path returned empty output");
    }
    Ok(std::path::PathBuf::from(source_dir))
}

fn run_edit_ignore_foreground(config: &AppConfig) -> Result<(i32, u64)> {
    let ignore_path = chezmoi_ignore_path_with_source(config.source_dir.as_deref())?;
    if let Some(parent) = ignore_path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    OpenOptions::new()
        .create(true)
        .append(true)
        .open(&ignore_path)
        .with_context(|| format!("failed to open {}", ignore_path.display()))?;

    let started = Instant::now();
    let status = Command::new("sh")
        .arg("-c")
        .arg("${VISUAL:-${EDITOR:-vi}} \"$1\"")
        .arg("sh")
        .arg(&ignore_path)
        .status()
        .with_context(|| format!("failed to launch editor for {}", ignore_path.display()))?;
    let elapsed = elapsed_millis_u64(started);

    Ok((status.code().unwrap_or(-1), elapsed))
}

fn infer_destination_for_target_with_config(
    target: Option<&Path>,
    config: &AppConfig,
) -> std::path::PathBuf {
    let working_dir = config.working_dir.clone();
    let home_dir = config
        .destination_dir
        .clone()
        .or_else(dirs::home_dir)
        .unwrap_or_else(|| working_dir.clone());

    destination_for_target_with_bases(target, &home_dir, &working_dir)
}

pub(crate) fn destination_for_target_with_bases(
    target: Option<&Path>,
    home_dir: &Path,
    working_dir: &Path,
) -> std::path::PathBuf {
    match target {
        Some(path) if path.is_absolute() && path.starts_with(home_dir) => home_dir.to_path_buf(),
        Some(path) if path.is_absolute() && path.starts_with(working_dir) => {
            working_dir.to_path_buf()
        }
        Some(path) if path.is_absolute() => home_dir.to_path_buf(),
        Some(_) => working_dir.to_path_buf(),
        None => home_dir.to_path_buf(),
    }
}

fn elapsed_millis_u64(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
}

pub(crate) fn squash_lines(input: &str) -> String {
    input
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .take(5)
        .collect::<Vec<_>>()
        .join(" | ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::AppConfig;
    use crate::domain::ChangeKind;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};
    use tokio::sync::mpsc;

    #[test]
    fn squash_lines_limits_output() {
        let text = "a\n\n b\n c \n d\n e\n f\n";
        let got = squash_lines(text);
        assert_eq!(got, "a | b | c | d | e");
    }

    #[test]
    fn build_action_requests_expands_marked_targets() {
        let mut app = App::new(AppConfig::default());
        app.status_entries = vec![
            crate::domain::StatusEntry {
                path: PathBuf::from(".a"),
                actual_vs_state: ChangeKind::Modified,
                actual_vs_target: ChangeKind::Modified,
            },
            crate::domain::StatusEntry {
                path: PathBuf::from(".b"),
                actual_vs_state: ChangeKind::Modified,
                actual_vs_target: ChangeKind::Modified,
            },
        ];
        app.switch_view(crate::domain::ListView::Status);
        app.toggle_selected_mark();
        app.select_next();
        app.toggle_selected_mark();

        let requests = build_action_requests(&app, Action::Forget);
        assert_eq!(requests.len(), 2);
        assert!(
            requests
                .iter()
                .all(|req| req.target.as_ref().is_some_and(|p| p.is_absolute()))
        );
    }

    #[test]
    fn validate_action_requests_rejects_directory_add() {
        let app = App::new(AppConfig::default());
        let dir = std::env::temp_dir().join(format!("chezmoi_tui_add_dir_{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("create dir");
        let requests = vec![ActionRequest {
            action: Action::Add,
            target: Some(dir.clone()),
            chattr_attrs: None,
        }];
        let message = validate_action_requests(&app, Action::Add, &requests);
        assert!(message.is_some());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn build_action_requests_creates_targeted_readd_requests_for_marked_entries() {
        let temp_root = std::env::temp_dir().join(format!(
            "chezmoi_tui_readd_requests_{}_{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("time")
                .as_nanos()
        ));
        std::fs::create_dir_all(&temp_root).expect("create temp root");
        std::fs::write(temp_root.join(".a"), "a").expect("write a");
        std::fs::write(temp_root.join(".b"), "b").expect("write b");

        let mut app = App::new(AppConfig::default());
        app.home_dir = temp_root.clone();
        app.status_entries = vec![
            crate::domain::StatusEntry {
                path: PathBuf::from(".a"),
                actual_vs_state: ChangeKind::None,
                actual_vs_target: ChangeKind::Modified,
            },
            crate::domain::StatusEntry {
                path: PathBuf::from(".b"),
                actual_vs_state: ChangeKind::None,
                actual_vs_target: ChangeKind::Modified,
            },
        ];
        app.switch_view(crate::domain::ListView::Status);
        app.toggle_selected_mark();
        app.select_next();
        app.toggle_selected_mark();

        let requests = build_action_requests(&app, Action::ReAdd);
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[0].target.as_ref(), Some(&temp_root.join(".a")));
        assert_eq!(requests[1].target.as_ref(), Some(&temp_root.join(".b")));

        let _ = std::fs::remove_dir_all(temp_root);
    }

    #[test]
    fn build_action_requests_creates_targeted_apply_requests_for_marked_status_entries() {
        let temp_root = std::env::temp_dir().join(format!(
            "chezmoi_tui_apply_requests_{}_{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("time")
                .as_nanos()
        ));
        std::fs::create_dir_all(&temp_root).expect("create temp root");
        std::fs::write(temp_root.join(".a"), "a").expect("write a");
        std::fs::write(temp_root.join(".b"), "b").expect("write b");

        let mut app = App::new(AppConfig::default());
        app.home_dir = temp_root.clone();
        app.status_entries = vec![
            crate::domain::StatusEntry {
                path: PathBuf::from(".a"),
                actual_vs_state: ChangeKind::None,
                actual_vs_target: ChangeKind::Modified,
            },
            crate::domain::StatusEntry {
                path: PathBuf::from(".b"),
                actual_vs_state: ChangeKind::None,
                actual_vs_target: ChangeKind::Added,
            },
        ];
        app.switch_view(crate::domain::ListView::Status);
        app.toggle_selected_mark();
        app.select_next();
        app.toggle_selected_mark();

        let requests = build_action_requests(&app, Action::Apply);
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[0].target.as_ref(), Some(&temp_root.join(".a")));
        assert_eq!(requests[1].target.as_ref(), Some(&temp_root.join(".b")));

        let _ = std::fs::remove_dir_all(temp_root);
    }

    #[test]
    fn build_action_requests_keeps_global_apply_outside_target_views() {
        let mut app = App::new(AppConfig::default());
        app.source_entries = vec![PathBuf::from("dot_zshrc")];
        app.switch_view(crate::domain::ListView::Source);

        let requests = build_action_requests(&app, Action::Apply);
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].target, None);
    }

    #[test]
    fn build_action_requests_keeps_global_apply_without_marks_in_status_view() {
        let mut app = App::new(AppConfig::default());
        app.status_entries = vec![crate::domain::StatusEntry {
            path: PathBuf::from(".zshrc"),
            actual_vs_state: ChangeKind::None,
            actual_vs_target: ChangeKind::Modified,
        }];
        app.switch_view(crate::domain::ListView::Status);

        let requests = build_action_requests(&app, Action::Apply);
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].target, None);
    }

    #[test]
    fn validate_action_requests_rejects_readd_for_non_modified_status_entries() {
        let temp_root = std::env::temp_dir().join(format!(
            "chezmoi_tui_readd_invalid_status_{}_{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("time")
                .as_nanos()
        ));
        std::fs::create_dir_all(&temp_root).expect("create temp root");
        std::fs::write(temp_root.join(".gitconfig"), "[user]").expect("write file");

        let mut app = App::new(AppConfig::default());
        app.home_dir = temp_root.clone();
        app.status_entries = vec![crate::domain::StatusEntry {
            path: PathBuf::from(".gitconfig"),
            actual_vs_state: ChangeKind::None,
            actual_vs_target: ChangeKind::Added,
        }];
        app.switch_view(crate::domain::ListView::Status);

        let requests = build_action_requests(&app, Action::ReAdd);
        let message = validate_action_requests(&app, Action::ReAdd, &requests);

        assert_eq!(
            message.as_deref(),
            Some("re-add is available only for modified files in status view")
        );

        let _ = std::fs::remove_dir_all(temp_root);
    }

    #[test]
    fn validate_action_requests_rejects_readd_for_directory_targets() {
        let temp_root = std::env::temp_dir().join(format!(
            "chezmoi_tui_readd_invalid_dir_{}_{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("time")
                .as_nanos()
        ));
        let managed_dir = temp_root.join(".config");
        std::fs::create_dir_all(&managed_dir).expect("create dir");

        let mut app = App::new(AppConfig::default());
        app.home_dir = temp_root.clone();
        app.status_entries = vec![crate::domain::StatusEntry {
            path: PathBuf::from(".config"),
            actual_vs_state: ChangeKind::None,
            actual_vs_target: ChangeKind::Modified,
        }];
        app.switch_view(crate::domain::ListView::Status);

        let requests = build_action_requests(&app, Action::ReAdd);
        let message = validate_action_requests(&app, Action::ReAdd, &requests);

        assert_eq!(
            message.as_deref(),
            Some("re-add is available only for files")
        );

        let _ = std::fs::remove_dir_all(temp_root);
    }

    #[test]
    fn ignore_action_error_is_logged_without_returning_error() {
        let mut app = App::new(AppConfig::default());
        let (task_tx, mut task_rx) = mpsc::unbounded_channel::<BackendTask>();
        let missing_target = std::env::temp_dir().join(format!(
            "chezmoi_tui_missing_ignore_{}_{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("time")
                .as_nanos()
        ));
        let request = ActionRequest {
            action: Action::Ignore,
            target: Some(missing_target),
            chattr_attrs: None,
        };

        let result = execute_action_request(&mut app, &task_tx, request);
        assert!(result.is_ok());
        assert!(task_rx.try_recv().is_err());
        assert!(
            app.logs
                .iter()
                .any(|line| line.contains("ignore action error")),
            "expected ignore error log, got: {:?}",
            app.logs
        );
    }
}
