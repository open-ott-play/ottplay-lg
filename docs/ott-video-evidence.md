# Native video evidence in OTT status

`status` and `state` add `data.video_evidence` to successful replies. Existing
empty parameters and `playback_failure_version: 2` or `3` keep their meaning;
there is no new request parameter and no change to the failure object. Older
consumers can ignore this independent object. No controller upgrade is required.

The object has exactly three required members:

- `version`: integer `1`.
- `window_present_allowed`: boolean from the application window gate. It starts
  `true`; SDL WILL/DID background events set it false and DID foreground sets it
  true. It describes permission to present the UI, not whether the panel is on,
  the application is physically visible, or video has been displayed.
- `native`: `null`, or the current attributed native observation described below.

A non-null `native` has two required members:

- `position_callbacks`: unsigned 32-bit count of accepted type-0 native position
  callbacks. This is not a count of decoded frames, shown frames, or audio frames.
- `sink_report`: `null` until a recognized sink-counter callback arrives; then an
  object containing exactly `callback_type`, `callback_count`, `firmware_major`
  and `raw_value`.

The report preserves the latest callback's numeric payload without interpreting
it as proof of a picture. `raw_value` is the exact signed 64-bit integer encoded
as a decimal string (1–20 ASCII characters), so JavaScript cannot round it. Zero
and negative reports are retained, not converted to absence. `callback_count`
counts recognized report arrivals, not frames; it is in `1..4294967295` and
saturates at the upper bound while the raw report continues to update.
`callback_type` is `47` for firmware major less than 5, or `49` for major 5 and
later. `firmware_major` is unsigned 32-bit; zero means unknown and uses the
existing older-firmware mapping. That fallback is not verified firmware identity.
A missing report means unavailable evidence, not zero displayed frames.

## Ownership and reset

The application includes native evidence only for a tagged matching OTT playback
with no replacement request pending. Missing or mismatched OTT ownership yields
`native: null`, including native playback outside this OTT owner. The media
reader independently requires the current physical Load attempt and active native
callback epoch. A retained logical identity or Engine object is insufficient.
Release, abandoned and stale epochs cannot supply observations. The application
window permission remains available independently of media ownership.

A single existing native-session lock covers callback tuple writes, the snapshot
and reset. The read does not call the sink, consume a presentation event, drain
heartbeat counters, start a worker, change playback or inspect remote devices.
No URLs, titles, file paths, credentials or private owner identifiers are included.

Counters reset on native-session reset, including a new physical Load or teardown.
An in-place seek does not reset these session counters; a seek that reloads can.
`position_callbacks` retains unsigned 32-bit wrapping behavior. Consequently,
clients must not treat decreases as playback failure or compare these samples as
lifetime counters; this contract has no public physical-session token. It does not
promise a frame rate or a reliable cross-Load delta. A repeated last sink value may
simply mean that no further callback has arrived.

## Verification limits

Host tests exercise callback ingress, exact ownership rejection, reset,
saturation, JSON string precision, passive reads, the actual SDL permission gate
and status parsing. They do not establish physical display, decoding correctness,
audio output, panel power or video motion. Those require separate device evidence.
