//! Integration tests using a fake `chezmoi` binary to verify that
//! `ShellChezmoiClient` passes correct arguments to the underlying command
//! and parses its output correctly.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

use chezmoi_tui::domain::{Action, ActionRequest};
use chezmoi_tui::infra::ChezmoiClient;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn sh_quote(path: &std::path::Path) -> String {
    let raw = path.to_string_lossy();
    format!("'{}'", raw.replace('\'', r"'\''"))
}

/// Write a shell script that logs argv and simulates `chezmoi` subcommands.
fn write_fake_chezmoi(
    dir: &std::path::Path,
    log: &std::path::Path,
    source_dir: &std::path::Path,
) -> PathBuf {
    let bin = dir.join("chezmoi");
    let script = format!(
        r#"#!/usr/bin/env sh
set -eu

log={log}
source_dir={source_dir}

# Log argv: one argument per line so boundary safety can be verified.
printf 'BEGIN\n' >> "$log"
i=0
for arg in "$@"; do
  printf 'arg[%s]=%s\n' "$i" "$arg" >> "$log"
  i=$((i + 1))
done
printf 'END\n' >> "$log"

while [ "$#" -gt 0 ]; do
  case "$1" in
    --destination)
      shift 2
      ;;
    --source)
      shift 2
      ;;
    --)
      shift
      break
      ;;
    *)
      break
      ;;
  esac
done

cmd="${{1:-}}"
if [ "$#" -gt 0 ]; then
  shift
fi

case "$cmd" in
  status)
    printf ' M .zshrc\n'
    ;;
  managed)
    printf '[".zshrc",".config/nvim/init.lua"]\n'
    ;;
  unmanaged)
    printf 'tmp.txt\n'
    ;;
  source-path)
    printf '%s\n' "$source_dir"
    ;;
  diff)
    printf 'diff --git a/.zshrc b/.zshrc\n'
    ;;
  data)
    printf '{{"chezmoi":{{"hostname":"fake"}}}}\n'
    ;;
  doctor)
    printf 'ok\n'
    ;;
  apply|forget|chattr|destroy|add|edit|merge|update|re-add)
    printf 'ran %s\n' "$cmd"
    ;;
  *)
    printf 'unknown command: %s\n' "$cmd" >&2
    exit 2
    ;;
esac
"#,
        log = sh_quote(log),
        source_dir = sh_quote(source_dir),
    );

    fs::write(&bin, script).expect("write fake chezmoi");

    let mut perms = fs::metadata(&bin).expect("metadata").permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&bin, perms).expect("chmod fake chezmoi");

    bin
}

struct FakeChezmoi {
    _temp: tempfile::TempDir,
    bin: PathBuf,
    home: PathBuf,
    work: PathBuf,
    source: PathBuf,
    log: PathBuf,
}

impl FakeChezmoi {
    fn new() -> Self {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path();

        let home = root.join("home");
        let work = root.join("work");
        let source = root.join("source");
        let log = root.join("fake.log");

        fs::create_dir_all(&home).expect("home");
        fs::create_dir_all(&work).expect("work");
        fs::create_dir_all(&source).expect("source");

        let bin = write_fake_chezmoi(root, &log, &source);

        Self {
            _temp: temp,
            bin,
            home,
            work,
            source,
            log,
        }
    }

    fn client(&self) -> chezmoi_tui::infra::ShellChezmoiClient {
        chezmoi_tui::infra::ShellChezmoiClient::new(
            self.bin.to_string_lossy(),
            self.home.clone(),
            self.work.clone(),
            Some(self.source.clone()),
        )
    }

    /// Parse the fake log into a flat list of argument values.
    fn logged_args(&self) -> Vec<String> {
        let content = fs::read_to_string(&self.log).unwrap_or_default();
        let mut args = Vec::new();
        for line in content.lines() {
            if let Some((_key, value)) = line.split_once('=') {
                args.push(value.to_string());
            }
        }
        args
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[test]
fn shell_client_reads_status_from_fake_chezmoi() {
    let fake = FakeChezmoi::new();
    let client = fake.client();
    let status = client.status().expect("status");

    assert_eq!(status.len(), 1);
    assert_eq!(status[0].path, PathBuf::from(".zshrc"));
}

#[test]
fn shell_client_reads_managed_from_fake_chezmoi() {
    let fake = FakeChezmoi::new();
    let client = fake.client();
    let managed = client.managed().expect("managed");

    assert!(managed.contains(&PathBuf::from(".zshrc")));
    assert!(managed.contains(&PathBuf::from(".config/nvim/init.lua")));
}

#[test]
fn shell_client_reads_diff_from_fake_chezmoi() {
    let fake = FakeChezmoi::new();
    let client = fake.client();
    let diff = client.diff(None).expect("diff");

    assert!(diff.text.contains("diff --git"));
}

#[test]
fn shell_client_passes_option_like_targets_after_double_dash() {
    for target_name in ["--help", "-n"] {
        let fake = FakeChezmoi::new();
        let request = ActionRequest {
            action: Action::Forget,
            target: Some(PathBuf::from(target_name)),
            chattr_attrs: None,
        };

        let client = fake.client();
        let result = client.run(&request).expect("run forget");

        assert!(result.exit_code == 0, "exit code for target {target_name}");

        let args = fake.logged_args();
        let double_dash = args
            .iter()
            .position(|arg| arg == "--")
            .expect("-- arg separator");
        assert_eq!(
            args.get(double_dash + 1).map(String::as_str),
            Some(target_name),
            "target {target_name} should appear after --"
        );
    }
}

#[test]
fn shell_client_passes_destination_flag() {
    let fake = FakeChezmoi::new();
    let _ = fake.client().status().expect("status");

    let args = fake.logged_args();
    let dest_idx = args
        .iter()
        .position(|a| a == "--destination")
        .expect("--destination flag");
    // The value after --destination should be the home directory.
    assert_eq!(
        args.get(dest_idx + 1).map(String::as_str),
        Some(fake.home.to_str().unwrap())
    );
}

#[test]
fn shell_client_passes_source_flag() {
    let fake = FakeChezmoi::new();
    let _ = fake.client().status().expect("status");

    let args = fake.logged_args();
    let source_idx = args
        .iter()
        .position(|a| a == "--source")
        .expect("--source flag");
    assert_eq!(
        args.get(source_idx + 1).map(String::as_str),
        Some(fake.source.to_str().unwrap())
    );
}

#[test]
fn shell_client_forget_includes_force_flags() {
    let fake = FakeChezmoi::new();
    let request = ActionRequest {
        action: Action::Forget,
        target: Some(PathBuf::from(".zshrc")),
        chattr_attrs: None,
    };
    let client = fake.client();
    let _ = client.run(&request).expect("run forget");

    let args = fake.logged_args();
    // The full argv includes: --destination, <home>, --source, <src>,
    // forget, --force, --no-tty, --, .zshrc
    assert!(args.contains(&"--force".to_string()));
    assert!(args.contains(&"--no-tty".to_string()));
    assert!(args.contains(&"forget".to_string()));
}

#[test]
fn shell_client_destroy_target_after_double_dash() {
    let fake = FakeChezmoi::new();
    let request = ActionRequest {
        action: Action::Destroy,
        target: Some(PathBuf::from(".zshrc")),
        chattr_attrs: None,
    };
    let client = fake.client();
    let _ = client.run(&request).expect("run destroy");

    let args = fake.logged_args();
    // destroy should appear as the subcommand, with target after --
    let has_destroy = args.iter().any(|a| a == "destroy");
    assert!(has_destroy, "destroy subcommand should be in args");
    // The target should appear after "--" separator
    let after_double: Vec<&String> = args.iter().skip_while(|a| *a != "--").skip(1).collect();
    assert!(
        after_double.contains(&&".zshrc".to_string()),
        "target should appear after --"
    );
}
