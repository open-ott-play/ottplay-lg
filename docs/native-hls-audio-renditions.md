# Native HLS audio renditions

The native player can join a video-only HLS VOD playlist with one of the external
`EXT-X-MEDIA:TYPE=AUDIO` playlists advertised by its master. This is separate from
embedded audio tracks in a multiplexed transport stream and from Plex's track
selection. It uses the existing native decoder and owned HTTP transport.

## Using it

Open a supported master playlist and open the player's audio track menu. The
master's rendition names and languages appear there. The declared default is
selected initially; when there is no declared default, the first rendition is
used. Selecting a row prepares that rendition off the UI thread. The checked row
changes only after the current playback session accepts the prepared selection.
A failed preparation leaves the current selection unchanged. Later media or
transport failures stop with an error; they never silently select another language.

The same retained audio group is available through the existing version 1
`media_tracks` list/select API. List is passive: it does not fetch or inspect
media. It lists the active master audio group, not an embedded-track inventory
inferred from one segment. Unsupported HLS sources without that retained
presentation remain unavailable through this API. Subtitle group declarations do
not block otherwise supported audio/video playback. A selected group can expose
manual WebVTT choices when the separate-rendition TS VOD has a verified video
clock anchor; see [HLS WebVTT subtitles](native-hls-webvtt.md). Unselected subtitle
playlists are not fetched. Default/forced auto-selection and browser-player
changes are not included.

Remote row IDs are short-lived catalog references. Clients must send back the
returned playback, source-generation and catalog references when selecting a
row; they cannot substitute a stream ordinal or rendition URL. Alternative
renditions are checked when prepared, so a listed row is not proof that every
alternative can play. Codec metadata is left unknown rather than inferred from
the currently selected language. A successful selection reply says only
`accepted: true`, `outcome: requested`, with `observed: null`; it does not report
that audio has been heard or decoded.

## Supported presentation

- A finite master/media VOD presentation, with `EXT-X-ENDLIST` on each media playlist.
- One H.264 video elementary stream per video TS segment and one AAC audio stream
  per audio TS segment. The device must support the selected AAC format natively.
- External audio rendition URIs, a single selected video variant (the lowest
  declared bandwidth), and consistent descriptors across segments.
- A first explicit `EXT-X-PROGRAM-DATE-TIME` on both media playlists, with the same
  initial date. The encoders may have different raw PTS epochs and segment sizes.
- Continuous raw packet clocks and positive durations for every mapped sample.
  Video packets require PTS. Missing individual AAC PTS may be reconstructed within
  a segment from a real packet PTS anchor and positive packet durations. Complete
  ADTS frames with an LC core and one raw-data block per frame additionally permit
  missing durations to be derived from their checked core sample count and rate.
  Fractional durations, including 44.1 kHz audio, advance a cumulative rational
  clock in the original time base instead of accumulating rounded frame steps.
  Supplied PTS, DTS and positive durations remain unchanged. Only this validated
  ADTS path fills an absent DTS from its anchored PTS; DTS never substitutes for
  a missing PTS anchor. Audio without any PTS anchor is refused. Existing gap,
  overlap and format-change checks still apply. This does not expand the device's
  supported decoder formats. Media-sequence numbers are independent; their
  numeric values are never used to align audio with video.

Live/sliding playlists, discontinuities, encryption, fMP4/init maps, byte ranges,
partial segments, separate video groups, non-WebVTT subtitle groups, multiple
elementary streams in one rendition segment, and software-only audio decoding
remain outside this route. Existing multiplexed HLS and Plex paths keep their own behavior.

## Synchronization and seeking

The first explicit program date binds each encoder's raw PTS clock to the common
presentation clock. Subsequent packets keep that binding. The joiner does not
reset timestamps at segment boundaries, sort B-frames by presentation time, or
stretch audio to conceal gaps. Raw segment continuity and audio sample continuity
allow at most 1 ms for timestamp quantization. Gaps, repeated segments, descriptor
changes and unannounced clock resets fail closed.

`EXTINF` durations may be rounded. Dates extrapolated from those durations are
navigation estimates, not a new time origin or evidence of a precise clock.
Explicit later program dates are checked against the retained raw clock, allowing
both the initial and later timestamps' decimal resolution (with a 1 ms minimum).
They do not rebase queued timestamps. A coarse initial date necessarily limits
how precisely independent encoder clocks can be aligned.

A seek retains both original raw-clock bindings and clears only the continuity
cursor and queued audio tail. If playback first starts at a saved position, the
first video and audio segments are read to establish those bindings without
feeding the decoder. The playlist estimate locates a candidate segment, then its
real packet interval corrects rounding drift. Audio is sought to the actual video
boundary. The decoder retains its requested position and owns preroll. Pause is
not changed by selecting a language.

## Ownership and limits

The presentation holds the original source, landed master, chosen variant and
rendition URLs privately in memory. Rendition IDs belong to one immutable
presentation; they are not stream ordinals or portable saved IDs. Custom debug
representations omit URLs and credentials. The serialized source/profile format
is unchanged.

The application runs at most one preparation worker and keeps only the latest
queued choice. A newer request cancels older authority immediately. Completion
requires the same playback identity, immutable presentation, user-intent revision
and worker ticket. Reopening even the same URL invalidates old menu actions and
worker results. Closing or cancelling the originating menu also retires its
addressed request: a later worker result cannot switch the language. Trusted
unaddressed requests keep their own route authority. Standby, playback shutdown
and kiosk policy also gate publication.

UI and remote selections share that one worker and supersede each other. A
remote request retains its original invocation and the earlier of its request
and catalog expiry times. Renewing the list cannot extend an in-flight request.
Before publishing a result, the app rechecks the remote grant, runtime, playback,
route intent, worker and current playback policy. Expired or revoked requests
are retired and cannot change the active language; replacing them does not
cancel a newer selection. An original grant that is still current receives one
terminal reply. The existing remote delivery guard drops replies from a revoked
generation instead of sending them through a replacement grant.

While a UI or remote HLS track preparation still owns its result, kiosk policy
changes are refused; read-only kiosk status remains available. Automatic kiosk
capture waits without consuming its unsaved intent. Suspension still cancels
that unsaved intent before the wait check. A source that requires a server
playback lease must retain that lease; a lease held only for cleanup is not
playback authority. Direct HLS sources do not acquire a server-lease requirement.

HTTP requests use the existing bounded, cancellable owned transport; FFmpeg is
explicitly given MPEG-TS input and is not allowed to open network URLs. A segment
body is limited to 8 MiB and 16,384 samples. The joint queue is limited to 32 MiB
and 32,768 samples. Temporary demux buffers and the currently held video/incoming
audio segment are additional to that queue; these limits are not a claim that the
whole player uses only 32 MiB. Audio spanning a video boundary is emitted once,
while its unconsumed future samples remain queued for the next interval.

## Verification scope

Pure/contract tests cover parser precision, independent sequence numbers, retained
clocks, raw gaps and repeats, rounded durations, B-frame order, audio tails,
session-bound selection, cancellation/coalescing and menu identity. Separate
ignored bundled-demuxer tests generate synthetic localhost H.264/AAC fixtures.
They check host demux/join output, exact raw timing for 48 kHz audio and video,
and bounded tick rounding for 44.1 kHz audio. They also check selected 220/880 Hz
tones through CLI decoding, seek boundaries and rejection of a missing rendition.
Even these real-media host tests do not establish TV hardware decoding, physical
audio/video synchronization, an in-session UI/remote language switch, or a
successful native deployment.
