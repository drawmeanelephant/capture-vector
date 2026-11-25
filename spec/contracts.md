# Contracts (v3 — normative)

The shared vocabulary. Wave 0 turns this into compiling Rust across
`cv-core` + `cv-contracts`. All ports are **synchronous / channel-sink**
— `cv-core` has no async, matching rune's `&mut State` + wake-drain
model. Sources of truth for Boris payloads: `docs/contracts/` upstream,
never our imagination.

## Identity (`cv-core::id`)

```rust
pub struct NounId { pub account: AccountId, pub kind: NounKind,
                    pub stable: StableId }     // StableId NEVER a fs path
pub struct NounKind(String);                   // private field: mint only
                                               // via NounKind::register
pub mod noun_kind { pub const PAGE: &str = "page"; pub const FINDING: &str
    = "finding"; pub const PLAN: &str = "plan"; pub const RECEIPT: &str
    = "receipt"; pub const NOTE: &str = "note"; pub const OUTPUT: &str = "output";
    pub const ALL: &[&str] = &[ .. ]; }        // closed minting set
pub struct Selection { pub account: AccountId, pub mailbox: String,
                       pub noun: Option<NounId> }
```

Noun-view registry with a mandatory generic fallback; a test asserts
every produced kind has a registered view.

## Events & dataflow (`cv-core::events`)

```rust
pub struct Envelope { pub workspace: WorkspaceId, pub epoch: SnapshotEpoch,
                      pub build: Option<BuildId>, pub cursor: Cursor,
                      pub event_id: EventId, pub payload: AppEvent }
pub enum AppEvent { Audit(AuditEvent), Engine(EngineEvent), Notes(NotesEvent),
                    Publish(PublishEvent), Shell(ShellEvent), /* per-lane files
                    under cv-core/src/events/, lane-namespaced */ }
```

Rules: commit boundary = event + store mutation + derived-work outbox
atomically; cursor advances **after** commit; derived work idempotent by
`event_id + input_hash`; early-arriving findings buffered per spec.
The cascade paragraph (build-finished → persist IR → rules re-eval →
findings refresh → staleness badges, ordering + replay) is written in
`cv-core` docs before wave 1.

## Determinism (`cv-core::sys`)

```rust
pub trait Clock { fn now(&self) -> Timestamp }        // FixedClock in tests
pub trait IdGen { fn next(&mut self) -> StableId }    // seeded in tests
```

Diagnostics carry `build_version: BuildId` — spans are computed against
that build's bytes; the editor's stale policy is defined, not emergent.

## Engine (`cv-core::engine`) — capability-honest, synchronous

```rust
pub trait Engine {
    fn locate(&self) -> Result<EngineInfo>;              // version, caps probe
    fn capabilities(&self) -> EngineCaps;                // per-OS honesty
    fn snapshot(&self) -> Result<ArtifactSet>;           // disk-truth resync
    fn watch(&self, sink: EventSink) -> Result<WatchHandle>;   // caps-gated
    fn run(&self, cmd: &Verb, sink: EventSink) -> Result<ExitClass>;
    fn write_content(&self, draft: &PageDraft, fp: Fingerprint)
                     -> Result<WriteOutcome>;            // staged+atomic
}
pub struct EngineCaps { pub watch: bool, pub serve: bool,
                        pub authoring: bool, pub writes: bool }
pub enum ExitClass { Ok, ContentFailure, UsageError, IoFailure }  // 0/1/2/3
pub struct Cursor { pub session: SessionId, pub seq: u64 }        // CV-owned
pub struct Fingerprint { pub sha256: Hash, pub mtime: Timestamp } // stolen
                                                                  // from boris-editor
```

`write_content` does a fingerprinted staged write (`*.tmp` + atomic
rename, Windows retry semantics) — **no frontmatter validation in
Rust**; `boris validate` is the judge. `EngineScheduler` serializes
mutating verbs, parallelizes reads, treats a live watch's `dist/` seat
as a held lock.

## Store (`cv-core::store`)

```rust
pub trait Store {
    fn put(&self, ns: Namespace, key: &StableId, doc: &Value) -> Result<()>;
    fn get(&self, ns: Namespace, key: &StableId) -> Result<Option<Value>>;
    fn query(&self, ns: Namespace, pred: &Predicate) -> Result<Vec<Value>>;
}
pub struct Predicate { pub conj: Vec<Term> }   // field op value ONLY —
                                               // no grammar, joins, regex
```

- SQLite = rebuildable index in OS app-data; JSONL in `.cv/` = truth.
- Per-namespace migration tracks `(ns, version)`; no global counter.
- Boris IR is **never** persisted to SQLite — `Arc<GraphDocument>` +
  in-memory `NounAttrRow` scans.

## Audit (`cv-core::rules`)

```rust
pub struct ArtifactSet {               // content-hashed manifest
    pub ir: IrRef, pub html: HtmlRef, pub exports: Vec<ExportRef>,
    pub deploy: TargetDeployProfile,   // base_url, slash policy, robots contract
    pub missing: Vec<ArtifactKind> }   // explicit unavailable inputs
pub struct Rule { pub id: AuditCode, pub severity: Severity,
    pub tier: AuthorityTier,           // NormativeSpec|EngineDocumented|Heuristic
    pub archetypes: Vec<Archetype>,    // trunk|satellite|faq|article|changelog…
    pub check: CheckSpec,              // closed versioned algebra: typed
                                       // selectors + thresholds
    pub remediation: String, pub citation: Citation }
pub struct Finding { pub code: AuditCode, pub tier: AuthorityTier, pub severity: Severity,
    pub status: CheckStatus,           // checked|passed|failed|limited
    pub subject: NounId, pub key: DedupKey,  // (code, subject, hash(target))
    pub evidence: Evidence, pub fix: Option<FixIt> }
```

`DedupKey`'s third component is a hash of the offending target — N
broken links on one page stay N findings; `Heuristic` can never block.

## Publish (`cv-core::publish`)

```rust
pub struct PreflightGrant { pub artifact_set_hash: Hash, pub graph_hash: Hash,
    pub plan_hash: Hash, pub target_cfg_hash: Hash, pub ruleset_hash: Hash,
    pub boris_version: String }        // sealed — publish delivers exactly this
pub enum RunState { Dispatching, Submitted, Unknown, Complete(RunStatus) }
pub enum RunStatus { Complete, Partial, Failed, Incomplete }  // Boris's words
```

Approval persisted **before** dispatch. `Unknown` = crash after contact,
requires reconciliation — never auto-retry. Preflight builds the
**target profile** into its own output dir (never the watch build).

## Boris mirrors (`cv-contracts::boris`)

Serde-only mirrors, version-keyed, `serde(default)` tolerance on
additive fields; unknown variants decode to typed `Unknown` — never
crash on drift. Fixture-decode tests are the entire suite; fixtures are
real captures only (hand-authored Boris JSON is banned).

## Migration-lab mirrors (`cv-contracts::lab`)

Mirrors of `boris-migration-lab` output, keyed by `(format, schema_version)`
per document role (`report.json`, `inventory.json`, `provenance/<id>.json`,
`provenance_manifest.json`, and the mode-specific manifests). One format id
names a family, so the file's role picks the mirror. Additive fields under
a known version decode; an unknown format or version is a typed
`DecodeError`, never a guess. The shared conversion class
(`exact`/`transformed`/`unsupported`/`human_review`) is `Disposition`; a
value outside it decodes to `Unknown`. Fixtures are pinned-lab captures in
`fixtures/captures/lab/` (`scripts/capture-lab-fixtures.sh`), replayed by
`cv_fixtures::StubLab`.

## Wave-0 must also pin down

- Diagnostic span units (byte vs char vs UTF-16; 0/1-based; CRLF).
- Per-window vs shared `Selection` scope + dirty-close semantics.
- Scale envelope numbers (pages, diagnostics/sec, artifacts).
- The `cv-core` event-cascade paragraph.
