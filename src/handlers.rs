use crate::actions::{
    build_action_requests, dispatch_action_request, execute_action_request, maybe_continue_batch,
    send_task, squash_lines, stop_batch_and_refresh, validate_action_requests,
};
use crate::app::{
    App, BackendEvent, BackendTask, ConfirmStep, InputKind, ModalState, PaneFocus, SearchScope,
};
use crate::domain::{Action, ActionRequest, ListView};
use crate::ignore::IgnorePatternMode;
use crate::preview::maybe_enqueue_auto_detail;
use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::time::Instant;
use tokio::sync::mpsc::Sender;

pub fn handle_backend_event(
    app: &mut App,
    task_tx: &Sender<BackendTask>,
    event: BackendEvent,
) -> Result<()> {
    match event {
        BackendEvent::RefreshStarted => {
            app.begin_refresh();
            app.invalidate_detail();
        }
        BackendEvent::StatusLoaded(result) => {
            match result {
                Ok(status) => app.apply_status_refresh(status),
                Err(message) => {
                    let message = format!("status unavailable: {message}");
                    app.note_refresh_error(message.clone());
                    app.set_error_notice(message);
                }
            }
            maybe_enqueue_auto_detail(app, task_tx)?;
        }
        BackendEvent::ManagedLoaded(result) => {
            match result {
                Ok(managed) => app.apply_managed_refresh(managed),
                Err(message) => {
                    let message = format!("managed unavailable: {message}");
                    app.note_refresh_error(message.clone());
                    app.set_error_notice(message);
                }
            }
            maybe_enqueue_auto_detail(app, task_tx)?;
        }
        BackendEvent::UnmanagedLoaded(result) => {
            match result {
                Ok(unmanaged) => app.apply_unmanaged_refresh(unmanaged),
                Err(message) => {
                    let message = format!("unmanaged unavailable: {message}");
                    app.note_refresh_error(message.clone());
                    app.set_error_notice(message);
                }
            }
            maybe_enqueue_auto_detail(app, task_tx)?;
        }
        BackendEvent::SourceLoaded(result) => {
            match result {
                Ok((source_dir, source)) => {
                    app.apply_source_refresh(Some(source_dir), source);
                }
                Err(message) => {
                    let message = format!("source unavailable: {message}");
                    app.note_refresh_error(message.clone());
                    app.set_error_notice(message);
                }
            }
            maybe_enqueue_auto_detail(app, task_tx)?;
        }
        BackendEvent::IgnoreLoaded(result) => {
            let ignore_error = match result {
                Ok(patterns) => app
                    .set_ignore_patterns(patterns)
                    .err()
                    .map(|error| format!("{error:#}")),
                Err(message) => {
                    app.clear_ignore_patterns();
                    Some(message)
                }
            };
            if let Some(message) = ignore_error {
                let message = format!(
                    "ignore rules unavailable; Unmanaged descendant expansion and search are disabled ({message})"
                );
                app.note_refresh_error(message.clone());
                app.set_error_notice(message);
            }
            maybe_enqueue_auto_detail(app, task_tx)?;
        }
        BackendEvent::RefreshFinished => {
            app.finish_read_task();
            app.finish_refresh();
            maybe_enqueue_auto_detail(app, task_tx)?;
        }
        BackendEvent::RefreshCoalesced | BackendEvent::DetailCoalesced { .. } => {
            app.finish_read_task();
        }
        BackendEvent::DetailError {
            request_id,
            context,
            message,
        } => {
            app.finish_read_task();
            if app.accepts_detail_request(request_id) {
                let message = format!("{context} failed: {message}");
                app.log(message.clone());
                app.set_error_notice(message);
            }
        }
        BackendEvent::DiffLoaded {
            request_id,
            target,
            diff,
        } => {
            app.finish_read_task();
            if app.accepts_detail_request(request_id) {
                app.set_detail_diff(target.as_deref(), diff.text);
            }
        }
        BackendEvent::PreviewLoaded {
            request_id,
            target,
            origin,
            content,
        } => {
            app.finish_read_task();
            if app.accepts_detail_request(request_id) {
                app.set_detail_preview(&target, origin, content);
            }
        }
        BackendEvent::ActionFinished { request, result } => {
            if request.action.is_read_only() {
                app.finish_read_task();
            } else {
                app.finish_mutation_task();
            }
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
            app.log(format!(
                "action {} {} exit={} duration={}ms timed_out={} output_limited={} stdout_truncated={} stderr_truncated={}",
                request.action.label(),
                target,
                result.exit_code,
                result.duration_ms,
                result.timed_out,
                result.output_limited,
                result.stdout_truncated,
                result.stderr_truncated,
            ));
            let verify_match = request.action == Action::Verify && result.exit_code == 0;
            let verify_drift = request.action == Action::Verify
                && result.exit_code == 1
                && !result.timed_out
                && !result.output_limited
                && !result.stdout_truncated
                && !result.stderr_truncated
                && result.stderr.trim().is_empty();
            let failed = result.timed_out
                || result.output_limited
                || result.stdout_truncated
                || result.stderr_truncated
                || result.exit_code != 0 && !verify_drift;
            if result.timed_out {
                app.set_error_notice(format!("{} timed out", request.action.label()));
            } else if result.output_limited {
                app.set_error_notice(format!("{} output limit exceeded", request.action.label()));
            } else if result.stdout_truncated || result.stderr_truncated {
                app.set_error_notice(format!(
                    "{} output was truncated; treat the result as incomplete",
                    request.action.label()
                ));
            } else if verify_match {
                app.set_success_notice(format!("verify matched for {target}"));
            } else if verify_drift {
                app.set_info_notice(format!("verify detected drift for {target}"));
            } else if result.exit_code == 0 {
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
                    result.exit_code
                ));
            }
            if !result.stderr.trim().is_empty() {
                app.log(format!("stderr: {}", squash_lines(&result.stderr)));
                if let Some(hint) = recovery_hint(&result.stderr) {
                    app.log(format!("hint: {hint}"));
                }
            }

            if request.action.is_read_only() && request.action.shows_output() {
                show_read_only_action_output(app, &request, &result);
            }

            if request.action == Action::Add
                && result.exit_code == 0
                && let Some(attrs) = followup_chattr_attrs(request.chattr_attrs.as_deref())
            {
                send_task(
                    app,
                    task_tx,
                    BackendTask::RunAction {
                        request: ActionRequest {
                            action: Action::Chattr,
                            targets: request.targets[..1].to_vec(),
                            chattr_attrs: Some(attrs),
                        },
                    },
                )?;
                return Ok(());
            }

            if failed {
                if app.batch_in_progress() {
                    stop_batch_and_refresh(app, task_tx)?;
                } else if !request.action.is_read_only() {
                    send_task(app, task_tx, BackendTask::RefreshAll)?;
                }
            } else if app.batch_in_progress() {
                maybe_continue_batch(app, task_tx)?;
            } else if !request.action.is_read_only() {
                send_task(app, task_tx, BackendTask::RefreshAll)?;
            }
        }
        BackendEvent::InspectionFinished {
            request_id,
            request,
            result,
        } => {
            app.finish_read_task();
            if !app.accepts_detail_request(request_id) {
                return Ok(());
            }
            let target = action_scope_label(&request);
            app.log(format!(
                "action {} {} exit={} duration={}ms timed_out={} output_limited={} stdout_truncated={} stderr_truncated={}",
                request.action.label(),
                target,
                result.exit_code,
                result.duration_ms,
                result.timed_out,
                result.output_limited,
                result.stdout_truncated,
                result.stderr_truncated,
            ));
            let verify_match = request.action == Action::Verify && result.exit_code == 0;
            let verify_drift = request.action == Action::Verify
                && result.exit_code == 1
                && !result.timed_out
                && !result.output_limited
                && !result.stdout_truncated
                && !result.stderr_truncated
                && result.stderr.trim().is_empty();
            if result.timed_out {
                app.set_error_notice(format!("{} timed out", request.action.label()));
            } else if result.output_limited {
                app.set_error_notice(format!("{} output limit exceeded", request.action.label()));
            } else if result.stdout_truncated || result.stderr_truncated {
                app.set_error_notice(format!(
                    "{} output was truncated; treat the result as incomplete",
                    request.action.label()
                ));
            } else if verify_match {
                app.set_success_notice(format!("verify matched for {target}"));
            } else if verify_drift {
                app.set_info_notice(format!("verify detected drift for {target}"));
            } else if result.exit_code == 0 {
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
                    result.exit_code
                ));
            }
            if !result.stderr.trim().is_empty() {
                app.log(format!("stderr: {}", squash_lines(&result.stderr)));
                if let Some(hint) = recovery_hint(&result.stderr) {
                    app.log(format!("hint: {hint}"));
                }
            }
            show_read_only_action_output(app, &request, &result);
        }
        BackendEvent::ApplyPlanPrepared {
            request_id,
            request,
            status,
            snapshot,
        } => {
            app.finish_read_task();
            if !app.accepts_apply_plan_request(request_id) {
                return Ok(());
            }
            app.status_entries = status;
            if app.view == ListView::Status {
                app.rebuild_visible_entries();
            }
            app.open_apply_plan_with_snapshot(request, snapshot);
            app.set_info_notice("apply plan refreshed");
        }
        BackendEvent::ApplyPlanChanged {
            request_id,
            request,
            status,
            snapshot,
        } => {
            app.finish_mutation_task();
            if app.accepts_apply_plan_request(request_id) {
                app.status_entries = status;
                if app.view == ListView::Status {
                    app.rebuild_visible_entries();
                }
                app.open_apply_plan_with_snapshot(request, snapshot);
                app.set_error_notice(
                    "apply plan changed; review the refreshed plan before applying",
                );
            }
        }
        BackendEvent::Error {
            context,
            message,
            mutation,
        } => {
            if mutation {
                app.finish_mutation_task();
            } else {
                app.finish_read_task();
            }
            let hint = recovery_hint(&message);
            app.log(format!("error[{context}]: {message}"));
            if let Some(hint) = hint {
                app.log(format!("hint: {hint}"));
                app.set_error_notice(format!("{context} error: {message} — {hint}"));
            } else {
                app.set_error_notice(format!("{context} error: {message}"));
            }
            if matches!(context.as_str(), "action" | "apply-reviewed") {
                if app.batch_in_progress() {
                    stop_batch_and_refresh(app, task_tx)?;
                } else if mutation {
                    send_task(app, task_tx, BackendTask::RefreshAll)?;
                }
            }
        }
    }

    Ok(())
}

fn show_read_only_action_output(
    app: &mut App,
    request: &ActionRequest,
    result: &crate::domain::CommandResult,
) {
    let action = request.action;
    let mut output = result.stdout.clone();
    if !result.stderr.trim().is_empty() {
        if !output.is_empty() {
            output.push_str("\n\n");
        }
        output.push_str("stderr:\n");
        output.push_str(&result.stderr);
    }
    if output.trim().is_empty() {
        output = format!("{} produced no output", action.label());
    }

    if matches!(action, Action::GitDiff | Action::DiffAll) {
        app.set_detail_diff(None, output);
        app.detail_title = format!("Diff: {} — {}", action.label(), action_scope_label(request));
    } else {
        let title = format!(
            "Preview: {} — {}",
            action.label(),
            action_scope_label(request)
        );
        app.set_detail_preview(
            std::path::Path::new(&title),
            crate::app::PreviewOrigin::Source,
            output,
        );
        app.detail_title = title;
    }
}

fn action_scope_label(request: &ActionRequest) -> String {
    match request.targets.as_slice() {
        [] => "(all)".to_string(),
        [target] => target.display().to_string(),
        targets => format!(
            "{} targets: {}",
            targets.len(),
            targets
                .iter()
                .map(|target| target.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

fn recovery_hint(message: &str) -> Option<&'static str> {
    let lower = message.to_ascii_lowercase();
    if lower.contains("no such file or directory") && lower.contains("chezmoi")
        || lower.contains("command not found")
    {
        return Some("install chezmoi or fix PATH, then run doctor");
    }
    if lower.contains("permission denied") {
        return Some("check file permissions or destination/source ownership");
    }
    if lower.contains("source") && (lower.contains("not found") || lower.contains("no such")) {
        return Some("check --source/config source_dir or run open-source-dir/doctor");
    }
    if lower.contains("config") && (lower.contains("parse") || lower.contains("toml")) {
        return Some("check config.toml syntax or retry with --no-config");
    }
    if lower.contains("failed to execute") || lower.contains("exit=") {
        return Some("inspect stderr above; run doctor if the cause is unclear");
    }
    None
}

pub fn handle_key_event(app: &mut App, key: KeyEvent, task_tx: &Sender<BackendTask>) -> Result<()> {
    if key.modifiers == KeyModifiers::CONTROL && key.code == KeyCode::Char('c') {
        if !matches!(app.modal, ModalState::None) {
            cancel_modal(app);
            return Ok(());
        }
        if app.has_mutating_work() {
            let message =
                "mutation or batch still running; Ctrl+C cannot abandon the subprocess".to_string();
            app.log(message.clone());
            app.set_error_notice(message);
            return Ok(());
        }
        app.should_quit = true;
        return Ok(());
    }

    match app.modal.clone() {
        ModalState::None => handle_key_without_modal(app, key, task_tx),
        ModalState::ListFilter { .. } => handle_list_filter_key(app, key, task_tx),
        ModalState::Ignore { .. } => handle_ignore_key(app, key, task_tx),
        ModalState::AddOptions { .. } => handle_add_options_key(app, key, task_tx),
        ModalState::ActionMenu { .. } => handle_action_menu_key(app, key, task_tx),
        ModalState::Help { .. } | ModalState::NoticeHistory { .. } => {
            handle_scroll_modal_key(app, key)
        }
        ModalState::Search { .. } => handle_search_key(app, key),
        ModalState::ActionPreflight { .. } => handle_action_preflight_key(app, key, task_tx),
        ModalState::Confirm { .. } => handle_confirm_key(app, key, task_tx),
        ModalState::ConfirmApply { .. } => handle_confirm_apply_key(app, key, task_tx),
        ModalState::ApplyPlan { .. } => handle_apply_plan_key(app, key, task_tx),
        ModalState::Input { .. } => handle_input_key(app, key, task_tx),
    }
}

fn cancel_modal(app: &mut App) {
    if let ModalState::ListFilter { original, .. } = app.modal.clone() {
        app.apply_list_filter_immediately(original);
    }
    app.close_modal();
}

fn handle_key_without_modal(
    app: &mut App,
    key: KeyEvent,
    task_tx: &Sender<BackendTask>,
) -> Result<()> {
    let mut selection_changed = false;

    match key.code {
        KeyCode::Char('q') => {
            if app.has_mutating_work() {
                let message = format!(
                    "quit refused: {}",
                    app.busy_message().unwrap_or("mutation or batch pending")
                );
                app.log(message.clone());
                app.set_error_notice(message);
            } else {
                app.should_quit = true;
            }
        }
        KeyCode::Char('?') => app.open_help(),
        KeyCode::Char('!') => app.open_notice_history(),
        KeyCode::Char('/') if app.focus == crate::app::PaneFocus::List => app.open_list_filter(),
        KeyCode::Char('/') if app.focus == PaneFocus::Detail => {
            app.open_search(SearchScope::Detail)
        }
        KeyCode::Char('/') if app.focus == PaneFocus::Log => app.open_search(SearchScope::Log),
        KeyCode::Esc
            if app.focus == crate::app::PaneFocus::List && !app.list_filter().is_empty() =>
        {
            app.apply_list_filter_immediately(String::new());
            selection_changed = true;
        }
        KeyCode::Tab => app.focus = app.focus.next(),
        KeyCode::BackTab => app.focus = app.focus.prev(),
        KeyCode::Char('p') if key.modifiers == KeyModifiers::CONTROL => app.open_action_menu(),
        KeyCode::Char('m') => app.toggle_layout_mode_for_focus(),
        KeyCode::Char('n') if app.focus == PaneFocus::Detail => {
            if app.detail_kind == crate::app::DetailKind::Diff
                && app.active_search_label().is_none()
            {
                app.jump_next_diff_hunk();
            } else {
                app.next_search_match(SearchScope::Detail);
            }
        }
        KeyCode::Char('N') if app.focus == PaneFocus::Detail => {
            if app.detail_kind == crate::app::DetailKind::Diff
                && app.active_search_label().is_none()
            {
                app.jump_prev_diff_hunk();
            } else {
                app.prev_search_match(SearchScope::Detail);
            }
        }
        KeyCode::Char('n') if app.focus == PaneFocus::Log => {
            app.next_search_match(SearchScope::Log);
        }
        KeyCode::Char('N') if app.focus == PaneFocus::Log => {
            app.prev_search_match(SearchScope::Log);
        }
        KeyCode::Char('L') if app.focus == PaneFocus::Detail => {
            app.scroll_detail_right(8);
        }
        KeyCode::Char('H') if app.focus == PaneFocus::Detail => {
            app.scroll_detail_left(8);
        }
        KeyCode::Char('L') if app.focus == PaneFocus::Log => {
            app.scroll_log_right(8);
        }
        KeyCode::Char('H') if app.focus == PaneFocus::Log => {
            app.scroll_log_left(8);
        }
        KeyCode::Char(' ') if app.focus == crate::app::PaneFocus::List => {
            let _ = app.toggle_selected_mark();
        }
        KeyCode::Char('c')
            if key.modifiers.is_empty()
                && app.focus == crate::app::PaneFocus::List
                && app.clear_marked_entries() =>
        {
            app.log("cleared multi-selection".to_string());
        }
        KeyCode::Char('j') | KeyCode::Down => match app.focus {
            crate::app::PaneFocus::Detail => {
                app.scroll_detail_down(1);
            }
            crate::app::PaneFocus::Log => {
                app.scroll_log_down(1);
            }
            crate::app::PaneFocus::List => {
                app.select_next();
                selection_changed = true;
            }
        },
        KeyCode::Char('k') | KeyCode::Up => match app.focus {
            crate::app::PaneFocus::Detail => {
                app.scroll_detail_up(1);
            }
            crate::app::PaneFocus::Log => {
                app.scroll_log_up(1);
            }
            crate::app::PaneFocus::List => {
                app.select_prev();
                selection_changed = true;
            }
        },
        KeyCode::Home | KeyCode::Char('g') if app.focus == PaneFocus::List => {
            app.select_first();
            selection_changed = true;
        }
        KeyCode::End | KeyCode::Char('G') if app.focus == PaneFocus::List => {
            app.select_last();
            selection_changed = true;
        }
        KeyCode::PageDown => match app.focus {
            crate::app::PaneFocus::Detail => {
                app.scroll_detail_down(20);
            }
            crate::app::PaneFocus::Log => {
                app.scroll_log_down(20);
            }
            crate::app::PaneFocus::List => {
                app.select_page(1, 20);
                selection_changed = true;
            }
        },
        KeyCode::PageUp => match app.focus {
            crate::app::PaneFocus::Detail => {
                app.scroll_detail_up(20);
            }
            crate::app::PaneFocus::Log => {
                app.scroll_log_up(20);
            }
            crate::app::PaneFocus::List => {
                app.select_page(-1, 20);
                selection_changed = true;
            }
        },
        KeyCode::Char('l') | KeyCode::Right
            if app.focus == PaneFocus::List && app.expand_selected_directory() =>
        {
            selection_changed = true;
        }
        KeyCode::Char('h') | KeyCode::Left
            if app.focus == PaneFocus::List && app.collapse_selected_directory_or_parent() =>
        {
            selection_changed = true;
        }
        KeyCode::Char('1') => {
            app.switch_view(ListView::Status);
            selection_changed = true;
        }
        KeyCode::Char('2') => {
            app.switch_view(ListView::Managed);
            selection_changed = true;
        }
        KeyCode::Char('3') => {
            app.switch_view(ListView::Unmanaged);
            selection_changed = true;
        }
        KeyCode::Char('4') => {
            app.switch_view(ListView::Source);
            selection_changed = true;
        }
        KeyCode::Char('r') => send_task(app, task_tx, BackendTask::RefreshAll)?,
        KeyCode::Char('d') if key.modifiers == KeyModifiers::CONTROL => match app.focus {
            crate::app::PaneFocus::Detail => {
                app.scroll_detail_down(20);
            }
            crate::app::PaneFocus::Log => {
                app.scroll_log_down(20);
            }
            crate::app::PaneFocus::List => {}
        },
        KeyCode::Char('u') if key.modifiers == KeyModifiers::CONTROL => match app.focus {
            crate::app::PaneFocus::Detail => {
                app.scroll_detail_up(20);
            }
            crate::app::PaneFocus::Log => {
                app.scroll_log_up(20);
            }
            crate::app::PaneFocus::List => {}
        },
        KeyCode::Char('d') if key.modifiers.is_empty() && app.focus == PaneFocus::List => {
            if app.view == ListView::Source {
                app.set_info_notice(
                    "Source entries have no destination diff; use Status or Managed for diff",
                );
                return Ok(());
            }
            if app.view == ListView::Unmanaged {
                load_selected_preview(app, task_tx)?;
                return Ok(());
            }
            let Some(target) = app.selected_absolute_path() else {
                app.set_error_notice("select a file before requesting a diff");
                return Ok(());
            };
            app.cancel_pending_detail();
            let request_id = app.begin_detail_request();
            send_task(
                app,
                task_tx,
                BackendTask::LoadDiff {
                    request_id,
                    target: Some(target),
                },
            )?;
        }
        KeyCode::Enter if app.focus == PaneFocus::List => {
            if app.toggle_selected_directory() {
                selection_changed = true;
            } else if matches!(app.view, ListView::Unmanaged | ListView::Source) {
                load_selected_preview(app, task_tx)?;
            } else {
                let Some(target) = app.selected_absolute_path() else {
                    app.set_error_notice("select a file before requesting a diff");
                    return Ok(());
                };
                app.cancel_pending_detail();
                let request_id = app.begin_detail_request();
                send_task(
                    app,
                    task_tx,
                    BackendTask::LoadDiff {
                        request_id,
                        target: Some(target),
                    },
                )?;
            }
        }
        KeyCode::Char('v') if app.focus == PaneFocus::List => {
            load_selected_preview(app, task_tx)?;
        }
        KeyCode::Char('a') => app.open_action_menu(),
        KeyCode::Char('e') => {
            if reject_new_action_while_mutating(app, Action::Edit) {
                return Ok(());
            }
            let request = ActionRequest {
                action: Action::Edit,
                targets: app.selected_absolute_path().into_iter().collect(),
                chattr_attrs: None,
            };
            if request.targets.is_empty() {
                app.log("edit requires a target path".to_string());
            } else if !app.selected_is_managed() {
                app.log("edit is available only for managed files".to_string());
            } else {
                execute_action_request(app, task_tx, request)?;
            }
        }
        _ => {}
    }

    if selection_changed {
        app.invalidate_detail();
        maybe_enqueue_auto_detail(app, task_tx)?;
    }

    Ok(())
}

fn load_selected_preview(app: &mut App, task_tx: &Sender<BackendTask>) -> Result<()> {
    if app.selected_is_directory() {
        app.cancel_pending_detail();
        app.clear_detail();
        return Ok(());
    }
    let (Some(target), Some(absolute)) = (app.selected_path(), app.selected_absolute_path()) else {
        app.log("No target selected for preview".to_string());
        return Ok(());
    };
    app.cancel_pending_detail();
    let request_id = app.begin_detail_request();
    send_task(
        app,
        task_tx,
        BackendTask::LoadPreview {
            request_id,
            target,
            absolute,
            origin: app.preview_origin_for_view(app.view),
        },
    )
}

fn handle_list_filter_key(
    app: &mut App,
    key: KeyEvent,
    task_tx: &Sender<BackendTask>,
) -> Result<()> {
    let mut immediate_filter: Option<String> = None;
    let mut finalize = false;
    let mut committed = false;
    let mut restore_filter: Option<String> = None;

    {
        let ModalState::ListFilter { value, original } = &mut app.modal else {
            return Ok(());
        };

        match key.code {
            KeyCode::Esc => {
                restore_filter = Some(original.clone());
                finalize = true;
            }
            KeyCode::Enter => {
                finalize = true;
                committed = true;
                immediate_filter = Some(value.clone());
            }
            KeyCode::Backspace => {
                value.pop();
                immediate_filter = Some(value.clone());
            }
            KeyCode::Char(c)
                if !key.modifiers.contains(KeyModifiers::CONTROL)
                    && !key.modifiers.contains(KeyModifiers::ALT)
                    && !key.modifiers.contains(KeyModifiers::SUPER) =>
            {
                value.push(c);
                immediate_filter = Some(value.clone());
            }
            _ => {}
        }
    }

    if let Some(filter) = immediate_filter {
        if finalize {
            app.apply_list_filter_immediately(filter);
        } else {
            app.stage_list_filter(filter, Instant::now());
        }
    }

    if let Some(filter) = restore_filter {
        app.apply_list_filter_immediately(filter);
        app.close_modal();
        app.rebuild_visible_entries();
        maybe_enqueue_auto_detail(app, task_tx)?;
        return Ok(());
    }

    if finalize {
        app.close_modal();
        if committed {
            app.rebuild_visible_entries();
        }
        maybe_enqueue_auto_detail(app, task_tx)?;
    }

    Ok(())
}

fn handle_ignore_key(app: &mut App, key: KeyEvent, task_tx: &Sender<BackendTask>) -> Result<()> {
    let mut start_requests: Option<Vec<ActionRequest>> = None;

    {
        let ModalState::Ignore {
            requests,
            selected,
            global_only,
        } = &mut app.modal
        else {
            return Ok(());
        };

        match key.code {
            KeyCode::Esc => {
                app.close_modal();
                return Ok(());
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if *global_only {
                    *selected = IgnorePatternMode::ALL.len() - 1;
                } else {
                    *selected = (*selected + 1) % IgnorePatternMode::ALL.len();
                }
            }
            KeyCode::Up | KeyCode::Char('k') => {
                if *global_only || *selected == 0 {
                    *selected = IgnorePatternMode::ALL.len() - 1;
                } else {
                    *selected -= 1;
                }
            }
            KeyCode::Enter => {
                let mode = IgnorePatternMode::from_index(*selected).tag().to_string();
                let mut prepared = requests.clone();
                for request in &mut prepared {
                    request.chattr_attrs = Some(mode.clone());
                }
                start_requests = Some(prepared);
            }
            _ => {}
        }
    }

    if let Some(requests) = start_requests {
        let count = requests.len();
        if count > 1 {
            app.log(format!("batch queued: action=ignore targets={count}"));
        }
        app.close_modal();
        start_action_requests_or_preflight(app, task_tx, requests)?;
    }

    Ok(())
}

fn add_options_attrs(
    template: bool,
    private: bool,
    executable: bool,
    encrypted: bool,
) -> Option<String> {
    let attrs = [
        (template, "template"),
        (private, "private"),
        (executable, "executable"),
        (encrypted, "encrypted"),
    ]
    .into_iter()
    .filter_map(|(enabled, attr)| enabled.then_some(attr))
    .collect::<Vec<_>>();

    if attrs.is_empty() {
        None
    } else {
        Some(attrs.join(","))
    }
}

fn followup_chattr_attrs(attrs: Option<&str>) -> Option<String> {
    let retained: Vec<&str> = attrs?
        .split(',')
        .map(str::trim)
        .filter(|attr| !attr.is_empty() && !matches!(*attr, "template" | "encrypt" | "encrypted"))
        .collect();
    (!retained.is_empty()).then(|| retained.join(","))
}

fn handle_add_options_key(
    app: &mut App,
    key: KeyEvent,
    task_tx: &Sender<BackendTask>,
) -> Result<()> {
    let mut start_requests: Option<Vec<ActionRequest>> = None;

    {
        let ModalState::AddOptions {
            requests,
            selected,
            template,
            private,
            executable,
            encrypted,
        } = &mut app.modal
        else {
            return Ok(());
        };

        match key.code {
            KeyCode::Esc => {
                app.close_modal();
                return Ok(());
            }
            KeyCode::Down | KeyCode::Char('j') => {
                *selected = (*selected + 1) % 4;
            }
            KeyCode::Up | KeyCode::Char('k') => {
                *selected = if *selected == 0 { 3 } else { *selected - 1 };
            }
            KeyCode::Char(' ') => match *selected {
                0 => *template = !*template,
                1 => *private = !*private,
                2 => *executable = !*executable,
                3 => *encrypted = !*encrypted,
                _ => {}
            },
            KeyCode::Enter => {
                let attrs = add_options_attrs(*template, *private, *executable, *encrypted);
                let mut prepared = requests.clone();
                for request in &mut prepared {
                    request.chattr_attrs = attrs.clone();
                }
                start_requests = Some(prepared);
            }
            _ => {}
        }
    }

    if let Some(requests) = start_requests {
        let count = requests.len();
        if count > 1 {
            app.log(format!("batch queued: action=add targets={count}"));
        }
        app.close_modal();
        start_action_requests_or_preflight(app, task_tx, requests)?;
    }

    Ok(())
}

fn handle_action_menu_key(
    app: &mut App,
    key: KeyEvent,
    task_tx: &Sender<BackendTask>,
) -> Result<()> {
    let mut selected_action: Option<Action> = None;
    let mut no_action_match = false;

    let (selected_index, filter_text) = match &app.modal {
        ModalState::ActionMenu { selected, filter } => (*selected, filter.clone()),
        _ => return Ok(()),
    };

    match key.code {
        KeyCode::Esc => {
            app.invalidate_apply_plan_request();
            app.close_modal();
        }
        KeyCode::Down => {
            let indices = app.action_menu_indices(&filter_text);
            if !indices.is_empty()
                && let ModalState::ActionMenu { selected, .. } = &mut app.modal
            {
                *selected = (selected_index + 1) % indices.len();
            }
        }
        KeyCode::Char('n') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            let indices = app.action_menu_indices(&filter_text);
            if !indices.is_empty()
                && let ModalState::ActionMenu { selected, .. } = &mut app.modal
            {
                *selected = (selected_index + 1) % indices.len();
            }
        }
        KeyCode::Up => {
            let indices = app.action_menu_indices(&filter_text);
            if !indices.is_empty()
                && let ModalState::ActionMenu { selected, .. } = &mut app.modal
            {
                *selected = if selected_index == 0 {
                    indices.len() - 1
                } else {
                    selected_index - 1
                };
            }
        }
        KeyCode::Char('p') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            let indices = app.action_menu_indices(&filter_text);
            if !indices.is_empty()
                && let ModalState::ActionMenu { selected, .. } = &mut app.modal
            {
                *selected = if selected_index == 0 {
                    indices.len() - 1
                } else {
                    selected_index - 1
                };
            }
        }
        KeyCode::Backspace => {
            if let ModalState::ActionMenu { selected, filter } = &mut app.modal {
                filter.pop();
                *selected = 0;
            }
        }
        KeyCode::Char(c)
            if !key.modifiers.contains(KeyModifiers::CONTROL)
                && !key.modifiers.contains(KeyModifiers::ALT)
                && !key.modifiers.contains(KeyModifiers::SUPER) =>
        {
            if let ModalState::ActionMenu { selected, filter } = &mut app.modal {
                filter.push(c);
                *selected = 0;
            }
        }
        KeyCode::Enter => {
            let indices = app.action_menu_indices(&filter_text);
            if let Some(index) = indices.get(selected_index).copied() {
                selected_action = App::action_by_index(index);
            } else {
                no_action_match = true;
            }
        }
        _ => {}
    }

    if no_action_match {
        app.log("No action matches the current filter".to_string());
        return Ok(());
    }

    if let Some(action) = selected_action {
        if reject_new_action_while_mutating(app, action) {
            app.close_modal();
            return Ok(());
        }

        let requests = build_action_requests(app, action);
        if requests.is_empty() {
            app.log(format!("{} requires a target file", action.label()));
            app.close_modal();
            return Ok(());
        }
        if let Some(message) = validate_action_requests(app, action, &requests) {
            app.log(message);
            app.close_modal();
            app.clear_batch();
            return Ok(());
        }
        if action == Action::Ignore {
            app.close_modal();
            app.open_ignore_menu(requests);
            return Ok(());
        }
        if action == Action::Add {
            app.close_modal();
            app.open_add_options_menu(requests);
            return Ok(());
        }

        let count = requests.len();
        if count > 1 {
            app.log(format!(
                "batch queued: action={} targets={}",
                action.label(),
                count
            ));
        }

        app.close_modal();
        start_action_requests_or_preflight(app, task_tx, requests)?;
    }

    Ok(())
}

fn reject_new_action_while_mutating(app: &mut App, action: Action) -> bool {
    if !app.has_mutating_work() {
        return false;
    }

    let running = app
        .busy_message()
        .unwrap_or("mutation or batch pending")
        .to_string();
    let message = format!(
        "{} blocked: wait for the current mutation or batch to finish ({running})",
        action.label()
    );
    app.log(message.clone());
    app.set_error_notice(message);
    true
}

fn action_requests_require_preflight(requests: &[ActionRequest]) -> bool {
    let Some(action) = requests.first().map(|request| request.action) else {
        return false;
    };
    if requests.len() > 1 {
        return true;
    }
    matches!(
        action,
        Action::Update
            | Action::MergeAll
            | Action::ExternalDiff
            | Action::Forget
            | Action::Chattr
            | Action::Destroy
            | Action::Purge
    )
}

fn start_action_requests_or_preflight(
    app: &mut App,
    task_tx: &Sender<BackendTask>,
    requests: Vec<ActionRequest>,
) -> Result<()> {
    if requests.is_empty() {
        return Ok(());
    }

    if action_requests_require_preflight(&requests) {
        app.open_action_preflight(requests);
        return Ok(());
    }

    start_action_requests_now(app, task_tx, requests)
}

fn start_action_requests_now(
    app: &mut App,
    task_tx: &Sender<BackendTask>,
    requests: Vec<ActionRequest>,
) -> Result<()> {
    let count = requests.len();
    let action = requests.first().map(|request| request.action);
    if count > 1
        && let Some(action) = action
    {
        app.log(format!(
            "batch queued: action={} targets={}",
            action.label(),
            count
        ));
    }

    if let Some(first) = app.start_batch(requests) {
        dispatch_action_request(app, task_tx, first)?;
    }
    Ok(())
}

fn handle_action_preflight_key(
    app: &mut App,
    key: KeyEvent,
    task_tx: &Sender<BackendTask>,
) -> Result<()> {
    let requests = match &app.modal {
        ModalState::ActionPreflight { requests, .. } => requests.clone(),
        _ => return Ok(()),
    };

    match key.code {
        KeyCode::Esc => {
            app.invalidate_apply_plan_request();
            app.close_modal();
        }
        KeyCode::Enter => {
            app.close_modal();
            start_action_requests_now(app, task_tx, requests)?;
        }
        KeyCode::Down | KeyCode::Char('j') => {
            app.scroll_modal_down(1);
        }
        KeyCode::Up | KeyCode::Char('k') => {
            app.scroll_modal_up(1);
        }
        KeyCode::PageDown => {
            app.scroll_modal_down(10);
        }
        KeyCode::PageUp => {
            app.scroll_modal_up(10);
        }
        _ => {}
    }

    Ok(())
}

fn handle_scroll_modal_key(app: &mut App, key: KeyEvent) -> Result<()> {
    match key.code {
        KeyCode::Esc | KeyCode::Char('?') => app.close_modal(),
        KeyCode::Down | KeyCode::Char('j') => {
            app.scroll_modal_down(1);
        }
        KeyCode::Up | KeyCode::Char('k') => {
            app.scroll_modal_up(1);
        }
        KeyCode::PageDown => {
            app.scroll_modal_down(10);
        }
        KeyCode::PageUp => {
            app.scroll_modal_up(10);
        }
        _ => {}
    }
    Ok(())
}

fn handle_search_key(app: &mut App, key: KeyEvent) -> Result<()> {
    let mut apply: Option<(SearchScope, String)> = None;
    {
        let ModalState::Search { scope, value } = &mut app.modal else {
            return Ok(());
        };
        match key.code {
            KeyCode::Esc => {
                app.close_modal();
                return Ok(());
            }
            KeyCode::Enter => apply = Some((*scope, value.clone())),
            KeyCode::Backspace => {
                value.pop();
            }
            KeyCode::Char(c)
                if !key.modifiers.contains(KeyModifiers::CONTROL)
                    && !key.modifiers.contains(KeyModifiers::ALT)
                    && !key.modifiers.contains(KeyModifiers::SUPER) =>
            {
                value.push(c);
            }
            _ => {}
        }
    }
    if let Some((scope, value)) = apply {
        let found = app.apply_search(scope, value.clone());
        app.close_modal();
        if found {
            app.set_info_notice(format!("search matched: /{}", value.trim()));
        } else {
            app.set_error_notice(format!("search not found: /{}", value.trim()));
        }
    }
    Ok(())
}

fn handle_apply_plan_key(
    app: &mut App,
    key: KeyEvent,
    task_tx: &Sender<BackendTask>,
) -> Result<()> {
    let (request, snapshot) = match &app.modal {
        ModalState::ApplyPlan {
            request, snapshot, ..
        } => (request.clone(), Some(snapshot.clone())),
        _ => return Ok(()),
    };

    match key.code {
        KeyCode::Esc => {
            app.invalidate_apply_plan_request();
            app.close_modal();
        }
        KeyCode::Enter => {
            if let Some(snapshot) = snapshot {
                app.open_confirm_apply(request, snapshot);
            }
        }
        KeyCode::Char('d') => {
            let request_id = app.begin_detail_request();
            send_task(
                app,
                task_tx,
                BackendTask::LoadDiff {
                    request_id,
                    target: None,
                },
            )?;
        }
        KeyCode::Down | KeyCode::Char('j') => {
            app.scroll_modal_down(1);
        }
        KeyCode::Up | KeyCode::Char('k') => {
            app.scroll_modal_up(1);
        }
        KeyCode::PageDown => {
            app.scroll_modal_down(10);
        }
        KeyCode::PageUp => {
            app.scroll_modal_up(10);
        }
        _ => {}
    }

    Ok(())
}

fn handle_confirm_apply_key(
    app: &mut App,
    key: KeyEvent,
    task_tx: &Sender<BackendTask>,
) -> Result<()> {
    let (request, expected_snapshot) = match &app.modal {
        ModalState::ConfirmApply { request, snapshot } => (request.clone(), snapshot.clone()),
        _ => return Ok(()),
    };

    match key.code {
        KeyCode::Esc => app.close_modal(),
        KeyCode::Enter => {
            let request_id = app.begin_apply_plan_request();
            app.close_modal();
            send_task(
                app,
                task_tx,
                BackendTask::ApplyReviewed {
                    request_id,
                    request,
                    destination: app.home_dir.clone(),
                    expected_snapshot,
                },
            )?;
        }
        _ => {}
    }
    Ok(())
}

fn handle_confirm_key(app: &mut App, key: KeyEvent, task_tx: &Sender<BackendTask>) -> Result<()> {
    let mut execute_request: Option<ActionRequest> = None;
    let mut pending_log: Option<String> = None;

    {
        let ModalState::Confirm {
            request,
            step,
            typed,
        } = &mut app.modal
        else {
            return Ok(());
        };

        match key.code {
            KeyCode::Esc => {
                if app.batch_in_progress() {
                    app.clear_batch();
                    app.log("batch canceled".to_string());
                }
                app.close_modal();
                return Ok(());
            }
            KeyCode::Enter => match step {
                ConfirmStep::Primary => {
                    if request.requires_strict_confirmation()
                        || (request.action.is_dangerous()
                            && app.config.require_two_step_confirmation)
                    {
                        *step = ConfirmStep::DangerPhrase;
                    } else {
                        execute_request = Some(request.clone());
                    }
                }
                ConfirmStep::DangerPhrase => {
                    if let Some(phrase) = request.confirmation_phrase() {
                        if typed.as_str() == phrase {
                            execute_request = Some(request.clone());
                        } else {
                            pending_log = Some(format!(
                                "Confirmation phrase mismatch. required={phrase} input={typed}"
                            ));
                        }
                    }
                }
            },
            KeyCode::Backspace => {
                if matches!(step, ConfirmStep::DangerPhrase) {
                    typed.pop();
                }
            }
            KeyCode::Char(c) => {
                if matches!(step, ConfirmStep::DangerPhrase) {
                    typed.push(c);
                }
            }
            _ => {}
        }
    }

    if let Some(line) = pending_log {
        app.log(line);
    }

    if let Some(request) = execute_request {
        if app.batch_in_progress() && !request.action.is_dangerous() {
            app.mark_batch_confirmed();
        }
        app.close_modal();
        execute_action_request(app, task_tx, request)?;
    }

    Ok(())
}

fn handle_input_key(app: &mut App, key: KeyEvent, task_tx: &Sender<BackendTask>) -> Result<()> {
    let mut ready_request: Option<ActionRequest> = None;

    {
        let ModalState::Input {
            kind,
            request,
            value,
        } = &mut app.modal
        else {
            return Ok(());
        };

        match key.code {
            KeyCode::Esc => {
                if app.batch_in_progress() {
                    app.clear_batch();
                    app.log("batch canceled".to_string());
                }
                app.close_modal();
                return Ok(());
            }
            KeyCode::Enter => match kind {
                InputKind::ChattrAttrs => {
                    if value.trim().is_empty() {
                        app.log("Please enter chattr attributes".to_string());
                    } else {
                        let mut req = request.clone();
                        req.chattr_attrs = Some(value.trim().to_string());
                        ready_request = Some(req);
                    }
                }
            },
            KeyCode::Backspace => {
                value.pop();
            }
            KeyCode::Char(c) => value.push(c),
            _ => {}
        }
    }

    if let Some(request) = ready_request {
        if request.action == Action::Chattr
            && let Some(attrs) = request.chattr_attrs.clone()
        {
            app.apply_chattr_attrs_to_batch(&attrs);
        }
        app.close_modal();
        start_action_requests_or_preflight(app, task_tx, vec![request])?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{ApplyPlan, ApplyPlanSnapshot, NoticeTone};
    use crate::config::AppConfig;
    use crate::domain::{ChangeKind, StatusEntry};
    use std::fs;
    use std::path::Path;
    use std::path::PathBuf;
    use tokio::sync::mpsc;

    fn result(exit_code: i32, stdout: &str, stderr: &str) -> crate::domain::CommandResult {
        crate::domain::CommandResult {
            exit_code,
            stdout: stdout.to_string(),
            stderr: stderr.to_string(),
            duration_ms: 1,
            timed_out: false,
            output_limited: false,
            stdout_truncated: false,
            stderr_truncated: false,
        }
    }

    #[test]
    fn invalid_ignore_pattern_reports_degraded_refresh() {
        let mut app = App::new(AppConfig::default());
        let (task_tx, _task_rx) = mpsc::channel::<BackendTask>(8);
        app.begin_busy_task();

        handle_backend_event(&mut app, &task_tx, BackendEvent::RefreshStarted)
            .expect("refresh starts");
        handle_backend_event(
            &mut app,
            &task_tx,
            BackendEvent::IgnoreLoaded(Ok(vec!["[".to_string()])),
        )
        .expect("ignore failure is isolated");
        handle_backend_event(&mut app, &task_tx, BackendEvent::RefreshFinished)
            .expect("refresh still completes");

        assert!(!app.is_busy());
        let notice = app.latest_notice().expect("notice");
        assert_eq!(notice.tone, NoticeTone::Error);
        assert!(notice.message.contains("ignore rules"));
        assert!(notice.message.contains("disabled"));
    }

    #[test]
    fn busy_stays_true_until_all_in_flight_events_finish() {
        let mut app = App::new(AppConfig::default());
        let (task_tx, _task_rx) = mpsc::channel::<BackendTask>(8);
        app.begin_busy_task();
        app.begin_busy_task();

        handle_backend_event(
            &mut app,
            &task_tx,
            BackendEvent::PreviewLoaded {
                request_id: 1,
                target: PathBuf::from("a"),
                origin: crate::app::PreviewOrigin::Destination,
                content: "a".to_string(),
            },
        )
        .expect("handle preview");
        assert!(app.is_busy());
        assert_eq!(app.busy_task_count(), 1);

        handle_backend_event(
            &mut app,
            &task_tx,
            BackendEvent::Error {
                context: "diff".to_string(),
                message: "boom".to_string(),
                mutation: false,
            },
        )
        .expect("handle error");
        assert!(!app.is_busy());
        assert_eq!(app.busy_task_count(), 0);
    }

    #[test]
    fn doctor_action_output_is_shown_in_detail() {
        let mut app = App::new(AppConfig::default());
        let (task_tx, _task_rx) = mpsc::channel::<BackendTask>(8);
        app.begin_busy_task();

        handle_backend_event(
            &mut app,
            &task_tx,
            BackendEvent::ActionFinished {
                request: ActionRequest {
                    action: Action::Doctor,
                    targets: Vec::new(),
                    chattr_attrs: None,
                },
                result: crate::domain::CommandResult {
                    exit_code: 0,
                    stdout: "ok\n".to_string(),
                    stderr: String::new(),
                    duration_ms: 10,
                    timed_out: false,
                    output_limited: false,
                    stdout_truncated: false,
                    stderr_truncated: false,
                },
            },
        )
        .expect("handle doctor");

        assert!(app.detail_title.contains("doctor"));
        assert_eq!(app.detail_text, "ok\n");
    }

    #[test]
    fn action_failure_sets_error_notice() {
        let mut app = App::new(AppConfig::default());
        let (task_tx, _task_rx) = mpsc::channel::<BackendTask>(8);
        app.begin_busy_task();

        handle_backend_event(
            &mut app,
            &task_tx,
            BackendEvent::ActionFinished {
                request: ActionRequest {
                    action: Action::Apply,
                    targets: Vec::new(),
                    chattr_attrs: None,
                },
                result: crate::domain::CommandResult {
                    exit_code: 1,
                    stdout: String::new(),
                    stderr: "failed".to_string(),
                    duration_ms: 10,
                    timed_out: false,
                    output_limited: false,
                    stdout_truncated: false,
                    stderr_truncated: false,
                },
            },
        )
        .expect("handle action failure");

        let notice = app.latest_notice().expect("notice");
        assert_eq!(notice.tone, crate::app::NoticeTone::Error);
        assert!(notice.message.contains("apply failed"));
    }

    #[test]
    fn truncated_output_is_failure_and_does_not_refresh_read_only_actions() {
        let mut app = App::new(AppConfig::default());
        let (task_tx, mut task_rx) = mpsc::channel::<BackendTask>(8);
        app.begin_read_task_with_message("inspect");

        handle_backend_event(
            &mut app,
            &task_tx,
            BackendEvent::ActionFinished {
                request: ActionRequest {
                    action: Action::Cat,
                    targets: vec![PathBuf::from(".zshrc")],
                    chattr_attrs: None,
                },
                result: crate::domain::CommandResult {
                    exit_code: 0,
                    stdout: "partial".to_string(),
                    stderr: String::new(),
                    duration_ms: 10,
                    timed_out: false,
                    output_limited: false,
                    stdout_truncated: true,
                    stderr_truncated: false,
                },
            },
        )
        .expect("truncated action");

        assert_eq!(app.latest_notice().expect("notice").tone, NoticeTone::Error);
        assert!(task_rx.try_recv().is_err());
        assert!(!app.is_busy());
    }

    #[test]
    fn truncated_mutating_batch_stops_and_refreshes_partial_state() {
        let mut app = App::new(AppConfig::default());
        let (task_tx, mut task_rx) = mpsc::channel::<BackendTask>(8);
        let first = ActionRequest {
            action: Action::Forget,
            targets: vec![PathBuf::from("/tmp/a")],
            chattr_attrs: None,
        };
        let second = ActionRequest {
            action: Action::Forget,
            targets: vec![PathBuf::from("/tmp/b")],
            chattr_attrs: None,
        };
        app.start_batch(vec![first.clone(), second]);
        app.begin_mutation_task_with_message("forget");

        handle_backend_event(
            &mut app,
            &task_tx,
            BackendEvent::ActionFinished {
                request: first,
                result: crate::domain::CommandResult {
                    exit_code: 0,
                    stdout: String::new(),
                    stderr: "partial stderr".to_string(),
                    duration_ms: 10,
                    timed_out: false,
                    output_limited: false,
                    stdout_truncated: false,
                    stderr_truncated: true,
                },
            },
        )
        .expect("truncated batch action");

        assert!(!app.batch_in_progress());
        assert!(matches!(
            task_rx.try_recv().expect("refresh"),
            BackendTask::RefreshAll
        ));
    }

    #[test]
    fn verify_exit_codes_above_one_remain_failures() {
        let mut app = App::new(AppConfig::default());
        let (task_tx, mut task_rx) = mpsc::channel::<BackendTask>(8);
        app.begin_read_task_with_message("verify");

        handle_backend_event(
            &mut app,
            &task_tx,
            BackendEvent::ActionFinished {
                request: ActionRequest {
                    action: Action::Verify,
                    targets: Vec::new(),
                    chattr_attrs: None,
                },
                result: crate::domain::CommandResult {
                    exit_code: 2,
                    stdout: String::new(),
                    stderr: "verify broke".to_string(),
                    duration_ms: 10,
                    timed_out: false,
                    output_limited: false,
                    stdout_truncated: false,
                    stderr_truncated: false,
                },
            },
        )
        .expect("verify failure");

        assert_eq!(app.latest_notice().expect("notice").tone, NoticeTone::Error);
        assert!(
            app.latest_notice()
                .is_some_and(|notice| notice.message.contains("failed"))
        );
        assert!(task_rx.try_recv().is_err());
    }

    #[test]
    fn verify_exit_one_with_stderr_is_an_execution_error() {
        let mut app = App::new(AppConfig::default());
        let (task_tx, mut task_rx) = mpsc::channel::<BackendTask>(8);
        app.begin_read_task_with_message("verify");
        let request_id = app.begin_detail_request();

        handle_backend_event(
            &mut app,
            &task_tx,
            BackendEvent::InspectionFinished {
                request_id,
                request: ActionRequest {
                    action: Action::Verify,
                    targets: Vec::new(),
                    chattr_attrs: None,
                },
                result: result(1, "", "chezmoi: failed to parse config"),
            },
        )
        .expect("verify config error");

        let notice = app.latest_notice().expect("notice");
        assert_eq!(notice.tone, NoticeTone::Error);
        assert!(notice.message.contains("failed"));
        assert!(
            app.logs
                .iter()
                .any(|line| line.contains("failed to parse config"))
        );
        assert!(task_rx.try_recv().is_err());
    }

    #[test]
    fn backend_error_sets_error_notice() {
        let mut app = App::new(AppConfig::default());
        let (task_tx, _task_rx) = mpsc::channel::<BackendTask>(8);
        app.begin_busy_task();

        handle_backend_event(
            &mut app,
            &task_tx,
            BackendEvent::Error {
                context: "refresh".to_string(),
                message: "no chezmoi".to_string(),
                mutation: false,
            },
        )
        .expect("handle backend error");

        let notice = app.latest_notice().expect("notice");
        assert_eq!(notice.tone, crate::app::NoticeTone::Error);
        assert!(notice.message.contains("refresh error"));
        assert!(notice.message.contains("no chezmoi"));
    }

    #[test]
    fn question_key_opens_help_modal() {
        let mut app = App::new(AppConfig::default());
        let (task_tx, _task_rx) = mpsc::channel::<BackendTask>(8);
        let key = KeyEvent::new(KeyCode::Char('?'), KeyModifiers::NONE);

        handle_key_without_modal(&mut app, key, &task_tx).expect("handle key");
        assert!(matches!(app.modal, ModalState::Help { .. }));
    }

    #[test]
    fn list_filter_typing_is_debounced() {
        let mut app = App::new(AppConfig::default());
        app.open_list_filter();
        let (task_tx, _task_rx) = mpsc::channel::<BackendTask>(8);
        let started = Instant::now();

        handle_list_filter_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('z'), KeyModifiers::NONE),
            &task_tx,
        )
        .expect("handle key");

        assert!(matches!(app.modal, ModalState::ListFilter { .. }));
        assert_eq!(app.list_filter(), "");
        assert!(app.flush_staged_filter(started + std::time::Duration::from_millis(121)));
        assert_eq!(app.list_filter(), "z");
    }

    #[test]
    fn list_filter_enter_applies_immediately_and_closes_modal() {
        let mut app = App::new(AppConfig::default());
        app.open_list_filter();
        let (task_tx, _task_rx) = mpsc::channel::<BackendTask>(8);

        handle_list_filter_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('z'), KeyModifiers::NONE),
            &task_tx,
        )
        .expect("handle key");
        handle_list_filter_key(
            &mut app,
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
            &task_tx,
        )
        .expect("handle key");

        assert_eq!(app.list_filter(), "z");
        assert!(matches!(app.modal, ModalState::None));
    }

    #[test]
    fn list_filter_esc_restores_original_value() {
        let mut app = App::new(AppConfig::default());
        app.apply_list_filter_immediately("git".to_string());
        app.open_list_filter();
        let (task_tx, _task_rx) = mpsc::channel::<BackendTask>(8);

        handle_list_filter_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('z'), KeyModifiers::NONE),
            &task_tx,
        )
        .expect("handle key");
        handle_list_filter_key(
            &mut app,
            KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
            &task_tx,
        )
        .expect("handle key");

        assert_eq!(app.list_filter(), "git");
        assert!(matches!(app.modal, ModalState::None));
    }

    #[test]
    fn esc_without_modal_clears_applied_list_filter() {
        let mut app = App::new(AppConfig::default());
        app.apply_list_filter_immediately("git".to_string());
        let (task_tx, _task_rx) = mpsc::channel::<BackendTask>(8);

        handle_key_without_modal(
            &mut app,
            KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
            &task_tx,
        )
        .expect("handle key");

        assert!(app.list_filter().is_empty());
        assert!(matches!(app.modal, ModalState::None));
    }

    #[test]
    fn switching_to_managed_view_enqueues_auto_preview() {
        let mut app = App::new(AppConfig::default());
        app.managed_entries = vec![PathBuf::from(".zshrc")];
        let (task_tx, mut task_rx) = mpsc::channel::<BackendTask>(8);
        let key = KeyEvent::new(KeyCode::Char('2'), KeyModifiers::NONE);

        handle_key_without_modal(&mut app, key, &task_tx).expect("handle key");
        crate::preview::flush_pending_auto_detail(
            &mut app,
            &task_tx,
            Instant::now() + std::time::Duration::from_millis(100),
        )
        .expect("flush auto preview");

        let task = task_rx.try_recv().expect("preview task");
        assert!(matches!(
            task,
            BackendTask::LoadPreview { target, request_id, .. }
                if target == std::path::Path::new(".zshrc") && request_id > 0
        ));
    }

    #[test]
    fn stale_preview_event_does_not_overwrite_latest_detail() {
        let mut app = App::new(AppConfig::default());
        let (task_tx, _task_rx) = mpsc::channel::<BackendTask>(8);
        let stale_request_id = app.begin_detail_request();
        let latest_request_id = app.begin_detail_request();

        handle_backend_event(
            &mut app,
            &task_tx,
            BackendEvent::PreviewLoaded {
                request_id: stale_request_id,
                target: PathBuf::from("old.txt"),
                origin: crate::app::PreviewOrigin::Destination,
                content: "old".to_string(),
            },
        )
        .expect("handle stale preview");
        assert!(app.detail_text.is_empty());

        handle_backend_event(
            &mut app,
            &task_tx,
            BackendEvent::PreviewLoaded {
                request_id: latest_request_id,
                target: PathBuf::from("new.txt"),
                origin: crate::app::PreviewOrigin::Destination,
                content: "new".to_string(),
            },
        )
        .expect("handle latest preview");
        assert_eq!(app.detail_text, "new");
    }

    #[test]
    fn stale_diff_event_does_not_overwrite_latest_detail() {
        let mut app = App::new(AppConfig::default());
        let (task_tx, _task_rx) = mpsc::channel::<BackendTask>(8);
        let stale_request_id = app.begin_detail_request();
        let latest_request_id = app.begin_detail_request();

        handle_backend_event(
            &mut app,
            &task_tx,
            BackendEvent::DiffLoaded {
                request_id: latest_request_id,
                target: Some(PathBuf::from("new.txt")),
                diff: crate::domain::DiffText {
                    text: "new diff".to_string(),
                },
            },
        )
        .expect("handle latest diff");
        assert_eq!(app.detail_text, "new diff");

        handle_backend_event(
            &mut app,
            &task_tx,
            BackendEvent::DiffLoaded {
                request_id: stale_request_id,
                target: Some(PathBuf::from("old.txt")),
                diff: crate::domain::DiffText {
                    text: "old diff".to_string(),
                },
            },
        )
        .expect("handle stale diff");
        assert_eq!(app.detail_text, "new diff");
    }

    #[test]
    fn explicit_inspection_output_wins_over_an_older_diff_and_names_its_target() {
        let mut app = App::new(AppConfig::default());
        let (task_tx, _task_rx) = mpsc::channel::<BackendTask>(8);
        app.begin_read_task_with_message("old diff");
        app.begin_read_task_with_message("cat");
        let stale_diff_id = app.begin_detail_request();
        let inspection_id = app.begin_detail_request();
        let request = ActionRequest {
            action: Action::Cat,
            targets: vec![PathBuf::from("/tmp/.zshrc")],
            chattr_attrs: None,
        };

        handle_backend_event(
            &mut app,
            &task_tx,
            BackendEvent::InspectionFinished {
                request_id: inspection_id,
                request,
                result: result(0, "rendered config", ""),
            },
        )
        .expect("inspection");
        handle_backend_event(
            &mut app,
            &task_tx,
            BackendEvent::DiffLoaded {
                request_id: stale_diff_id,
                target: Some(PathBuf::from("/tmp/.zshrc")),
                diff: crate::domain::DiffText {
                    text: "stale diff".to_string(),
                },
            },
        )
        .expect("stale diff");

        assert_eq!(app.detail_text, "rendered config");
        assert!(app.detail_title.contains("cat"));
        assert!(app.detail_title.contains("/tmp/.zshrc"));
        assert!(!app.is_busy());
    }

    #[test]
    fn inspection_after_view_change_cannot_overwrite_current_detail() {
        let mut app = App::new(AppConfig::default());
        let (task_tx, _task_rx) = mpsc::channel::<BackendTask>(8);
        app.begin_read_task_with_message("cat");
        let inspection_id = app.begin_detail_request();
        app.set_detail_preview(
            Path::new("new-view"),
            crate::app::PreviewOrigin::Destination,
            "new view detail".to_string(),
        );

        handle_key_without_modal(
            &mut app,
            KeyEvent::new(KeyCode::Char('2'), KeyModifiers::NONE),
            &task_tx,
        )
        .expect("switch view");
        handle_backend_event(
            &mut app,
            &task_tx,
            BackendEvent::InspectionFinished {
                request_id: inspection_id,
                request: ActionRequest {
                    action: Action::Cat,
                    targets: vec![PathBuf::from("/tmp/old")],
                    chattr_attrs: None,
                },
                result: result(0, "stale inspection", ""),
            },
        )
        .expect("stale inspection");

        assert!(!app.detail_text.contains("stale inspection"));
        assert!(!app.is_busy());
    }

    #[test]
    fn add_action_opens_attribute_wizard() {
        let mut app = App::new(AppConfig::default());
        app.unmanaged_entries = vec![PathBuf::from("new.txt")];
        app.switch_view(ListView::Unmanaged);
        let (task_tx, _task_rx) = mpsc::channel::<BackendTask>(8);
        app.open_action_menu();
        app.modal = ModalState::ActionMenu {
            selected: 0,
            filter: "add".to_string(),
        };

        handle_action_menu_key(
            &mut app,
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
            &task_tx,
        )
        .expect("select add");

        assert!(matches!(app.modal, ModalState::AddOptions { .. }));
    }

    #[test]
    fn ignore_wizard_outside_destination_selects_global_name_only() {
        let temp = tempfile::tempdir().expect("tempdir");
        let mut app = App::new(AppConfig::default());
        app.home_dir = temp.path().join("home");
        app.open_ignore_menu(vec![ActionRequest {
            action: Action::Ignore,
            targets: vec![temp.path().join("outside/cache")],
            chattr_attrs: None,
        }]);

        assert!(matches!(
            app.modal,
            ModalState::Ignore {
                selected: 4,
                global_only: true,
                ..
            }
        ));
    }

    #[test]
    fn add_options_apply_attrs_to_requests() {
        let temp = tempfile::tempdir().expect("tempdir");
        let target = temp.path().join("new.txt");
        std::fs::write(&target, "new").expect("create target");
        let mut app = App::new(AppConfig::default());
        app.home_dir = temp.path().to_path_buf();
        let (task_tx, mut task_rx) = mpsc::channel::<BackendTask>(8);
        app.open_add_options_menu(vec![ActionRequest {
            action: Action::Add,
            targets: vec![target],
            chattr_attrs: None,
        }]);

        handle_add_options_key(
            &mut app,
            KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE),
            &task_tx,
        )
        .expect("toggle template");
        handle_add_options_key(
            &mut app,
            KeyEvent::new(KeyCode::Down, KeyModifiers::NONE),
            &task_tx,
        )
        .expect("move private");
        handle_add_options_key(
            &mut app,
            KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE),
            &task_tx,
        )
        .expect("toggle private");
        handle_add_options_key(
            &mut app,
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
            &task_tx,
        )
        .expect("start add");

        let task = task_rx.try_recv().expect("add task");
        assert!(matches!(
            task,
            BackendTask::RunAction {
                request: ActionRequest {
                    action: Action::Add,
                    chattr_attrs: Some(attrs),
                    ..
                }
            } if attrs == "template,private"
        ));
    }

    #[test]
    fn successful_add_with_attrs_dispatches_chattr() {
        let mut app = App::new(AppConfig::default());
        let (task_tx, mut task_rx) = mpsc::channel::<BackendTask>(8);

        handle_backend_event(
            &mut app,
            &task_tx,
            BackendEvent::ActionFinished {
                request: ActionRequest {
                    action: Action::Add,
                    targets: vec![PathBuf::from("/tmp/new.txt")],
                    chattr_attrs: Some("template,private".to_string()),
                },
                result: crate::domain::CommandResult {
                    exit_code: 0,
                    stdout: String::new(),
                    stderr: String::new(),
                    duration_ms: 1,
                    timed_out: false,
                    output_limited: false,
                    stdout_truncated: false,
                    stderr_truncated: false,
                },
            },
        )
        .expect("handle add finished");

        let task = task_rx.try_recv().expect("chattr task");
        assert!(matches!(
            task,
            BackendTask::RunAction {
                request: ActionRequest {
                    action: Action::Chattr,
                    chattr_attrs: Some(attrs),
                    ..
                }
            } if attrs == "private"
        ));
    }

    #[test]
    fn add_followup_retains_permission_attrs_only() {
        assert_eq!(
            followup_chattr_attrs(Some("template,encrypted,private,executable")).as_deref(),
            Some("private,executable")
        );
        assert_eq!(
            followup_chattr_attrs(Some("template,encrypt")).as_deref(),
            None
        );
    }

    #[test]
    fn apply_opens_plan_before_confirmation() {
        let mut app = App::new(AppConfig::default());
        let (task_tx, mut task_rx) = mpsc::channel::<BackendTask>(8);

        dispatch_action_request(
            &mut app,
            &task_tx,
            ActionRequest {
                action: Action::Apply,
                targets: Vec::new(),
                chattr_attrs: None,
            },
        )
        .expect("dispatch apply");

        let task = task_rx.try_recv().expect("prepare apply plan task");
        assert!(matches!(
            task,
            BackendTask::PrepareApplyPlan {
                request: ActionRequest {
                    action: Action::Apply,
                    ..
                },
                ..
            }
        ));
        assert!(matches!(app.modal, ModalState::None));
    }

    #[test]
    fn apply_plan_enter_opens_apply_specific_confirmation() {
        let mut app = App::new(AppConfig::default());
        let (task_tx, mut task_rx) = mpsc::channel::<BackendTask>(8);
        let request = ActionRequest {
            action: Action::Apply,
            targets: Vec::new(),
            chattr_attrs: None,
        };
        let snapshot = ApplyPlanSnapshot {
            targets: Vec::new(),
            plan: ApplyPlan::default(),
            diff_fingerprint: 0,
        };
        app.open_apply_plan_with_snapshot(request, snapshot);

        handle_apply_plan_key(
            &mut app,
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
            &task_tx,
        )
        .expect("continue from apply plan");

        assert!(task_rx.try_recv().is_err());
        assert!(matches!(
            app.modal,
            ModalState::ConfirmApply {
                request: ActionRequest {
                    action: Action::Apply,
                    ..
                },
                ..
            }
        ));
    }

    #[test]
    fn batch_confirmation_is_reused_for_remaining_items() {
        let mut app = App::new(AppConfig::default());
        app.home_dir = PathBuf::from("/tmp");
        app.managed_entries = vec![PathBuf::from("a"), PathBuf::from("b")];
        let (task_tx, mut task_rx) = mpsc::channel::<BackendTask>(8);
        let first = ActionRequest {
            action: Action::Forget,
            targets: vec![PathBuf::from("/tmp/a")],
            chattr_attrs: None,
        };
        let second = ActionRequest {
            action: Action::Forget,
            targets: vec![PathBuf::from("/tmp/b")],
            chattr_attrs: None,
        };
        let first = app
            .start_batch(vec![first, second])
            .expect("first batch request");

        dispatch_action_request(&mut app, &task_tx, first).expect("dispatch first");
        assert!(matches!(app.modal, ModalState::Confirm { .. }));

        handle_confirm_key(
            &mut app,
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
            &task_tx,
        )
        .expect("confirm first");
        let task = task_rx.try_recv().expect("first task dispatched");
        assert!(matches!(
            task,
            BackendTask::RunAction {
                request: ActionRequest {
                    action: Action::Forget,
                    ..
                }
            }
        ));

        maybe_continue_batch(&mut app, &task_tx).expect("continue batch");
        assert!(matches!(app.modal, ModalState::None));
        let task = task_rx.try_recv().expect("second task dispatched");
        assert!(matches!(
            task,
            BackendTask::RunAction {
                request: ActionRequest {
                    action: Action::Forget,
                    ..
                }
            }
        ));
    }

    #[test]
    fn dangerous_batch_confirmation_is_not_reused_for_next_target() {
        let mut app = App::new(AppConfig::default());
        app.home_dir = PathBuf::from("/tmp");
        app.managed_entries = vec![PathBuf::from("a"), PathBuf::from("b")];
        let (task_tx, mut task_rx) = mpsc::channel::<BackendTask>(8);
        let first = ActionRequest {
            action: Action::Destroy,
            targets: vec![PathBuf::from("/tmp/a")],
            chattr_attrs: None,
        };
        let second = ActionRequest {
            action: Action::Destroy,
            targets: vec![PathBuf::from("/tmp/b")],
            chattr_attrs: None,
        };
        let first = app
            .start_batch(vec![first, second])
            .expect("first batch request");

        dispatch_action_request(&mut app, &task_tx, first).expect("dispatch first");
        app.modal = ModalState::Confirm {
            request: ActionRequest {
                action: Action::Destroy,
                targets: vec![PathBuf::from("/tmp/a")],
                chattr_attrs: None,
            },
            step: ConfirmStep::DangerPhrase,
            typed: "DESTROY /tmp/a".to_string(),
        };

        handle_confirm_key(
            &mut app,
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
            &task_tx,
        )
        .expect("confirm first destroy");
        let task = task_rx.try_recv().expect("first destroy dispatched");
        assert!(matches!(
            task,
            BackendTask::RunAction {
                request: ActionRequest {
                    action: Action::Destroy,
                    ..
                }
            }
        ));

        maybe_continue_batch(&mut app, &task_tx).expect("continue batch");

        assert!(matches!(
            app.modal,
            ModalState::Confirm {
                request: ActionRequest {
                    action: Action::Destroy,
                    ref targets,
                    ..
                },
                step: ConfirmStep::Primary,
                ..
            } if targets == &[PathBuf::from("/tmp/b")]
        ));
        assert!(task_rx.try_recv().is_err());
    }

    #[test]
    fn destroy_requires_phrase_even_when_two_step_config_is_disabled() {
        let mut app = App::new(AppConfig::default());
        app.config.require_two_step_confirmation = false;
        app.modal = ModalState::Confirm {
            request: ActionRequest {
                action: Action::Destroy,
                targets: vec![PathBuf::from("/tmp/target.txt")],
                chattr_attrs: None,
            },
            step: ConfirmStep::Primary,
            typed: String::new(),
        };
        let (task_tx, _task_rx) = mpsc::channel::<BackendTask>(8);

        handle_confirm_key(
            &mut app,
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
            &task_tx,
        )
        .expect("handle confirm");

        assert!(matches!(
            app.modal,
            ModalState::Confirm {
                step: ConfirmStep::DangerPhrase,
                ..
            }
        ));
    }

    #[test]
    fn destroy_phrase_must_include_target() {
        let mut app = App::new(AppConfig::default());
        app.modal = ModalState::Confirm {
            request: ActionRequest {
                action: Action::Destroy,
                targets: vec![PathBuf::from("/tmp/target.txt")],
                chattr_attrs: None,
            },
            step: ConfirmStep::DangerPhrase,
            typed: "DESTROY".to_string(),
        };
        let (task_tx, mut task_rx) = mpsc::channel::<BackendTask>(8);

        handle_confirm_key(
            &mut app,
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
            &task_tx,
        )
        .expect("handle confirm");

        assert!(matches!(app.modal, ModalState::Confirm { .. }));
        assert!(task_rx.try_recv().is_err());
    }

    #[test]
    fn destroy_runs_only_after_full_phrase_match() {
        let mut app = App::new(AppConfig::default());
        app.modal = ModalState::Confirm {
            request: ActionRequest {
                action: Action::Destroy,
                targets: vec![PathBuf::from("/tmp/target.txt")],
                chattr_attrs: None,
            },
            step: ConfirmStep::DangerPhrase,
            typed: "DESTROY /tmp/target.txt".to_string(),
        };
        let (task_tx, mut task_rx) = mpsc::channel::<BackendTask>(8);

        handle_confirm_key(
            &mut app,
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
            &task_tx,
        )
        .expect("handle confirm");

        assert!(matches!(app.modal, ModalState::None));
        let task = task_rx.try_recv().expect("task dispatched");
        assert!(matches!(
            task,
            BackendTask::RunAction {
                request: ActionRequest {
                    action: Action::Destroy,
                    ..
                }
            }
        ));
    }

    #[test]
    fn apply_plan_prepared_opens_modal_from_fresh_status() {
        let mut app = App::new(AppConfig::default());
        let (task_tx, _task_rx) = mpsc::channel::<BackendTask>(8);
        let request_id = app.begin_apply_plan_request();

        handle_backend_event(
            &mut app,
            &task_tx,
            BackendEvent::ApplyPlanPrepared {
                request_id,
                request: ActionRequest {
                    action: Action::Apply,
                    targets: Vec::new(),
                    chattr_attrs: None,
                },
                status: vec![StatusEntry {
                    path: PathBuf::from(".zshrc"),
                    actual_vs_state: ChangeKind::None,
                    actual_vs_target: ChangeKind::Modified,
                }],
                snapshot: ApplyPlanSnapshot {
                    targets: Vec::new(),
                    plan: ApplyPlan {
                        modified: vec![PathBuf::from(".zshrc")],
                        ..ApplyPlan::default()
                    },
                    diff_fingerprint: 123,
                },
            },
        )
        .expect("handle event");

        let ModalState::ApplyPlan { snapshot, .. } = &app.modal else {
            panic!("expected apply plan modal");
        };

        assert_eq!(snapshot.plan.modified, vec![PathBuf::from(".zshrc")]);
        assert_eq!(snapshot.diff_fingerprint, 123);
    }

    #[test]
    fn apply_plan_changed_snapshot_reopens_modal() {
        let mut app = App::new(AppConfig::default());
        let (task_tx, _task_rx) = mpsc::channel::<BackendTask>(8);
        let request_id = app.begin_apply_plan_request();

        handle_backend_event(
            &mut app,
            &task_tx,
            BackendEvent::ApplyPlanChanged {
                request_id,
                request: ActionRequest {
                    action: Action::Apply,
                    targets: Vec::new(),
                    chattr_attrs: None,
                },
                status: vec![StatusEntry {
                    path: PathBuf::from(".zshrc"),
                    actual_vs_state: ChangeKind::None,
                    actual_vs_target: ChangeKind::Modified,
                }],
                snapshot: ApplyPlanSnapshot {
                    targets: Vec::new(),
                    plan: ApplyPlan {
                        modified: vec![PathBuf::from(".zshrc")],
                        ..ApplyPlan::default()
                    },
                    diff_fingerprint: 200,
                },
            },
        )
        .expect("handle event");

        // Snapshot changed (different fingerprint), so modal should reopen instead of proceed.
        let ModalState::ApplyPlan { snapshot, .. } = &app.modal else {
            panic!("expected apply plan modal");
        };
        assert_eq!(snapshot.diff_fingerprint, 200);

        let notice = app.latest_notice().expect("notice");
        assert_eq!(notice.tone, NoticeTone::Error);
        assert!(notice.message.contains("apply plan changed"));
    }

    #[test]
    fn apply_confirmation_enqueues_one_reviewed_mutation_job() {
        let mut app = App::new(AppConfig::default());
        let (task_tx, mut task_rx) = mpsc::channel::<BackendTask>(8);
        let request = ActionRequest {
            action: Action::Apply,
            targets: vec![PathBuf::from("/tmp/a")],
            chattr_attrs: None,
        };
        let snapshot = ApplyPlanSnapshot {
            targets: request.targets.clone(),
            plan: ApplyPlan {
                modified: vec![PathBuf::from("a")],
                ..ApplyPlan::default()
            },
            diff_fingerprint: 100,
        };
        app.open_confirm_apply(request.clone(), snapshot.clone());

        handle_confirm_apply_key(
            &mut app,
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
            &task_tx,
        )
        .expect("confirm apply");

        let task = task_rx.try_recv().expect("reviewed apply task");
        assert!(matches!(
            task,
            BackendTask::ApplyReviewed {
                request: reviewed,
                expected_snapshot,
                ..
            } if reviewed == request && expected_snapshot == snapshot
        ));
        assert!(app.has_mutating_work());
    }

    #[test]
    fn refresh_error_survives_later_component_success() {
        let mut app = App::new(AppConfig::default());
        let (task_tx, _task_rx) = mpsc::channel::<BackendTask>(8);
        app.begin_busy_task();

        handle_backend_event(&mut app, &task_tx, BackendEvent::RefreshStarted)
            .expect("refresh started");
        handle_backend_event(
            &mut app,
            &task_tx,
            BackendEvent::StatusLoaded(Err("status timed out".to_string())),
        )
        .expect("status failure");
        handle_backend_event(
            &mut app,
            &task_tx,
            BackendEvent::ManagedLoaded(Ok(vec![PathBuf::from(".zshrc")])),
        )
        .expect("managed success");
        handle_backend_event(&mut app, &task_tx, BackendEvent::RefreshFinished)
            .expect("refresh finished");

        let notice = app.latest_notice().expect("notice");
        assert_eq!(notice.tone, NoticeTone::Error);
        assert!(notice.message.contains("refresh partially completed"));
        assert!(notice.message.contains("status unavailable"));
        assert!(notice.message.contains("status timed out"));
    }

    #[test]
    fn stale_detail_error_does_not_replace_newest_content() {
        let mut app = App::new(AppConfig::default());
        let (task_tx, _task_rx) = mpsc::channel::<BackendTask>(8);
        app.begin_busy_task();
        app.begin_busy_task();
        let stale = app.begin_detail_request();
        let latest = app.begin_detail_request();

        handle_backend_event(
            &mut app,
            &task_tx,
            BackendEvent::PreviewLoaded {
                request_id: latest,
                target: PathBuf::from("new"),
                origin: crate::app::PreviewOrigin::Destination,
                content: "newest".to_string(),
            },
        )
        .expect("latest preview");
        handle_backend_event(
            &mut app,
            &task_tx,
            BackendEvent::DetailError {
                request_id: stale,
                context: "preview".to_string(),
                message: "old failure".to_string(),
            },
        )
        .expect("stale error");

        assert_eq!(app.detail_text, "newest");
        assert!(!app.logs.iter().any(|line| line.contains("old failure")));
    }

    #[test]
    fn selecting_directory_clears_detail_and_pending_request() {
        let temp = tempfile::tempdir().expect("tempdir");
        fs::create_dir_all(temp.path().join("z-dir")).expect("create dir");
        fs::write(temp.path().join("a-file"), "file").expect("write file");
        let mut app = App::new(AppConfig::default());
        app.home_dir = temp.path().to_path_buf();
        app.managed_entries = vec![PathBuf::from("a-file"), PathBuf::from("z-dir")];
        app.switch_view(ListView::Managed);
        let (task_tx, _task_rx) = mpsc::channel::<BackendTask>(1);
        crate::preview::maybe_enqueue_auto_detail(&mut app, &task_tx).expect("stage file detail");
        assert!(app.pending_detail_due_at().is_some());

        app.select_next();
        crate::preview::maybe_enqueue_auto_detail(&mut app, &task_tx)
            .expect("clear directory detail");

        assert!(app.pending_detail_due_at().is_none());
        assert!(app.detail_text.is_empty());
    }

    #[test]
    fn navigation_invalidates_previous_detail_before_debounce() {
        let mut app = App::new(AppConfig::default());
        app.managed_entries = vec![PathBuf::from("a"), PathBuf::from("b")];
        app.switch_view(ListView::Managed);
        let request_id = app.begin_detail_request();
        app.set_detail_preview(
            Path::new("a"),
            crate::app::PreviewOrigin::Destination,
            "stale".to_string(),
        );
        assert!(app.accepts_detail_request(request_id));
        let (task_tx, _task_rx) = mpsc::channel::<BackendTask>(8);

        handle_key_without_modal(
            &mut app,
            KeyEvent::new(KeyCode::Down, KeyModifiers::NONE),
            &task_tx,
        )
        .expect("navigate");

        assert!(app.detail_text.is_empty());
        assert!(!app.accepts_detail_request(request_id));
        assert!(app.pending_detail_due_at().is_some());
    }

    #[test]
    fn batch_timeout_stops_remaining_targets_and_refreshes_partial_state() {
        let mut app = App::new(AppConfig::default());
        let (task_tx, mut task_rx) = mpsc::channel::<BackendTask>(8);
        let first = ActionRequest {
            action: Action::Forget,
            targets: vec![PathBuf::from("/tmp/a")],
            chattr_attrs: None,
        };
        let second = ActionRequest {
            action: Action::Forget,
            targets: vec![PathBuf::from("/tmp/b")],
            chattr_attrs: None,
        };
        app.start_batch(vec![first.clone(), second])
            .expect("first batch request");

        handle_backend_event(
            &mut app,
            &task_tx,
            BackendEvent::ActionFinished {
                request: first,
                result: crate::domain::CommandResult {
                    exit_code: -1,
                    stdout: String::new(),
                    stderr: String::new(),
                    duration_ms: 500,
                    timed_out: true,
                    output_limited: false,
                    stdout_truncated: false,
                    stderr_truncated: false,
                },
            },
        )
        .expect("timeout");

        assert!(!app.batch_in_progress());
        assert!(
            app.latest_notice()
                .is_some_and(|notice| notice.message.contains("batch stopped"))
        );
        assert!(matches!(
            task_rx.try_recv().expect("partial refresh"),
            BackendTask::RefreshAll
        ));
    }

    #[test]
    fn stale_apply_plan_result_does_not_reopen_modal() {
        let mut app = App::new(AppConfig::default());
        let (task_tx, _task_rx) = mpsc::channel::<BackendTask>(8);
        app.begin_busy_task();
        let stale = app.begin_apply_plan_request();
        let _latest = app.begin_apply_plan_request();

        handle_backend_event(
            &mut app,
            &task_tx,
            BackendEvent::ApplyPlanPrepared {
                request_id: stale,
                request: ActionRequest {
                    action: Action::Apply,
                    targets: Vec::new(),
                    chattr_attrs: None,
                },
                status: vec![StatusEntry {
                    path: PathBuf::from(".zshrc"),
                    actual_vs_state: ChangeKind::None,
                    actual_vs_target: ChangeKind::Modified,
                }],
                snapshot: ApplyPlanSnapshot {
                    targets: Vec::new(),
                    plan: ApplyPlan::default(),
                    diff_fingerprint: 1,
                },
            },
        )
        .expect("stale apply plan");

        assert!(matches!(app.modal, ModalState::None));
    }

    #[test]
    fn ctrl_c_cancels_list_filter_back_to_original_value() {
        let mut app = App::new(AppConfig::default());
        app.apply_list_filter_immediately("git".to_string());
        app.open_list_filter();
        let (task_tx, _task_rx) = mpsc::channel::<BackendTask>(8);
        handle_list_filter_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('z'), KeyModifiers::NONE),
            &task_tx,
        )
        .expect("stage filter");
        app.apply_list_filter_immediately("gitz".to_string());

        handle_key_event(
            &mut app,
            KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL),
            &task_tx,
        )
        .expect("cancel modal");

        assert_eq!(app.list_filter(), "git");
        assert!(matches!(app.modal, ModalState::None));
    }

    #[test]
    fn source_enter_previews_source_file_instead_of_running_diff() {
        let temp = tempfile::tempdir().expect("source");
        let file = temp.path().join("dot_zshrc");
        fs::write(&file, "export A=1").expect("write source");
        let mut app = App::new(AppConfig::default());
        app.apply_source_refresh(
            Some(temp.path().to_path_buf()),
            vec![PathBuf::from("dot_zshrc")],
        );
        app.switch_view(ListView::Source);
        let (task_tx, mut task_rx) = mpsc::channel::<BackendTask>(8);

        handle_key_without_modal(
            &mut app,
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
            &task_tx,
        )
        .expect("preview source");

        assert!(matches!(
            task_rx.try_recv().expect("preview task"),
            BackendTask::LoadPreview {
                absolute,
                origin: crate::app::PreviewOrigin::Source,
                ..
            } if absolute == file
        ));
    }

    #[test]
    fn quit_blocks_mutations_but_allows_read_only_loads() {
        let (task_tx, _task_rx) = mpsc::channel::<BackendTask>(8);
        let mut mutating = App::new(AppConfig::default());
        mutating.begin_mutation_task_with_message("forget /tmp/a");
        handle_key_event(
            &mut mutating,
            KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE),
            &task_tx,
        )
        .expect("blocked quit");
        assert!(!mutating.should_quit);
        assert!(
            mutating
                .latest_notice()
                .is_some_and(|notice| notice.message.contains("quit refused"))
        );

        let mut reading = App::new(AppConfig::default());
        reading.begin_read_task_with_message("refresh");
        handle_key_event(
            &mut reading,
            KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL),
            &task_tx,
        )
        .expect("read-only quit");
        assert!(reading.should_quit);
    }

    #[test]
    fn aggregate_apply_request_uses_one_plan_for_all_targets() {
        let mut app = App::new(AppConfig::default());
        let (task_tx, mut task_rx) = mpsc::channel::<BackendTask>(8);
        let request = ActionRequest {
            action: Action::Apply,
            targets: vec![PathBuf::from("/tmp/a"), PathBuf::from("/tmp/b")],
            chattr_attrs: None,
        };

        dispatch_action_request(&mut app, &task_tx, request).expect("dispatch apply");

        assert!(matches!(
            task_rx.try_recv().expect("prepare plan"),
            BackendTask::PrepareApplyPlan {
                request: ActionRequest { targets, .. },
                ..
            } if targets == [PathBuf::from("/tmp/a"), PathBuf::from("/tmp/b")]
        ));
        assert!(!app.batch_in_progress());
    }

    #[test]
    fn empty_selection_never_starts_diff_all() {
        let mut app = App::new(AppConfig::default());
        app.switch_view(ListView::Status);
        let (task_tx, mut task_rx) = mpsc::channel::<BackendTask>(8);

        for key in [KeyCode::Char('d'), KeyCode::Enter] {
            handle_key_without_modal(&mut app, KeyEvent::new(key, KeyModifiers::NONE), &task_tx)
                .expect("empty diff request");
            assert!(task_rx.try_recv().is_err());
        }
        assert!(
            app.latest_notice()
                .is_some_and(|notice| notice.message.contains("select a file"))
        );
    }

    #[test]
    fn component_errors_stay_in_state_when_notice_rendering_is_disabled() {
        let mut app = App::new(AppConfig::default());
        app.config.show_notices = false;
        let (task_tx, _task_rx) = mpsc::channel::<BackendTask>(8);

        handle_backend_event(
            &mut app,
            &task_tx,
            BackendEvent::StatusLoaded(Err("permission denied".to_string())),
        )
        .expect("status error");

        let notice = app.latest_notice().expect("error retained");
        assert_eq!(notice.tone, NoticeTone::Error);
        assert!(notice.message.contains("permission denied"));
    }

    #[test]
    fn list_navigation_and_palette_keys_are_focus_gated() {
        let mut app = App::new(AppConfig::default());
        app.managed_entries = vec![PathBuf::from("a"), PathBuf::from("b"), PathBuf::from("c")];
        app.switch_view(ListView::Managed);
        let (task_tx, _task_rx) = mpsc::channel::<BackendTask>(8);

        handle_key_without_modal(
            &mut app,
            KeyEvent::new(KeyCode::End, KeyModifiers::NONE),
            &task_tx,
        )
        .expect("End");
        assert_eq!(app.selected_index, 2);
        handle_key_without_modal(
            &mut app,
            KeyEvent::new(KeyCode::Home, KeyModifiers::NONE),
            &task_tx,
        )
        .expect("Home");
        assert_eq!(app.selected_index, 0);
        handle_key_without_modal(
            &mut app,
            KeyEvent::new(KeyCode::BackTab, KeyModifiers::NONE),
            &task_tx,
        )
        .expect("BackTab");
        assert_eq!(app.focus, PaneFocus::Log);
        handle_key_without_modal(
            &mut app,
            KeyEvent::new(KeyCode::Char('p'), KeyModifiers::CONTROL),
            &task_tx,
        )
        .expect("palette alias");
        assert!(matches!(app.modal, ModalState::ActionMenu { .. }));
    }

    #[test]
    fn action_filter_treats_j_as_text_and_ctrl_n_as_navigation() {
        let mut app = App::new(AppConfig::default());
        app.switch_view(ListView::Managed);
        app.open_action_menu();
        let (task_tx, _task_rx) = mpsc::channel::<BackendTask>(8);

        handle_action_menu_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE),
            &task_tx,
        )
        .expect("type filter");
        let ModalState::ActionMenu { filter, .. } = &app.modal else {
            panic!("action menu");
        };
        assert_eq!(filter, "j");

        handle_action_menu_key(
            &mut app,
            KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE),
            &task_tx,
        )
        .expect("clear filter");
        handle_action_menu_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('n'), KeyModifiers::CONTROL),
            &task_tx,
        )
        .expect("move selection");
        let ModalState::ActionMenu { selected, .. } = &app.modal else {
            panic!("action menu");
        };
        assert_eq!(*selected, 1);
    }
}
