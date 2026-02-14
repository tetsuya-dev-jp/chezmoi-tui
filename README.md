# chezmoi-tui

A Rust TUI to visualize `chezmoi` state and run major operations safely.

## Current Features (MVP)

- 3-pane UI
- Left: list (`status` / `managed` / `unmanaged`)
- Top-right: diff / file content preview
- Bottom-right: execution log
- `unmanaged` supports directory expansion so you can select only required files
- Action menu
- `apply`, `update`, `re-add`, `merge`, `merge-all`
- `add`, `edit`, `forget`, `chattr`
- `destroy`, `purge`
- Safety mechanisms
- Confirmation dialog only for dangerous actions
- Additional confirmation phrase required for `destroy` / `purge`
- Config persistence
- `~/.config/chezmoi-tui/config.toml` (XDG)

## Requirements

- Rust 1.93+
- `chezmoi` available in `PATH`
- macOS / Linux

## Run

```bash
cargo run
```

## Keybindings

- `1` / `2` / `3`: switch list view (`status`, `managed`, `unmanaged`)
- `j` / `k` or `↑` / `↓`: move selection
- `l` / `→`: expand directory (`unmanaged` view)
- `h` / `←`: collapse directory (`unmanaged` view)
- `Tab`: move pane focus
- `Enter` or `d`: load diff for selected target
- `v`: preview selected file content (read-only)
- Extension-based syntax highlighting is applied in preview
- In `unmanaged` view, preview is loaded automatically when a file is selected
- When a directory is selected, detail pane stays empty
- `j` / `k`: scroll preview/diff when `Detail` pane is focused
- `PgUp` / `PgDn`, `Ctrl+u` / `Ctrl+d`: larger scroll in `Detail` pane
- `a`: open action menu
- `e`: run `edit` for selected file
- `r`: refresh lists
- `q` or `Ctrl+C`: quit

## Implementation Notes

- `managed --format json` can return plain text depending on environment, so both JSON and line-based parsing are supported.
- `status` symbols are mapped into internal model entries.
- Direct `add` on a directory is intentionally blocked to avoid accidents. Expand the directory and select files explicitly.

## Tests

```bash
cargo test
```
