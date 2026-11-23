# Round 3 — Claude Opus 5.5 review of PROMPT-v2

## Attacks (verified against editor-host.md)
A. Windows: host watch/validate daemons ALWAYS FALSE on Windows, no
   signal handler. Need per-OS watch owner (probe capability); verify
   raw boris watch works on Windows; one watch owner per OS, not split.
   Ring: 100 events/4MiB, pull-only single-threaded host -> at 5k pages,
   gap-resync every poll. Fix: CV-owned cursor (engine_session_id, seq)
   over normalized stream; session change = full resync.
B. Watch daemon holds "dist-writer seat" incl. backoff -> "serialize
   mutating verbs" is false while watch runs; torn dist/ during publish.
   Fix: stop-watch before dist-writing verbs OR separate output dir;
   scheduler models the seat as a held lock.
C. Preflight TOCTOU: watch build is loopback --serve build -> base_url/
   canonical/og:url/sitemap loc/llms.txt links are localhost-shaped.
   Fix: Preflight builds TARGET profile into its own dir, rules on those
   bytes, artifact-set hash recorded in ledger; publish refuses if hash
   differs. URL-dependent findings on watch builds marked `limited`.
D. OliverRender has no consumer: rune_kit::Markdown displays it, not
   Oliver HTML; rune_kit is a THIRD Markdown dialect -> parity lane
   can't converge. Fix: drop OliverRender from interactive path; render
   only after save from Boris's own build; label Markdown preview
   "draft sketch" or delete.

## Rulings
- Engine boundary: keep dual adapter but demote host to impl detail
  behind CV-owned cursor/session. BYO binary is DEAD: host contract is
  closed/lockstep -> vendor boris+boris-editor as ONE pin.
- rusqlite: right engine, wrong place (synced .cv WAL corruption) ->
  DB in OS app-data keyed by project id; JSONL trail in .cv is source of
  truth, SQLite = rebuildable index. Don't duplicate host recovery
  snapshots.
- Single cv-ui: agree; make modules self-register via xtask-generated
  list (integrator-owned) to kill merge hotspot.
- Contract-hardening wave: needs exit criteria = conformance suite green
  on pinned binary on every watch-capable OS; type changes ship codemods.

## What everyone missed
1. AppEvent merge conflicts: additive variants still collide textually
   in one file -> lane-namespaced variants AppEvent::Audit(AuditEvent),
   payload enums in lane-owned files under cv-core/src/events/.
2. PNG goldens can't gate on 3 OSes (GPU/font differences; macOS CI may
   lack Metal) -> element-tree goldens are the 3-OS gate; PNG gates one
   canonical OS w/ software rasterizer, advisory elsewhere.
3. Finding dedup key under-specified: code+subject collapses N broken
   links on one page -> third component = hash of offending target
   (URL/attr value); lock in frozen contract or packs invent their own.
