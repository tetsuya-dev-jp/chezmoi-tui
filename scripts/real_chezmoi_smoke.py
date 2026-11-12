#!/usr/bin/env python3
"""Exercise chezmoi-tui against an isolated real chezmoi."""

from __future__ import annotations

import argparse
import fcntl
import os
import pty
import re
import select
import shutil
import signal
import struct
import subprocess
import sys
import tempfile
import termios
import time
from pathlib import Path

COMMAND_TIMEOUT = 30
PTY_TIMEOUT = 20
ANSI_CSI = re.compile(rb"\x1b\[[0-?]*[ -/]*[@-~]")
ANSI_OSC = re.compile(rb"\x1b\][^\x07]*(?:\x07|\x1b\\)")


class SmokeError(RuntimeError):
    pass


def display(value: str | Path) -> str:
    return os.fspath(value)


def run(
    args: list[str | Path],
    *,
    env: dict[str, str],
    cwd: Path,
    expected: tuple[int, ...] = (0,),
    input_bytes: bytes | None = None,
) -> subprocess.CompletedProcess[bytes]:
    command = [display(arg) for arg in args]
    try:
        result = subprocess.run(
            command,
            cwd=cwd,
            env=env,
            input=input_bytes,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            timeout=COMMAND_TIMEOUT,
            check=False,
        )
    except (OSError, subprocess.TimeoutExpired) as error:
        raise SmokeError(f"command failed to run: {command!r}: {error}") from error
    if result.returncode not in expected:
        stdout = result.stdout.decode("utf-8", "replace")[-2000:]
        stderr = result.stderr.decode("utf-8", "replace")[-2000:]
        raise SmokeError(
            f"command returned {result.returncode}, expected {expected}: {command!r}\n"
            f"stdout:\n{stdout}\nstderr:\n{stderr}"
        )
    return result


def visible(raw: bytes) -> str:
    raw = ANSI_OSC.sub(b"", raw)
    raw = ANSI_CSI.sub(b"", raw)
    return raw.decode("utf-8", "replace").replace("\r", "")


def write(path: Path, content: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(content, encoding="utf-8")


def source_snapshot(root: Path) -> list[tuple[str, str, bytes]]:
    snapshot = []
    for path in sorted(root.rglob("*")):
        relative = path.relative_to(root).as_posix()
        if path.is_symlink():
            snapshot.append((relative, "symlink", os.fsencode(os.readlink(path))))
        elif path.is_dir():
            snapshot.append((relative, "directory", b""))
        else:
            snapshot.append((relative, "file", path.read_bytes()))
    return snapshot


def assert_contains(value: str, expected: str, context: str) -> None:
    if expected not in value:
        raise SmokeError(f"{context}: expected {expected!r}, got:\n{value[-4000:]}")


def chezmoi(
    binary: Path,
    config: Path,
    cache: Path,
    state: Path,
    env: dict[str, str],
    root: Path,
    *args: str | Path,
    source: Path | None = None,
    destination: Path | None = None,
    expected: tuple[int, ...] = (0,),
    input_bytes: bytes | None = None,
) -> subprocess.CompletedProcess[bytes]:
    command: list[str | Path] = [
        binary,
        "--config",
        config,
        "--cache",
        cache,
        "--persistent-state",
        state,
        "--no-tty",
        "--no-pager",
        "--color=false",
        "--progress=false",
    ]
    if source is not None:
        command.extend(["--source", source])
    if destination is not None:
        command.extend(["--destination", destination])
    command.extend(args)
    return run(
        command,
        env=env,
        cwd=root,
        expected=expected,
        input_bytes=input_bytes,
    )


def pty_smoke(
    *,
    tui: Path,
    args: list[str],
    env: dict[str, str],
    cwd: Path,
    exercise_views: bool,
) -> None:
    command = [display(tui), *args]
    pid, master = pty.fork()
    if pid == 0:
        try:
            os.chdir(cwd)
            os.execvpe(display(tui), command, env)
        except BaseException as error:
            os.write(2, f"failed to exec TUI: {error}\n".encode())
            os._exit(127)

    fcntl.ioctl(master, termios.TIOCSWINSZ, struct.pack("HHHH", 40, 180, 0, 0))

    def wait_for(token: str, raw: bytearray) -> None:
        deadline = time.monotonic() + PTY_TIMEOUT
        while token not in visible(bytes(raw)):
            wait = deadline - time.monotonic()
            if wait <= 0:
                raise SmokeError(
                    f"TUI did not render {token!r} within {PTY_TIMEOUT}s; screen:\n"
                    f"{visible(bytes(raw))[-4000:]}"
                )
            ready, _, _ = select.select([master], [], [], min(wait, 0.25))
            if ready:
                try:
                    raw.extend(os.read(master, 65536))
                except OSError:
                    break
            done, status = os.waitpid(pid, os.WNOHANG)
            if done and not ready:
                code = os.waitstatus_to_exitcode(status)
                raise SmokeError(
                    f"TUI exited {code} before rendering {token!r}; screen:\n"
                    f"{visible(bytes(raw))[-4000:]}"
                )

    def send(keys: bytes, token: str, raw: bytearray) -> None:
        raw.clear()
        os.write(master, keys)
        wait_for(token, raw)

    def send_keys(keys: bytes) -> None:
        os.write(master, keys)
        deadline = time.monotonic() + 0.25
        while time.monotonic() < deadline:
            ready, _, _ = select.select([master], [], [], 0.05)
            if ready:
                try:
                    os.read(master, 65536)
                except OSError:
                    return

    try:
        startup = bytearray()
        wait_for(".profile", startup)
        startup_screen = visible(bytes(startup))
        assert_contains(startup_screen, "Status", "initial status header")
        assert_contains(startup_screen, ".profile", "initial status view")
        if ".dest-probe" in startup_screen:
            raise SmokeError(
                "configured destination was not used; matching target appeared pending"
            )

        if exercise_views:
            tabbed_header = "\u25b6 1 Status" in startup_screen
            send(b"2", "\u25b6 2 Managed" if tabbed_header else "Managed", bytearray())
            send(b"3", "\u25b6 3 Unmanaged" if tabbed_header else "Unmanaged", bytearray())
            send(b"4", "\u25b6 4 Source" if tabbed_header else "Source", bytearray())
            send(b"1", "\u25b6 1 Status" if tabbed_header else "Status", bytearray())
            send(b"a", "Action Menu", bytearray())
            send_keys(b"\x1b")
            send(b"?", "Help / Legends", bytearray())
            send_keys(b"\x1b")
        send(b"q", "", bytearray())

        deadline = time.monotonic() + PTY_TIMEOUT
        while True:
            done, status = os.waitpid(pid, os.WNOHANG)
            if done:
                code = os.waitstatus_to_exitcode(status)
                if code != 0:
                    raise SmokeError(f"TUI exited with status {code}")
                return
            if time.monotonic() >= deadline:
                raise SmokeError("TUI did not quit within the bounded timeout")
            select.select([master], [], [], 0.1)
            try:
                os.read(master, 65536)
            except OSError:
                pass
    finally:
        try:
            os.waitpid(pid, os.WNOHANG)
            os.kill(pid, signal.SIGKILL)
        except (ChildProcessError, ProcessLookupError):
            pass
        try:
            os.waitpid(pid, 0)
        except ChildProcessError:
            pass
        os.close(master)


def smoke(chezmoi_binary: Path, tui: Path) -> None:
    with tempfile.TemporaryDirectory(prefix="chezmoi-tui-smoke-") as temporary:
        root = Path(temporary)
        config_root = root / "config"
        home = root / "home"
        data_root = root / "data"
        cache_root = root / "cache"
        state_root = root / "state"
        runtime = root / "runtime"
        source_root = runtime / "source"
        source = source_root / "home"
        destination = runtime / "destination"
        configured_destination = runtime / "configured-destination"
        work = destination / "workspace with spaces"
        configured_work = configured_destination / "workspace with spaces"
        state = state_root / "chezmoi.boltdb"
        cache = cache_root / "chezmoi"
        empty_config = config_root / "empty.toml"
        configured_config = config_root / "chezmoi" / "chezmoi.toml"

        for path in (
            config_root,
            home,
            data_root,
            cache_root,
            state_root,
            source_root,
            source,
            destination,
            configured_destination,
        ):
            path.mkdir(parents=True, exist_ok=True)

        write(empty_config, "")
        write(
            configured_config,
            f"""sourceDir = "{source_root}"
destDir = "{configured_destination}"

[data]
fixture = "configured"

[update]
check = false
""",
        )
        write(source_root / ".chezmoiroot", "home\n")
        write(source / ".chezmoiignore", ".ignored\n")
        write(source / "dot_profile.tmpl", 'profile={{"rendered"}}\n')
        write(source / "dot_dest_probe", "destination probe\n")
        write(source / "dot_ignored", "ignored source\n")
        write(work / "file name.txt", "unmanaged payload\n")
        write(configured_work / "file name.txt", "configured unmanaged payload\n")
        write(destination / ".dest-probe", "destination probe\n")
        write(configured_destination / ".dest-probe", "destination probe\n")

        env = {
            "PATH": os.pathsep.join(
                [display(chezmoi_binary.parent), os.environ.get("PATH", "/usr/bin:/bin")]
            ),
            "HOME": display(home),
            "XDG_CONFIG_HOME": display(config_root),
            "XDG_DATA_HOME": display(data_root),
            "XDG_CACHE_HOME": display(cache_root),
            "XDG_STATE_HOME": display(state_root),
            "USER": "chezmoi-tui-smoke",
            "LOGNAME": "chezmoi-tui-smoke",
            "LANG": "C.UTF-8",
            "LC_ALL": "C.UTF-8",
            "TERM": "xterm-256color",
            "GIT_CONFIG_NOSYSTEM": "1",
            "GIT_CONFIG_GLOBAL": os.devnull,
            "GIT_TERMINAL_PROMPT": "0",
        }

        def empty(*args: str | Path) -> subprocess.CompletedProcess[bytes]:
            return chezmoi(
                chezmoi_binary,
                empty_config,
                cache,
                state,
                env,
                root,
                *args,
                source=source_root,
                destination=destination,
            )

        version = run([chezmoi_binary, "--version"], env=env, cwd=root)
        assert_contains(
            version.stdout.decode("utf-8", "replace"),
            "chezmoi version v2.74.1",
            "chezmoi version",
        )
        tui_version = run([tui, "--version"], env=env, cwd=root)
        assert_contains(
            tui_version.stdout.decode("utf-8", "replace"),
            "chezmoi-tui ",
            "TUI version",
        )

        status = empty("status").stdout.decode("utf-8", "replace")
        assert_contains(status, ".profile", "status")
        if ".ignored" in status:
            raise SmokeError(f"ignored source entry appeared in status:\n{status}")

        managed = empty("managed", "--format", "json").stdout.decode("utf-8", "replace")
        assert_contains(managed, ".profile", "managed")
        if ".ignored" in managed:
            raise SmokeError(f"ignored source entry appeared in managed:\n{managed}")

        ignored = empty("ignored", "--nul-path-separator").stdout
        if b".ignored\0" not in ignored:
            raise SmokeError(f"ignored entry missing from ignored output: {ignored!r}")

        scoped = empty(
            "unmanaged",
            "--path-style=absolute",
            "--nul-path-separator",
            "--",
            work,
        ).stdout
        scoped_paths = [os.fsdecode(item) for item in scoped.split(b"\0") if item]
        if scoped_paths != [display(work)]:
            raise SmokeError(f"unexpected scoped unmanaged output: {scoped_paths!r}")

        rendered = empty("cat", destination / ".profile").stdout.decode()
        if rendered != "profile=rendered\n":
            raise SmokeError(f"unexpected rendered cat output: {rendered!r}")

        source_path = Path(empty("source-path", destination / ".profile").stdout.decode().strip())
        if source_path != source / "dot_profile.tmpl":
            raise SmokeError(f"unexpected source path: {source_path}")
        target_path = Path(
            empty("target-path", source / "dot_profile.tmpl").stdout.decode().strip()
        )
        if target_path != destination / ".profile":
            raise SmokeError(f"unexpected target path: {target_path}")

        configured_target = chezmoi(
            chezmoi_binary,
            configured_config,
            cache,
            state,
            env,
            root,
            "target-path",
            source / "dot_profile.tmpl",
        ).stdout.decode().strip()
        if Path(configured_target) != configured_destination / ".profile":
            raise SmokeError(f"configured destination was not resolved: {configured_target}")
        configured_source = chezmoi(
            chezmoi_binary,
            configured_config,
            cache,
            state,
            env,
            root,
            "source-path",
            configured_destination / ".profile",
        ).stdout.decode().strip()
        if Path(configured_source) != source / "dot_profile.tmpl":
            raise SmokeError(f"configured source was not resolved: {configured_source}")
        configured_data = chezmoi(
            chezmoi_binary,
            configured_config,
            cache,
            state,
            env,
            root,
            "execute-template",
            input_bytes=b"{{ .fixture }}",
        ).stdout.decode()
        if configured_data != "configured":
            raise SmokeError(f"configured data was not rendered: {configured_data!r}")

        profile = destination / ".profile"
        empty(
            "apply",
            "--error-on-conflict=true",
            "--no-tty",
            "--",
            profile,
        )
        if profile.read_text(encoding="utf-8") != "profile=rendered\n":
            raise SmokeError("safe scoped apply did not write the expected target")
        empty("verify", "--", profile)
        write(profile, "profile=locally-modified\n")
        chezmoi(
            chezmoi_binary,
            empty_config,
            cache,
            state,
            env,
            root,
            "verify",
            "--",
            profile,
            source=source_root,
            destination=destination,
            expected=(1,),
        )
        chezmoi(
            chezmoi_binary,
            empty_config,
            cache,
            state,
            env,
            root,
            "apply",
            "--error-on-conflict=true",
            "--no-tty",
            "--",
            profile,
            source=source_root,
            destination=destination,
            expected=(1,),
        )
        if profile.read_text(encoding="utf-8") != "profile=locally-modified\n":
            raise SmokeError("conflicting apply modified the destination")

        preview_directory = destination / "preview directory"
        write(preview_directory / "nested/one.txt", "preview one\n")
        write(preview_directory / "two.txt", "preview two\n")
        before_preview = source_snapshot(source_root)
        preview = empty(
            "add",
            "--dry-run",
            "--verbose",
            "--recursive",
            "--",
            preview_directory,
        )
        after_preview = source_snapshot(source_root)
        if after_preview != before_preview:
            raise SmokeError("add directory dry-run changed the source tree")
        preview_output = (preview.stdout + preview.stderr).decode("utf-8", "replace")
        assert_contains(preview_output, "preview directory", "add directory preview")

        added = destination / "added-script.txt"
        write(added, 'added={{"template"}}\n')
        empty("add", "--template", "--", added)
        empty("chattr", "--", "private,executable", added)
        added_source = Path(empty("source-path", added).stdout.decode().strip())
        if added_source.name != "private_executable_added-script.txt.tmpl":
            raise SmokeError(f"unexpected add/chattr source name: {added_source.name}")
        if added_source.read_text(encoding="utf-8") != 'added={{"template"}}\n':
            raise SmokeError("add --template did not preserve template source")
        empty("forget", "--force", "--no-tty", "--", added)
        if added_source.exists():
            raise SmokeError("forget did not remove the source entry")

        pty_smoke(
            tui=tui,
            args=[
                "--source",
                display(source_root),
                "--destination",
                display(destination),
                "--no-config",
                "--no-auto-preview",
            ],
            env=env,
            cwd=work,
            exercise_views=True,
        )
        pty_smoke(
            tui=tui,
            args=["--no-config", "--no-auto-preview"],
            env=env,
            cwd=configured_work,
            exercise_views=False,
        )
        print("real chezmoi smoke passed")


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--chezmoi", type=Path)
    parser.add_argument("--tui", type=Path)
    args = parser.parse_args()

    chezmoi_value = args.chezmoi or shutil.which("chezmoi")
    tui_value = args.tui or shutil.which("chezmoi-tui")
    if not chezmoi_value:
        parser.error("--chezmoi is required when chezmoi is not on PATH")
    if not tui_value:
        parser.error("--tui is required when chezmoi-tui is not on PATH")

    chezmoi_binary = Path(chezmoi_value).resolve()
    tui = Path(tui_value).resolve()
    for name, binary in (("chezmoi", chezmoi_binary), ("TUI", tui)):
        if not binary.is_file() or not os.access(binary, os.X_OK):
            parser.error(f"{name} executable not found: {binary}")

    try:
        smoke(chezmoi_binary, tui)
    except SmokeError as error:
        print(f"real chezmoi smoke failed: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
