# Round 3 — Kimi K3.1 Pro review of PROMPT-v2

## Attacks
1. OLD SPEC KIT STILL NORMATIVE: README routes through v1 architecture;
   L0 + contracts.md still mandate old crate map AND the Rust
   frontmatter validation v2 bans -> rewrite/quarantine kit before
   fan-out, don't just add v2.
2. Dataflow delegates the hardest decision: needs concrete event
   envelope {workspace, snapshot_epoch, build_id, cursor, event_id} and
   ATOMIC commit boundary (event + store mutation + derived-work outbox
   committed together; cursor advances only after commit). Derived work
   idempotent by event_id + input_hash; explicit buffering/eviction for
   findings arriving before IR.
3. Capability-honest engine contract: host and raw aren't equivalent
   (watch/recovery/authoring/cursor differ; host watch unsupported on
   Windows) -> expose capability ports (watch, authoring, writes,
   commands); raw recovery = snapshot + full resync; conformance suite
   vs Stub AND Raw AND Host; live adapter probe in wave 0.
4. CheckSpec has no executable semantics: define closed versioned
   algebra w/ typed selectors + thresholds; ArtifactSet = content-hashed
   manifest {IR, final HTML, exports, target/profile context,
   missing-input states}; canonical/robots/JSON-LD need FINAL HTML not
   IR; unsupported checks -> `limited`, never silent pass.
5. PreflightGrant: bind report to artifact_set_hash + frozen_graph_hash
   + plan_hash + target_config_hash + ruleset_hash + boris_version;
   publish exactly that sealed plan; persist approval BEFORE dispatch;
   states dispatching -> submitted | unknown | complete; crash after
   network contact = `unknown` requiring reconciliation, never
   auto-retry.

## Rulings
No reversal on: NounKind, rusqlite, single cv-ui, no-webview split,
contract-hardening wave. Necessary amendment = capability-honest engine
contracts + earlier live-adapter verification.
