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
   itself is the visible coordination point after that. CI checks that
   the PR is linked to a claim; see [Pull request check](#pull-request-check).
6. **Do not claim ungrounded bug reports for implementation.** Issues
   labeled `needs-info` may be claimed only for triage/grounding.
7. **Claims do not reserve files.** Before touching shared contracts
   (`cv-core`, `cv-contracts`, `spec/architecture.md`, root build files),
   check open PRs and say that scope in the claim. Contract changes land
   as their own PR first.
8. **Maintainers can override.** A maintainer may release a stale or
   incorrect claim before expiry by posting a release event with the
   claim id and reason. Do not edit or delete claim history.
9. **Only the claiming GitHub actor can extend or release a claim.**
   Heartbeats and releases from other comment authors are ignored unless
   the author is a repository owner, member, or collaborator. This keeps
   random issue commenters from cancelling or extending someone else's
   lease while preserving maintainer override.

## Commands

Use the helper from the repository root. It requires the GitHub CLI
(`gh`) authenticated for `drawmeanelephant/capture-vector`.

```sh
# Check whether an issue is free.
python3 scripts/agent-claim.py status 42

# Claim it for four hours. Use a unique agent/session name. Run it on
# the PR branch so the claim records it (or pass --branch).
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

# Run the pull request check locally (read-only).
python3 scripts/agent-claim.py check-pr 155
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
| 4 | `check-pr` found a claim problem | run the printed fix |

## Pull request check

The `pr-claims` workflow (`.github/workflows/pr-claims.yml`) runs
`python3 scripts/agent-claim.py check-pr <pr>` on every pull request when
it is opened, edited, reopened, or pushed. It reads only the PR, the open
PR list, and the referenced issues' comments, with a read-only token.

The check reads issue references from the PR body:

- **Closing references** (`Fixes #N`, `Closes #N`, `Resolves #N`, and
  their tense variants) are enforced.
- **`Part of #N`** references produce a notice only. Tracking issues and
  epics are shared by many PRs on purpose, so claim the child issue
  instead.
- Bare `#N` mentions are prose and are ignored. A PR with no references
  passes.

For each closing reference, the check fails when any of these is true:

1. **No claim is linked to this PR.** A claim is linked when its `branch`
   field equals the PR head branch, when its `session` or `scope` names
   the head branch as a whole word, or when the PR body names its id on a
   `Claim: claim-…` line. The claim may be active or released, because
   rule 5 releases claims when the PR opens. A claim released with reason
   `lost-race` does not count.
2. **Another session holds the issue.** An active claim that is not
   linked to this PR fails the check when it is earlier than this PR's
   claim, or when this PR has no claim. When this PR's claim is earlier,
   the later claim produces a warning, because that agent should yield.
3. **An older open PR already closes the issue.** Two open PRs that both
   close the same issue fail the newer one, ordered by creation time. The
   older PR gets a warning, so the PR that followed the protocol stays
   green. Resolve the conflict by closing one PR or by changing the
   newer one to `Part of #N`.

Each failure prints the command that fixes it, for example:

```sh
python3 scripts/agent-claim.py claim 56 --agent <unique-agent-id> --branch feat/issue-56-editor
```

After you claim, add `Claim: <claim-id>` to the PR body, which reruns the
check, and then release the claim with `--reason pr-opened`.

`claim` records the current git branch in the `branch` field unless the
branch is `main`, `master`, or a detached `HEAD`. Pass `--branch <name>`
when you claim from another checkout, or `--branch ''` to omit it. Claim
from the branch you will open the PR from, so that the link is
automatic.

### Maintainer PRs

Agents in this repository usually push and open PRs with a maintainer's
GitHub credentials. Author association therefore cannot tell a human
from an agent, and the check does not exempt maintainers automatically.
A maintainer who deliberately skips the claim writes `Claim: maintainer`
in the PR body. If the PR author is a repository owner, member, or
collaborator, a missing claim becomes a warning. The opt-out never
overrides rules 2 or 3.

The check is cooperative, like the rest of the protocol. It runs from the
PR's own checkout, so a PR that edits `scripts/agent-claim.py` changes
the check it runs. Reviewers should read changes to that script.

`check-pr` exits 0 when the check passes, 4 when it fails, and 1 when it
cannot read GitHub.
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
  "session": "https://app.devin.ai/sessions/38da6fa2d0f54f30b0b81a293a5bcce3",
  "branch": "feat/issue-42-example"
}
```

Claiming this issue for **implementation** until `2026-11-25T03:30:00Z`.
````

`session` and `branch` are optional, but the pull request check needs
`branch` (or the branch named in `session`) to link the claim to the PR.
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
