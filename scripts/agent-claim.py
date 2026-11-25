#!/usr/bin/env python3
"""Cooperative issue claims for parallel agents.

The claim ledger is the issue's comment stream. GitHub does not provide a
compare-and-swap issue assignment operation, so claiming is deliberately
optimistic: post a structured claim, re-read the ledger, and yield when an
earlier active claim exists. The earliest comment timestamp wins.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import subprocess
import sys
import uuid
from dataclasses import dataclass
from datetime import datetime, timedelta, timezone
from typing import Any

MARKER = "capture-vector-claim:v1"
CLAIM_LABEL = "agent: claimed"
MAX_CLAIM_HOURS = 24.0

JSON_BLOCK_RE = re.compile(
    r"<!--\s*capture-vector-claim:v1\s*-->\s*```json\s*(\{.*?\})\s*```",
    re.DOTALL,
)


class ClaimError(RuntimeError):
    pass


@dataclass(frozen=True)
class ClaimEvent:
    record: dict[str, Any]
    comment_id: int
    created_at: datetime
    author_login: str
    author_association: str

    @property
    def claim_id(self) -> str:
        return str(self.record.get("id", ""))

    @property
    def event(self) -> str:
        return str(self.record.get("event", ""))

    @property
    def agent(self) -> str:
        return str(self.record.get("agent", "unknown"))

    @property
    def expires_at(self) -> datetime | None:
        raw = self.record.get("expires_at")
        if not isinstance(raw, str):
            return None
        return parse_time(raw)


def utc_now() -> datetime:
    return datetime.now(timezone.utc)


def format_time(value: datetime) -> str:
    return value.astimezone(timezone.utc).isoformat(timespec="seconds").replace("+00:00", "Z")


def parse_time(value: str) -> datetime:
    try:
        return datetime.fromisoformat(value.replace("Z", "+00:00"))
    except ValueError as exc:
        raise ClaimError(f"invalid timestamp in claim record: {value!r}") from exc


def run(cmd: list[str], *, input_text: str | None = None) -> str:
    try:
        proc = subprocess.run(
            cmd,
            input=input_text,
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            check=False,
        )
    except FileNotFoundError as exc:
        raise ClaimError(f"required executable not found: {cmd[0]}") from exc
    if proc.returncode != 0:
        detail = proc.stderr.strip() or proc.stdout.strip() or "command failed"
        raise ClaimError(f"{' '.join(cmd[:2])}: {detail}")
    return proc.stdout


def repo_slug(explicit: str | None = None) -> str:
    if explicit:
        return explicit.removesuffix(".git")
    if os.environ.get("GITHUB_REPOSITORY"):
        return os.environ["GITHUB_REPOSITORY"]

    url = run(["git", "config", "--get", "remote.origin.url"]).strip()
    match = re.search(r"github\.com[:/]([A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+?)(?:\.git)?/?$", url)
    if not match:
        raise ClaimError(f"cannot derive owner/repo from origin URL: {url}")
    return match.group(1)


def gh_api(repo: str, path: str, *, method: str = "GET", fields: dict[str, str] | None = None) -> Any:
    separator = "&" if "?" in path else "?"
    if method == "GET" and "per_page=" not in path:
        path = f"{path}{separator}per_page=100"

    cmd = ["gh", "api", f"repos/{repo}/{path}"]
    if method != "GET":
        cmd.extend(["-X", method])
    if method == "GET":
        cmd.extend(["--paginate", "--slurp"])
    for key, value in (fields or {}).items():
        cmd.extend(["-f", f"{key}={value}"])

    raw = run(cmd)
    data = json.loads(raw or "null")
    if method == "GET" and isinstance(data, list):
        flattened: list[Any] = []
        for page in data:
            if isinstance(page, list):
                flattened.extend(page)
            else:
                flattened.append(page)
        return flattened
    return data


def parse_claim_events(comments: list[dict[str, Any]]) -> list[ClaimEvent]:
    events: list[ClaimEvent] = []
    for comment in comments:
        body = comment.get("body") or ""
        match = JSON_BLOCK_RE.search(body)
        if not match:
            continue
        try:
            record = json.loads(match.group(1))
        except json.JSONDecodeError:
            continue
        if record.get("cv_claim") != 1 or not record.get("id"):
            continue
        events.append(
            ClaimEvent(
                record=record,
                comment_id=int(comment.get("id", 0)),
                created_at=parse_time(str(comment.get("created_at"))),
                author_login=str((comment.get("user") or {}).get("login", "")),
                author_association=str(comment.get("author_association", "")),
            )
        )
    return sorted(events, key=lambda event: (event.created_at, event.comment_id))


def active_claims(events: list[ClaimEvent], now: datetime | None = None) -> list[ClaimEvent]:
    now = now or utc_now()
    latest: dict[str, ClaimEvent] = {}
    first_seen: dict[str, tuple[datetime, int]] = {}
    claim_authors: dict[str, str] = {}
    released: set[str] = set()
    maintainer_associations = {"OWNER", "MEMBER", "COLLABORATOR"}
    for event in events:
        if event.claim_id in released:
            continue
        if event.event == "claim":
            first_seen.setdefault(event.claim_id, (event.created_at, event.comment_id))
            claim_authors[event.claim_id] = event.author_login
            latest[event.claim_id] = event
        elif event.event == "heartbeat":
            if event.author_login == claim_authors.get(event.claim_id):
                latest[event.claim_id] = event
        elif event.event == "release":
            same_author = event.author_login == claim_authors.get(event.claim_id)
            maintainer = event.author_association in maintainer_associations
            if same_author or maintainer:
                latest.pop(event.claim_id, None)
                released.add(event.claim_id)

    active = []
    for event in latest.values():
        try:
            expires = event.expires_at
        except ClaimError:
            continue
        if expires is not None and expires > now:
            active.append(event)
    return sorted(
        active,
        key=lambda event: first_seen.get(event.claim_id, (event.created_at, event.comment_id)),
    )


def claim_body(record: dict[str, Any]) -> str:
    payload = json.dumps(record, indent=2, sort_keys=True)
    lines = [
        f"<!-- {MARKER} -->",
        "```json",
        payload,
        "```",
        "",
    ]
    if record["event"] == "claim":
        lines.extend(
            [
                f"Claiming this issue for **{record['scope']}** until `{record['expires_at']}`.",
                f"Agent: `{record['agent']}`",
            ]
        )
        if record.get("session"):
            lines.append(f"Session: {record['session']}")
    elif record["event"] == "heartbeat":
        lines.append(f"Heartbeat for claim `{record['id']}`; active until `{record['expires_at']}`.")
    else:
        reason = record.get("reason", "done")
        lines.append(f"Releasing claim `{record['id']}` ({reason}).")
    return "\n".join(lines) + "\n"


def fetch_events(repo: str, issue: int) -> list[ClaimEvent]:
    comments = gh_api(repo, f"issues/{issue}/comments")
    if not isinstance(comments, list):
        raise ClaimError("unexpected comments response from GitHub")
    return parse_claim_events(comments)


def post_event(repo: str, issue: int, record: dict[str, Any]) -> None:
    gh_api(
        repo,
        f"issues/{issue}/comments",
        method="POST",
        fields={"body": claim_body(record)},
    )


def best_effort_label(repo: str, issue: int, label: str, action: str) -> None:
    flag = "--add-label" if action == "add" else "--remove-label"
    proc = subprocess.run(
        ["gh", "issue", "edit", str(issue), "--repo", repo, flag, label],
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        check=False,
    )
    if proc.returncode != 0:
        print(f"warning: could not {action} label {label!r}: {proc.stderr.strip()}", file=sys.stderr)


def require_agent(args: argparse.Namespace) -> str:
    agent = args.agent or os.environ.get("CV_AGENT")
    if not agent:
        raise ClaimError("pass --agent or set CV_AGENT to a unique agent/session name")
    return agent


def new_record(args: argparse.Namespace, event: str) -> dict[str, Any]:
    now = utc_now()
    expires = now + timedelta(hours=args.hours)
    record: dict[str, Any] = {
        "cv_claim": 1,
        "event": event,
        "id": args.claim_id or f"claim-{format_time(now).replace(':', '').replace('-', '')}-{uuid.uuid4().hex[:8]}",
        "agent": require_agent(args),
        "scope": args.scope,
        "expires_at": format_time(expires),
    }
    if args.session:
        record["session"] = args.session
    if args.reason:
        record["reason"] = args.reason
    return record


def print_claims(claims: list[ClaimEvent]) -> None:
    if not claims:
        print("no active claims")
        return
    for event in claims:
        record = event.record
        print(
            f"{event.claim_id}\t{event.agent}\t{record.get('scope', 'implementation')}"
            f"\texpires {record.get('expires_at')}"
        )


def cmd_status(args: argparse.Namespace) -> int:
    events = fetch_events(args.repo, args.issue)
    claims = active_claims(events)
    if args.json:
        print(json.dumps([event.record for event in claims], indent=2, sort_keys=True))
    else:
        print_claims(claims)
    return 0


def cmd_claim(args: argparse.Namespace) -> int:
    existing = active_claims(fetch_events(args.repo, args.issue))
    if existing:
        print("issue already has an active claim:", file=sys.stderr)
        print_claims(existing)
        return 2

    record = new_record(args, "claim")
    post_event(args.repo, args.issue, record)

    after = active_claims(fetch_events(args.repo, args.issue))
    mine = next((event for event in after if event.claim_id == record["id"]), None)
    if mine is None:
        raise ClaimError("claim was posted but is not active; check for a malformed or immediate release event")

    winner = after[0] if after else mine
    if winner.claim_id != record["id"]:
        release = dict(record)
        release["event"] = "release"
        release["reason"] = "lost-race"
        post_event(args.repo, args.issue, release)
        print(f"lost claim race to {winner.claim_id} ({winner.agent}); yielded", file=sys.stderr)
        return 3

    if not args.no_label:
        best_effort_label(args.repo, args.issue, CLAIM_LABEL, "add")
    print(f"claimed #{args.issue}: {record['id']} until {record['expires_at']}")
    print(f"heartbeat: python3 scripts/agent-claim.py heartbeat {args.issue} --claim-id {record['id']} --agent {record['agent']}")
    print(f"release:   python3 scripts/agent-claim.py release {args.issue} --claim-id {record['id']} --agent {record['agent']}")
    return 0


def cmd_heartbeat(args: argparse.Namespace) -> int:
    agent = require_agent(args)
    active = active_claims(fetch_events(args.repo, args.issue))
    existing = next((event for event in active if event.claim_id == args.claim_id), None)
    if existing is None:
        raise ClaimError(f"no active claim {args.claim_id!r} on issue #{args.issue}")
    if existing.agent != agent:
        raise ClaimError(f"claim {args.claim_id!r} belongs to {existing.agent!r}, not {agent!r}")

    record = new_record(args, "heartbeat")
    record["id"] = args.claim_id
    post_event(args.repo, args.issue, record)
    print(f"heartbeat posted for {args.claim_id} until {record['expires_at']}")
    return 0


def cmd_release(args: argparse.Namespace) -> int:
    events = fetch_events(args.repo, args.issue)
    if not any(event.claim_id == args.claim_id for event in events):
        raise ClaimError(f"no claim {args.claim_id!r} found on issue #{args.issue}")

    record = new_record(args, "release")
    record["id"] = args.claim_id
    if args.reason:
        record["reason"] = args.reason
    post_event(args.repo, args.issue, record)

    claims = active_claims(fetch_events(args.repo, args.issue))
    if not claims and not args.no_label:
        best_effort_label(args.repo, args.issue, CLAIM_LABEL, "remove")
    print(f"released {args.claim_id}")
    return 0


def cmd_scan(args: argparse.Namespace) -> int:
    issues = gh_api(args.repo, "issues?state=open")
    found: list[tuple[int, ClaimEvent]] = []
    for issue in issues:
        if "pull_request" in issue:
            continue
        number = int(issue["number"])
        for event in active_claims(fetch_events(args.repo, number)):
            found.append((number, event))

    if args.json:
        print(
            json.dumps(
                [
                    {"issue": number, **event.record}
                    for number, event in sorted(found, key=lambda item: item[0])
                ],
                indent=2,
                sort_keys=True,
            )
        )
    elif not found:
        print("no active claims on open issues")
    else:
        for number, event in sorted(found, key=lambda item: item[0]):
            print(f"#{number}\t{event.claim_id}\t{event.agent}\texpires {event.record.get('expires_at')}")
    return 0


def add_common(parser: argparse.ArgumentParser) -> None:
    parser.add_argument("--repo", help="owner/repo; defaults to GITHUB_REPOSITORY or origin")
    parser.add_argument("--json", action="store_true", help="emit machine-readable JSON where supported")


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    subparsers = parser.add_subparsers(dest="command", required=True)

    status = subparsers.add_parser("status", help="show active claims for one issue")
    status.add_argument("issue", type=int)
    add_common(status)
    status.set_defaults(func=cmd_status)

    scan = subparsers.add_parser("scan", help="scan open issues for active claims")
    add_common(scan)
    scan.set_defaults(func=cmd_scan)

    for name, help_text, func in [
        ("claim", "claim an issue", cmd_claim),
        ("heartbeat", "extend an active claim", cmd_heartbeat),
        ("release", "release a claim", cmd_release),
    ]:
        sub = subparsers.add_parser(name, help=help_text)
        sub.add_argument("issue", type=int)
        sub.add_argument("--agent", help="unique agent/session name (or set CV_AGENT)")
        sub.add_argument("--session", help="agent session URL or other resumable reference")
        sub.add_argument("--scope", default="implementation", help="claimed scope; use lane/file detail when useful")
        sub.add_argument("--hours", type=float, default=4.0, help="lease length in hours (default: 4, max: 24)")
        sub.add_argument("--claim-id", help="existing claim id for heartbeat/release")
        sub.add_argument("--reason", help="release reason")
        sub.add_argument("--no-label", action="store_true", help="do not mirror status into issue labels")
        add_common(sub)
        sub.set_defaults(func=func)

    args = parser.parse_args(argv)
    args.repo = repo_slug(args.repo)
    if hasattr(args, "hours") and not 0 < args.hours <= MAX_CLAIM_HOURS:
        parser.error(f"--hours must be between 0 and {MAX_CLAIM_HOURS}")
    if getattr(args, "command", "") in {"heartbeat", "release"} and not args.claim_id:
        parser.error(f"{args.command} requires --claim-id")

    try:
        return int(args.func(args))
    except ClaimError as exc:
        print(f"agent-claim: {exc}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
