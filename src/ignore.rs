use crate::app::App;
use crate::domain::ActionRequest;
use anyhow::{Context, Result};
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::Path;
use std::process::Command;

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
        .target
        .as_deref()
        .context("ignore requires a target file or directory")?;

    let is_dir = fs::symlink_metadata(target)
        .with_context(|| format!("failed to stat ignore target: {}", target.display()))?
        .file_type()
        .is_dir();

    let working_dir = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let home_dir = dirs::home_dir().unwrap_or_else(|| working_dir.clone());
    let mode = request
        .chattr_attrs
        .as_deref()
        .and_then(IgnorePatternMode::from_tag)
        .unwrap_or(IgnorePatternMode::Auto);
    let pattern = build_ignore_pattern(target, is_dir, &home_dir, mode)?;
    let ignore_path = chezmoi_ignore_path()?;

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
        let name = target
            .file_name()
            .and_then(|name| name.to_str())
            .with_context(|| {
                format!("cannot infer ignore name from target: {}", target.display())
            })?;
        let escaped = escape_ignore_glob_component(name);
        return Ok(if is_dir {
            format!("**/{escaped}/**")
        } else {
            format!("**/{escaped}")
        });
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

    let mut pattern = normalize_ignore_path(&relative);
    if pattern.is_empty() || pattern == "." {
        anyhow::bail!("ignore target resolved to an empty pattern");
    }

    let suffix = match mode {
        IgnorePatternMode::Auto => {
            if is_dir {
                "/**"
            } else {
                ""
            }
        }
        IgnorePatternMode::Exact => "",
        IgnorePatternMode::Children => {
            if is_dir {
                "/*"
            } else {
                ""
            }
        }
        IgnorePatternMode::Recursive => {
            if is_dir {
                "/**"
            } else {
                ""
            }
        }
        IgnorePatternMode::GlobalName => "",
    };

    if !suffix.is_empty() {
        pattern = pattern.trim_end_matches('/').to_string();
        pattern.push_str(suffix);
    }

    Ok(pattern)
}

fn normalize_ignore_path(path: &Path) -> String {
    path.to_string_lossy()
        .replace('\\', "/")
        .trim_start_matches("./")
        .trim_start_matches('/')
        .to_string()
}

fn escape_ignore_glob_component(name: &str) -> String {
    let mut escaped = String::with_capacity(name.len());
    for ch in name.chars() {
        if matches!(ch, '\\' | '/' | '*' | '?' | '[' | ']' | '{' | '}' | '!') {
            escaped.push('\\');
        }
        escaped.push(ch);
    }
    escaped
}

pub(crate) fn chezmoi_ignore_path() -> Result<std::path::PathBuf> {
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

    Ok(std::path::PathBuf::from(source_dir).join(".chezmoiignore"))
}

fn append_unique_line(path: &Path, line: &str) -> Result<bool> {
    let existing = match fs::read_to_string(path) {
        Ok(content) => content,
        Err(err) if err.kind() == io::ErrorKind::NotFound => String::new(),
        Err(err) => {
            return Err(err).with_context(|| format!("failed to read {}", path.display()));
        }
    };

    if existing.lines().any(|entry| entry.trim() == line) {
        return Ok(true);
    }

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }

    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .with_context(|| format!("failed to open {} for append", path.display()))?;

    if !existing.is_empty() && !existing.ends_with('\n') {
        file.write_all(b"\n")
            .with_context(|| format!("failed to append newline to {}", path.display()))?;
    }
    writeln!(file, "{line}").with_context(|| format!("failed to append to {}", path.display()))?;

    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn build_ignore_pattern_uses_home_relative_path_when_target_is_under_home() {
        let home = Path::new("/home/tetsuya");
        let target = Path::new("/home/tetsuya/dev/chezmoi-tui/.git");
        let got = build_ignore_pattern(target, true, home, IgnorePatternMode::Auto)
            .expect("build ignore pattern");
        assert_eq!(got, "dev/chezmoi-tui/.git/**");
    }

    #[test]
    fn build_ignore_pattern_fails_for_path_outside_home() {
        let home = Path::new("/home/tetsuya");
        let target = Path::new("/tmp/chezmoi-tui/.cache");
        assert!(build_ignore_pattern(target, true, home, IgnorePatternMode::Auto).is_err());
    }

    #[test]
    fn build_ignore_pattern_mode_children() {
        let home = Path::new("/home/tetsuya");
        let target = Path::new("/home/tetsuya/dev/chezmoi-tui/.cache");
        let got = build_ignore_pattern(target, true, home, IgnorePatternMode::Children)
            .expect("build ignore pattern");
        assert_eq!(got, "dev/chezmoi-tui/.cache/*");
    }

    #[test]
    fn build_ignore_pattern_mode_exact_for_directory() {
        let home = Path::new("/home/tetsuya");
        let target = Path::new("/home/tetsuya/dev/chezmoi-tui/.cache");
        let got = build_ignore_pattern(target, true, home, IgnorePatternMode::Exact)
            .expect("build ignore pattern");
        assert_eq!(got, "dev/chezmoi-tui/.cache");
    }

    #[test]
    fn build_ignore_pattern_mode_global_name_for_directory() {
        let home = Path::new("/home/tetsuya");
        let target = Path::new("/home/tetsuya/dev/chezmoi-tui/.git");
        let got = build_ignore_pattern(target, true, home, IgnorePatternMode::GlobalName)
            .expect("build ignore pattern");
        assert_eq!(got, "**/.git/**");
    }

    #[test]
    fn build_ignore_pattern_mode_global_name_for_file() {
        let home = Path::new("/home/tetsuya");
        let target = Path::new("/home/tetsuya/dev/chezmoi-tui/.DS_Store");
        let got = build_ignore_pattern(target, false, home, IgnorePatternMode::GlobalName)
            .expect("build ignore pattern");
        assert_eq!(got, "**/.DS_Store");
    }

    #[test]
    fn build_ignore_pattern_mode_global_name_escapes_glob_tokens() {
        let home = Path::new("/home/tetsuya");
        let target = Path::new("/home/tetsuya/dev/chezmoi-tui/[ab]*?.txt");
        let got = build_ignore_pattern(target, false, home, IgnorePatternMode::GlobalName)
            .expect("build ignore pattern");
        assert_eq!(got, "**/\\[ab\\]\\*\\?.txt");
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
    fn append_unique_line_appends_once_and_avoids_duplicates() {
        let file = std::env::temp_dir().join(format!(
            "chezmoi_tui_ignore_{}_{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("time")
                .as_nanos()
        ));
        std::fs::write(&file, "a").expect("write seed");

        let first = append_unique_line(&file, "b").expect("append first");
        assert!(!first);
        assert_eq!(std::fs::read_to_string(&file).expect("read file"), "a\nb\n");

        let second = append_unique_line(&file, "b").expect("append duplicate");
        assert!(second);
        assert_eq!(std::fs::read_to_string(&file).expect("read file"), "a\nb\n");

        let _ = std::fs::remove_file(file);
    }
}
