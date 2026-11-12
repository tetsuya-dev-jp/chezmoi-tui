use anyhow::{Context, Result};
use std::fs::{File, OpenOptions};
#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use tracing_subscriber::EnvFilter;
use tracing_subscriber::fmt;
use tracing_subscriber::prelude::*;

#[cfg(target_os = "linux")]
const O_NONBLOCK: i32 = 0x800;
#[cfg(target_os = "macos")]
const O_NONBLOCK: i32 = 0x4;

pub fn init(log_file: Option<&Path>) -> Result<()> {
    let Some(log_file) = log_file else {
        return Ok(());
    };

    if let Some(parent) = log_file.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create log directory {}", parent.display()))?;
    }

    let file = open_log_file(log_file)?;

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

fn open_log_file(path: &Path) -> Result<File> {
    match std::fs::metadata(path) {
        Ok(metadata) if !metadata.is_file() => {
            anyhow::bail!("log file {} is not a regular file", path.display());
        }
        Ok(_) => {}
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => {
            return Err(err)
                .with_context(|| format!("failed to inspect log file {}", path.display()));
        }
    }

    let mut options = OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    options.mode(0o600);
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    options.custom_flags(O_NONBLOCK);
    let file = options
        .open(path)
        .with_context(|| format!("failed to open log file {}", path.display()))?;

    if !file
        .metadata()
        .with_context(|| format!("failed to inspect log file {}", path.display()))?
        .is_file()
    {
        anyhow::bail!("log file {} is not a regular file", path.display());
    }

    #[cfg(unix)]
    file.set_permissions(std::fs::Permissions::from_mode(0o600))
        .with_context(|| format!("failed to restrict log file {}", path.display()))?;

    Ok(file)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn init_without_log_file_is_noop() {
        init(None).expect("init without log file");
    }

    #[cfg(unix)]
    #[test]
    fn created_log_file_is_user_only() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("diagnostics.log");

        let file = open_log_file(&path).expect("create log file");
        drop(file);

        let mode = std::fs::metadata(path)
            .expect("log metadata")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600);
    }

    #[cfg(unix)]
    #[test]
    fn existing_log_file_permissions_are_restricted() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("diagnostics.log");
        std::fs::write(&path, "").expect("create log file");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644))
            .expect("set initial permissions");

        let file = open_log_file(&path).expect("open existing log file");
        let mode = file
            .metadata()
            .expect("opened log metadata")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600);
    }

    #[cfg(unix)]
    #[test]
    fn fifo_log_file_is_rejected_without_blocking() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("diagnostics.fifo");
        let status = std::process::Command::new("mkfifo")
            .arg(&path)
            .status()
            .expect("run mkfifo");
        assert!(status.success());

        let err = open_log_file(&path).expect_err("FIFO must be rejected");
        assert!(format!("{err:#}").contains("not a regular file"));
    }
}
