#!/usr/bin/env python3
"""Unit tests for scripts/agent-claim.py's pure ledger and PR-check logic."""

from __future__ import annotations

import importlib.util
import sys
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
MODULE_PATH = ROOT / "scripts" / "agent-claim.py"

spec = importlib.util.spec_from_file_location("agent_claim", MODULE_PATH)
if spec is None or spec.loader is None:
    raise RuntimeError(f"cannot load {MODULE_PATH}")
agent_claim = importlib.util.module_from_spec(spec)
sys.modules["agent_claim"] = agent_claim
spec.loader.exec_module(agent_claim)


def comment(
    comment_id: int,
    created_at: str,
    record: dict,
    author_login: str = "agent-actor",
    author_association: str = "CONTRIBUTOR",
) -> dict:
    return {
        "id": comment_id,
        "created_at": created_at,
        "body": agent_claim.claim_body(record),
        "user": {"login": author_login},
        "author_association": author_association,
    }


def record(event: str, claim_id: str, expires_at: str, agent: str = "agent-a") -> dict:
    return {
        "cv_claim": 1,
        "event": event,
        "id": claim_id,
        "agent": agent,
        "scope": "implementation",
        "expires_at": expires_at,
    }


class ClaimLedgerTests(unittest.TestCase):
    def test_claim_body_round_trips(self) -> None:
        events = agent_claim.parse_claim_events(
            [comment(7, "2026-01-01T00:00:00Z", record("claim", "claim-a", "2026-01-01T04:00:00Z"))]
        )

        self.assertEqual(len(events), 1)
        self.assertEqual(events[0].claim_id, "claim-a")
        self.assertEqual(events[0].comment_id, 7)

    def test_claim_is_active_only_before_expiry(self) -> None:
        events = agent_claim.parse_claim_events(
            [comment(1, "2026-01-01T00:00:00Z", record("claim", "claim-a", "2026-01-01T04:00:00Z"))]
        )

        before = agent_claim.parse_time("2026-01-01T03:59:59Z")
        after = agent_claim.parse_time("2026-01-01T04:00:00Z")
        self.assertEqual(len(agent_claim.active_claims(events, before)), 1)
        self.assertEqual(agent_claim.active_claims(events, after), [])

    def test_heartbeat_extends_and_release_closes(self) -> None:
        comments = [
            comment(1, "2026-01-01T00:00:00Z", record("claim", "claim-a", "2026-01-01T04:00:00Z")),
            comment(2, "2026-01-01T01:00:00Z", record("heartbeat", "claim-a", "2026-01-01T05:00:00Z")),
        ]
        events = agent_claim.parse_claim_events(comments)
        at_four_thirty = agent_claim.parse_time("2026-01-01T04:30:00Z")
        self.assertEqual(len(agent_claim.active_claims(events, at_four_thirty)), 1)

        comments.append(
            comment(3, "2026-01-01T02:00:00Z", record("release", "claim-a", "2026-01-01T05:00:00Z"))
        )
        events = agent_claim.parse_claim_events(comments)
        self.assertEqual(agent_claim.active_claims(events, at_four_thirty), [])

        comments.append(
            comment(4, "2026-01-01T02:30:00Z", record("heartbeat", "claim-a", "2026-01-01T06:00:00Z"))
        )
        events = agent_claim.parse_claim_events(comments)
        self.assertEqual(agent_claim.active_claims(events, at_four_thirty), [])

    def test_earliest_comment_wins_a_race(self) -> None:
        events = agent_claim.parse_claim_events(
            [
                comment(20, "2026-01-01T00:01:00Z", record("claim", "claim-b", "2026-01-01T04:00:00Z", "agent-b")),
                comment(10, "2026-01-01T00:00:00Z", record("claim", "claim-a", "2026-01-01T04:00:00Z", "agent-a")),
            ]
        )

        active = agent_claim.active_claims(events, agent_claim.parse_time("2026-01-01T01:00:00Z"))
        self.assertEqual([event.claim_id for event in active], ["claim-a", "claim-b"])

    def test_heartbeat_does_not_change_original_claim_priority(self) -> None:
        events = agent_claim.parse_claim_events(
            [
                comment(1, "2026-01-01T00:00:00Z", record("claim", "claim-a", "2026-01-01T04:00:00Z", "agent-a")),
                comment(2, "2026-01-01T00:01:00Z", record("claim", "claim-b", "2026-01-01T04:00:00Z", "agent-b")),
                comment(3, "2026-01-01T00:02:00Z", record("heartbeat", "claim-a", "2026-01-01T05:00:00Z", "agent-a")),
            ]
        )

        active = agent_claim.active_claims(events, agent_claim.parse_time("2026-01-01T01:00:00Z"))
        self.assertEqual([event.claim_id for event in active], ["claim-a", "claim-b"])

    def test_non_author_cannot_heartbeat_or_release_another_agents_claim(self) -> None:
        events = agent_claim.parse_claim_events(
            [
                comment(1, "2026-01-01T00:00:00Z", record("claim", "claim-a", "2026-01-01T04:00:00Z"), "agent-actor"),
                comment(2, "2026-01-01T00:01:00Z", record("release", "claim-a", "2026-01-01T04:00:00Z"), "intruder"),
                comment(3, "2026-01-01T00:02:00Z", record("heartbeat", "claim-a", "2026-01-01T05:00:00Z"), "intruder"),
            ]
        )

        active = agent_claim.active_claims(events, agent_claim.parse_time("2026-01-01T01:00:00Z"))
        self.assertEqual([event.claim_id for event in active], ["claim-a"])
        self.assertEqual(active[0].record["expires_at"], "2026-01-01T04:00:00Z")

    def test_maintainer_can_release_another_agents_claim(self) -> None:
        events = agent_claim.parse_claim_events(
            [
                comment(1, "2026-01-01T00:00:00Z", record("claim", "claim-a", "2026-01-01T04:00:00Z"), "agent-actor"),
                comment(
                    2,
                    "2026-01-01T00:01:00Z",
                    record("release", "claim-a", "2026-01-01T04:00:00Z"),
                    "maintainer",
                    "OWNER",
                ),
            ]
        )

        active = agent_claim.active_claims(events, agent_claim.parse_time("2026-01-01T01:00:00Z"))
        self.assertEqual(active, [])

    def test_malformed_and_non_claim_comments_are_ignored(self) -> None:
        comments = [
            {"id": 1, "created_at": "2026-01-01T00:00:00Z", "body": "ordinary comment"},
            {
                "id": 2,
                "created_at": "2026-01-01T00:01:00Z",
                "body": "<!-- capture-vector-claim:v1 -->\n```json\n{bad json\n```",
            },
        ]

        self.assertEqual(agent_claim.parse_claim_events(comments), [])


def claim_record(claim_id: str, expires_at: str, agent: str = "agent-a", **extra: str) -> dict:
    return {**record("claim", claim_id, expires_at, agent), **extra}


def pull(number: int, branch: str, body: str, created_at: str, association: str = "OWNER") -> dict:
    return {
        "number": number,
        "head": {"ref": branch},
        "body": body,
        "created_at": created_at,
        "author_association": association,
    }


REPO = "drawmeanelephant/capture-vector"
NOW = agent_claim.parse_time("2026-01-01T01:00:00Z")


def check(pr: dict, open_prs: list[dict], ledger_comments: dict[int, list[dict]]) -> list:
    ledgers = {issue: agent_claim.parse_claim_events(items) for issue, items in ledger_comments.items()}
    return agent_claim.evaluate_pr(pr, open_prs, ledgers, REPO, NOW)


def levels(findings: list) -> list[str]:
    return [finding.level for finding in findings]


def errors(findings: list) -> list[str]:
    return [finding.message for finding in findings if finding.level == "error"]


class PullRequestCheckTests(unittest.TestCase):
    def test_only_keyword_references_to_this_repo_are_checked(self) -> None:
        refs = agent_claim.issue_refs(
            "Fixes #56. Closes: #57, resolved #58. Part of #42. Stacked on #153. "
            "Fixes other/repo#9. Fixes drawmeanelephant/capture-vector#60.",
            REPO,
        )

        self.assertEqual(
            [(ref.number, ref.closing) for ref in refs],
            [(42, False), (56, True), (57, True), (58, True), (60, True)],
        )

    def test_unclaimed_issue_fails_with_the_claim_command(self) -> None:
        pr = pull(10, "feat/issue-56-editor", "Fixes #56.", "2026-01-01T00:30:00Z")

        failures = errors(check(pr, [pr], {56: []}))

        self.assertEqual(len(failures), 1)
        self.assertIn(
            "python3 scripts/agent-claim.py claim 56 --agent <unique-agent-id> --branch feat/issue-56-editor",
            failures[0],
        )

    def test_claim_released_on_pr_open_passes_when_session_names_the_branch(self) -> None:
        pr = pull(10, "feat/issue-157-dialog-layer", "Fixes #157", "2026-01-01T00:30:00Z")
        claim = claim_record(
            "claim-a", "2026-01-01T04:00:00Z", session="factory-droid worktree feat/issue-157-dialog-layer"
        )
        released = {**record("release", "claim-a", "2026-01-01T04:00:00Z"), "reason": "pr-opened"}

        findings = check(
            pr,
            [pr],
            {157: [comment(1, "2026-01-01T00:00:00Z", claim), comment(2, "2026-01-01T00:31:00Z", released)]},
        )

        self.assertEqual(errors(findings), [])

    def test_branch_field_or_claim_line_links_a_claim(self) -> None:
        by_branch = claim_record("claim-a", "2026-01-01T04:00:00Z", branch="fix/x")
        pr = pull(10, "fix/x", "Fixes #5", "2026-01-01T00:30:00Z")
        self.assertEqual(errors(check(pr, [pr], {5: [comment(1, "2026-01-01T00:00:00Z", by_branch)]})), [])

        by_id = claim_record("claim-b", "2026-01-01T04:00:00Z")
        pr = pull(11, "renamed-branch", "Fixes #6\n\nClaim: claim-b", "2026-01-01T00:30:00Z")
        self.assertEqual(errors(check(pr, [pr], {6: [comment(1, "2026-01-01T00:00:00Z", by_id)]})), [])

    def test_branch_must_match_as_a_whole_token(self) -> None:
        claim = claim_record("claim-a", "2026-01-01T04:00:00Z", session="worktree feat/issue-56-editor-chrome")
        pr = pull(10, "feat/issue-56-editor", "Fixes #56", "2026-01-01T00:30:00Z")

        self.assertEqual(len(errors(check(pr, [pr], {56: [comment(1, "2026-01-01T00:00:00Z", claim)]}))), 1)

    def test_issue_claimed_by_another_session_fails(self) -> None:
        claim = claim_record("claim-a", "2026-01-01T04:00:00Z", "agent-other", branch="feat/other")
        pr = pull(10, "feat/mine", "Fixes #7", "2026-01-01T00:30:00Z")

        failures = errors(check(pr, [pr], {7: [comment(1, "2026-01-01T00:00:00Z", claim)]}))

        self.assertEqual(len(failures), 1)
        self.assertIn("claimed by another session: claim-a (agent-other)", failures[0])

    def test_claim_that_lost_a_race_does_not_count(self) -> None:
        winner = claim_record("claim-a", "2026-01-01T04:00:00Z", "agent-a", branch="feat/a")
        loser = claim_record("claim-b", "2026-01-01T04:00:00Z", "agent-b", branch="feat/b")
        yielded = {**record("release", "claim-b", "2026-01-01T04:00:00Z", "agent-b"), "reason": "lost-race"}
        pr = pull(10, "feat/b", "Fixes #8", "2026-01-01T00:30:00Z")

        failures = errors(
            check(
                pr,
                [pr],
                {
                    8: [
                        comment(1, "2026-01-01T00:00:00Z", winner),
                        comment(2, "2026-01-01T00:00:01Z", loser),
                        comment(3, "2026-01-01T00:00:02Z", yielded),
                    ]
                },
            )
        )

        self.assertEqual(len(failures), 1)
        self.assertIn("claim-a", failures[0])

    def test_unyielded_race_loser_fails_and_winner_is_warned(self) -> None:
        winner = claim_record("claim-a", "2026-01-01T04:00:00Z", "agent-a", branch="feat/a")
        loser = claim_record("claim-b", "2026-01-01T04:00:00Z", "agent-b", branch="feat/b")
        ledger = {9: [comment(1, "2026-01-01T00:00:00Z", winner), comment(2, "2026-01-01T00:00:01Z", loser)]}

        lose_pr = pull(11, "feat/b", "Fixes #9", "2026-01-01T00:30:00Z")
        self.assertIn("earlier active claim claim-a", errors(check(lose_pr, [lose_pr], ledger))[0])

        win_pr = pull(10, "feat/a", "Fixes #9", "2026-01-01T00:20:00Z")
        findings = check(win_pr, [win_pr], ledger)
        self.assertEqual(errors(findings), [])
        self.assertIn("warning", levels(findings))

    def test_second_open_pr_closing_the_same_issue_fails(self) -> None:
        claim_a = claim_record("claim-a", "2026-01-01T04:00:00Z", session="worktree feat/issue-56-editor")
        claim_b = claim_record("claim-b", "2026-01-01T04:00:00Z", session="worktree feat/issue-56-editor-chrome")
        ledger = {56: [comment(1, "2026-01-01T00:00:00Z", claim_a), comment(2, "2026-01-01T00:10:00Z", claim_b)]}
        first = pull(155, "feat/issue-56-editor", "Closes #56. Part of #42.", "2026-01-01T00:20:00Z")
        second = pull(156, "feat/issue-56-editor-chrome", "Fixes #56.", "2026-01-01T00:40:00Z")

        second_failures = errors(check(second, [first, second], ledger))
        self.assertTrue(any("already closed by open PR #155" in message for message in second_failures))

        first_findings = check(first, [first, second], ledger)
        self.assertFalse(any("PR #156" in message for message in errors(first_findings)))
        self.assertTrue(any("newer open PR #156" in f.message for f in first_findings if f.level == "warning"))

    def test_part_of_references_are_not_claim_checked_or_exclusive(self) -> None:
        first = pull(1, "feat/a", "Part of #42", "2026-01-01T00:20:00Z")
        second = pull(2, "feat/b", "Part of #42", "2026-01-01T00:40:00Z")

        self.assertEqual(errors(check(second, [first, second], {})), [])

    def test_maintainer_opt_out_downgrades_only_a_missing_claim(self) -> None:
        body = "Fixes #12\n\nClaim: maintainer"
        owner_pr = pull(10, "maint/fix", body, "2026-01-01T00:30:00Z", "OWNER")
        self.assertEqual(errors(check(owner_pr, [owner_pr], {12: []})), [])

        outside_pr = pull(11, "contrib/fix", body, "2026-01-01T00:30:00Z", "CONTRIBUTOR")
        self.assertEqual(len(errors(check(outside_pr, [outside_pr], {12: []}))), 1)

        held = claim_record("claim-a", "2026-01-01T04:00:00Z", "agent-other", branch="feat/other")
        self.assertEqual(
            len(errors(check(owner_pr, [owner_pr], {12: [comment(1, "2026-01-01T00:00:00Z", held)]}))), 1
        )

    def test_pr_without_issue_references_passes(self) -> None:
        pr = pull(10, "dependabot/cargo/x", "Bumps x from 1 to 2. See #3.", "2026-01-01T00:30:00Z")

        self.assertEqual(levels(check(pr, [pr], {})), ["notice"])

    def test_explicit_empty_branch_omits_the_branch_field(self) -> None:
        self.assertIsNone(agent_claim.claim_branch(""))
        self.assertEqual(agent_claim.claim_branch("feat/x"), "feat/x")


if __name__ == "__main__":
    unittest.main()
