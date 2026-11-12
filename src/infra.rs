use crate::domain::{Action, ActionRequest, ChangeKind, CommandResult, DiffText, StatusEntry};
use anyhow::{Context, Result, bail};
use std::collections::BTreeSet;
use std::ffi::{OsStr, OsString};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdout, Command, ExitStatus, Stdio};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, Instant};

pub trait ChezmoiClient: Send + Sync {
    fn status(&self) -> Result<Vec<StatusEntry>>;
    fn managed(&self) -> Result<Vec<PathBuf>>;
    fn unmanaged(&self) -> Result<Vec<PathBuf>>;
    /// Rendered `.chezmoiignore` lines (the file may be a template).
    fn ignore_patterns(&self) -> Result<Vec<String>>;
    fn source(&self) -> Result<(PathBuf, Vec<PathBuf>)>;
    fn diff(&self, targets: &[PathBuf]) -> Result<DiffText>;
    fn diff_cancellable(&self, targets: &[PathBuf], _cancelled: &AtomicBool) -> Result<DiffText> {
        self.diff(targets)
    }
    fn run(&self, request: &ActionRequest) -> Result<CommandResult>;
    fn run_cancellable(
        &self,
        request: &ActionRequest,
        _cancelled: &AtomicBool,
    ) -> Result<CommandResult> {
        self.run(request)
    }
    /// Request cancellation of the currently running process tree.
    fn cancel_pending(&self) {}
}

#[derive(Debug, Clone)]
pub struct ShellChezmoiClient {
    binary: String,
    home_dir: PathBuf,
    working_dir: PathBuf,
    source_dir: Option<PathBuf>,
    config_file: Option<PathBuf>,
    cancelled: Arc<AtomicBool>,
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
            config_file: None,
            cancelled: Arc::new(AtomicBool::new(false)),
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
            config_file: None,
            cancelled: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn with_config_file(mut self, config_file: Option<PathBuf>) -> Self {
        self.config_file = config_file;
        self
    }

    pub fn resolve_destination(
        binary: &str,
        source_dir: Option<&Path>,
        config_file: Option<&Path>,
    ) -> Result<PathBuf> {
        let mut cmd = Command::new(binary);
        if let Some(source_dir) = source_dir {
            cmd.arg("--source").arg(source_dir);
        }
        if let Some(config_file) = config_file {
            cmd.arg("--config").arg(config_file);
        }
        let args = vec![
            os("target-path"),
            os("--no-tty"),
            os("--progress=false"),
            os("--no-pager"),
            os("--color=false"),
        ];
        cmd.args(&args).stdin(Stdio::null());
        let result =
            run_command_with_limits_mode(cmd, &args, CommandLimits::default(), None, None, true)?;
        ensure_complete_success(&result, "chezmoi target-path")?;
        parse_destination_output(&result.stdout)
    }

    fn run_raw<I, S>(&self, args: I, destination_dir: &Path) -> Result<CommandResult>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        self.run_raw_with_input(
            args,
            destination_dir,
            None,
            CommandLimits::default(),
            false,
            None,
        )
    }

    fn run_raw_machine<I, S>(&self, args: I, destination_dir: &Path) -> Result<CommandResult>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        self.run_raw_with_input(
            args,
            destination_dir,
            None,
            CommandLimits::default(),
            true,
            None,
        )
    }

    fn run_raw_detail<I, S>(
        &self,
        args: I,
        destination_dir: &Path,
        detail_cancelled: &AtomicBool,
    ) -> Result<CommandResult>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        self.run_raw_with_input(
            args,
            destination_dir,
            None,
            CommandLimits::default(),
            false,
            Some(detail_cancelled),
        )
    }

    fn run_raw_with_input<I, S>(
        &self,
        args: I,
        destination_dir: &Path,
        input: Option<&str>,
        limits: CommandLimits,
        strict_stdout_utf8: bool,
        detail_cancelled: Option<&AtomicBool>,
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
        if let Some(config_file) = &self.config_file {
            cmd.arg("--config").arg(config_file);
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
        } else {
            cmd.stdin(Stdio::null());
        }

        if self.command_cancelled(detail_cancelled) {
            bail!("chezmoi command cancelled before start");
        }
        let result = run_command_with_limits_mode(
            cmd,
            &args,
            limits,
            Some(&self.cancelled),
            detail_cancelled,
            strict_stdout_utf8,
        )?;

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

    fn command_cancelled(&self, detail_cancelled: Option<&AtomicBool>) -> bool {
        self.cancelled.load(Ordering::Acquire)
            || detail_cancelled.is_some_and(|flag| flag.load(Ordering::Acquire))
    }

    fn destination_for_targets(&self, _targets: &[PathBuf]) -> &Path {
        // Unmanaged browsing may use cwd as its scan context, but diffs and
        // mutations must target the one destination resolved for the session.
        &self.home_dir
    }

    fn browsing_destination(&self) -> &Path {
        if self.working_dir.starts_with(&self.home_dir) {
            &self.home_dir
        } else {
            &self.working_dir
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

#[derive(Debug)]
struct LimitedText {
    text: String,
    truncated: bool,
}

pub(crate) struct OwnedChild {
    child: Child,
    reaped: bool,
}

impl OwnedChild {
    pub(crate) fn new(child: Child) -> Self {
        Self {
            child,
            reaped: false,
        }
    }

    pub(crate) fn take_stdout(&mut self) -> Option<ChildStdout> {
        self.child.stdout.take()
    }

    fn try_wait(&mut self) -> std::io::Result<Option<ExitStatus>> {
        let status = self.child.try_wait()?;
        self.reaped = status.is_some();
        Ok(status)
    }

    pub(crate) fn wait(&mut self) -> std::io::Result<ExitStatus> {
        let status = self.child.wait()?;
        self.reaped = true;
        Ok(status)
    }

    pub(crate) fn kill_and_wait(&mut self) -> std::io::Result<ExitStatus> {
        kill_command_tree(&mut self.child);
        self.wait()
    }
}

impl Drop for OwnedChild {
    fn drop(&mut self) {
        if !self.reaped {
            kill_command_tree(&mut self.child);
            let _ = self.wait();
        }
    }
}

fn append_marker(text: &mut String, marker: &str) {
    if !text.ends_with('\n') && !text.is_empty() {
        text.push('\n');
    }
    text.push_str(marker);
    text.push('\n');
}

fn read_temp_file_limited(
    mut file: File,
    max_bytes: usize,
    strict_stdout_utf8: bool,
) -> Result<LimitedText> {
    file.seek(SeekFrom::Start(0))?;

    let mut limited = file.take((max_bytes + 1) as u64);
    let mut bytes = Vec::new();
    limited.read_to_end(&mut bytes)?;

    let truncated = bytes.len() > max_bytes;
    if truncated {
        bytes.truncate(max_bytes);
    }

    let mut text = if strict_stdout_utf8 {
        std::str::from_utf8(&bytes)
            .context(
                "command produced path-bearing output that is not valid UTF-8; refusing to parse it",
            )?
            .to_owned()
    } else {
        String::from_utf8_lossy(&bytes).to_string()
    };

    if truncated {
        text.push_str(&format!(
            "\n--- output truncated at {max_bytes} bytes ---\n"
        ));
    }

    Ok(LimitedText { text, truncated })
}

fn run_command_with_limits_mode(
    mut cmd: Command,
    args_for_log: &[OsString],
    limits: CommandLimits,
    shutdown_cancelled: Option<&AtomicBool>,
    detail_cancelled: Option<&AtomicBool>,
    strict_stdout_utf8: bool,
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

    // Keep subprocesses in their own group so limits and cancellation stop descendants too.
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }

    let mut child = OwnedChild::new(
        cmd.stdout(Stdio::from(stdout_for_child))
            .stderr(Stdio::from(stderr_for_child))
            .spawn()
            .with_context(|| format!("failed to spawn command {args_for_log:?}"))?,
    );

    let deadline = started + limits.timeout;
    let mut timed_out = false;
    let mut output_limited = false;

    let mut poll_delay = Duration::from_millis(2);
    loop {
        if shutdown_cancelled.is_some_and(|flag| flag.load(Ordering::Acquire))
            || detail_cancelled.is_some_and(|flag| flag.load(Ordering::Acquire))
        {
            child
                .kill_and_wait()
                .with_context(|| "failed to wait after cancellation")?;
            bail!("chezmoi command cancelled");
        }

        let stdout_len = stdout_file.metadata()?.len();
        let stderr_len = stderr_file.metadata()?.len();
        if stdout_len > limits.max_stdout_bytes as u64
            || stderr_len > limits.max_stderr_bytes as u64
        {
            output_limited = true;
            let status = child
                .kill_and_wait()
                .with_context(|| "failed to wait after output limit kill")?;
            let duration_ms = elapsed_millis_u64(started);

            let stdout_read =
                read_temp_file_limited(stdout_file, limits.max_stdout_bytes, strict_stdout_utf8)?;
            let mut stderr_read =
                read_temp_file_limited(stderr_file, limits.max_stderr_bytes, false)?;
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

            let stdout_read =
                read_temp_file_limited(stdout_file, limits.max_stdout_bytes, strict_stdout_utf8)?;
            let stderr_read = read_temp_file_limited(stderr_file, limits.max_stderr_bytes, false)?;

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
            let status = child
                .kill_and_wait()
                .with_context(|| "failed to wait after killing child")?;
            let duration_ms = elapsed_millis_u64(started);

            let stdout_read =
                read_temp_file_limited(stdout_file, limits.max_stdout_bytes, strict_stdout_utf8)?;
            let mut stderr_read =
                read_temp_file_limited(stderr_file, limits.max_stderr_bytes, false)?;
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

        std::thread::sleep(poll_delay);
        poll_delay = (poll_delay * 2).min(Duration::from_millis(10));
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
        let managed = self.managed()?;
        if let Some(path) = managed.iter().find(|path| {
            path.as_os_str()
                .to_str()
                .is_some_and(|path| path.contains(['\n', '\r']))
        }) {
            bail!(
                "chezmoi status cannot safely represent a managed path containing a newline or carriage return: {path:?}"
            );
        }

        let result = self.run_raw_machine(machine_args("status"), &self.home_dir)?;
        ensure_complete_success(&result, "chezmoi status")?;
        parse_status_output(&result.stdout)
    }

    fn managed(&self) -> Result<Vec<PathBuf>> {
        let result = self.run_raw_machine(
            machine_args_with(
                "managed",
                &["--path-style=relative", "--nul-path-separator"],
            ),
            &self.home_dir,
        )?;
        ensure_complete_success(&result, "chezmoi managed")?;
        Ok(parse_managed_output(&result.stdout))
    }

    fn unmanaged(&self) -> Result<Vec<PathBuf>> {
        // This read-only view may browse outside the session destination using
        // cwd as its own chezmoi context; mutations never take this path.
        let destination = self.browsing_destination();

        let mut args = machine_args_with(
            "unmanaged",
            &["--path-style=absolute", "--nul-path-separator", "--"],
        );
        args.push(self.working_dir.as_os_str().to_os_string());
        let result = self.run_raw_machine(args, destination)?;
        ensure_complete_success(&result, "chezmoi unmanaged")?;
        let paths = parse_unmanaged_output(&result.stdout);
        unmanaged_paths_relative_to_cwd(paths, destination, &self.working_dir)
    }

    fn ignore_patterns(&self) -> Result<Vec<String>> {
        let destination = self.browsing_destination();
        let source_dir = self.source_dir(destination)?;
        // Let chezmoi decide which source directories are active. A raw walk
        // would render templates under ignored or external subtrees that
        // chezmoi deliberately skips, disabling otherwise-valid filtering.
        let result = self.run_raw_machine(
            machine_args_with(
                "managed",
                &[
                    "--include=dirs",
                    "--exclude=externals",
                    "--path-style=source-absolute",
                    "--nul-path-separator",
                ],
            ),
            destination,
        )?;
        ensure_complete_success(&result, "chezmoi managed source directories")?;
        let active_directories = parse_managed_output(&result.stdout);
        let mut patterns = Vec::new();
        for ignore_file in collect_ignore_files(&source_dir, active_directories)? {
            let rendered = self.execute_template(&ignore_file, destination)?;
            let parent = ignore_file.parent().context("ignore file has no parent")?;
            if parent == source_dir {
                patterns.extend(rendered.lines().map(str::to_owned));
                continue;
            }

            // Ask chezmoi to decode source attributes instead of maintaining a
            // second decoder for private_, exact_, dot_, literal_, etc.
            let mut args = machine_args("target-path");
            args.push(os("--"));
            args.push(parent.as_os_str().to_os_string());
            let result = self.run_raw_machine(args, destination)?;
            ensure_complete_success(&result, "chezmoi target-path")?;
            let target = parse_path_record(&result.stdout, "chezmoi target-path")?;
            let relative = target.strip_prefix(destination).with_context(|| {
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
        let source_dir = self.source_dir(&self.home_dir)?;
        let paths = list_source_paths(&source_dir)?;
        Ok((source_dir, paths))
    }

    fn diff(&self, targets: &[PathBuf]) -> Result<DiffText> {
        let args = diff_args(targets);
        let destination = self.destination_for_targets(targets);

        let result = self.run_raw(&args, destination)?;
        ensure_complete_success(&result, "chezmoi diff")?;

        Ok(DiffText {
            text: result.stdout,
        })
    }

    fn diff_cancellable(&self, targets: &[PathBuf], cancelled: &AtomicBool) -> Result<DiffText> {
        let args = diff_args(targets);
        let destination = self.destination_for_targets(targets);

        let result = self.run_raw_detail(&args, destination, cancelled)?;
        ensure_complete_success(&result, "chezmoi diff")?;

        Ok(DiffText {
            text: result.stdout,
        })
    }

    fn run(&self, request: &ActionRequest) -> Result<CommandResult> {
        let args = background_action_args(action_to_args(request)?);
        let destination = self.destination_for_targets(&request.targets);
        self.run_raw(&args, destination)
    }

    fn run_cancellable(
        &self,
        request: &ActionRequest,
        cancelled: &AtomicBool,
    ) -> Result<CommandResult> {
        let args = background_action_args(action_to_args(request)?);
        let destination = self.destination_for_targets(&request.targets);
        self.run_raw_detail(&args, destination, cancelled)
    }

    fn cancel_pending(&self) {
        self.cancelled.store(true, Ordering::Release);
    }
}

impl ShellChezmoiClient {
    fn source_dir(&self, destination: &Path) -> Result<PathBuf> {
        // --source is a working tree, not necessarily the effective source root
        // when .chezmoiroot is present. Let chezmoi resolve it in both cases.
        let result = self.run_raw_machine(machine_args("source-path"), destination)?;
        ensure_complete_success(&result, "chezmoi source-path")?;
        parse_path_record(&result.stdout, "chezmoi source-path")
    }

    /// Render an ignore template with its source-root-relative template name.
    fn execute_template(&self, template_path: &Path, destination: &Path) -> Result<String> {
        // --file uses the same source-relative name as SourceState; targetFile is
        // empty in both paths.
        let mut args = machine_args("execute-template");
        args.push(os("--file=true"));
        args.push(os("--"));
        args.push(template_path.as_os_str().to_os_string());
        let result = self.run_raw_machine(args, destination)?;
        ensure_complete_success(&result, "chezmoi execute-template")?;
        Ok(result.stdout)
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
    parse_nul_path_records(output)
}

pub fn parse_unmanaged_output(output: &str) -> Vec<PathBuf> {
    parse_nul_path_records(output)
}

fn parse_nul_path_records(output: &str) -> Vec<PathBuf> {
    output
        .split('\0')
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
        .collect()
}

fn list_source_paths(source_dir: &Path) -> Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    collect_source_paths(source_dir, source_dir, false, &mut out)?;
    out.sort_by(|a, b| a.to_string_lossy().cmp(&b.to_string_lossy()));
    Ok(out)
}

fn collect_source_paths(
    base: &Path,
    current: &Path,
    inside_special: bool,
    out: &mut Vec<PathBuf>,
) -> Result<()> {
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
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let is_special = inside_special || name.starts_with(".chezmoi");
        if matches!(name.as_ref(), ".git" | ".hg" | ".svn")
            || (name.starts_with('.') && !is_special)
        {
            continue;
        }
        out.push(relative);
        let file_type = entry
            .file_type()
            .with_context(|| format!("failed to inspect {}", path.display()))?;
        if file_type.is_dir() && !file_type.is_symlink() {
            collect_source_paths(base, &path, is_special, out)?;
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

fn unmanaged_paths_relative_to_cwd(
    paths: Vec<PathBuf>,
    destination: &Path,
    working_dir: &Path,
) -> Result<Vec<PathBuf>> {
    let cwd_in_destination = working_dir.strip_prefix(destination).with_context(|| {
        format!(
            "working directory {} is outside destination {}",
            working_dir.display(),
            destination.display()
        )
    })?;
    let relative_paths: Result<BTreeSet<_>> = paths
        .into_iter()
        .map(|path| {
            let relative = if path.is_absolute() {
                path.strip_prefix(working_dir)
                    .with_context(|| {
                        format!(
                            "chezmoi unmanaged returned path outside working directory: {}",
                            path.display()
                        )
                    })?
                    .to_path_buf()
            } else if path == cwd_in_destination || cwd_in_destination.starts_with(&path) {
                PathBuf::from(".")
            } else if cwd_in_destination.as_os_str().is_empty() {
                path
            } else {
                path.strip_prefix(cwd_in_destination)
                    .with_context(|| {
                        format!(
                            "chezmoi unmanaged returned path outside working directory: {}",
                            path.display()
                        )
                    })?
                    .to_path_buf()
            };
            Ok(if relative.as_os_str().is_empty() {
                PathBuf::from(".")
            } else {
                relative
            })
        })
        .collect();
    Ok(relative_paths?.into_iter().collect())
}

fn parse_destination_output(output: &str) -> Result<PathBuf> {
    let destination = parse_path_record(output, "chezmoi target-path")?;
    if !destination.is_absolute() {
        bail!(
            "chezmoi target-path returned a non-absolute destination: {}",
            destination.display()
        );
    }
    Ok(destination)
}

fn parse_path_record(output: &str, command: &str) -> Result<PathBuf> {
    let output = output.strip_suffix('\n').unwrap_or(output);
    if output.is_empty() || output.contains('\r') || output.contains('\n') {
        bail!("{command} returned an invalid path record");
    }
    Ok(PathBuf::from(output))
}

pub fn action_to_args(request: &ActionRequest) -> Result<Vec<OsString>> {
    let action = request.action;
    let targets = request.targets.clone();
    let target = || targets.first().map(|path| path.as_os_str().to_os_string());

    let args = match action {
        Action::Apply => apply_args(&targets),
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
        Action::ReAdd => vec![os("re-add"), os("--"), required_target(target(), action)?],
        Action::Merge => {
            let mut args = vec![os("merge")];
            if let Some(path) = target() {
                args.push(os("--"));
                args.push(path);
            }
            args
        }
        Action::MergeAll => vec![os("merge-all")],
        Action::Add => add_args(request.chattr_attrs.as_deref(), target(), action)?,
        Action::Ignore => {
            bail!("ignore is an internal action and does not map to a chezmoi CLI command")
        }
        Action::Edit => vec![os("edit"), os("--"), required_target(target(), action)?],
        Action::Forget => vec![
            os("forget"),
            os("--force"),
            os("--no-tty"),
            os("--"),
            required_target(target(), action)?,
        ],
        Action::Chattr => vec![
            os("chattr"),
            os("--"),
            request
                .chattr_attrs
                .as_deref()
                .map(OsString::from)
                .context("chattr requires attributes")?,
            required_target(target(), action)?,
        ],
        Action::Destroy => vec![
            os("destroy"),
            os("--force"),
            os("--no-tty"),
            os("--"),
            required_target(target(), action)?,
        ],
        Action::Purge => vec![os("purge"), os("--force"), os("--no-tty")],
        Action::Verify => {
            let mut args = vec![os("verify")];
            if !targets.is_empty() {
                args.push(os("--"));
                args.extend(targets.iter().map(|path| path.as_os_str().to_os_string()));
            }
            args
        }
        Action::Cat => vec![os("cat"), os("--"), required_target(target(), action)?],
        Action::Ignored => vec![os("ignored")],
        Action::DumpConfig => vec![os("dump-config"), os("--format=json")],
        Action::DiffAll => vec![
            os("diff"),
            os("--no-pager"),
            os("--use-builtin-diff"),
            os("--color=true"),
        ],
        Action::GitStatus => vec![
            os("git"),
            os("--"),
            os("--no-pager"),
            os("status"),
            os("--short"),
            os("--branch"),
        ],
        Action::GitDiff => vec![
            os("git"),
            os("--"),
            os("--no-pager"),
            os("diff"),
            os("--no-ext-diff"),
            os("--no-color"),
        ],
        Action::GitLog => vec![
            os("git"),
            os("--"),
            os("--no-pager"),
            os("log"),
            os("--oneline"),
            os("--decorate"),
            os("-n"),
            os("30"),
        ],
        Action::GitPull => vec![os("update"), os("--apply=false")],
        Action::GitPush => vec![os("git"), os("--"), os("push")],
        Action::GitCommit => vec![os("git"), os("--"), os("commit")],
        Action::GitStage => vec![os("git"), os("--"), os("add"), os("--")]
            .into_iter()
            .chain(required_targets(&targets, action)?)
            .collect(),
        Action::EditSource => bail!("edit-source is handled by the foreground editor path"),
        Action::ReAddAll => vec![os("re-add")],
        Action::Init => vec![os("init")],
        Action::AddDirectory => vec![
            os("add"),
            os("--recursive"),
            os("--"),
            required_target(target(), action)?,
        ],
        Action::PreviewAddDirectory => vec![
            os("add"),
            os("--dry-run"),
            os("--verbose"),
            os("--recursive"),
            os("--"),
            required_target(target(), action)?,
        ],
    };

    Ok(args)
}

fn apply_args(targets: &[PathBuf]) -> Vec<OsString> {
    let mut args = vec![os("apply"), os("--error-on-conflict=true"), os("--no-tty")];
    if !targets.is_empty() {
        args.push(os("--"));
        args.extend(targets.iter().map(|path| path.as_os_str().to_os_string()));
    }
    args
}

fn add_args(
    attrs: Option<&str>,
    target: Option<OsString>,
    action: Action,
) -> Result<Vec<OsString>> {
    let mut template = false;
    let mut encrypt = false;
    for attr in attrs
        .into_iter()
        .flat_map(|attrs| attrs.split(','))
        .map(str::trim)
        .filter(|attr| !attr.is_empty())
    {
        match attr {
            "template" => template = true,
            "encrypt" | "encrypted" => encrypt = true,
            "private" | "executable" => {}
            _ => bail!("unsupported add attribute: {attr}"),
        }
    }

    let mut args = vec![os("add")];
    if template {
        args.push(os("--template"));
    }
    if encrypt {
        args.push(os("--encrypt"));
    }
    args.push(os("--"));
    args.push(required_target(target, action)?);
    Ok(args)
}

fn required_target(target: Option<OsString>, action: Action) -> Result<OsString> {
    target.with_context(|| format!("{} requires target", action.label()))
}

fn required_targets(targets: &[PathBuf], action: Action) -> Result<Vec<OsString>> {
    if targets.is_empty() {
        bail!("{} requires target", action.label());
    }
    Ok(targets
        .iter()
        .map(|path| path.as_os_str().to_os_string())
        .collect())
}

fn diff_args(targets: &[PathBuf]) -> Vec<OsString> {
    let mut args = vec![
        os("diff"),
        os("--no-tty"),
        os("--progress=false"),
        os("--no-pager"),
        os("--recursive"),
        os("--use-builtin-diff"),
        os("--color=true"),
    ];
    if !targets.is_empty() {
        args.push(os("--"));
        args.extend(targets.iter().map(|path| path.as_os_str().to_os_string()));
    }
    args
}

fn os(value: &str) -> OsString {
    OsString::from(value)
}

fn machine_args(command: &str) -> Vec<OsString> {
    machine_args_with(command, &[])
}

fn machine_args_with(command: &str, args: &[&str]) -> Vec<OsString> {
    let mut output = Vec::with_capacity(5 + args.len());
    output.push(os(command));
    output.extend(
        [
            "--no-tty",
            "--progress=false",
            "--no-pager",
            "--color=false",
        ]
        .into_iter()
        .map(os),
    );
    output.extend(args.iter().map(|arg| os(arg)));
    output
}

fn background_action_args(mut args: Vec<OsString>) -> Vec<OsString> {
    let insert_at = args
        .iter()
        .position(|arg| arg == OsStr::new("--"))
        .unwrap_or(args.len());
    let mut flags = Vec::new();
    for flag in [
        "--no-tty",
        "--progress=false",
        "--no-pager",
        "--color=false",
    ] {
        let already_set = if flag == "--color=false" {
            args.iter()
                .filter_map(|arg| arg.to_str())
                .any(|arg| arg.starts_with("--color="))
        } else {
            args.iter().any(|arg| arg == OsStr::new(flag))
        };
        if !already_set {
            flags.push(os(flag));
        }
    }
    args.splice(insert_at..insert_at, flags);
    args
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    static SCRIPT_FIXTURES: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn script_guard() -> std::sync::MutexGuard<'static, ()> {
        SCRIPT_FIXTURES
            .lock()
            .unwrap_or_else(|error| error.into_inner())
    }

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
    fn parse_managed_nul_preserves_all_path_whitespace() {
        let records = ".zshrc\0name with trailing space \0line\nbreak\0";
        assert_eq!(
            parse_managed_output(records),
            vec![
                PathBuf::from(".zshrc"),
                PathBuf::from("name with trailing space "),
                PathBuf::from("line\nbreak"),
            ]
        );
    }

    #[test]
    fn parse_unmanaged_nul_accepts_unterminated_final_record() {
        let output = ".cache/file\0.local/tmp";
        assert_eq!(
            parse_unmanaged_output(output),
            vec![PathBuf::from(".cache/file"), PathBuf::from(".local/tmp")]
        );
    }

    #[test]
    fn parse_unmanaged_nul_preserves_spaces_and_newlines() {
        let output = "/tmp/work/a file\0/tmp/work/line\nbreak\0/tmp/work/ padded \0";
        assert_eq!(
            parse_unmanaged_output(output),
            vec![
                PathBuf::from("/tmp/work/a file"),
                PathBuf::from("/tmp/work/line\nbreak"),
                PathBuf::from("/tmp/work/ padded "),
            ]
        );
    }

    #[test]
    fn destination_output_must_be_one_absolute_path() {
        assert_eq!(
            parse_destination_output("/tmp/destination with spaces\n").expect("absolute path"),
            PathBuf::from("/tmp/destination with spaces")
        );
        assert_eq!(
            parse_destination_output("/tmp/destination with spaces \n").expect("trailing space"),
            PathBuf::from("/tmp/destination with spaces ")
        );
        assert!(parse_destination_output("/tmp/destination\r\n").is_err());
        assert!(parse_destination_output("relative/path\n").is_err());
        assert!(parse_destination_output("\n").is_err());
        assert!(parse_destination_output("/tmp/one\n/tmp/two\n").is_err());
    }

    #[test]
    fn unmanaged_paths_are_relative_to_working_dir() {
        let paths = vec![
            PathBuf::from("/home/tetsuya/dev/chezmoi-tui/.git"),
            PathBuf::from("/home/tetsuya/dev/chezmoi-tui/src"),
        ];
        let got = unmanaged_paths_relative_to_cwd(
            paths,
            Path::new("/home/tetsuya"),
            Path::new("/home/tetsuya/dev/chezmoi-tui"),
        )
        .expect("scope unmanaged");
        assert_eq!(got, vec![PathBuf::from(".git"), PathBuf::from("src")]);
    }

    #[test]
    fn unmanaged_cwd_maps_to_working_root() {
        let paths = vec![PathBuf::from("/home/tetsuya/dev/chezmoi-tui")];
        let got = unmanaged_paths_relative_to_cwd(
            paths,
            Path::new("/home/tetsuya"),
            Path::new("/home/tetsuya/dev/chezmoi-tui"),
        )
        .expect("scope unmanaged");
        assert_eq!(got, vec![PathBuf::from(".")]);
    }

    #[test]
    fn unmanaged_legacy_relative_output_is_scoped_to_cwd() {
        let paths = vec![
            PathBuf::from("dev/chezmoi-tui"),
            PathBuf::from("dev/chezmoi-tui/src"),
        ];
        let got = unmanaged_paths_relative_to_cwd(
            paths,
            Path::new("/home/tetsuya"),
            Path::new("/home/tetsuya/dev/chezmoi-tui"),
        )
        .expect("scope legacy unmanaged");
        assert_eq!(got, vec![PathBuf::from("."), PathBuf::from("src")]);
    }

    #[test]
    fn action_mapping_includes_danger_and_chattr() {
        let purge = ActionRequest {
            action: Action::Purge,
            targets: Vec::new(),
            chattr_attrs: None,
        };
        assert_eq!(
            action_to_args(&purge).expect("purge args"),
            vec![os("purge"), os("--force"), os("--no-tty")]
        );

        let doctor = ActionRequest {
            action: Action::Doctor,
            targets: Vec::new(),
            chattr_attrs: None,
        };
        assert_eq!(
            action_to_args(&doctor).expect("doctor args"),
            vec![os("doctor")]
        );

        let data = ActionRequest {
            action: Action::Data,
            targets: Vec::new(),
            chattr_attrs: None,
        };
        assert_eq!(
            action_to_args(&data).expect("data args"),
            vec![os("data"), os("--format"), os("json")]
        );

        let open_source_dir = ActionRequest {
            action: Action::OpenSourceDir,
            targets: Vec::new(),
            chattr_attrs: None,
        };
        assert!(action_to_args(&open_source_dir).is_err());

        let edit = ActionRequest {
            action: Action::Edit,
            targets: vec![PathBuf::from(".zshrc")],
            chattr_attrs: None,
        };
        assert_eq!(
            action_to_args(&edit).expect("edit args"),
            vec![os("edit"), os("--"), os(".zshrc")]
        );

        let apply = ActionRequest {
            action: Action::Apply,
            targets: vec![PathBuf::from(".zshrc")],
            chattr_attrs: None,
        };
        assert_eq!(
            action_to_args(&apply).expect("apply args"),
            vec![
                os("apply"),
                os("--error-on-conflict=true"),
                os("--no-tty"),
                os("--"),
                os(".zshrc"),
            ]
        );

        let edit_config = ActionRequest {
            action: Action::EditConfig,
            targets: Vec::new(),
            chattr_attrs: None,
        };
        assert_eq!(
            action_to_args(&edit_config).expect("edit-config args"),
            vec![os("edit-config")]
        );

        let edit_config_template = ActionRequest {
            action: Action::EditConfigTemplate,
            targets: Vec::new(),
            chattr_attrs: None,
        };
        assert_eq!(
            action_to_args(&edit_config_template).expect("edit-config-template args"),
            vec![os("edit-config-template")]
        );

        let forget = ActionRequest {
            action: Action::Forget,
            targets: vec![PathBuf::from(".zshrc")],
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
            targets: vec![PathBuf::from(".zshrc")],
            chattr_attrs: Some("private,template".to_string()),
        };
        assert_eq!(
            action_to_args(&chattr).expect("chattr args"),
            vec![os("chattr"), os("--"), os("private,template"), os(".zshrc")]
        );

        let readd = ActionRequest {
            action: Action::ReAdd,
            targets: vec![PathBuf::from(".zshrc")],
            chattr_attrs: None,
        };
        assert_eq!(
            action_to_args(&readd).expect("re-add args"),
            vec![os("re-add"), os("--"), os(".zshrc")]
        );

        let ignore = ActionRequest {
            action: Action::Ignore,
            targets: vec![PathBuf::from(".cache")],
            chattr_attrs: None,
        };
        assert!(action_to_args(&ignore).is_err());

        let edit_ignore = ActionRequest {
            action: Action::EditIgnore,
            targets: Vec::new(),
            chattr_attrs: None,
        };
        assert!(action_to_args(&edit_ignore).is_err());
    }

    #[test]
    fn diff_target_args_are_option_safe_and_recursive() {
        let got = diff_args(&[PathBuf::from("-n"), PathBuf::from("dir with space")]);
        assert_eq!(
            got,
            vec![
                os("diff"),
                os("--no-tty"),
                os("--progress=false"),
                os("--no-pager"),
                os("--recursive"),
                os("--use-builtin-diff"),
                os("--color=true"),
                os("--"),
                os("-n"),
                os("dir with space"),
            ]
        );
    }

    #[test]
    fn diff_args_include_recursive_without_target() {
        let got = diff_args(&[]);
        assert_eq!(
            got,
            vec![
                os("diff"),
                os("--no-tty"),
                os("--progress=false"),
                os("--no-pager"),
                os("--recursive"),
                os("--use-builtin-diff"),
                os("--color=true")
            ]
        );
    }

    #[test]
    fn add_maps_template_and_encryption_to_initial_flags() {
        let request = ActionRequest {
            action: Action::Add,
            targets: vec![PathBuf::from("secret.txt")],
            chattr_attrs: Some("encrypted,template,private".to_string()),
        };
        assert_eq!(
            action_to_args(&request).expect("add args"),
            vec![
                os("add"),
                os("--template"),
                os("--encrypt"),
                os("--"),
                os("secret.txt"),
            ]
        );
    }

    #[test]
    fn add_rejects_unknown_attribute_labels() {
        let request = ActionRequest {
            action: Action::Add,
            targets: vec![PathBuf::from("secret.txt")],
            chattr_attrs: Some("encrypted,unknown".to_string()),
        };
        assert!(
            action_to_args(&request)
                .expect_err("unknown attribute")
                .to_string()
                .contains("unsupported add attribute: unknown")
        );
    }

    #[test]
    fn destroy_forces_only_after_the_application_confirmation() {
        let request = ActionRequest {
            action: Action::Destroy,
            targets: vec![PathBuf::from("old.txt")],
            chattr_attrs: None,
        };
        assert_eq!(
            action_to_args(&request).expect("destroy args"),
            vec![
                os("destroy"),
                os("--force"),
                os("--no-tty"),
                os("--"),
                os("old.txt"),
            ]
        );
    }

    #[test]
    fn new_action_arguments_follow_the_feature_contract() {
        let targets = vec![
            PathBuf::from("/source/file with spaces"),
            PathBuf::from("/source/second"),
        ];
        let cases = [
            (
                Action::Verify,
                vec![
                    os("verify"),
                    os("--"),
                    os("/source/file with spaces"),
                    os("/source/second"),
                ],
            ),
            (
                Action::Cat,
                vec![os("cat"), os("--"), os("/source/file with spaces")],
            ),
            (Action::Ignored, vec![os("ignored")]),
            (
                Action::DumpConfig,
                vec![os("dump-config"), os("--format=json")],
            ),
            (
                Action::DiffAll,
                vec![
                    os("diff"),
                    os("--no-pager"),
                    os("--use-builtin-diff"),
                    os("--color=true"),
                ],
            ),
            (
                Action::GitStatus,
                vec![
                    os("git"),
                    os("--"),
                    os("--no-pager"),
                    os("status"),
                    os("--short"),
                    os("--branch"),
                ],
            ),
            (
                Action::GitDiff,
                vec![
                    os("git"),
                    os("--"),
                    os("--no-pager"),
                    os("diff"),
                    os("--no-ext-diff"),
                    os("--no-color"),
                ],
            ),
            (
                Action::GitLog,
                vec![
                    os("git"),
                    os("--"),
                    os("--no-pager"),
                    os("log"),
                    os("--oneline"),
                    os("--decorate"),
                    os("-n"),
                    os("30"),
                ],
            ),
            (Action::GitPull, vec![os("update"), os("--apply=false")]),
            (Action::GitPush, vec![os("git"), os("--"), os("push")]),
            (Action::GitCommit, vec![os("git"), os("--"), os("commit")]),
            (
                Action::GitStage,
                vec![
                    os("git"),
                    os("--"),
                    os("add"),
                    os("--"),
                    os("/source/file with spaces"),
                    os("/source/second"),
                ],
            ),
            (Action::ReAddAll, vec![os("re-add")]),
            (Action::Init, vec![os("init")]),
            (
                Action::AddDirectory,
                vec![
                    os("add"),
                    os("--recursive"),
                    os("--"),
                    os("/source/file with spaces"),
                ],
            ),
            (
                Action::PreviewAddDirectory,
                vec![
                    os("add"),
                    os("--dry-run"),
                    os("--verbose"),
                    os("--recursive"),
                    os("--"),
                    os("/source/file with spaces"),
                ],
            ),
        ];

        for (action, expected) in cases {
            let request = ActionRequest {
                action,
                targets: if matches!(
                    action,
                    Action::Ignored
                        | Action::DumpConfig
                        | Action::DiffAll
                        | Action::GitStatus
                        | Action::GitDiff
                        | Action::GitLog
                        | Action::GitPull
                        | Action::GitPush
                        | Action::GitCommit
                        | Action::ReAddAll
                        | Action::Init
                ) {
                    Vec::new()
                } else {
                    targets.clone()
                },
                chattr_attrs: None,
            };
            let actual = action_to_args(&request)
                .unwrap_or_else(|error| panic!("{}: {error}", action.label()));
            assert_eq!(actual, expected);
        }

        let verify_all = ActionRequest {
            action: Action::Verify,
            targets: Vec::new(),
            chattr_attrs: None,
        };
        assert_eq!(
            action_to_args(&verify_all).expect("verify all"),
            vec![os("verify")]
        );

        let diff_all = ActionRequest {
            action: Action::DiffAll,
            targets: Vec::new(),
            chattr_attrs: None,
        };
        let background_diff = background_action_args(action_to_args(&diff_all).expect("diff all"));
        assert!(background_diff.iter().any(|arg| arg == "--color=true"));
        assert!(!background_diff.iter().any(|arg| arg == "--color=false"));

        let edit_source = ActionRequest {
            action: Action::EditSource,
            targets,
            chattr_attrs: None,
        };
        assert!(
            action_to_args(&edit_source)
                .expect_err("internal foreground action")
                .to_string()
                .contains("foreground editor")
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

        let _guard = script_guard();
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
case "$*" in
  *managed*) printf '.zshrc\0' ;;
  *) printf ' A .zshrc\n' ;;
esac
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
    fn destination_for_target_always_uses_resolved_session_destination() {
        let client = ShellChezmoiClient {
            home_dir: PathBuf::from("/tmp/home"),
            working_dir: PathBuf::from("/tmp/work"),
            ..ShellChezmoiClient::default()
        };

        let got = client.destination_for_targets(&[PathBuf::from("/tmp/home/.zshrc")]);
        assert_eq!(got, Path::new("/tmp/home"));
        let got = client.destination_for_targets(&[PathBuf::from("/tmp/work/.zshrc")]);
        assert_eq!(got, Path::new("/tmp/home"));
    }

    #[test]
    fn read_temp_file_limited_truncates_large_output() {
        let mut file = tempfile::tempfile().expect("temp file");
        std::io::Write::write_all(&mut file, "a".repeat(100).as_bytes()).expect("write");
        let result = read_temp_file_limited(file, 10, false).expect("read limited");

        assert!(result.truncated);
        assert!(result.text.contains("output truncated"));
    }

    #[test]
    fn strict_path_output_rejects_invalid_utf8() {
        let mut file = tempfile::tempfile().expect("temp file");
        file.write_all(b"bad\xff")
            .expect("write invalid path bytes");

        let error = read_temp_file_limited(file, 64, true).expect_err("invalid path must fail");

        assert!(error.to_string().contains("not valid UTF-8"));
    }

    #[cfg(unix)]
    #[test]
    fn owned_child_drop_kills_and_reaps_after_post_spawn_error() {
        use std::os::unix::fs::PermissionsExt;
        use std::os::unix::process::CommandExt;

        let _guard = script_guard();
        let temp = tempfile::tempdir().expect("tempdir");
        let binary = temp.path().join("slow-child");
        let pid_file = temp.path().join("pid");
        std::fs::write(
            &binary,
            format!(
                "#!/bin/sh\nprintf '%s' \"$$\" > '{}'\nexec sleep 30\n",
                pid_file.display()
            ),
        )
        .expect("write child");
        std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o755))
            .expect("make executable");

        let error = (|| -> Result<()> {
            let mut command = Command::new(&binary);
            command.process_group(0);
            let _child = OwnedChild::new(command.spawn().expect("spawn child"));
            let deadline = Instant::now() + Duration::from_secs(1);
            while !pid_file.exists() && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(5));
            }
            bail!("injected error after spawn")
        })()
        .expect_err("injected error");
        assert!(error.to_string().contains("injected error"));

        let pid = std::fs::read_to_string(pid_file).expect("child pid");
        let alive = Command::new("kill")
            .args(["-0", &pid])
            .output()
            .expect("check child");
        assert!(!alive.status.success(), "owned child was not reaped");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn cancellation_kills_and_reaps_the_owned_process_tree() {
        use std::os::unix::fs::PermissionsExt;

        let _guard = script_guard();
        let temp = tempfile::tempdir().expect("tempdir");
        let binary = temp.path().join("fake-chezmoi");
        let pids = temp.path().join("pids");
        let script = format!(
            "#!/bin/sh\nsleep 30 &\nprintf '%s %s' \"$$\" \"$!\" > '{}'\nwait\n",
            pids.display()
        );
        std::fs::write(&binary, script).expect("write fake");
        std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o755))
            .expect("make executable");
        let client = ShellChezmoiClient::new(
            binary.to_string_lossy(),
            temp.path().to_path_buf(),
            temp.path().to_path_buf(),
            None,
        );
        let worker_client = client.clone();
        let worker = std::thread::spawn(move || worker_client.status());

        let deadline = Instant::now() + Duration::from_secs(2);
        while !pids.exists() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        let ids = std::fs::read_to_string(&pids).expect("fake PIDs");
        let ids: Vec<_> = ids.split_whitespace().collect();

        let started = Instant::now();
        client.cancel_pending();
        let error = worker.join().expect("worker").expect_err("cancelled");
        assert!(error.to_string().contains("cancelled"));
        assert!(started.elapsed() < Duration::from_secs(2));

        let deadline = Instant::now() + Duration::from_secs(1);
        for (index, pid) in ids.into_iter().enumerate() {
            while Instant::now() < deadline {
                let stat = std::fs::read_to_string(format!("/proc/{pid}/stat"));
                let stopped = match stat {
                    Err(err)
                        if err.kind() == std::io::ErrorKind::NotFound
                            || err.raw_os_error() == Some(3) =>
                    {
                        true
                    }
                    Ok(stat) => {
                        index == 0
                            || stat
                                .rsplit_once(") ")
                                .is_some_and(|(_, rest)| rest.starts_with('Z'))
                    }
                    Err(err) => panic!("cannot inspect process {pid}: {err}"),
                };
                if stopped {
                    break;
                }
                std::thread::sleep(Duration::from_millis(5));
            }
            let stat = std::fs::read_to_string(format!("/proc/{pid}/stat"));
            if index == 0 {
                assert!(
                    stat.is_err(),
                    "renderer process {pid} survived cancellation"
                );
            } else {
                assert!(
                    stat.is_err()
                        || stat.as_deref().is_ok_and(|stat| stat
                            .rsplit_once(") ")
                            .is_some_and(|(_, rest)| rest.starts_with('Z'))),
                    "descendant process {pid} survived cancellation"
                );
            }
        }
        assert!(
            client
                .status()
                .expect_err("sticky cancellation")
                .to_string()
                .contains("cancelled before start")
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn template_output_limit_terminates_children_and_reaps_renderer() {
        use std::os::unix::fs::PermissionsExt;

        let _guard = script_guard();
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
                true,
                None,
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
        let _guard = script_guard();
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
                true,
                None,
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
        let _guard = script_guard();
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
                true,
                None,
            )
            .expect("render without pipe deadlock");

        ensure_complete_success(&result, "chezmoi execute-template").expect("successful rendering");
        assert!(result.stdout.starts_with(&"\0".repeat(128 * 1024)));
        assert!(result.stdout.ends_with(&input));
    }

    #[cfg(unix)]
    #[test]
    fn template_that_exits_without_reading_stdin_reports_failure() {
        let _guard = script_guard();
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
                true,
                None,
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

        let result = run_command_with_limits_mode(
            cmd,
            &[OsString::from("stdout-test")],
            CommandLimits {
                timeout: Duration::from_secs(1),
                max_stdout_bytes: 1024,
                max_stderr_bytes: 1024,
            },
            None,
            None,
            false,
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

        let result = run_command_with_limits_mode(
            cmd,
            &[OsString::from("sleep-test")],
            CommandLimits {
                timeout: Duration::from_millis(100),
                max_stdout_bytes: 1024,
                max_stderr_bytes: 1024,
            },
            None,
            None,
            false,
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

        let result = run_command_with_limits_mode(
            cmd,
            &[OsString::from("output-limit-test")],
            CommandLimits {
                timeout: Duration::from_secs(5),
                max_stdout_bytes: 1024,
                max_stderr_bytes: 1024,
            },
            None,
            None,
            false,
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
