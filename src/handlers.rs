use crate::actions::{
    build_action_requests, dispatch_action_request, execute_action_request, maybe_continue_batch,
    send_task, squash_lines, validate_action_requests,
};
use crate::app::{App, BackendEvent, BackendTask, ConfirmStep, InputKind, ModalState};
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
        } => {
            app.apply_refresh_entries(status, managed, unmanaged);
            app.rebuild_visible_entries();
            app.busy = false;
            maybe_enqueue_auto_detail(app, task_tx)?;
        }
        BackendEvent::DiffLoaded { target, diff } => {
            app.set_detail_diff(target.as_deref(), diff.text);
            app.busy = false;
        }
        BackendEvent::PreviewLoaded { target, content } => {
            app.set_detail_preview(&target, content);
            app.busy = false;
        }
        BackendEvent::ActionFinished { request, result } => {
            app.busy = false;
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
            if !result.stderr.trim().is_empty() {
                app.log(format!("stderr: {}", squash_lines(&result.stderr)));
            }

            if app.batch_in_progress() {
                maybe_continue_batch(app, task_tx)?;
            } else if result.exit_code == 0 {
                send_task(app, task_tx, BackendTask::RefreshAll)?;
            }
        }
        BackendEvent::Error { context, message } => {
            app.busy = false;
            app.log(format!("error[{context}]: {message}"));
            if context == "action" && app.batch_in_progress() {
                maybe_continue_batch(app, task_tx)?;
            }
        }
    }

    Ok(())
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
        ModalState::ActionMenu { .. } => handle_action_menu_key(app, key, task_tx),
        ModalState::Confirm { .. } => handle_confirm_key(app, key, task_tx),
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
        KeyCode::Char('?') => app.toggle_footer_help(),
        KeyCode::Char('/') if app.focus == crate::app::PaneFocus::List => app.open_list_filter(),
        KeyCode::Esc
            if app.focus == crate::app::PaneFocus::List && !app.list_filter().is_empty() =>
        {
            app.apply_list_filter_immediately(String::new());
            selection_changed = true;
        }
        KeyCode::Tab => app.focus = app.focus.next(),
        KeyCode::Char(' ') if app.focus == crate::app::PaneFocus::List => {
            let _ = app.toggle_selected_mark();
        }
        KeyCode::Char('c')
            if key.modifiers.is_empty() && app.focus == crate::app::PaneFocus::List =>
        {
            if app.clear_marked_entries() {
                app.log("cleared multi-selection".to_string());
            }
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
        KeyCode::Char('l') | KeyCode::Right => {
            if app.expand_selected_directory() {
                selection_changed = true;
            }
        }
        KeyCode::Char('h') | KeyCode::Left => {
            if app.collapse_selected_directory_or_parent() {
                selection_changed = true;
            }
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
            if app.view == ListView::Unmanaged && app.selected_is_directory() {
                app.clear_detail();
                return Ok(());
            }
            send_task(
                app,
                task_tx,
                BackendTask::LoadDiff {
                    target: app.selected_absolute_path(),
                },
            )?;
        }
        KeyCode::Enter => {
            if app.view == ListView::Unmanaged && app.selected_is_directory() {
                app.clear_detail();
                return Ok(());
            }
            send_task(
                app,
                task_tx,
                BackendTask::LoadDiff {
                    target: app.selected_absolute_path(),
                },
            )?;
        }
        KeyCode::Char('v') => match (app.selected_path(), app.selected_absolute_path()) {
            (Some(target), Some(absolute)) => {
                if app.view == ListView::Unmanaged && app.selected_is_directory() {
                    app.clear_detail();
                    return Ok(());
                }
                send_task(app, task_tx, BackendTask::LoadPreview { target, absolute })?;
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
        if let Some(first) = app.start_batch(requests) {
            dispatch_action_request(app, task_tx, first)?;
        }
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
    let view = app.view;

    let ModalState::ActionMenu { selected, filter } = &mut app.modal else {
        return Ok(());
    };

    match key.code {
        KeyCode::Esc => app.close_modal(),
        KeyCode::Down => {
            let indices = App::action_menu_indices(view, filter);
            if !indices.is_empty() {
                *selected = (*selected + 1) % indices.len();
            }
        }
        KeyCode::Up => {
            let indices = App::action_menu_indices(view, filter);
            if !indices.is_empty() {
                if *selected == 0 {
                    *selected = indices.len() - 1;
                } else {
                    *selected -= 1;
                }
            }
        }
        KeyCode::Backspace => {
            filter.pop();
            *selected = 0;
        }
        KeyCode::Char(c)
            if !key.modifiers.contains(KeyModifiers::CONTROL)
                && !key.modifiers.contains(KeyModifiers::ALT)
                && !key.modifiers.contains(KeyModifiers::SUPER) =>
        {
            filter.push(c);
            *selected = 0;
        }
        KeyCode::Enter => {
            let indices = App::action_menu_indices(view, filter);
            if let Some(index) = indices.get(*selected).copied() {
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

        let count = requests.len();
        if count > 1 {
            app.log(format!(
                "batch queued: action={} targets={}",
                action.label(),
                count
            ));
        }

        if let Some(first) = app.start_batch(requests) {
            app.close_modal();
            dispatch_action_request(app, task_tx, first)?;
        } else {
            app.close_modal();
        }
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
        dispatch_action_request(app, task_tx, request)?;
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
    fn question_key_toggles_footer_help() {
        let mut app = App::new(AppConfig::default());
        let (task_tx, _task_rx) = mpsc::unbounded_channel::<BackendTask>();
        let key = KeyEvent::new(KeyCode::Char('?'), KeyModifiers::NONE);

        handle_key_without_modal(&mut app, key, &task_tx).expect("handle key");
        assert!(app.footer_help);

        handle_key_without_modal(&mut app, key, &task_tx).expect("handle key");
        assert!(!app.footer_help);
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
            BackendTask::LoadPreview { target, .. } if target == std::path::Path::new(".zshrc")
        ));
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
