mod actions;
mod app;
mod backend;
mod config;
mod domain;
mod handlers;
mod ignore;
mod infra;
mod preview;
mod terminal;
mod ui;

use crate::actions::{run_foreground_action, send_task};
use crate::app::{App, BackendEvent, BackendTask};
use crate::backend::worker_loop;
use crate::config::AppConfig;
use crate::handlers::{handle_backend_event, handle_key_event};
use crate::infra::{ChezmoiClient, ShellChezmoiClient};
use crate::terminal::{restore_terminal, setup_terminal};
use anyhow::{Context, Result};
use crossterm::event::{self, Event, KeyEventKind};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use std::io;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

#[tokio::main]
async fn main() -> Result<()> {
    setup_terminal()?;
    let mut terminal =
        Terminal::new(CrosstermBackend::new(io::stdout())).context("failed to create terminal")?;

    let run_result = run_app(&mut terminal, AppConfig::default()).await;

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
