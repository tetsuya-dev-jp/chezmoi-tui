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
destination=''

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
      destination="$2"
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
    case " $* " in
      *" --path-style=source-absolute "*)
        if [ -f "$source_dir/.active-source-dirs" ]; then
          cat "$source_dir/.active-source-dirs"
        else
          for directory in "$source_dir"/*/; do
            [ -d "$directory" ] && [ ! -L "${{directory%/}}" ] || continue
            printf '%s\n' "${{directory%/}}"
          done
        fi
        ;;
      *) printf '[".zshrc",".config/nvim/init.lua"]\n' ;;
    esac
    ;;
  unmanaged)
    printf 'tmp.txt\n'
    ;;
  source-path)
    printf '%s\n' "$source_dir"
    ;;
  execute-template)
    cat
    ;;
  target-path)
    printf '%s/.config\n' "$destination"
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

// Serialize shell fixtures: concurrent fork/exec can temporarily inherit another
// test's open script writer and cause Linux ETXTBSY even after that writer closes.
static SHELL_FIXTURES: std::sync::Mutex<()> = std::sync::Mutex::new(());

struct FakeChezmoi {
    _guard: std::sync::MutexGuard<'static, ()>,
    _temp: tempfile::TempDir,
    bin: PathBuf,
    home: PathBuf,
    work: PathBuf,
    source: PathBuf,
    log: PathBuf,
}

impl FakeChezmoi {
    fn new() -> Self {
        let guard = SHELL_FIXTURES
            .lock()
            .unwrap_or_else(|error| error.into_inner());
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
            _guard: guard,
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
fn shell_client_does_not_render_ignored_source_subtrees() {
    let fake = FakeChezmoi::new();
    let ignored = fake.source.join("Foo");
    fs::create_dir(&ignored).expect("ignored source directory");
    fs::write(fake.source.join(".chezmoiignore"), "Foo\nBar/secret\n").expect("root ignore");
    fs::write(ignored.join(".chezmoiignore"), "BAD_TEMPLATE\n").expect("inactive template");
    fs::write(fake.source.join(".active-source-dirs"), "")
        .expect("chezmoi reports no active source directories");
    let script = fs::read_to_string(&fake.bin).expect("read fake");
    fs::write(
        &fake.bin,
        script.replace(
            "    cat\n",
            "    template=$(cat)\n    case \"$template\" in\n      *BAD_TEMPLATE*) printf 'inactive template must not execute' >&2; exit 7 ;;\n      *) printf '%s\\n' \"$template\" ;;\n    esac\n",
        ),
    )
    .expect("configure renderer");

    assert_eq!(
        fake.client().ignore_patterns().expect("valid active rules"),
        vec!["Foo", "Bar/secret"]
    );
}

#[test]
fn shell_client_loads_ignore_template_suffix() {
    let fake = FakeChezmoi::new();
    fs::write(
        fake.source.join(".chezmoiignore.tmpl"),
        "cache/**\n!cache/keep\n",
    )
    .expect("write ignore template");

    assert_eq!(
        fake.client().ignore_patterns().expect("ignore patterns"),
        vec!["cache/**", "!cache/keep"]
    );
    assert!(
        fake.logged_args()
            .iter()
            .any(|arg| arg == "execute-template")
    );
}

#[test]
fn shell_client_rejects_template_output_above_default_limit() {
    let fake = FakeChezmoi::new();
    fs::write(fake.source.join(".chezmoiignore"), "input\n").expect("write ignore");
    let script = fs::read_to_string(&fake.bin).expect("read fake");
    fs::write(
        &fake.bin,
        script.replace(
            "    cat\n",
            "    dd if=/dev/zero bs=1048576 count=5 2>/dev/null\n",
        ),
    )
    .expect("write noisy template renderer");

    let error = fake.client().ignore_patterns().expect_err("output limit");
    assert!(
        error
            .to_string()
            .contains("output was truncated or limited")
    );
}

#[test]
fn shell_client_loads_nested_ignore_relative_to_decoded_target_directory() {
    let fake = FakeChezmoi::new();
    let nested = fake.source.join("private_dot_config");
    fs::create_dir(&nested).expect("nested source directory");
    fs::write(
        nested.join(".chezmoiignore.tmpl"),
        "  cache/** # comment\n !cache/keep\n# comment\n\n",
    )
    .expect("write nested ignore");

    assert_eq!(
        fake.client().ignore_patterns().expect("nested patterns"),
        vec![
            ".config/cache/** # comment",
            "!.config/cache/keep",
            "# comment",
            ""
        ]
    );
    assert!(fake.logged_args().iter().any(|arg| arg == "target-path"));
}

#[test]
fn shell_client_uses_authoritative_source_root_over_source_override() {
    let fake = FakeChezmoi::new();
    let effective = fake.source.join("actual-root");
    fs::create_dir(&effective).expect("effective root");
    fs::write(fake.source.join(".chezmoiroot"), "actual-root\n").expect("root marker");
    fs::write(fake.source.join(".chezmoiignore"), "wrong-root\n").expect("decoy ignore");
    fs::write(effective.join(".chezmoiignore"), "actual-root-rule\n").expect("root ignore");
    write_fake_chezmoi(fake._temp.path(), &fake.log, &effective);

    assert_eq!(
        fake.client().ignore_patterns().expect("authoritative root"),
        vec!["actual-root-rule"]
    );
    assert!(fake.logged_args().iter().any(|arg| arg == "source-path"));
}

#[test]
fn shell_client_renders_ignore_template_instead_of_returning_source() {
    let fake = FakeChezmoi::new();
    let input = "{{ if eq .chezmoi.os \"linux\" }}\ncache/**\n{{ end }}\n";
    fs::write(fake.source.join(".chezmoiignore"), input).expect("write template");
    let script = fs::read_to_string(&fake.bin).expect("read fake");
    fs::write(
        &fake.bin,
        script.replace(
            "    cat\n",
            "    template=$(cat)\n    case \"$template\" in\n      *chezmoi.os*) printf 'cache/**\\n' ;;\n      *) exit 9 ;;\n    esac\n",
        ),
    )
    .expect("configure renderer");

    assert_eq!(
        fake.client().ignore_patterns().expect("rendered rules"),
        vec!["cache/**"]
    );
}

#[test]
fn shell_client_without_ignore_files_does_not_run_renderer() {
    let fake = FakeChezmoi::new();
    for directory in [
        ".git",
        ".chezmoitemplates",
        ".chezmoidata",
        ".chezmoiscripts",
    ] {
        let directory = fake.source.join(directory);
        fs::create_dir(&directory).expect("special directory");
        fs::write(directory.join(".chezmoiignore"), "not-a-rule\n").expect("decoy ignore");
    }
    fs::write(fake.source.join(".chezmoiignore.tmpl.bak"), "not-a-rule\n").expect("backup");

    assert!(
        fake.client()
            .ignore_patterns()
            .expect("no ignore files")
            .is_empty()
    );
    assert!(
        !fake
            .logged_args()
            .iter()
            .any(|arg| arg == "execute-template")
    );
}

#[test]
fn shell_client_loads_both_ignore_names_without_filename_precedence() {
    let fake = FakeChezmoi::new();
    fs::write(
        fake.source.join(".chezmoiignore"),
        "plain/**\n!shared/keep\n",
    )
    .expect("plain ignore");
    fs::write(
        fake.source.join(".chezmoiignore.tmpl"),
        "suffix/**\nshared/**\n",
    )
    .expect("suffix ignore");

    assert_eq!(
        fake.client().ignore_patterns().expect("both files"),
        vec!["plain/**", "!shared/keep", "suffix/**", "shared/**"]
    );
    assert_eq!(
        fake.logged_args()
            .iter()
            .filter(|arg| *arg == "execute-template")
            .count(),
        2
    );
}

#[test]
fn shell_client_propagates_template_failure_without_partial_patterns() {
    let fake = FakeChezmoi::new();
    fs::write(fake.source.join(".chezmoiignore"), "invalid template\n").expect("template");
    let script = fs::read_to_string(&fake.bin).expect("read fake");
    fs::write(
        &fake.bin,
        script.replace(
            "    cat\n",
            "    printf 'partial-rule\\n'\n    printf 'bad template\\n' >&2\n    exit 7\n",
        ),
    )
    .expect("failing renderer");

    let error = fake
        .client()
        .ignore_patterns()
        .expect_err("renderer failure");
    assert!(
        error
            .to_string()
            .contains("chezmoi execute-template failed: bad template")
    );
}

#[test]
fn shell_client_rejects_template_stderr_above_default_limit() {
    let fake = FakeChezmoi::new();
    fs::write(fake.source.join(".chezmoiignore"), "input\n").expect("write ignore");
    let script = fs::read_to_string(&fake.bin).expect("read fake");
    fs::write(
        &fake.bin,
        script.replace(
            "    cat\n",
            "    dd if=/dev/zero bs=1048576 count=1 >&2 2>/dev/null\n",
        ),
    )
    .expect("noisy stderr renderer");

    let error = fake.client().ignore_patterns().expect_err("stderr limit");
    assert!(
        error
            .to_string()
            .contains("output was truncated or limited")
    );
}

#[test]
fn shell_client_does_not_descend_source_directory_symlinks() {
    let fake = FakeChezmoi::new();
    let stored = fake.source.join(".hidden-store");
    fs::create_dir(&stored).expect("stored directory");
    fs::write(stored.join(".chezmoiignore"), "cache/**\n").expect("stored ignore");
    std::os::unix::fs::symlink(&stored, fake.source.join("dot_config")).expect("source alias");

    // Real chezmoi visits a symlink directory entry but does not recurse into it.
    assert!(
        fake.client()
            .ignore_patterns()
            .expect("symlinked source")
            .is_empty()
    );
}

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
