# Release Notes: v0.3.0

`0.3.0` completes the correctness, UI, action, dependency, and release-process
modernization described here. Local verification passed on Linux for Rust
1.99.1 and the Rust 1.88.0 MSRV. Distribution is performed through the
[crates.io package](https://crates.io/crates/chezmoi-tui/0.3.0) and
[GitHub release](https://github.com/tetsuya-dev-jp/chezmoi-tui/releases/tag/v0.3.0).

## Highlights

`0.3.0` modernizes the terminal workspace, extends the practical action set,
and tightens the boundary between read-only inspection, scoped mutation,
foreground tools, and destructive operations.

### Interface

- Added a persistent two-row header with all four tabs and live item counts.
- Reworked the wide layout into a List/Detail workspace with a compact four-row
  Log drawer. Focusing Log uses the configured Detail/Log split.
- Added a focused single-pane layout for terminals narrower than 90 columns or
  shorter than 18 rows.
- Added focused-pane maximize/restore, reverse focus traversal, first/last and
  page navigation, and `Ctrl+P` as an action-menu alias.
- Consolidated footer/help hints and finalized legible, quiet selection and
  focus styling without duplicate key hints.
- Added contextual empty/loading/error states, Unicode-aware width handling,
  monochrome markers, and `NO_COLOR` handling.
- Added four sanitized, reproducible SVG views generated from synthetic UI
  states and reviewed for identifiers.

### Refresh and detail loading

- Refresh results are published per view, so Status, Managed, Unmanaged, Source,
  and ignore data can become usable independently.
- Repeated refreshes coalesce; mutations are serialized; obsolete detail
  responses are rejected by request ID.
- Automatic diffs and previews are debounced, and queued detail work keeps only
  the newest request.
- Unmanaged list search uses an incremental, time-budgeted index instead of an
  unbounded UI-thread scan.
- List marks remain stable through filtering and batch selection.
- File preview rejects FIFOs/devices and binary content, bounds text reads, and
  reports truncation.
- Background subprocesses use null stdin, process-tree cancellation, command
  timeouts, and bounded stdout/stderr.

### Actions

The action model now defines 37 workflow-grouped actions:

- Read-only inspection: `verify`, `cat`, `ignored`, `dump-config`, `diff-all`,
  `git-status`, `git-diff`, and `git-log`.
- Source and import: `edit-source`, `re-add-all`, `init`, `add-directory`, and
  `preview-add-directory`.
- Source Git: `git-stage`, `git-commit`, `git-push`, and `git-pull`.

Existing practical workflows remain available for apply, add, attributes,
ignore rules, edits, merge, update, diagnostics, and exact destroy/purge.

Important semantics:

- Managed/Status list preview remains a destination-file view. `cat` is the
  explicit action for inspecting rendered target content.
- `git-pull` means source update without applying. `git-push` pushes existing
  commits. `git-commit` commits staged changes. `git-stage` stages the selected
  source file.
- Directory import has recursive dry-run preview and confirmed recursive add;
  it does not add `--exact` or `--follow`.
- `init` and bulk `re-add-all` are explicit, guarded actions rather than startup
  side effects.
- Aggregate Apply uses one plan and one confirmation, then one serialized
  mutation job that revalidates immediately before applying. No second mutation
  can interleave between that validation and apply.
- Managed directories are supported with a complete recursive descendant plan
  and `chezmoi diff --recursive`.

## Fixes and safety

- Resolve the effective destination through chezmoi when `--destination` is not
  supplied, and keep Unmanaged browsing separate from mutation targeting.
- Reject Add/AddDirectory outside the session destination. Ignore outside the
  destination is valid only through the explicit Global by name rule.
- Use the effective source root resolved through `source-path`, including
  `.chezmoiroot`.
- Preserve raw-source semantics in Source view while clearly treating it as raw
  files rather than decoded target state.
- Reapply rendered ignore rules during Unmanaged descent and bounded searching.
  Failure disables descendant/search trust and surfaces an error.
- Stop foreground batches on failure and refresh partial mutations.
- Keep typed `DESTROY <target>` and `PURGE ALL` checks even when the optional
  second confirmation step is disabled in configuration.
- Keep sensitive inspection explicit. `cat` and `dump-config` do not require an
  extra confirmation modal, but list navigation does not invoke them and their
  output is not persisted automatically.
- Route primary output from read-only inspection actions into the detail pane
  while preserving explicit exit-code handling for drift and failures.
- Secret-bearing inspection and maintenance remain available through the
  explicit foreground chezmoi shell. The TUI does not remove existing
  chezmoi secret-management functionality or persist inspected output.
- Ignore updates use a nonblocking cross-process flock, one assembled payload,
  a 4 MiB cap, and non-regular-file rejection.
- Detail commands are cancellable through the terminal handoff boundary. A new
  selection or mutation cancels the obsolete process without leaving a stale
  error or canceling unrelated mutation work.

## Configuration and CLI

- Added `--chezmoi` to select the chezmoi binary.
- Added `--chezmoi-config` to pass chezmoi's own configuration file to every
  invocation.
- Kept `--config` as the distinct chezmoi-tui TOML file.
- Documented platform paths from `dirs::config_dir()`:
  `~/.config/chezmoi-tui/config.toml` on Linux and
  `~/Library/Application Support/chezmoi-tui/config.toml` on macOS.
- Missing explicitly selected TUI config files now fail clearly, and unknown
  TOML fields are rejected.
- Clarified layout settings: `list_ratio` controls the wide List/right split;
  `detail_ratio` applies when Log is focused; otherwise Log is the compact
  drawer. `footer_help` adds the extra key sheet.
- `tools.editor` and `tools.external_diff` split a program from its arguments,
  accept quoted words, reject invalid quoting, and are not passed through a
  shell.

## Rust library API migration

`0.3.0` changes the public request API:

- `ActionRequest::target: Option<PathBuf>` becomes
  `ActionRequest::targets: Vec<PathBuf>`.
- An empty `targets` vector means all targets for optional-target actions such
  as Apply and Verify.
- `ChezmoiClient::diff` takes a multi-target slice: `diff(&[PathBuf])`.

CLI command names and existing operational flags remain compatible. The
documented exceptions are stricter errors for missing explicit TUI config files
and rejection of unknown TUI configuration keys.

## Security dependency updates

The baseline [OSV querybatch](https://api.osv.dev/v1/querybatch) checked 211
registry packages and found five advisory-matched crate versions:
[`anyhow` 1.0.101](https://rustsec.org/advisories/RUSTSEC-2026-0190.html),
[`lru` 0.16.3](https://rustsec.org/advisories/RUSTSEC-2026-0253.html),
[`rand` 0.8.5](https://rustsec.org/advisories/RUSTSEC-2026-0097.html),
[`rustix` 1.1.3](https://rustsec.org/advisories/RUSTSEC-2026-0486.html), and
[`tokio` 1.49.0](https://rustsec.org/advisories/RUSTSEC-2026-0386.html). All
were RustSec informational `unsound` advisories. An advisory match identifies
an affected version range; it does not by itself establish reachability or an
exploitable vulnerability in this application.

The lockfile now uses patched versions, including `anyhow` 1.0.104, `lru`
0.18.5, `rand` 0.8.6, `ratatui` 0.30.2 with `ratatui-core` 0.1.2, `rustix`
1.1.6, and `tokio` 1.54.1. The final OSV querybatch checked 223 registry
packages on 2026-11-12 and reported zero advisory matches.

Zero known matches is not proof that the graph has no unknown or future
vulnerabilities. The scan covers the current lockfile and must be repeated if
that lockfile changes before publication.

## Performance

The final shared PTY run completed 14 of 14 cases for both v0.2.2 and v0.3.0.
Synthetic fixtures used three source files, 5,000 fake Git objects in the large
scenario, and a 120x40 terminal.

| Scenario | v0.2.2 first status p50 | v0.3.0 first status p50 | Difference |
| --- | ---: | ---: | ---: |
| small | 204.485 ms | 36.468 ms | 82.17% lower, about 5.61x |
| large-git | 204.482 ms | 36.469 ms | 82.17% lower, about 5.61x |

First-frame medians were 3.537 ms for small and 3.532 ms for large-git, close
to the v0.2.2 range. No first-frame improvement is claimed. Both v0.3.0 status
medians are approximately equal, so the result does not demonstrate a
large-git-specific benefit. Method and host details are in
[the performance method](docs/PERFORMANCE.md).

## Known limitations

- Non-UTF-8 path-bearing machine output is rejected before parsing. Managed names
  containing LF or CR also fail closed before status parsing because the status
  line format cannot represent them unambiguously.
- Interactive key, encryption, decryption, custom secret-manager, archive,
  container, SSH, and upgrade workflows remain explicit foreground chezmoi shell
  operations. Their advanced options are intentionally not duplicated by forms.

## Upgrade from v0.2.x

Upgrade with:

```bash
cargo install chezmoi-tui --version 0.3.0 --locked --force
```

Binary users can install the archive for their platform from the
[GitHub release](https://github.com/tetsuya-dev-jp/chezmoi-tui/releases/tag/v0.3.0).
The release workflow publishes Linux x86_64/aarch64 and macOS x86_64/aarch64
archives.

Existing TUI configuration keys documented for `v0.2.x` remain recognizable,
including layout and safety settings. Unknown or misspelled keys are rejected
instead of being ignored. Use `--config` for chezmoi-tui TOML and
`--chezmoi-config` for chezmoi's own config.

## Local verification

The final local checks passed:

- formatting, locked all-target check, and all-target Clippy with warnings denied;
- Rust 1.99.1 and Rust 1.88.0 tests: 312 library, 1 main, 9 public-flow, and
  39 fake-chezmoi tests, for 361 passed and zero failed on each toolchain;
- macOS aarch64 all-target cross-Clippy with warnings denied;
- actionlint for both workflows;
- the final release binary in an actual PTY smoke against official chezmoi
  `v2.74.1`;
- four regenerated synthetic SVG screenshots with privacy review; and
- the final paired startup benchmark.

The macOS result is a cross-compilation Clippy check. Repository CI is the
runtime check for macOS rather than this Linux host.

The release workflow validates the tag/version relationship, builds four
archives, runs the locked package dry-run as part of publishing, and creates a
draft GitHub release. A maintainer verifies the archives, then publishes the
release. External package availability is evidenced by those destinations and
their publication metadata.
