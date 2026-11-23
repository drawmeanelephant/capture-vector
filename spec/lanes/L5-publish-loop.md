# L5 — Publish Loop (`cv-publish` + `cv-ledger` + Publish module)

> Worker brief. Publishing back to the engines is the point. Every run
> is a first-class noun with a sealed grant and honest crash semantics.

## Owns

- `crates/cv-publish/`, `crates/cv-ledger/`,
  `crates/cv-model/src/modules/publish/` + surfaces dir,
  `fixtures/publish/`, `fixtures/ui/golden/publish/`

## Depends on

- L0 contracts (`PreflightGrant`, `RunState`, `ArtifactSet`), `Engine`
  trait (tested through `StubEngine`).

## Deliverables

1. **Target registry**: GitHub Pages, Standard.site/ATProto, Nostr
   NIP-23, static dist — verified/unverified labeled exactly as Boris
   labels them. Each target declares its `TargetDeployProfile`
   (base_url, slash policy, robots contract).
2. **Preflight**: builds the **target profile into its own output dir**
   (never the loopback watch build), runs validate + check + ruleset on
   those exact bytes, issues a `PreflightGrant` sealing
   `artifact_set+graph+plan+target_cfg+ruleset+boris_version` hashes.
3. **Publish**: delivers exactly the sealed plan; hash mismatch =
   refuse. Approval persisted **before** dispatch. States
   `dispatching → submitted | unknown | complete`; crash after contact
   = `unknown` + reconciliation flow, never auto-retry.
4. **`cv-ledger`**: artifacts→checks→claims→deliveries as first-class
   nouns — JSONL in `.cv/` is truth; SQLite is the index.
5. **Ship-the-outputs pass**: after a successful run, machine exports
   (sitemap, RSS, llms.txt, context/RAG) confirmed present+fresh in
   evidence — "always publishing back" enforced.
6. **Delivery adapters** (extension point): copy-to-folder,
   run-command, HTTP POST — IndexNow-class adapters slot here.
7. **Secrets**: keychain (mock for CI), stdin channel only; canary test
   proves no secret in logs/events/docs.
8. **Publish module**: nav = targets + runs; list = runs/steps;
   reading = run detail/live progress; inspector = evidence + grant.
   Goldens: idle, planning, preflight-blocked, mid-run, complete,
   partial, failed, `unknown`, degraded.

## Done when

- Fixture reports render as runs; evidence reachable ≤3 clicks.
- Sealed-hash mismatch refuses publish (test).
- `unknown` recovery demonstrated: kill mid-dispatch → reconcile prompt.
- Store snapshot proves failed run leaves project untouched.

## Must not

- Sign/contact relays yourself. Store secrets in namespaces. Mark a run
  complete from UI optimism. Publish on a timer — explicit intent only.
