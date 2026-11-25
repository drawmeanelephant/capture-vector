# Agent issue claims

Capture Vector is built by parallel lanes and often by parallel agents.
The issue claim protocol prevents two agents from implementing the same
GitHub issue at the same time without hiding work in chat, branches, or a
local database.

## Rules

1. **Issue comments are the ledger.** A claim exists only as a
   structured `capture-vector-claim:v1` comment on the issue. Labels are
   a visual mirror, branch names are not claims, and "I am looking at
   this" in a chat session is not a claim.
2. **One active implementation claim per issue.** Keep issues small;
   split work instead of making overlapping scoped claims.
3. **Claims are leases, not ownership.** The default lease is 4 hours and
   the maximum is 24 hours. Heartbeat before expiry while actively
   working. An expired claim does not block anyone.
4. **The earliest active claim wins.** GitHub has no compare-and-swap
   operation for issues, so claiming is optimistic: post the claim,
   re-read the comments, and yield if an earlier active claim appeared.
5. **Release early.** Release the claim when the PR is opened, when work
   is blocked on a human, or when stopping for any other reason. The PR
   itself is the visible coordination point after that.
6. **Do not claim ungrounded bug reports for implementation.** Issues
   labeled `needs-info` may be claimed only for triage/grounding.
7. **Claims do not reserve files.** Before touching shared contracts
   (`cv-core`, `cv-contracts`, `spec/architecture.md`, root build files),
   check open PRs and say that scope in the claim. Contract changes land
   as their own PR first.
8. **Maintainers can override.** A maintainer may release a stale or
   incorrect claim before expiry by posting a release event with the
   claim id and reason. Do not edit or delete claim history.

## Commands

Use the helper from the repository root. It requires the GitHub CLI
(`gh`) authenticated for `drawmeanelephant/capture-vector`.

```sh
# Check whether an issue is free.
python3 scripts/agent-claim.py status 42

# Claim it for four hours. Use a unique agent/session name.
python3 scripts/agent-claim.py claim 42 \
  --agent devin-38da6fa2d0f54f30b0b81a293a5bcce3 \
  --session https://app.devin.ai/sessions/38da6fa2d0f54f30b0b81a293a5bcce3

# Extend the lease while still working.
python3 scripts/agent-claim.py heartbeat 42 \
  --claim-id claim-20261124T233000Z-abcdef12 \
  --agent devin-38da6fa2d0f54f30b0b81a293a5bcce3

# Release it when a PR is open or work stops.
python3 scripts/agent-claim.py release 42 \
  --claim-id claim-20261124T233000Z-abcdef12 \
  --agent devin-38da6fa2d0f54f30b0b81a293a5bcce3 \
  --reason pr-opened

# Scan all open issues for active claims.
python3 scripts/agent-claim.py scan
```

Set `CV_AGENT` to a unique agent/session name to omit `--agent`. Use a
name another agent cannot accidentally reuse; a Devin session id or a
human-maintainer handle plus machine name is good.

The script prints non-zero statuses an agent can act on:

| Exit | Meaning | Agent behavior |
|---:|---|---|
| 0 | claim/state changed successfully | continue |
| 2 | active claim already exists | pick another issue |
| 3 | simultaneous claim race lost | yield; do not keep working |

## Manual claim format

Agents with native GitHub issue tools can post the same structure without
the script. Humans should almost always use the script.

````markdown
<!-- capture-vector-claim:v1 -->
```json
{
  "cv_claim": 1,
  "event": "claim",
  "id": "claim-20261124T233000Z-abcdef12",
  "agent": "devin-38da6fa2d0f54f30b0b81a293a5bcce3",
  "scope": "implementation",
  "expires_at": "2026-11-25T03:30:00Z",
  "session": "https://app.devin.ai/sessions/38da6fa2d0f54f30b0b81a293a5bcce3"
}
```

Claiming this issue for **implementation** until `2026-11-25T03:30:00Z`.
````

Events are `claim`, `heartbeat`, and `release`. A heartbeat or release
uses the original `id` and a fresh `expires_at`/`reason` as appropriate.
Timestamps are UTC RFC 3339.

## Labels

`agent: claimed` is a convenience mirror so maintainers can scan the
issue list. The script adds it after a successful claim and removes it
when the last active claim is released. Never treat the label as proof:
stale labels are possible, while the comment ledger is authoritative.

## Failure handling

- If `gh` is missing or unauthenticated, stop before implementation or
  post the manual claim through whatever authenticated GitHub tool is
  available. Do not skip the ledger.
- If the claim command succeeds but the issue later shows an earlier
  active claim, yield even if local edits exist. Push useful work to a
  draft branch or attach it to the winning agent's issue discussion.
- If a session dies, its lease expires on its own. Maintainers may
  release it early after checking that no active PR or session is
  actually carrying the work.
