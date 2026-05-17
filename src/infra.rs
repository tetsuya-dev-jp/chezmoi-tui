use crate::domain::{Action, ActionRequest, ChangeKind, CommandResult, DiffText, StatusEntry};
use anyhow::{Context, Result, bail};
use serde_json::Value;
use std::collections::BTreeSet;
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;

pub trait ChezmoiClient: Send + Sync {
    fn status(&self) -> Result<Vec<StatusEntry>>;
    fn managed(&self) -> Result<Vec<PathBuf>>;
    fn unmanaged(&self) -> Result<Vec<PathBuf>>;
    fn source(&self) -> Result<(PathBuf, Vec<PathBuf>)>;
    fn diff(&self, target: Option<&Path>) -> Result<DiffText>;
    fn run(&self, request: &ActionRequest) -> Result<CommandResult>;
}

#[derive(Debug, Clone)]
pub struct ShellChezmoiClient {
    binary: String,
    home_dir: PathBuf,
    working_dir: PathBuf,
    source_dir: Option<PathBuf>,
}

impl Default for ShellChezmoiClient {
    fn default() -> Self {
        let working_dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let home_dir = dirs::home_dir().unwrap_or_else(|| working_dir.clone());
        Self {
            binary: "chezmoi".to_string(),
            home_dir,
            working_dir,
            source_dir: None,
        }
    }
}

impl ShellChezmoiClient {
    pub fn new(
        binary: impl Into<String>,
        home_dir: PathBuf,
        working_dir: PathBuf,
        source_dir: Option<PathBuf>,
    ) -> Self {
        Self {
            binary: binary.into(),
            home_dir,
            working_dir,
            source_dir,
        }
    }

    fn run_raw<I, S>(&self, args: I, destination_dir: &Path) -> Result<CommandResult>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let args: Vec<OsString> = args
            .into_iter()
            .map(|arg| arg.as_ref().to_os_string())
            .collect();
        let mut cmd = Command::new(&self.binary);
        cmd.arg("--destination").arg(destination_dir);
        if let Some(source_dir) = &self.source_dir {
            cmd.arg("--source").arg(source_dir);
        }
        cmd.args(&args);

        tracing::debug!(
            binary = %self.binary,
            destination = %destination_dir.display(),
            source = self.source_dir.as_ref().map(|path| path.display().to_string()),
            args = ?args,
            "running chezmoi command"
        );
        let started = Instant::now();
        let output = cmd
            .output()
            .with_context(|| format!("failed to execute {} {:?}", self.binary, args))?;
        let duration_ms = elapsed_millis_u64(started);

        let exit_code = output.status.code().unwrap_or(-1);
        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();

        tracing::info!(
            binary = %self.binary,
            args = ?args,
            exit_code,
            duration_ms,
            stderr = %squash_for_log(&stderr),
            "chezmoi command finished"
        );

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
        let result = self.run_raw(["status"], &self.home_dir)?;
        if result.exit_code != 0 {
            bail!("chezmoi status failed: {}", result.stderr.trim());
        }
        parse_status_output(&result.stdout)
    }

    fn managed(&self) -> Result<Vec<PathBuf>> {
        let result = self.run_raw(["managed", "--format", "json"], &self.home_dir)?;
        if result.exit_code != 0 {
            bail!("chezmoi managed failed: {}", result.stderr.trim());
        }
        Ok(parse_managed_output(&result.stdout))
    }

    fn unmanaged(&self) -> Result<Vec<PathBuf>> {
        let use_home_destination = self.working_dir.starts_with(&self.home_dir);
        let destination = if use_home_destination {
            &self.home_dir
        } else {
            &self.working_dir
        };

        let result = self.run_raw(["unmanaged"], destination)?;
        if result.exit_code != 0 {
            bail!("chezmoi unmanaged failed: {}", result.stderr.trim());
        }

        let paths = parse_unmanaged_output(&result.stdout);
        if use_home_destination {
            let mut scoped =
                filter_unmanaged_to_working_dir(paths, &self.home_dir, &self.working_dir);

            if scoped.iter().any(|path| path == Path::new(".")) {
                scoped = self.expand_working_root_entries_from_home(scoped)?;
            }

            Ok(scoped)
        } else {
            Ok(paths)
        }
    }

    fn source(&self) -> Result<(PathBuf, Vec<PathBuf>)> {
        let source_dir = self.source_dir()?;
        let paths = list_source_paths(&source_dir)?;
        Ok((source_dir, paths))
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

impl ShellChezmoiClient {
    fn source_dir(&self) -> Result<PathBuf> {
        if let Some(source_dir) = &self.source_dir {
            return Ok(source_dir.clone());
        }

        let result = self.run_raw(["source-path"], &self.home_dir)?;
        if result.exit_code != 0 {
            bail!("chezmoi source-path failed: {}", result.stderr.trim());
        }
        let source_dir = result.stdout.trim();
        if source_dir.is_empty() {
            bail!("chezmoi source-path returned empty output");
        }
        Ok(PathBuf::from(source_dir))
    }

    fn expand_working_root_entries_from_home(&self, scoped: Vec<PathBuf>) -> Result<Vec<PathBuf>> {
        let mut merged: BTreeSet<PathBuf> = scoped
            .into_iter()
            .filter(|path| path != Path::new("."))
            .collect();

        let mut home_results = Vec::new();
        let read_dir = std::fs::read_dir(&self.working_dir)
            .with_context(|| format!("failed to read {}", self.working_dir.display()))?;
        for entry in read_dir {
            let child = entry
                .with_context(|| format!("failed to read child in {}", self.working_dir.display()))?
                .path();
            let args = vec![os("unmanaged"), os("--"), child.into_os_string()];
            let result = self.run_raw(&args, &self.home_dir)?;
            if result.exit_code != 0 {
                bail!("chezmoi unmanaged failed: {}", result.stderr.trim());
            }
            home_results.extend(parse_unmanaged_output(&result.stdout));
        }

        let expanded =
            filter_unmanaged_to_working_dir(home_results, &self.home_dir, &self.working_dir);
        merged.extend(expanded.into_iter().filter(|path| path != Path::new(".")));

        Ok(merged.into_iter().collect())
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

pub fn parse_managed_output(output: &str) -> Vec<PathBuf> {
    let trimmed = output.trim();
    if trimmed.is_empty() {
        return Vec::new();
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
        return paths;
    }

    trimmed
        .lines()
        .map(|line| PathBuf::from(line.trim()))
        .filter(|path| !path.as_os_str().is_empty())
        .collect()
}

pub fn parse_unmanaged_output(output: &str) -> Vec<PathBuf> {
    output
        .lines()
        .map(|line| PathBuf::from(line.trim()))
        .filter(|path| !path.as_os_str().is_empty())
        .collect()
}

fn list_source_paths(source_dir: &Path) -> Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    collect_source_paths(source_dir, source_dir, &mut out)?;
    out.sort_by(|a, b| a.to_string_lossy().cmp(&b.to_string_lossy()));
    Ok(out)
}

fn collect_source_paths(base: &Path, current: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    let read_dir = std::fs::read_dir(current)
        .with_context(|| format!("failed to read source dir {}", current.display()))?;
    for entry in read_dir {
        let entry =
            entry.with_context(|| format!("failed to read child in {}", current.display()))?;
        let path = entry.path();
        let relative = path
            .strip_prefix(base)
            .unwrap_or(path.as_path())
            .to_path_buf();
        if relative.as_os_str().is_empty() {
            continue;
        }
        out.push(relative);
        let file_type = entry
            .file_type()
            .with_context(|| format!("failed to inspect {}", path.display()))?;
        if file_type.is_dir() && !file_type.is_symlink() {
            collect_source_paths(base, &path, out)?;
        }
    }
    Ok(())
}

fn elapsed_millis_u64(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
}

fn squash_for_log(input: &str) -> String {
    input
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .take(3)
        .collect::<Vec<_>>()
        .join(" | ")
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

    let scoped: BTreeSet<PathBuf> = paths
        .into_iter()
        .filter_map(|path| {
            let relative = path_relative_to_home(path, home_dir)?;
            if relative == working_rel_to_home || working_rel_to_home.starts_with(&relative) {
                return Some(PathBuf::from("."));
            }

            let scoped = relative.strip_prefix(working_rel_to_home).ok()?;
            Some(if scoped.as_os_str().is_empty() {
                PathBuf::from(".")
            } else {
                scoped.to_path_buf()
            })
        })
        .collect();

    scoped.into_iter().collect()
}

fn path_relative_to_home(path: PathBuf, home_dir: &Path) -> Option<PathBuf> {
    if path.is_absolute() {
        path.strip_prefix(home_dir).ok().map(Path::to_path_buf)
    } else {
        Some(path)
    }
}

pub fn action_to_args(request: &ActionRequest) -> Result<Vec<OsString>> {
    let action = request.action;
    let target = request
        .target
        .as_ref()
        .map(|path| path.as_os_str().to_os_string());

    let args = match action {
        Action::Apply => vec![os("apply")],
        Action::Doctor => vec![os("doctor")],
        Action::Data => vec![os("data"), os("--format"), os("json")],
        Action::OpenSourceDir => {
            bail!(
                "open-source-dir is a foreground action and does not map to a chezmoi CLI command"
            )
        }
        Action::Update => vec![os("update")],
        Action::EditConfig => vec![os("edit-config")],
        Action::EditConfigTemplate => vec![os("edit-config-template")],
        Action::EditIgnore => {
            bail!("edit-ignore is an internal action and does not map to a chezmoi CLI command")
        }
        Action::ReAdd => vec![os("re-add"), os("--"), required_target(target, action)?],
        Action::Merge => {
            let mut args = vec![os("merge")];
            if let Some(path) = target {
                args.push(os("--"));
                args.push(path);
            }
            args
        }
        Action::MergeAll => vec![os("merge-all")],
        Action::Add => vec![os("add"), os("--"), required_target(target, action)?],
        Action::Ignore => {
            bail!("ignore is an internal action and does not map to a chezmoi CLI command")
        }
        Action::Edit => vec![os("edit"), os("--"), required_target(target, action)?],
        Action::Forget => vec![
            os("forget"),
            os("--force"),
            os("--no-tty"),
            os("--"),
            required_target(target, action)?,
        ],
        Action::Chattr => vec![
            os("chattr"),
            os("--"),
            request
                .chattr_attrs
                .as_deref()
                .map(OsString::from)
                .context("chattr requires attributes")?,
            required_target(target, action)?,
        ],
        Action::Destroy => vec![os("destroy"), os("--"), required_target(target, action)?],
        Action::Purge => vec![os("purge"), os("--force"), os("--no-tty")],
    };

    Ok(args)
}

fn required_target(target: Option<OsString>, action: Action) -> Result<OsString> {
    target.with_context(|| format!("{} requires target", action.label()))
}

fn diff_args(target: Option<&Path>) -> Vec<OsString> {
    let mut args = vec![
        os("diff"),
        os("--no-pager"),
        os("--use-builtin-diff"),
        os("--color=true"),
    ];
    if let Some(path) = target {
        args.push(os("--"));
        args.push(path.as_os_str().to_os_string());
    }
    args
}

fn os(value: &str) -> OsString {
    OsString::from(value)
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
            parse_managed_output(json),
            vec![PathBuf::from(".zshrc"), PathBuf::from(".gitconfig")]
        );

        let lines = ".zshrc\n.gitconfig\n";
        assert_eq!(
            parse_managed_output(lines),
            vec![PathBuf::from(".zshrc"), PathBuf::from(".gitconfig")]
        );
    }

    #[test]
    fn parse_unmanaged_lines() {
        let output = ".cache/file\n.local/tmp\n";
        assert_eq!(
            parse_unmanaged_output(output),
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
    fn unmanaged_ancestor_path_maps_to_working_root() {
        let paths = vec![PathBuf::from("dev"), PathBuf::from("dev/chezmoi-tui/src")];
        let got = filter_unmanaged_to_working_dir(
            paths,
            Path::new("/home/tetsuya"),
            Path::new("/home/tetsuya/dev/chezmoi-tui"),
        );
        assert_eq!(got, vec![PathBuf::from("."), PathBuf::from("src")]);
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
            vec![os("purge"), os("--force"), os("--no-tty")]
        );

        let doctor = ActionRequest {
            action: Action::Doctor,
            target: None,
            chattr_attrs: None,
        };
        assert_eq!(
            action_to_args(&doctor).expect("doctor args"),
            vec![os("doctor")]
        );

        let data = ActionRequest {
            action: Action::Data,
            target: None,
            chattr_attrs: None,
        };
        assert_eq!(
            action_to_args(&data).expect("data args"),
            vec![os("data"), os("--format"), os("json")]
        );

        let open_source_dir = ActionRequest {
            action: Action::OpenSourceDir,
            target: None,
            chattr_attrs: None,
        };
        assert!(action_to_args(&open_source_dir).is_err());

        let edit = ActionRequest {
            action: Action::Edit,
            target: Some(PathBuf::from(".zshrc")),
            chattr_attrs: None,
        };
        assert_eq!(
            action_to_args(&edit).expect("edit args"),
            vec![os("edit"), os("--"), os(".zshrc")]
        );

        let edit_config = ActionRequest {
            action: Action::EditConfig,
            target: None,
            chattr_attrs: None,
        };
        assert_eq!(
            action_to_args(&edit_config).expect("edit-config args"),
            vec![os("edit-config")]
        );

        let edit_config_template = ActionRequest {
            action: Action::EditConfigTemplate,
            target: None,
            chattr_attrs: None,
        };
        assert_eq!(
            action_to_args(&edit_config_template).expect("edit-config-template args"),
            vec![os("edit-config-template")]
        );

        let forget = ActionRequest {
            action: Action::Forget,
            target: Some(PathBuf::from(".zshrc")),
            chattr_attrs: None,
        };
        assert_eq!(
            action_to_args(&forget).expect("forget args"),
            vec![
                os("forget"),
                os("--force"),
                os("--no-tty"),
                os("--"),
                os(".zshrc"),
            ]
        );

        let chattr = ActionRequest {
            action: Action::Chattr,
            target: Some(PathBuf::from(".zshrc")),
            chattr_attrs: Some("private,template".to_string()),
        };
        assert_eq!(
            action_to_args(&chattr).expect("chattr args"),
            vec![os("chattr"), os("--"), os("private,template"), os(".zshrc")]
        );

        let readd = ActionRequest {
            action: Action::ReAdd,
            target: Some(PathBuf::from(".zshrc")),
            chattr_attrs: None,
        };
        assert_eq!(
            action_to_args(&readd).expect("re-add args"),
            vec![os("re-add"), os("--"), os(".zshrc")]
        );

        let ignore = ActionRequest {
            action: Action::Ignore,
            target: Some(PathBuf::from(".cache")),
            chattr_attrs: None,
        };
        assert!(action_to_args(&ignore).is_err());

        let edit_ignore = ActionRequest {
            action: Action::EditIgnore,
            target: None,
            chattr_attrs: None,
        };
        assert!(action_to_args(&edit_ignore).is_err());
    }

    #[test]
    fn diff_target_args_are_option_safe() {
        let got = diff_args(Some(Path::new("-n")));
        assert_eq!(
            got,
            vec![
                os("diff"),
                os("--no-pager"),
                os("--use-builtin-diff"),
                os("--color=true"),
                os("--"),
                os("-n")
            ]
        );
    }

    #[test]
    fn diff_args_force_builtin_colorized_diff_without_target() {
        let got = diff_args(None);
        assert_eq!(
            got,
            vec![
                os("diff"),
                os("--no-pager"),
                os("--use-builtin-diff"),
                os("--color=true")
            ]
        );
    }

    #[test]
    fn default_client_uses_current_dir_for_working_destination() {
        let client = ShellChezmoiClient::default();
        assert_eq!(
            client.working_dir,
            std::env::current_dir().expect("current dir")
        );
    }

    #[cfg(unix)]
    #[test]
    fn shell_client_passes_destination_and_source_to_chezmoi() {
        use std::os::unix::fs::PermissionsExt;
        use std::time::{SystemTime, UNIX_EPOCH};

        let root = std::env::temp_dir().join(format!(
            "chezmoi_tui_fake_{}_{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("time")
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).expect("create root");
        let log = root.join("args.log");
        let fake = root.join("chezmoi");
        std::fs::write(
            &fake,
            format!(
                r#"#!/bin/sh
printf '%s\n' "$@" > '{}'
printf ' A .zshrc\n'
"#,
                log.display()
            ),
        )
        .expect("write fake chezmoi");
        let mut perms = std::fs::metadata(&fake).expect("metadata").permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&fake, perms).expect("chmod");

        let client = ShellChezmoiClient::new(
            fake.display().to_string(),
            root.join("home"),
            root.join("work"),
            Some(root.join("source")),
        );

        let status = client.status().expect("status");
        assert_eq!(status.len(), 1);
        let args = std::fs::read_to_string(&log).expect("read log");
        assert!(args.contains("--destination\n"));
        assert!(args.contains("home\n"));
        assert!(args.contains("--source\n"));
        assert!(args.contains("source\n"));
        assert!(args.contains("status\n"));

        let _ = std::fs::remove_dir_all(root);
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
