# VPortal in the native player

This guide describes the native source with the folder-category integration applied. Use the qualification receipt for the actual installed build; source changes and test definitions alone do not establish that a released package contains or runs them.

## Browse films like channels

Configure a VPortal profile with the link supplied by your provider. Treat the link as private: it may contain the account key. Open the profile's Channel list to browse playable films and episodes. The list fills as the provider pages arrive; it need not wait for the complete library before presenting the first playable entries.

Use **Category selection** to filter the same list by a provider folder. **All** shows the collected playable entries. A parent category includes the entries in its descendant folders: selecting `Films` also includes an item in `Films / Comedy`. A discovered empty folder remains a category with no playable rows; it does not become a fake channel. Folder names are labels, so a slash inside a real folder name does not create an extra parent.

Remote controllers use [immutable catalogue snapshots](remote-catalog-snapshots.md) to keep category and channel selection consistent while discovery continues.

The library is still being discovered while later pages arrive. An incomplete scan is not evidence that a category is empty or that every file has been collected. Cancellation or a provider error must remain distinct from successful completion.

## Descriptions and pictures

For a film or episode, the information comes from VPortal rather than a television EPG. The source carries the supplied description, artwork, title, year and duration. Episodes can inherit missing metadata from their parent title. Recognised formatting markup is converted to plain text. The preview starts at the source-owned synopsis boundary, while **Description** retains the generated facts and full admitted text. See [Modern provider browsing](modern-provider-browsing.md) for navigation and verification details.

A picture requires both a supplied artwork address and a successful image fetch. A screenshot of an empty picture area does not establish that the provider omitted it. Native UI display and image loading must be checked on the exact build in addition to the parser tests.

## Playback and saved items

When an item supplies a provider request, playback resolves it into a current media address; items supplied as direct media URLs use those URLs. The profile link and provider credentials remain in the source layer; displayed folder names do not authorize access. VPortal is a network source, unlike a Local folder on an attached drive.

This category change preserves existing item IDs and portable Favorites identities. It does not merge two otherwise identical entries published through different folders. Existing folder-scoped Favorites must continue to refer to that folder. Conflicting entries with the same identity are rejected rather than silently selecting the first one.

## Choose a playback quality

When the complete native quality feature is installed, choosing **Play** for a VPortal item with multiple valid named variants opens the standard quality list in both Classic and Modern. A single usable source starts without an extra selector. Select the required variant, or use **Back** to cancel.

The selected provider key is saved for that profile and source. Playback starts only after the preference is confirmed saved. A failed save remains an error rather than starting from an unconfirmed preference. Leaving while a save is already being written can preserve the preference, but cannot restart the cancelled video.

Automatic continuation, resume, random playback, remote playback and kiosk operation do not open this interactive selector. They use the stored exact provider key when it is available, otherwise the provider's default source. Changing the profile's source invalidates its old preference. This setting does not change item IDs or Favorites identities.

The selector admits at most 16 playable named variants, with each key limited to 256 UTF-8 bytes and no control characters or URL labels. An oversized or invalid interactive response is rejected. These are protocol limits, not a guarantee about arbitrary provider-side resources: cleanup covers admitted variants, the selected/default source and validated top-level cleanup control, not unadmitted allocations from a malformed response.

The quality backend and UI form a separate integration from the folder-category patch. A categories-only build does not provide the selector. Verify the actual candidate and both UI modes before documenting an installed build as quality-enabled.

## Pre-play diagnostics

In builds containing this diagnostic extension, authenticated `state` and `status` replies include `data.preplay` with `version: 1`. Older builds may omit it. This field observes the current or most recently completed instrumented VPortal attempt. It is independent of catalogue loading and of the existing `ready` and `playback` fields.

The object contains `phase`, nullable `attempt_generation`, `phase_elapsed_ms`, nullable `last_failure`, and nullable `cancel_reason`. Phases are `idle`, `resolving`, `inspecting`, `awaiting_quality`, `admitting`, `admitted`, `failed`, and `cancelled`. `idle` has no attempt generation and zero elapsed time. The elapsed value measures time since the application accepted the current phase; terminal records freeze the duration of the phase that ended. It is observation time, not a deadline or progress percentage.

`admitted` records acceptance by the native-start boundary. Use the existing playback state to determine whether playback is running; this diagnostic alone does not establish decoded audio or video. A `quality_save_failed` failure can remain attached to `awaiting_quality` while the existing selector permits retry. Failure and cancellation fields use fixed codes; the preplay object contains no source URL, credentials, media title or raw error message.

## Verify an installed build

Use a small test portal with a parent, a nested folder, an empty folder, a long description and an image. Confirm that the parent includes the nested item, the empty folder has no fake video, the full Description reaches the end of the supplied text, and the image is actually visible. Then verify playback and Back/cancel while another page is pending.

The prepared Mac fixture contains synthetic media and description/image sentinels. It is not a real provider account and does not prove LG decoding or firmware behavior. The previous fixture expectation used direct leaf categories only; after this patch its expected category membership must explicitly include descendants. Do not treat that older expectation as a passing check of the new behavior.
