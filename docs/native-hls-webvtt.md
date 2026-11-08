# Native external HLS WebVTT

This path extends the native separate-audio-rendition MPEG-TS VOD player. It does
not change the browser player, the Plex subtitle route, or embedded AVStream
subtitle support.

## Selection and playback

The selected master variant's `SUBTITLES` group supplies presentation-scoped
choices in the shared Audio/Subtitles menu and `media_tracks` API. A candidate
appears only when the video has a verified clock anchor. Master metadata is not
proof that a subtitle playlist uses a supported format: the selected playlist
and each segment are validated before the checkmark changes. Unselected subtitle
playlists are never fetched. Default and forced flags do not automatically select
a track; the initial state is Off.

Selection prepares a complete, bounded VOD timeline on the existing HLS worker.
It converts the result through the shared WebVTT-to-ASS pipeline and publishes it
to the existing sidecar/libass renderer. Font, color, outline, background and
position use the common text appearance settings. WebVTT text markup follows
the existing converter's behavior; CSS, regions and arbitrary authored layout
are not promised.

Preparing another track does not clear the current subtitles. A failed fetch,
invalid timestamp map, unsupported playlist or exceeded budget leaves A/V and
the previous subtitle choice running. A successful subtitle change or Off acts
in place without a native audio reload. Off supersedes unfinished preparation.
The installed complete script survives seek and same-item reload; a new logical
item or retirement of the source clears it.

## Time and ownership

The first video segment is read through the existing private inspection input.
Its actual video presentation timestamps are bound by the same `LaneClock`
mechanism used for video playback. Program date/time supplies the presentation
origin; it is not interpreted as an MPEG transport timestamp.

Each WebVTT segment requires a `WEBVTT` header. `X-TIMESTAMP-MAP` binds local cue
time to the 90 kHz MPEG timestamp domain; a missing map means LOCAL 0 to MPEGTS 0.
The 33-bit timestamp wrap is resolved against the verified video clock and the
subtitle segment's time window. Invalid or ambiguous mapping fails selection.
Cue start/end times retain their full duration across segment boundaries. Empty
segments are valid. Duplicate cues repeated in adjacent segments are removed
only after mapping their time and comparing their content and settings.

The existing worker owns cancellation and bounded fetching. Publication checks
the logical playback identity, presentation identity, user intent revision and
worker ticket, then checks again inside the route's projection reservation.
A newer selection, Off, applied seek or source replacement retires an unfinished
result. A matching URL alone never grants publication authority.

## Boundaries and verification

Supported input is finite, continuous, unencrypted MPEG-TS VOD with separate
audio/video renditions and full WebVTT segment files. Ordinary multiplexed HLS,
live/sliding playlists, discontinuities, encryption, byte ranges, initialization
maps, partial segments and non-WebVTT subtitle formats remain outside this path.
They must not be advertised as embedded subtitle tracks or mapped onto Local or
Plex identities.

Limits include 1 MiB per text response, 4 MiB of subtitle input and generated
output (including the generated ASS header), 4 MiB of retained expanded segment
URIs, 2,048 segments, 32,768 unique cues, 15 seconds per fetch and 60 seconds for
the complete preparation. The converter checks cancellation and the deadline
while processing text, and rejects expanded output before exceeding its cap. A failed or
cancelled preparation never falls back to a different subtitle or audio track.

Regression fixtures cover timestamp mapping/wrap, absent maps, full overlapping
cues, empty segments, deduplication, malformed input, limits, Off, late results,
same-URL replacement, seek retention and unchanged A/V after preparation failure.
Passing host fixtures validates those contracts; it does not establish actual
television rasterization or physical A/V synchronization. Device acceptance must
select two real languages, seek both ways, switch Off during preparation and
verify that a failed subtitle URL leaves playback and the previous choice intact.
