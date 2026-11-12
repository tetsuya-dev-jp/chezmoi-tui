use crate::app::App;
use crate::domain::ActionRequest;
use anyhow::{Context, Result};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;
#[cfg(unix)]
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};
use std::process::Command;

#[cfg(target_os = "linux")]
const O_NONBLOCK: i32 = 0x800;
#[cfg(target_os = "macos")]
const O_NONBLOCK: i32 = 0x4;

const MAX_IGNORE_BYTES: u64 = 4 * 1024 * 1024;

#[cfg(unix)]
const LOCK_EX: std::ffi::c_int = 2;
#[cfg(unix)]
const LOCK_NB: std::ffi::c_int = 4;
#[cfg(unix)]
const LOCK_UN: std::ffi::c_int = 8;
#[cfg(unix)]
unsafe extern "C" {
    fn flock(fd: std::ffi::c_int, operation: std::ffi::c_int) -> std::ffi::c_int;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum IgnorePatternMode {
    Auto,
    Exact,
    Children,
    Recursive,
    GlobalName,
}

impl IgnorePatternMode {
    pub(crate) const ALL: [IgnorePatternMode; 5] = [
        IgnorePatternMode::Auto,
        IgnorePatternMode::Exact,
        IgnorePatternMode::Children,
        IgnorePatternMode::Recursive,
        IgnorePatternMode::GlobalName,
    ];

    pub(crate) fn tag(self) -> &'static str {
        match self {
            IgnorePatternMode::Auto => "auto",
            IgnorePatternMode::Exact => "exact",
            IgnorePatternMode::Children => "children",
            IgnorePatternMode::Recursive => "recursive",
            IgnorePatternMode::GlobalName => "global-name",
        }
    }

    pub(crate) fn from_tag(tag: &str) -> Option<Self> {
        match tag {
            "auto" => Some(IgnorePatternMode::Auto),
            "exact" => Some(IgnorePatternMode::Exact),
            "children" => Some(IgnorePatternMode::Children),
            "recursive" => Some(IgnorePatternMode::Recursive),
            "global-name" => Some(IgnorePatternMode::GlobalName),
            _ => None,
        }
    }

    pub(crate) fn from_index(index: usize) -> Self {
        *Self::ALL.get(index).unwrap_or(&IgnorePatternMode::Auto)
    }
}

pub(crate) fn run_internal_ignore_action(app: &mut App, request: &ActionRequest) -> Result<()> {
    let target = request
        .targets
        .first()
        .map(PathBuf::as_path)
        .context("ignore requires a target file or directory")?;

    let is_dir = fs::symlink_metadata(target)
        .with_context(|| format!("failed to stat ignore target: {}", target.display()))?
        .file_type()
        .is_dir();

    let home_dir = app.home_dir.clone();
    let mode = request
        .chattr_attrs
        .as_deref()
        .and_then(IgnorePatternMode::from_tag)
        .unwrap_or(IgnorePatternMode::Auto);
    let pattern = build_ignore_pattern(target, is_dir, &home_dir, mode)?;
    let source_root = app
        .effective_source_dir()
        .context("source root still loading; refresh/wait")?;
    let ignore_path = chezmoi_ignore_path_with_source(Some(source_root))?;

    let already_exists = append_unique_line(&ignore_path, &pattern)?;
    if already_exists {
        app.log(format!("ignore pattern already exists: {pattern}"));
    } else {
        app.log(format!("ignore pattern added: {pattern}"));
    }

    Ok(())
}

fn build_ignore_pattern(
    target: &Path,
    is_dir: bool,
    home_dir: &Path,
    mode: IgnorePatternMode,
) -> Result<String> {
    if mode == IgnorePatternMode::GlobalName {
        let name = target.file_name().with_context(|| {
            format!("cannot infer ignore name from target: {}", target.display())
        })?;
        let escaped = escape_ignore_glob_component(name)?;
        let pattern = if is_dir {
            format!("**/{escaped}/**")
        } else {
            format!("**/{escaped}")
        };
        return Ok(ignore_template_line(&pattern));
    }

    let relative = target
        .strip_prefix(home_dir)
        .with_context(|| {
            format!(
                "ignore target is outside home directory: target={} home={}",
                target.display(),
                home_dir.display()
            )
        })?
        .to_path_buf();

    let components = relative
        .components()
        .map(|component| match component {
            std::path::Component::Normal(name) => escape_ignore_glob_component(name),
            _ => anyhow::bail!(
                "ignore target contains a non-normal path component: {}",
                target.display()
            ),
        })
        .collect::<Result<Vec<_>>>()?;
    if components.is_empty() {
        anyhow::bail!("ignore target resolved to an empty pattern");
    }
    let mut pattern = components.join("/");

    let suffix = match mode {
        IgnorePatternMode::Exact | IgnorePatternMode::GlobalName => "",
        IgnorePatternMode::Children => {
            if is_dir {
                "/*"
            } else {
                ""
            }
        }
        IgnorePatternMode::Auto | IgnorePatternMode::Recursive => {
            if is_dir {
                "/**"
            } else {
                ""
            }
        }
    };

    if !suffix.is_empty() {
        pattern = pattern.trim_end_matches('/').to_string();
        pattern.push_str(suffix);
    }

    Ok(ignore_template_line(&pattern))
}

fn escape_ignore_glob_component(name: &std::ffi::OsStr) -> Result<String> {
    let name = name
        .to_str()
        .with_context(|| format!("ignore name is not valid UTF-8: {name:?}"))?;
    if let Some(character) = name
        .chars()
        .find(|character| matches!(character, '\n' | '\r' | '\0'))
    {
        anyhow::bail!(
            "ignore name contains unsupported control character {:?}: {:?}",
            character,
            name
        );
    }

    let mut escaped = String::with_capacity(name.len());
    for ch in name.chars() {
        match ch {
            '\\' | '*' | '?' | '[' | ']' | '{' | '}' | '!' => {
                escaped.push('\\');
                escaped.push(ch);
            }
            // Chezmoi strips whitespace followed by '#' as a comment.
            '#' => escaped.push_str("[#]"),
            // TrimSpace runs after rendering, so hide whitespace in a class.
            ch if ch.is_whitespace() => escaped.push_str(&format!("[{ch}]")),
            _ => escaped.push(ch),
        }
    }
    Ok(escaped)
}

fn ignore_template_line(pattern: &str) -> String {
    let quoted = serde_json::to_string(pattern).expect("serializing a string cannot fail");
    format!("{{{{ {quoted} }}}}")
}

pub(crate) fn chezmoi_ignore_path_with_source(
    source_dir: Option<&Path>,
) -> Result<std::path::PathBuf> {
    let source_dir = match source_dir {
        Some(source_dir) => source_dir.to_path_buf(),
        None => {
            let output = Command::new("chezmoi")
                .arg("source-path")
                .output()
                .context("failed to execute chezmoi source-path")?;
            if !output.status.success() {
                anyhow::bail!(
                    "chezmoi source-path failed: {}",
                    String::from_utf8_lossy(&output.stderr).trim()
                );
            }

            let source_dir = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if source_dir.is_empty() {
                anyhow::bail!("chezmoi source-path returned empty output");
            }
            std::path::PathBuf::from(source_dir)
        }
    };

    let plain = source_dir.join(".chezmoiignore");
    let template = source_dir.join(".chezmoiignore.tmpl");
    if plain.try_exists()? || !template.try_exists()? {
        Ok(plain)
    } else {
        Ok(template)
    }
}

fn append_unique_line(path: &Path, line: &str) -> Result<bool> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }

    match fs::metadata(path) {
        Ok(metadata) if !metadata.is_file() => {
            anyhow::bail!("ignore file {} is not a regular file", path.display());
        }
        Ok(_) => {}
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => {
            return Err(err)
                .with_context(|| format!("failed to inspect ignore file {}", path.display()));
        }
    }

    let mut options = OpenOptions::new();
    options.create(true).read(true).append(true);
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    options.custom_flags(O_NONBLOCK);
    let mut file = options
        .open(path)
        .with_context(|| format!("failed to open {} for update", path.display()))?;
    if !file
        .metadata()
        .with_context(|| format!("failed to inspect ignore file {}", path.display()))?
        .is_file()
    {
        anyhow::bail!("ignore file {} is not a regular file", path.display());
    }
    let _lock = lock_ignore_file(&file).with_context(|| {
        format!(
            "failed to lock {} for ignore update; another process may be updating it",
            path.display()
        )
    })?;

    let mut existing = String::new();
    (&mut file)
        .take(MAX_IGNORE_BYTES + 1)
        .read_to_string(&mut existing)
        .with_context(|| format!("failed to read {}", path.display()))?;
    if existing.len() as u64 > MAX_IGNORE_BYTES {
        anyhow::bail!(
            "ignore file {} exceeds the 4 MiB read limit",
            path.display()
        );
    }
    if existing.lines().any(|entry| entry.trim() == line) {
        return Ok(true);
    }

    let needs_separator = !existing.is_empty() && !existing.ends_with('\n');
    let mut append = String::with_capacity(line.len() + 2);
    if needs_separator {
        append.push('\n');
    }
    append.push_str(line);
    append.push('\n');
    file.write_all(append.as_bytes())
        .with_context(|| format!("failed to append to {}", path.display()))?;

    Ok(false)
}

#[cfg(unix)]
struct IgnoreFileLock {
    fd: std::os::unix::io::RawFd,
}

#[cfg(unix)]
impl Drop for IgnoreFileLock {
    fn drop(&mut self) {
        unsafe {
            flock(self.fd, LOCK_UN);
        }
    }
}

#[cfg(unix)]
fn lock_ignore_file(file: &File) -> std::io::Result<IgnoreFileLock> {
    if unsafe { flock(file.as_raw_fd(), LOCK_EX | LOCK_NB) } == 0 {
        Ok(IgnoreFileLock {
            fd: file.as_raw_fd(),
        })
    } else {
        Err(std::io::Error::last_os_error())
    }
}

#[cfg(not(unix))]
fn lock_ignore_file(_file: &File) -> std::io::Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::AppConfig;
    use crate::domain::{Action, ActionRequest};
    use crate::ignored_scan::IgnoreMatcher;

    fn rendered_pattern(source: &str) -> String {
        let quoted = source
            .strip_prefix("{{ ")
            .and_then(|source| source.strip_suffix(" }}"))
            .expect("generated template expression");
        serde_json::from_str(quoted).expect("quoted pattern")
    }

    #[test]
    fn build_ignore_pattern_uses_home_relative_path_when_target_is_under_home() {
        let home = Path::new("/home/tetsuya");
        let target = Path::new("/home/tetsuya/dev/chezmoi-tui/.git");
        let got = build_ignore_pattern(target, true, home, IgnorePatternMode::Auto)
            .expect("build ignore pattern");
        assert_eq!(got, r#"{{ "dev/chezmoi-tui/.git/**" }}"#);
    }

    #[test]
    fn build_ignore_pattern_fails_for_path_outside_home() {
        let home = Path::new("/home/tetsuya");
        let target = Path::new("/tmp/chezmoi-tui/.cache");
        assert!(build_ignore_pattern(target, true, home, IgnorePatternMode::Auto).is_err());

        let traversal = Path::new("/home/tetsuya/../outside/.cache");
        assert!(build_ignore_pattern(traversal, true, home, IgnorePatternMode::Auto).is_err());
    }

    #[test]
    fn build_ignore_pattern_mode_children() {
        let home = Path::new("/home/tetsuya");
        let target = Path::new("/home/tetsuya/dev/chezmoi-tui/.cache");
        let got = build_ignore_pattern(target, true, home, IgnorePatternMode::Children)
            .expect("build ignore pattern");
        assert_eq!(got, r#"{{ "dev/chezmoi-tui/.cache/*" }}"#);
    }

    #[test]
    fn build_ignore_pattern_mode_exact_for_directory() {
        let home = Path::new("/home/tetsuya");
        let target = Path::new("/home/tetsuya/dev/chezmoi-tui/.cache");
        let got = build_ignore_pattern(target, true, home, IgnorePatternMode::Exact)
            .expect("build ignore pattern");
        assert_eq!(got, r#"{{ "dev/chezmoi-tui/.cache" }}"#);
    }

    #[test]
    fn build_ignore_pattern_mode_global_name_for_directory() {
        let home = Path::new("/home/tetsuya");
        let target = Path::new("/home/tetsuya/dev/chezmoi-tui/.git");
        let got = build_ignore_pattern(target, true, home, IgnorePatternMode::GlobalName)
            .expect("build ignore pattern");
        assert_eq!(got, r#"{{ "**/.git/**" }}"#);
    }

    #[test]
    fn build_ignore_pattern_mode_global_name_for_file() {
        let home = Path::new("/home/tetsuya");
        let target = Path::new("/home/tetsuya/dev/chezmoi-tui/.DS_Store");
        let got = build_ignore_pattern(target, false, home, IgnorePatternMode::GlobalName)
            .expect("build ignore pattern");
        assert_eq!(got, r#"{{ "**/.DS_Store" }}"#);
    }

    #[test]
    fn build_ignore_pattern_mode_global_name_escapes_glob_tokens() {
        let home = Path::new("/home/tetsuya");
        let target = Path::new("/home/tetsuya/dev/chezmoi-tui/[ab]*?.txt");
        let got = build_ignore_pattern(target, false, home, IgnorePatternMode::GlobalName)
            .expect("build ignore pattern");
        assert_eq!(got, r#"{{ "**/\\[ab\\]\\*\\?.txt" }}"#);
    }

    #[test]
    fn all_path_modes_escape_literal_glob_comment_and_template_syntax() {
        let home = Path::new("/home/tetsuya");
        let name = "!a*b?[c]{d}\\e #{{evil}}";
        let target = home.join("parent?[x]").join(name);
        let escaped_name = r"\!a\*b\?\[c\]\{d\}\\e[ ][#]\{\{evil\}\}";
        let escaped_parent = r"parent\?\[x\]";

        for (mode, suffix) in [
            (IgnorePatternMode::Exact, ""),
            (IgnorePatternMode::Auto, "/**"),
            (IgnorePatternMode::Children, "/*"),
            (IgnorePatternMode::Recursive, "/**"),
        ] {
            let got = build_ignore_pattern(&target, true, home, mode).expect("build pattern");
            assert_eq!(
                rendered_pattern(&got),
                format!("{escaped_parent}/{escaped_name}{suffix}")
            );
        }

        let got = build_ignore_pattern(&target, true, home, IgnorePatternMode::GlobalName)
            .expect("build global pattern");
        assert_eq!(rendered_pattern(&got), format!("**/{escaped_name}/**"));

        let matcher = IgnoreMatcher::from_patterns([build_ignore_pattern(
            &target,
            false,
            home,
            IgnorePatternMode::Exact,
        )
        .map(|line| rendered_pattern(&line))
        .expect("build exact pattern")])
        .expect("literal pattern compiles");
        assert!(matcher.is_ignored(&format!("parent?[x]/{name}")));
        assert!(!matcher.is_ignored("parentX/other"));
    }

    #[test]
    fn ignore_names_reject_line_breaks_and_nul() {
        for name in ["line\nbreak", "line\rbreak", "nul\0byte"] {
            let home = Path::new("/home/tetsuya");
            let target = home.join(name);
            let err = build_ignore_pattern(&target, false, home, IgnorePatternMode::Exact)
                .expect_err("unsafe name fails");
            assert!(format!("{err:#}").contains("unsupported control character"));
        }
    }

    #[test]
    fn ignore_mode_from_tag_parses_known_values() {
        assert_eq!(
            IgnorePatternMode::from_tag("recursive"),
            Some(IgnorePatternMode::Recursive)
        );
        assert_eq!(
            IgnorePatternMode::from_tag("global-name"),
            Some(IgnorePatternMode::GlobalName)
        );
        assert_eq!(IgnorePatternMode::from_tag("unknown"), None);
    }

    #[test]
    fn run_internal_ignore_action_uses_configured_destination_base() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().to_path_buf();
        let destination = root.join("home");
        let source = root.join("source");
        let target = destination.join("project/.cache");
        std::fs::create_dir_all(&target).expect("create target dir");
        std::fs::create_dir_all(&source).expect("create source dir");

        let config = AppConfig {
            destination_dir: Some(destination.clone()),
            source_dir: Some(source.clone()),
            working_dir: root.join("work"),
            ..AppConfig::default()
        };
        let mut app = App::new(config);
        app.apply_source_refresh(Some(source.clone()), Vec::new());
        let request = ActionRequest {
            action: Action::Ignore,
            targets: vec![target],
            chattr_attrs: Some(IgnorePatternMode::Auto.tag().to_string()),
        };

        run_internal_ignore_action(&mut app, &request).expect("ignore action");

        let ignore = std::fs::read_to_string(source.join(".chezmoiignore")).expect("read ignore");
        assert_eq!(ignore, "{{ \"project/.cache/**\" }}\n");
    }

    #[test]
    fn run_internal_ignore_action_rejects_destination_relative_mode_outside_destination() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().to_path_buf();
        let destination = root.join("home");
        let source = root.join("source");
        let target = root.join("outside/.cache");
        std::fs::create_dir_all(&target).expect("create target dir");
        std::fs::create_dir_all(&source).expect("create source dir");

        let config = AppConfig {
            destination_dir: Some(destination.clone()),
            source_dir: Some(source.clone()),
            working_dir: root.join("work"),
            ..AppConfig::default()
        };
        let mut app = App::new(config);
        app.apply_source_refresh(Some(source.clone()), Vec::new());
        let request = ActionRequest {
            action: Action::Ignore,
            targets: vec![target.clone()],
            chattr_attrs: Some(IgnorePatternMode::Exact.tag().to_string()),
        };

        let err = run_internal_ignore_action(&mut app, &request).expect_err("outside target fails");
        let message = format!("{err:#}");
        assert!(message.contains(&target.display().to_string()));
        assert!(message.contains(&destination.display().to_string()));
    }

    #[test]
    fn run_internal_ignore_action_allows_global_name_outside_destination() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().to_path_buf();
        let destination = root.join("home");
        let source = root.join("source");
        let target = root.join("outside/.cache");
        std::fs::create_dir_all(&target).expect("create target dir");
        std::fs::create_dir_all(&source).expect("create source dir");

        let config = AppConfig {
            destination_dir: Some(destination),
            source_dir: Some(source.clone()),
            working_dir: root.join("work"),
            ..AppConfig::default()
        };
        let mut app = App::new(config);
        app.apply_source_refresh(Some(source.clone()), Vec::new());
        let request = ActionRequest {
            action: Action::Ignore,
            targets: vec![target],
            chattr_attrs: Some(IgnorePatternMode::GlobalName.tag().to_string()),
        };

        run_internal_ignore_action(&mut app, &request).expect("global name ignore action");

        let ignore = std::fs::read_to_string(source.join(".chezmoiignore")).expect("read ignore");
        assert_eq!(ignore, "{{ \"**/.cache/**\" }}\n");
    }

    #[test]
    fn run_internal_ignore_action_appends_to_existing_root_template() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().to_path_buf();
        let destination = root.join("home");
        let source = root.join("source");
        let target = destination.join("cache");
        std::fs::create_dir_all(&target).expect("create target dir");
        std::fs::create_dir_all(&source).expect("create source dir");
        let ignore_template = source.join(".chezmoiignore.tmpl");
        let user_content = "{{ if eq .chezmoi.os \"linux\" }}\n";
        std::fs::write(&ignore_template, user_content).expect("write user template");

        let config = AppConfig {
            destination_dir: Some(destination),
            source_dir: Some(source.clone()),
            working_dir: root.join("work"),
            ..AppConfig::default()
        };
        let mut app = App::new(config);
        app.apply_source_refresh(Some(source.clone()), Vec::new());
        let request = ActionRequest {
            action: Action::Ignore,
            targets: vec![target],
            chattr_attrs: Some(IgnorePatternMode::Exact.tag().to_string()),
        };

        run_internal_ignore_action(&mut app, &request).expect("first ignore action");
        run_internal_ignore_action(&mut app, &request).expect("duplicate ignore action");

        assert_eq!(
            std::fs::read_to_string(ignore_template).expect("read ignore template"),
            format!("{user_content}{{{{ \"cache\" }}}}\n")
        );
        assert!(!source.join(".chezmoiignore").exists());
    }

    #[test]
    fn existing_root_ignore_file_prefers_plain_then_template() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path();
        assert_eq!(
            chezmoi_ignore_path_with_source(Some(root)).expect("new ignore path"),
            root.join(".chezmoiignore")
        );

        std::fs::write(root.join(".chezmoiignore.tmpl"), "").expect("write template");
        assert_eq!(
            chezmoi_ignore_path_with_source(Some(root)).expect("template ignore path"),
            root.join(".chezmoiignore.tmpl")
        );

        std::fs::write(root.join(".chezmoiignore"), "").expect("write plain");
        assert_eq!(
            chezmoi_ignore_path_with_source(Some(root)).expect("plain ignore path"),
            root.join(".chezmoiignore")
        );
    }

    #[test]
    fn append_unique_line_appends_once_and_avoids_duplicates() {
        let temp_f = tempfile::tempdir().expect("tempdir");
        let file = temp_f.path().join("file_test");
        std::fs::write(&file, "a").expect("write seed");

        let first = append_unique_line(&file, "b").expect("append first");
        assert!(!first);
        assert_eq!(std::fs::read_to_string(&file).expect("read file"), "a\nb\n");

        let second = append_unique_line(&file, "b").expect("append duplicate");
        assert!(second);
        assert_eq!(std::fs::read_to_string(&file).expect("read file"), "a\nb\n");

        let _ = std::fs::remove_file(file);
    }

    #[cfg(unix)]
    #[test]
    fn append_unique_line_rejects_fifo_without_blocking() {
        let temp_f = tempfile::tempdir().expect("tempdir");
        let path = temp_f.path().join("ignore.fifo");
        let status = std::process::Command::new("mkfifo")
            .arg(&path)
            .status()
            .expect("run mkfifo");
        assert!(status.success());

        let err = append_unique_line(&path, "pattern").expect_err("FIFO must be rejected");
        assert!(format!("{err:#}").contains("not a regular file"));
    }

    #[cfg(unix)]
    #[test]
    fn append_unique_line_follows_regular_symlink() {
        let temp_f = tempfile::tempdir().expect("tempdir");
        let target = temp_f.path().join("target");
        let link = temp_f.path().join("ignore");
        std::fs::write(&target, "original").expect("write target");
        std::os::unix::fs::symlink(&target, &link).expect("create symlink");

        append_unique_line(&link, "pattern").expect("append through symlink");

        assert_eq!(
            std::fs::read_to_string(target).expect("read target"),
            "original\npattern\n"
        );
    }

    #[test]
    fn append_unique_line_rejects_oversized_file_before_append() {
        let temp_f = tempfile::tempdir().expect("tempdir");
        let path = temp_f.path().join("ignore");
        let original = vec![b'a'; MAX_IGNORE_BYTES as usize + 1];
        std::fs::write(&path, &original).expect("write oversized file");

        let err = append_unique_line(&path, "pattern").expect_err("oversized file fails");
        assert!(format!("{err:#}").contains("exceeds the 4 MiB read limit"));
        assert_eq!(std::fs::read(path).expect("read unchanged file"), original);
    }

    #[cfg(unix)]
    #[test]
    fn append_unique_line_lock_contention_leaves_original_intact() {
        let temp_f = tempfile::tempdir().expect("tempdir");
        let path = temp_f.path().join("file_test");
        std::fs::write(&path, "original").expect("write seed");

        let locked = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&path)
            .expect("open locked file");
        let lock = lock_ignore_file(&locked).expect("lock seed");

        let err = append_unique_line(&path, "contender").expect_err("locked append fails");
        assert!(format!("{err:#}").contains("another process may be updating"));
        assert_eq!(
            std::fs::read_to_string(&path).expect("read original"),
            "original"
        );

        drop(lock);
        drop(locked);
        append_unique_line(&path, "contender").expect("append after unlock");
        assert_eq!(
            std::fs::read_to_string(&path).expect("read appended"),
            "original\ncontender\n"
        );
    }
}
