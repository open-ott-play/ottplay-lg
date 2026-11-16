# Native playback startup

## Ownership of the loading indicator

The native player draws its own loading indicator through PlayerScreen, player_hud::classic::draw_readout, StatusOverlay and the shared Spinner widget. This is application-rendered UI, not a webOS decoder overlay. Classic and Modern place the ordinary loading orbit in the top-left corner, matching the desktop player. Non-loading modal status overlays keep their own centred layout. Radius, colour, dot count, placement and animation can be changed in the UI implementation; there is currently no dedicated user setting for these properties.

The channel name and ordinal belong to the separate playback information area. During the top-left working readout and inline transport buffering, the name remains and the ordinal is hidden. The already-derived transport mark distinguishes a real loading spinner from directional seek arrows, which retain ordinary numbering. Normal playback information, channel lists and explicit numeric entry preserve their existing numbering. A numeric prefix that is part of the actual channel name is not stripped.

## Acquisition and first frame

For ordinary external HLS, source preparation performs an inspection before admission. The inspection can fetch the master, media playlist and a selected media segment to identify codecs and route compatibility. The playback path opens the playlist and segment again. The FFmpeg segment path then completes demuxing and validation into HlsSegmentOutput before publishing access units to the decoder. Large or slow segments can therefore delay the first playable picture even after some access units have already been parsed.

The whole startup time must be split into source resolution, inspection, admitted playback acquisition/probe, decoder prime and the first rendered frame. A spinner animation is not a measurement of any of those phases. Existing preplay diagnostics expose resolving/inspecting durations; segment diagnostics include open_ms, open_probe_ms, first_au_ms and total_ms. The first parsed access unit is not the first published or rendered frame.

The external-HLS path and adaptive-controller routes have different buffering contracts. A saved browser buffer preference must not be assumed to configure the native path. Likewise, the bundled Hls.js defaults do not establish progressive fragment delivery: compare the actual engine configuration and selected route.

## Comparing implementations

Use the same source, channel/rendition, authentication state and network for native, Tauri and the hosted player. Separate cold and warm starts. Bind each measurement to one playback attempt and record request admission through the first rendered frame, not just a successful play command or an inspector completion. Record segment size/duration, HTTP/probe timings, retries and decoder prime so that network delay is distinguishable from application work. Keep source URLs and credentials in private evidence.

Historical diagnostics show that inspection and whole-segment processing can be substantial. They are evidence for investigating these phases, not a controlled cross-player performance result. Safe optimisation candidates include avoiding redundant validated inspection/acquisition work and designing bounded incremental segment publication. Any such change must retain codec admission, cancellation, source ownership, transport-clock/discontinuity validation, A/V synchronisation and partial-failure recovery. Reducing every buffer or dropping inspection is not an established fix.

## Channel-change information

An accepted fullscreen next/previous request immediately reveals the selected target in the information footer. A separate source-fenced paint snapshot supplies the target title, artwork and available guide facts while the decoder is still acquiring it. This snapshot does not replace the actual playing identity or grant transport authority. Acquisition keeps the footer alive, normal auto-hide resumes after settling, and deliberate dismissal is respected. Repeated channel steps advance from the latest pending target and supersede older work; only the last valid request may commit.

The application resolver can run before the media route marks itself busy. During that exact current request, or a current paced same-channel recovery cooldown, the player displays loading rather than an outgoing decoder error. Cancellation, expiry, source/profile replacement and terminal failure retire that presentation. It does not clear native error flags or change playback authority.

## Loading titles, artwork and live recovery

The catalogue wait cover uses the source content title, not the numbered display label. Channel artwork in catalogue rows and the playback footer is fitted inside its slot with preserved aspect ratio; poster shelves retain their separate cover-crop layout. During live playback, Up first reveals the footer and a fresh second Up opens current programme information. Classic expands above the footer; Modern uses its information card. Finite-video and channel-rocker navigation retain their existing roles.

A retained failed-channel receipt may supply presentation facts even while the UI cannot accept actions. This does not grant playback authority. Catalogue recovery still rechecks source, profile, category and input ownership, and a replaced source invalidates the old channel facts. Live EOF retains the same source-bound paced recovery model rather than falling into a generic player footer.

For extensionless HTTP sources, inspection completes the already-open response after recognizing an HLS header. It preserves the consumed prefix, validated final URL, body cap, cancellation and original deadline. This avoids a redundant initial manifest request; selected-manifest timeouts remain separately observable and are not automatically classified as fixed.

## Durable selection and current programme information

An admitted selection/source change is checkpointed without waiting for the playhead update interval. Same-selection playhead saves and failed write retries retain the ten-second interval; a newer choice while a writer is busy keeps its immediate-save opportunity. Live bookmarks identify the channel and resume at the live edge; finite videos retain their saved position. A synchronous refusal to restore a known live channel cannot automatically choose another channel.

Current programme information can use an already-admitted exact-source guide after a completion notification has gone. An explicit metadata failure remains cleared until a later accepted completion. A current title/time can open information even without a synopsis; expired and unrelated details are excluded. Authenticated status exposes only presence/counts and resume-match booleans under row_guide.player for diagnosis, without source addresses or programme contents.

For the recognised channel-proxy export route, a thirteen-digit timestamp q is omitted only from the channel identity. Playback still uses the complete fresh URL. General q selectors, source host/path, provider ID, title and group remain meaningful. Existing portable Favorites bind through their provider/name locators; old opaque resume hashes cannot recover their lost URL context. Capture a source-checked channel handoff during an upgrade from the older identity scheme; later restarts resolve the stable ID after playlist refresh.

## Transient live-source recovery

Initial and selected HLS playlist requests, and active playlist refreshes, may
retry up to three positively identified transient network/server failures. The
requests and short backoffs share the original absolute allowance (at most
fifteen seconds); a valid first response keeps its full allowance. Refresh
retries retain the same manifest, buffered A/V, cursor and decoder. Shared
finite/caption readers keep their separate existing request policy.

A fully exhausted preparation deadline or an attributed live disconnect enters
the exact selected channel's recovery policy, including an exact CLI selection.
It retries after 1, 2, 4, 8, 15 and then at most once every 30 seconds while that
source and viewer intent remain valid. It never tries a neighboring channel.
Thirty seconds of sustained native position progress renews the fast retry
allowance; merely entering Playing or briefly showing a picture does not.
Finite-file failure handling retains its existing bounded policy.

Stop, Back, pause/suspend, replacement requests and changed source/profile/list
ownership cancel or refuse old recovery. Each reopening runs ordinary source,
parental and kiosk admission again. Authentication, TLS trust, malformed media
and unknown errors are not treated as positively identified transient faults.
A generic unavailable-media message alone is not retry evidence: inspection
retains closed timeout/network/server reasons before a native session exists.

The original remote command is completed once with its actual result. Automatic
recovery belongs to the retained player selection; it does not replay the RPC
or claim that playback succeeded before admission.

## Logical HLS entry and temporary media sessions

For plain live HLS without a separate presentation descriptor, inspection keeps
the validated logical input, including the original input before a redirect.
The native worker follows at most four initial playlist documents under one
fifteen-second absolute allowance to acquire a fresh media-session address.
The first media snapshot is consumed directly, then its landed address is fixed
for subsequent refreshes; a later media-to-master change is still rejected.
Relative children use the validated redirect landing as their base, while
route and observed-track ownership remain bound to the logical source.

This avoids treating a short-lived address obtained during a separate media
inspection as the durable channel entry. Codec, track, cancellation and trust
checks still apply. Finite playlists and subtitle/separate-audio presentation
descriptors retain their existing source handling. A checked premature end
before HTTP Content-Length permits same-source live recovery after discarding
the incomplete private segment result; it does not authorize blind demux replay.

## Inspection body failures

After headers have been accepted, media probing must retain a checked HTTP body
transport or deadline failure before libavformat cleanup. Converting that cause
to a generic container error prevents the existing exact-source live recovery
from recognizing a temporary interruption. Extensionless sniffing follows the
same rule. Cancellation, the probe byte limit, malformed data, local I/O and
trust/authentication refusals remain distinct; generic probe failures are not
retry authority. Receiver acceptance must distinguish queue/input progression
from actual displayed video and audible synchronization.

## Passive preparation diagnostics

With a matching controller and CLI, `ott PLAYER debug --preparation` reads the
current preparation phase, a closed cause code and pending retry delay. Add
`-j` for the bounded JSON report. The command starts no media requests and does
not restart playback. Older players without the capability return unsupported.

The separate `playback_preparation_diagnostics` version-1 capability uses the
current runtime token from capabilities. Requests contain only `version` and
`runtime`; replies expose no source URL, cookie, title, profile identifier or
catalogue entry. Existing preparation version-1 status is unchanged. A missing
pending retry projects null failure/delay counters rather than implying a stop
reason. Source/profile/generation changes retire the captured inspection cause.

## Source-owned HTTP context

External HLS uses one bounded ephemeral HTTP context for the initial master and
child traversal, segments, refreshes and connection replacement. Inspection has
its own private context; native playback creates an independent one. Cookies
are handled in memory by libcurl and never written to disk, logged or projected
to remote diagnostics. Default unrelated transfers keep their existing mode.
Domain-scoped cookies are not replayed without public-suffix validation support;
host-only cookies and connection reuse remain available. Malformed/import or
resource-limit failures remain closed refusals. No cookie state crosses source
or playback-generation ownership boundaries.

Legacy capture `frames` and `frame_age_ms` describe native position callbacks,
not decoded or displayed frames. Their ages, and callback error ages, are
computed in the media producer's monotonic clock. `callback_error_code` is the
firmware number, not event type 18. The final matched failed-session sample is
retained before stopping the decoder; a later retry cannot overwrite it with
another source's counters.
