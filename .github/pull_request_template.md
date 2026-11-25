## Summary

What changed and which lane/signal it serves.

## Why

The problem or requirement this addresses. Link the issue when one
exists (`Closes #123`).

Claim: claim-<id from scripts/agent-claim.py claim>

## Testing

- [ ] `cargo test --workspace --locked`
- [ ] `cargo fmt --all -- --check`
- [ ] `cargo clippy --workspace --all-targets --no-deps -- -D warnings`
- [ ] `./scripts/check-hygiene.sh`
- [ ] UI change: attached the `--tree` diff or screenshots
- [ ] Fixture change: regenerated with `scripts/capture-fixtures.sh`

## Risk and rollback

What can break, who is affected, and how to revert (usually
`git revert <sha>`). Note Windows/Linux if the change is OS-specific.

## Checklist

- [ ] No secrets, page bodies, or personal data in logs, fixtures, or
      committed files.
- [ ] AGENTS.md / README updated if commands, env vars, or conventions
      changed.
- [ ] New behavior has a test (unit or integration) that fails without
      the change.
- [ ] If this closes an issue, the `Claim:` line above names its claim
      and the claim was released with reason `pr-opened` (see
      `docs/agent-claims.md`). Maintainers skipping the claim write
      `Claim: maintainer` instead.
