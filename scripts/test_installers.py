#!/usr/bin/env python3
"""Hermetic behavior checks for the asdf plugin scripts."""

from __future__ import annotations

import io
import os
import shutil
import subprocess
import tarfile
import tempfile
import unittest
from pathlib import Path

REPO = Path(__file__).resolve().parents[1]
BIN = REPO / "bin"
VERSION = "0.3.1"


class InstallerTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory(prefix="chezmoi-tui-installers-")
        self.root = Path(self.temporary.name)
        self.fake_bin = self.root / "fake-bin"
        self.fake_bin.mkdir()
        self.curl_log = self.root / "curl.log"
        self.fixture_archive = self.root / "fixture.tar.gz"
        self.write_archive(self.fixture_archive, executable=True)
        self.write_tools("Linux", "x86_64")
        self.write_ldd("ldd (Ubuntu GLIBC 2.43-2ubuntu2.4) 2.43")

    def tearDown(self) -> None:
        self.temporary.cleanup()

    def write_tools(self, system: str, machine: str) -> None:
        (self.fake_bin / "uname").write_text(
            "#!/usr/bin/env bash\n"
            "case \"${1:-}\" in\n"
            f"  -s) printf '%s\\n' '{system}' ;;\n"
            f"  -m) printf '%s\\n' '{machine}' ;;\n"
            "  *) exit 1 ;;\n"
            "esac\n",
            encoding="utf-8",
        )
        (self.fake_bin / "curl").write_text(
            "#!/usr/bin/env bash\n"
            "set -euo pipefail\n"
            "output=\n"
            "url=$*\n"
            "while [[ $# -gt 0 ]]; do\n"
            "  case \"$1\" in\n"
            "    --output) output=$2; shift 2 ;;\n"
            "    *) shift ;;\n"
            "  esac\n"
            "done\n"
            "printf '%s\\n' \"$url\" >> \"$CURL_LOG\"\n"
            "cp \"$FIXTURE_ARCHIVE\" \"$output\"\n",
            encoding="utf-8",
        )
        for tool in ("uname", "curl"):
            (self.fake_bin / tool).chmod(0o755)

    def write_ldd(self, output: str) -> None:
        (self.fake_bin / "ldd").write_text(
            "#!/usr/bin/env bash\n"
            f"printf '%s\\n' '{output}'\n",
            encoding="utf-8",
        )
        (self.fake_bin / "ldd").chmod(0o755)

    @staticmethod
    def write_archive(path: Path, *, executable: bool, member: str = "chezmoi-tui") -> None:
        data = b"#!/bin/sh\nprintf 'chezmoi-tui 0.3.1\\n'\n"
        with tarfile.open(path, "w:gz") as archive:
            info = tarfile.TarInfo(member)
            info.mode = 0o755 if executable else 0o644
            info.size = len(data)
            archive.addfile(info, io.BytesIO(data))

    def environment(self) -> dict[str, str]:
        return {
            "PATH": f"{self.fake_bin}:/usr/bin:/bin",
            "CURL_LOG": str(self.curl_log),
            "FIXTURE_ARCHIVE": str(self.fixture_archive),
            "ASDF_INSTALL_TYPE": "version",
            "ASDF_INSTALL_VERSION": VERSION,
            "ASDF_INSTALL_PATH": str(self.root / "install"),
            "ASDF_DOWNLOAD_PATH": str(self.root / "download"),
        }

    def run_script(self, name: str, env: dict[str, str]) -> subprocess.CompletedProcess[str]:
        return subprocess.run(
            ["bash", str(BIN / name)],
            env=env,
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            check=False,
        )

    def test_malformed_version_is_rejected_before_download(self) -> None:
        env = self.environment()
        env["ASDF_INSTALL_VERSION"] = "0.3.1-beta.1"
        result = self.run_script("download", env)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("stable MAJOR.MINOR.PATCH", result.stderr)
        self.assertFalse(self.curl_log.exists())

    def test_unsupported_platform_is_rejected_before_download(self) -> None:
        self.write_tools("Plan9", "x86_64")
        result = self.run_script("download", self.environment())
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("unsupported operating system", result.stderr)
        self.assertFalse(self.curl_log.exists())

    def test_root_binary_archive_downloads_and_installs(self) -> None:
        env = self.environment()
        downloaded = self.run_script("download", env)
        self.assertEqual(downloaded.returncode, 0, downloaded.stderr)
        Path(env["ASDF_INSTALL_PATH"]).mkdir()
        installed = self.run_script("install", env)
        self.assertEqual(installed.returncode, 0, installed.stderr)
        binary = Path(env["ASDF_INSTALL_PATH"]) / "bin/chezmoi-tui"
        self.assertTrue(binary.is_file())
        self.assertTrue(os.access(binary, os.X_OK))
        self.assertIn("chezmoi-tui 0.3.1", binary.read_text(encoding="utf-8"))
        self.assertIn(
            "releases/download/v0.3.1/chezmoi-tui_0.3.1_linux-x86_64-musl.tar.gz",
            self.curl_log.read_text(encoding="utf-8"),
        )

    def test_legacy_linux_asset_rules(self) -> None:
        env = self.environment()
        env["ASDF_INSTALL_VERSION"] = "0.3.0"
        result = self.run_script("download", env)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn(
            "chezmoi-tui_0.3.0_linux-x86_64.tar.gz",
            self.curl_log.read_text(encoding="utf-8"),
        )

        self.write_ldd("musl libc")
        env["ASDF_DOWNLOAD_PATH"] = str(self.root / "legacy-musl")
        result = self.run_script("download", env)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("not available before 0.3.1", result.stderr)

    def test_populated_install_path_is_preserved(self) -> None:
        env = self.environment()
        downloaded = self.run_script("download", env)
        self.assertEqual(downloaded.returncode, 0, downloaded.stderr)
        existing = Path(env["ASDF_INSTALL_PATH"])
        existing.mkdir()
        marker = existing / "user-data"
        marker.write_text("keep\n", encoding="utf-8")
        result = self.run_script("install", env)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("install path must be empty", result.stderr)
        self.assertEqual(marker.read_text(encoding="utf-8"), "keep\n")

    def test_unsafe_archive_members_are_rejected(self) -> None:
        for member in ("../chezmoi-tui", "chezmoi-tui-link"):
            with self.subTest(member=member):
                disallowed = self.root / f"{member.replace('/', '-')}.tar.gz"
                self.write_archive(disallowed, executable=True, member=member)
                env = self.environment()
                env["FIXTURE_ARCHIVE"] = str(disallowed)
                result = self.run_script("download", env)
                self.assertNotEqual(result.returncode, 0)
                self.assertIn("unexpected member", result.stderr)

        symlink_archive = self.root / "symlink.tar.gz"
        with tarfile.open(symlink_archive, "w:gz") as archive:
            info = tarfile.TarInfo("chezmoi-tui")
            info.type = tarfile.SYMTYPE
            info.linkname = "/bin/sh"
            archive.addfile(info)
        env = self.environment()
        env["FIXTURE_ARCHIVE"] = str(symlink_archive)
        result = self.run_script("download", env)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("non-regular member", result.stderr)

        corrupted = self.root / "corrupted.tar.gz"
        corrupted.write_bytes(b"not a tarball")
        env["FIXTURE_ARCHIVE"] = str(corrupted)
        result = self.run_script("download", env)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("invalid or corrupted archive", result.stderr)

    def test_shellcheck(self) -> None:
        shellcheck = shutil.which("shellcheck")
        if shellcheck is None:
            self.skipTest("shellcheck is not installed")
        scripts = [
            str(BIN / name)
            for name in ("list-all", "download", "install", "latest-stable")
        ]
        scripts.append(str(REPO / "lib/utils.bash"))
        result = subprocess.run(
            [shellcheck, "-x", *scripts],
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            check=False,
        )
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)


if __name__ == "__main__":
    unittest.main()
