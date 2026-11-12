//! Integration tests using a fake `chezmoi` binary to verify that
//! `ShellChezmoiClient` passes correct arguments to the underlying command
//! and parses its output correctly.

use std::ffi::OsString;
use std::fs;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
#[cfg(target_os = "linux")]
use std::sync::Arc;
#[cfg(target_os = "linux")]
use std::time::{Duration, Instant};

#[cfg(target_os = "linux")]
use chezmoi_tui::app::{BackendEvent, BackendTask};
#[cfg(target_os = "linux")]
use chezmoi_tui::backend::worker_loop;
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
    --config)
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
command_args=" $* "

target=''
while [ "$#" -gt 0 ]; do
  case "$1" in
    --)
      shift
      target="${{1:-}}"
      break
      ;;
    *)
      shift
      ;;
  esac
done

case "$cmd" in
  status)
    if [ -f "$source_dir/.fake-status-output" ]; then
      cat "$source_dir/.fake-status-output"
      exit 0
    fi
    if [ -f "$source_dir/.fake-read-stdin-status" ] && IFS= read -r line; then
      printf 'background status received stdin: %s\n' "$line" >&2
      exit 6
    fi
    if [ -f "$source_dir/.fake-slow-status" ]; then
      sleep 30 &
      printf '%s %s' "$$" "$!" > "$source_dir/.fake-status-pids"
      wait
    fi
    printf ' M .zshrc\n'
    ;;
  managed)
    if [ -f "$source_dir/.fake-managed-output" ]; then
      cat "$source_dir/.fake-managed-output"
    elif [ -f "$source_dir/.fake-managed-dirs-output" ]; then
      cat "$source_dir/.fake-managed-dirs-output"
    else
      case "$command_args" in
      *" --path-style=source-absolute "*)
        if [ -f "$source_dir/.active-source-dirs" ]; then
          tr '\n' '\000' < "$source_dir/.active-source-dirs"
        else
          for directory in "$source_dir"/*/; do
            [ -d "$directory" ] && [ ! -L "${{directory%/}}" ] || continue
            printf '%s\0' "${{directory%/}}"
          done
        fi
        ;;
      *) printf '%s\0' ".zshrc" ".config/nvim/init.lua" ;;
      esac
    fi
    ;;
  unmanaged)
    if [ -f "$source_dir/.fake-unmanaged-output" ]; then
      cat "$source_dir/.fake-unmanaged-output"
    elif [ -f "$source_dir/.fake-fail-unmanaged" ]; then
      printf 'scoped unmanaged failed\n' >&2
      exit 7
    elif [ -f "$source_dir/.fake-large-unmanaged" ]; then
      dd if=/dev/zero bs=1048576 count=5 2>/dev/null
    elif [ -n "$target" ]; then
      printf '%s\0' "$target"
    else
      printf 'tmp.txt\n'
    fi
    ;;
  source-path)
    printf '%s\n' "$source_dir"
    ;;
  execute-template)
    if [ -n "$target" ]; then
      cat "$target"
    else
      cat
    fi
    ;;
  target-path)
    if [ -f "$source_dir/.fake-target-path-failure" ]; then
      printf 'cannot resolve destination: uninitialized source\n' >&2
      exit 9
    fi
    if [ -z "$destination" ] && [ -f "$source_dir/.fake-destination" ]; then
      destination=$(cat "$source_dir/.fake-destination")
    fi
    if [ -n "$target" ]; then
      printf '%s/.config\n' "$destination"
    else
      printf '%s\n' "$destination"
    fi
    ;;
  diff)
    if [ -f "$source_dir/.fake-fail-diff" ]; then
      printf 'diff failed\n' >&2
      exit 8
    fi
    if [ -f "$source_dir/.fake-cancellable-diff" ]; then
      case "$target" in
        */slow-a)
          sleep 30 &
          printf '%s %s' "$$" "$!" > "$source_dir/.fake-cancellable-diff-pids"
          : > "$source_dir/.fake-cancellable-diff-started"
          wait
          ;;
        */fast-b)
          printf '+DIFF_PAYLOAD_B_IMMEDIATE\n'
          exit 0
          ;;
      esac
    fi
    if [ -f "$source_dir/.fake-large-diff" ]; then
      dd if=/dev/zero bs=1048576 count=5 2>/dev/null
      exit 0
    fi
    printf 'diff --git a/.zshrc b/.zshrc\n'
    ;;
  data)
    printf '{{"chezmoi":{{"hostname":"fake"}}}}\n'
    ;;
  doctor)
    printf 'ok\n'
    ;;
  cat)
    if [ -f "$source_dir/.fake-slow-cat" ]; then
      sleep 30 &
      printf '%s %s' "$$" "$!" > "$source_dir/.fake-slow-cat-pids"
      : > "$source_dir/.fake-slow-cat-started"
      wait
    fi
    printf 'cat output\n'
    ;;
  apply|forget|chattr|destroy|add|edit|merge|update|re-add)
    if [ -f "$source_dir/.fake-fail-$cmd" ]; then
      printf '%s failed\n' "$cmd" >&2
      exit 7
    fi
    if [ -f "$source_dir/.fake-slow-$cmd" ]; then
      : > "$source_dir/.fake-$cmd-started"
      sleep 1
    fi
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

    fn logged_invocations(&self) -> Vec<Vec<String>> {
        let content = fs::read_to_string(&self.log).unwrap_or_default();
        let mut invocations = Vec::new();
        let mut current = None;
        for line in content.lines() {
            match line {
                "BEGIN" => current = Some(Vec::new()),
                "END" => {
                    if let Some(args) = current.take() {
                        invocations.push(args);
                    }
                }
                _ => {
                    if let (Some(args), Some((_key, value))) = (&mut current, line.split_once('='))
                    {
                        args.push(value.to_string());
                    }
                }
            }
        }
        invocations
    }
}

#[cfg(target_os = "linux")]
async fn wait_for_path(path: &std::path::Path) {
    tokio::time::timeout(Duration::from_secs(2), async {
        while !path.exists() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("timed out waiting for {}", path.display()));
}

#[cfg(target_os = "linux")]
async fn wait_for_reaped_pids(path: &std::path::Path) {
    let pids = fs::read_to_string(path).expect("cancelled process PIDs");
    let deadline = Instant::now() + Duration::from_secs(1);
    for pid in pids.split_whitespace() {
        loop {
            let stat = fs::read_to_string(format!("/proc/{pid}/stat"));
            let gone_or_zombie = match stat {
                Err(error)
                    if error.kind() == std::io::ErrorKind::NotFound
                        || error.raw_os_error() == Some(3) =>
                {
                    true
                }
                Ok(stat) => stat
                    .rsplit_once(") ")
                    .is_some_and(|(_, rest)| rest.starts_with('Z')),
                Err(error) => panic!("cannot inspect process {pid}: {error}"),
            };
            if gone_or_zombie {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "cancelled process {pid} survived"
            );
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
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
            "      cat \"$target\"\n",
            "      template=$(cat \"$target\")\n      case \"$template\" in\n        *BAD_TEMPLATE*) printf 'inactive template must not execute' >&2; exit 7 ;;\n        *) printf '%s\\n' \"$template\" ;;\n      esac\n",
        ),
    )
    .expect("configure renderer");

    assert_eq!(
        fake.client().ignore_patterns().expect("valid active rules"),
        vec!["Foo", "Bar/secret"]
    );
}

#[cfg(target_os = "linux")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn running_detail_kill_leaves_no_stale_error_and_does_not_cancel_mutation() {
    let fake = FakeChezmoi::new();
    fs::write(fake.source.join(".fake-cancellable-diff"), "").expect("enable slow diff");
    fs::write(fake.source.join(".fake-slow-forget"), "").expect("enable slow mutation");

    let client: Arc<dyn ChezmoiClient> = Arc::new(fake.client());
    let (task_tx, task_rx) = tokio::sync::mpsc::channel(8);
    let (event_tx, mut event_rx) = tokio::sync::mpsc::channel(8);
    let worker = tokio::spawn(worker_loop(client, task_rx, event_tx));

    task_tx
        .send(BackendTask::LoadDiff {
            request_id: 1,
            target: Some(fake.home.join("slow-a")),
        })
        .await
        .expect("queue slow diff");
    wait_for_path(&fake.source.join(".fake-cancellable-diff-started")).await;

    task_tx
        .send(BackendTask::RunAction {
            request: ActionRequest {
                action: Action::Forget,
                targets: vec![fake.home.join(".mutation")],
                chattr_attrs: None,
            },
        })
        .await
        .expect("queue mutation");
    wait_for_path(&fake.source.join(".fake-forget-started")).await;

    task_tx
        .send(BackendTask::LoadDiff {
            request_id: 2,
            target: Some(fake.home.join("fast-b")),
        })
        .await
        .expect("queue replacement diff");

    let mut cancelled_a = false;
    let mut loaded_b = false;
    let mut mutation_finished = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(4);
    while !loaded_b || !mutation_finished {
        let event = tokio::time::timeout_at(deadline, event_rx.recv())
            .await
            .expect("timed out waiting for backend events")
            .expect("backend event channel closed");
        match event {
            BackendEvent::DetailCoalesced { request_id: 1 } => cancelled_a = true,
            BackendEvent::DiffLoaded {
                request_id: 2,
                diff,
                ..
            } => {
                assert!(diff.text.contains("DIFF_PAYLOAD_B_IMMEDIATE"));
                loaded_b = true;
            }
            BackendEvent::ActionFinished { request, result } => {
                assert_eq!(request.action, Action::Forget);
                assert_eq!(result.exit_code, 0);
                mutation_finished = true;
            }
            BackendEvent::DetailError {
                request_id,
                message,
                ..
            } => panic!("stale request {request_id} surfaced an error: {message}"),
            other => panic!("unexpected backend event: {other:?}"),
        }
    }
    assert!(cancelled_a, "running detail was not reported as coalesced");

    drop(task_tx);
    worker.await.expect("backend worker");

    wait_for_reaped_pids(&fake.source.join(".fake-cancellable-diff-pids")).await;
}

#[cfg(target_os = "linux")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn running_inspection_kill_leaves_no_stale_error() {
    let fake = FakeChezmoi::new();
    fs::write(fake.source.join(".fake-slow-cat"), "").expect("enable slow inspection");
    fs::write(fake.source.join(".fake-cancellable-diff"), "").expect("enable fast diff");

    let client: Arc<dyn ChezmoiClient> = Arc::new(fake.client());
    let (task_tx, task_rx) = tokio::sync::mpsc::channel(8);
    let (event_tx, mut event_rx) = tokio::sync::mpsc::channel(8);
    let worker = tokio::spawn(worker_loop(client, task_rx, event_tx));

    task_tx
        .send(BackendTask::Inspect {
            request_id: 10,
            request: ActionRequest {
                action: Action::Cat,
                targets: vec![fake.home.join("slow-inspect")],
                chattr_attrs: None,
            },
        })
        .await
        .expect("queue slow inspection");
    wait_for_path(&fake.source.join(".fake-slow-cat-started")).await;

    task_tx
        .send(BackendTask::LoadDiff {
            request_id: 11,
            target: Some(fake.home.join("fast-b")),
        })
        .await
        .expect("queue replacement diff");

    let mut cancelled_inspection = false;
    let mut loaded_b = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    while !loaded_b {
        let event = tokio::time::timeout_at(deadline, event_rx.recv())
            .await
            .expect("timed out waiting for backend events")
            .expect("backend event channel closed");
        match event {
            BackendEvent::DetailCoalesced { request_id: 10 } => cancelled_inspection = true,
            BackendEvent::DiffLoaded {
                request_id: 11,
                diff,
                ..
            } => {
                assert!(diff.text.contains("DIFF_PAYLOAD_B_IMMEDIATE"));
                loaded_b = true;
            }
            BackendEvent::DetailError {
                request_id,
                message,
                ..
            } => panic!("stale request {request_id} surfaced an error: {message}"),
            other => panic!("unexpected backend event: {other:?}"),
        }
    }
    assert!(
        cancelled_inspection,
        "running inspection was not reported as coalesced"
    );

    drop(task_tx);
    worker.await.expect("backend worker");
    wait_for_reaped_pids(&fake.source.join(".fake-slow-cat-pids")).await;
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
    let args = fake.logged_args();
    assert!(args.iter().any(|arg| arg == "--file=true"));
    assert!(
        args.iter()
            .any(|arg| arg == fake.source.join(".chezmoiignore.tmpl").to_str().unwrap())
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
            "      cat \"$target\"\n",
            "      dd if=/dev/zero bs=1048576 count=5 2>/dev/null\n",
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
fn shell_client_uses_cwd_context_for_unmanaged_and_ignore_but_home_for_source() {
    let fake = FakeChezmoi::new();
    let nested = fake.source.join("private_dot_config");
    fs::create_dir(&nested).expect("nested source directory");
    fs::write(fake.source.join(".chezmoiignore"), "root-only\n").expect("root ignore");
    fs::write(nested.join(".chezmoiignore"), "nested-only\n").expect("nested ignore");

    let client = fake.client();
    client.unmanaged().expect("unmanaged");
    assert_eq!(
        client.ignore_patterns().expect("ignore patterns"),
        vec!["root-only", ".config/nested-only"]
    );
    client.status().expect("status");
    client.source().expect("source");

    let invocations = fake.logged_invocations();
    let expected = [
        ("unmanaged", &fake.work),
        ("source-path", &fake.work),
        ("managed", &fake.work),
        ("execute-template", &fake.work),
        ("execute-template", &fake.work),
        ("target-path", &fake.work),
        ("managed", &fake.home),
        ("status", &fake.home),
        ("source-path", &fake.home),
    ];
    assert_eq!(invocations.len(), expected.len());
    for (invocation, (expected_command, expected_destination)) in invocations.iter().zip(expected) {
        assert!(
            invocation.iter().any(|arg| arg == expected_command),
            "missing {expected_command}: {invocation:?}"
        );
        let destination = invocation
            .iter()
            .position(|arg| arg == "--destination")
            .map(|index| invocation[index + 1].as_str())
            .expect("destination");
        assert_eq!(
            destination,
            expected_destination.to_str().expect("UTF-8 fixture path")
        );
    }
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
    let (source_root, source_paths) = fake.client().source().expect("source");
    assert_eq!(source_root, effective);
    assert_eq!(source_paths, vec![PathBuf::from(".chezmoiignore")]);
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
            "      cat \"$target\"\n",
            "      template=$(cat \"$target\")\n      case \"$template\" in\n        *chezmoi.os*) printf 'cache/**\\n' ;;\n        *) exit 9 ;;\n      esac\n",
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
            "      cat \"$target\"\n",
            "      printf 'partial-rule\\n'\n      printf 'bad template\\n' >&2\n      exit 7\n",
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
            "      cat \"$target\"\n",
            "      dd if=/dev/zero bs=1048576 count=1 >&2 2>/dev/null\n",
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
fn shell_client_rejects_status_when_managed_name_can_inject_a_valid_row() {
    let fake = FakeChezmoi::new();
    fs::write(
        fake.source.join(".fake-managed-output"),
        b".foo\n M victim\0victim\0",
    )
    .expect("managed output with embedded newline");
    fs::write(
        fake.source.join(".fake-status-output"),
        b" A .foo\n M victim\n A victim\n",
    )
    .expect("ambiguous status output");

    let error = fake
        .client()
        .status()
        .expect_err("newline path must fail closed");

    assert!(error.to_string().contains("cannot safely represent"));
    let invocations = fake.logged_invocations();
    assert!(
        invocations
            .iter()
            .any(|args| args.iter().any(|arg| arg == "managed"))
    );
    assert!(
        !invocations
            .iter()
            .any(|args| args.iter().any(|arg| arg == "status")),
        "status must not run after managed paths prove the output ambiguous"
    );
}

#[test]
fn shell_client_reads_managed_from_fake_chezmoi() {
    let fake = FakeChezmoi::new();
    let client = fake.client();
    let managed = client.managed().expect("managed");

    assert!(managed.contains(&PathBuf::from(".zshrc")));
    assert!(managed.contains(&PathBuf::from(".config/nvim/init.lua")));
    let args = fake.logged_args();
    assert!(args.iter().any(|arg| arg == "--path-style=relative"));
    assert!(args.iter().any(|arg| arg == "--nul-path-separator"));
}

#[test]
fn shell_client_reads_diff_from_fake_chezmoi() {
    let fake = FakeChezmoi::new();
    let client = fake.client();
    let diff = client.diff(&[]).expect("diff");

    assert!(diff.text.contains("diff --git"));
}

#[test]
fn shell_client_passes_option_like_targets_after_double_dash() {
    for target_name in ["--help", "-n"] {
        let fake = FakeChezmoi::new();
        let request = ActionRequest {
            action: Action::Forget,
            targets: vec![PathBuf::from(target_name)],
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
        targets: vec![PathBuf::from(".zshrc")],
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
        targets: vec![PathBuf::from(".zshrc")],
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

#[test]
fn shell_client_queries_cwd_once_and_preserves_relative_paths() {
    let fake = FakeChezmoi::new();
    let spaced = fake.work.join("a file");
    let nested = fake.work.join("nested dir");
    let mut output = Vec::new();
    output.extend_from_slice(spaced.as_os_str().as_encoded_bytes());
    output.push(0);
    output.extend_from_slice(nested.as_os_str().as_encoded_bytes());
    output.push(0);
    fs::write(fake.source.join(".fake-unmanaged-output"), output).expect("unmanaged output");

    let paths = fake.client().unmanaged().expect("unmanaged paths");
    assert_eq!(
        paths,
        vec![PathBuf::from("a file"), PathBuf::from("nested dir")]
    );

    let args = fake.logged_args();
    assert_eq!(
        args.iter()
            .filter(|arg| arg.as_str() == "unmanaged")
            .count(),
        1
    );
    assert!(args.iter().any(|arg| arg == "--path-style=absolute"));
    assert!(args.iter().any(|arg| arg == "--nul-path-separator"));
    assert!(args.iter().any(|arg| arg == "--no-tty"));
    assert!(args.iter().any(|arg| arg == "--progress=false"));
    assert!(args.iter().any(|arg| arg == "--no-pager"));
    assert!(args.iter().any(|arg| arg == "--color=false"));
    assert!(args.iter().any(|arg| arg == fake.work.to_str().unwrap()));
    let destination = args
        .iter()
        .position(|arg| arg == "--destination")
        .expect("destination flag");
    assert_eq!(
        args.get(destination + 1).map(String::as_str),
        fake.work.to_str(),
        "cwd outside destination should become the invocation destination"
    );
}

#[test]
fn shell_client_rejects_non_utf8_machine_paths_without_aliasing_fffd_neighbor() {
    let fake = FakeChezmoi::new();
    let invalid_name = OsString::from_vec(b"bad\xff".to_vec());
    let invalid = fake.work.join(&invalid_name);
    let neighbor = fake.work.join("bad\u{fffd}");
    fs::write(&invalid, "invalid-byte file").expect("invalid-byte fixture");
    fs::write(&neighbor, "replacement-character file").expect("replacement fixture");
    let mut output = invalid.as_os_str().as_bytes().to_vec();
    output.push(0);
    fs::write(fake.source.join(".fake-unmanaged-output"), output).expect("raw unmanaged path");

    let error = fake
        .client()
        .unmanaged()
        .expect_err("invalid path bytes must fail closed");

    assert!(error.to_string().contains("not valid UTF-8"));
    assert_eq!(
        fs::read_to_string(invalid).expect("invalid fixture"),
        "invalid-byte file"
    );
    assert_eq!(
        fs::read_to_string(neighbor).expect("neighbor fixture"),
        "replacement-character file"
    );
}

#[test]
fn shell_client_rejects_non_utf8_managed_and_active_directory_paths() {
    {
        let fake = FakeChezmoi::new();
        fs::write(
            fake.source.join(".fake-managed-output"),
            b"bad\xff\0bad\xef\xbf\xbd\0",
        )
        .expect("raw managed paths");
        let error = fake.client().managed().expect_err("invalid managed path");
        assert!(error.to_string().contains("not valid UTF-8"));
    }
    {
        let fake = FakeChezmoi::new();
        fs::write(fake.source.join(".fake-managed-dirs-output"), b"bad\xff\0")
            .expect("raw active source directory");
        let error = fake
            .client()
            .ignore_patterns()
            .expect_err("invalid active directory path");
        assert!(error.to_string().contains("not valid UTF-8"));
    }
}

#[test]
fn shell_client_preserves_distinct_trailing_space_path_records() {
    let fake = FakeChezmoi::new();
    let plain = fake.work.join("name");
    let spaced = fake.work.join("name ");
    fs::write(&plain, "plain").expect("plain fixture");
    fs::write(&spaced, "spaced").expect("spaced fixture");
    let mut output = plain.as_os_str().as_bytes().to_vec();
    output.push(0);
    output.extend_from_slice(spaced.as_os_str().as_bytes());
    output.push(0);
    fs::write(fake.source.join(".fake-unmanaged-output"), output).expect("unmanaged output");

    assert_eq!(
        fake.client().unmanaged().expect("distinct unmanaged paths"),
        vec![PathBuf::from("name"), PathBuf::from("name ")]
    );

    fs::write(fake.source.join(".fake-managed-output"), b"name\0name \0").expect("managed output");
    assert_eq!(
        fake.client().managed().expect("distinct managed paths"),
        vec![PathBuf::from("name"), PathBuf::from("name ")]
    );
}

#[test]
fn shell_client_queries_inside_destination_cwd_directly_once() {
    let fake = FakeChezmoi::new();
    let cwd = fake.home.join("nested dir");
    fs::create_dir(&cwd).expect("nested cwd");
    let file = cwd.join("a file");
    fs::write(&file, "fixture").expect("nested file");
    let mut output = Vec::new();
    output.extend_from_slice(cwd.as_os_str().as_encoded_bytes());
    output.push(0);
    output.extend_from_slice(file.as_os_str().as_encoded_bytes());
    output.push(0);
    fs::write(fake.source.join(".fake-unmanaged-output"), output).expect("unmanaged output");

    let client = chezmoi_tui::infra::ShellChezmoiClient::new(
        fake.bin.to_string_lossy(),
        fake.home.clone(),
        cwd.clone(),
        Some(fake.source.clone()),
    );
    assert_eq!(
        client.unmanaged().expect("unmanaged paths"),
        vec![PathBuf::from("."), PathBuf::from("a file")]
    );

    let args = fake.logged_args();
    assert_eq!(
        args.iter()
            .filter(|arg| arg.as_str() == "unmanaged")
            .count(),
        1
    );
    assert!(args.iter().any(|arg| arg == cwd.to_str().unwrap()));
    let destination = args
        .iter()
        .position(|arg| arg == "--destination")
        .expect("destination flag");
    assert_eq!(
        args.get(destination + 1).map(String::as_str),
        fake.home.to_str()
    );
}

#[test]
fn shell_client_preserves_unmanaged_cwd_root_for_lazy_filtered_expansion() {
    let fake = FakeChezmoi::new();

    assert_eq!(
        fake.client().unmanaged().expect("unmanaged root"),
        vec![PathBuf::from(".")]
    );
    assert_eq!(
        fake.logged_args()
            .iter()
            .filter(|arg| arg.as_str() == "unmanaged")
            .count(),
        1
    );
}

#[test]
fn shell_client_rejects_incomplete_unmanaged_and_diff_commands() {
    {
        let fake = FakeChezmoi::new();
        fs::write(fake.source.join(".fake-fail-unmanaged"), "fail")
            .expect("unmanaged failure marker");
        let error = fake.client().unmanaged().expect_err("unmanaged failure");
        assert!(error.to_string().contains("scoped unmanaged failed"));
    }
    {
        let fake = FakeChezmoi::new();
        fs::write(fake.source.join(".fake-fail-diff"), "fail").expect("diff failure marker");
        let error = fake.client().diff(&[]).expect_err("diff failure");
        assert!(error.to_string().contains("diff failed"));
    }
}

#[test]
fn shell_client_rejects_truncated_unmanaged_and_diff_output() {
    {
        let fake = FakeChezmoi::new();
        fs::write(fake.source.join(".fake-large-unmanaged"), "large")
            .expect("large unmanaged marker");
        let error = fake
            .client()
            .unmanaged()
            .expect_err("unmanaged output limit");
        assert!(
            error
                .to_string()
                .contains("output was truncated or limited")
        );
    }
    {
        let fake = FakeChezmoi::new();
        fs::write(fake.source.join(".fake-large-diff"), "large").expect("large diff marker");
        let error = fake.client().diff(&[]).expect_err("diff output limit");
        assert!(
            error
                .to_string()
                .contains("output was truncated or limited")
        );
    }
}

#[test]
fn shell_client_prunes_source_metadata_and_ignored_dotfiles() {
    let fake = FakeChezmoi::new();
    for path in [
        "dot_zshrc",
        ".chezmoiignore",
        ".chezmoitemplates/private_dot_template",
        ".git/config",
        ".hg/store",
        ".svn/entries",
        ".cache/secret",
        ".github/workflow",
        "private_dot_config/app.toml",
    ] {
        let path = fake.source.join(path);
        fs::create_dir_all(path.parent().expect("parent")).expect("source parent");
        fs::write(path, "fixture").expect("source file");
    }
    let outside = fake._temp.path().join("outside-source");
    fs::create_dir(&outside).expect("outside");
    fs::write(outside.join("leaked"), "fixture").expect("outside file");
    std::os::unix::fs::symlink(&outside, fake.source.join("linked-dir")).expect("source symlink");

    let (_, paths) = fake.client().source().expect("source files");
    assert_eq!(
        paths,
        vec![
            PathBuf::from(".chezmoiignore"),
            PathBuf::from(".chezmoitemplates"),
            PathBuf::from(".chezmoitemplates/private_dot_template"),
            PathBuf::from("dot_zshrc"),
            PathBuf::from("linked-dir"),
            PathBuf::from("private_dot_config"),
            PathBuf::from("private_dot_config/app.toml"),
        ]
    );
}

#[test]
fn shell_client_adds_template_and_encryption_on_initial_add() {
    let fake = FakeChezmoi::new();
    let request = ActionRequest {
        action: Action::Add,
        targets: vec![PathBuf::from("secret file")],
        chattr_attrs: Some("encrypted,template,private,executable".to_string()),
    };

    let result = fake.client().run(&request).expect("add");
    assert_eq!(result.exit_code, 0);
    let args = fake.logged_args();
    let add = args
        .iter()
        .position(|arg| arg == "add")
        .expect("add command");
    assert!(args[add + 1..].iter().any(|arg| arg == "--template"));
    assert!(args[add + 1..].iter().any(|arg| arg == "--encrypt"));
    assert!(!args[add + 1..].iter().any(|arg| arg == "encrypted"));
    assert!(!args[add + 1..].iter().any(|arg| arg == "private"));
    assert!(!args[add + 1..].iter().any(|arg| arg == "executable"));
    assert!(args.iter().any(|arg| arg == "secret file"));
    let destination = args
        .iter()
        .position(|arg| arg == "--destination")
        .expect("destination flag");
    assert_eq!(
        args.get(destination + 1).map(String::as_str),
        fake.home.to_str(),
        "add must use the resolved session destination"
    );
}

#[test]
fn shell_client_reports_initial_add_failure() {
    let fake = FakeChezmoi::new();
    fs::write(fake.source.join(".fake-fail-add"), "fail").expect("add failure marker");
    let request = ActionRequest {
        action: Action::Add,
        targets: vec![PathBuf::from("secret.txt")],
        chattr_attrs: Some("encrypted".to_string()),
    };

    let result = fake.client().run(&request).expect("failed command result");
    assert_eq!(result.exit_code, 7);
    assert!(result.stderr.contains("add failed"));
}

#[test]
fn shell_client_apply_and_destroy_are_noninteractive_and_safe() {
    {
        let fake = FakeChezmoi::new();
        let apply = ActionRequest {
            action: Action::Apply,
            targets: vec![PathBuf::from(".zshrc")],
            chattr_attrs: None,
        };
        fake.client().run(&apply).expect("apply");
        let args = fake.logged_args();
        assert!(args.iter().any(|arg| arg == "--error-on-conflict=true"));
        assert!(!args.iter().any(|arg| arg == "--force"));
        assert!(args.iter().any(|arg| arg == "--no-tty"));
    }
    {
        let fake = FakeChezmoi::new();
        let destroy = ActionRequest {
            action: Action::Destroy,
            targets: vec![PathBuf::from("old.txt")],
            chattr_attrs: None,
        };
        fake.client().run(&destroy).expect("destroy");
        let args = fake.logged_args();
        assert!(args.iter().any(|arg| arg == "--force"));
        assert!(args.iter().any(|arg| arg == "--no-tty"));
    }
}

#[test]
fn shell_client_background_stdin_is_null() {
    let fake = FakeChezmoi::new();
    fs::write(fake.source.join(".fake-read-stdin-status"), "fixture").expect("stdin marker");

    assert_eq!(fake.client().status().expect("status").len(), 1);
}

#[test]
fn shell_client_config_builder_is_applied_to_invocations() {
    let fake = FakeChezmoi::new();
    let config = fake._temp.path().join("chezmoi config.toml");
    fs::write(&config, "destDir = '/tmp'\n").expect("config");

    fake.client()
        .with_config_file(Some(config.clone()))
        .status()
        .expect("status");
    let args = fake.logged_args();
    let config_flag = args
        .iter()
        .position(|arg| arg == "--config")
        .expect("config flag");
    assert_eq!(
        args.get(config_flag + 1).map(String::as_str),
        config.to_str()
    );
}

#[test]
fn shell_client_resolve_destination_honors_source_and_config_without_destination_flag() {
    let fake = FakeChezmoi::new();
    let destination = fake._temp.path().join("configured destination");
    let config = fake._temp.path().join("config.toml");
    fs::write(&config, "fixture").expect("config");
    fs::write(
        fake.source.join(".fake-destination"),
        destination.as_os_str().as_encoded_bytes(),
    )
    .expect("destination marker");

    let resolved = chezmoi_tui::infra::ShellChezmoiClient::resolve_destination(
        fake.bin.to_str().expect("binary"),
        Some(&fake.source),
        Some(&config),
    )
    .expect("resolved destination");
    assert_eq!(resolved, destination);

    let args = fake.logged_args();
    assert!(args.iter().any(|arg| arg == "target-path"));
    assert!(!args.iter().any(|arg| arg == "--destination"));
    let source = args
        .iter()
        .position(|arg| arg == "--source")
        .expect("source flag");
    assert_eq!(
        args.get(source + 1).map(String::as_str),
        fake.source.to_str()
    );
    let config_flag = args
        .iter()
        .position(|arg| arg == "--config")
        .expect("config flag");
    assert_eq!(
        args.get(config_flag + 1).map(String::as_str),
        config.to_str()
    );
}

#[test]
fn shell_client_resolve_destination_preserves_command_error_details() {
    let fake = FakeChezmoi::new();
    fs::write(fake.source.join(".fake-target-path-failure"), "fixture").expect("failure marker");

    let error = chezmoi_tui::infra::ShellChezmoiClient::resolve_destination(
        fake.bin.to_str().expect("binary"),
        Some(&fake.source),
        None,
    )
    .expect_err("target-path failure");
    assert!(
        error
            .to_string()
            .contains("cannot resolve destination: uninitialized source")
    );
}
