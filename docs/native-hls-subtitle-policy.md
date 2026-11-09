# Native HLS subtitle selection

The native player keeps its own subtitle selection policy. It does not inherit Plex account preferences or interface language. The default is manual selection.

## Settings and persistence

`nativeSubtitleMode` is an application-scoped, readable/writable string: `off` (manual, default), `auto`, or `forced`. `nativeSubtitleLanguage` is an application-scoped string, default empty (manifest preference). The local **Native subtitles** group edits these values through the same validated save path as remote commands.

Language values are a bounded language-tag subset: at most 35 ASCII bytes, first component two or three letters, subsequent hyphen-separated components one to eight ASCII letters/digits. Examples: `en`, `eng`, `pt-BR`, `zh-Hant`. Empty is valid. Unknown fields, controls, underscores, empty components and oversized values are rejected. Common local picker languages are shortcuts; a valid custom remote language remains stored.

Both fields share internal durable `nativeSubtitleRevision` with subtitle appearance/size/position/tone. No Saved schema change is required. The wire revision is an opaque receipt token; clients must not construct it from the internal numeric counter.

## Remote contract

Use the existing `player_settings` action and current player runtime identity. Schema discovery uses `params: {"op":"schema","runtime":"<runtime>"}` with existing pagination. Get one field with:

```json
{"action":"player_settings","params":{"op":"get","runtime":"<runtime>","id":"nativeSubtitleMode"}}
```

The reply includes the descriptor, `context.token`, and `setting.revision`, plus persisted/effective values where applicable. Set using those exact returned tokens:

```json
{"action":"player_settings","params":{"op":"set","runtime":"<runtime>","id":"nativeSubtitleMode","value":"auto","context":"<context.token>","revision":"<setting.revision>"}}
```

For language use `id: nativeSubtitleLanguage` and e.g. `value: en`. Read a fresh receipt before each write. A successful mode, language or geometry edit invalidates outstanding receipts in their shared revision domain. On `revision_conflict`, `runtime_changed`, or `source_changed`, rediscover/get again before deciding whether to retry. Do not blindly replay a possibly completed write. Settings descriptors advertise `apply: immediate`; current manual choices retain precedence.

## Selection precedence

Auto considers declared DEFAULT/FORCED tracks or AUTOSELECT tracks matching the preferred language. Forced mode considers only explicitly FORCED tracks. Ranking is exact language tag, normalized language/primary subtag (including ISO639 aliases), DEFAULT, AUTOSELECT, then manifest order. Invalid or unsupported rendition references are excluded. Metadata flags are not guessed from names.

A per-logical-playback intent distinguishes untouched, automatic, manual-track and manual-Off states. A manual choice supersedes a pending automatic result and survives refresh, retry and audio reload. A fresh logical source resets item intent and applies the saved policy. Changing a global preference does not override the current item's manual choice. Auto-to-Off clears an automatically owned selection. Valid strict kiosks enforce saved policy while continuing to block manual/remote track changes.

## Live transport and validation scope

Live multiplexed MPEG-TS subtitles use the committed decoder PES clock, HLS discontinuity epoch and session identity. Rolling refresh is bounded, reuses overlapping segment bodies, and rejects stale epochs/cancelled sources. Live external AUDIO groups use paired rolling H.264/AAC MPEG-TS playlists with precise explicit program-date-time anchors per epoch, persistent packet clocks and proven AAC coverage before publishing each video interval. Audio/video media sequence numbers are independent. Missing timing evidence, incompatible epochs, mutated overlapping segments, gaps, unsupported formats and bounded epoch-acquisition timeouts fail closed. A shared first-output timestamp offset keeps the native video/audio/subtitle timeline near zero without changing relative timing. Encryption, fMP4 maps and byte ranges retain their existing restrictions.

Host validation includes the HLS suite, policy/intent tests, bounded HTTP refresh fixtures and a real bundled-FFmpeg H264/AAC TS inspect-to-decoder-to-route-to-ASS test. These checks are not physical television acceptance.

Paired live validation also includes five focused tests (including a real HTTP publisher that advances in the wrong epoch without extending its deadline) and a real independent H.264/B-frame video plus AAC audio rolling-window test. The latter uses unrelated sequence numbers and verifies refreshed playlists, first native timestamp zero and preserved audio/video lead.
