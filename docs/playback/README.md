# Playback

The playback pipeline, its controls and the media formats it accepts.

- [Adaptive playback: current contract](../adaptive-playback.md) — the shipped Auto decisions and their mathematics.
- [Adaptive playback: historical implementation plan](../adaptive-playback-plan.md) — the 2026-08-26 design sequence and its baseline.
- [Adaptive playback: derivation archive](../adaptive-playback-spec.md) — the superseded Phase 3 measurement and review trail.
- [Native playback startup](../native-playback-startup.md) — who owns the loading indicator and where it is drawn.
- [Playback navigation](../playback-navigation.md) — CH+/CH− and catalogue actions for live channels.
- [Playback display settings](../playback-osd-settings.md) — the two application preferences on the OTT Settings page.
- [Playback settings](../playback-settings.md) — the Video & playback and Audio & subtitles pages, saved per profile.
- [Drill-in sub-menus in the player track-menu popover](../player-submenus.md) — the multi-page Subtitles and Audio popover design record.
- [Dolby Vision and Dolby Atmos on webOS 4.10.2](../dolby-vision.md) — how the app declares both formats and what the set's binaries say.
- [Native ASS and SSA subtitles](../ass-subtitles.md) — authored script layout on direct play and the burn-in fallback.
- [Soft WebVTT subtitles during transcode](../soft-subs-plan.md) — the plan kept as history after the on-device premise was refuted.
- [Native video sandbox and Repair](../native-video-sandbox.md) — the `jail_missing_rtkmem` failure and the cached device fact.
- [Detail-hero trailer UX](../trailer-ux-plan.md) — the dwell, logo shrink and full-trailer mode, with its 2026-09-17 revisions.

## HLS and track selection

- [Native HLS audio renditions](../native-hls-audio-renditions.md) — joining a video-only VOD playlist with an external audio playlist.
- [Embedded MP2 audio in native HLS](../native-hls-mp2.md) — decoding an inspected embedded MP2 track to stereo PCM.
- [Native HLS track parity status](../native-hls-parity-work.md) — the current candidate's live WebVTT and AUDIO group status.
- [Native HLS subtitle selection](../native-hls-subtitle-policy.md) — the player's own manual subtitle policy and its persistence.
- [Native external HLS WebVTT](../native-hls-webvtt.md) — the subtitle path that extends the separate-audio VOD player.
- [Shared behavior reference](../shared-behavior-contracts.md) — parity requirements pinned to an OTTPlay FOSS commit.

## Engine and pipeline work

- [Buffer-feed playback plan (Starfish BUFFERSTREAM)](../buffer-feed-plan.md) — why the pipeline moved in-process.
- [Engine port design (Rust-first step 5)](../engine-port-design.md) — the design for porting the buffer-feed engine to Rust.
- [libavformat demuxer plan](../ffmpeg-demuxer-plan.md) — the executed replacement of `mkv.rs` by `ff.rs`.
- [#42 Kodi-parity buffer-feed hardening](../kodi-parity-42-plan.md) — the plan derived from Kodi and ss4s source diffs and on-device probes.
- [Player pointer regression (#131)](../testing-player-pointer.md) — the authored 1920×1080 coordinates the FIFO and Lab Control accept.
