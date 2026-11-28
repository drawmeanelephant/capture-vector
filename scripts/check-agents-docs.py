#!/usr/bin/env python3
"""Validate the local paths, links, and commands documented for agents.

This is deliberately conservative: it checks paths that look like repository
paths and skips prose, shell commands, env assignments, and workspace-relative
page names. CI and the pre-commit hook run it.
"""

from __future__ import annotations

import os
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]

DOCS = [
    "AGENTS.md",
    "README.md",
    "CONTRIBUTING.md",
    "SECURITY.md",
    "docs/README.md",
    "docs/user-guide.md",
    "docs/agent-claims.md",
    "docs/architecture-overview.md",
    "docs/TRIAGE.md",
    "docs/PACKAGING.md",
    "docs/runbooks/desktop-app.md",
    "docs/tech-debt.md",
]

REQUIRED = [
    "AGENTS.md",
    "README.md",
    ".env.example",
    "Cargo.toml",
    "Cargo.lock",
    "rust-toolchain.toml",
    "deny.toml",
    "fixtures/MANIFEST.toml",
    "spec/PROMPT-v3.md",
    "spec/architecture.md",
    "spec/contracts.md",
    "spec/GAMEPLAN.md",
    "spike/FINDINGS.md",
    "docs/README.md",
    "docs/user-guide.md",
    "docs/agent-claims.md",
    "docs/architecture-overview.md",
    "docs/runbooks/desktop-app.md",
    "docs/tech-debt.md",
    "scripts/agent-claim.py",
    "scripts/test-agent-claim.py",
    "scripts/test-fetch-migration-lab.py",
    "scripts/fetch-rune.sh",
    "scripts/fetch-migration-lab.sh",
    "scripts/dev.sh",
    "scripts/demo.sh",
    "scripts/install-hooks.sh",
    "scripts/lint.sh",
    "scripts/check-hygiene.sh",
    "scripts/check-agents-docs.py",
    "scripts/check-version-drift.py",
    "scripts/check-duplicates.py",
    "scripts/check-large-files.sh",
    "scripts/check-todos.sh",
    "scripts/check-secrets.sh",
    "scripts/check-unused-deps.sh",
    "scripts/release-notes.sh",
    ".githooks/pre-commit",
]

EXECUTABLE = [
    "scripts/agent-claim.py",
    "scripts/test-agent-claim.py",
    "scripts/test-fetch-migration-lab.py",
    "scripts/fetch-rune.sh",
    "scripts/fetch-migration-lab.sh",
    "scripts/dev.sh",
    "scripts/demo.sh",
    "scripts/install-hooks.sh",
    "scripts/lint.sh",
    "scripts/check-hygiene.sh",
    "scripts/check-large-files.sh",
    "scripts/check-todos.sh",
    "scripts/check-secrets.sh",
    "scripts/check-unused-deps.sh",
    "scripts/release-notes.sh",
    ".githooks/pre-commit",
]

PREFIXES = (
    "spec/",
    "crates/",
    "scripts/",
    "fixtures/",
    "spike/",
    "rulepacks/",
    "assets/",
    "docs/",
    "xtask/",
)

SKIP_PREFIXES = (
    "target/",
    "dist/",
    ".cv/",
    "vendor/",
    "tests/",
    "content/",
    "~",
    "/",
    "./",
)

SUFFIXES = (
    ".md",
    ".sh",
    ".toml",
    ".rs",
    ".json",
    ".png",
    ".yml",
    ".yaml",
    ".svg",
    ".example",
)

ROOT_FILES = {
    "README.md",
    "AGENTS.md",
    "Cargo.toml",
    "Cargo.lock",
    "rust-toolchain.toml",
    "deny.toml",
    ".env.example",
    "CODEOWNERS",
}

LINK_RE = re.compile(r"\[[^\]]*\]\(([^)]+)\)")
HTML_SRC_RE = re.compile(r'(?:src|href)="([^"]+)"')
BACKTICK_RE = re.compile(r"`([^`\n]+)`")
LINE_SUFFIX_RE = re.compile(r"^(.*?)(?::\d+(?::\d+)?)$")


def check_path(rel: str, doc: str, problems: list[str]) -> None:
    rel = rel.strip()
    if not rel or rel.startswith("#"):
        return
    if "://" in rel or rel.startswith(("mailto:", "http:", "https:")):
        return
    if rel.startswith("<") or any(ch in rel for ch in "*{}|"):
        return

    rel = rel.split("#", 1)[0]
    match = LINE_SUFFIX_RE.match(rel)
    if match:
        rel = match.group(1)
    rel = rel.rstrip("/")
    if not rel:
        return
    if rel.startswith(SKIP_PREFIXES):
        return

    if "/" in rel:
        if rel.startswith("crates/<") or not rel.startswith(PREFIXES):
            return
    elif rel not in ROOT_FILES:
        return

    if not (ROOT / rel).exists():
        problems.append(f"{doc}: missing path `{rel}`")


def main() -> int:
    problems: list[str] = []

    for rel in REQUIRED:
        if not (ROOT / rel).exists():
            problems.append(f"required file missing: {rel}")

    for rel in EXECUTABLE:
        path = ROOT / rel
        if path.exists() and not os.access(path, os.X_OK):
            problems.append(f"not executable: {rel}")

    for doc in DOCS:
        path = ROOT / doc
        if not path.exists():
            problems.append(f"missing doc: {doc}")
            continue
        text = path.read_text(encoding="utf-8")

        for target in LINK_RE.findall(text) + HTML_SRC_RE.findall(text):
            check_path(target, doc, problems)

        for span in BACKTICK_RE.findall(text):
            span = span.strip()
            if not span or any(ch.isspace() for ch in span):
                continue
            if any(ch in span for ch in "<>{}$|'\"=:()[]"):
                continue
            if span.startswith(("CV_", "mod+", "cargo ", "git ", "boris/", "rune/", "oliver/")):
                continue
            if span.startswith("-") or span.startswith("--"):
                continue
            if not (span.startswith(PREFIXES) or span.endswith(SUFFIXES)):
                continue
            check_path(span, doc, problems)

    if problems:
        for p in problems:
            print(p, file=sys.stderr)
        print(f"agents-docs: {len(problems)} problem(s)", file=sys.stderr)
        return 1

    print("agents-docs: ok")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
