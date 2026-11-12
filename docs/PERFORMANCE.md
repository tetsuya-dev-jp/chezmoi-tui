# Startup performance measurement

This note records the final shared synthetic PTY comparison for v0.2.2 and
v0.3.0 and the limits of its interpretation.

## Method

[`scripts/benchmark.py`](../scripts/benchmark.py) creates a temporary fixture,
launches the selected TUI binary in a 120x40 PTY, and records time from launch to
two observable events:

- **First frame:** the terminal enters the alternate screen and positions the
  cursor at the first cell.
- **First status:** the selected synthetic status row `M .zshrc` becomes visible.

Each scenario uses seven warm runs with a one-second idle period before sending
`q`. The timing origin is immediately before `fork`, so process launch and exec
time are included. The PTY is polled every 5 ms. The report contains per-scenario
minimum, median, and maximum values and records any run failure.

The fixture has three source files, three destination files, and a synthetic
status change. The `large-git` scenario adds 5,000 fake objects below
`.git/objects`. It does not exercise remote Git access, encryption, templates,
large managed trees, or slow `chezmoi` commands.

## Host metadata

Both reported datasets are local warm synthetic runs on the same machine:

- Linux 7.0.0-38-generic, x86_64 GNU/Linux.
- 13th Gen Intel Core i5-13500 with 20 logical CPUs.
- Rust 1.99.1 release-profile binary.

No hostname, username, or user filesystem path is part of the retained reports.

## Recorded v0.2.2 baseline

The shared paired baseline used `chezmoi-tui-v0.2.2` with 14 of 14 runs complete
and no recorded failures. The sanitized report is
[baseline.json](performance/baseline.json).

| Scenario | First frame p50 | First status p50 |
| --- | ---: | ---: |
| small | 3.438 ms | 204.485 ms |
| large-git, 5,000 fake objects | 3.557 ms | 204.482 ms |

The two scenarios produced nearly identical medians. This measurement **does
not demonstrate** that pruning `.git` improves startup latency. It establishes a
repeatable baseline under the conditions above.

## Final comparison

The final v0.3.0 dataset also completed 14 of 14 cases with no recorded failures.
Its sanitized report is [current.json](performance/current.json).

| Scenario | v0.2.2 first frame | v0.3.0 first frame | v0.2.2 first status | v0.3.0 first status |
| --- | ---: | ---: | ---: | ---: |
| small | 3.438 ms | 3.537 ms | 204.485 ms | 36.468 ms |
| large-git | 3.557 ms | 3.532 ms | 204.482 ms | 36.469 ms |

The selected-status medians represent an approximately 82.17% reduction for
both scenarios (about 5.61x). The result does not show a large-git-specific
advantage because the two v0.3.0 scenarios are effectively equal.

First-frame medians remain near 3.54 ms, close to v0.2.2. The small change does
not establish a first-frame improvement or regression, and no first-frame speed
claim is made.

The improvement applies to time until the synthetic selected status becomes
visible. Do not generalize it to other terminals, filesystems, operating systems,
network-backed commands, or real dotfile trees.

## Detail cancellation

A separate artificial PTY regression delayed payload A for three seconds,
navigated to payload B, and measured time to render B. The former behavior waited
about 3,045.694 ms. The corrected process-tree cancellation produced:

| Build | B rendered after navigation | Outcome |
| --- | ---: | --- |
| debug | 161.194 ms | A absent, B visible, no stale error, exit 0 |
| release | 140.916 ms | A absent, B visible, no stale error, exit 0 |

This is one targeted regression case, not a general command-latency benchmark.
The actual-process test also verifies that detail cancellation does not cancel an
unrelated mutation.

## Reproduce

Build the binary under test, then run:

```bash
python3 scripts/benchmark.py \
  --binary target/release/chezmoi-tui \
  --chezmoi /path/to/chezmoi \
  --runs 7 \
  --output paired-final.json
```

Both reports have zero failures and use the same fixture and timing settings.
Only sanitized reports are retained in the repository.
