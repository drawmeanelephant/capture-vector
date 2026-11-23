# L2 — Shell, Chrome & Verification Harness (`cv-app` + `cv-widgets` + `cv-model` scaffolding)

> Worker brief. You own the window chrome, the module rail, the theme —
> and the headless proof harness every surface lane uses. Note the v3
> split: state/view-models in `cv-model`, rune contact only in
> `cv-widgets`.

## Owns

- `crates/cv-app/` (shell wiring), `crates/cv-widgets/`,
  `crates/cv-model/` (slices + builders scaffolding),
  `fixtures/ui/golden/`

## Depends on

- L0 contracts + `DemoModule`. Fully functional with only DemoModule.

## Deliverables

1. **Module rail + chrome grammar**: domain-oriented rail (Pages /
   Research / Audit / Publish / Activity); nav / list / reading /
   inspector providers rendered generically from `*Model` data.
2. **Module self-registration**: each module registers through one
   lane-owned file; the integrator-owned registry list is xtask-
   generated — no merge hotspot.
3. **Theme as data** (`cv-theme` tokens: color/type/spacing/motion) —
   "Outlook did EDM": dense, keyboard-first, sharp springs, dark+light,
   reduced-motion variant.
4. **Command palette** dispatching lane `CommandSpec`s; verbs live here,
   not on the rail.
5. **Headless harness**: scripted-input driver → named state →
   element-tree + layout-geometry JSON goldens (the 3-OS gate) + PNG on
   canonical rasterizer. Bless mechanic + named reviewer doc'd.
6. **Multi-window scope** (wave-0 decision implemented): per-window vs
   shared `Selection`, dirty-close semantics; companion-window host for
   the editor lane.
7. **Virtualized lists** everywhere — the 5k-page envelope is a hard
   requirement, not a perf ticket.
8. **Degraded states** goldens per module: engine-absent,
   version-mismatch, dead-mid-watch, store-corrupt.

## Done when

- Element-tree goldens green on 3 OSes; PNG green on canonical config.
- Switcher, inspector toggle, palette command driven by scripted input.
- DemoModule exercises every degraded state.
- No crate but `cv-widgets` imports `rune` (cargo-deny proves it).

## Must not

- Spawn processes, read stores, or import lanes — render models, emit
  intents. Hardcode module IDs. Verify "by eye."
