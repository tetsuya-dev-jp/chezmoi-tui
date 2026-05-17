use anyhow::{Context, Result};
use std::fs::OpenOptions;
use std::path::Path;
use tracing_subscriber::EnvFilter;
use tracing_subscriber::fmt;
use tracing_subscriber::prelude::*;

pub(crate) fn init(log_file: Option<&Path>) -> Result<()> {
    let Some(log_file) = log_file else {
        return Ok(());
    };

    if let Some(parent) = log_file.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create log directory {}", parent.display()))?;
    }

    let file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_file)
        .with_context(|| format!("failed to open log file {}", log_file.display()))?;

    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let layer = fmt::layer()
        .with_writer(file)
        .with_ansi(false)
        .with_target(true)
        .with_thread_ids(false)
        .with_thread_names(false);

    tracing_subscriber::registry()
        .with(filter)
        .with(layer)
        .try_init()
        .context("failed to initialize diagnostics")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn init_without_log_file_is_noop() {
        init(None).expect("init without log file");
    }
}
