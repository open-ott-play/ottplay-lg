# UI and rendering

The retained UI framework, the design system and the measurements behind the
frame budget and the glass material.

- [PlxNative design system](../DESIGN.md) — the index that points at the files which enforce each design claim.
- [retui — the retained UI framework](../ui-framework.md) — the original design note, kept for its rationale.
- [retui — framework improvements](../ui-framework-improvements.md) — the improvement record, with the newer heartbeat field names.
- [UI restructure plan (spec v4)](../ui-restructure-spec-v4.md) — the design record cited by section number from `ui/mod.rs`.
- [Shared UI System — Migration Plan](../ui-system-migration.md) — the token sweep and view-tree milestone.
- [UI View-Tree Migration Plan (retui Steps 5–7)](../ui-viewtree-plan.md) — the view-tree plan, with the renamed heartbeat fields.
- [retui invalidation: the motion capability](../retui-invalidation-design.md) — the invalidation defect and what the restructure phase actually shipped.
- [Stores as machines — restructure phase 4](../stores-as-machines.md) — endpoint recovery through each store's own `run` outcome.
- [Declarative forms and one Settings navigation path](../settings-form.md) — the agreed migration to declared row lists and one drill-down stack.
- [Player-HUD plugins](../hud-plugins.md) — the format, renderer, registry, sideload trust and PR breakdown.
- [Poster request scheduling](../poster-scheduler-design.md) — typed state, explicit demand and bounded speculation.
- [Image cache](../image-cache.md) — storage ownership and the two demand workers for remote artwork.

## Frame budget and glass

- [The frame budget — what this television will actually give you](../glass-hardware-budget.md) — the measured budget for deciding whether a screen idea fits.
- [Liquid glass — what the hardware will and will not give us](../liquid-glass.md) — the envelope the backdrop-blur material can be designed inside.
- [EGL partial update, Transaction Elimination, and the opaque region](../egl-partial-update-and-damage.md) — the eight measured batches on the dev set.
- [Backdrop blur: what the refresh cadence costs](../backdrop-blur-cadence.md) — the archived 2026-08-19 cadence experiment.
- [Backdrop blur: baseline graph and HWCNT measurement](../backdrop-blur-profiling.md) — the baseline for the direct-render comparison, with the 2026-09-19 addendum.
- [Verdict: no per-region damage tracking](../perf-damage-tracking-verdict.md) — the decision and the renamed heartbeat fields.
- [View buffers, and whether the 50 is heat](../perf-view-buffers-and-thermal.md) — the investigation into the 50 Hz ceiling.
