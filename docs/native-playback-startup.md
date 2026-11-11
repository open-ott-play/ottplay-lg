# Native playback startup

## Ownership of the loading indicator

The native player draws its own loading indicator through PlayerScreen, player_hud::classic::draw_readout, StatusOverlay and the shared Spinner widget. This is application-rendered UI, not a webOS decoder overlay. Classic and Modern use the same central readout. Radius, colour, dot count, placement and animation can be changed in the UI implementation; there is currently no dedicated user setting for these properties.

The channel name and ordinal belong to the separate playback information area. During the central working readout and inline transport buffering, the name remains and the ordinal is hidden. The already-derived transport mark distinguishes a real loading spinner from directional seek arrows, which retain ordinary numbering. Normal playback information, channel lists and explicit numeric entry preserve their existing numbering. A numeric prefix that is part of the actual channel name is not stripped.

## Acquisition and first frame

For ordinary external HLS, source preparation performs an inspection before admission. The inspection can fetch the master, media playlist and a selected media segment to identify codecs and route compatibility. The playback path opens the playlist and segment again. The FFmpeg segment path then completes demuxing and validation into HlsSegmentOutput before publishing access units to the decoder. Large or slow segments can therefore delay the first playable picture even after some access units have already been parsed.

The whole startup time must be split into source resolution, inspection, admitted playback acquisition/probe, decoder prime and the first rendered frame. A spinner animation is not a measurement of any of those phases. Existing preplay diagnostics expose resolving/inspecting durations; segment diagnostics include open_ms, open_probe_ms, first_au_ms and total_ms. The first parsed access unit is not the first published or rendered frame.

The external-HLS path and adaptive-controller routes have different buffering contracts. A saved browser buffer preference must not be assumed to configure the native path. Likewise, the bundled Hls.js defaults do not establish progressive fragment delivery: compare the actual engine configuration and selected route.

## Comparing implementations

Use the same source, channel/rendition, authentication state and network for native, Tauri and the hosted player. Separate cold and warm starts. Bind each measurement to one playback attempt and record request admission through the first rendered frame, not just a successful play command or an inspector completion. Record segment size/duration, HTTP/probe timings, retries and decoder prime so that network delay is distinguishable from application work. Keep source URLs and credentials in private evidence.

Historical diagnostics show that inspection and whole-segment processing can be substantial. They are evidence for investigating these phases, not a controlled cross-player performance result. Safe optimisation candidates include avoiding redundant validated inspection/acquisition work and designing bounded incremental segment publication. Any such change must retain codec admission, cancellation, source ownership, transport-clock/discontinuity validation, A/V synchronisation and partial-failure recovery. Reducing every buffer or dropping inspection is not an established fix.

## Channel-change information

An accepted fullscreen next/previous request that queues a replacement reveals the information footer without publishing a new playing identity. The outgoing channel facts remain until the replacement is admitted, at which point the normal source-bound metadata publication updates them. Acquisition keeps the footer alive, normal auto-hide resumes after settling, and deliberate dismissal is respected.

The application resolver can run before the media route marks itself busy. During that exact current request, or a current bounded same-channel recovery cooldown, the player displays loading rather than an outgoing decoder error. Cancellation, expiry, source/profile replacement and terminal failure retire that presentation. It does not clear native error flags or change playback authority.

## Loading titles, artwork and live recovery

The catalogue wait cover uses the source content title, not the numbered display label. Channel artwork in catalogue rows and the playback footer is fitted inside its slot with preserved aspect ratio; poster shelves retain their separate cover-crop layout. During live playback, Up first reveals the footer and a fresh second Up opens current programme information. Classic expands above the footer; Modern uses its information card. Finite-video and channel-rocker navigation retain their existing roles.

A retained failed-channel receipt may supply presentation facts even while the UI cannot accept actions. This does not grant playback authority. Catalogue recovery still rechecks source, profile, category and input ownership, and a replaced source invalidates the old channel facts. Live EOF retains the same bounded recovery model rather than falling into a generic player footer.

For extensionless HTTP sources, inspection completes the already-open response after recognizing an HLS header. It preserves the consumed prefix, validated final URL, body cap, cancellation and original deadline. This avoids a redundant initial manifest request; selected-manifest timeouts remain separately observable and are not automatically classified as fixed.
