#!/usr/bin/env python3
"""Unit tests for scripts/agent-claim.py's pure ledger logic."""

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


def comment(comment_id: int, created_at: str, record: dict) -> dict:
    return {
        "id": comment_id,
        "created_at": created_at,
        "body": agent_claim.claim_body(record),
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


if __name__ == "__main__":
    unittest.main()
