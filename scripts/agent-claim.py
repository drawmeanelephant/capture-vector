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
MAINTAINER_ASSOCIATIONS = frozenset({"OWNER", "MEMBER", "COLLABORATOR"})
CHECK_FAILED = 4

JSON_BLOCK_RE = re.compile(
    r"<!--\s*capture-vector-claim:v1\s*-->\s*```json\s*(\{.*?\})\s*```",
    re.DOTALL,
)


ISSUE_REF_RE = re.compile(
    r"\b(?P<keyword>close[sd]?|fix(?:e[sd])?|resolve[sd]?|part\s+of)\b:?\s+"
    r"(?P<repo>[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+)?#(?P<number>\d+)\b",
    re.IGNORECASE,
)
PR_CLAIM_LINE_RE = re.compile(r"^\s*claim:\s*(?P<value>.+?)\s*$", re.IGNORECASE | re.MULTILINE)
CLAIM_ID_RE = re.compile(r"claim-[A-Za-z0-9_-]+")
NO_CLAIM_BRANCHES = frozenset({"", "HEAD", "main", "master"})


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


@dataclass
class ClaimHistory:
    first: ClaimEvent
    latest: ClaimEvent
    release: ClaimEvent | None = None

    @property
    def order(self) -> tuple[datetime, int]:
        return (self.first.created_at, self.first.comment_id)

    def is_active(self, now: datetime) -> bool:
        if self.release is not None:
            return False
        try:
            expires = self.latest.expires_at
        except ClaimError:
            return False
        return expires is not None and expires > now


def claim_histories(events: list[ClaimEvent]) -> list[ClaimHistory]:
    """Fold the ledger into one history per claim id, in claim order.

    Heartbeats and releases count only from the claim's comment author or a
    maintainer (release only); everything after an honored release is ignored.
    """
    histories: dict[str, ClaimHistory] = {}
    claim_authors: dict[str, str] = {}
    for event in events:
        history = histories.get(event.claim_id)
        if history is not None and history.release is not None:
            continue
        if event.event == "claim":
            claim_authors[event.claim_id] = event.author_login
            if history is None:
                histories[event.claim_id] = ClaimHistory(first=event, latest=event)
            else:
                history.latest = event
        elif history is None:
            continue
        elif event.event == "heartbeat":
            if event.author_login == claim_authors.get(event.claim_id):
                history.latest = event
        elif event.event == "release":
            same_author = event.author_login == claim_authors.get(event.claim_id)
            if same_author or event.author_association in MAINTAINER_ASSOCIATIONS:
                history.release = event
    return sorted(histories.values(), key=lambda history: history.order)


def active_claims(events: list[ClaimEvent], now: datetime | None = None) -> list[ClaimEvent]:
    now = now or utc_now()
    return [history.latest for history in claim_histories(events) if history.is_active(now)]


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
        if record.get("branch"):
            lines.append(f"Branch: `{record['branch']}`")
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
    if event == "claim":
        branch = claim_branch(args.branch)
        if branch:
            record["branch"] = branch
    if args.reason:
        record["reason"] = args.reason
    return record


def claim_branch(explicit: str | None) -> str | None:
    if explicit is not None:
        return explicit or None
    try:
        branch = run(["git", "rev-parse", "--abbrev-ref", "HEAD"]).strip()
    except ClaimError:
        return None
    return None if branch in NO_CLAIM_BRANCHES else branch


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


@dataclass(frozen=True)
class IssueRef:
    number: int
    closing: bool


@dataclass(frozen=True)
class Finding:
    level: str  # "error" | "warning" | "notice"
    issue: int | None
    message: str


def issue_refs(body: str, repo: str) -> list[IssueRef]:
    """Issue references a PR body makes with a closing keyword or `Part of`.

    Bare `#N` mentions are prose, not work claims, and are ignored, as are
    references to other repositories.
    """
    refs: dict[int, IssueRef] = {}
    for match in ISSUE_REF_RE.finditer(body or ""):
        other_repo = match.group("repo")
        if other_repo and other_repo.lower() != repo.lower():
            continue
        number = int(match.group("number"))
        closing = not match.group("keyword").lower().startswith("part")
        previous = refs.get(number)
        refs[number] = IssueRef(number, closing or (previous is not None and previous.closing))
    return sorted(refs.values(), key=lambda ref: ref.number)


def pr_claim_values(body: str) -> tuple[set[str], bool]:
    """Claim ids named on `Claim:` lines, and whether one says `maintainer`."""
    ids: set[str] = set()
    maintainer = False
    for match in PR_CLAIM_LINE_RE.finditer(body or ""):
        value = match.group("value")
        ids.update(CLAIM_ID_RE.findall(value))
        if re.search(r"\bmaintainer\b", value, re.IGNORECASE):
            maintainer = True
    return ids, maintainer


def claim_links_branch(record: dict[str, Any], branch: str) -> bool:
    if not branch:
        return False
    if record.get("branch") == branch:
        return True
    token = re.compile(rf"(?<![\w./-]){re.escape(branch)}(?![\w./-])")
    return any(token.search(str(record.get(key) or "")) for key in ("session", "scope"))


def claim_command(issue: int, branch: str) -> str:
    return (
        f"python3 scripts/agent-claim.py claim {issue} "
        f"--agent <unique-agent-id> --branch {branch}"
    )


def evaluate_pr(
    pr: dict[str, Any],
    open_prs: list[dict[str, Any]],
    ledgers: dict[int, list[ClaimEvent]],
    repo: str,
    now: datetime | None = None,
) -> list[Finding]:
    """Check one PR's issue references against claim ledgers and other open PRs.

    A claim counts when it names this PR's head branch (`branch` field, or the
    branch as a token in `session`/`scope`) or when the PR body lists its id
    on a `Claim:` line. Released claims count, because the protocol releases
    on `pr-opened`; only claims that yielded a race (`lost-race`) do not.
    """
    now = now or utc_now()
    number = int(pr["number"])
    branch = str((pr.get("head") or {}).get("ref", ""))
    body = pr.get("body") or ""
    named_ids, maintainer_opt_out = pr_claim_values(body)
    maintainer_ok = maintainer_opt_out and pr.get("author_association") in MAINTAINER_ASSOCIATIONS
    findings: list[Finding] = []

    refs = issue_refs(body, repo)
    if not refs:
        findings.append(Finding("notice", None, "no Fixes/Closes/Resolves/Part of #N references; nothing to check"))
        return findings

    for ref in refs:
        if not ref.closing:
            findings.append(
                Finding(
                    "notice",
                    ref.number,
                    f"#{ref.number} is referenced with 'Part of'; tracking issues are shared and are not claim-checked",
                )
            )
            continue

        histories = claim_histories(ledgers.get(ref.number, []))
        usable = [h for h in histories if (h.release is None or h.release.record.get("reason") != "lost-race")]
        mine = [
            h
            for h in usable
            if h.first.claim_id in named_ids or claim_links_branch(h.first.record, branch)
        ]
        foreign_active = [h for h in histories if h.is_active(now) and all(h is not m for m in mine)]

        if mine:
            first_mine = mine[0]
            earlier = [h for h in foreign_active if h.order < first_mine.order]
            later = [h for h in foreign_active if h.order > first_mine.order]
            for h in earlier:
                findings.append(
                    Finding(
                        "error",
                        ref.number,
                        f"#{ref.number} has an earlier active claim {h.first.claim_id} by {h.first.agent}; "
                        f"this PR's claim {first_mine.first.claim_id} should have yielded",
                    )
                )
            for h in later:
                findings.append(
                    Finding(
                        "warning",
                        ref.number,
                        f"#{ref.number} also has a later active claim {h.first.claim_id} by {h.first.agent}; "
                        "that agent should yield to this PR",
                    )
                )
            if not earlier:
                findings.append(
                    Finding("notice", ref.number, f"#{ref.number} is claimed by {first_mine.first.claim_id} ({first_mine.first.agent})")
                )
        else:
            if foreign_active:
                holder = foreign_active[0].first
                problem = f"#{ref.number} is claimed by another session: {holder.claim_id} ({holder.agent})"
                findings.append(Finding("error", ref.number, f"{problem}; pick another issue or coordinate on #{ref.number}"))
            else:
                problem = f"#{ref.number} has no claim linked to branch {branch!r} or to a 'Claim:' line in the PR body"
                fix = (
                    f"claim it with `{claim_command(ref.number, branch)}`, add 'Claim: <claim-id>' to the PR body, "
                    "then release the claim with --reason pr-opened"
                )
                if maintainer_ok:
                    findings.append(Finding("warning", ref.number, f"{problem}; accepted because a maintainer wrote 'Claim: maintainer'"))
                else:
                    findings.append(Finding("error", ref.number, f"{problem}; {fix}"))

        for other in open_prs:
            other_number = int(other.get("number", 0))
            if other_number == number:
                continue
            if not any(r.number == ref.number and r.closing for r in issue_refs(other.get("body") or "", repo)):
                continue
            other_branch = (other.get("head") or {}).get("ref", "?")
            other_key = (str(other.get("created_at", "")), other_number)
            this_key = (str(pr.get("created_at", "")), number)
            if other_key < this_key:
                findings.append(
                    Finding(
                        "error",
                        ref.number,
                        f"#{ref.number} is already closed by open PR #{other_number} ({other_branch}); "
                        f"close one of the two PRs, or change this PR's reference to 'Part of #{ref.number}'",
                    )
                )
            else:
                findings.append(
                    Finding(
                        "warning",
                        ref.number,
                        f"newer open PR #{other_number} ({other_branch}) also closes #{ref.number}; its check fails until one is closed",
                    )
                )
    return findings


def report_findings(findings: list[Finding], pr_number: int) -> None:
    annotate = os.environ.get("GITHUB_ACTIONS") == "true"
    for finding in findings:
        if annotate and finding.level in {"error", "warning"}:
            print(f"::{finding.level} title=PR #{pr_number} claim check::{finding.message}")
        else:
            print(f"{finding.level}: {finding.message}")


def single(data: Any) -> dict[str, Any]:
    if isinstance(data, list) and len(data) == 1 and isinstance(data[0], dict):
        return data[0]
    if isinstance(data, dict):
        return data
    raise ClaimError("unexpected pull request response from GitHub")


def cmd_check_pr(args: argparse.Namespace) -> int:
    pr = single(gh_api(args.repo, f"pulls/{args.pr}"))
    open_prs = gh_api(args.repo, "pulls?state=open")
    if not isinstance(open_prs, list):
        raise ClaimError("unexpected open pull request response from GitHub")
    refs = issue_refs(pr.get("body") or "", args.repo)
    ledgers = {ref.number: fetch_events(args.repo, ref.number) for ref in refs if ref.closing}

    findings = evaluate_pr(pr, open_prs, ledgers, args.repo)
    if args.json:
        print(json.dumps([finding.__dict__ for finding in findings], indent=2, sort_keys=True))
    else:
        report_findings(findings, int(pr["number"]))
    if any(finding.level == "error" for finding in findings):
        print(
            f"claim check failed for PR #{pr['number']}; see docs/agent-claims.md#pull-request-check",
            file=sys.stderr,
        )
        return CHECK_FAILED
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

    check_pr = subparsers.add_parser(
        "check-pr",
        help="check a PR's Fixes/Closes references against claim ledgers and other open PRs",
    )
    check_pr.add_argument("pr", type=int)
    add_common(check_pr)
    check_pr.set_defaults(func=cmd_check_pr)

    for name, help_text, func in [
        ("claim", "claim an issue", cmd_claim),
        ("heartbeat", "extend an active claim", cmd_heartbeat),
        ("release", "release a claim", cmd_release),
    ]:
        sub = subparsers.add_parser(name, help=help_text)
        sub.add_argument("issue", type=int)
        sub.add_argument("--agent", help="unique agent/session name (or set CV_AGENT)")
        sub.add_argument("--session", help="agent session URL or other resumable reference")
        sub.add_argument(
            "--branch",
            help="PR head branch this claim covers (claim only; defaults to the current git branch, '' to omit)",
        )
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
