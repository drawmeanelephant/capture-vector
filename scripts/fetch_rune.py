#!/usr/bin/env python3
"""Fetch the exact Rune source and its required patched crates, without submodules."""
from pathlib import Path
import subprocess

PIN = "2a59b0f95b82972478a497c42151dbcb3ca5c69f"
root = Path(__file__).resolve().parents[1]
dest = root / "spike" / "vendor" / "rune"
if (dest / ".git").exists():
    actual = subprocess.check_output(["git", "-C", str(dest), "rev-parse", "HEAD"], text=True).strip()
    if actual != PIN:
        raise SystemExit(f"existing Rune checkout is {actual}, expected {PIN}")
else:
    dest.parent.mkdir(parents=True, exist_ok=True)
    subprocess.run(["git", "init", "--quiet", str(dest)], check=True)
    subprocess.run(["git", "-C", str(dest), "remote", "add", "origin", "https://github.com/anomalyco/rune.git"], check=True)
    subprocess.run(["git", "-C", str(dest), "fetch", "--depth=1", "origin", PIN], check=True)
    subprocess.run(["git", "-C", str(dest), "checkout", "--detach", "FETCH_HEAD"], check=True)
actual = subprocess.check_output(["git", "-C", str(dest), "rev-parse", "HEAD"], text=True).strip()
assert actual == PIN, actual
for relative in ["crates/rune", "crates/rune-kit", "vendor/wgpu-hal", "vendor/winit", "vendor/sctk-adwaita", "vendor/parley_engine", "vendor/accesskit_windows", "vendor/accesskit_macos"]:
    assert (dest / relative).is_dir(), relative
print(f"Rune source verified: {actual}")
