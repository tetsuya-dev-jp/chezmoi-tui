use crate::actions::send_task;
use crate::app::{App, BackendTask, DetailKind};
use crate::domain::ListView;
use anyhow::{Context, Result};
use std::fmt::Write as _;
use std::fs::{self, File};
use std::io::{ErrorKind, Read};
use std::path::Path;
use tokio::sync::mpsc::UnboundedSender;

const PREVIEW_MAX_BYTES: usize = 64 * 1024;
const PREVIEW_BINARY_SAMPLE_BYTES: usize = 4096;

pub(crate) fn load_file_preview(path: &Path) -> Result<String> {
    let metadata = fs::symlink_metadata(path)
        .with_context(|| format!("preview target metadata failed: {}", path.display()))?;
    let kind = metadata.file_type();

    if kind.is_dir() {
        return Ok("This is a directory. Expand it and select a file inside.".to_string());
    }
    if kind.is_symlink() {
        match fs::metadata(path) {
            Ok(target) if target.is_dir() => {
                return Ok(
                    "This is a directory symlink. Directory links are shown but not expanded by default."
                        .to_string(),
                );
            }
            Ok(_) => {}
            Err(err) if err.kind() == ErrorKind::NotFound => {
                return Ok("Cannot preview broken symlink.".to_string());
            }
            Err(err) => {
                return Err(err).with_context(|| {
                    format!("failed to inspect symlink target: {}", path.display())
                });
            }
        }
    }

    let file = File::open(path).with_context(|| format!("failed to read: {}", path.display()))?;
    let mut bytes = Vec::with_capacity(PREVIEW_MAX_BYTES + 1);
    file.take((PREVIEW_MAX_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .with_context(|| format!("failed to read: {}", path.display()))?;

    let sample_len = bytes.len().min(PREVIEW_BINARY_SAMPLE_BYTES);
    if bytes[..sample_len].contains(&0) {
        return Ok("Cannot preview binary file.".to_string());
    }

    let is_truncated = bytes.len() > PREVIEW_MAX_BYTES;
    if is_truncated {
        bytes.truncate(PREVIEW_MAX_BYTES);
    }

    let mut text = String::from_utf8_lossy(&bytes).to_string();
    if is_truncated {
        let _ = write!(
            text,
            "\n\n--- preview truncated at {} bytes (file size: {} bytes) ---",
            PREVIEW_MAX_BYTES,
            metadata.len()
        );
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

pub(crate) fn maybe_enqueue_auto_detail(
    app: &mut App,
    task_tx: &UnboundedSender<BackendTask>,
) -> Result<()> {
    maybe_enqueue_status_diff(app, task_tx)?;
    maybe_enqueue_managed_preview(app, task_tx)?;
    maybe_enqueue_unmanaged_preview(app, task_tx)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

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

    #[cfg(unix)]
    #[test]
    fn preview_reports_directory_symlink() {
        use std::os::unix::fs::symlink;

        let root = std::env::temp_dir().join(format!(
            "chezmoi_tui_preview_symlink_dir_{}_{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("time")
                .as_nanos()
        ));
        let real_dir = root.join("real");
        std::fs::create_dir_all(&real_dir).expect("create real dir");
        std::fs::write(real_dir.join("inside.txt"), "inside").expect("write file");
        let link = root.join("linkdir");
        symlink(&real_dir, &link).expect("create symlink");

        let got = load_file_preview(&link).expect("preview");
        assert!(got.contains("directory symlink"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    #[test]
    fn preview_reports_broken_symlink() {
        use std::os::unix::fs::symlink;

        let root = std::env::temp_dir().join(format!(
            "chezmoi_tui_preview_broken_symlink_{}_{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("time")
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).expect("create root");
        let link = root.join("broken");
        symlink(root.join("missing.txt"), &link).expect("create broken symlink");

        let got = load_file_preview(&link).expect("preview");
        assert!(got.contains("broken symlink"));
        let _ = std::fs::remove_dir_all(root);
    }
}
