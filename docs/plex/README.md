# Plex

The Plex Media Server API layer, its design history and the Plex-specific
behaviour of the native client.

- [Plex Media Server API — reference for the native webOS client](../plex-api.md) — the endpoints verified against a live server.
- [Plex API Catalog](../plex-api-catalog.md) — every called PMS operation reconciled against the official OpenAPI 3.1 spec.
- [Plex API layer — typed Rust design](../plex-api-design.md) — the design note for `rust-modules/src/plex/`, with its superseded address shape.
- [Plex API migration map](../plex-api-migration.md) — the executed move from raw calls to the typed `crate::plex` client.
- [Plex Native Playback Plan](../plex-native-plan.md) — direct-playing everything the SM90 can decode while keeping a proper PMS session.
- [The Plex Pass dependency audit](../plex-pass-audit.md) — every Pass-gated feature the app touches and what breaks on a free server.
- [PMS API Reference](../pms-api.md) — the hand-curated, live-verified subset of paths the app uses.
- [PMS HLS protocol probe](../pms-hls-protocol-probe.md) — the 2026-08-24 host-only probe over a forced transcode.
- [Shared (non-owned) Plex servers](../shared-servers.md) — how Plex structures a shared server and how to integrate one.
- [Design brief — Plex Pass awareness in the player UI](../design-briefs/plex-pass-ui.md) — the visual baseline for Pass states, with the 2026-08-11 resolution.
