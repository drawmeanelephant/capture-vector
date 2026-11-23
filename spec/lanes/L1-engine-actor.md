# L1 — Engine (`cv-engine`)

> Worker brief. You own the only place processes exist — RawSubprocess
> on all three OSes. `boris-editor` (upstream host) was evaluated and
> rejected for v1; its proven mechanics are yours to reimplement in our
> contract, not to wrap.

## Owns

- `crates/cv-engine/`, `fixtures/` capture commands + `MANIFEST.toml`

## Depends on

- L0 contracts (`cv-core::engine` capability ports, `Cursor`,
  `Fingerprint`, `Envelope`), `cv-fixtures`.

## Deliverables

1. **Locate + gate**: `CV_BORIS_BIN` → bundled → PATH → sibling;
   `boris --version` gate + `--build-info` recorded to ledger. Mismatch
   → typed error surfaced to Doctor, never silent.
2. **Capability probe** per OS at startup: watch supported? serve?
   authoring? Windows falls back to one-shot builds + file watcher —
   probed, recorded, surfaced; not assumed.
3. **EngineScheduler** per workspace: serialize mutating verbs,
   parallelize reads, one watch session; a live watch's `dist/` seat is
   a held lock — mutating verbs stop-watch first or use a separate
   output dir.
4. **Watch session**: `boris watch --watch-json` — NDJSON on **stderr**,
   `hello` + `watch_events_schema==1` handshake or kill child; malformed
   lines handled explicitly; `broadcast` Lagged → snapshot resync.
5. **CV-owned cursor** `(engine_session_id, seq)`; session change or gap
   → `Engine::snapshot()` (re-read `.boris/` + last reports = disk
   truth). Bounded internal ring (host's 100-event cap taught us why).
6. **Fingerprinted staged writes** (`SHA-256`+mtime check, `*.tmp`+
   rename, Windows sharing-violation retry). `ContentWriter` never
   validates frontmatter — `boris validate` judges.
7. **Exit mapping** `0/1/2/3` → ExitClass; stderr captured verbatim into
   Activity events. Cancellation kills child + process group.
8. **Secrets channel**: stdin-only buffers for `nostr sign --key-stdin`;
   API cannot accept a key as a plain arg.
9. **Tests**: fake-boris shim emitting recorded NDJSON (POSIX + .bat);
   `Stub ≡ Process` conformance suite on same streams; resync test:
   kill watch mid-session → snapshot+cursor converges, no ghosts.

## Done when

- `cargo test -p cv-engine` green, no real binary.
- Conformance suite green vs shim on all 3 OSes.
- Live smoke `#[ignore]` passes with real pinned boris when bin set.
- Resync test proves no ghost diagnostics after restart.

## Must not

- Spawn anything outside this crate. Reimplement Boris semantics.
  Read secrets from env/argv/files. Tolerate a malformed line silently.
