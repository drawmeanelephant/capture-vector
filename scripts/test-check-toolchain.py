#!/usr/bin/env python3
"""Tests for scripts/check-toolchain.sh using a fake rustc on PATH."""

import os
import stat
import subprocess
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SCRIPT = ROOT / "scripts" / "check-toolchain.sh"


def pinned_channel() -> str:
    for line in (ROOT / "rust-toolchain.toml").read_text().splitlines():
        if line.strip().startswith("channel"):
            return line.split("=", 1)[1].strip().strip('"')
    raise AssertionError("no channel in rust-toolchain.toml")


def run_with_rustc(version: str, **env: str) -> subprocess.CompletedProcess:
    with tempfile.TemporaryDirectory() as tmp:
        fake = Path(tmp) / "rustc"
        fake.write_text(f"#!/bin/sh\necho 'rustc {version} (0000000 2026-01-01)'\n")
        fake.chmod(fake.stat().st_mode | stat.S_IXUSR)
        full_env = {**os.environ, "PATH": f"{tmp}{os.pathsep}{os.environ['PATH']}"}
        full_env.pop("CV_ALLOW_TOOLCHAIN_DRIFT", None)
        full_env.update(env)
        return subprocess.run(
            ["bash", str(SCRIPT)], env=full_env, capture_output=True, text=True
        )


class CheckToolchainTests(unittest.TestCase):
    def test_matching_rustc_passes_silently(self) -> None:
        result = run_with_rustc(pinned_channel())
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stderr, "")

    def test_drifted_rustc_fails_with_fix(self) -> None:
        result = run_with_rustc("0.0.1")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("error: toolchain", result.stderr)
        self.assertIn("rustup run", result.stderr)

    def test_drift_override_warns_and_passes(self) -> None:
        result = run_with_rustc("0.0.1", CV_ALLOW_TOOLCHAIN_DRIFT="1")
        self.assertEqual(result.returncode, 0)
        self.assertIn("warning: toolchain", result.stderr)


if __name__ == "__main__":
    unittest.main()
