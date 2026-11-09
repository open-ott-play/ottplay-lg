# Native HLS track parity status

This describes the current source candidate, including committed media snapshots `37feb00` and `401055f`. It must not be read as the capability list of the earlier published `0.3.3-beta.1` package. Release and device claims require the exact package/revision and its validation records.

## Live external WebVTT and AUDIO groups

Implemented source paths retain the selected master subtitle group and exact logical playback identity. WebVTT refresh uses a bounded off-frame worker, rolling segment cache, actual committed video packet clock, timestamp-map unwrapping and decoder discontinuity/restart epoch. Off, a new selection, source replacement and stale epochs invalidate publication and rendering.

Separate live H.264/AAC MPEG-TS AUDIO groups use independent bounded playlist refresh. Precise explicit PDT establishes epoch anchors; actual raw packet clocks and complete AAC coverage authorize each joined interval. Media sequence numbers are never aligned across lanes. A shared output offset places the first actual native AU at zero while preserving audio/video/subtitle timing. Audio switching refetches the current pair; wrong-epoch acquisition has a fixed deadline. Missing evidence, incompatible formats, changed overlap and gaps fail closed.

Host verification: 236 HLS tests passed, plus a real independent H.264/B-frame video and AAC rolling HTTP fixture. This is not physical TV acceptance. See [subtitle policy and settings](native-hls-subtitle-policy.md).

## Default and forced policy

Manifest DEFAULT/AUTOSELECT/FORCED facts are retained separately from inspected stream dispositions. The native app has its own persisted `nativeSubtitleMode` and `nativeSubtitleLanguage`, independent of Plex account and UI language. Manual mode is the default. Automatic choices use existing cancellable preparation/revision tickets; manual track and explicit Off intent win over stale automatic results and survive retries/audio reload. Valid strict kiosks enforce saved policy while manual/remote edits remain restricted.

## Observed embedded inventory and in-band decoding

This block remains under integration and must not be advertised from parser/helper presence alone. One fully validated/fed segment can establish an observed snapshot, not a complete whole-presentation inventory. Transport identity requires validated PAT/PMT program/PID evidence, checked AVStream.id binding, codec identity, logical playback, native-worker and discontinuity ownership. AVStream indexes and dense UI ordinals are not PIDs. Changed/missing identities must never fall back to another language or same ordinal.

The legacy `media_tracks` v1 completeness contract remains strict. An explicit negotiated v2 observed-snapshot protocol is being integrated for partial/mixed catalogues, with separate evidence labels and receipt invalidation. Unsupported embedded subtitle rows remain unselectable until selected-only packet decoding, post-feed cue commit, cancellation and renderer retirement are wired and tested.

## CEA caption boundary

The FFmpeg component is `ccaption` (runtime `cc_dec`, EIA-608 codec). Its source rejects CEA-708 payload types. Adding it to a build is not caption playback: bounded GA94/A53 extraction, selected 608 service, source/epoch-safe decoder state and a playhead-aware committed screen-update renderer still need end-to-end validation. No 608 or 708 rendering capability is claimed by this work-in-progress status.

## Device acceptance

Host tests can establish catalog ownership, cancellation, clock mapping and
selection persistence. A real LG run must still establish audio language,
subtitle timing, pause/seek behavior and picture/audio synchronization against
the same package hash. No such device acceptance is claimed by this document.

## Reminder boundary

The existing reminder worker runs inside the application. It does not register a
webOS wake service or a persistent OS alarm. Background or powered-off delivery
requires a separately designed system service and firmware-specific permission
and power-management validation; changing the reminder timer alone cannot
provide it.
