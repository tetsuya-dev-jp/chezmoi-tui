#!/usr/bin/env python3
"""Minimal reproducible PTY startup benchmark for chezmoi-tui."""
from __future__ import annotations

import argparse
import errno
import fcntl
import json
import os
import pty
import select
import shutil
import signal
import statistics
import struct
import subprocess
import sys
import tempfile
import termios
import time
from pathlib import Path

REPO = Path(__file__).resolve().parents[1]
COLUMNS, ROWS = 120, 40
FRAME_MARKER = b"\x1b[?1049h\x1b[1;1H"
STATUS_MARKER = b"M .zshrc"
MAX_CAPTURE = 16 * 1024 * 1024
SCENARIOS = ("small", "large-git")


def write(path: Path, content: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(content, encoding="utf-8")


def build_fixture(root: Path) -> dict[str, Path]:
    paths = {
        "config": root / "xdg/config/chezmoi/chezmoi.toml",
        "home": root / "home",
        "xdg": root / "xdg",
        "destination": root / "destination",
        "source_small": root / "sources/small",
        "source_large": root / "sources/large-git",
    }
    write(paths["config"],
          "# Intentionally empty: the benchmark supplies source and destination explicitly.\n")
    source_files = {
        "dot_zshrc": "export BENCH_SOURCE=1\n",
        "dot_gitconfig": "[user]\n\tname = Benchmark\n",
        "dot_config/nvim/init.lua": "vim.opt.number = true\n",
    }
    destination_files = {
        ".zshrc": "export BENCH_DEST=1\n",
        ".gitconfig": source_files["dot_gitconfig"],
        ".config/nvim/init.lua": source_files["dot_config/nvim/init.lua"],
    }
    for name, content in source_files.items():
        write(paths["source_small"] / name, content)
    for name, content in destination_files.items():
        write(paths["destination"] / name, content)
    shutil.copytree(paths["source_small"], paths["source_large"])
    write(paths["source_large"] / ".git/HEAD", "ref: refs/heads/main\n")
    objects = paths["source_large"] / ".git/objects"
    for index in range(5000):
        write(objects / f"{index % 63:02x}" / f"{index:038x}", "fake object\n")
    return paths


def fixture_env(paths: dict[str, Path], scenario: str, chezmoi: Path) -> dict[str, str]:
    paths["home"].mkdir(parents=True, exist_ok=True)
    for name in ("config", "cache", "data", "state", "tmp"):
        (paths["xdg"] / name).mkdir(parents=True, exist_ok=True)
    source = paths["source_small"] if scenario == "small" else paths["source_large"]
    return {
        "PATH": f"{chezmoi.parent}:/usr/local/bin:/usr/bin:/bin", "HOME": str(paths["home"]),
        "XDG_CONFIG_HOME": str(paths["xdg"] / "config"),
        "XDG_CACHE_HOME": str(paths["xdg"] / "cache"),
        "XDG_DATA_HOME": str(paths["xdg"] / "data"), "XDG_STATE_HOME": str(paths["xdg"] / "state"),
        "CHEZMOI_CONFIG": str(paths["config"]), "CHEZMOI_SOURCE_DIR": str(source),
        "CHEZMOI_DEST_DIR": str(paths["destination"]), "TMPDIR": str(paths["xdg"] / "tmp"),
        "USER": "benchmark", "LOGNAME": "benchmark",
        "SHELL": "/bin/sh", "TERM": "xterm-256color",
        "LANG": "C.UTF-8", "LC_ALL": "C.UTF-8",
        "TZ": "Asia/Tokyo",
    }


def terminate_group(pid: int) -> None:
    for sig in (signal.SIGHUP, signal.SIGTERM, signal.SIGKILL):
        try:
            os.killpg(pid, sig)
        except ProcessLookupError:
            return
        time.sleep(0.05)


def run_tui(binary: Path, chezmoi: Path, paths: dict[str, Path], scenario: str,
            timeout: float, idle_seconds: float) -> dict[str, object]:
    source = paths["source_small"] if scenario == "small" else paths["source_large"]
    env = fixture_env(paths, scenario, chezmoi)
    argv = [str(binary), "--no-config", "--view", "status",
            "--source", str(source), "--destination", str(paths["destination"])]
    master, slave = pty.openpty()
    fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", ROWS, COLUMNS, 0, 0))
    started = time.perf_counter()
    pid = os.fork()
    if pid == 0:
        try:
            os.close(master)
            os.setsid()
            fcntl.ioctl(slave, termios.TIOCSCTTY, 0)
            for descriptor in (0, 1, 2):
                os.dup2(slave, descriptor)
            if slave > 2:
                os.close(slave)
            os.chdir(paths["destination"])
            os.execve(argv[0], argv, env)
        except BaseException as error:
            os.write(2, f"benchmark child failed: {error}\n".encode())
            os._exit(127)
    os.close(slave)
    capture = bytearray()
    frame = status = wait_status = idle_deadline = failure = None
    quit_sent = False
    try:
        while time.perf_counter() - started < timeout:
            if len(capture) > MAX_CAPTURE:
                failure = "terminal capture limit exceeded"
                break
            if select.select([master], [], [], 0.005)[0]:
                try:
                    chunk = os.read(master, 65536)
                except OSError as error:
                    if error.errno != errno.EIO:
                        raise
                    chunk = b""
                capture.extend(chunk)
                observed = time.perf_counter() - started
                if frame is None and FRAME_MARKER in capture:
                    frame = observed
                if status is None and STATUS_MARKER in capture:
                    status = observed
            waited, child_status = os.waitpid(pid, os.WNOHANG)
            if waited == pid:
                wait_status = child_status
                break
            if frame is not None and status is not None:
                if idle_deadline is None:
                    idle_deadline = time.perf_counter() + idle_seconds
                elif not quit_sent and time.perf_counter() >= idle_deadline:
                    try:
                        os.write(master, b"q")
                        quit_sent = True
                    except OSError as error:
                        failure = f"failed to send q: {error}"
                        break
        if wait_status is None and failure is None:
            failure = "TUI exceeded benchmark timeout"
    finally:
        if wait_status is None:
            terminate_group(pid)
            try:
                _, wait_status = os.waitpid(pid, 0)
            except ChildProcessError:
                pass
        os.close(master)
    exit_code = os.waitstatus_to_exitcode(wait_status) if wait_status is not None else -1
    if exit_code != 0 and failure is None:
        failure = f"TUI exited with code {exit_code}"
    if frame is None and failure is None:
        failure = "first frame was not detected"
    if status is None and failure is None:
        failure = "first status item was not detected"
    return {"first_frame_seconds": frame, "first_status_seconds": status, "exit_code": exit_code,
            "failure": failure}


def summary(values: list[float | None]) -> dict[str, float | None]:
    present = [value for value in values if value is not None]
    return {"p50_seconds": statistics.median(present) if present else None,
            "min_seconds": min(present) if present else None,
            "max_seconds": max(present) if present else None}


def parse_args(argv: list[str] | None = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=REPO / "target/release/chezmoi-tui")
    parser.add_argument("--chezmoi", type=Path,
                        default=Path(shutil.which("chezmoi") or "chezmoi"))
    parser.add_argument("--output", type=Path)
    parser.add_argument("--runs", type=int, default=7)
    parser.add_argument("--timeout", type=float, default=30.0)
    parser.add_argument("--idle-seconds", type=float, default=1.0)
    parser.add_argument("--self-test", action="store_true")
    args = parser.parse_args(argv)
    if args.runs < 5:
        parser.error("--runs must be at least 5")
    if args.timeout <= 0 or args.idle_seconds < 0:
        parser.error("--timeout must be positive and --idle-seconds must be non-negative")
    return args


def executable(path: Path) -> Path:
    located = shutil.which(str(path))
    candidate = Path(located) if located else path
    if not candidate.is_file():
        raise SystemExit(f"executable is not a file: {path}")
    return candidate.resolve()


def self_test() -> None:
    assert summary([3.0, 1.0, 2.0]) == {
        "p50_seconds": 2.0, "min_seconds": 1.0, "max_seconds": 3.0}
    for bad in (["--runs", "4"], ["--timeout", "0"], ["--idle-seconds", "-1"]):
        try:
            parse_args(bad)
        except SystemExit as error:
            assert error.code == 2
        else:
            raise AssertionError(f"invalid arguments accepted: {bad}")
    child = subprocess.Popen(["sleep", "60"], start_new_session=True)
    terminate_group(child.pid)
    assert child.wait(timeout=5) < 0, "cleanup process group was not terminated"
    with tempfile.TemporaryDirectory(prefix="chezmoi-tui-benchmark-self-test-") as temporary:
        root = Path(temporary) / "fixture"
        paths = build_fixture(root)
        assert (paths["source_small"] / "dot_zshrc").read_text(encoding="utf-8") == "export BENCH_SOURCE=1\n"
        assert (paths["destination"] / ".zshrc").read_text(encoding="utf-8") == "export BENCH_DEST=1\n"
        assert sum(1 for _ in (paths["source_large"] / ".git/objects").rglob("*")) == 5063
    assert not root.exists(), "temporary fixture was not cleaned up"


def main(argv: list[str] | None = None) -> int:
    args = parse_args(argv)
    if args.self_test:
        self_test()
        print("self-test: ok")
        return 0
    binary, chezmoi = executable(args.binary), executable(args.chezmoi)
    with tempfile.TemporaryDirectory(prefix="chezmoi-tui-benchmark-") as temporary:
        paths = build_fixture(Path(temporary) / "fixture")
        results: dict[str, object] = {}
        failures: list[dict[str, object]] = []
        for scenario in SCENARIOS:
            runs = [run_tui(binary, chezmoi, paths, scenario, args.timeout,
                            args.idle_seconds) for _ in range(args.runs)]
            for index, run in enumerate(runs, 1):
                if run["failure"] is not None:
                    failures.append({"scenario": scenario, "run": index, "failure": run["failure"]})
            results[scenario] = {
                "first_frame": summary([run["first_frame_seconds"] for run in runs]),
                "first_status": summary([run["first_status_seconds"] for run in runs]),
            }
    report = {
        "benchmark": "chezmoi-tui-pty-startup", "binary": binary.name, "chezmoi": chezmoi.name,
        "fixture": {"source_files": 3, "git_objects": 5000},
        "runs_per_scenario": args.runs, "pty": {"columns": COLUMNS, "rows": ROWS},
        "timing": {
            "origin": "immediately before fork; includes process launch/exec overhead",
            "first_frame": "reachability of ESC[?1049h followed by ESC[1;1H in PTY bytes",
            "first_status": "reachability of selected row bytes 'M .zshrc'",
            "poll_interval_seconds": 0.005, "idle_seconds": args.idle_seconds, "quit_key": "q",
        },
        "results": results, "failures": failures,
    }
    rendered = json.dumps(report, indent=2, sort_keys=True, ensure_ascii=False) + "\n"
    if args.output:
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(rendered, encoding="utf-8")
    else:
        sys.stdout.write(rendered)
    return 1 if failures else 0


if __name__ == "__main__":
    raise SystemExit(main())
