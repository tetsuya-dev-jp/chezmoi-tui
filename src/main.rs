use anyhow::{Context, Result};
use chezmoi_tui::actions::{run_foreground_action, send_task};
use chezmoi_tui::app::{App, BackendEvent, BackendTask};
use chezmoi_tui::backend::worker_loop;
use chezmoi_tui::cli::CliArgs;
use chezmoi_tui::config::AppConfig;
use chezmoi_tui::diagnostics;
use chezmoi_tui::handlers::{handle_backend_event, handle_key_event};
use chezmoi_tui::infra::{ChezmoiClient, ShellChezmoiClient};
use chezmoi_tui::preview::flush_pending_auto_detail;
use chezmoi_tui::terminal::TerminalGuard;
use chezmoi_tui::ui;
use clap::Parser;
use crossterm::event::{self, Event, KeyEventKind};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use std::io;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

const BACKEND_TASK_QUEUE_CAPACITY: usize = 64;
const BACKEND_EVENT_QUEUE_CAPACITY: usize = 64;
const IDLE_POLL: Duration = Duration::from_millis(250);
const BUSY_POLL: Duration = Duration::from_millis(16);
const SCAN_POLL: Duration = Duration::from_millis(20);

#[tokio::main]
async fn main() -> Result<()> {
    let mut config = AppConfig::from_cli(CliArgs::parse())?;
    diagnostics::init(config.log_file.as_deref())?;
    if config.destination_dir.is_none() {
        config.destination_dir = Some(
            ShellChezmoiClient::resolve_destination(
                &config.chezmoi_binary,
                config.source_dir.as_deref(),
                config.chezmoi_config.as_deref(),
            )
            .context(
                "failed to resolve chezmoi destination; install chezmoi and initialize it, or pass --destination",
            )?,
        );
    }
    tracing::info!(?config, "starting chezmoi-tui");

    let client: Arc<dyn ChezmoiClient> = Arc::new(
        ShellChezmoiClient::new(
            config.chezmoi_binary.clone(),
            config
                .destination_dir
                .clone()
                .or_else(dirs::home_dir)
                .unwrap_or_else(|| config.working_dir.clone()),
            config.working_dir.clone(),
            config.source_dir.clone(),
        )
        .with_config_file(config.chezmoi_config.clone()),
    );

    let mut terminal_guard = TerminalGuard::enter()?;
    let mut terminal =
        Terminal::new(CrosstermBackend::new(io::stdout())).context("failed to create terminal")?;
    let run_result = run_app(&mut terminal, config, client.clone());

    client.cancel_pending();
    terminal_guard.restore(&mut terminal)?;
    run_result
}

fn run_app(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    config: AppConfig,
    client: Arc<dyn ChezmoiClient>,
) -> Result<()> {
    tracing::info!(?config, "initializing app");
    let mut app = App::new(config);
    let (task_tx, task_rx) = mpsc::channel::<BackendTask>(BACKEND_TASK_QUEUE_CAPACITY);
    let (event_tx, mut event_rx) = mpsc::channel::<BackendEvent>(BACKEND_EVENT_QUEUE_CAPACITY);
    let worker = tokio::spawn(worker_loop(client.clone(), task_rx, event_tx));

    let result = run_event_loop(terminal, &mut app, &task_tx, &mut event_rx);
    client.cancel_pending();
    worker.abort();
    result
}

fn run_event_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut App,
    task_tx: &mpsc::Sender<BackendTask>,
    event_rx: &mut mpsc::Receiver<BackendEvent>,
) -> Result<()> {
    send_task(app, task_tx, BackendTask::RefreshAll)?;
    let mut dirty = true;
    let mut last_busy_draw = Instant::now();

    while !app.should_quit {
        while let Ok(event) = event_rx.try_recv() {
            handle_backend_event(app, task_tx, event)?;
            dirty = true;
        }

        if let Some(request) = app.pending_foreground.take() {
            run_foreground_action(terminal, app, task_tx, &request)?;
            dirty = true;
        }

        let now = Instant::now();
        if app.flush_staged_filter(now) {
            dirty = true;
        }
        if app.advance_unmanaged_filter_scan(now) {
            dirty = true;
        }
        flush_pending_auto_detail(app, task_tx, now)?;

        if dirty {
            terminal.draw(|frame| ui::draw(frame, app))?;
            dirty = false;
            last_busy_draw = Instant::now();
        }

        while event::poll(Duration::ZERO).context("event poll failed")? {
            match event::read().context("event read failed")? {
                Event::Key(key) if key.kind == KeyEventKind::Press => {
                    handle_key_event(app, key, task_tx)?;
                    dirty = true;
                }
                Event::Resize(_, _) => dirty = true,
                _ => {}
            }
        }
        if dirty {
            continue;
        }

        let now = Instant::now();
        let wait = next_poll_timeout(app, now, last_busy_draw);
        if app.is_busy() && wait.is_zero() {
            dirty = true;
        }
        if wait.is_zero() {
            continue;
        }
        let _ = event::poll(wait).context("event poll failed")?;
    }
    Ok(())
}

fn next_poll_timeout(app: &App, now: Instant, last_busy_draw: Instant) -> Duration {
    let mut wait = IDLE_POLL;
    if app.is_busy() {
        wait = wait.min(BUSY_POLL.saturating_sub(now.saturating_duration_since(last_busy_draw)));
    }
    if app.unmanaged_filter_incomplete() {
        wait = wait.min(SCAN_POLL);
    }
    if let Some(due_at) = app.pending_detail_due_at() {
        wait = wait.min(due_at.saturating_duration_since(now));
    }
    wait
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn busy_poll_uses_short_latency_without_idle_spin() {
        let mut app = App::new(AppConfig::default());
        let now = Instant::now();
        assert_eq!(next_poll_timeout(&app, now, now), IDLE_POLL);

        app.begin_read_task_with_message("refresh");
        assert_eq!(next_poll_timeout(&app, now, now), BUSY_POLL);
        assert!(BUSY_POLL <= Duration::from_millis(16));
    }
}
