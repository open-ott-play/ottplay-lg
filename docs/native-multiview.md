# Native multi-view integration

Multi-view remains an experimental integration candidate within the native
application. A layout, an accepted control request, a successful host test and
physical TV playback are different qualification results. Geometry and host
checks do not establish a qualified multi-view playback feature.

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

The protected setting is empty by default: no configured service means no
discovery request or service session. An advertised multi-view capability of `0`
signals missing qualification; it is not a command-dispatch kill switch.
Explicitly configuring a correctly paired service can permit raw experimental
`multiview` commands after the existing policy, service, source and native-start
checks. The controller/CLI may independently refuse an unadvertised operation.

Background, unavailable protected state, changed credentials or a changed
installation identity cancel the old binding. A cancelled worker must retire
before its replacement starts. Service capabilities with zero admitted inputs
are metadata, not playback readiness.

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
Each provider item also needs an independently authorized server catalog entry;
a saved Favorite alone does not establish server playback support. TV-local
files and system television sources need a separate server-readable mapping or
relay and are not exported as local device paths.

Every mutation retains the original runtime, scene, geometry and relevant slot
binding. A late completion may clean up only the resources it created. The
ordinary saved provider is not changed by a temporary multi-view command.

Current limitations include serialized metadata/probe and focus/audio work,
unsupported replacement of an occupied slot, and incomplete Off modes. Off
closes the owned backend session; restoring the previous ordinary source and
promoting the active slot are not implemented. Independent focus/audio during
source preparation remains a qualification requirement.

The controller/CLI uses `mv`, with human-facing windows 1–9 and protocol slots
0–8. Capability reporting and the experimental command admission described above
are separate boundaries. Strict single-source kiosk policy rejects multi-view. Per-window seek, pause and
resume require dedicated backend support and must not accidentally control the
whole composed output.

## Qualification

The actual private-v1 bridge test requires an explicitly owned fixture listener
and handshake and is ignored in the ordinary host suite. Its presence does not
mean that the integration gate, sustained media or physical TV acceptance has
passed.

Use synthetic sources and the real command parser, service client, scene
coordinator, output admission and final native-start gate. Parser-only tests and
mock control receipts do not establish playback. Verify cancellation, source
replacement, lease renewal, single audio ownership and sustained decoded output
before qualifying a server profile. Qualify ARM packaging and physical LG
playback separately, preserving the primary application identity and profiles.
