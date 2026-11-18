# Architecture and development

The module layout, build, code rules and the audits that shaped them.

- [Detailed agent reference](../agent-reference.md) — the long-form architecture, build, portability and verification reference.
- [Module layers](../module-layers.md) — the `rust-modules` cycle and the leaf `plx_shared_core` crate.
- [Rust-first migration plan](../rust-first-plan.md) — moving from a C core with Rust leaves to a Rust-first app.
- [Sans-IO: logic never performs IO](../sans-io.md) — the rule and the four ledgers that gate it.
- [Writing Sans-IO code](../sans-io-guide.md) — how to write a store, screen or machine step under that rule.
- [Canonical native operator adapter](../native-operator-core-adapter.md) — running the pinned shared core in the isolated QuickJS context.
- [Bound native provider configuration, version 1](../native-provider-settings-v1.md) — the additive contract for Edem and 43 named operators.
- [Why Plex sign-in can work while OTT Save fails](../native-credential-storage.md) — the native ARM storage paths in this tree.
- [Building PlxNative](../building.md) — the build, the two installs, the device loop and the test tiers.
- [The disk budget](../disk-budget.md) — the no-op build preflight, `make disk` and the retained planner design.
- [Troubleshooting native CI](../ci-troubleshooting.md) — checking the current PR head rather than a historical run.
- [TODOS](../TODOS.md) — the open work list.

## Audits and reviews

- [Architecture & code review — 2026-07-26](../architecture-review-2026-07-26.md) — 179 findings reduced to 53 by two adversarial lenses.
- [Codebase-Wide Simplification Audit](../simplification-audit-2026-07-18.md) — 96 candidate findings triaged against the tree.
- [Async model review](../async-model-review.md) — the 84-finding audit prompted by a frozen HUD during loading.
- [Async model — decision](../async-model-decision.md) — which concurrency model the project adopts and why.
- [Known issues](../known-issues.md) — the entry to read before decompiling firmware or blaming LG.
- [Parity gaps vs. the official Plex clients](../parity-gaps.md) — the historical 2026-07-29 Plex-client comparison.
- [Native LG capability status](../native-capability-status.md) — the 0.4.23 candidate inventory that replaces it.
