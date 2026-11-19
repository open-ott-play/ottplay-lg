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
share one pixel layout. In **Follows focus**, the active window plays at full
player gain and other windows play at the background level (20% by default).
Set that level in **Settings → Interface → Multiview background volume** or run
`ott l_ setting multiviewBackgroundVolume 15`. A value of 0 gives audio
only to the focused window. Selecting one explicit audio window isolates its
sound. The player mixes decoded PCM into one SDL output and applies the ordinary
volume and mute controls before playback.

Each occupied window uses the current Classic or Modern player footer. Selecting
a new item reveals that window's information for the configured display time.
Empty windows leave the video area clear.

Local source choices combine captured saved Favorites and the current catalog.
They retain provider and profile identities, so M3U, VPortal, Plex and authorized
local files can coexist without changing the saved ordinary provider. Opening a
local file retains the existing read permit; no copy or remote file relay is
required. Requests carry opaque source handles bound to the current scene and
slot, never raw URLs, paths or credentials. Refresh or page the source picker to
see more captured catalog rows; discovery is bounded to 1152 entries.

Software limits are admission limits, not a promise of nine simultaneous HD
streams. Each slot has one decoder thread, a 1080p input-pixel ceiling, a bounded
mailbox and a layout-specific presentation limit. The four-window grid targets
up to 30 fps in the focused window and 25 fps in the other three. Moving focus
keeps the existing playback session and audio. Other layouts use a 15 fps ceiling.
A 1080p frame can use half-size YUV planes on a 960×540 tile when both dimensions
retain enough detail after cropping. Unsupported codecs, pixel formats or
sources fail that slot independently. Supported output is 8-bit planar YUV420.
Authorized local files and progressive HTTP media whose final response advertises
byte ranges support seek. The decoder reports its opened transport capability to
remote clients; HLS and unavailable windows do not advertise seek. Accepted paused
seeks publish a landing picture and remain paused. Seeking suppresses preroll PCM
until the target picture, and later pause changes cannot cancel an accepted seek. The initial HLS
reader supports same-origin TS/fMP4 playlists; encrypted, byte-range,
discontinuous and external-audio renditions are rejected. HLS seek is unavailable.
Plex uses direct media; this path does not silently start a transcoder.

Pause/resume and supported seeks target the active slot. Backgrounding, policy
changes and source revocation cancel local work. A replacement waits for its old
decoder and provider lease to retire. Explicit Off may restore the captured
ordinary item only after a fresh authorization check; background retirement must
not restart it. A previous Favorite is rebound through its own saved profile and
fresh ordinary playback policy, even when a different profile is being browsed.
Changed or removed Favorites cannot silently restore another item. Strict
single-source kiosk rejects multi-view.

The feature must be qualified in stages: pure tests, actual concurrent decoding,
GL and audio output, ARM packaging, then sustained playback on the target TV.
Host compilation alone does not establish physical TV capacity or playback.

To check a local grid, run `ott -j NAME status --details | jq .multiview_video_evidence`
twice, several seconds apart. The slot IDs are zero-based. Compare the same
scene and slot generation in both readings. Each successful GLES draw of a newly
uploaded video frame increments `drawn_frames` once. `last_pts_us` gives its
media timestamp. UI redraws of a held picture leave both values unchanged.
These readings measure rendering inside the application. Verify audio and
the visible panel separately. If a window stops, `last_stop` can identify the
source, decoder or renderer stage. A new source attempt clears the old reason.
For example, `source_temporarily_unavailable` can result from an upstream
timeout or HTTP 502, 503 or 504. `source_read` means the reader could not
supply the requested bytes. To investigate a repeatable failure, record the
window, channel and time and compare that interval with the gateway log.
A selected window waiting to retry reports `starting`. If recovery ends, it
reports `failed` and keeps the last frame count and PTS for that selection.
Stopping or replacing the window clears those measurements. To measure how
smoothly an individual window is playing, take two samples from an unchanged
scene and slot generation. Divide the increase in `drawn_frames` by the wall
time between samples. For example, 250 new frames in ten seconds gives 25 fps.
A growing media timestamp alone can also occur when the picture is jerky.
`draw_interval_p95_ms` and `draw_interval_max_ms` summarize the most recent
32 intervals between new pictures submitted to the renderer. A held picture
adds no samples. `draw_intervals_ge_250ms_count` counts all intervals of at least
250 ms in the current window generation. `draw_interval_lifetime_max_ms` is the
longest measured interval, or `null` until the second picture.
These two fields preserve gaps that have rolled out of the 32-interval sample.
A deliberate pause contributes its full elapsed interval when the first new
picture appears after resume. Compare samples during continuous playback in
the same window generation to look for unexpected stalls.
Once playback has started, the player reopens a window automatically only
when its selected HLS playlist confirms a live broadcast. A movie with an
unknown catalogue duration keeps its failure visible without rewinding.

Run this command twice several seconds apart to see each CLI window number
and compare the same generation:

```sh
ott -j l_ status --details | jq '.multiview_video_evidence.slots[] |
  {window: (.slot_id + 1), slot_generation, drawn_frames, last_pts_us, last_stop,
   draw_interval_p95_ms, draw_intervals_ge_250ms_count, draw_interval_lifetime_max_ms}'
```

### Replacing a channel on a session-limited HLS server

Some HLS servers keep a session open briefly after its last request. On a
four-session server, switching the fourth window can then evict another
window. A gateway with this behavior can send the response header
`OTT-Multiview-Admission: refresh-peers-v1` on its successful live HLS
playlists. Configure the header on the device-specific media route.

For direct M3U playback, OTTPlay then waits for the old reader to close and
for the surviving windows on that origin to read their existing streams
before opening the replacement. The other pictures and their audio keep
playing. If the surviving sessions cannot confirm a read in time, the
target window reports the timeout. Other HLS origins and TV-local files
keep their usual start behavior.

## Using local scenes

The grid and perimeter layouts use the entire screen. Neighboring tiles share
the same edge. PiP places its inset against the screen edge. Each multi-window
picture fills its tile at its display aspect ratio, cropping equally from
opposite sides when the tile and source proportions differ. A single window
shows the full picture. Black bars embedded in the broadcast stay in the video.

Open **Settings → Multiview** and choose a layout. Move the highlight with the
arrow keys, then press **OK** to activate that window and open its **Channel
list**. With the Magic Remote pointer, click the window to do the same. Clicking
near the top of a window opens its **Menu**. Both gestures follow the same
rectangles as rendering, including the inset in picture-in-picture.

Press **Yellow** or **M** for the active window's **Menu**. It offers source
selection, play or pause, previous and next, restart, audio selection and stop
when those actions are available. **Blue** or **I** reveals the active
window's **Info** footer. **Back** closes the list or menu to the same grid.
Selecting a different source changes only that window. **Close multiview**
restores the previous ordinary source after retirement and fresh authorization.

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

With a CLI that lists named selection in `ott PLAYER mv --help`, assign an
M3U channel or an authorized TV-local file directly. In this example, replace
the player alias and file name with your own:

```sh
ott l_ mv layout grid4
ott l_ mv 1 m3u "VF Disney Movie"
ott l_ mv 2 m3u "BCU Premiere HD"
ott l_ mv 3 local "Movie.mp4"
ott l_ mv sources 4 --provider m3u --search "РЕН ТВ HD"
ott l_ mv window 4 play TICKET_FROM_WINDOW_4
```

Use the file's exact displayed name from the player's Local catalog or Favorites.
Its spelling and case matter. When two channels share a name, choose the ticket
for the intended entry or pass its `--source` ID with the named command.

Repeat the source-list/play pair for `vportal` or `local`, or use a returned
opaque profile token with `--profile`. Apply each fresh source ticket before
refreshing discovery; tickets and cursors cannot be reused across windows or
scenes. A displayed name or a numeric profile position is not a source token.

Other layouts are `pip`, `grid4`, and `perimeter --satellites 8`. Use explicit
`off restore_previous`: the CLI's unqualified Off defaults to `promote_active`,
which this initial local backend does not advertise. Source support and actual
admission are checked for each window independently; successful CLI dispatch is
not a decoded-frame or TV-capacity measurement.

## Hosted output qualification

The Native checks workflow runs these checks:

- Build the pinned bundled FFmpeg component set, assert the C ABI, and record
  source, configuration, library and header identities.
- Execute the explicit mixed nine-session test: three held files, three HTTP
  sources and three HLS sources, with seek, cancellation and retirement checks.
- Run four independent live HLS readers against rolling playlists on loopback.
  Two feeds encounter a temporary fault on a media segment (HTTP 503 on one,
  an HTTP 200 response containing the gateway timeout message on another).
  Verify both segments are requested again. For 23.976, 29.97, 25 and 50 fps
  inputs, count the frames each reader actually delivers over its media timeline
  both before and after moving focus from the first window to the second.
- Execute the production YUV Texture and SDL Sink paths in separate OpenGL core
  and GLES processes using owned Mesa/Xvfb and virtual PulseAudio resources.

The output test compares framebuffer pixels with a CPU YUV reference, updates
one window, checks ordinary UI drawing afterwards, rejects invalid planes and
foreign contexts, and checks texture retirement. Real decoded PCM passes through
SDL; the owned PulseAudio monitor must capture nonzero audio. Owner changes,
queue clearing, draining and reopening are checked separately. Four independent
monitor captures measure the same decoded PCM at full gain, half gain, mute and
restored gain. Receipts retain raw PCM and the declared steady measurement window;
bytes already accepted by the audio driver are not claimed to stop instantly.

Each stage verifies the same source and decoder artifact before execution.
The output receipt records actual GL/GLSL versions, texture format and stride,
SDL driver and PCM format. Raw frames, PNGs, hashes and logs remain CI artifacts.
These are virtual host results, not a measurement of LG graphics, speakers,
thermal limits or nine HD streams. The integration tests are explicitly ignored
in the ordinary Rust suite and run only in their dedicated hosted stages.

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
