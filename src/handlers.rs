use crate::actions::{
    build_action_requests, dispatch_action_request, execute_action_request, maybe_continue_batch,
    send_task, squash_lines, validate_action_requests,
};
use crate::app::{
    App, BackendEvent, BackendTask, ConfirmStep, InputKind, ModalState, PaneFocus, SearchScope,
};
use crate::domain::{Action, ActionRequest, ListView};
use crate::ignore::IgnorePatternMode;
use crate::preview::maybe_enqueue_auto_detail;
use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use tokio::sync::mpsc::UnboundedSender;

pub(crate) fn handle_backend_event(
    app: &mut App,
    task_tx: &UnboundedSender<BackendTask>,
    event: BackendEvent,
) -> Result<()> {
    match event {
        BackendEvent::Refreshed {
            status,
            managed,
            unmanaged,
            source_dir,
            source,
        } => {
            app.finish_busy_task();
            app.apply_refresh_entries(status, managed, unmanaged, source_dir, source);
            app.rebuild_visible_entries();
            app.set_info_notice("refresh completed");
            maybe_enqueue_auto_detail(app, task_tx)?;
        }
        BackendEvent::DiffLoaded {
            request_id,
            target,
            diff,
        } => {
            app.finish_busy_task();
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
            app.finish_busy_task();
            if app.accepts_detail_request(request_id) {
                app.set_detail_preview(&target, origin, content);
            }
        }
        BackendEvent::ActionFinished { request, result } => {
            app.finish_busy_task();
            let target = request
                .target
                .as_ref()
                .map_or_else(|| "(none)".to_string(), |p| p.display().to_string());
            app.log(format!(
                "action {} {} exit={} duration={}ms",
                request.action.label(),
                target,
                result.exit_code,
                result.duration_ms
            ));
            if result.exit_code == 0 {
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

            if matches!(request.action, Action::Doctor | Action::Data) {
                let mut output = result.stdout.clone();
                if !result.stderr.trim().is_empty() {
                    if !output.is_empty() {
                        output.push_str("\n\n");
                    }
                    output.push_str("stderr:\n");
                    output.push_str(&result.stderr);
                }
                if output.trim().is_empty() {
                    output = format!("{} produced no output", request.action.label());
                }
                app.set_detail_preview(
                    std::path::Path::new(request.action.label()),
                    crate::app::PreviewOrigin::Source,
                    output,
                );
            }

            if request.action == Action::Add
                && result.exit_code == 0
                && let Some(attrs) = request.chattr_attrs.clone()
                && !attrs.trim().is_empty()
            {
                send_task(
                    app,
                    task_tx,
                    BackendTask::RunAction {
                        request: ActionRequest {
                            action: Action::Chattr,
                            target: request.target,
                            chattr_attrs: Some(attrs),
                        },
                    },
                )?;
                return Ok(());
            }

            if app.batch_in_progress() {
                maybe_continue_batch(app, task_tx)?;
            } else if result.exit_code == 0 {
                send_task(app, task_tx, BackendTask::RefreshAll)?;
            }
        }
        BackendEvent::Error { context, message } => {
            app.finish_busy_task();
            let hint = recovery_hint(&message);
            app.log(format!("error[{context}]: {message}"));
            if let Some(hint) = hint {
                app.log(format!("hint: {hint}"));
                app.set_error_notice(format!("{context} error: {message} — {hint}"));
            } else {
                app.set_error_notice(format!("{context} error: {message}"));
            }
            if context == "action" && app.batch_in_progress() {
                maybe_continue_batch(app, task_tx)?;
            }
        }
    }

    Ok(())
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

pub(crate) fn handle_key_event(
    app: &mut App,
    key: KeyEvent,
    task_tx: &UnboundedSender<BackendTask>,
) -> Result<()> {
    if key.modifiers == KeyModifiers::CONTROL && key.code == KeyCode::Char('c') {
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
        ModalState::ApplyPlan { .. } => handle_apply_plan_key(app, key, task_tx),
        ModalState::Input { .. } => handle_input_key(app, key, task_tx),
    }
}

fn handle_key_without_modal(
    app: &mut App,
    key: KeyEvent,
    task_tx: &UnboundedSender<BackendTask>,
) -> Result<()> {
    let mut selection_changed = false;

    match key.code {
        KeyCode::Char('q') => app.should_quit = true,
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
        KeyCode::PageDown => match app.focus {
            crate::app::PaneFocus::Detail => {
                app.scroll_detail_down(20);
            }
            crate::app::PaneFocus::Log => {
                app.scroll_log_down(20);
            }
            crate::app::PaneFocus::List => {}
        },
        KeyCode::PageUp => match app.focus {
            crate::app::PaneFocus::Detail => {
                app.scroll_detail_up(20);
            }
            crate::app::PaneFocus::Log => {
                app.scroll_log_up(20);
            }
            crate::app::PaneFocus::List => {}
        },
        KeyCode::Char('l') | KeyCode::Right if app.expand_selected_directory() => {
            selection_changed = true;
        }
        KeyCode::Char('h') | KeyCode::Left if app.collapse_selected_directory_or_parent() => {
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
        KeyCode::Char('d') if key.modifiers.is_empty() => {
            if matches!(app.view, ListView::Unmanaged | ListView::Source)
                && app.selected_is_directory()
            {
                app.clear_detail();
                return Ok(());
            }
            let request_id = app.begin_detail_request();
            let target = app.selected_absolute_path();
            send_task(app, task_tx, BackendTask::LoadDiff { request_id, target })?;
        }
        KeyCode::Enter => {
            if matches!(app.view, ListView::Unmanaged | ListView::Source)
                && app.selected_is_directory()
            {
                app.clear_detail();
                return Ok(());
            }
            let request_id = app.begin_detail_request();
            let target = app.selected_absolute_path();
            send_task(app, task_tx, BackendTask::LoadDiff { request_id, target })?;
        }
        KeyCode::Char('v') => match (app.selected_path(), app.selected_absolute_path()) {
            (Some(target), Some(absolute)) => {
                if matches!(app.view, ListView::Unmanaged | ListView::Source)
                    && app.selected_is_directory()
                {
                    app.clear_detail();
                    return Ok(());
                }
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
                )?;
            }
            _ => app.log("No target selected for preview".to_string()),
        },
        KeyCode::Char('a') => app.open_action_menu(),
        KeyCode::Char('e') => {
            let request = ActionRequest {
                action: Action::Edit,
                target: app.selected_absolute_path(),
                chattr_attrs: None,
            };
            if request.target.is_none() {
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
        maybe_enqueue_auto_detail(app, task_tx)?;
    }

    Ok(())
}

fn handle_list_filter_key(
    app: &mut App,
    key: KeyEvent,
    task_tx: &UnboundedSender<BackendTask>,
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
        app.apply_list_filter_immediately(filter);
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

fn handle_ignore_key(
    app: &mut App,
    key: KeyEvent,
    task_tx: &UnboundedSender<BackendTask>,
) -> Result<()> {
    let mut start_requests: Option<Vec<ActionRequest>> = None;

    {
        let ModalState::Ignore { requests, selected } = &mut app.modal else {
            return Ok(());
        };

        match key.code {
            KeyCode::Esc => {
                app.close_modal();
                return Ok(());
            }
            KeyCode::Down | KeyCode::Char('j') => {
                *selected = (*selected + 1) % IgnorePatternMode::ALL.len();
            }
            KeyCode::Up | KeyCode::Char('k') => {
                if *selected == 0 {
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

fn handle_add_options_key(
    app: &mut App,
    key: KeyEvent,
    task_tx: &UnboundedSender<BackendTask>,
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
    task_tx: &UnboundedSender<BackendTask>,
) -> Result<()> {
    let mut selected_action: Option<Action> = None;
    let mut no_action_match = false;

    let (selected_index, filter_text) = match &app.modal {
        ModalState::ActionMenu { selected, filter } => (*selected, filter.clone()),
        _ => return Ok(()),
    };

    match key.code {
        KeyCode::Esc => app.close_modal(),
        KeyCode::Down => {
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

fn action_requests_require_preflight(requests: &[ActionRequest]) -> bool {
    let Some(action) = requests.first().map(|request| request.action) else {
        return false;
    };
    if requests.len() > 1 {
        return true;
    }
    matches!(
        action,
        Action::Apply
            | Action::Update
            | Action::MergeAll
            | Action::Forget
            | Action::Chattr
            | Action::Destroy
            | Action::Purge
    )
}

fn start_action_requests_or_preflight(
    app: &mut App,
    task_tx: &UnboundedSender<BackendTask>,
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
    task_tx: &UnboundedSender<BackendTask>,
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
    task_tx: &UnboundedSender<BackendTask>,
) -> Result<()> {
    let requests = match &app.modal {
        ModalState::ActionPreflight { requests, .. } => requests.clone(),
        _ => return Ok(()),
    };

    match key.code {
        KeyCode::Esc => app.close_modal(),
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
    task_tx: &UnboundedSender<BackendTask>,
) -> Result<()> {
    let request = match &app.modal {
        ModalState::ApplyPlan { request, .. } => request.clone(),
        _ => return Ok(()),
    };

    match key.code {
        KeyCode::Esc => app.close_modal(),
        KeyCode::Enter => {
            app.close_modal();
            if request.action.requires_confirmation() && !app.batch_confirmed() {
                app.open_confirm(request);
            } else {
                execute_action_request(app, task_tx, request)?;
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

fn handle_confirm_key(
    app: &mut App,
    key: KeyEvent,
    task_tx: &UnboundedSender<BackendTask>,
) -> Result<()> {
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

fn handle_input_key(
    app: &mut App,
    key: KeyEvent,
    task_tx: &UnboundedSender<BackendTask>,
) -> Result<()> {
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
    use crate::config::AppConfig;
    use std::path::PathBuf;
    use tokio::sync::mpsc;

    #[test]
    fn busy_stays_true_until_all_in_flight_events_finish() {
        let mut app = App::new(AppConfig::default());
        let (task_tx, _task_rx) = mpsc::unbounded_channel::<BackendTask>();
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
            },
        )
        .expect("handle error");
        assert!(!app.is_busy());
        assert_eq!(app.busy_task_count(), 0);
    }

    #[test]
    fn doctor_action_output_is_shown_in_detail() {
        let mut app = App::new(AppConfig::default());
        let (task_tx, _task_rx) = mpsc::unbounded_channel::<BackendTask>();
        app.begin_busy_task();

        handle_backend_event(
            &mut app,
            &task_tx,
            BackendEvent::ActionFinished {
                request: ActionRequest {
                    action: Action::Doctor,
                    target: None,
                    chattr_attrs: None,
                },
                result: crate::domain::CommandResult {
                    exit_code: 0,
                    stdout: "ok\n".to_string(),
                    stderr: String::new(),
                    duration_ms: 10,
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
        let (task_tx, _task_rx) = mpsc::unbounded_channel::<BackendTask>();
        app.begin_busy_task();

        handle_backend_event(
            &mut app,
            &task_tx,
            BackendEvent::ActionFinished {
                request: ActionRequest {
                    action: Action::Apply,
                    target: None,
                    chattr_attrs: None,
                },
                result: crate::domain::CommandResult {
                    exit_code: 1,
                    stdout: String::new(),
                    stderr: "failed".to_string(),
                    duration_ms: 10,
                },
            },
        )
        .expect("handle action failure");

        let notice = app.latest_notice().expect("notice");
        assert_eq!(notice.tone, crate::app::NoticeTone::Error);
        assert!(notice.message.contains("apply failed"));
    }

    #[test]
    fn backend_error_sets_error_notice() {
        let mut app = App::new(AppConfig::default());
        let (task_tx, _task_rx) = mpsc::unbounded_channel::<BackendTask>();
        app.begin_busy_task();

        handle_backend_event(
            &mut app,
            &task_tx,
            BackendEvent::Error {
                context: "refresh".to_string(),
                message: "no chezmoi".to_string(),
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
        let (task_tx, _task_rx) = mpsc::unbounded_channel::<BackendTask>();
        let key = KeyEvent::new(KeyCode::Char('?'), KeyModifiers::NONE);

        handle_key_without_modal(&mut app, key, &task_tx).expect("handle key");
        assert!(matches!(app.modal, ModalState::Help { .. }));
    }

    #[test]
    fn list_filter_typing_applies_immediately() {
        let mut app = App::new(AppConfig::default());
        app.open_list_filter();
        let (task_tx, _task_rx) = mpsc::unbounded_channel::<BackendTask>();

        handle_list_filter_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('z'), KeyModifiers::NONE),
            &task_tx,
        )
        .expect("handle key");

        assert!(matches!(app.modal, ModalState::ListFilter { .. }));
        assert_eq!(app.list_filter(), "z");
    }

    #[test]
    fn list_filter_enter_applies_immediately_and_closes_modal() {
        let mut app = App::new(AppConfig::default());
        app.open_list_filter();
        let (task_tx, _task_rx) = mpsc::unbounded_channel::<BackendTask>();

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
        let (task_tx, _task_rx) = mpsc::unbounded_channel::<BackendTask>();

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
        let (task_tx, _task_rx) = mpsc::unbounded_channel::<BackendTask>();

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
        let (task_tx, mut task_rx) = mpsc::unbounded_channel::<BackendTask>();
        let key = KeyEvent::new(KeyCode::Char('2'), KeyModifiers::NONE);

        handle_key_without_modal(&mut app, key, &task_tx).expect("handle key");

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
        let (task_tx, _task_rx) = mpsc::unbounded_channel::<BackendTask>();
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
        let (task_tx, _task_rx) = mpsc::unbounded_channel::<BackendTask>();
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
    fn add_action_opens_attribute_wizard() {
        let mut app = App::new(AppConfig::default());
        app.unmanaged_entries = vec![PathBuf::from("new.txt")];
        app.switch_view(ListView::Unmanaged);
        let (task_tx, _task_rx) = mpsc::unbounded_channel::<BackendTask>();
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
    fn add_options_apply_attrs_to_requests() {
        let mut app = App::new(AppConfig::default());
        let (task_tx, mut task_rx) = mpsc::unbounded_channel::<BackendTask>();
        app.open_add_options_menu(vec![ActionRequest {
            action: Action::Add,
            target: Some(PathBuf::from("/tmp/new.txt")),
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
        let (task_tx, mut task_rx) = mpsc::unbounded_channel::<BackendTask>();

        handle_backend_event(
            &mut app,
            &task_tx,
            BackendEvent::ActionFinished {
                request: ActionRequest {
                    action: Action::Add,
                    target: Some(PathBuf::from("/tmp/new.txt")),
                    chattr_attrs: Some("template,private".to_string()),
                },
                result: crate::domain::CommandResult {
                    exit_code: 0,
                    stdout: String::new(),
                    stderr: String::new(),
                    duration_ms: 1,
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
            } if attrs == "template,private"
        ));
    }

    #[test]
    fn apply_opens_plan_before_confirmation() {
        let mut app = App::new(AppConfig::default());
        let (task_tx, mut task_rx) = mpsc::unbounded_channel::<BackendTask>();

        dispatch_action_request(
            &mut app,
            &task_tx,
            ActionRequest {
                action: Action::Apply,
                target: None,
                chattr_attrs: None,
            },
        )
        .expect("dispatch apply");

        assert!(matches!(
            app.modal,
            ModalState::ApplyPlan {
                request: ActionRequest {
                    action: Action::Apply,
                    ..
                },
                ..
            }
        ));
        assert!(task_rx.try_recv().is_err());
    }

    #[test]
    fn apply_plan_enter_opens_confirmation() {
        let mut app = App::new(AppConfig::default());
        let (task_tx, mut task_rx) = mpsc::unbounded_channel::<BackendTask>();
        app.open_apply_plan(ActionRequest {
            action: Action::Apply,
            target: None,
            chattr_attrs: None,
        });

        handle_apply_plan_key(
            &mut app,
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
            &task_tx,
        )
        .expect("continue from apply plan");

        assert!(matches!(
            app.modal,
            ModalState::Confirm {
                request: ActionRequest {
                    action: Action::Apply,
                    ..
                },
                ..
            }
        ));
        assert!(task_rx.try_recv().is_err());
    }

    #[test]
    fn batch_confirmation_is_reused_for_remaining_items() {
        let mut app = App::new(AppConfig::default());
        let (task_tx, mut task_rx) = mpsc::unbounded_channel::<BackendTask>();
        let first = ActionRequest {
            action: Action::Forget,
            target: Some(PathBuf::from("/tmp/a")),
            chattr_attrs: None,
        };
        let second = ActionRequest {
            action: Action::Forget,
            target: Some(PathBuf::from("/tmp/b")),
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
        let (task_tx, mut task_rx) = mpsc::unbounded_channel::<BackendTask>();
        let first = ActionRequest {
            action: Action::Destroy,
            target: Some(PathBuf::from("/tmp/a")),
            chattr_attrs: None,
        };
        let second = ActionRequest {
            action: Action::Destroy,
            target: Some(PathBuf::from("/tmp/b")),
            chattr_attrs: None,
        };
        let first = app
            .start_batch(vec![first, second])
            .expect("first batch request");

        dispatch_action_request(&mut app, &task_tx, first).expect("dispatch first");
        app.modal = ModalState::Confirm {
            request: ActionRequest {
                action: Action::Destroy,
                target: Some(PathBuf::from("/tmp/a")),
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
                    ref target,
                    ..
                },
                step: ConfirmStep::Primary,
                ..
            } if target.as_deref() == Some(std::path::Path::new("/tmp/b"))
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
                target: Some(PathBuf::from("/tmp/target.txt")),
                chattr_attrs: None,
            },
            step: ConfirmStep::Primary,
            typed: String::new(),
        };
        let (task_tx, _task_rx) = mpsc::unbounded_channel::<BackendTask>();

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
                target: Some(PathBuf::from("/tmp/target.txt")),
                chattr_attrs: None,
            },
            step: ConfirmStep::DangerPhrase,
            typed: "DESTROY".to_string(),
        };
        let (task_tx, mut task_rx) = mpsc::unbounded_channel::<BackendTask>();

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
                target: Some(PathBuf::from("/tmp/target.txt")),
                chattr_attrs: None,
            },
            step: ConfirmStep::DangerPhrase,
            typed: "DESTROY /tmp/target.txt".to_string(),
        };
        let (task_tx, mut task_rx) = mpsc::unbounded_channel::<BackendTask>();

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
}
