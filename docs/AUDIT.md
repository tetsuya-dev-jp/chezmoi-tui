# chezmoi-tui modernization audit

## Evidence status

This is the canonical English audit for the completed 0.3.0 modernization. It
records product dispositions, findings, fixes, measurements, and the evidence
used for engineering closure. External package availability is determined by
the distribution service metadata linked from the README.

| Evidence area | Disposition | Current evidence |
| --- | --- | --- |
| Scope and upstream coverage | Complete | Code, contracts, workflows, tests, dependency graph, and all 52 upstream command pages reviewed. |
| Public API migration | Verified | ActionRequest targets is a Vec of paths; ChezmoiClient diff and all migrated callers accept the target slice. |
| Linux quality gates | Verified | Formatting, locked all-target check, and all-target Clippy with warnings denied passed. |
| Test matrix | Verified | Rust 1.99.1 and Rust 1.88.0 each passed 312 library, 1 main, 9 public-flow, and 39 fake-chezmoi tests: 361 passed, 0 failed. |
| Cross-target and workflow checks | Verified | macOS aarch64 all-target cross-Clippy and actionlint for both workflows passed. This is not a claim of local macOS runtime execution. |
| Release-binary smoke | Verified | The final release binary passed an actual PTY smoke against official chezmoi v2.74.1. |
| Dependency scan | Verified for known advisories | Final OSV query checked 223 registry packages with zero matches on 2026-11-12. |
| Startup performance | Final measurement | Paired synthetic runs completed 14/14 for baseline and final; first-status median improved about 82.17%. |
| Detail cancellation | Verified | Actual process regression canceled obsolete detail without stale error or mutation cancellation. |
| Screenshot review | Verified | Four final synthetic SVG views were regenerated and reviewed without user identifiers. |
| Distribution | Configured | Main CI and the tag workflow cross-compile macOS ARM on `macos-15-intel` and validate Mach-O `arm64` with `lipo`; the tag workflow also builds four archives, dry-runs publishing, publishes the crate, and creates a draft GitHub release. ARM runtime execution is not tested. |

## Scope and baseline

The audit baseline was commit b297fcf (v0.2.2). Its automated suite passed 193
unit tests and 19 integration tests, for 212 total tests. The modernization
targets official chezmoi v2.74.1 at commit 2f68f47e.

The review covered the Rust engine, process runner, backend queues, action
policy, handlers, terminal layout, configuration, ignore processing, preview
reader, tests, CI and release workflows, dependency graph, and the official
command reference. No production dotfiles or user configuration were used in
the audit.

The command snapshot contains 52 pages under
[the official command reference](https://www.chezmoi.io/reference/commands/):
48 canonical command families and four alias pages. The matrix below covers all
52 pages and groups aliases with their canonical command.

[FINDINGS.md](FINDINGS.md) preserves all 90 original engine, UX, feature, and
maintenance items, including implemented, partial, rejected, intentionally
unimplemented, and unmeasured proposals. It is the detailed traceability ledger;
this audit remains the canonical severity and release source.

## Method and evidence labels

Evidence came from independent source audits, isolated fake-chezmoi probes,
official chezmoi v2.74.1 smoke runs, current source inspection, the feature
contract, the final dependency report, screenshots, and paired PTY benchmarks.

- **Observed** means reproduced from source behavior, a test, a command, or a
  measurement.
- **Plausible cause** means supported by code structure but not proven to be the
  cause of a measured result.
- **Verified** means the behavior has both a source/test contract and final gate
  evidence where applicable.
- **Intentional boundary** means the product explicitly delegates or declines an
  expansion and documents the resulting behavior.

Advisory matching is not exploitability. Raw Source display is valid when its
tree type is labeled and explained. Local display of ignored or rendered data is
not, by itself, remote exfiltration. Findings below focus on wrong-tree behavior,
silent failure, destructive scope, hangs, misleading state, and measured user
impact.

## Deduplicated findings

| ID | Severity | Finding | Disposition |
| --- | --- | --- | --- |
| F-001 | High | A configured chezmoi destDir could be overridden, causing reads or writes to use the wrong destination. | Verified through effective destination resolution, explicit override separation, and destination-boundary tests. |
| F-002 | High | Multi-target apply could bypass the reviewed plan, interleave with another mutation, or continue after partial failure. | One plan and confirmation feed one serialized fresh validation-plus-apply job. Stop-on-failure, mutation guards, virtual managed-directory warnings, and no-op Apply preflight behavior are covered by final tests. |
| F-003 | Medium | A .chezmoiroot source layout could make configured and effective source roots diverge. | Verified: source-path supplies the effective root for source view, ignore writes, previews, and source actions. |
| F-004 | Medium | Ignore evaluation could fail open or hide a later-rule error; an ignore-file append could split one rule across writes without cross-process coordination. | Verified: rendered-rule failures invalidate descendants and search trust. Escaping/root handling, nonblocking flock, one-payload append, a 4 MiB cap, and non-regular-file rejection are covered by tests. |
| F-005 | Medium | Prompting foreground commands could consume TUI input or receive no terminal. | Verified: interactive actions use terminal restore and foreground handoff; background commands use null stdin. |
| F-006 | Medium | FIFOs, devices, sockets, broken links, or large/binary files could block or flood preview. | Verified: metadata rejection, bounded reads, binary detection, and truncation reporting cover special-file paths. |
| F-007 | Medium | Rapid selection changes could display stale detail or discard a useful request. | Verified: debounce, request IDs, latest-slot coalescing, process cancellation, and stale-error suppression are covered. |
| F-008 | Medium | Filtering could lose marks or change the intended batch set. | Verified: marks survive filtering and hidden paths remain in batch targeting. |
| F-009 | Medium | Fixed-size modals could clip action, confirmation, or error content. | Verified across compact modal tests, action/preflight content, and responsive layout checks. |
| F-010 | Medium | Hidden or duplicated bindings made actions harder to discover. | Verified: BackTab, Home/End, Ctrl+P, focus gating, grouped footer hints, and quiet selection are tested and documented. |
| F-011 | Medium | Quit could abandon in-flight mutation or batch work without explanation. | Verified: quit refusal protects mutation/batch work while read-only loading remains interruptible. |
| F-012 | Medium | Timeout, truncation, invalid UTF-8, or partial machine output could be presented as success. | Verified: nonzero, timeout, truncation, and output-limit states fail closed. Non-UTF-8 paths are unsupported. Managed names containing LF/CR are rejected before status because the line-oriented format could alias an embedded line as another row. |
| F-013 | Medium | New actions could have inconsistent palette metadata, target rules, argv, output, or foreground behavior. | Verified through the 37-action typed policy, action-flow tests, mutation guards, foreground routing, result rendering, and public API tests. |
| F-014 | Low | Source view could be mistaken for decoded managed state. | Verified: Source is labeled as a filtered raw source tree, help explains that scope, and cat is the explicit rendered-target action. |
| F-015 | Low | Width, color, or focus choices could reduce accessibility. | Verified: Unicode cell width, monochrome markers, quiet selection, NO_COLOR, compact layouts, and hunk-derived line numbers are implemented and tested. |
| F-016 | Low | Redraw, formatting, subprocess, and filtering overhead could increase latency. | Final first-status median is about 82.17% lower. First-frame and separate filter/render costs are not claimed as improvements. |
| F-017 | Release blocker | A release cannot be certified while final integration and quality gates are incomplete. | Resolved for local closure: both test toolchains, all-target checks, cross-target Clippy, actionlint, real-chezmoi smoke, screenshots, dependencies, and performance passed. |
| F-018 | Informational | Five baseline crate versions matched RustSec informational unsoundness advisories. | Resolved for known advisories by compatible upgrades and a final 223-package OSV query with zero matches. This is not proof of all security. |
| F-019 | Medium | A diff already running from the prior tail can continue for roughly three seconds after selection moves away. | Resolved: actual process cancellation removes obsolete detail without a stale error and preserves unrelated mutation. In one artificial three-second PTY regression, render latency fell to 161.194 ms in debug and 140.916 ms in release. |

## Dependency audit

The baseline OSV querybatch checked 211 registry packages. It found five crate
versions with advisory matches; rand had two aliases for the same advisory.

| Crate | Baseline | Advisory | Patch floor | Final |
| --- | --- | --- | ---: | ---: |
| anyhow | 1.0.101 | [RUSTSEC-2026-0190](https://rustsec.org/advisories/RUSTSEC-2026-0190.html) | 1.0.103 | 1.0.104 |
| lru | 0.16.3 | [RUSTSEC-2026-0253](https://rustsec.org/advisories/RUSTSEC-2026-0253.html) | 0.18.2 | 0.18.5 |
| rand | 0.8.5 | [RUSTSEC-2026-0097](https://rustsec.org/advisories/RUSTSEC-2026-0097.html), [GHSA-cq8v-f236-94qc](https://github.com/github/advisory-database/blob/main/advisories/github-reviewed/2026/04/GHSA-cq8v-f236-94qc/GHSA-cq8v-f236-94qc.json) | 0.8.6 | 0.8.6 |
| rustix | 1.1.3 | [RUSTSEC-2026-0486](https://rustsec.org/advisories/RUSTSEC-2026-0486.html) | 1.1.6 | 1.1.6 |
| tokio | 1.49.0 | [RUSTSEC-2026-0386](https://rustsec.org/advisories/RUSTSEC-2026-0386.html) | 1.51.6 | 1.54.1 |

All five advisory records were RustSec informational unsoundness entries. The
match identified an affected version range; it did not prove that the unsafe
condition was reachable or exploitable in this application.

The final [OSV querybatch](https://api.osv.dev/v1/querybatch) checked 223
registry packages on 2026-11-12 and reported zero known matches. Current relevant
versions include anyhow 1.0.104, lru 0.18.5, rand 0.8.6, ratatui 0.30.2 with
ratatui-core 0.1.2, rustix 1.1.6, and tokio 1.54.1.

Zero known matches is an observed advisory result, not proof that the dependency
graph has no unknown or future vulnerability.

## Startup performance

The reproducible PTY harness ran seven samples per scenario for v0.2.2 and the
final v0.3.0 release binary. Both reported 14 of 14 successful cases. The
synthetic fixture used three source files, 5,000 fake Git objects in the large
scenario, and a 120x40 terminal.

| Scenario | v0.2.2 first status p50 | v0.3.0 first status p50 |
| --- | ---: | ---: |
| small | 204.485 ms | 36.468 ms |
| large-git | 204.482 ms | 36.469 ms |

The final release reduced median time to the selected status by about 82.17%
(about 5.61x) in both scenarios. They are effectively equal, so the result does
not demonstrate a large-git-specific pruning benefit. First-frame medians were
3.537 ms for small and 3.532 ms for large-git; no first-frame improvement is
claimed.

The method, final sanitized reports, host metadata, and limitations are in
[PERFORMANCE.md](PERFORMANCE.md).

## Complete upstream command disposition

The table describes product placement. Native means a TUI view or typed action;
foreground shell means terminal handoff for interactive, key, archive, container,
SSH, upgrade, or advanced security work. A shell disposition is supported
coverage and not a safety exclusion.

| Upstream page | Disposition | Behavior and validation |
| --- | --- | --- |
| [add](https://www.chezmoi.io/reference/commands/add/) (alias: [manage](https://www.chezmoi.io/reference/commands/manage/)) | Native guarded | File add and confirmed recursive directory add with dry-run preview; no exact or follow option; outside-destination mutation rejected. |
| [age-keygen](https://www.chezmoi.io/reference/commands/age-keygen/) | Foreground shell | Key generation retains explicit terminal I/O and key lifecycle. |
| [age](https://www.chezmoi.io/reference/commands/age/) | Foreground shell | Age encryption and decryption stay in chezmoi's interactive command. |
| [apply](https://www.chezmoi.io/reference/commands/apply/) | Native guarded | Reviewed plan, one confirmation, fresh validation and apply in one serialized job. |
| [archive](https://www.chezmoi.io/reference/commands/archive/) | Foreground shell | Archive and WebDAV output retain terminal and filesystem control. |
| [cat-config](https://www.chezmoi.io/reference/commands/cat-config/) | Foreground shell | Explicit config inspection through chezmoi; no persistent body capture. |
| [cat](https://www.chezmoi.io/reference/commands/cat/) | Native read-only | Explicit rendered-target inspection; no extra confirmation, automatic render, or persistence. |
| [cd](https://www.chezmoi.io/reference/commands/cd/) | Foreground shell | Uses the chezmoi-configured environment for an interactive shell. |
| [chattr](https://www.chezmoi.io/reference/commands/chattr/) | Native guarded | Typed attribute wizard for exact managed entries. |
| [completion](https://www.chezmoi.io/reference/commands/completion/) | Shell setup | Installation and shell setup outside the TUI state model. |
| [data](https://www.chezmoi.io/reference/commands/data/) | Native read-only | Explicit template data output; local display may be sensitive. |
| [decrypt](https://www.chezmoi.io/reference/commands/decrypt/) | Foreground shell | Secret and key workflow remains explicit and interactive. |
| [destroy](https://www.chezmoi.io/reference/commands/destroy/) | Native strict | Exact managed target plus typed DESTROY phrase. |
| [diff](https://www.chezmoi.io/reference/commands/diff/) | Native read-only | Per-target and full diff with recursive directory handling and built-in diff output. |
| [docker](https://www.chezmoi.io/reference/commands/docker/) (alias: [podman](https://www.chezmoi.io/reference/commands/podman/)) | Foreground shell | Container commands retain the user's interactive terminal. |
| [doctor](https://www.chezmoi.io/reference/commands/doctor/) | Native read-only | Diagnostics output is bounded and displayed in the detail pane. |
| [dump-config](https://www.chezmoi.io/reference/commands/dump-config/) | Native read-only | Explicit JSON config inspection; no extra confirmation or persistence. |
| [dump](https://www.chezmoi.io/reference/commands/dump/) | Foreground shell | Broad target-state export stays with chezmoi. |
| [edit-config-template](https://www.chezmoi.io/reference/commands/edit-config-template/) | Foreground | Foreground editor handoff with terminal restoration. |
| [edit-config](https://www.chezmoi.io/reference/commands/edit-config/) | Foreground | Foreground editor handoff for chezmoi configuration. |
| [edit-encrypted](https://www.chezmoi.io/reference/commands/edit-encrypted/) | Foreground shell | Plaintext temp-file and key lifecycle remains inside chezmoi. |
| [edit](https://www.chezmoi.io/reference/commands/edit/) | Foreground | Managed target maps to chezmoi edit and a foreground editor. |
| [encrypt](https://www.chezmoi.io/reference/commands/encrypt/) | Foreground shell | Encryption remains an explicit security command. |
| [execute-template](https://www.chezmoi.io/reference/commands/execute-template/) | Foreground shell | No hidden arbitrary template execution path. |
| [forget](https://www.chezmoi.io/reference/commands/forget/) (alias: [unmanage](https://www.chezmoi.io/reference/commands/unmanage/)) | Native guarded | Exact-managed removal with confirmation. |
| [generate](https://www.chezmoi.io/reference/commands/generate/) | Foreground shell | Niche script and commit-message generation remain explicit. |
| [git](https://www.chezmoi.io/reference/commands/git/) | Native fixed actions | Status, diff, log, stage, commit, push, and source update without apply; no arbitrary argv editor. |
| [help](https://www.chezmoi.io/reference/commands/help/) | Informational shell | General help remains a shell and documentation workflow. |
| [ignored](https://www.chezmoi.io/reference/commands/ignored/) | Native read-only | Explicit local list of ignored names. |
| [import](https://www.chezmoi.io/reference/commands/import/) | Foreground shell | Complex archive import retains interactive choices and source/destination changes. |
| [init](https://www.chezmoi.io/reference/commands/init/) | Native guarded | Explicit initialization with confirmation and foreground prompts; no automatic startup init. |
| [license](https://www.chezmoi.io/reference/commands/license/) | Informational shell | Static license information remains outside the TUI workflow. |
| [managed](https://www.chezmoi.io/reference/commands/managed/) (alias: [list](https://www.chezmoi.io/reference/commands/list/)) | Native read-only | Primary managed target tree with destination preview. |
| [merge-all](https://www.chezmoi.io/reference/commands/merge-all/) | Foreground | Confirmed interactive merge-all handoff. |
| [merge](https://www.chezmoi.io/reference/commands/merge/) | Foreground | Interactive three-way merge for the selected target. |
| [purge](https://www.chezmoi.io/reference/commands/purge/) | Native strict | Broad removal behind the typed PURGE ALL phrase. |
| [push](https://www.chezmoi.io/reference/commands/push/) | Foreground shell | Auto add, commit, and push remains explicit; distinct from pushing existing commits. |
| [re-add](https://www.chezmoi.io/reference/commands/re-add/) | Native guarded | Selected modified file and explicit re-add-all; directories and ineligible files are rejected. |
| [secret](https://www.chezmoi.io/reference/commands/secret/) | Foreground shell | Custom secret-manager operations retain their native interactive workflow. |
| [source-path](https://www.chezmoi.io/reference/commands/source-path/) | Internal resolver | Resolves the effective source root, including .chezmoiroot. |
| [ssh](https://www.chezmoi.io/reference/commands/ssh/) | Foreground shell | SSH requires inherited terminal input and output. |
| [state](https://www.chezmoi.io/reference/commands/state/) | Foreground shell | Sensitive state inspection and editing remain specialized interactive commands. |
| [status](https://www.chezmoi.io/reference/commands/status/) | Native read-only | Primary destination-difference view with selected-target diff. |
| [target-path](https://www.chezmoi.io/reference/commands/target-path/) | Internal resolver | Resolves exact target mapping for destination and ignore safety. |
| [unmanaged](https://www.chezmoi.io/reference/commands/unmanaged/) | Native read-only | Bounded current-directory tree with ignore-aware descendants. Outside-destination Add and AddDirectory are disabled; Ignore permits only explicit Global by name. |
| [update](https://www.chezmoi.io/reference/commands/update/) | Foreground | Confirmed source update plus apply handoff. |
| [upgrade](https://www.chezmoi.io/reference/commands/upgrade/) | Foreground shell | Binary replacement remains outside the TUI state model. |
| [verify](https://www.chezmoi.io/reference/commands/verify/) | Native read-only | Target or all-entry verification; exit 0 is match, exit 1 is drift, other results are failures. |

## Security and interaction boundaries

Raw Source browsing is supported and clearly labeled. It shows filtered source
files under the effective root, not decoded destination state. The cwd-based
Unmanaged view supports browsing beyond the current destination, with mutation
scope made explicit.

Cat and dump-config require only an explicit palette selection. They do not open
an extra confirmation modal, run automatically, add decryption behavior, or
persist output. Their local result may contain sensitive values. Secret-bearing
and advanced security commands remain available through foreground chezmoi shell
handoff, so the TUI does not remove native functionality.

Machine-readable path output is parsed strictly. Invalid UTF-8, partial output,
timeouts, and limits fail closed. Managed names containing LF/CR are rejected
before status parsing because chezmoi status is line-oriented and an embedded
line could alias another row. These paths are unsupported rather than silently
replaced.

## Considered and rejected expansion

- Reimplementing chezmoi source state, templating, encryption, or apply.
- A custom secret-manager UI or automatic decrypted-content display.
- Background polling of remote Git state.
- An arbitrary shell or argv command executor.
- Embedded editors, merge tools, SSH clients, container managers, or upgraders.
- Separate bespoke forms for every upstream command.
- A broad cache without a settled freshness and privacy contract.
- Replacing the ignore engine, adding a web frontend, or a large framework migration.

Fixed native actions plus explicit foreground handoff cover practical tasks
without creating a second source of truth or duplicating mature interactive
tools.

## Final verification evidence

| Check | Result |
| --- | --- |
| Rust 1.99.1 tests | 312 library + 1 main + 9 public flows + 39 fake chezmoi = 361 passed, 0 failed |
| Rust 1.88.0 tests | 312 library + 1 main + 9 public flows + 39 fake chezmoi = 361 passed, 0 failed |
| Formatting, all-target check, all-target Clippy | Passed with warnings denied |
| macOS aarch64 all-target cross-Clippy | Passed; not a local macOS runtime claim |
| Workflow syntax | actionlint passed for both workflows |
| Real chezmoi | Final release binary passed actual PTY smoke with v2.74.1 |
| Detail cancellation | Actual process regression passed without stale error or mutation cancellation |
| Ignore safety | 4 MiB, FIFO, lock-contention, symlink, unchanged-file, escaping, and root tests passed |
| Dependencies | OSV querybatch: 223 registry packages, 0 known matches |
| Screenshots | Four final synthetic SVGs regenerated and privacy-reviewed |
| Startup benchmark | Both datasets 14/14; first-status p50 about 82.17% lower |

The repository release workflow validates versions, builds four archives,
performs the locked package dry-run as part of publication, publishes to
crates.io, and creates a draft GitHub release. A maintainer verifies the archives
before publishing that release. This audit does not infer service availability
beyond the package and release destinations linked from the README.
