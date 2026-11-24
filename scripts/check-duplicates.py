#!/usr/bin/env python3
"""Detect substantial copy-paste in workspace source.

A match is a 40-line window of normalized code that appears at two
non-overlapping places. Generated files and inline test modules are skipped;
small repeated idioms are below the window size. Known duplicates live in
`scripts/duplicate-baseline.json` so the check is a ratchet, not a rewrite.
"""

from __future__ import annotations

import hashlib
import json
import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
WINDOW = 40
MIN_MEANINGFUL = 25
BASELINE = ROOT / "scripts" / "duplicate-baseline.json"
TRIVIAL = re.compile(r"^[\s{}()\[\],;]+$")


def workspace_sources() -> list[Path]:
    out = subprocess.run(
        ["cargo", "metadata", "--format-version", "1", "--no-deps"],
        cwd=ROOT,
        check=True,
        capture_output=True,
        text=True,
    )
    meta = json.loads(out.stdout)
    members = set(meta["workspace_members"])
    files: list[Path] = []
    for pkg in meta["packages"]:
        if pkg["id"] not in members:
            continue
        src = Path(pkg["manifest_path"]).parent / "src"
        files.extend(sorted(src.rglob("*.rs")))
    return files


def normalized(path: Path) -> tuple[list[str], str]:
    lines: list[str] = []
    for raw in path.read_text(encoding="utf-8", errors="replace").splitlines():
        if "#[cfg(test)]" in raw:
            break
        line = raw.strip()
        if not line or TRIVIAL.match(line) or line.startswith("//"):
            continue
        if "@generated" in line:
            return [], "generated"
        lines.append(re.sub(r"\s+", " ", line))
    return lines, "source"


def main() -> int:
    baseline: dict[str, str] = {}
    if BASELINE.exists():
        baseline = json.loads(BASELINE.read_text(encoding="utf-8"))

    locations: dict[str, list[tuple[str, int]]] = {}
    for path in workspace_sources():
        lines, kind = normalized(path)
        if kind != "source" or len(lines) < WINDOW:
            continue
        rel = path.relative_to(ROOT).as_posix()
        for i in range(len(lines) - WINDOW + 1):
            window = lines[i : i + WINDOW]
            if sum(1 for line in window if not TRIVIAL.match(line)) < MIN_MEANINGFUL:
                continue
            digest = hashlib.sha256("\n".join(window).encode()).hexdigest()[:16]
            locations.setdefault(digest, []).append((rel, i + 1))

    findings: list[tuple[str, tuple[str, int], tuple[str, int]]] = []
    for digest, places in locations.items():
        if digest in baseline:
            continue
        first = places[0]
        for other in places[1:]:
            if first[0] != other[0] or abs(first[1] - other[1]) >= WINDOW:
                findings.append((digest, first, other))
                break

    if not findings:
        print("duplicates: ok (no new 40-line duplicate windows)")
        return 0

    for digest, a, b in findings:
        print(f"duplicate {digest}: {a[0]}:{a[1]} and {b[0]}:{b[1]}", file=sys.stderr)
    print(
        "duplicates: extract a shared helper, or record an intentional copy in "
        f"{BASELINE.relative_to(ROOT)}",
        file=sys.stderr,
    )
    return 1


if __name__ == "__main__":
    raise SystemExit(main())
