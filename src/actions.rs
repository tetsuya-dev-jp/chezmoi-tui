use crate::app::{App, BackendTask, InputKind};
use crate::domain::{Action, ActionRequest};
use crate::ignore::{chezmoi_ignore_path, run_internal_ignore_action};
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
    app.busy = true;
    task_tx
        .send(task)
        .map_err(|err| anyhow::anyhow!("failed to dispatch task: {err}"))
}

pub(crate) fn run_foreground_action(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut App,
    task_tx: &UnboundedSender<BackendTask>,
    request: ActionRequest,
) -> Result<()> {
    restore_terminal(terminal)?;

    let result = run_action_foreground(&request);

    setup_terminal()?;
    terminal.clear()?;

    app.busy = false;

    match result {
        Ok((code, elapsed)) => {
            let target = request
                .target
                .as_ref()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| "(none)".to_string());
            app.log(format!(
                "foreground action done: {} {} exit={} duration={}ms",
                request.action.label(),
                target,
                code,
                elapsed
            ));

            if app.batch_in_progress() {
                maybe_continue_batch(app, task_tx)?;
            } else if code == 0 {
                send_task(app, task_tx, BackendTask::RefreshAll)?;
            }
        }
        Err(err) => {
            app.log(format!("foreground action error: {err:#}"));
            if app.batch_in_progress() {
                maybe_continue_batch(app, task_tx)?;
            }
        }
    }

    Ok(())
}

fn run_action_foreground(request: &ActionRequest) -> Result<(i32, u64)> {
    match request.action {
        Action::EditIgnore => run_edit_ignore_foreground(),
        _ => run_chezmoi_foreground(request),
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
    if request.action.is_dangerous() {
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
    ) {
        app.pending_foreground = Some(request);
        app.busy = true;
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

    let action = app.batch_action().map(|a| a.label()).unwrap_or("unknown");
    let total = app.batch_total();
    app.log(format!("batch completed: action={action} total={total}"));
    app.clear_batch();
    send_task(app, task_tx, BackendTask::RefreshAll)
}

pub(crate) fn build_action_requests(app: &App, action: Action) -> Vec<ActionRequest> {
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

    if action == Action::Edit
        && targets
            .iter()
            .any(|path| !app.is_absolute_path_managed(path))
    {
        return Some("edit is available only for managed files".to_string());
    }

    None
}

fn run_chezmoi_foreground(request: &ActionRequest) -> Result<(i32, u64)> {
    let args = action_to_args(request)?;
    let destination_dir = infer_destination_for_target(request.target.as_deref());
    let started = Instant::now();
    let status = Command::new("chezmoi")
        .arg("--destination")
        .arg(destination_dir)
        .args(args)
        .status()
        .context("failed to start foreground chezmoi command")?;
    let elapsed = started.elapsed().as_millis() as u64;

    Ok((status.code().unwrap_or(-1), elapsed))
}

fn run_edit_ignore_foreground() -> Result<(i32, u64)> {
    let ignore_path = chezmoi_ignore_path()?;
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
    let elapsed = started.elapsed().as_millis() as u64;

    Ok((status.code().unwrap_or(-1), elapsed))
}

pub(crate) fn infer_destination_for_target(target: Option<&Path>) -> std::path::PathBuf {
    let working_dir = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let home_dir = dirs::home_dir().unwrap_or_else(|| working_dir.clone());

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
