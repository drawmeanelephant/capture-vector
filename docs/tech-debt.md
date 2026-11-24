# Tech-debt ledger

This is the single place to record deliberate shortcuts. Code markers must
link to an issue, and `scripts/check-todos.sh` rejects a bare
`TODO`/`FIXME`/`HACK`/`XXX` in source.

Marker format:

```rust
// TODO(#123): replace the fixture replay with the process engine.
```

Ledger format: one row per item, reviewed when the linked issue closes.

| Item | Area | Why it is deferred | Tracking |
|---|---|---|---|
| Windows native UI unproven | UI | Rune/WARP needs a real Windows run; CI builds but does not drive the window | issue #31, issue #26 |
| Linux native UI unproven | UI | No Linux desktop runner in the current matrix | issue #26 |
| IME/rich-text parity | UI | Needs a real window per OS | issue #26 |
| PNG goldens canonical only on Lavapipe | Testing | Other rasterizers differ by font hinting and rounding | `fixtures/ui/golden/README.md` |
| `cv-store` is a stub | Storage | Store lands with the lanes that need it; unused deps removed until then | `crates/cv-store/src/lib.rs` |
| Packaging/signing/distribution | Build | Not started; pinned Boris fetch exists | issue #12 |
| Acceptance-gate wiring | Build | Gates exist but are not yet all enforced in one place | issue #11 |

Rules:

- Add an entry when you land a shortcut; remove it when the issue closes.
- Do not use the ledger to file normal bugs; use the issue templates.
- A marker without an issue number fails the pre-commit hook.
