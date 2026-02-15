mod app;
mod config;
mod domain;
mod infra;
mod ui;

use crate::app::{App, BackendEvent, BackendTask, ConfirmStep, DetailKind, InputKind, ModalState};
use crate::config::AppConfig;
use crate::domain::{Action, ActionRequest, ListView};
use crate::infra::{ChezmoiClient, ShellChezmoiClient, action_to_args};
use anyhow::{Context, Result};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use std::fs;
use std::io;
use std::path::Path;
use std::process::Command;
use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};

const PREVIEW_MAX_BYTES: usize = 64 * 1024;
const PREVIEW_BINARY_SAMPLE_BYTES: usize = 4096;

#[tokio::main]
async fn main() -> Result<()> {
    let config = match AppConfig::load_or_default() {
        Ok(cfg) => cfg,
        Err(err) => {
            eprintln!("failed to load config, using defaults: {err:#}");
            AppConfig::default()
        }
    };

    setup_terminal()?;
    let mut terminal =
        Terminal::new(CrosstermBackend::new(io::stdout())).context("failed to create terminal")?;

    let run_result = run_app(&mut terminal, config).await;

    restore_terminal(&mut terminal)?;
    if let Err(err) = run_result {
        eprintln!("{err:#}");
        std::process::exit(1);
    }

    Ok(())
}

async fn run_app(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    config: AppConfig,
) -> Result<()> {
    let mut app = App::new(config);
    let client: Arc<dyn ChezmoiClient> = Arc::new(ShellChezmoiClient::default());

    let (task_tx, task_rx) = mpsc::unbounded_channel::<BackendTask>();
    let (event_tx, mut event_rx) = mpsc::unbounded_channel::<BackendEvent>();

    tokio::spawn(worker_loop(client, task_rx, event_tx));

    send_task(&mut app, &task_tx, BackendTask::RefreshAll)?;

    while !app.should_quit {
        while let Ok(event) = event_rx.try_recv() {
            handle_backend_event(&mut app, &task_tx, event)?;
        }

        if let Some(request) = app.pending_foreground.take() {
            run_foreground_action(terminal, &mut app, &task_tx, request)?;
        }

        terminal.draw(|frame| ui::draw(frame, &mut app))?;

        if event::poll(Duration::from_millis(100)).context("event poll failed")?
            && let Event::Key(key) = event::read().context("event read failed")?
            && key.kind == KeyEventKind::Press
        {
            handle_key_event(&mut app, key, &task_tx)?;
        }
    }

    if let Err(err) = app.config.save() {
        eprintln!("failed to save config: {err:#}");
    }

    Ok(())
}

async fn worker_loop(
    client: Arc<dyn ChezmoiClient>,
    mut task_rx: UnboundedReceiver<BackendTask>,
    event_tx: UnboundedSender<BackendEvent>,
) {
    while let Some(task) = task_rx.recv().await {
        match task {
            BackendTask::RefreshAll => {
                let c1 = client.clone();
                let status = tokio::task::spawn_blocking(move || c1.status()).await;
                let c2 = client.clone();
                let managed = tokio::task::spawn_blocking(move || c2.managed()).await;
                let c3 = client.clone();
                let unmanaged = tokio::task::spawn_blocking(move || c3.unmanaged()).await;

                match (status, managed, unmanaged) {
                    (Ok(Ok(status)), Ok(Ok(managed)), Ok(Ok(unmanaged))) => {
                        if event_tx
                            .send(BackendEvent::Refreshed {
                                status,
                                managed,
                                unmanaged,
                            })
                            .is_err()
                        {
                            break;
                        }
                    }
                    (s, m, u) => {
                        let message = format!(
                            "refresh failed: status={:?}, managed={:?}, unmanaged={:?}",
                            flatten_error(s),
                            flatten_error(m),
                            flatten_error(u)
                        );
                        if event_tx
                            .send(BackendEvent::Error {
                                context: "refresh".to_string(),
                                message,
                            })
                            .is_err()
                        {
                            break;
                        }
                    }
                }
            }
            BackendTask::LoadDiff { target } => {
                let c = client.clone();
                let target_for_worker = target.clone();
                let result =
                    tokio::task::spawn_blocking(move || c.diff(target_for_worker.as_deref())).await;
                match result {
                    Ok(Ok(diff)) => {
                        if event_tx
                            .send(BackendEvent::DiffLoaded { target, diff })
                            .is_err()
                        {
                            break;
                        }
                    }
                    other => {
                        if event_tx
                            .send(BackendEvent::Error {
                                context: "diff".to_string(),
                                message: format!("diff failed: {:?}", flatten_error(other)),
                            })
                            .is_err()
                        {
                            break;
                        }
                    }
                }
            }
            BackendTask::LoadPreview { target, absolute } => {
                let result =
                    tokio::task::spawn_blocking(move || load_file_preview(&absolute)).await;
                match result {
                    Ok(Ok(content)) => {
                        if event_tx
                            .send(BackendEvent::PreviewLoaded { target, content })
                            .is_err()
                        {
                            break;
                        }
                    }
                    other => {
                        if event_tx
                            .send(BackendEvent::Error {
                                context: "preview".to_string(),
                                message: format!("preview failed: {:?}", flatten_error(other)),
                            })
                            .is_err()
                        {
                            break;
                        }
                    }
                }
            }
            BackendTask::RunAction { request } => {
                let c = client.clone();
                let req = request.clone();
                let result = tokio::task::spawn_blocking(move || c.run(&req)).await;
                match result {
                    Ok(Ok(result)) => {
                        if event_tx
                            .send(BackendEvent::ActionFinished { request, result })
                            .is_err()
                        {
                            break;
                        }
                    }
                    other => {
                        if event_tx
                            .send(BackendEvent::Error {
                                context: "action".to_string(),
                                message: format!("action failed: {:?}", flatten_error(other)),
                            })
                            .is_err()
                        {
                            break;
                        }
                    }
                }
            }
        }
    }
}

fn flatten_error<T>(res: std::result::Result<anyhow::Result<T>, tokio::task::JoinError>) -> String {
    match res {
        Ok(Ok(_)) => "ok".to_string(),
        Ok(Err(err)) => format!("{err:#}"),
        Err(err) => format!("join error: {err}"),
    }
}

fn handle_backend_event(
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
            app.status_entries = status;
            app.managed_entries = managed;
            app.unmanaged_entries = unmanaged;
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
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| "(none)".to_string());
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

            if result.exit_code == 0 {
                send_task(app, task_tx, BackendTask::RefreshAll)?;
            }
        }
        BackendEvent::Error { context, message } => {
            app.busy = false;
            app.log(format!("error[{context}]: {message}"));
        }
    }

    Ok(())
}

fn handle_key_event(
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
        KeyCode::Tab => app.focus = app.focus.next(),
        KeyCode::Char('j') | KeyCode::Down => {
            if app.focus == crate::app::PaneFocus::Detail {
                app.scroll_detail_down(1);
            } else {
                app.select_next();
                selection_changed = true;
            }
        }
        KeyCode::Char('k') | KeyCode::Up => {
            if app.focus == crate::app::PaneFocus::Detail {
                app.scroll_detail_up(1);
            } else {
                app.select_prev();
                selection_changed = true;
            }
        }
        KeyCode::PageDown => {
            if app.focus == crate::app::PaneFocus::Detail {
                app.scroll_detail_down(20);
            }
        }
        KeyCode::PageUp => {
            if app.focus == crate::app::PaneFocus::Detail {
                app.scroll_detail_up(20);
            }
        }
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
        KeyCode::Char('2') => app.switch_view(ListView::Managed),
        KeyCode::Char('3') => {
            app.switch_view(ListView::Unmanaged);
            selection_changed = true;
        }
        KeyCode::Char('r') => send_task(app, task_tx, BackendTask::RefreshAll)?,
        KeyCode::Char('d') if key.modifiers == KeyModifiers::CONTROL => {
            if app.focus == crate::app::PaneFocus::Detail {
                app.scroll_detail_down(20);
            }
        }
        KeyCode::Char('u') if key.modifiers == KeyModifiers::CONTROL => {
            if app.focus == crate::app::PaneFocus::Detail {
                app.scroll_detail_up(20);
            }
        }
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
        let request = ActionRequest {
            action,
            target: app.selected_absolute_path(),
            chattr_attrs: None,
        };

        if action.needs_target() && request.target.is_none() {
            app.log(format!("{} requires a target file", action.label()));
            app.close_modal();
            return Ok(());
        }
        if action == Action::Add && app.selected_is_directory() {
            app.log(
                "Adding a whole directory is disabled. Expand it and select only required files."
                    .to_string(),
            );
            app.close_modal();
            return Ok(());
        }
        if action == Action::Edit && !app.selected_is_managed() {
            app.log("edit is available only for managed files".to_string());
            app.close_modal();
            return Ok(());
        }

        if action == Action::Chattr {
            app.open_input(InputKind::ChattrAttrs, request);
        } else if action.is_dangerous() {
            app.open_confirm(request);
        } else {
            app.close_modal();
            execute_action_request(app, task_tx, request)?;
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
                app.close_modal();
                return Ok(());
            }
            KeyCode::Enter => match step {
                ConfirmStep::Primary => {
                    if request.action.is_dangerous() && app.config.require_two_step_confirmation {
                        *step = ConfirmStep::DangerPhrase;
                    } else {
                        execute_request = Some(request.clone());
                    }
                }
                ConfirmStep::DangerPhrase => {
                    if let Some(phrase) = request.action.confirm_phrase() {
                        if typed == phrase {
                            execute_request = Some(request.clone());
                        } else {
                            pending_log = Some(format!(
                                "Confirmation phrase mismatch. required={} input={}",
                                phrase, typed
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
        if request.action.is_dangerous() {
            app.open_confirm(request);
        } else {
            app.close_modal();
            execute_action_request(app, task_tx, request)?;
        }
    }

    Ok(())
}

fn execute_action_request(
    app: &mut App,
    task_tx: &UnboundedSender<BackendTask>,
    request: ActionRequest,
) -> Result<()> {
    if matches!(
        request.action,
        Action::Edit | Action::Update | Action::Merge | Action::MergeAll
    ) {
        app.pending_foreground = Some(request);
        app.busy = true;
    } else {
        send_task(app, task_tx, BackendTask::RunAction { request })?;
    }
    Ok(())
}

fn send_task(
    app: &mut App,
    task_tx: &UnboundedSender<BackendTask>,
    task: BackendTask,
) -> Result<()> {
    app.busy = true;
    task_tx
        .send(task)
        .map_err(|err| anyhow::anyhow!("failed to dispatch task: {err}"))
}

fn run_foreground_action(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut App,
    task_tx: &UnboundedSender<BackendTask>,
    request: ActionRequest,
) -> Result<()> {
    restore_terminal(terminal)?;

    let result = run_chezmoi_foreground(&request);

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

            if code == 0 {
                send_task(app, task_tx, BackendTask::RefreshAll)?;
            }
        }
        Err(err) => {
            app.log(format!("foreground action error: {err:#}"));
        }
    }

    Ok(())
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

fn infer_destination_for_target(target: Option<&Path>) -> std::path::PathBuf {
    let working_dir = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let home_dir = dirs::home_dir().unwrap_or_else(|| working_dir.clone());

    match target {
        Some(path) if path.is_absolute() && path.starts_with(&home_dir) => home_dir,
        Some(path) if path.is_absolute() && path.starts_with(&working_dir) => working_dir,
        Some(path) if path.is_absolute() => home_dir,
        Some(_) => working_dir,
        None => home_dir,
    }
}

fn squash_lines(input: &str) -> String {
    input
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .take(5)
        .collect::<Vec<_>>()
        .join(" | ")
}

fn load_file_preview(path: &Path) -> Result<String> {
    let metadata = fs::symlink_metadata(path)
        .with_context(|| format!("preview target metadata failed: {}", path.display()))?;
    if metadata.file_type().is_dir() {
        return Ok("This is a directory. Expand it and select a file inside.".to_string());
    }

    let bytes = fs::read(path).with_context(|| format!("failed to read: {}", path.display()))?;
    let sample_len = bytes.len().min(PREVIEW_BINARY_SAMPLE_BYTES);
    if bytes[..sample_len].contains(&0) {
        return Ok("Cannot preview binary file.".to_string());
    }

    let limit = bytes.len().min(PREVIEW_MAX_BYTES);
    let mut text = String::from_utf8_lossy(&bytes[..limit]).to_string();
    if bytes.len() > PREVIEW_MAX_BYTES {
        text.push_str(&format!(
            "\n\n--- preview truncated at {} bytes (file size: {} bytes) ---",
            PREVIEW_MAX_BYTES,
            bytes.len()
        ));
    }
    Ok(text)
}

fn maybe_enqueue_unmanaged_preview(
    app: &mut App,
    task_tx: &UnboundedSender<BackendTask>,
) -> Result<()> {
    if app.view != ListView::Unmanaged {
        return Ok(());
    }
    if app.selected_is_directory() {
        app.clear_detail();
        return Ok(());
    }

    let (Some(target), Some(absolute)) = (app.selected_path(), app.selected_absolute_path()) else {
        return Ok(());
    };

    if app.detail_kind == DetailKind::Preview && app.detail_target.as_ref() == Some(&target) {
        return Ok(());
    }

    send_task(app, task_tx, BackendTask::LoadPreview { target, absolute })
}

fn maybe_enqueue_managed_preview(
    app: &mut App,
    task_tx: &UnboundedSender<BackendTask>,
) -> Result<()> {
    if app.view != ListView::Managed {
        return Ok(());
    }
    if app.selected_is_directory() {
        app.clear_detail();
        return Ok(());
    }

    let (Some(target), Some(absolute)) = (app.selected_path(), app.selected_absolute_path()) else {
        return Ok(());
    };

    if app.detail_kind == DetailKind::Preview && app.detail_target.as_ref() == Some(&target) {
        return Ok(());
    }

    send_task(app, task_tx, BackendTask::LoadPreview { target, absolute })
}

fn maybe_enqueue_status_diff(app: &mut App, task_tx: &UnboundedSender<BackendTask>) -> Result<()> {
    if app.view != ListView::Status {
        return Ok(());
    }

    let Some(target) = app.selected_absolute_path() else {
        return Ok(());
    };
    if app.detail_kind == DetailKind::Diff && app.detail_target.as_ref() == Some(&target) {
        return Ok(());
    }

    send_task(
        app,
        task_tx,
        BackendTask::LoadDiff {
            target: Some(target),
        },
    )
}

fn maybe_enqueue_auto_detail(app: &mut App, task_tx: &UnboundedSender<BackendTask>) -> Result<()> {
    maybe_enqueue_status_diff(app, task_tx)?;
    maybe_enqueue_managed_preview(app, task_tx)?;
    maybe_enqueue_unmanaged_preview(app, task_tx)?;
    Ok(())
}

fn setup_terminal() -> Result<()> {
    enable_raw_mode().context("failed to enable raw mode")?;
    execute!(io::stdout(), EnterAlternateScreen).context("failed to enter alternate screen")?;
    Ok(())
}

fn restore_terminal(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>) -> Result<()> {
    disable_raw_mode().context("failed to disable raw mode")?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)
        .context("failed to leave alternate screen")?;
    terminal.show_cursor().context("failed to show cursor")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flatten_error_formats_all_cases() {
        let ok = flatten_error::<()>(Ok(Ok(())));
        assert_eq!(ok, "ok");

        let err = flatten_error::<()>(Ok(Err(anyhow::anyhow!("boom"))));
        assert!(err.contains("boom"));
    }

    #[test]
    fn squash_lines_limits_output() {
        let text = "a\n\n b\n c \n d\n e\n f\n";
        let got = squash_lines(text);
        assert_eq!(got, "a | b | c | d | e");
    }

    #[test]
    fn preview_rejects_binary_files() {
        let file =
            std::env::temp_dir().join(format!("chezmoi_tui_preview_bin_{}", std::process::id()));
        std::fs::write(&file, [0, 159, 146, 150]).expect("write binary");
        let got = load_file_preview(&file).expect("preview");
        assert!(got.contains("binary file"));
        let _ = std::fs::remove_file(file);
    }

    #[test]
    fn preview_truncates_large_text() {
        let file =
            std::env::temp_dir().join(format!("chezmoi_tui_preview_txt_{}", std::process::id()));
        let payload = "a".repeat(PREVIEW_MAX_BYTES + 128);
        std::fs::write(&file, payload).expect("write text");
        let got = load_file_preview(&file).expect("preview");
        assert!(got.contains("preview truncated"));
        let _ = std::fs::remove_file(file);
    }
}
