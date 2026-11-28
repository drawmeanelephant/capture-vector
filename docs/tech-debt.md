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
| Windows native UI unproven | UI | CI runs the headless tests on `windows-latest` but never drives a window. The WARP boot-companion crash (#31) has a workaround (`1146170`), but no Windows real-window pass is recorded in `spike/FINDINGS.md` | issue #26, issue #90 |
| Linux native UI evidence is narrow | UI | Multi-window and ibus/anthy IME passed on one real X11 desktop (#40, #41). PR #210 migration adoption has one operator-reported scripted native Ubuntu 22.04 X11/llvmpipe run with real pinned Boris/lab binaries in a second VM; only a final-state screenshot is preserved, so artifact-backed reproduction remains needed (see `spike/FINDINGS.md`). CI has no Linux desktop runner, and Wayland, hardware GPUs, distribution differences, native portal dialogs, and physical-machine behavior remain unproven | issue #26, issue #90 |
| IME and rich-text parity | UI | Windows IME is unproven. macOS has US dead keys only, with Japanese conversion and candidate placement unproven. No typed `TextEdit::rich` probe exists | issue #26 |
| PNG goldens canonical only on Lavapipe | Testing | Other rasterizers differ by font hinting and rounding; `rune-ref` is not a drop-in | `fixtures/ui/golden/README.md` |
| `Store` port is journal-append-only | Storage | `cv_store::docs::DiskStore` (issue #272) implements `cv_core::store::Store` on `.cv/store/docs.jsonl` + a rebuildable app-data index, but the only production caller is the workbench's `shell.journal` event journal; smart-mailbox `query` consumers are still unwired | issue #11 |
| Packaging signing and physical-machine verification | Build | Pinned Boris fetch, bundled lookup, release pipeline and sidecar migrations landed. Signing, notarization and 3-OS launch parity need owner decisions | issue #12, `docs/PACKAGING.md` |
| Acceptance-gate close-out | Build | Every gate was verified green; the issue waits on the maintainer's close-out | issue #11 |
| Standard-site implicit session root | Engine boundary | User-controlled custom session verbs could make Boris create or mutate its implicit `$HOME` sessions root. Pinned Boris requires an absolute `--session-root` while Capture Vector accepts only workspace-relative output paths, so the boundary does not compose. No production app wiring exposes those verbs today; this is not a decision on Boris-native v1 publish scope, but needs a hardening decision before `Engine::run` exposes user-controlled custom session verbs | issue #222, round-36 verifier candidate |

Open question for the maintainer:

- Should #11 close now that its gates are verified, so its row can go?

Rules:

- Add an entry when you land a shortcut; remove it when the issue closes.
- Do not use the ledger to file normal bugs; use the issue templates.
- A marker without an issue number fails the pre-commit hook.
