# Native multi-view integration

Multi-view is an integration candidate, separate from the installed single-video
application. A layout, an accepted control request, a successful host test and
physical TV playback are different qualification results. Do not enable a
playback capability from geometry tests alone.

## Playback architecture

The paired service decodes independent inputs and composes one H.264/AAC HLS
output. The LG application plays that output with its existing native video
pipeline and renders window controls over it. Four or nine visible windows do
not imply four or nine hardware decoders on the television.

Layouts are single, picture-in-picture, four equal windows, nine equal windows,
and a central window with up to eight clockwise perimeter windows. The native
application chooses exact even-pixel rectangles from an admitted output profile.
Pointer hit testing uses the same rectangles. Focus, the highlighted window and
the single audio owner are separate state. TV volume remains global.

## Service authority

`nativeMultiviewService` is a sensitive, installation-specific application
setting. It contains a compact JSON string of at most 4096 UTF-8 bytes with
`server_id`, HTTPS `origin`, `audience` (`ottplay-multiview-v1`),
`installation_id` and `control_bearer`. The installation must match the player's
persisted remote-control device identity. Provision credentials separately for
each installation. This setting is excluded from portable Local profiles;
reads and receipts return redacted metadata.

No configured service means no discovery request. Background, unavailable
protected state, changed credentials or a changed installation identity cancel
the old binding. A cancelled worker must retire before its replacement starts.
Capabilities with zero admitted inputs are metadata, not playback readiness.

The explicitly paired HTTPS compositor is trusted as a manifest and redirect
authority. Its control bearer is never passed to the media player. The server
enforces signed media-lease paths and expiry. Generic native HLS acquisition trusts that configured manifest authority; it
does not independently confine every fetched child to the signed lease prefix.

## Sources and controls

The first native source snapshot is bounded to existing saved Favorites, including
their original provider/profile bindings. It is not a complete browser for every
inactive provider catalog. Choices are intersected with the server's enrolled
catalog using exact identity digests and independently checked source authority.
Names, folders and equal display labels do not grant access. Native URLs, paths
and provider credentials are not accepted as multi-view control parameters.

Every mutation retains the original runtime, scene, geometry and relevant slot
binding. Long metadata/probe operations must not block unrelated focus or audio
changes. A late completion may clean up only the resources it created. The
ordinary saved provider is not changed by a temporary multi-view command.

The controller/CLI uses `mv`, with human-facing windows 1–9 and protocol slots
0–8. Actual operations remain gated by the player's advertised capabilities.
Strict single-source kiosk policy rejects multi-view. Per-window seek, pause and
resume require dedicated backend support and must not accidentally control the
whole composed output.

## Qualification

Use synthetic sources and the real command parser, service client, scene
coordinator, output admission and final native-start gate. Parser-only tests and
mock control receipts do not establish playback. Verify cancellation, source
replacement, lease renewal, single audio ownership and sustained decoded output
before qualifying a server profile. Qualify ARM packaging and physical LG
playback separately, preserving the primary application identity and profiles.
