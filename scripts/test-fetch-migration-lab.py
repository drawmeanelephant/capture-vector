#!/usr/bin/env python3
"""Regression tests for scripts/fetch-migration-lab.sh."""

from __future__ import annotations

import os
import re
import shlex
import shutil
import stat
import subprocess
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SCRIPT = ROOT / "scripts" / "fetch-migration-lab.sh"
MANIFEST = ROOT / "fixtures" / "MANIFEST.toml"


def pin(key: str) -> str:
    text = MANIFEST.read_text()
    section = re.search(r"(?ms)^\[migration_lab\]\n(.*?)(?=^\[|\Z)", text)
    if section is None:
        raise AssertionError("MANIFEST.toml missing [migration_lab]")
    match = re.search(rf'^{re.escape(key)} = "([^"]+)"', section.group(1), re.MULTILINE)
    if match is None:
        raise AssertionError(f"MANIFEST.toml migration-lab pin missing {key}")
    return match.group(1)


TOOL_ID = pin("tool_id")
COMMIT = pin("commit")
ZIG_SERIES = pin("zig_series")
ZIG_VERSION = pin("zig_version")


def write_executable(path: Path, contents: str) -> None:
    path.write_text(contents)
    path.chmod(path.stat().st_mode | stat.S_IXUSR)


def install_fake_git(bin_dir: Path) -> None:
    write_executable(
        bin_dir / "git",
        f"""#!/bin/sh
dir=
if [ "$1" = "-C" ]; then
  dir=$2
  shift 2
fi
cmd=$1
shift
case "$cmd" in
clone)
  mkdir -p "$3/.git" || exit 1
  ;;
fetch|checkout)
  exit 0
  ;;
rev-parse)
  case "$1" in
  --git-dir)
    if [ -n "$dir" ] && [ -d "$dir/.git" ]; then
      printf '%s\\n' "$dir/.git"
    else
      exit 1
    fi
    ;;
  HEAD)
    printf '%s\\n' '{COMMIT}'
    ;;
  *)
    exit 1
    ;;
  esac
  ;;
*)
  echo "fake git: unexpected $cmd $*" >&2
  exit 1
  ;;
esac
""",
    )


def install_fake_zig(path: Path, version: str, log: Path) -> None:
    if "'" in version or "'" in TOOL_ID:
        raise AssertionError("test fixture does not quote single quotes")
    write_executable(
        path,
        f"""#!/bin/sh
printf '%s\\n' "$1" >> {shlex.quote(str(log))}
if [ "$1" = version ]; then
  echo '{version}'
  exit 0
fi
if [ "$1" = build ]; then
  mkdir -p zig-out/bin || exit 1
  cat > zig-out/bin/boris-migration-lab <<'LAB_EOF'
#!/bin/sh
if [ "$1" = --version ]; then
  echo '{TOOL_ID}'
  exit 0
fi
exit 1
LAB_EOF
  chmod +x zig-out/bin/boris-migration-lab || exit 1
  exit 0
fi
exit 1
""",
    )


def install_fake_cygpath(bin_dir: Path, converted: Path) -> None:
    write_executable(
        bin_dir / "cygpath",
        f"""#!/bin/sh
if [ "$1" = "-u" ]; then
  printf '%s\\n' {shlex.quote(str(converted))}
  exit 0
fi
exit 1
""",
    )


def prepare(tmp: Path) -> tuple[Path, Path]:
    root = tmp / "repo"
    (root / "scripts").mkdir(parents=True)
    (root / "fixtures").mkdir()
    shutil.copy2(SCRIPT, root / "scripts" / SCRIPT.name)
    shutil.copy2(MANIFEST, root / "fixtures" / MANIFEST.name)
    bin_dir = tmp / "bin"
    bin_dir.mkdir()
    install_fake_git(bin_dir)
    return root, bin_dir


def run_direct(root: Path, bin_dir: Path, zig_value: str) -> subprocess.CompletedProcess[str]:
    env = os.environ.copy()
    env["PATH"] = str(bin_dir) + os.pathsep + env.get("PATH", "")
    command = f"ZIG={shlex.quote(zig_value)} ./scripts/fetch-migration-lab.sh"
    return subprocess.run(
        ["bash", "-c", command],
        cwd=root,
        env=env,
        capture_output=True,
        text=True,
    )


class ManifestPinTests(unittest.TestCase):
    def test_zig_series_is_the_major_minor_series_of_the_capture_compiler(self) -> None:
        self.assertRegex(ZIG_SERIES, r"^\d+\.\d+$")
        self.assertTrue(ZIG_VERSION.startswith(ZIG_SERIES + "."))


@unittest.skipIf(os.name == "nt", "the fake compiler and git are POSIX shell executables")
class FetchMigrationLabScriptTests(unittest.TestCase):
    def test_inline_zig_override_reaches_directly_executed_script(self) -> None:
        with tempfile.TemporaryDirectory() as raw_tmp:
            tmp = Path(raw_tmp)
            root, bin_dir = prepare(tmp)
            default_log = tmp / "default-zig.log"
            custom_log = tmp / "custom-zig.log"
            custom_zig = bin_dir / "zig-custom"
            install_fake_zig(bin_dir / "zig", ZIG_VERSION, default_log)
            install_fake_zig(custom_zig, ZIG_SERIES + ".0", custom_log)

            result = run_direct(root, bin_dir, str(custom_zig))

            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual(custom_log.read_text().splitlines(), ["version", "build"])
            self.assertFalse(default_log.exists())
            self.assertNotIn("warning", result.stderr)

    def test_zig_outside_the_pinned_series_is_refused_before_build(self) -> None:
        with tempfile.TemporaryDirectory() as raw_tmp:
            tmp = Path(raw_tmp)
            root, bin_dir = prepare(tmp)
            custom_log = tmp / "custom-zig.log"
            custom_zig = bin_dir / "zig-custom"
            major, minor = ZIG_SERIES.split(".", 1)
            outside_series = f"{major}.{int(minor) + 1}.0"
            install_fake_zig(custom_zig, outside_series, custom_log)

            result = run_direct(root, bin_dir, str(custom_zig))

            self.assertNotEqual(result.returncode, 0)
            self.assertIn("unsupported", result.stderr)
            self.assertIn(ZIG_SERIES + ".x", result.stderr)
            self.assertEqual(custom_log.read_text().splitlines(), ["version"])
            self.assertFalse((root / "vendor" / "boris-migration-lab").exists())

    def test_windows_drive_zig_path_is_normalized_with_cygpath(self) -> None:
        with tempfile.TemporaryDirectory() as raw_tmp:
            tmp = Path(raw_tmp)
            root, bin_dir = prepare(tmp)
            custom_log = tmp / "custom-zig.log"
            custom_zig = bin_dir / "zig-custom"
            install_fake_zig(custom_zig, ZIG_VERSION, custom_log)
            install_fake_cygpath(bin_dir, custom_zig)

            result = run_direct(root, bin_dir, "C:\\tools\\zig.exe")

            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual(custom_log.read_text().splitlines(), ["version", "build"])


if __name__ == "__main__":
    unittest.main()
