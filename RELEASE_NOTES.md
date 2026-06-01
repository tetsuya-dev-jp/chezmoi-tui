# Release Notes: v0.2.1

## Highlights

This release adds targeted apply support for marked `status` / `managed` entries, building on the v0.2.0 safety and workflow improvements.

## Targeted apply

- Marked `status` / `managed` entries can now run targeted `apply` commands; unmarked `apply` still runs a full `chezmoi apply`.
- Apply Plan is filtered for targeted apply requests so the preview matches the selected target(s).

## v0.2.0 safety and preflight review

- Added preflight review for broad/risky actions.
- Multi-target actions now show affected targets before execution.
- Apply Plan now shows all pending changes grouped by kind instead of only samples.
- Busy indicator now includes the current task when available.
- Common errors now include recovery hints in the notice/log.

## Help and discoverability

- Added `?` Help / Legends overlay.
- Added explanations for views, status columns, tree markers, symlink markers, and danger filtering.
- Action menu now explains unavailable actions with disabled reasons.
- Added notice history via `!`.

## Navigation and readability

- Added Detail/Log search with `n` / `N` next/previous navigation.
- Added diff hunk navigation with `n` / `N` when Detail is showing a diff and no search is active.
- Added horizontal scrolling for Detail/Log via `H` / `L`.
- Added focused pane maximize/restore with `m`.
- Improved long path display in compact list rows.
- Added filter match count in filtered list titles.
- Added tone-aware log styling.

## Advanced workflow improvements

- Added persistent layout config:
  - `ui.default_layout`
  - `ui.list_ratio`
  - `ui.detail_ratio`
  - `ui.footer_help`
- Added `external-diff` foreground action using `tools.external_diff`, `CHEZMOI_TUI_EXTERNAL_DIFF`, or `delta` fallback.
- Added `debug-context` action for troubleshooting current app/view state.
- Added Source view markers for encoded source-name attributes: `{tmpl,priv,exec,enc}`.
- Added wizard previews for ignore patterns and add attributes.

## Verification

Release preparation checks:

```text
cargo fmt --all -- --check
cargo test --locked
cargo clippy --all-targets --locked -- -D warnings
cargo publish --dry-run --locked
```

## Release steps

```bash
git tag v0.2.1
git push origin main
git push origin v0.2.1
```

Publishing is handled by the existing tag-based GitHub Actions workflow using crates.io Trusted Publishing.
