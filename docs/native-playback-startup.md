# Native playback startup

## Ownership of the loading indicator

The native player draws its own loading indicator through PlayerScreen, player_hud::classic::draw_readout, StatusOverlay and the shared Spinner widget. This is application-rendered UI, not a webOS decoder overlay. Classic and Modern use the same central readout. Radius, colour, dot count, placement and animation can be changed in the UI implementation; there is currently no dedicated user setting for these properties.

The channel name and ordinal belong to the separate playback information area. During the central working readout, the name remains and the ordinal is hidden. Normal playback information, channel lists and explicit numeric entry preserve their existing numbering. A numeric prefix that is part of the actual channel name is not stripped.

## Acquisition and first frame

For ordinary external HLS, source preparation performs an inspection before admission. The inspection can fetch the master, media playlist and a selected media segment to identify codecs and route compatibility. The playback path opens the playlist and segment again. The FFmpeg segment path then completes demuxing and validation into HlsSegmentOutput before publishing access units to the decoder. Large or slow segments can therefore delay the first playable picture even after some access units have already been parsed.

The whole startup time must be split into source resolution, inspection, admitted playback acquisition/probe, decoder prime and the first rendered frame. A spinner animation is not a measurement of any of those phases. Existing preplay diagnostics expose resolving/inspecting durations; segment diagnostics include open_ms, open_probe_ms, first_au_ms and total_ms. The first parsed access unit is not the first published or rendered frame.

The external-HLS path and adaptive-controller routes have different buffering contracts. A saved browser buffer preference must not be assumed to configure the native path. Likewise, the bundled Hls.js defaults do not establish progressive fragment delivery: compare the actual engine configuration and selected route.

## Comparing implementations

Use the same source, channel/rendition, authentication state and network for native, Tauri and the hosted player. Separate cold and warm starts. Bind each measurement to one playback attempt and record request admission through the first rendered frame, not just a successful play command or an inspector completion. Record segment size/duration, HTTP/probe timings, retries and decoder prime so that network delay is distinguishable from application work. Keep source URLs and credentials in private evidence.

Historical diagnostics show that inspection and whole-segment processing can be substantial. They are evidence for investigating these phases, not a controlled cross-player performance result. Safe optimisation candidates include avoiding redundant validated inspection/acquisition work and designing bounded incremental segment publication. Any such change must retain codec admission, cancellation, source ownership, transport-clock/discontinuity validation, A/V synchronisation and partial-failure recovery. Reducing every buffer or dropping inspection is not an established fix.
