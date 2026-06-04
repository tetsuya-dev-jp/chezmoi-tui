use anyhow::{Context, Result};
use crossterm::cursor::Show;
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use std::io;
use std::panic;
use std::sync::Once;

static PANIC_HOOK: Once = Once::new();

pub struct TerminalGuard {
    active: bool,
}

impl TerminalGuard {
    pub fn enter() -> Result<Self> {
        install_panic_restore_hook();
        setup_terminal()?;
        Ok(Self { active: true })
    }

    pub fn restore(&mut self, terminal: &mut Terminal<CrosstermBackend<io::Stdout>>) -> Result<()> {
        restore_terminal(terminal)?;
        self.active = false;
        Ok(())
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        if self.active {
            restore_terminal_state();
        }
    }
}

fn install_panic_restore_hook() {
    PANIC_HOOK.call_once(|| {
        let previous = panic::take_hook();
        panic::set_hook(Box::new(move |info| {
            restore_terminal_state();
            previous(info);
        }));
    });
}

pub fn setup_terminal() -> Result<()> {
    enable_raw_mode().context("failed to enable raw mode")?;
    execute!(io::stdout(), EnterAlternateScreen).context("failed to enter alternate screen")?;
    Ok(())
}

pub fn restore_terminal(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>) -> Result<()> {
    disable_raw_mode().context("failed to disable raw mode")?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen, Show)
        .context("failed to leave alternate screen")?;
    terminal.show_cursor().context("failed to show cursor")?;
    Ok(())
}

fn restore_terminal_state() {
    let _ = disable_raw_mode();
    let _ = execute!(io::stdout(), LeaveAlternateScreen, Show);
}
