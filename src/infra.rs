use crate::domain::{Action, ActionRequest, ChangeKind, CommandResult, DiffText, StatusEntry};
use anyhow::{Context, Result, bail};
use serde_json::Value;
use std::collections::BTreeSet;
use std::ffi::{OsStr, OsString};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

pub trait ChezmoiClient: Send + Sync {
    fn status(&self) -> Result<Vec<StatusEntry>>;
    fn managed(&self) -> Result<Vec<PathBuf>>;
    fn unmanaged(&self) -> Result<Vec<PathBuf>>;
    /// Rendered `.chezmoiignore` lines (the file may be a template).
    fn ignore_patterns(&self) -> Result<Vec<String>>;
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
        self.run_raw_with_input(args, destination_dir, None, CommandLimits::default())
    }

    fn run_raw_with_input<I, S>(
        &self,
        args: I,
        destination_dir: &Path,
        input: Option<&str>,
        limits: CommandLimits,
    ) -> Result<CommandResult>
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

        if let Some(input) = input {
            // Prepare stdin before spawning so writes cannot block on a child that
            // is producing output or leave a running child behind on write errors.
            let mut stdin = tempfile::tempfile().context("failed to create stdin temp file")?;
            stdin
                .write_all(input.as_bytes())
                .context("failed to write template stdin")?;
            stdin.seek(SeekFrom::Start(0))?;
            cmd.stdin(Stdio::from(stdin));
        }

        let result = run_command_with_limits(cmd, &args, limits)?;

        tracing::info!(
            binary = %self.binary,
            args = ?args,
            exit_code = result.exit_code,
            duration_ms = result.duration_ms,
            timed_out = result.timed_out,
            output_limited = result.output_limited,
            stdout_truncated = result.stdout_truncated,
            stderr_truncated = result.stderr_truncated,
            stderr = %squash_for_log(&result.stderr),
            "chezmoi command finished"
        );

        Ok(result)
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

const DEFAULT_COMMAND_TIMEOUT: Duration = Duration::from_secs(120);
const DEFAULT_MAX_STDOUT_BYTES: usize = 4 * 1024 * 1024;
const DEFAULT_MAX_STDERR_BYTES: usize = 512 * 1024;

#[derive(Debug, Clone, Copy)]
struct CommandLimits {
    timeout: Duration,
    max_stdout_bytes: usize,
    max_stderr_bytes: usize,
}

impl Default for CommandLimits {
    fn default() -> Self {
        Self {
            timeout: DEFAULT_COMMAND_TIMEOUT,
            max_stdout_bytes: DEFAULT_MAX_STDOUT_BYTES,
            max_stderr_bytes: DEFAULT_MAX_STDERR_BYTES,
        }
    }
}

struct LimitedText {
    text: String,
    truncated: bool,
}

fn append_marker(text: &mut String, marker: &str) {
    if !text.ends_with('\n') && !text.is_empty() {
        text.push('\n');
    }
    text.push_str(marker);
    text.push('\n');
}

fn read_temp_file_limited(mut file: File, max_bytes: usize) -> Result<LimitedText> {
    file.seek(SeekFrom::Start(0))?;

    let mut limited = file.take((max_bytes + 1) as u64);
    let mut bytes = Vec::new();
    limited.read_to_end(&mut bytes)?;

    let truncated = bytes.len() > max_bytes;
    if truncated {
        bytes.truncate(max_bytes);
    }

    let mut text = String::from_utf8_lossy(&bytes).to_string();

    if truncated {
        text.push_str(&format!(
            "\n--- output truncated at {max_bytes} bytes ---\n"
        ));
    }

    Ok(LimitedText { text, truncated })
}

fn run_command_with_limits(
    mut cmd: Command,
    args_for_log: &[OsString],
    limits: CommandLimits,
) -> Result<CommandResult> {
    let stdout_file = tempfile::tempfile().with_context(|| "failed to create stdout temp file")?;
    let stderr_file = tempfile::tempfile().with_context(|| "failed to create stderr temp file")?;

    let stdout_for_child = stdout_file
        .try_clone()
        .with_context(|| "failed to clone stdout temp file")?;
    let stderr_for_child = stderr_file
        .try_clone()
        .with_context(|| "failed to clone stderr temp file")?;

    let started = Instant::now();

    // Keep template subprocesses in their own group so a limit also stops
    // commands launched by template functions, not just the renderer itself.
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }

    let mut child = cmd
        .stdout(Stdio::from(stdout_for_child))
        .stderr(Stdio::from(stderr_for_child))
        .spawn()
        .with_context(|| format!("failed to spawn command {args_for_log:?}"))?;

    let deadline = started + limits.timeout;
    let mut timed_out = false;
    let mut output_limited = false;

    loop {
        let stdout_len = stdout_file.metadata()?.len();
        let stderr_len = stderr_file.metadata()?.len();
        if stdout_len > limits.max_stdout_bytes as u64
            || stderr_len > limits.max_stderr_bytes as u64
        {
            output_limited = true;
            kill_command_tree(&mut child);
            let status = child
                .wait()
                .with_context(|| "failed to wait after output limit kill")?;
            let duration_ms = elapsed_millis_u64(started);

            let stdout_read = read_temp_file_limited(stdout_file, limits.max_stdout_bytes)?;
            let mut stderr_read = read_temp_file_limited(stderr_file, limits.max_stderr_bytes)?;
            append_marker(
                &mut stderr_read.text,
                "--- command output limit exceeded and was killed ---",
            );

            return Ok(CommandResult {
                exit_code: status.code().unwrap_or(-1),
                stdout: stdout_read.text,
                stderr: stderr_read.text,
                duration_ms,
                timed_out,
                output_limited,
                stdout_truncated: stdout_read.truncated,
                stderr_truncated: stderr_read.truncated,
            });
        }

        if let Some(status) = child.try_wait().with_context(|| "failed to poll child")? {
            let duration_ms = elapsed_millis_u64(started);
            let exit_code = status.code().unwrap_or(-1);

            let stdout_read = read_temp_file_limited(stdout_file, limits.max_stdout_bytes)?;
            let stderr_read = read_temp_file_limited(stderr_file, limits.max_stderr_bytes)?;

            return Ok(CommandResult {
                exit_code,
                stdout: stdout_read.text,
                stderr: stderr_read.text,
                duration_ms,
                timed_out,
                output_limited,
                stdout_truncated: stdout_read.truncated,
                stderr_truncated: stderr_read.truncated,
            });
        }

        if Instant::now() >= deadline {
            timed_out = true;
            kill_command_tree(&mut child);
            let status = child
                .wait()
                .with_context(|| "failed to wait after killing child")?;
            let duration_ms = elapsed_millis_u64(started);

            let stdout_read = read_temp_file_limited(stdout_file, limits.max_stdout_bytes)?;
            let mut stderr_read = read_temp_file_limited(stderr_file, limits.max_stderr_bytes)?;
            append_marker(
                &mut stderr_read.text,
                "--- command timed out and was killed ---",
            );

            return Ok(CommandResult {
                exit_code: status.code().unwrap_or(-1),
                stdout: stdout_read.text,
                stderr: stderr_read.text,
                duration_ms,
                timed_out,
                output_limited,
                stdout_truncated: stdout_read.truncated,
                stderr_truncated: stderr_read.truncated,
            });
        }

        std::thread::sleep(Duration::from_millis(50));
    }
}

fn kill_command_tree(child: &mut std::process::Child) {
    #[cfg(unix)]
    {
        unsafe extern "C" {
            fn kill(pid: std::os::raw::c_int, signal: std::os::raw::c_int) -> std::os::raw::c_int;
        }
        if let Ok(pid) = i32::try_from(child.id()) {
            // SAFETY: the child was spawned in its own POSIX process group,
            // negative pid targets that group, and SIGKILL is 9 on POSIX systems.
            unsafe {
                kill(-pid, 9);
            }
        }
    }
    // Also handles non-Unix platforms and a group kill that failed.
    let _ = child.kill();
}

fn collect_ignore_files(
    source_dir: &Path,
    active_directories: Vec<PathBuf>,
) -> Result<Vec<PathBuf>> {
    let directories: BTreeSet<_> = active_directories
        .into_iter()
        .chain(std::iter::once(source_dir.to_path_buf()))
        .collect();
    let mut files = Vec::new();
    for directory in directories {
        if !directory.starts_with(source_dir) {
            bail!(
                "managed source directory is outside source root: {}",
                directory.display()
            );
        }
        let metadata = if directory == source_dir {
            std::fs::metadata(&directory)
        } else {
            std::fs::symlink_metadata(&directory)
        };
        match metadata {
            Ok(metadata) if metadata.file_type().is_dir() => {}
            Ok(_) => continue,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => continue,
            Err(err) => {
                return Err(err)
                    .with_context(|| format!("failed to inspect {}", directory.display()));
            }
        }
        for name in [".chezmoiignore", ".chezmoiignore.tmpl"] {
            let path = directory.join(name);
            if path.try_exists()? {
                files.push(path);
            }
        }
    }
    // Both names are loaded by chezmoi, not one selected in preference to the other.
    files.sort();
    Ok(files)
}

fn ensure_complete_success(result: &CommandResult, command: &str) -> Result<()> {
    if result.timed_out {
        bail!("{command} timed out");
    }
    if result.output_limited || result.stdout_truncated || result.stderr_truncated {
        bail!("{command} output was truncated or limited");
    }
    if result.exit_code != 0 {
        bail!("{command} failed: {}", result.stderr.trim());
    }
    Ok(())
}

impl ChezmoiClient for ShellChezmoiClient {
    fn status(&self) -> Result<Vec<StatusEntry>> {
        let result = self.run_raw(["status"], &self.home_dir)?;
        ensure_complete_success(&result, "chezmoi status")?;
        parse_status_output(&result.stdout)
    }

    fn managed(&self) -> Result<Vec<PathBuf>> {
        let result = self.run_raw(["managed", "--format", "json"], &self.home_dir)?;
        ensure_complete_success(&result, "chezmoi managed")?;
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

    fn ignore_patterns(&self) -> Result<Vec<String>> {
        let source_dir = self.source_dir()?;
        // Let chezmoi decide which source directories are active. A raw walk
        // would render templates under ignored or external subtrees that
        // chezmoi deliberately skips, disabling otherwise-valid filtering.
        let result = self.run_raw(
            [
                "managed",
                "--include=dirs",
                "--exclude=externals",
                "--path-style=source-absolute",
            ],
            &self.home_dir,
        )?;
        ensure_complete_success(&result, "chezmoi managed source directories")?;
        let active_directories = parse_managed_output(&result.stdout);
        let mut patterns = Vec::new();
        for ignore_file in collect_ignore_files(&source_dir, active_directories)? {
            let template = std::fs::read_to_string(&ignore_file)
                .with_context(|| format!("failed to read {}", ignore_file.display()))?;
            let rendered = self.execute_template(&template)?;
            let parent = ignore_file.parent().context("ignore file has no parent")?;
            if parent == source_dir {
                patterns.extend(rendered.lines().map(str::to_owned));
                continue;
            }

            // Ask chezmoi to decode source attributes instead of maintaining a
            // second decoder for private_, exact_, dot_, literal_, etc.
            let result = self.run_raw(
                [
                    OsStr::new("target-path"),
                    OsStr::new("--"),
                    parent.as_os_str(),
                ],
                &self.home_dir,
            )?;
            ensure_complete_success(&result, "chezmoi target-path")?;
            let target = PathBuf::from(result.stdout.trim());
            let relative = target.strip_prefix(&self.home_dir).with_context(|| {
                format!(
                    "ignore file target {} is outside destination",
                    target.display()
                )
            })?;
            let prefix = relative.to_string_lossy().replace('\\', "/");
            for line in rendered.lines() {
                let line = line.trim();
                if line.is_empty() || line.starts_with('#') {
                    patterns.push(line.to_owned());
                } else if let Some(pattern) = line.strip_prefix('!') {
                    patterns.push(format!("!{prefix}/{pattern}"));
                } else {
                    patterns.push(format!("{prefix}/{line}"));
                }
            }
        }
        Ok(patterns)
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
        // --source is a working tree, not necessarily the effective source root
        // when .chezmoiroot is present. Let chezmoi resolve it in both cases.
        let result = self.run_raw(["source-path"], &self.home_dir)?;
        ensure_complete_success(&result, "chezmoi source-path")?;
        let source_dir = result.stdout.trim();
        if source_dir.is_empty() {
            bail!("chezmoi source-path returned empty output");
        }
        Ok(PathBuf::from(source_dir))
    }

    /// Render an ignore template through the same bounded runner as other commands.
    fn execute_template(&self, template: &str) -> Result<String> {
        let result = self.run_raw_with_input(
            ["execute-template"],
            &self.home_dir,
            Some(template),
            CommandLimits::default(),
        )?;
        ensure_complete_success(&result, "chezmoi execute-template")?;
        Ok(result.stdout)
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
        Action::Apply => {
            let mut args = vec![os("apply")];
            if let Some(path) = target {
                args.push(os("--"));
                args.push(path);
            }
            args
        }
        Action::Doctor => vec![os("doctor")],
        Action::Data => vec![os("data"), os("--format"), os("json")],
        Action::OpenSourceDir => {
            bail!(
                "open-source-dir is a foreground action and does not map to a chezmoi CLI command"
            )
        }
        Action::ExternalDiff => {
            bail!(
                "external-diff is a foreground action and does not map to a direct chezmoi CLI command"
            )
        }
        Action::DebugContext => {
            bail!("debug-context is an internal action and does not map to a chezmoi CLI command")
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

        let apply = ActionRequest {
            action: Action::Apply,
            target: Some(PathBuf::from(".zshrc")),
            chattr_attrs: None,
        };
        assert_eq!(
            action_to_args(&apply).expect("apply args"),
            vec![os("apply"), os("--"), os(".zshrc")]
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

        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().to_path_buf();
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

    #[test]
    fn read_temp_file_limited_truncates_large_output() {
        let mut file = tempfile::tempfile().expect("temp file");
        std::io::Write::write_all(&mut file, "a".repeat(100).as_bytes()).expect("write");
        let result = read_temp_file_limited(file, 10).expect("read limited");

        assert!(result.truncated);
        assert!(result.text.contains("output truncated"));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn template_output_limit_terminates_children_and_reaps_renderer() {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempfile::tempdir().expect("tempdir");
        let binary = temp.path().join("fake-chezmoi");
        let pids = temp.path().join("pids");
        let script = format!(
            "#!/bin/sh\nsleep 30 &\nprintf '%s %s' \"$$\" \"$!\" > '{}'\nprintf 'excess output'\nwait\n",
            pids.display()
        );
        std::fs::write(&binary, script).expect("write renderer");
        std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o755))
            .expect("make executable");
        let client = ShellChezmoiClient::new(
            binary.to_str().expect("binary path").to_owned(),
            temp.path().to_path_buf(),
            temp.path().to_path_buf(),
            Some(temp.path().to_path_buf()),
        );
        let result = client
            .run_raw_with_input(
                ["execute-template"],
                temp.path(),
                Some("template input"),
                CommandLimits {
                    timeout: Duration::from_secs(1),
                    max_stdout_bytes: 4,
                    max_stderr_bytes: 64,
                },
            )
            .expect("bounded renderer");
        let ids = std::fs::read_to_string(pids).expect("renderer PIDs");
        let ids: Vec<_> = ids.split_whitespace().collect();
        let renderer_reaped = !PathBuf::from(format!("/proc/{}", ids[0])).exists();
        // SIGKILL delivery to descendants is asynchronous; the renderer wait
        // reaps only the direct child, so allow the worker time to stop.
        let deadline = Instant::now() + Duration::from_secs(1);
        let worker_stopped = loop {
            let state = std::fs::read_to_string(format!("/proc/{}/stat", ids[1]));
            let stopped = match state {
                Err(err)
                    if err.kind() == std::io::ErrorKind::NotFound
                        // Linux procfs can report ESRCH when a process exits
                        // between opening its stat file and reading it.
                        || err.raw_os_error() == Some(3) =>
                {
                    true
                }
                Ok(stat) => stat
                    .rsplit_once(") ")
                    .is_some_and(|(_, rest)| rest.starts_with('Z') || rest.starts_with('X')),
                Err(err) => panic!("cannot inspect renderer child: {err}"),
            };
            if stopped || Instant::now() >= deadline {
                break stopped;
            }
            std::thread::sleep(Duration::from_millis(10));
        };
        // Clean up the fixture even when the regression is present.
        if !worker_stopped {
            let _ = Command::new("kill").args(["-KILL", ids[1]]).output();
        }

        assert!(result.output_limited);
        assert!(renderer_reaped, "renderer was not reaped");
        assert!(worker_stopped, "renderer child still running");
    }

    #[cfg(unix)]
    fn template_test_client(temp: &tempfile::TempDir, script: &str) -> ShellChezmoiClient {
        use std::os::unix::fs::PermissionsExt;

        let binary = temp.path().join("template-renderer");
        std::fs::write(&binary, format!("#!/bin/sh\n{script}\n")).expect("write renderer");
        std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o755))
            .expect("make renderer executable");
        ShellChezmoiClient::new(
            binary.to_string_lossy(),
            temp.path().to_path_buf(),
            temp.path().to_path_buf(),
            None,
        )
    }

    #[cfg(unix)]
    #[test]
    fn template_stdin_that_is_never_read_still_times_out() {
        let temp = tempfile::tempdir().expect("tempdir");
        let pid_file = temp.path().join("renderer.pid");
        let client = template_test_client(
            &temp,
            &format!(
                "printf '%s' \"$$\" > '{}'\nexec sleep 30",
                pid_file.display()
            ),
        );
        let started = Instant::now();
        let result = client
            .run_raw_with_input(
                ["execute-template"],
                temp.path(),
                Some(&"x".repeat(512 * 1024)),
                CommandLimits {
                    timeout: Duration::from_millis(100),
                    max_stdout_bytes: 64,
                    max_stderr_bytes: 64,
                },
            )
            .expect("bounded renderer");

        assert!(result.timed_out);
        assert!(started.elapsed() < Duration::from_secs(2));
        assert!(
            ensure_complete_success(&result, "chezmoi execute-template")
                .expect_err("timeout must propagate")
                .to_string()
                .contains("timed out")
        );
        let pid = std::fs::read_to_string(pid_file).expect("renderer PID");
        let alive = Command::new("kill")
            .args(["-0", &pid])
            .output()
            .expect("check PID");
        assert!(!alive.status.success(), "renderer not reaped");
    }

    #[cfg(unix)]
    #[test]
    fn template_can_write_large_output_before_reading_large_stdin() {
        let temp = tempfile::tempdir().expect("tempdir");
        let client =
            template_test_client(&temp, "dd if=/dev/zero bs=65536 count=2 2>/dev/null\ncat");
        let input = "template-input".repeat(16 * 1024);
        let result = client
            .run_raw_with_input(
                ["execute-template"],
                temp.path(),
                Some(&input),
                CommandLimits {
                    timeout: Duration::from_secs(2),
                    max_stdout_bytes: 1024 * 1024,
                    max_stderr_bytes: 64,
                },
            )
            .expect("render without pipe deadlock");

        ensure_complete_success(&result, "chezmoi execute-template").expect("successful rendering");
        assert!(result.stdout.starts_with(&"\0".repeat(128 * 1024)));
        assert!(result.stdout.ends_with(&input));
    }

    #[cfg(unix)]
    #[test]
    fn template_that_exits_without_reading_stdin_reports_failure() {
        let temp = tempfile::tempdir().expect("tempdir");
        let client = template_test_client(&temp, "printf 'template rejected' >&2\nexit 7");
        let result = client
            .run_raw_with_input(
                ["execute-template"],
                temp.path(),
                Some(&"x".repeat(512 * 1024)),
                CommandLimits {
                    timeout: Duration::from_secs(1),
                    max_stdout_bytes: 64,
                    max_stderr_bytes: 64,
                },
            )
            .expect("no broken-pipe error or leaked child");

        assert_eq!(result.exit_code, 7);
        assert_eq!(result.stderr, "template rejected");
        assert!(!result.timed_out);
    }

    #[test]
    fn run_command_with_limits_captures_stdout() {
        let mut cmd = Command::new("sh");
        cmd.arg("-c").arg("printf hello");

        let result = run_command_with_limits(
            cmd,
            &[OsString::from("stdout-test")],
            CommandLimits {
                timeout: Duration::from_secs(1),
                max_stdout_bytes: 1024,
                max_stderr_bytes: 1024,
            },
        )
        .expect("run command");

        assert_eq!(result.stdout, "hello");
        assert!(!result.timed_out);
        assert!(!result.output_limited);
    }

    #[cfg(unix)]
    #[test]
    fn run_command_with_limits_kills_timed_out_command() {
        let mut cmd = Command::new("sh");
        cmd.arg("-c").arg("sleep 5");

        let result = run_command_with_limits(
            cmd,
            &[OsString::from("sleep-test")],
            CommandLimits {
                timeout: Duration::from_millis(100),
                max_stdout_bytes: 1024,
                max_stderr_bytes: 1024,
            },
        )
        .expect("run command");

        assert!(result.timed_out);
        assert!(!result.stderr.is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn run_command_with_limits_kills_output_limited_command() {
        let mut cmd = Command::new("sh");
        cmd.arg("-c").arg("yes x");

        let result = run_command_with_limits(
            cmd,
            &[OsString::from("output-limit-test")],
            CommandLimits {
                timeout: Duration::from_secs(5),
                max_stdout_bytes: 1024,
                max_stderr_bytes: 1024,
            },
        )
        .expect("run command");

        assert!(result.output_limited);
        assert!(result.stdout_truncated);
        assert!(result.stderr.contains("output limit exceeded"));
    }

    #[test]
    fn ensure_complete_success_rejects_truncated_output() {
        let result = CommandResult {
            exit_code: 0,
            stdout: "ok".to_string(),
            stderr: String::new(),
            duration_ms: 10,
            timed_out: false,
            output_limited: true,
            stdout_truncated: false,
            stderr_truncated: false,
        };
        assert!(ensure_complete_success(&result, "test").is_err());
    }

    #[test]
    fn ensure_complete_success_rejects_timed_out() {
        let result = CommandResult {
            exit_code: -1,
            stdout: String::new(),
            stderr: String::new(),
            duration_ms: 120000,
            timed_out: true,
            output_limited: false,
            stdout_truncated: false,
            stderr_truncated: false,
        };
        assert!(ensure_complete_success(&result, "test").is_err());
    }
}
