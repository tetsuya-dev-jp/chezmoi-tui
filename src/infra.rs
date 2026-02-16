use crate::domain::{Action, ActionRequest, ChangeKind, CommandResult, DiffText, StatusEntry};
use anyhow::{Context, Result, bail};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;

pub trait ChezmoiClient: Send + Sync {
    fn status(&self) -> Result<Vec<StatusEntry>>;
    fn managed(&self) -> Result<Vec<PathBuf>>;
    fn unmanaged(&self) -> Result<Vec<PathBuf>>;
    fn diff(&self, target: Option<&Path>) -> Result<DiffText>;
    fn run(&self, request: &ActionRequest) -> Result<CommandResult>;
}

#[derive(Debug, Clone)]
pub struct ShellChezmoiClient {
    binary: String,
    home_dir: PathBuf,
    working_dir: PathBuf,
}

impl Default for ShellChezmoiClient {
    fn default() -> Self {
        let working_dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let home_dir = dirs::home_dir().unwrap_or_else(|| working_dir.clone());
        Self {
            binary: "chezmoi".to_string(),
            home_dir,
            working_dir,
        }
    }
}

impl ShellChezmoiClient {
    fn run_raw(&self, args: &[String], destination_dir: &Path) -> Result<CommandResult> {
        let mut cmd = Command::new(&self.binary);
        cmd.arg("--destination").arg(destination_dir);
        cmd.args(args);

        let started = Instant::now();
        let output = cmd
            .output()
            .with_context(|| format!("failed to execute {} {:?}", self.binary, args))?;
        let duration_ms = started.elapsed().as_millis() as u64;

        let exit_code = output.status.code().unwrap_or(-1);
        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();

        Ok(CommandResult {
            exit_code,
            stdout,
            stderr,
            duration_ms,
        })
    }

    fn destination_for_target(&self, target: Option<&Path>) -> &Path {
        match target {
            Some(path) if path.is_absolute() => {
                if path.starts_with(&self.home_dir) {
                    &self.home_dir
                } else if path.starts_with(&self.working_dir) {
                    &self.working_dir
                } else {
                    &self.home_dir
                }
            }
            Some(_) => &self.working_dir,
            None => &self.home_dir,
        }
    }
}

impl ChezmoiClient for ShellChezmoiClient {
    fn status(&self) -> Result<Vec<StatusEntry>> {
        let result = self.run_raw(&["status".to_string()], &self.home_dir)?;
        if result.exit_code != 0 {
            bail!("chezmoi status failed: {}", result.stderr.trim());
        }
        parse_status_output(&result.stdout)
    }

    fn managed(&self) -> Result<Vec<PathBuf>> {
        let result = self.run_raw(
            &[
                "managed".to_string(),
                "--format".to_string(),
                "json".to_string(),
            ],
            &self.home_dir,
        )?;
        if result.exit_code != 0 {
            bail!("chezmoi managed failed: {}", result.stderr.trim());
        }
        parse_managed_output(&result.stdout)
    }

    fn unmanaged(&self) -> Result<Vec<PathBuf>> {
        let use_home_destination = self.working_dir.starts_with(&self.home_dir);
        let destination = if use_home_destination {
            &self.home_dir
        } else {
            &self.working_dir
        };

        let result = self.run_raw(&["unmanaged".to_string()], destination)?;
        if result.exit_code != 0 {
            bail!("chezmoi unmanaged failed: {}", result.stderr.trim());
        }

        let paths = parse_unmanaged_output(&result.stdout)?;
        if use_home_destination {
            Ok(filter_unmanaged_to_working_dir(
                paths,
                &self.home_dir,
                &self.working_dir,
            ))
        } else {
            Ok(paths)
        }
    }

    fn diff(&self, target: Option<&Path>) -> Result<DiffText> {
        let args = diff_args(target);
        let destination = self.destination_for_target(target);

        let result = self.run_raw(&args, destination)?;
        if result.exit_code != 0 {
            // chezmoi diff returns 0 even when differences exist; non-zero means execution error.
            bail!("chezmoi diff failed: {}", result.stderr.trim());
        }

        Ok(DiffText {
            text: result.stdout,
        })
    }

    fn run(&self, request: &ActionRequest) -> Result<CommandResult> {
        let args = action_to_args(request)?;
        let destination = self.destination_for_target(request.target.as_deref());
        self.run_raw(&args, destination)
    }
}

pub fn parse_status_output(output: &str) -> Result<Vec<StatusEntry>> {
    let mut entries = Vec::new();

    for (idx, raw) in output.lines().enumerate() {
        if raw.trim().is_empty() {
            continue;
        }

        let chars: Vec<char> = raw.chars().collect();
        if chars.len() < 4 {
            bail!("invalid status line {}: {:?}", idx + 1, raw);
        }

        let first = chars[0];
        let second = chars[1];
        let path = chars[3..].iter().collect::<String>();

        entries.push(StatusEntry {
            path: PathBuf::from(path),
            actual_vs_state: ChangeKind::from_status_char(first),
            actual_vs_target: ChangeKind::from_status_char(second),
        });
    }

    Ok(entries)
}

pub fn parse_managed_output(output: &str) -> Result<Vec<PathBuf>> {
    let trimmed = output.trim();
    if trimmed.is_empty() {
        return Ok(Vec::new());
    }

    if let Ok(json) = serde_json::from_str::<Value>(trimmed)
        && let Some(array) = json.as_array()
    {
        let mut paths = Vec::with_capacity(array.len());
        for item in array {
            if let Some(path) = item.as_str() {
                paths.push(PathBuf::from(path));
            }
        }
        return Ok(paths);
    }

    Ok(trimmed
        .lines()
        .map(|line| PathBuf::from(line.trim()))
        .filter(|path| !path.as_os_str().is_empty())
        .collect())
}

pub fn parse_unmanaged_output(output: &str) -> Result<Vec<PathBuf>> {
    Ok(output
        .lines()
        .map(|line| PathBuf::from(line.trim()))
        .filter(|path| !path.as_os_str().is_empty())
        .collect())
}

fn filter_unmanaged_to_working_dir(
    paths: Vec<PathBuf>,
    home_dir: &Path,
    working_dir: &Path,
) -> Vec<PathBuf> {
    if working_dir == home_dir {
        return paths
            .into_iter()
            .filter_map(|path| path_relative_to_home(path, home_dir))
            .collect();
    }

    let Ok(working_rel_to_home) = working_dir.strip_prefix(home_dir) else {
        return paths;
    };

    paths
        .into_iter()
        .filter_map(|path| {
            let relative = path_relative_to_home(path, home_dir)?;
            let scoped = relative.strip_prefix(working_rel_to_home).ok()?;
            if scoped.as_os_str().is_empty() {
                None
            } else {
                Some(scoped.to_path_buf())
            }
        })
        .collect()
}

fn path_relative_to_home(path: PathBuf, home_dir: &Path) -> Option<PathBuf> {
    if path.is_absolute() {
        path.strip_prefix(home_dir).ok().map(Path::to_path_buf)
    } else {
        Some(path)
    }
}

pub fn action_to_args(request: &ActionRequest) -> Result<Vec<String>> {
    let action = request.action;
    let target = request.target.as_ref().map(|p| p.display().to_string());

    let args = match action {
        Action::Apply => vec!["apply".to_string()],
        Action::Update => vec!["update".to_string()],
        Action::ReAdd => vec!["re-add".to_string()],
        Action::Merge => {
            let mut args = vec!["merge".to_string()];
            if let Some(path) = target {
                args.push("--".to_string());
                args.push(path);
            }
            args
        }
        Action::MergeAll => vec!["merge-all".to_string()],
        Action::Add => vec![
            "add".to_string(),
            "--".to_string(),
            required_target(target, action)?,
        ],
        Action::Edit => vec![
            "edit".to_string(),
            "--".to_string(),
            required_target(target, action)?,
        ],
        Action::Forget => vec![
            "forget".to_string(),
            "--force".to_string(),
            "--no-tty".to_string(),
            "--".to_string(),
            required_target(target, action)?,
        ],
        Action::Chattr => vec![
            "chattr".to_string(),
            "--".to_string(),
            request
                .chattr_attrs
                .clone()
                .context("chattr requires attributes")?,
            required_target(target, action)?,
        ],
        Action::Destroy => vec![
            "destroy".to_string(),
            "--".to_string(),
            required_target(target, action)?,
        ],
        Action::Purge => vec![
            "purge".to_string(),
            "--force".to_string(),
            "--no-tty".to_string(),
        ],
    };

    Ok(args)
}

fn required_target(target: Option<String>, action: Action) -> Result<String> {
    target.with_context(|| format!("{} requires target", action.label()))
}

fn diff_args(target: Option<&Path>) -> Vec<String> {
    let mut args = vec!["diff".to_string()];
    if let Some(path) = target {
        args.push("--".to_string());
        args.push(path.display().to_string());
    }
    args
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn parse_status_roundtrip() {
        let raw = " A .zshrc\nM  .gitconfig\nDR .local/bin/script\n";
        let entries = parse_status_output(raw).expect("should parse");
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].actual_vs_state, ChangeKind::None);
        assert_eq!(entries[0].actual_vs_target, ChangeKind::Added);
        assert_eq!(entries[0].path, PathBuf::from(".zshrc"));
        assert_eq!(entries[2].actual_vs_state, ChangeKind::Deleted);
        assert_eq!(entries[2].actual_vs_target, ChangeKind::Run);
    }

    #[test]
    fn parse_managed_json_and_lines() {
        let json = r#"[".zshrc", ".gitconfig"]"#;
        assert_eq!(
            parse_managed_output(json).expect("json parse"),
            vec![PathBuf::from(".zshrc"), PathBuf::from(".gitconfig")]
        );

        let lines = ".zshrc\n.gitconfig\n";
        assert_eq!(
            parse_managed_output(lines).expect("line parse"),
            vec![PathBuf::from(".zshrc"), PathBuf::from(".gitconfig")]
        );
    }

    #[test]
    fn parse_unmanaged_lines() {
        let output = ".cache/file\n.local/tmp\n";
        assert_eq!(
            parse_unmanaged_output(output).expect("line parse"),
            vec![PathBuf::from(".cache/file"), PathBuf::from(".local/tmp")]
        );
    }

    #[test]
    fn unmanaged_paths_are_scoped_to_working_dir_when_destination_is_home() {
        let paths = vec![
            PathBuf::from(".agents"),
            PathBuf::from("dev/chezmoi-tui/.git"),
            PathBuf::from("dev/chezmoi-tui/src"),
            PathBuf::from("dev/other-project/file"),
        ];
        let got = filter_unmanaged_to_working_dir(
            paths,
            Path::new("/home/tetsuya"),
            Path::new("/home/tetsuya/dev/chezmoi-tui"),
        );
        assert_eq!(got, vec![PathBuf::from(".git"), PathBuf::from("src")]);
    }

    #[test]
    fn unmanaged_paths_keep_home_relative_when_working_dir_is_home() {
        let paths = vec![
            PathBuf::from("/home/tetsuya/.cache"),
            PathBuf::from(".local/share"),
        ];
        let got = filter_unmanaged_to_working_dir(
            paths,
            Path::new("/home/tetsuya"),
            Path::new("/home/tetsuya"),
        );
        assert_eq!(
            got,
            vec![PathBuf::from(".cache"), PathBuf::from(".local/share")]
        );
    }

    #[test]
    fn action_mapping_includes_danger_and_chattr() {
        let purge = ActionRequest {
            action: Action::Purge,
            target: None,
            chattr_attrs: None,
        };
        assert_eq!(
            action_to_args(&purge).expect("purge args"),
            vec!["purge", "--force", "--no-tty"]
        );

        let edit = ActionRequest {
            action: Action::Edit,
            target: Some(PathBuf::from(".zshrc")),
            chattr_attrs: None,
        };
        assert_eq!(
            action_to_args(&edit).expect("edit args"),
            vec!["edit", "--", ".zshrc"]
        );

        let forget = ActionRequest {
            action: Action::Forget,
            target: Some(PathBuf::from(".zshrc")),
            chattr_attrs: None,
        };
        assert_eq!(
            action_to_args(&forget).expect("forget args"),
            vec!["forget", "--force", "--no-tty", "--", ".zshrc"]
        );

        let chattr = ActionRequest {
            action: Action::Chattr,
            target: Some(PathBuf::from(".zshrc")),
            chattr_attrs: Some("private,template".to_string()),
        };
        assert_eq!(
            action_to_args(&chattr).expect("chattr args"),
            vec!["chattr", "--", "private,template", ".zshrc"]
        );
    }

    #[test]
    fn diff_target_args_are_option_safe() {
        let got = diff_args(Some(Path::new("-n")));
        assert_eq!(got, vec!["diff", "--", "-n"]);
    }

    #[test]
    fn default_client_uses_current_dir_for_working_destination() {
        let client = ShellChezmoiClient::default();
        assert_eq!(
            client.working_dir,
            std::env::current_dir().expect("current dir")
        );
    }

    #[test]
    fn destination_for_target_prefers_home_for_home_paths() {
        let client = ShellChezmoiClient {
            home_dir: PathBuf::from("/tmp/home"),
            working_dir: PathBuf::from("/tmp/work"),
            ..ShellChezmoiClient::default()
        };

        let got = client.destination_for_target(Some(Path::new("/tmp/home/.zshrc")));
        assert_eq!(got, Path::new("/tmp/home"));
    }
}
