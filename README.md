# chezmoi-tui

`chezmoi-tui` is a Rust TUI that makes `chezmoi` state easier to inspect and operate.
It is built for users who want a visual workflow for day-to-day dotfile management.

## Why this exists

`chezmoi` is powerful, but CLI-only workflows can make it hard to see:

- what changed
- what is managed vs unmanaged
- what action is safe to run next

This project wraps core `chezmoi` operations in a 3-pane interface with safe defaults.

## Quick Start

### Requirements

- Rust 1.93+
- `chezmoi` available in `PATH`
- macOS or Linux

### Run from source

```bash
git clone https://github.com/tetsuya-dev-jp/chezmoi-tui.git
cd chezmoi-tui
cargo run
```

## Core Workflow

1. Press `r` to refresh.
2. Switch views with `1`/`2`/`3` (`status`, `managed`, `unmanaged`).
3. In `status`, selecting a file auto-loads its diff.
4. In `unmanaged`, selecting a file auto-loads preview.
5. Open action menu with `a` and run the needed operation.

## Features

- 3-pane UI
- Left: entries (`status` / `managed` / `unmanaged`)
- Top-right: diff or file preview
- Bottom-right: log (auto-follows latest entries)
- Rich diff rendering with line numbers and hunk headers
- Extension-based syntax highlighting in preview
- Expand/collapse directories in `unmanaged` view
- Persistent app config at `~/.config/chezmoi-tui/config.toml`

## Safety Model

- Confirmation is required only for dangerous actions (`destroy`, `purge`).
- `destroy` and `purge` require a confirmation phrase.
- `edit` is allowed only for managed files.
- Directory-wide `add` is blocked to avoid accidental bulk imports.
- `forget` and `purge` run non-interactively (`--force --no-tty`) to avoid TUI hangs.
- Foreground actions are used for operations that need interactive tools (for example merge tool/editor flows).

## Keybindings

| Key | Behavior |
| --- | --- |
| `1` / `2` / `3` | Switch view (`status`, `managed`, `unmanaged`) |
| `j` / `k` or `↑` / `↓` | Move selection |
| `h` / `l` or `←` / `→` | Collapse/expand directory (`unmanaged`) |
| `Tab` | Cycle focus (`List` → `Detail` → `Log`) |
| `Enter` or `d` | Load diff for selection |
| `v` | Load file preview |
| `PgUp` / `PgDn` | Page scroll in detail pane |
| `Ctrl+u` / `Ctrl+d` | Half-page scroll in detail pane |
| `a` | Open action menu |
| `(in Action Menu) type text` | Filter actions by command name |
| `e` | Run `edit` on selected target |
| `r` | Refresh all lists |
| `q` or `Ctrl+C` | Quit |

## Actions (Current)

- `apply`
- `update`
- `re-add`
- `merge`
- `merge-all`
- `add`
- `edit`
- `forget`
- `chattr`
- `destroy`
- `purge`

## Development

```bash
cargo fmt
cargo test
cargo clippy --all-targets -- -D warnings
```

## Contributing

Issues and pull requests are welcome.
If behavior changes, please update README and tests in the same PR.

## License

MIT (see `Cargo.toml` package metadata).
