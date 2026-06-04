use anyhow::{Context, Result};
use chezmoi_tui::actions::{run_foreground_action, send_task};
use chezmoi_tui::app::{App, BackendEvent, BackendTask};
use chezmoi_tui::backend::worker_loop;
use chezmoi_tui::cli::CliArgs;
use chezmoi_tui::config::AppConfig;
use chezmoi_tui::diagnostics;
use chezmoi_tui::handlers::{handle_backend_event, handle_key_event};
use chezmoi_tui::infra::{ChezmoiClient, ShellChezmoiClient};
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

#[tokio::main]
async fn main() -> Result<()> {
    let config = AppConfig::from_cli(CliArgs::parse())?;
    diagnostics::init(config.log_file.as_deref())?;
    tracing::info!(?config, "starting chezmoi-tui");
    let mut terminal_guard = TerminalGuard::enter()?;
    let mut terminal =
        Terminal::new(CrosstermBackend::new(io::stdout())).context("failed to create terminal")?;

    let run_result = run_app(&mut terminal, config);

    terminal_guard.restore(&mut terminal)?;
    if let Err(err) = run_result {
        eprintln!("{err:#}");
        std::process::exit(1);
    }

    Ok(())
}

const BACKEND_TASK_QUEUE_CAPACITY: usize = 64;
const BACKEND_EVENT_QUEUE_CAPACITY: usize = 64;

fn run_app(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>, config: AppConfig) -> Result<()> {
    tracing::info!(?config, "initializing app");
    let mut app = App::new(config);
    let client: Arc<dyn ChezmoiClient> = Arc::new(ShellChezmoiClient::new(
        "chezmoi",
        app.home_dir.clone(),
        app.config.working_dir.clone(),
        app.config.source_dir.clone(),
    ));

    let (task_tx, task_rx) = mpsc::channel::<BackendTask>(BACKEND_TASK_QUEUE_CAPACITY);
    let (event_tx, mut event_rx) = mpsc::channel::<BackendEvent>(BACKEND_EVENT_QUEUE_CAPACITY);

    tokio::spawn(worker_loop(client, task_rx, event_tx));

    send_task(&mut app, &task_tx, BackendTask::RefreshAll)?;

    while !app.should_quit {
        while let Ok(event) = event_rx.try_recv() {
            handle_backend_event(&mut app, &task_tx, event)?;
        }

        if let Some(request) = app.pending_foreground.take() {
            run_foreground_action(terminal, &mut app, &task_tx, &request)?;
        }

        app.flush_staged_filter(Instant::now());
        terminal.draw(|frame| ui::draw(frame, &mut app))?;

        if event::poll(Duration::from_millis(100)).context("event poll failed")?
            && let Event::Key(key) = event::read().context("event read failed")?
            && key.kind == KeyEventKind::Press
        {
            handle_key_event(&mut app, key, &task_tx)?;
        }
    }

    Ok(())
}
