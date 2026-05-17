# chezmoi-tui

<img width="803" height="461" alt="screenshot" src="https://github.com/user-attachments/assets/219eb2c8-7faf-4273-9bc0-0f3ac84cb1ac" />

`chezmoi-tui` is a Rust TUI that makes day-to-day `chezmoi` operations easier to inspect and run.
It focuses on visual clarity, fast navigation, and safer execution for common workflows.

## Why

`chezmoi` is powerful, but CLI-only usage can hide important context:

- what changed
- what is managed vs unmanaged
- which action is safe to run next

`chezmoi-tui` wraps core operations in a 3-pane interface with view-aware actions and safety checks.

## Scope

This project intentionally implements a practical subset of `chezmoi` commands.

View behavior:

| View | Data base path | Auto detail on selection | Tree expand/collapse |
| --- | --- | --- | --- |
| `status` | Home destination | Diff | No |
| `managed` | Home destination | File preview | Yes |
| `unmanaged` | Current working directory | File preview | Yes |
| `source` | chezmoi source directory | File preview | Yes |

Notes:

- Symlink directories are shown as directories, but they are not expanded by default.
- In `managed`/`unmanaged` trees, symlink markers are:
  - `[L]` for symlink directories
  - ` L ` for symlink files
  - `@` suffix on symlink names

## Installation

### Requirements

- Rust 1.85+ (edition 2024)
- `chezmoi` in `PATH`
- macOS or Linux

### Install from crates.io (recommended)

```bash
cargo install chezmoi-tui
```

### Run installed binary

```bash
chezmoi-tui
```

### Run from source (development)

```bash
git clone https://github.com/tetsuya-dev-jp/chezmoi-tui.git
cd chezmoi-tui
cargo run
```

## Usage

Start the app; it automatically loads the current state on startup:

```bash
chezmoi-tui
```

Useful options:

```bash
chezmoi-tui --destination ~/test-home
chezmoi-tui --source ~/.local/share/chezmoi
chezmoi-tui --view managed
chezmoi-tui --view source
chezmoi-tui --log-file /tmp/chezmoi-tui.log
chezmoi-tui --config ~/.config/chezmoi-tui/config.toml
chezmoi-tui --no-config
chezmoi-tui --no-auto-preview
```

## Core Workflow

1. Wait for the initial refresh, or press `r` to refresh again.
2. Switch views with `1`/`2`/`3`/`4`.
3. Move with `j`/`k` or arrow keys.
4. In `status`, diff is auto-loaded for selected file.
5. In `managed` / `unmanaged` / `source`, preview is auto-loaded for selected file with its origin in the detail title.
6. Use `Space` to mark multiple items and run batch actions from `a`.

The footer shows the active view base path, for example `dest=/home/user`, `cwd=/work/project`, or `source=~/.local/share/chezmoi`.

## Keybindings

Global:

| Key | Behavior |
| --- | --- |
| `1` / `2` / `3` / `4` | Switch view (`status`, `managed`, `unmanaged`, `source`) |
| `Tab` | Cycle focus (`List` -> `Detail` -> `Log`) |
| `a` | Open action menu |
| `r` | Refresh all lists |
| `?` | Open help and legends |
| `!` | Open notice history |
| `q` / `Ctrl+C` | Quit |

List focus:

| Key | Behavior |
| --- | --- |
| `j` / `k` or `↑` / `↓` | Move selection |
| `/` | Open list filter |
| `Space` | Toggle multi-select mark |
| `c` | Clear all marks |
| `h` / `l` or `←` / `→` | Collapse/expand tree (`managed`, `unmanaged`) |
| `d` or `Enter` | Load diff for selected file |
| `v` | Load file preview |
| `e` | Run `edit` on selected target (managed files only) |

Detail or log focus:

| Key | Behavior |
| --- | --- |
| `j` / `k` or `↑` / `↓` | Scroll |
| `PgUp` / `PgDn` | Page scroll |
| `Ctrl+u` / `Ctrl+d` | Half-page scroll |
| `/` | Search focused pane |

Action menu:

| Key | Behavior |
| --- | --- |
| type text | Filter by action label |
| `danger:<action>` | Reveal/filter dangerous actions (`destroy`, `purge`) |
| `↑` / `↓` | Move |
| `Enter` | Continue to preflight or execute low-risk action |
| `Esc` | Close |

Preflight / Apply Plan modals:

| Key | Behavior |
| --- | --- |
| `j` / `k` or `↑` / `↓` | Scroll review content |
| `PgUp` / `PgDn` | Page review content |
| `Enter` | Continue to confirmation/execution |
| `Esc` | Cancel before running |
| `d` | Load full diff from Apply Plan |

## Implemented Actions

Action visibility is view-aware.

| View | Actions |
| --- | --- |
| `status` | `apply`, `doctor`, `data`, `open-source-dir`, `update`, `edit-config`, `edit-config-template`, `edit-ignore`, `re-add`, `merge`, `merge-all`, `edit`, `forget`, `chattr`, `purge` |
| `managed` | `apply`, `doctor`, `data`, `open-source-dir`, `update`, `edit-config`, `edit-config-template`, `edit-ignore`, `edit`, `forget`, `chattr`, `destroy`, `purge` |
| `unmanaged` | `add`, `ignore`, `apply`, `doctor`, `data`, `open-source-dir`, `update`, `edit-config`, `edit-config-template`, `edit-ignore`, `purge` |
| `source` | `apply`, `doctor`, `data`, `open-source-dir`, `update`, `edit-config`, `edit-config-template`, `edit-ignore`, `purge` |

`add` opens an attribute wizard before importing files. Selected attributes are applied after add with `chattr`:

- `template`
- `private`
- `executable`
- `encrypted`

`ignore` opens a wizard with modes:

- `Auto` (file: exact, directory: `/**`)
- `Exact path`
- `Direct children` (`/*`)
- `Recursive` (`/**`)
- `Global by name` (example: `**/.git/**`)

## Safety Model

- Strict confirmation is always required for dangerous actions: `destroy`, `purge`.
- `destroy` and `purge` require typed confirmation phrases.
- Dangerous actions are hidden from plain action-menu filtering; use `danger:destroy` or `danger:purge` to reveal/filter them explicitly.
- The help overlay explains status symbols, views, tree markers, symlink markers, and danger filtering.
- The action menu can show unavailable actions with disabled reasons.
- Confirmation is also required for broad or state-changing actions: `apply`, `update`, `merge-all`, `forget`, `chattr`.
- Broad or risky operations open a preflight review before execution.
- Preflight shows the action, impact, command preview, and affected targets.
- `apply` opens an Apply Plan review before the normal confirmation.
- Apply Plan lists every pending change grouped by added/modified/deleted/run/unknown.
- Multi-target operations show all selected targets before any command runs.
- Batch confirmation is reused for remaining queued non-dangerous items after the first confirmation.
- Dangerous batch items require per-target confirmation.
- `edit` is restricted to managed files.
- Directory-wide `add` is blocked to avoid accidental bulk imports.
- `forget` and `purge` run with `--force --no-tty` to avoid TUI deadlocks.
- Interactive tools run in foreground (for example merge tool/editor flows).

## Configuration

Configuration is optional. Without a config file, built-in safe defaults are used.

Default config path:

```text
~/.config/chezmoi-tui/config.toml
```

Example:

```toml
[ui]
default_view = "status"
auto_preview = true
show_base_context = true
show_notices = true

[safety]
require_two_step_confirmation = true
show_apply_plan = true

[tools]
editor = "nvim"
external_diff = "delta"
```

CLI flags override config file values. Use `--config <path>` to load a specific config file or `--no-config` to disable config loading.

## Features

- 3-pane layout (List / Detail / Log)
- Rich diff rendering (hunk headers, line numbers, status-aware styling)
- File preview with extension-based syntax highlighting
- Tree navigation in `managed` and `unmanaged`
- Symlink-aware rendering and preview messages (directory link / broken link handling)
- Multi-select batch execution for selected-item actions
- Log auto-follow with manual scrolling
- Busy indicator includes the current task when available
- Common errors include recovery hints in the notice/log
- Help/legend overlay for views, status columns, tree markers, and safety rules
- Notice history for recent info/success/error messages
- Detail and log pane search
- Built-in safe defaults (no application config file)

## Development

```bash
cargo fmt
cargo test
cargo clippy --all-targets -- -D warnings
```

## Contributing

Issues and pull requests are welcome.
Please update README and tests together when behavior changes.

## License

MIT (see package metadata in `Cargo.toml`).
