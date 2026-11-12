# Original-audit traceability ledger

This ledger preserves every item from the three source audits while keeping the
canonical product findings in [AUDIT.md](AUDIT.md) concise. IDs are namespaced:

- E- items come from the original engine audit.
- U- items come from the original UX prioritized table.
- C- items come from the original feature audit and maintenance suggestions.

Coverage is 28 engine items, 24 UX items, 32 feature items, and 6 maintenance
areas: 90 total rows. The final test matrix passed 361 tests on both Rust
1.99.1 and Rust 1.88.0.

| Status | Meaning |
| --- | --- |
| Implemented | The source, tests, and final local gate evidence support the documented behavior. |
| Partial | A concrete portion exists with a documented limitation or an unmeasured optional idea. |
| Intentional | The original concern is resolved by a documented product boundary or configuration decision. |
| Rejected | The proposal is outside the maintenance scope, duplicates native chezmoi, or adds UI complexity without a demonstrated need. |
| Unmeasured | The structural change exists, but the stated performance claim lacks an isolated measurement. |

## Resolved defect notes

E-L12 now uses a nonblocking flock and assembles the separator, rule, and final
newline in one payload buffer. The source and final tests also cover a 4 MiB cap,
FIFO rejection, symlink-to-regular-file handling, and lock contention. The design
does not use an atomic rename transaction. E-M9 is an intentional bounded
local-edit design: CLI root lookups were removed, local metadata and write
operations remain synchronous, and the product does not claim a time bound or
that a network filesystem can never stall. E-L2/U-P0-5 now cancel obsolete detail
at the actual process boundary without stale errors or mutation cancellation.

## Engine audit traceability

| Original | Canonical | Status | Final disposition | Source or test anchor |
| --- | --- | --- | --- | --- |
| E-H1 | F-001 | Implemented | Destination is resolved separately from an explicit override and every client or foreground command uses the session destination. | <code>src/infra.rs destination_for_target_always_uses_resolved_session_destination</code> |
| E-H2 | F-001, F-004 | Intentional | cwd remains a browse base for Unmanaged. Add and AddDirectory are rejected outside destination; outside Ignore is allowed only through explicit Global by name. | <code>src/actions.rs existing_directory_policy_rejects_outside_destination_mutation</code> |
| E-H3 | F-003 | Implemented | source-path supplies the effective root for Source, ignore, preview, and source actions while the configured source remains separate. | <code>src/app.rs source_refresh_keeps_original_and_effective_roots_separate</code> |
| E-H4 | F-004 | Implemented | Unmanaged ignore matching uses the correct working base and fails closed when rendered rules are unavailable. | <code>src/app.rs unmanaged_without_ignore_rules_fails_closed_for_descendants</code> |
| E-M1 | F-002 | Implemented | Marked Apply requests become one target vector feeding one reviewed fresh validation and apply job. | <code>tests/action_flows.rs batch_apply_confirmation_enqueues_one_atomic_reviewed_job</code> |
| E-M2 | F-007, F-014 | Implemented | Preview identity includes origin and base context; stale responses are rejected by request ID. | <code>src/app.rs preview_title_includes_origin</code> and <code>src/handlers.rs stale_preview_event_does_not_overwrite_latest_detail</code> |
| E-M3 | F-013, F-014 | Implemented | Source and Unmanaged selection uses a file preview; diff remains a managed-target or full-diff action. | <code>src/handlers.rs source_enter_previews_source_file_instead_of_running_diff</code> |
| E-M4 | F-006 | Implemented | FIFOs, devices, directories, binary files, and unsupported links are rejected before an unbounded preview read. | <code>src/preview.rs preview_rejects_fifo_without_opening_it</code> |
| E-M5 | F-005 | Implemented | Potentially prompting commands restore the terminal and run foreground; background commands receive null stdin. | <code>src/action_policy.rs execution_metadata_distinguishes_shell_diff_and_inspection</code> |
| E-M6 | F-004, F-012 | Implemented | Ignore patterns escape glob, template, comment, whitespace, newline, NUL, and backslash hazards. | <code>src/ignore.rs all_path_modes_escape_literal_glob_comment_and_template_syntax</code> |
| E-M7 | F-008, F-016 | Partial | Filtering is debounced and uses a bounded incremental index with an incomplete-result signal. No filter latency benchmark exists. | <code>src/app.rs unmanaged_filter_index_scan_is_bounded_without_query_fallback</code> |
| E-M8 | F-008 | Implemented | Marks are stored independently of visible rows and hidden marks remain in the batch target set. | <code>src/app.rs marks_survive_filter_and_targets_include_hidden_paths</code> |
| E-M9 | F-016 | Intentional | source-path lookups moved out of the action. A bounded local metadata check and file update remain synchronous; no time bound or network-filesystem non-stall guarantee is claimed. | <code>src/ignore.rs run_internal_ignore_action</code> |
| E-M10 | F-016 | Implemented, unmeasured | Unmanaged is built from one bounded command path rather than a sequential child subprocess per entry. No N+1 latency benchmark exists. | <code>src/infra.rs unmanaged_paths_are_relative_to_working_dir</code> |
| E-M11 | F-003, F-016 | Partial, unmeasured | Source walking prunes VCS directories and refresh branches run in parallel; source-path work is still duplicated by separate client calls. | <code>src/infra.rs collect_source_paths</code> and <code>src/backend.rs start_refresh</code> |
| E-L1 | F-016 | Partial, unmeasured | Idle polling and status formatting were reduced, but no CPU, frame, or rendering microbenchmark proves a redraw improvement. | <code>src/main.rs busy_poll_uses_short_latency_without_idle_spin</code> |
| E-L2 | F-007, F-019 | Implemented | Running detail work is canceled at the process boundary. The actual-process regression verifies obsolete payload suppression, no stale error, and preservation of unrelated mutation. | <code>src/backend.rs running_detail_is_cancelled_without_stale_error</code> and <code>tests/fake_chezmoi.rs running_detail_kill_leaves_no_stale_error_and_does_not_cancel_mutation</code> |
| E-L3 | F-007 | Implemented | Apply-plan results carry request IDs so a dismissed or stale plan cannot reopen. | <code>src/handlers.rs stale_apply_plan_result_does_not_reopen_modal</code> |
| E-L4 | F-012, F-015 | Implemented | Diff parsing handles ANSI SGR safely, preserves color, and derives hunk and line-number navigation from parsed text. | <code>src/ui_diff.rs line_numbers_follow_hunks_and_ansi_colors_are_preserved</code> |
| E-L5 | F-007, F-015 | Implemented | Tree expansion and selection state are isolated by view. | <code>src/app.rs same_relative_directory_expansion_is_isolated_per_view</code> |
| E-L6 | F-010, F-016 | Implemented | Read-only action completion does not enqueue a full refresh. | <code>src/actions.rs read_only_batch_completion_does_not_request_refresh</code> |
| E-L7 | F-016 | Implemented, unmeasured | Runner polling backs off from 2 to 10 ms instead of a fixed 50 ms interval; no isolated subprocess-latency benchmark exists. | <code>src/infra.rs run_command_with_limits_mode</code> |
| E-L8 | F-012 | Implemented | Machine output rejects timeout, output limit, truncation, nonzero status, or incomplete required records before success. | <code>src/infra.rs ensure_complete_success_rejects_truncated_output</code> |
| E-L9 | F-012 | Implemented | NUL path records preserve whitespace. Invalid UTF-8 and managed names containing LF/CR fail closed; status refuses the ambiguous line-oriented input before parsing. | <code>tests/fake_chezmoi.rs shell_client_rejects_status_when_managed_name_can_inject_a_valid_row</code> |
| E-L10 | F-003, F-004 | Implemented | Ignore templates are rendered through chezmoi with file-relative template context rather than a synthetic stdin sourceFile. | <code>src/infra.rs execute_template</code> |
| E-L11 | F-010, F-015 | Implemented | Diff rendering derives real old/new line numbers from hunks and displays two columns; the README matches that behavior. | <code>src/ui_diff.rs line_numbers_follow_hunks_and_ansi_colors_are_preserved</code> |
| E-L12 | F-004 | Implemented | Nonblocking flock, one buffered append, a 4 MiB cap, and non-regular-file rejection prevent the original split-write and unbounded-read failures. No atomic rename claim is made. | <code>src/ignore.rs append_unique_line_rejects_oversized_file_before_append</code> and <code>append_unique_line_lock_contention_leaves_original_intact</code> |
| E-L13 | F-017 | Implemented | Final Linux tests, macOS-target Clippy, actionlint, and pinned real-chezmoi smoke cover the integration surface. | <code>tests/action_flows.rs</code> and <code>.github/workflows/ci.yml</code> |

## UX audit traceability

| Original | Canonical | Status | Final disposition | Source or test anchor |
| --- | --- | --- | --- | --- |
| U-P0-1 | F-009 | Implemented | Modals use bounded responsive geometry and required confirmation, wizard, or error content remains visible at compact sizes. | <code>src/ui.rs compact_modals_keep_required_content_visible</code> |
| U-P0-2 | F-010 | Implemented | Action filter uses a compact layout that retains its key-help/status area. | <code>src/ui.rs action_menu_rows_are_grouped_into_sections</code> |
| U-P0-3 | F-008 | Implemented | Filtering no longer destroys marks for hidden paths. | <code>src/app.rs marks_survive_filter_and_targets_include_hidden_paths</code> |
| U-P0-4 | F-008, F-016 | Implemented, unmeasured | List-filter typing is staged and flushed after a debounce; the Unmanaged index is bounded. No keystroke latency benchmark exists. | <code>src/handlers.rs list_filter_typing_is_debounced</code> |
| U-P0-5 | F-007, F-019 | Implemented | Preview storms are coalesced, stale replies rejected, and an already running obsolete detail process canceled without affecting mutation. | <code>tests/fake_chezmoi.rs running_detail_kill_leaves_no_stale_error_and_does_not_cancel_mutation</code> |
| U-P0-6 | F-011 | Implemented | Quit is refused while mutation or batch work is active and allowed while only read-only loads run. | <code>src/handlers.rs quit_blocks_mutations_but_allows_read_only_loads</code> |
| U-P0-7 | F-016 | Partial, unmeasured | Idle/busy polling and status formatting improved; no full-render CPU or incremental-render benchmark exists. | <code>src/main.rs</code> and <code>src/app.rs current_items_window</code> |
| U-P1-1 | F-005, F-013 | Implemented | Preflight mode is generated from typed foreground/background execution metadata. | <code>src/ui.rs preflight_mode_uses_action_metadata</code> |
| U-P1-2 | F-010 | Implemented | List mutation and detail navigation keys are focus-gated. | <code>src/handlers.rs list_navigation_and_palette_keys_are_focus_gated</code> |
| U-P1-3 | F-010 | Intentional | Core focus hints stay visible; footer_help adds a third sheet for additional keys instead of being required for basic operation. | <code>src/ui.rs footer_help_adds_a_third_key_sheet</code> |
| U-P1-4 | F-009, F-015 | Implemented | List, detail, log, loading, empty, and no-match states are distinct and contextual. | <code>src/ui.rs empty_states_distinguish_loading_and_no_filter_matches</code> |
| U-P1-5 | F-011, F-012 | Implemented | Errors remain in state when informational notices are disabled and component refresh does not overwrite the latest error. | <code>src/handlers.rs component_errors_stay_in_state_when_notice_rendering_is_disabled</code> |
| U-P1-6 | F-015 | Partial, intentional | Monochrome markers and NO_COLOR are supported. A full theme engine is rejected as maintenance burden without a current product need. | <code>src/ui.rs clearing_styles_preserves_no_color_text</code> |
| U-P1-7 | F-015 | Implemented | Visible-width calculations use terminal cells for CJK, emoji, and combining text. | <code>src/ui.rs long_unicode_path_preserves_the_visible_tail</code> |
| U-P1-8 | F-010 | Implemented | Action palette supports grouped sections, Ctrl-N/Ctrl-P navigation, exact-match ranking, and separate j-as-text behavior while filtering. | <code>src/handlers.rs action_filter_treats_j_as_text_and_ctrl_n_as_navigation</code> |
| U-P1-9 | F-010, F-014 | Implemented | README and UI now agree on Source, diff, cat, focus, configuration, and action behavior. | <code>README.md</code> and <code>docs/AUDIT.md</code> |
| U-P1-10 | F-015 | Implemented | Log styling recognizes explicit prefixes rather than arbitrary substring matches, and the title exposes follow/paused state. | <code>src/ui.rs styled_log_line</code> and <code>log_scroll_offset_keeps_latest_visible</code> |
| U-P1-11 | F-010 | Implemented | BackTab, Home/End, g/G, page movement, and non-wrapping bounded selection are available. | <code>src/app.rs pane_focus_reverses_in_opposite_order</code> |
| U-P2-1 | F-010, F-015 | Implemented | Path rows use pane-width-aware compaction and preserve the meaningful filename tail. | <code>src/ui.rs compact_path_row</code> |
| U-P2-2 | F-009, F-015 | Implemented | Titles and hints share compact width-aware clipping instead of unbounded fixed-width strings. | <code>src/ui.rs clip_title</code> and <code>compact_middle</code> |
| U-P2-3 | F-010 | Partial, rejected expansion | A visible input cursor and basic editing keys exist. A complete embedded text editor with paste, word edit, and full cursor movement is rejected because these are short forms. | <code>src/handlers.rs handle_input_key</code> and <code>src/ui.rs input_modal_places_cursor_after_the_value</code> |
| U-P2-4 | F-015 | Rejected | The modal clears its area; a full backdrop-dim theme layer is rejected as visual complexity without a demonstrated usability failure. | <code>src/ui.rs draw_modal</code> |
| U-P2-5 | F-009 | Intentional | list_ratio and detail_ratio have separate documented semantics rather than being forced into one complementary pair. | <code>src/config.rs</code> and <code>README.md</code> |
| U-P2-6 | F-015 | Rejected | No scrollbars are added to compact terminal panes; position counts, truncation, focus, and follow state cover the primary need. | <code>src/ui.rs</code> |

## Feature audit traceability

| Original | Canonical | Status | Final disposition | Source or test anchor |
| --- | --- | --- | --- | --- |
| C-P0-1 | F-001 | Implemented | Effective destination resolution and explicit override separation prevent wrong-tree reads and writes. | <code>src/infra.rs destination_for_target_always_uses_resolved_session_destination</code> |
| C-P0-2 | F-005 | Implemented | Prompting apply, destroy, merge, editor, SSH, container, and update work uses foreground handoff; parseable background work uses null stdin. | <code>src/action_policy.rs</code> and <code>src/actions.rs run_foreground_action</code> |
| C-P0-3 | F-014 | Intentional | Source remains a filtered raw source tree under the effective root. It is labeled raw and does not claim decoded source-state semantics. | <code>src/infra.rs collect_source_paths</code> and <code>README.md</code> |
| C-P0-4 | F-004 | Implemented | Rendered-rule failure disables descendant trust, search, preview, and import rather than showing ignored descendants. | <code>src/app.rs unmanaged_without_ignore_rules_fails_closed_for_descendants</code> |
| C-P0-5 | F-001, F-004 | Implemented | One effective base governs Unmanaged listing, target-path mapping, and ignore matching. | <code>src/app.rs unmanaged_ignore_base_is_working_dir_outside_destination</code> |
| C-P0-6 | F-004 | Implemented | Template escaping, root selection, control-character rejection, 4 MiB capping, non-regular-file rejection, flock, one-payload append, and unchanged-file behavior pass the final suite. | <code>src/ignore.rs append_unique_line_lock_contention_leaves_original_intact</code> |
| C-P0-7 | F-010, F-011 | Intentional | Typed DESTROY and PURGE phrases remain mandatory. require_two_step_confirmation cannot downgrade them and is documented as an extra-step switch. | <code>src/handlers.rs destroy_requires_phrase_even_when_two_step_config_is_disabled</code> |
| C-P1-1 | F-013 | Partial | ignored is a typed read-only action with result routing. A separate fifth Ignored tab was rejected to keep the four-view layout. | <code>src/domain.rs Action::Ignored</code> |
| C-P1-2 | F-013 | Implemented | verify accepts selected or all targets and treats exit 1 as drift rather than a generic failure. | <code>tests/action_flows.rs verify_distinguishes_match_from_drift_without_failure_or_refresh</code> |
| C-P1-3 | F-013, F-014 | Implemented | cat is an explicit rendered-target action; navigation never auto-renders and output is not persisted. | <code>tests/action_flows.rs cat_uses_the_exact_managed_selection_and_renders_output_without_refresh_or_log_leak</code> |
| C-P1-4 | F-013 | Implemented | Fixed Git status, diff, log, stage, commit, push, and source-only pull actions are present without arbitrary argv entry. | <code>src/infra.rs new_action_arguments_follow_the_feature_contract</code> |
| C-P1-5 | F-005 | Implemented | open-source-dir delegates to foreground chezmoi cd rather than launching a raw shell in the source directory. | <code>src/actions.rs run_open_source_dir_foreground</code> |
| C-P1-6 | F-005, F-013 | Intentional | external-diff now means the app-configured external tool and falls back to native chezmoi diff. It is documented as an app setting rather than a second chezmoi diff configuration engine. | <code>src/actions.rs run_external_diff_foreground</code> |
| C-P1-7 | F-012 | Intentional limitation | Output is bounded and strict path records fail closed. Non-UTF-8 paths and managed names containing LF/CR are unsupported rather than lossy-converted or aliased into status rows. | <code>src/infra.rs strict_path_output_rejects_invalid_utf8</code> |
| C-P1-8 | F-002, F-013 | Implemented | File Add, recursive directory preview/add, managed-directory plans, recursive diff, and exact managed actions have explicit target policies. No exact or follow mode is added. | <code>tests/action_flows.rs directory_preview_is_read_only_but_add_requires_confirmation_and_stays_in_destination</code> |
| C-P1-9 | F-013 | Intentional | The native Add/Chattr forms expose safe common attributes; unsupported labels are rejected instead of passed through as speculative argv. | <code>src/infra.rs add_rejects_unknown_attribute_labels</code> |
| C-P1-10 | F-016 | Implemented, unmeasured | Unmanaged avoids sequential per-child commands. No isolated large-tree throughput benchmark exists. | <code>src/infra.rs parse_unmanaged_output</code> |
| C-P1-11 | F-002 | Implemented | Mutation batches stop on first failure and refresh partial state; broad Apply remains one reviewed request. | <code>src/handlers.rs batch_timeout_stops_remaining_targets_and_refreshes_partial_state</code> |
| C-P1-12 | F-011, F-012 | Implemented | Logs are user-only on Unix, sensitive stdout is not persisted, output is bounded, and non-regular log files are rejected. | <code>src/diagnostics.rs created_log_file_is_user_only</code> |
| C-P1-13 | F-010 | Implemented | An explicit missing TUI config path is an error while an absent implicit platform config remains optional. | <code>src/config.rs explicit_missing_config_is_an_error</code> |
| C-P1-14 | F-010 | Implemented | README records platform paths, the 37-action contract, strict confirmation, and distinct CLI configuration flags. | <code>README.md</code> |
| C-P1-15 | F-014 | Implemented | Source attribute hints follow documented prefixes/suffixes, stop at literal, and reject substring false positives. | <code>src/app.rs source_attr_markers_ignore_attribute_substrings</code> |
| C-P2-1 | F-010 | Rejected | No TUI-owned completions subcommand. Users retain chezmoi and shell-native completion workflows; adding clap_complete is maintenance outside this modernization. | <code>src/cli.rs</code> |
| C-P2-2 | F-010 | Implemented | --chezmoi selects the binary, while --chezmoi-config remains distinct from the TUI --config. | <code>src/config.rs chezmoi_options_are_cli_only</code> |
| C-P2-3 | F-010, F-013 | Partial | Startup fails with actionable no-chezmoi guidance, Init is explicit, and doctor is available. A new pre-alternate-screen initialization wizard is rejected. | <code>src/main.rs</code> and <code>src/domain.rs Action::Init</code> |
| C-P2-4 | F-014 | Implemented | Managed and Status navigation previews destination bytes; cat is the explicit rendered-target comparison path. | <code>tests/action_flows.rs cat_uses_the_exact_managed_selection_and_renders_output_without_refresh_or_log_leak</code> |
| C-P2-5 | F-013 | Rejected | No external-entry badge or separate refresh-externals action. Externals remain native chezmoi behavior and apply refreshes them. | <code>src/domain.rs Action::ALL</code> |
| C-P2-6 | F-012 | Implemented | Refresh publishes each view independently, so one failing component does not discard successful lists. | <code>src/handlers.rs refresh_error_survives_later_component_success</code> |
| C-P2-7 | F-009 | Implemented | Managed directories derive expandability from managed descendants and retain directory-only branches. | <code>src/app.rs managed_view_keeps_directory_only_branch_without_managed_files</code> |
| C-P2-8 | F-004 | Implemented | Unmanaged excludes ignored descendants; ignored names are inspected through the explicit read-only action rather than offered for Add. | <code>src/app.rs unmanaged_tree_excludes_ignored_children_on_expand</code> |
| C-P2-9 | F-013 | Rejected | Doctor remains the native default command. Offline variants remain available through explicit CLI shell workflow instead of a second action form. | <code>src/infra.rs Action::Doctor</code> |
| C-P2-10 | F-017, F-018 | Implemented | CI pins and downloads official chezmoi v2.74.1 and runs an isolated PTY smoke; final-tree status remains a release gate. | <code>.github/workflows/ci.yml real-chezmoi</code> |

## Maintenance suggestion traceability

| Original | Canonical | Status | Final disposition | Source anchor |
| --- | --- | --- | --- | --- |
| C-9.1 | F-017, F-018 | Partial | CI has stable/MSRV, format, check, Clippy, tests, screenshots, and one job timeout. Read permissions, broad concurrency/timeouts, cargo deny, semver-checks, doc-tests, and coverage are not implemented or claimed. | <code>.github/workflows/ci.yml</code> |
| C-9.2 | F-017, F-018 | Implemented | Pinned real-chezmoi v2.74.1 smoke passed with the final release binary and actual PTY. | <code>scripts/real_chezmoi_smoke.py</code> and <code>.github/workflows/ci.yml</code> |
| C-9.3 | F-004, F-012 | Rejected, not implemented | No fuzz or real-chezmoi differential fuzz harness is claimed. Deterministic parser and ignore tests cover current regressions without adding a maintenance-heavy harness. | <code>src/ignored_scan.rs</code> and <code>src/infra.rs</code> tests |
| C-9.4 | F-017 | Intentional | Main CI and the release workflow cross-compile macOS ARM on `macos-15-intel` and validate Mach-O `arm64` with `lipo`. The release workflow builds four archives, validates versions, dry-runs publishing, and creates a draft release; ARM runtime execution is not tested. A maintainer verifies the tagged commit and archives before publication. Checksums are outside the configured workflow. | <code>.github/workflows/publish.yml</code> and <code>.github/workflows/ci.yml</code> |
| C-9.5 | F-017 | Partial | Dependabot is present; RELEASE_NOTES is the changelog. SECURITY.md, CONTRIBUTING.md, issue templates, and a custom cargo include set are outside this documentation modernization. | <code>.github/dependabot.yml</code> |
| C-9.6 | F-017, F-018 | Intentional | All configured local gates pass, including both test toolchains, screenshots, real-chezmoi smoke, and performance. cargo deny remains an optional future hardening step outside the configured gate. | <code>.github/workflows/ci.yml</code> and <code>.github/workflows/publish.yml</code> |

## Performance claims not isolated by the benchmark

| Claim area | What is established | What is not established |
| --- | --- | --- |
| Selected-status startup | v0.2.2 and v0.3.0 first-status medians with 14 of 14 cases per dataset; about 82.17% lower. | No claim beyond this synthetic PTY fixture. |
| First frame | Both current scenarios are in the low single-digit millisecond range. | No claimed improvement or regression; small and large moved in opposite directions. |
| Dot-git pruning | The large synthetic scenario includes 5,000 fake objects. | No large-git advantage: final small and large status medians are both about 36.47 ms. |
| Filtering | Debounce and bounded-index code paths exist. | No keystroke or large-tree filter latency measurement. |
| Preview coalescing | Latest-slot, request-ID, and actual-process cancellation tests pass. | The targeted three-second regression is not a general latency benchmark. |
| Redraw and polling | Idle polling, busy polling, and formatting have structural changes. | No CPU, frame-time, allocation, or render microbenchmark. |
| Unmanaged N+1 | The product no longer uses a sequential child-command-per-entry design. | No isolated multi-child subprocess benchmark. |
| Runner polling | The runner backs off at 2 to 10 ms rather than a fixed 50 ms poll. | No isolated command-latency benchmark. |

## Explicit non-claims

- No screen-reader or broader assistive-technology conformance is claimed.
- No full theme engine or dimmed modal backdrop is claimed.
- No complete embedded input editor is claimed.
- No TUI-owned shell-completions generator is claimed.
- No external-entry badge or separate external refresh command is claimed.
- No fuzz harness or differential ignore fuzzer is claimed.
- No custom secret manager or automatic decrypted-content renderer is claimed.
- No arbitrary command or shell executor is claimed.
- No complete decoded source-state view is claimed; Source remains labeled raw.
- No external service availability beyond distribution metadata is inferred.

## Final evidence summary

| Evidence | Result |
| --- | --- |
| Rust tests | 361 passed and 0 failed on each of Rust 1.99.1 and Rust 1.88.0 |
| Build gates | formatting, all-target check, Clippy, macOS-target cross-Clippy, and actionlint passed |
| Real integration | official chezmoi v2.74.1 actual PTY smoke passed with the release binary |
| Ignore safety | cap, FIFO, lock-contention, symlink, unchanged-file, escaping, and root cases passed |
| Detail cancellation | obsolete payload suppressed, no stale error, unrelated mutation preserved |
| Startup | both paired datasets 14/14; first-status p50 about 82.17% lower |
| Security scan | 223 registry packages, zero known OSV matches |
| Screenshots | four final synthetic SVGs regenerated and privacy-reviewed |

## Coverage audit

- Engine: 28 of 28 original summary IDs are present.
- UX: 24 of 24 prioritized table IDs are present.
- Feature: 32 of 32 P0/P1/P2 IDs are present.
- Maintenance: 6 of 6 sections 9.1 through 9.6 are present.
- Total: 90 of 90 original items are accounted for.

This ledger intentionally keeps proposals visible even when rejected. The
canonical [AUDIT.md](AUDIT.md) remains the source for severity, release state,
dependency evidence, upstream command coverage, and final closure.
