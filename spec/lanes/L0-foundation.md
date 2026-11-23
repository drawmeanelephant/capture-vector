# L0 — Foundation / Wave 0 (the only serializer)

> Worker brief. You land first and stay thin — but "thin" changed in v3:
> fixtures are now *real captures from the pinned binary*, which is the
> slowest deliverable. Stage accordingly: contracts + StubEngine +
> `happy/` first, the rest of the captured corpora before wave-1 gate.

## Owns

- `Cargo.toml` (workspace), `rust-toolchain.toml`, `crates/cv-core/`,
  `crates/cv-contracts/`, `crates/cv-fixtures/`, `fixtures/`,
  `.github/workflows/`, `xtask/`, `spec/UPGRADES.md`

## Deliverables

1. **Workspace skeleton** per `spec/architecture.md` v3 crate map
   (~13 crates). `cv-core`/`cv-contracts` depend on nothing
   in-workspace. `cargo-deny` config enforcing dependency discipline.
2. **Contract stubs** — everything in `spec/contracts.md` v3: NounId/
   NounKind/NounAttrRow, Envelope + lane-namespaced AppEvent, Clock/
   IdGen, sync Engine capability ports, Store, CheckSpec/ArtifactSet/
   Finding/DedupKey, PreflightGrant/RunState, Cursor/Fingerprint.
3. **Real fixture capture** — scripted capture against the pinned boris
   for ALL corpora (`happy/`, `broken-frontmatter/`, `broken-parent/`,
   `broken-wikilink/`, `broken-duplicate-id/`, `check`, `plan`, publish
   reports, watch NDJSON sessions). **Hand-authored Boris JSON is
   banned.** `fixtures/MANIFEST.toml` (per-corpus hash + boris version +
   capture command) + fixture-format `schema_version`.
4. **`StubEngine`/`StubStore`** replaying fixtures; the conformance
   harness (`Stub ≡ Process`) scaffolded so L1 fills it.
5. **Rune capability probe** — headless multi-window, 5k-row virtual
   list, `TextEdit::rich` decorations + IME on 3 OSes; results recorded.
   Probe failures change wave-1 scope, they're not swept under.
6. **Canary vertical slice** — one `StubEngine` `BuildFinished` →
   cascade → `cv-store` write → wake → element-tree redraw, scripted
   headless. Proves the async pattern every lane will copy.
7. **CI**: build+test matrix ×3 OSes, `CAPTURE_VECTOR_BORIS_BIN` unset;
   element-tree goldens gating all 3; PNG on canonical Lavapipe config;
   fmt + clippy deny(warnings).
8. **Empty shell**: module rail + DemoModule through `cv-model`/`cv-widgets`
   rendering a fixture mailbox headless; degraded-state goldens.
9. **Wave-0 contract pins**: diagnostic span units, selection scope per
   window, scale-envelope numbers, the event-cascade paragraph in
   `cv-core` docs.

## Done when

- `cargo test --workspace` green on 3 OSes, no boris, no display, no net.
- Canary slice + capability probe results committed.
- `MANIFEST.toml` covers every corpus; `xtask fixtures refresh` works.
- Deleting `crates/cv-app` keeps the workspace test-green.

## Must not

- Author fixture JSON by hand — capture or it doesn't exist.
- Implement lane behavior — stubs only.
- Let lanes dep on each other (cargo-deny enforces).
