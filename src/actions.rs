use crate::app::{App, BackendTask, InputKind, NoticeTone};
use crate::config::AppConfig;
use crate::domain::{Action, ActionRequest, ListView};
use crate::ignore::{chezmoi_ignore_path_with_source, run_internal_ignore_action};
use crate::infra::{OwnedChild, action_to_args};
use crate::terminal::{restore_terminal, setup_terminal};
use anyhow::{Context, Result};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use std::fs::{self, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Instant;
use tokio::sync::mpsc::Sender;

pub fn send_task(app: &mut App, task_tx: &Sender<BackendTask>, task: BackendTask) -> Result<()> {
    let label = backend_task_label(&task);
    let mutation = task_is_mutation(&task);

    match task_tx.try_send(task) {
        Ok(()) => {
            if mutation {
                app.begin_mutation_task_with_message(label);
            } else {
                app.begin_read_task_with_message(label);
            }
            Ok(())
        }
        Err(tokio::sync::mpsc::error::TrySendError::Full(task)) => {
            let dropped = backend_task_label(&task);
            app.set_error_notice(format!("backend queue is full; dropped {dropped}"));
            app.log(format!("backend queue full; dropped task={dropped}"));
            Ok(())
        }
        Err(tokio::sync::mpsc::error::TrySendError::Closed(err)) => {
            anyhow::bail!("backend task channel closed while dispatching {err:?}");
        }
    }
}

fn task_is_mutation(task: &BackendTask) -> bool {
    match task {
        BackendTask::RefreshAll
        | BackendTask::LoadDiff { .. }
        | BackendTask::LoadPreview { .. }
        | BackendTask::PrepareApplyPlan { .. } => false,
        BackendTask::ApplyReviewed { .. } => true,
        BackendTask::Inspect { .. } => false,
        BackendTask::RunAction { request } => !request.action.is_read_only(),
    }
}

fn backend_task_label(task: &BackendTask) -> String {
    match task {
        BackendTask::RefreshAll => "refresh".to_string(),
        BackendTask::LoadDiff { target, .. } => target.as_ref().map_or_else(
            || "diff all".to_string(),
            |path| format!("diff {}", path.display()),
        ),
        BackendTask::LoadPreview { target, .. } => format!("preview {}", target.display()),
        BackendTask::PrepareApplyPlan { .. } => "prepare apply plan".to_string(),
        BackendTask::ApplyReviewed { .. } => "validate and apply".to_string(),
        BackendTask::Inspect { request, .. } => format!(
            "{} {}",
            request.action.label(),
            request
                .targets
                .first()
                .map_or_else(|| "(all)".to_string(), |path| path.display().to_string())
        ),
        BackendTask::RunAction { request } => format!(
            "{} {}",
            request.action.label(),
            request
                .targets
                .first()
                .map_or_else(|| "(none)".to_string(), |path| path.display().to_string())
        ),
    }
}

pub fn run_foreground_action(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut App,
    task_tx: &Sender<BackendTask>,
    request: &ActionRequest,
) -> Result<()> {
    restore_terminal(terminal)?;

    let result = run_action_foreground(request, &app.config, app.effective_source_dir());

    setup_terminal()?;
    terminal.clear()?;

    if request.action.is_read_only() {
        app.finish_read_task();
    } else {
        app.finish_mutation_task();
    }

    match result {
        Ok((code, elapsed)) => {
            let target = request.targets.first().map_or_else(
                || "(none)".to_string(),
                |path| {
                    if request.targets.len() == 1 {
                        path.display().to_string()
                    } else {
                        format!("{} (+{} more)", path.display(), request.targets.len() - 1)
                    }
                },
            );
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
                if code == 0 {
                    maybe_continue_batch(app, task_tx)?;
                } else {
                    stop_batch_and_refresh(app, task_tx)?;
                }
            } else if !request.action.is_read_only() {
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
                stop_batch_and_refresh(app, task_tx)?;
            } else if !request.action.is_read_only() {
                send_task(app, task_tx, BackendTask::RefreshAll)?;
            }
        }
    }

    Ok(())
}

fn run_action_foreground(
    request: &ActionRequest,
    config: &AppConfig,
    effective_source_dir: Option<&Path>,
) -> Result<(i32, u64)> {
    match request.action {
        Action::EditIgnore => run_edit_ignore_foreground(config, effective_source_dir),
        Action::EditSource => run_edit_source_foreground(request, config),
        Action::OpenSourceDir => run_open_source_dir_foreground(config),
        Action::ExternalDiff => run_external_diff_foreground(config),
        _ => run_chezmoi_foreground(request, config),
    }
}

pub(crate) fn dispatch_action_request(
    app: &mut App,
    task_tx: &Sender<BackendTask>,
    request: ActionRequest,
) -> Result<()> {
    if let Some(message) =
        validate_action_requests(app, request.action, std::slice::from_ref(&request))
    {
        app.log(message.clone());
        app.set_error_notice(message);
        app.clear_batch();
        return Ok(());
    }

    if request.action == Action::Chattr && request.chattr_attrs.is_none() {
        app.open_input(InputKind::ChattrAttrs, request);
        return Ok(());
    }
    if request.action == Action::Apply && app.config.show_apply_plan {
        let request_id = app.begin_apply_plan_request();
        send_task(
            app,
            task_tx,
            BackendTask::PrepareApplyPlan {
                request_id,
                request,
                destination: app.home_dir.clone(),
            },
        )?;
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
    task_tx: &Sender<BackendTask>,
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
        if app.effective_source_dir().is_none() {
            let message =
                "ignore action unavailable: effective source root has not resolved yet".to_string();
            app.log(message.clone());
            app.set_error_notice(message);
            if app.batch_in_progress() {
                stop_batch_and_refresh(app, task_tx)?;
            }
            return Ok(());
        }
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
                    stop_batch_and_refresh(app, task_tx)?;
                }
            }
        }
        return Ok(());
    }

    if request.action.runs_foreground() {
        let target = request
            .targets
            .first()
            .map_or_else(|| "(none)".to_string(), |path| path.display().to_string());
        let label = format!(
            "{} {}{}",
            request.action.label(),
            target,
            if request.targets.len() > 1 {
                format!(" (+{} more)", request.targets.len() - 1)
            } else {
                String::new()
            }
        );
        app.pending_foreground = Some(request);
        if app
            .pending_foreground
            .as_ref()
            .is_some_and(|request| request.action.is_read_only())
        {
            app.begin_read_task_with_message(label);
        } else {
            app.begin_mutation_task_with_message(label);
        }
    } else if request.action.is_read_only() && request.action.shows_output() {
        let request_id = app.begin_detail_request();
        send_task(
            app,
            task_tx,
            BackendTask::Inspect {
                request_id,
                request,
            },
        )?;
    } else {
        send_task(app, task_tx, BackendTask::RunAction { request })?;
    }
    Ok(())
}

pub(crate) fn maybe_continue_batch(app: &mut App, task_tx: &Sender<BackendTask>) -> Result<()> {
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
    let read_only = app
        .batch_action()
        .is_some_and(|action| action.is_read_only());
    app.clear_batch();
    if read_only {
        Ok(())
    } else {
        send_task(app, task_tx, BackendTask::RefreshAll)
    }
}

pub(crate) fn stop_batch_and_refresh(app: &mut App, task_tx: &Sender<BackendTask>) -> Result<()> {
    let action = app
        .batch_action()
        .map_or("unknown", super::domain::Action::label);
    let total = app.batch_total();
    let read_only = app
        .batch_action()
        .is_some_and(|action| action.is_read_only());
    app.clear_batch();
    app.set_error_notice(format!(
        "batch stopped after failure: action={action} total={total}"
    ));
    if read_only {
        Ok(())
    } else {
        send_task(app, task_tx, BackendTask::RefreshAll)
    }
}

pub(crate) fn build_action_requests(app: &App, action: Action) -> Vec<ActionRequest> {
    if action == Action::Apply
        && app.marked_count() > 0
        && matches!(app.view, ListView::Status | ListView::Managed)
    {
        let targets = app.selected_action_targets_absolute();
        if !targets.is_empty() {
            return vec![ActionRequest {
                action,
                targets,
                chattr_attrs: None,
            }];
        }
    }

    if action == Action::Verify
        && app.marked_count() > 0
        && matches!(app.view, ListView::Status | ListView::Managed)
    {
        let targets = app.selected_action_targets_absolute();
        if !targets.is_empty() {
            return vec![ActionRequest {
                action,
                targets,
                chattr_attrs: None,
            }];
        }
    }

    if action == Action::GitStage && app.view == ListView::Source {
        let targets = app.selected_action_targets_absolute();
        if !targets.is_empty() {
            return vec![ActionRequest {
                action,
                targets,
                chattr_attrs: None,
            }];
        }
    }

    if !action.needs_target() {
        return vec![ActionRequest {
            action,
            targets: Vec::new(),
            chattr_attrs: None,
        }];
    }

    app.selected_action_targets_absolute()
        .into_iter()
        .map(|target| ActionRequest {
            action,
            targets: vec![target],
            chattr_attrs: None,
        })
        .collect()
}

pub(crate) fn validate_action_requests(
    app: &App,
    action: Action,
    requests: &[ActionRequest],
) -> Option<String> {
    use crate::action_policy::{TargetPolicy, policy_for};

    let policy = policy_for(action);

    if matches!(policy.target, TargetPolicy::ModifiedStatusFile) {
        if requests.is_empty() {
            return Some(format!("{} requires a target file", action.label()));
        }
        let targets: Vec<&Path> = requests
            .iter()
            .flat_map(|req| req.targets.iter().map(PathBuf::as_path))
            .collect();
        if targets.is_empty() {
            return Some(format!("{} requires a target file", action.label()));
        }
        if targets.iter().any(|path| path.is_dir()) {
            return Some("re-add is available only for files".to_string());
        }
        if !app.readd_selection_is_eligible() {
            return Some("re-add accepts only modified files in status view".to_string());
        }
        return None;
    }

    if requests.is_empty() {
        return Some(format!("{} requires a target file", action.label()));
    }

    let targets: Vec<&Path> = requests
        .iter()
        .flat_map(|req| req.targets.iter().map(PathBuf::as_path))
        .collect();

    if action == Action::Add
        && targets
            .iter()
            .any(|target| !target.starts_with(&app.home_dir))
    {
        return Some(
            "outside destination; start with --destination ... to manage this tree".to_string(),
        );
    }
    let ignore_has_outside_target = action == Action::Ignore
        && requests
            .iter()
            .flat_map(|request| &request.targets)
            .any(|target| !target.starts_with(&app.home_dir));
    let selected_ignore_mode = requests
        .iter()
        .find_map(|request| request.chattr_attrs.as_deref());
    if ignore_has_outside_target && selected_ignore_mode.is_some_and(|mode| mode != "global-name") {
        return Some(
            "ignore targets outside the destination support only the global-by-name rule"
                .to_string(),
        );
    }

    match policy.target {
        TargetPolicy::None | TargetPolicy::Optional => {}
        TargetPolicy::Required => {
            if targets.is_empty() {
                return Some(format!("{} requires a target file", action.label()));
            }
        }
        TargetPolicy::ExactManaged => {
            if targets.is_empty()
                || targets
                    .iter()
                    .any(|path| !app.is_absolute_path_managed(path))
            {
                return Some(format!(
                    "{} is available only for exact managed entries",
                    action.label()
                ));
            }
        }
        TargetPolicy::ExistingNonDirectory => {
            if targets.is_empty() || targets.iter().any(|path| path.is_dir()) {
                return Some(
                    "Adding a whole directory is disabled. Expand it and select only required files."
                        .to_string(),
                );
            }
        }
        TargetPolicy::ExistingDirectory => {
            if targets.is_empty() || targets.iter().any(|path| !path.is_dir()) {
                return Some(format!("{} requires a directory", action.label()));
            }
            if action == Action::AddDirectory
                && targets.iter().any(|path| !path.starts_with(&app.home_dir))
            {
                return Some(
                    "outside destination; start with --destination ... to manage this tree"
                        .to_string(),
                );
            }
        }
        TargetPolicy::ExistingSourceFile => {
            let source_root = app.effective_source_dir();
            if app.view != crate::domain::ListView::Source {
                return Some(format!(
                    "{} is available only in Source view",
                    action.label()
                ));
            }
            if targets.is_empty()
                || source_root.is_none()
                || targets.iter().any(|path| {
                    !source_root.is_some_and(|root| path.starts_with(root))
                        || std::fs::metadata(path).is_err()
                        || std::fs::metadata(path).is_ok_and(|metadata| metadata.is_dir())
                })
            {
                return Some(format!(
                    "{} requires a regular source file or symlink-to-file under the effective source root",
                    action.label()
                ));
            }
        }
        TargetPolicy::IgnoreEligible => {
            if targets.is_empty() {
                return Some(format!("{} requires a target file", action.label()));
            }
        }
        TargetPolicy::ModifiedStatusFile => {
            // handled above
        }
    }

    None
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ToolInvocation {
    program: std::ffi::OsString,
    args: Vec<std::ffi::OsString>,
}

fn parse_tool_invocation(raw: &str, field_name: &str) -> Result<ToolInvocation> {
    let parts = shlex::split(raw)
        .with_context(|| format!("{field_name} contains invalid shell quoting"))?;

    if parts.is_empty() {
        anyhow::bail!("{field_name} must not be empty");
    }

    Ok(ToolInvocation {
        program: parts[0].clone().into(),
        args: parts[1..].iter().map(Into::into).collect(),
    })
}

fn resolve_editor_invocation_from(
    configured: Option<&str>,
    visual: Option<&str>,
    editor: Option<&str>,
) -> Result<ToolInvocation> {
    if let Some(cfg) = configured {
        return parse_tool_invocation(cfg, "tools.editor");
    }

    if let Some(v) = visual
        && !v.trim().is_empty()
    {
        return parse_tool_invocation(v, "VISUAL");
    }

    if let Some(e) = editor
        && !e.trim().is_empty()
    {
        return parse_tool_invocation(e, "EDITOR");
    }

    parse_tool_invocation("vi", "fallback editor")
}

fn resolve_editor_invocation(config: &AppConfig) -> Result<ToolInvocation> {
    resolve_editor_invocation_from(
        config.editor.as_deref(),
        std::env::var("VISUAL").ok().as_deref(),
        std::env::var("EDITOR").ok().as_deref(),
    )
}

fn run_chezmoi_foreground(request: &ActionRequest, config: &AppConfig) -> Result<(i32, u64)> {
    let args = action_to_args(request)?;
    let started = Instant::now();
    let mut command = configured_chezmoi_command(config);

    if matches!(
        request.action,
        Action::Edit | Action::EditConfig | Action::EditConfigTemplate
    ) && let Some(editor) = config.editor.as_deref()
    {
        command.env("EDITOR", editor);
        command.env("VISUAL", editor);
    }

    let status = command
        .args(args)
        .status()
        .context("failed to start foreground chezmoi command")?;
    let elapsed = elapsed_millis_u64(started);

    Ok((status.code().unwrap_or(-1), elapsed))
}

fn run_open_source_dir_foreground(config: &AppConfig) -> Result<(i32, u64)> {
    let started = Instant::now();
    let status = configured_chezmoi_command(config)
        .arg("cd")
        .status()
        .context("failed to launch chezmoi cd")?;
    let elapsed = elapsed_millis_u64(started);
    Ok((status.code().unwrap_or(-1), elapsed))
}

fn run_edit_source_foreground(request: &ActionRequest, config: &AppConfig) -> Result<(i32, u64)> {
    let target = request
        .targets
        .first()
        .map(PathBuf::as_path)
        .context("edit-source requires a source file")?;
    let editor = resolve_editor_invocation(config)?;
    let started = Instant::now();
    let status = Command::new(&editor.program)
        .args(&editor.args)
        .arg(target)
        .status()
        .with_context(|| {
            format!(
                "failed to launch editor {} for {}",
                editor.program.to_string_lossy(),
                target.display()
            )
        })?;
    let elapsed = elapsed_millis_u64(started);
    Ok((status.code().unwrap_or(-1), elapsed))
}

fn run_external_diff_foreground(config: &AppConfig) -> Result<(i32, u64)> {
    let tool_raw = config
        .external_diff
        .clone()
        .or_else(|| std::env::var("CHEZMOI_TUI_EXTERNAL_DIFF").ok());
    let Some(tool_raw) = tool_raw else {
        return run_native_diff_foreground(config);
    };

    let tool = parse_tool_invocation(&tool_raw, "tools.external_diff")?;

    let started = Instant::now();

    let mut diff_command = configured_chezmoi_command(config);
    diff_command
        .arg("diff")
        .arg("--no-pager")
        .arg("--use-builtin-diff")
        .arg("--color=true")
        .stdout(Stdio::piped());

    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        diff_command.process_group(0);
    }

    let mut diff_child = OwnedChild::new(
        diff_command
            .spawn()
            .context("failed to start chezmoi diff command")?,
    );

    let diff_stdout = diff_child
        .take_stdout()
        .context("failed to capture chezmoi diff stdout")?;

    let tool_status = match Command::new(&tool.program)
        .args(&tool.args)
        .stdin(Stdio::from(diff_stdout))
        .status()
    {
        Ok(status) => status,
        Err(error) => {
            diff_child
                .kill_and_wait()
                .context("failed to stop chezmoi diff after external tool launch failure")?;
            return Err(error).with_context(|| {
                format!(
                    "failed to launch external diff tool {}",
                    tool.program.to_string_lossy()
                )
            });
        }
    };

    if !tool_status.success() {
        diff_child
            .kill_and_wait()
            .context("failed to stop chezmoi diff after external tool failure")?;
        return Ok((
            tool_status.code().unwrap_or(-1),
            elapsed_millis_u64(started),
        ));
    }

    let diff_status = diff_child
        .wait()
        .context("failed to wait for chezmoi diff command")?;

    let elapsed = elapsed_millis_u64(started);

    let code = diff_status.code().unwrap_or(-1);

    Ok((code, elapsed))
}

fn run_native_diff_foreground(config: &AppConfig) -> Result<(i32, u64)> {
    let started = Instant::now();
    let status = configured_chezmoi_command(config)
        .arg("diff")
        .status()
        .context("failed to launch native chezmoi diff")?;
    let elapsed = elapsed_millis_u64(started);
    Ok((status.code().unwrap_or(-1), elapsed))
}

fn resolve_source_dir_for_foreground(
    config: &AppConfig,
    effective_source_dir: Option<&Path>,
) -> Result<std::path::PathBuf> {
    if let Some(source_dir) = effective_source_dir {
        return Ok(source_dir.to_path_buf());
    }

    let mut command = Command::new(&config.chezmoi_binary);
    if let Some(config_file) = &config.chezmoi_config {
        command.arg("--config").arg(config_file);
    }
    if let Some(source_dir) = &config.source_dir {
        command.arg("--source").arg(source_dir);
    }
    let output = command
        .arg("source-path")
        .output()
        .context("failed to execute chezmoi source-path")?;
    if !output.status.success() {
        anyhow::bail!(
            "chezmoi source-path failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    parse_path_record_output(&output.stdout, "chezmoi source-path")
}

fn parse_path_record_output(output: &[u8], command: &str) -> Result<PathBuf> {
    let output = std::str::from_utf8(output)
        .with_context(|| format!("{command} returned a path that is not valid UTF-8"))?;
    let output = output.strip_suffix('\n').unwrap_or(output);
    if output.is_empty() || output.contains('\r') || output.contains('\n') {
        anyhow::bail!("{command} returned an invalid path record");
    }
    Ok(PathBuf::from(output))
}

fn configured_chezmoi_command(config: &AppConfig) -> Command {
    let destination_dir = config
        .destination_dir
        .clone()
        .or_else(dirs::home_dir)
        .unwrap_or_else(|| config.working_dir.clone());
    let mut command = Command::new(&config.chezmoi_binary);
    if let Some(config_file) = &config.chezmoi_config {
        command.arg("--config").arg(config_file);
    }
    command.arg("--destination").arg(destination_dir);
    if let Some(source_dir) = &config.source_dir {
        command.arg("--source").arg(source_dir);
    }
    command
}

fn run_edit_ignore_foreground(
    config: &AppConfig,
    effective_source_dir: Option<&Path>,
) -> Result<(i32, u64)> {
    let source_dir = resolve_source_dir_for_foreground(config, effective_source_dir)?;
    let ignore_path = chezmoi_ignore_path_with_source(Some(&source_dir))?;
    if let Some(parent) = ignore_path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    OpenOptions::new()
        .create(true)
        .append(true)
        .open(&ignore_path)
        .with_context(|| format!("failed to open {}", ignore_path.display()))?;

    let editor = resolve_editor_invocation(config)?;

    let started = Instant::now();
    let status = Command::new(&editor.program)
        .args(&editor.args)
        .arg(&ignore_path)
        .status()
        .with_context(|| {
            format!(
                "failed to launch editor {} for {}",
                editor.program.to_string_lossy(),
                ignore_path.display()
            )
        })?;
    let elapsed = elapsed_millis_u64(started);

    Ok((status.code().unwrap_or(-1), elapsed))
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
    use tokio::sync::mpsc;

    #[cfg(unix)]
    static SCRIPT_FIXTURES: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[cfg(unix)]
    fn script_guard() -> std::sync::MutexGuard<'static, ()> {
        SCRIPT_FIXTURES
            .lock()
            .unwrap_or_else(|error| error.into_inner())
    }

    #[cfg(unix)]
    fn write_script(path: &Path, source: &str) {
        use std::os::unix::fs::PermissionsExt;

        std::fs::write(path, source).expect("write script");
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))
            .expect("make script executable");
    }

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
                .flat_map(|req| &req.targets)
                .all(|path| path.is_absolute())
        );
    }

    #[test]
    fn validate_action_requests_rejects_directory_add() {
        let app = App::new(AppConfig::default());
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let dir = temp_dir.path().to_path_buf();
        std::fs::create_dir_all(&dir).expect("create dir");
        let requests = vec![ActionRequest {
            action: Action::Add,
            targets: vec![dir.clone()],
            chattr_attrs: None,
        }];
        let message = validate_action_requests(&app, Action::Add, &requests);
        assert!(message.is_some());
    }

    #[test]
    fn build_action_requests_creates_targeted_readd_requests_for_marked_entries() {
        let temp = tempfile::tempdir().expect("tempdir");
        let temp_root = temp.path().to_path_buf();
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
        assert_eq!(requests[0].targets, vec![temp_root.join(".a")]);
        assert_eq!(requests[1].targets, vec![temp_root.join(".b")]);
    }

    #[test]
    fn hidden_modified_mark_remains_targeted_and_eligible_for_readd() {
        let temp = tempfile::tempdir().expect("tempdir");
        std::fs::write(temp.path().join(".a"), "a").expect("write a");
        let mut app = App::new(AppConfig::default());
        app.home_dir = temp.path().to_path_buf();
        app.status_entries = vec![crate::domain::StatusEntry {
            path: PathBuf::from(".a"),
            actual_vs_state: ChangeKind::None,
            actual_vs_target: ChangeKind::Modified,
        }];
        app.switch_view(crate::domain::ListView::Status);
        app.toggle_selected_mark();
        app.apply_list_filter_immediately("missing".to_string());

        let requests = build_action_requests(&app, Action::ReAdd);
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].targets, vec![temp.path().join(".a")]);
        assert!(validate_action_requests(&app, Action::ReAdd, &requests).is_none());
    }

    #[test]
    fn hidden_ineligible_mark_does_not_bypass_readd_validation() {
        let temp = tempfile::tempdir().expect("tempdir");
        std::fs::write(temp.path().join(".a"), "a").expect("write a");
        std::fs::write(temp.path().join(".b"), "b").expect("write b");
        let mut app = App::new(AppConfig::default());
        app.home_dir = temp.path().to_path_buf();
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
        app.apply_list_filter_immediately("missing".to_string());

        let requests = build_action_requests(&app, Action::ReAdd);
        assert_eq!(requests.len(), 2);
        assert!(
            !app.readd_selection_is_eligible(),
            "hidden Added entry must make the complete marked selection ineligible"
        );
        let message = validate_action_requests(&app, Action::ReAdd, &requests);
        assert!(
            message
                .as_deref()
                .is_some_and(|text| text.contains("only modified files")),
            "expected a modified-files validation error, got {message:?}"
        );
    }

    #[test]
    fn build_action_requests_creates_targeted_apply_requests_for_marked_status_entries() {
        let temp = tempfile::tempdir().expect("tempdir");
        let temp_root = temp.path().to_path_buf();
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
        assert_eq!(requests.len(), 1);
        assert_eq!(
            requests[0].targets,
            vec![temp_root.join(".a"), temp_root.join(".b")]
        );
    }

    #[test]
    fn build_action_requests_keeps_global_apply_outside_target_views() {
        let mut app = App::new(AppConfig::default());
        app.source_entries = vec![PathBuf::from("dot_zshrc")];
        app.switch_view(crate::domain::ListView::Source);

        let requests = build_action_requests(&app, Action::Apply);
        assert_eq!(requests.len(), 1);
        assert!(requests[0].targets.is_empty());
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
        assert!(requests[0].targets.is_empty());
    }

    #[test]
    fn validate_action_requests_rejects_readd_for_non_modified_status_entries() {
        let temp = tempfile::tempdir().expect("tempdir");
        let temp_root = temp.path().to_path_buf();
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
            Some("re-add accepts only modified files in status view")
        );
    }

    #[test]
    fn validate_action_requests_rejects_readd_for_directory_targets() {
        let temp = tempfile::tempdir().expect("tempdir");
        let temp_root = temp.path().to_path_buf();
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
    }

    #[test]
    fn ignore_action_error_is_logged_without_returning_error() {
        let mut app = App::new(AppConfig::default());
        let (task_tx, mut task_rx) = mpsc::channel::<BackendTask>(8);
        let temp_mt = tempfile::tempdir().expect("tempdir");
        let missing_target = temp_mt.path().join("missing_file");
        let request = ActionRequest {
            action: Action::Ignore,
            targets: vec![missing_target],
            chattr_attrs: None,
        };

        let result = execute_action_request(&mut app, &task_tx, request);
        assert!(result.is_ok());
        assert!(task_rx.try_recv().is_err());
        assert!(
            app.logs.iter().any(|line| line.contains("ignore action")),
            "expected ignore error log, got: {:?}",
            app.logs
        );
    }

    #[test]
    fn parse_tool_invocation_splits_program_and_args() {
        let tool = parse_tool_invocation("delta --paging=always", "tools.external_diff")
            .expect("parse tool");
        assert_eq!(tool.program, std::ffi::OsString::from("delta"));
        assert_eq!(tool.args, vec![std::ffi::OsString::from("--paging=always")]);
    }

    #[test]
    fn parse_tool_invocation_rejects_empty_value() {
        let err = parse_tool_invocation("   ", "tools.external_diff")
            .expect_err("empty tool should fail");
        assert!(err.to_string().contains("tools.external_diff"));
    }

    #[test]
    fn parse_tool_invocation_does_not_interpret_shell_metacharacters() {
        let tool = parse_tool_invocation("delta; rm -rf /tmp/x", "tools.external_diff")
            .expect("parse tool");
        assert_eq!(tool.program, std::ffi::OsString::from("delta;"));
        assert_eq!(
            tool.args,
            vec![
                std::ffi::OsString::from("rm"),
                std::ffi::OsString::from("-rf"),
                std::ffi::OsString::from("/tmp/x"),
            ]
        );
    }

    #[test]
    fn parse_tool_invocation_respects_quoted_words() {
        let tool = parse_tool_invocation(r#""editor tool" --wait "two words""#, "tools.editor")
            .expect("parse quoted tool");
        assert_eq!(tool.program, std::ffi::OsString::from("editor tool"));
        assert_eq!(
            tool.args,
            vec![
                std::ffi::OsString::from("--wait"),
                std::ffi::OsString::from("two words"),
            ]
        );
    }

    #[test]
    fn parse_tool_invocation_rejects_unterminated_quotes() {
        let error = parse_tool_invocation(r#"nvim "unterminated"#, "tools.editor")
            .expect_err("invalid quoting must fail");
        assert!(error.to_string().contains("invalid shell quoting"));
    }

    #[test]
    fn path_record_rejects_cr_but_preserves_trailing_space() {
        assert!(parse_path_record_output(b"/tmp/path\r\n", "source-path").is_err());
        assert_eq!(
            parse_path_record_output(b"/tmp/path \n", "source-path").expect("trailing space"),
            PathBuf::from("/tmp/path ")
        );
    }

    #[cfg(unix)]
    #[test]
    fn native_diff_default_delegates_directly_to_chezmoi() {
        let _guard = script_guard();
        let temp = tempfile::tempdir().expect("tempdir");
        let chezmoi = temp.path().join("chezmoi");
        let log = temp.path().join("args");
        write_script(
            &chezmoi,
            &format!("#!/bin/sh\nprintf '%s\\n' \"$@\" > '{}'\n", log.display()),
        );
        let config = AppConfig {
            chezmoi_binary: chezmoi.display().to_string(),
            destination_dir: Some(temp.path().to_path_buf()),
            working_dir: temp.path().to_path_buf(),
            ..AppConfig::default()
        };

        let (code, _) = run_native_diff_foreground(&config).expect("native diff");

        assert_eq!(code, 0);
        assert_eq!(
            std::fs::read_to_string(log).expect("args").lines().last(),
            Some("diff")
        );
    }

    #[cfg(unix)]
    #[test]
    fn failed_external_pager_kills_slow_owned_diff_before_returning() {
        let _guard = script_guard();
        let temp = tempfile::tempdir().expect("tempdir");
        let chezmoi = temp.path().join("chezmoi");
        let pager = temp.path().join("pager");
        let pid_file = temp.path().join("diff.pid");
        write_script(
            &chezmoi,
            &format!(
                "#!/bin/sh\nprintf '%s' \"$$\" > '{}'\nexec sleep 30\n",
                pid_file.display()
            ),
        );
        write_script(
            &pager,
            &format!(
                "#!/bin/sh\nwhile [ ! -f '{}' ]; do sleep 0.01; done\nexec /bin/false\n",
                pid_file.display()
            ),
        );
        let config = AppConfig {
            chezmoi_binary: chezmoi.display().to_string(),
            external_diff: Some(pager.display().to_string()),
            destination_dir: Some(temp.path().to_path_buf()),
            working_dir: temp.path().to_path_buf(),
            ..AppConfig::default()
        };

        let started = Instant::now();
        let (code, _) = run_external_diff_foreground(&config).expect("pager exit result");

        assert_eq!(code, 1);
        assert!(started.elapsed() < std::time::Duration::from_secs(2));
        let pid = std::fs::read_to_string(pid_file).expect("diff pid");
        let alive = Command::new("kill")
            .args(["-0", &pid])
            .output()
            .expect("check diff");
        assert!(!alive.status.success(), "failed pager left diff running");
    }

    #[cfg(unix)]
    #[test]
    fn missing_external_pager_cleans_owned_diff_before_returning() {
        let _guard = script_guard();
        let temp = tempfile::tempdir().expect("tempdir");
        let chezmoi = temp.path().join("chezmoi");
        let pid_file = temp.path().join("diff.pid");
        write_script(
            &chezmoi,
            &format!(
                "#!/bin/sh\nprintf '%s' \"$$\" > '{}'\nexec sleep 30\n",
                pid_file.display()
            ),
        );
        let config = AppConfig {
            chezmoi_binary: chezmoi.display().to_string(),
            external_diff: Some(temp.path().join("missing-pager").display().to_string()),
            destination_dir: Some(temp.path().to_path_buf()),
            working_dir: temp.path().to_path_buf(),
            ..AppConfig::default()
        };

        let started = Instant::now();
        let error = run_external_diff_foreground(&config).expect_err("missing pager");

        assert!(
            error
                .to_string()
                .contains("failed to launch external diff tool")
        );
        assert!(started.elapsed() < std::time::Duration::from_secs(2));
        if let Ok(pid) = std::fs::read_to_string(pid_file) {
            let alive = Command::new("kill")
                .args(["-0", &pid])
                .output()
                .expect("check diff");
            assert!(!alive.status.success(), "spawn failure left diff running");
        }
    }

    #[test]
    fn verify_uses_marked_status_targets() {
        let temp = tempfile::tempdir().expect("tempdir");
        let mut app = App::new(AppConfig::default());
        app.home_dir = temp.path().to_path_buf();
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

        let requests = build_action_requests(&app, Action::Verify);
        assert_eq!(requests.len(), 1);
        assert_eq!(
            requests[0].targets,
            vec![temp.path().join(".a"), temp.path().join(".b")]
        );
    }

    #[test]
    fn verify_without_marks_remains_targetless_in_source_view() {
        let mut app = App::new(AppConfig::default());
        app.source_entries = vec![PathBuf::from("dot_file")];
        app.switch_view(crate::domain::ListView::Source);

        let requests = build_action_requests(&app, Action::Verify);
        assert_eq!(requests.len(), 1);
        assert!(requests[0].targets.is_empty());
    }

    #[test]
    fn read_only_foreground_action_uses_read_busy_count() {
        let mut app = App::new(AppConfig::default());
        let (task_tx, _task_rx) = mpsc::channel(8);
        execute_action_request(
            &mut app,
            &task_tx,
            ActionRequest {
                action: Action::ExternalDiff,
                targets: Vec::new(),
                chattr_attrs: None,
            },
        )
        .expect("queue foreground read-only action");

        assert!(app.pending_foreground.is_some());
        assert_eq!(app.busy_task_count(), 1);
        assert!(!app.has_mutating_work());
        app.finish_read_task();
    }

    #[test]
    fn read_only_batch_completion_does_not_request_refresh() {
        let mut app = App::new(AppConfig::default());
        let (task_tx, mut task_rx) = mpsc::channel(8);
        let requests = vec![
            ActionRequest {
                action: Action::Doctor,
                targets: Vec::new(),
                chattr_attrs: None,
            },
            ActionRequest {
                action: Action::Doctor,
                targets: Vec::new(),
                chattr_attrs: None,
            },
        ];
        app.start_batch(requests);
        app.pop_next_batch_request();
        app.pop_next_batch_request();

        maybe_continue_batch(&mut app, &task_tx).expect("complete read-only batch");

        assert!(!app.batch_in_progress());
        assert!(task_rx.try_recv().is_err());
    }

    #[cfg(unix)]
    #[test]
    fn edit_source_launches_configured_editor_for_the_resolved_file() {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempfile::tempdir().expect("tempdir");
        let editor = temp.path().join("editor with spaces");
        let log = editor.with_extension("log");
        std::fs::write(&editor, "#!/bin/sh\nprintf '%s\\n' \"$@\" > \"$0.log\"\n")
            .expect("write editor");
        let mut permissions = std::fs::metadata(&editor)
            .expect("stat editor")
            .permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&editor, permissions).expect("chmod editor");
        let source_file = temp.path().join("source file");
        std::fs::write(&source_file, "content").expect("write source");

        let config = AppConfig {
            editor: Some(format!("\"{}\" --wait", editor.display())),
            ..AppConfig::default()
        };
        let request = ActionRequest {
            action: Action::EditSource,
            targets: vec![source_file.clone()],
            chattr_attrs: None,
        };
        let result = run_edit_source_foreground(&request, &config);

        assert_eq!(result.expect("launch editor").0, 0);
        let args = std::fs::read_to_string(log).expect("editor log");
        assert_eq!(
            args.lines().collect::<Vec<_>>(),
            vec!["--wait", source_file.to_str().expect("utf-8 path")]
        );
    }

    #[test]
    fn validate_action_requests_rejects_destroy_for_virtual_managed_directory() {
        let mut app = App::new(AppConfig::default());
        let temp_home = tempfile::tempdir().expect("tempdir");
        app.home_dir = temp_home.path().to_path_buf();
        app.managed_entries = vec![PathBuf::from(".config/nvim/init.lua")];

        let request = ActionRequest {
            action: Action::Destroy,
            targets: vec![app.home_dir.join(".config")],
            chattr_attrs: None,
        };

        let message = validate_action_requests(&app, Action::Destroy, &[request]);

        assert_eq!(
            message.as_deref(),
            Some("destroy is available only for exact managed entries")
        );
    }

    #[test]
    fn validate_action_requests_allows_destroy_for_exact_managed_file() {
        let mut app = App::new(AppConfig::default());
        let temp_home = tempfile::tempdir().expect("tempdir");
        app.home_dir = temp_home.path().to_path_buf();
        app.managed_entries = vec![PathBuf::from(".config/nvim/init.lua")];

        let request = ActionRequest {
            action: Action::Destroy,
            targets: vec![app.home_dir.join(".config/nvim/init.lua")],
            chattr_attrs: None,
        };

        let message = validate_action_requests(&app, Action::Destroy, &[request]);

        assert!(message.is_none());
    }

    #[test]
    fn validate_action_requests_rejects_forget_for_virtual_managed_directory() {
        let mut app = App::new(AppConfig::default());
        let temp_home = tempfile::tempdir().expect("tempdir");
        app.home_dir = temp_home.path().to_path_buf();
        app.managed_entries = vec![PathBuf::from(".config/nvim/init.lua")];

        let request = ActionRequest {
            action: Action::Forget,
            targets: vec![app.home_dir.join(".config/nvim")],
            chattr_attrs: None,
        };

        let message = validate_action_requests(&app, Action::Forget, &[request]);

        assert_eq!(
            message.as_deref(),
            Some("forget is available only for exact managed entries")
        );
    }

    #[test]
    fn validate_action_requests_rejects_chattr_for_virtual_managed_directory() {
        let mut app = App::new(AppConfig::default());
        let temp_home = tempfile::tempdir().expect("tempdir");
        app.home_dir = temp_home.path().to_path_buf();
        app.managed_entries = vec![PathBuf::from(".config/nvim/init.lua")];

        let request = ActionRequest {
            action: Action::Chattr,
            targets: vec![app.home_dir.join(".config/nvim")],
            chattr_attrs: Some("private".to_string()),
        };

        let message = validate_action_requests(&app, Action::Chattr, &[request]);

        assert_eq!(
            message.as_deref(),
            Some("chattr is available only for exact managed entries")
        );
    }

    #[test]
    fn validate_action_requests_allows_destroy_for_exact_managed_directory() {
        let mut app = App::new(AppConfig::default());
        let temp_home = tempfile::tempdir().expect("tempdir");
        app.home_dir = temp_home.path().to_path_buf();
        app.managed_entries = vec![PathBuf::from(".config")];

        let request = ActionRequest {
            action: Action::Destroy,
            targets: vec![app.home_dir.join(".config")],
            chattr_attrs: None,
        };

        let message = validate_action_requests(&app, Action::Destroy, &[request]);

        assert!(message.is_none());
    }

    #[test]
    fn resolve_editor_prefers_configured_editor() {
        let editor = resolve_editor_invocation_from(Some("nvim"), Some("code --wait"), Some("vim"))
            .expect("resolve editor");
        assert_eq!(editor.program, std::ffi::OsString::from("nvim"));
    }

    #[test]
    fn resolve_editor_falls_back_to_visual_then_editor_then_vi() {
        let editor = resolve_editor_invocation_from(None, Some("code --wait"), Some("vim"))
            .expect("resolve editor");
        assert_eq!(editor.program, std::ffi::OsString::from("code"));
        assert_eq!(editor.args, vec![std::ffi::OsString::from("--wait")]);

        let editor =
            resolve_editor_invocation_from(None, None, Some("emacs")).expect("resolve editor");
        assert_eq!(editor.program, std::ffi::OsString::from("emacs"));

        let editor = resolve_editor_invocation_from(None, None, None).expect("resolve editor");
        assert_eq!(editor.program, std::ffi::OsString::from("vi"));
    }

    #[test]
    fn resolve_editor_skips_empty_visual_and_editor() {
        let editor =
            resolve_editor_invocation_from(None, Some("   "), Some("vim")).expect("resolve editor");
        assert_eq!(editor.program, std::ffi::OsString::from("vim"));
    }

    #[test]
    fn required_target_policy_rejects_missing_target_but_optional_apply_allows_it() {
        let app = App::new(AppConfig::default());
        let missing = ActionRequest {
            action: Action::Merge,
            targets: Vec::new(),
            chattr_attrs: None,
        };
        assert!(
            validate_action_requests(&app, Action::Merge, &[missing])
                .is_some_and(|message| message.contains("requires a target"))
        );

        let apply = ActionRequest {
            action: Action::Apply,
            targets: Vec::new(),
            chattr_attrs: None,
        };
        assert!(validate_action_requests(&app, Action::Apply, &[apply]).is_none());
    }

    #[test]
    fn existing_directory_policy_rejects_outside_destination_mutation() {
        let destination = tempfile::tempdir().expect("destination");
        let outside = tempfile::tempdir().expect("outside");
        let mut app = App::new(AppConfig::default());
        app.home_dir = destination.path().to_path_buf();
        app.switch_view(crate::domain::ListView::Unmanaged);

        let add = ActionRequest {
            action: Action::AddDirectory,
            targets: vec![outside.path().to_path_buf()],
            chattr_attrs: None,
        };
        let message = validate_action_requests(&app, Action::AddDirectory, &[add]);
        assert_eq!(
            message.as_deref(),
            Some("outside destination; start with --destination ... to manage this tree")
        );

        let preview = ActionRequest {
            action: Action::PreviewAddDirectory,
            targets: vec![outside.path().to_path_buf()],
            chattr_attrs: None,
        };
        assert!(validate_action_requests(&app, Action::PreviewAddDirectory, &[preview]).is_none());
    }

    #[test]
    fn outside_ignore_allows_only_the_global_name_dispatch_mode() {
        let destination = tempfile::tempdir().expect("destination");
        let outside = tempfile::tempdir().expect("outside");
        let mut app = App::new(AppConfig::default());
        app.home_dir = destination.path().to_path_buf();

        let mut request = ActionRequest {
            action: Action::Ignore,
            targets: vec![outside.path().join("cache")],
            chattr_attrs: None,
        };
        assert!(validate_action_requests(&app, Action::Ignore, &[request.clone()]).is_none());

        request.chattr_attrs = Some("exact".to_string());
        assert!(
            validate_action_requests(&app, Action::Ignore, &[request.clone()])
                .is_some_and(|message| message.contains("global-by-name"))
        );

        request.chattr_attrs = Some("global-name".to_string());
        assert!(validate_action_requests(&app, Action::Ignore, &[request]).is_none());
    }

    #[test]
    fn existing_source_file_policy_checks_effective_root_and_file_type() {
        let source = tempfile::tempdir().expect("source");
        let outside = tempfile::tempdir().expect("outside");
        let file = source.path().join("private_file");
        let directory = source.path().join("directory");
        let outside_file = outside.path().join("outside-file");
        std::fs::write(&file, "file").expect("write source file");
        std::fs::create_dir_all(&directory).expect("create source directory");
        std::fs::write(&outside_file, "outside").expect("write outside file");

        let mut app = App::new(AppConfig::default());
        app.apply_source_refresh(
            Some(source.path().to_path_buf()),
            vec![PathBuf::from("private_file")],
        );
        app.switch_view(crate::domain::ListView::Source);

        for action in [Action::GitStage, Action::EditSource] {
            let request = ActionRequest {
                action,
                targets: vec![file.clone()],
                chattr_attrs: None,
            };
            assert!(validate_action_requests(&app, action, &[request]).is_none());

            let request = ActionRequest {
                action,
                targets: vec![directory.clone()],
                chattr_attrs: None,
            };
            assert!(validate_action_requests(&app, action, &[request]).is_some());

            let request = ActionRequest {
                action,
                targets: vec![outside_file.clone()],
                chattr_attrs: None,
            };
            assert!(validate_action_requests(&app, action, &[request]).is_some());
        }
    }

    #[cfg(unix)]
    #[test]
    fn existing_source_file_policy_accepts_symlink_to_file() {
        use std::os::unix::fs::symlink;

        let source = tempfile::tempdir().expect("source");
        let target = source.path().join("target");
        let link = source.path().join("link");
        std::fs::write(&target, "target").expect("write target");
        symlink(&target, &link).expect("create symlink");

        let mut app = App::new(AppConfig::default());
        app.apply_source_refresh(
            Some(source.path().to_path_buf()),
            vec![PathBuf::from("link")],
        );
        app.switch_view(crate::domain::ListView::Source);
        let request = ActionRequest {
            action: Action::EditSource,
            targets: vec![link],
            chattr_attrs: None,
        };

        assert!(validate_action_requests(&app, Action::EditSource, &[request]).is_none());
    }

    #[test]
    fn internal_ignore_uses_effective_root_and_restores_original_source() {
        let destination = tempfile::tempdir().expect("destination");
        let original = tempfile::tempdir().expect("original source");
        std::fs::write(original.path().join(".chezmoiroot"), "inner\n").expect("write chezmoiroot");
        let effective = original.path().join("inner");
        std::fs::create_dir_all(&effective).expect("create effective root");
        let target = destination.path().join("secret.txt");
        std::fs::write(&target, "secret").expect("write target");
        let config = AppConfig {
            destination_dir: Some(destination.path().to_path_buf()),
            source_dir: Some(original.path().to_path_buf()),
            working_dir: destination.path().to_path_buf(),
            ..AppConfig::default()
        };
        let mut app = App::new(config);
        app.apply_source_refresh(Some(effective.clone()), Vec::new());
        let (task_tx, mut task_rx) = mpsc::channel::<BackendTask>(8);
        let request = ActionRequest {
            action: Action::Ignore,
            targets: vec![target],
            chattr_attrs: Some("exact".to_string()),
        };

        execute_action_request(&mut app, &task_tx, request).expect("ignore action");

        let ignore = std::fs::read_to_string(effective.join(".chezmoiignore"))
            .expect("read effective ignore");
        assert!(ignore.contains("secret.txt"));
        assert!(
            !original.path().join(".chezmoiignore").exists(),
            "ignore file must not be written beside configured origin"
        );
        assert_eq!(
            app.original_source_dir(),
            Some(original.path()),
            "configured --source must remain immutable"
        );
        assert!(matches!(
            task_rx.try_recv().expect("refresh after ignore"),
            BackendTask::RefreshAll
        ));
    }
}
