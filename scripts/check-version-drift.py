#!/usr/bin/env python3
"""Fail when workspace members declare different requirements for one crate.

Versions are centralized in `[workspace.dependencies]`. A member that adds a
direct dependency instead can silently drift from its siblings; this compares
the resolved metadata for every workspace member and reports differing
requirements. Path dependencies are ignored (the vendored rune tree is one
source).
"""

from __future__ import annotations

import json
import subprocess
import sys
from collections import defaultdict
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def metadata() -> dict:
    out = subprocess.run(
        ["cargo", "metadata", "--format-version", "1", "--no-deps"],
        cwd=ROOT,
        check=True,
        capture_output=True,
        text=True,
    )
    return json.loads(out.stdout)


def main() -> int:
    meta = metadata()
    members = set(meta["workspace_members"])
    seen: dict[str, dict[str, set[str]]] = defaultdict(lambda: defaultdict(set))

    for pkg in meta["packages"]:
        if pkg["id"] not in members:
            continue
        for dep in pkg["dependencies"]:
            if dep.get("path"):
                continue
            seen[dep["name"]][dep["req"]].add(pkg["name"])

    drift = {name: reqs for name, reqs in seen.items() if len(reqs) > 1}
    if not drift:
        print(f"version-drift: ok ({len(seen)} dependency names, no differing requirements)")
        return 0

    for name, reqs in sorted(drift.items()):
        print(f"version drift: {name}", file=sys.stderr)
        for req, packages in sorted(reqs.items()):
            print(f"  {req}: {', '.join(sorted(packages))}", file=sys.stderr)
    print(
        "version-drift: declare the dependency once in [workspace.dependencies]",
        file=sys.stderr,
    )
    return 1


if __name__ == "__main__":
    raise SystemExit(main())
