# Native multi-view integration

Multi-view remains an experimental integration candidate within the native
application. A layout, an accepted control request, a successful host test and
physical TV playback are different qualification results. Geometry and host
checks do not establish a qualified multi-view playback feature.

## Device-local playback (default)

`nativeMultiviewBackend` selects `local` (default) or `service`. Change it only
while all scene work is idle; a busy request is rejected. The local backend does
not discover, enroll in, or send media to a compositor. Each occupied window owns
an independent bounded software decoder, a private source authorization and one
latest planar YUV frame. GLES renders those planes directly into the common
layout. It does not create nine instances of the single global hardware player.

The same scene supports PiP, four equal windows, nine equal windows, and a large
central window with up to eight satellites. Rendering and Magic Remote hit tests
share one pixel layout. Selecting a window changes the active input; one audio
owner feeds the queued SDL output. Owner changes, pause, seek and scene retirement
flush queued audio. Volume remains a television-wide control.

Local source choices combine captured saved Favorites and the current catalog.
They retain provider and profile identities, so M3U, VPortal, Plex and authorized
local files can coexist without changing the saved ordinary provider. Opening a
local file retains the existing read permit; no copy or remote file relay is
required. Requests carry opaque source handles bound to the current scene and
slot, never raw URLs, paths or credentials. Refresh or page the source picker to
see more captured catalog rows; discovery is bounded to 1152 entries.

Software limits are admission limits, not a promise of nine simultaneous HD
streams. Each slot has one decoder thread, a 1080p input-pixel ceiling, a bounded
mailbox and a 15 fps presentation ceiling. Unsupported codecs, pixel formats or
sources fail that slot independently. Supported output is 8-bit planar YUV420.
HTTP progressive media and authorized local files support seek. The initial HLS
reader supports same-origin TS/fMP4 playlists; encrypted, byte-range,
discontinuous and external-audio renditions are rejected. HLS seek is unavailable.
Plex uses direct media; this path does not silently start a transcoder.

Pause/resume and supported seeks target the active slot. Backgrounding, policy
changes and source revocation cancel local work. A replacement waits for its old
decoder and provider lease to retire. Explicit Off may restore the captured
ordinary item only after a fresh authorization check; background retirement must
not restart it. Strict single-source kiosk rejects multi-view.

The feature must be qualified in stages: pure tests, actual concurrent decoding,
GL and audio output, ARM packaging, then sustained playback on the target TV.
Host compilation alone does not establish physical TV capacity or playback.

## Using local scenes

Open **Settings → Multiview** and choose a layout. On the video surface,
press Back to open its controls, then use **Choose video / active slot** to pick
a source for each window. The slot menu also selects the active video and audio owner. On the
video surface, the shared focus controls and Magic Remote pointer use the same
rectangles as rendering. Back opens the Multiview controls; Close restores the
previous ordinary source only after retirement and fresh authorization.

On a build that advertises these capabilities, the existing CLI uses windows
1–9 (the wire protocol uses 0–8):

```sh
ott l mv status
ott l mv layout grid9
ott l mv sources 1 --provider m3u
ott l mv window 1 play TICKET_FROM_WINDOW_1
ott l mv sources 2 --provider plex
ott l mv window 2 play TICKET_FROM_WINDOW_2
ott l mv focus 2
ott l mv audio follows-focus
ott l mv off restore_previous
```

Repeat the source-list/play pair for `vportal` or `local`, or use a returned
opaque profile token with `--profile`. Apply each fresh source ticket before
refreshing discovery; tickets and cursors cannot be reused across windows or
scenes. A displayed name or a numeric profile position is not a source token.

Other layouts are `pip`, `grid4`, and `perimeter --satellites 8`. Use explicit
`off restore_previous`: the CLI's unqualified Off defaults to `promote_active`,
which this initial local backend does not advertise. Source support and actual
admission are checked for each window independently; successful CLI dispatch is
not a decoded-frame or TV-capacity measurement.

## Optional service backend

The paired service decodes independent inputs and composes one H.264/AAC HLS
output. The LG application plays that output with its existing native video
pipeline and renders window controls over it. Four or nine visible windows do
not imply four or nine hardware decoders on the television.

Layouts are single, picture-in-picture, four equal windows, nine equal windows,
and a central window with up to eight clockwise perimeter windows. The native
application chooses exact even-pixel rectangles from an admitted output profile.
Pointer hit testing uses the same rectangles. Focus, the highlighted window and
the single audio owner are separate state. TV volume remains global.

### Service authority

`nativeMultiviewService` is a sensitive, installation-specific application
setting. It contains a compact JSON string of at most 4096 UTF-8 bytes with
`server_id`, HTTPS `origin`, `audience` (`ottplay-multiview-v1`),
`installation_id` and `control_bearer`. The installation must match the player's
persisted remote-control device identity. Provision credentials separately for
each installation. This setting is excluded from portable Local profiles;
reads and receipts return redacted metadata.

The protected setting is empty by default. Even if credentials exist, only explicit
`service` mode permits discovery or a service session. An advertised multi-view capability of `0`
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

### Service sources and controls

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

Service-backend limitations include serialized metadata/probe and focus/audio work,
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
