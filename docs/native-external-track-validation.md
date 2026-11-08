# Native external-source track validation

The native player uses the same decoder for Plex and independent Local, M3U,
and VPortal sources. Track-selection evidence must cover independent sources:
a Plex-only menu test and a pipeline test with a preselected audio codec do not
prove that their track inventories reach the shared picker.

## Caption controls and ownership

The shared Style page offers these finite, validated choices for plain text:

- Font: bundled Inter Bold (default) or Inter Regular. Script-specific fallback
  fonts remain automatic; this does not load arbitrary fonts.
- Size and position: the existing five size and three position choices.
- Text color: white (default), yellow, green, cyan, blue, magenta, or red.
- Outline thickness: 0, 1, 2 (default), 3, or 4 logical pixels.
- Black background opacity: 0% (default), 25%, 50%, 75%, or 100%.
- Brightness: the existing white-to-charcoal tone multiplier, separate from
  text color. The prior Color row is now named Brightness.

`media::player::text_style::TextAppearance` carries font, color, outline and
background. `TextStyle::current(appearance)` receives that appearance explicitly;
no new process-global appearance setting is read by the renderer. The menu reads
`route::subtitle_text_appearance(&PlaybackSession)` and emits
`TrackCommit::TextAppearance` with a complete proposed value. It keeps the last
acknowledged checkmark until the owner publishes a saved value. Busy and a
confirmed SaveFailed outcome offer an explicit Retry without claiming success.
SaveUncertain latches further writes until reconciliation or restart; it does not
offer Retry. Stale or unavailable playback also has no Retry action.

The application integration contract stores this appearance install-wide as
`nativeSubtitleAppearance`, for Plex and external providers alike. Existing
Plex profile size, position and brightness preferences keep their own ownership.
External playback uses `nativeSubtitleSize`, `nativeSubtitlePosition` and
`nativeSubtitleTone` only when those keys exist; entering Plex must not overwrite
its profile preferences with external values. Returning to Plex restores only
those three fields from the active profile mirror, not unrelated quality or
audio settings. The menu reads external geometry and brightness through
`route::native_subtitle_preferences(&PlaybackSession)`, an optional owned
`NativeSubtitlePreferences { size, position, tone }` snapshot. Its checkmarks
change only when the owner publishes saved values; it does not poll global
preferences each frame. Plex retains its existing preference behavior.
Persistence and restart tests below must verify this contract.

Authored ASS/SSA and bitmap subtitles retain their own appearance. Their font,
size, position, text color, outline and background controls are disabled with an
explanation; brightness remains a multiplier over their existing pixels. Burned
subtitles cannot be restyled by the client. A track being marked Supported means
that its transport and shipped renderer admit the operation, not that playback
has already succeeded on a particular television.

The shared Audio/Subtitles picker reads an external source's `ExternalTracks`
catalog before any Plex metadata. Rows show the language, supplied title, codec
and track number; audio rows also show the known channel count. Unsupported
tracks remain visible but disabled with an availability explanation. A playing
transport default can therefore remain checked even when switching it is not
supported. Subtitle Off is offered with supported subtitle choices; Timing and
Style appear for an admitted, selected client-rendered subtitle.

`TrackCommit::External(ExternalTrackChoice)` carries an opaque catalog-bound
choice to `route::apply_external_track`. It never manufactures a Plex stream ID
or uses a Plex selection request. The menu keeps the acknowledged selection
until the owner publishes the new one, and a same-catalog update preserves focus.
A new catalog invalidates pushed pages. Even reopening the same address creates
a new identity; a stale menu choice must be rejected. Unknown disposition flags
must not become Original or Forced-only claims. Independent multiplexed H.264/AAC
MPEG-TS HLS permits an unambiguous embedded AAC choice. A descriptor is frozen
with the source and worker; each segment must retain the same audio descriptors,
although stream order may change. Missing, ambiguous or changed tracks stop the
stream rather than switching to another language. Reopen the source to inspect a
changed track table. Remote enumeration of this embedded inventory remains
unavailable because one segment is not a complete presentation catalog. Finite
VOD with supported external AUDIO renditions instead exposes the retained active
master audio group through the native menu and the existing `media_tracks` API,
as described in [HLS audio renditions](native-hls-audio-renditions.md). An otherwise
supported audio/video master may declare subtitle groups without blocking playback.
The verified separate-rendition TS VOD route also offers manual external WebVTT
choices through its presentation catalog, not the embedded AVStream inventory;
see [HLS WebVTT subtitles](native-hls-webvtt.md). Unselected subtitle playlists
are not fetched. Embedded HLS subtitles, ordinary multiplexed HLS subtitle groups
and automatic default/forced subtitle selection remain unavailable. No HLS
selection is redirected into a Plex operation.

## Small synthetic fixture

Generate outside the checkout, without a media server or account:

```sh
python3 tests/test_external_tracks_fixture.py
python3 tests/fixtures/make_external_tracks_fixture.py --out /tmp/ott-external-tracks
```

The generator requires host `ffmpeg` with `libx264` and AAC encoding, and
`ffprobe`. It reuses the suite's authored SRT and PGS generators. It writes two
12-second files and `external-tracks-receipt.json`; generated media is not
committed. The moving 320×180 test pattern keeps the encode small.

`external-tracks-text.mkv` deliberately interleaves stream kinds:

- Stream 0: H.264 video.
- Stream 1: audio ordinal 0, AAC mono/48 kHz, `eng`, `English 220 Hz`, default.
- Stream 2: subtitle ordinal 0, SubRip, `eng`, `English SDH`, hearing impaired.
- Stream 3: audio ordinal 1, AAC mono/48 kHz, `spa`, `Spanish 880 Hz`.
- Stream 4: subtitle ordinal 1, SubRip, `rus`, `Russian forced`, forced.

`external-tracks-bitmap.mkv` carries the same video, audio ordinal 0, and one
English PGS stream. Its subtitle canvas is 640×360, deliberately different from
the video raster. The bitmap appears at 0 seconds and clears at 8 seconds.

The receipt records actual stream indexes, dense per-kind ordinals, codec,
language, title, disposition, channels/rate, duration, file size and SHA-256.
Verification decodes the audio and measures its two tones, extracts both text
tracks and checks all three cues at 0/4/8 seconds, and decodes the PGS display
and clear into one rectangle followed by zero rectangles. It does not infer
content from container labels. Hashes identify this run's outputs; different
FFmpeg versions need not produce identical container bytes.

Disposition flags in this receipt are independent `ffprobe` truth. A native
inspector without a verified disposition accessor must report them as unknown;
it must not infer forced/default from a title or claim the flag is false.

## Focused regression gates

These are acceptance conditions, not a claim that an unexecuted gate passed.
Use actual shared route/picker entry points, not a parallel test implementation.

1. **Enumeration through the shipped demuxer.** Inspect the text file using the
   native `ff::inspect` entry point with the host build of bundled FFmpeg. Assert
   two audio and two subtitle rows, language/title equality, and mappings
   `(stream 1, ordinal 0)`, `(3, 1)`, `(2, 0)`, `(4, 1)`. A video-only/no-audio
   file must still be accepted. A stale/cancelled inspection must not publish
   tracks into a newer source. The old first-audio-only inspection is the RED
   baseline; a system-FFmpeg receipt alone cannot turn this gate green.
2. **Independent-source menu.** Admit an external source with that inventory
   through `route::request_external_source`, then open the shared track picker.
   Run once with no Plex playing metadata and again with unrelated old Plex
   metadata. It must list only the external source's rows, retain both labels,
   expose subtitle Off, and use dense kind ordinals rather than stream indexes.
   The baseline picker reads only `MetadataView::playing()` and cannot satisfy
   this condition by receiving new inspector fields alone.
3. **Selection retains source and position.** Starting at 5 seconds, commit
   audio ordinal 1 through the real picker-to-playback dispatch. Assert the
   canonical external source, OTT selected entry and play generation remain
   the same; the native reload target is the sampled position, not zero.
   Repeat while paused and require pause to remain. An unsupported selected
   codec returns a refusal and preserves the current valid selection/route.
   No Plex request, watch-history write or new catalog resolution is permitted.
4. **Subtitle select and Off do not reload media.** Commit text ordinal 1 then
   Off through the same dispatch. Assert render ordinal 1 then disabled, the
   same source/position and no audio/video restart. Replace the source and
   reject a stale commit from the previous source's inventory generation.
5. **Capabilities describe the actual renderer.** Exercise every implemented
   plain-text Style field and verify the generated header and rendered result.
   ASS and bitmap choices must remain inert for text-only fields, including a
   stale direct action after the renderer changes. Check brightness separately.
   All ten locale forms must fit and remain navigable. The new menu regression
   `text_appearance_picks_wait_for_acknowledgement_and_keep_brightness_separate`
   checks proposed values, retry after no acknowledgement, and checkmark changes
   only after the owner publication.
   `native_geometry_and_tone_wait_for_owner_acknowledgement_and_allow_retry`
   exercises the same acknowledgement boundary for external size, position and
   brightness, including a retry after the owner keeps its previous values.
   Record results against the exact build being evaluated rather than treating
   the tests' presence as a passing result.
6. **Preferences survive restart without crossing owners.** Save appearance
   while playing both Plex and an external source, restart, and compare the full
   saved value. Exercise Busy, persistence failure and rapid successive requests.
   External size/position/tone must not overwrite a different Plex profile's
   values; absent external keys must not reset an existing preference. Switch
   External → Plex → External and preserve unrelated quality/audio choices.

The external picker tests in `appkit/src/external_track_menu.rs` cover real
source admission without Plex metadata, stale Plex metadata isolation,
same-catalog audio acknowledgement, subtitle Off and renderer capabilities,
reopening the same source with a fresh identity, qualified embedded AAC
selection, and refusal of unsupported HLS selections.
The fixture settles only the pure controller; it does not open media files,
start a native decoder or claim hardware playback success.

Existing pure coverage to reuse includes
`media/src/route/plan_track_selection_tests.rs`,
`data/src/metadata/track_label.rs`, `appkit/src/track_menu.rs`'s renderer-kind
style tests, and `screens/src/player/overlay_tests.rs`'s real track-commit effects.
The application `ott_runtime_tests` fixture provides an inert real App/Runtime
for source replacement and late-result tests without network or private state.
Host demux/ASS tests in `media/src/ff_image_subtitle_tests.rs` and
`ff_subtitle_charset_tests.rs` require the bundled host libraries and are
explicitly ignored by the ordinary host suite when those libraries are absent.

Existing larger generators remain useful for extended coverage:
`movie_h264_ac3_many_audio` has eight differently labeled/codeced audio tracks;
`movie_h264_ac3_1080p` has four text tracks; `make_ass_fixture.py` makes two
styled tracks plus fonts; `make_text_stress.py` covers multilingual text and
timing edge cases. The checked-in `mkv_zlib_subs.mkv` exercises compressed and
uncompressed PGS/VobSub decoding, but contains no video and is not by itself an
external playback fixture.

## Device handoff, not executed by fixture generation

After the owner grants a TV slot, follow the repository's TV-lock and guest/mock
workflow. Use only these synthetic files via a Local source or a controlled
fixture HTTP server. Do not substitute a household channel or Plex account.

- Open the text file and confirm both audio and subtitle labels. At about
  5 seconds select Spanish: the pitch changes from 220 Hz to 880 Hz, the same
  test pattern remains, and playback continues near 5 seconds rather than
  restarting. Reopen the file for the paused variant: switch while paused,
  ensure it stays paused, then resume and verify the new tone.
- Select each text track and Off. Read the on-screen `S0 English` or
  `S1 Русский` cue. Off removes it without interrupting sound or picture.
  Exercise font, size, position, text color, outline, background and brightness
  independently, and photograph both the panel and the resulting text.
- Open the PGS variant, select its only subtitle, inspect its scale and clear
  at 8 seconds. Geometry controls that do not apply to bitmaps must not imply
  a working font resize or text recoloring. Repeat the capability check with an
  authored ASS fixture. Reopen if necessary; the short fixture is intentional.
- Replace Local with M3U or VPortal referring to the same fixture. Confirm no
  old track row survives, then repeat select/Off. For an HLS-specific claim,
  supply a separately verified alternate-rendition HLS fixture: an MKV over
  HTTP does not exercise that path.

Record build identity, source kind, fixture SHA-256, selected kind/ordinal,
before/after position and paused state, menu captures and panel observation.
Host checks establish metadata and lifecycle behavior; they cannot prove LG
audio routing, video-plane composition or native subtitle rasterization.
