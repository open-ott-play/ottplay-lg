# Native menu visibility

Open **OTT → Settings → Menu items**. Select rows to stage **Visible** or **Hidden**, then choose **Save Settings**. The menus change only after the save succeeds. **Reset menu items** stages all configurable items as visible; it also requires **Save Settings**. **Cancel** discards changes that have not been submitted. Defaults are unchanged: every item is visible where the current source supports it.

The choice belongs to this installation, not to the selected provider or profile. It persists across restarts. Changing a profile, using a temporary Local/Plex queue, or opening global Favorites does not reset it.

These are installation-wide menu choices, separate from the provider-scoped channel number/name/programme/description flags under **Settings → Channel list**. They do not alter parental protection, kiosk restrictions, playback capabilities, keyboard shortcuts, or remote command permissions. **Settings**, **Back**, **Close**, and **Reset menu items** cannot be hidden. An unavailable or unverified protected settings document shows the default menus; its actions still use the normal authorization checks.

## Supported native menus

- **OTT Browse/Actions:** Providers, Profiles, Channel list, Categories, Favorites, Search (including Clear search), Description, Play, Add/remove Favorite, and Refresh.
- **OTT Settings:** Audio tracks and Subtitles and appearance. Hiding these menu entries does not disable the audio/subtitle controls already available in the player HUD or the track panel.
- **Player overflow (`…`):** Playback quality, Playback speed, and Stats for nerds. Capability checks still apply. An explicit playback-failure recovery action can open the quality page even if its normal overflow entry is hidden.

Picture/Zoom, Picture-in-Picture and archive actions are outside these visibility toggles. Native Picture/Zoom entries are offered only when the current playback exposes the corresponding capability. Main application navigation tabs, playback transport controls and track-panel tabs are outside these menu choices. The OTT Search and Description header shortcuts follow their matching menu choices.

## Remote settings

Use the existing version-one `player_settings` schema/get/set flow. Discover the descriptor, read its current revision and context, then submit an integer value with that revision and context. `1` means Visible; `0` means Hidden. Boolean `true`/`false` values are rejected.

The setting IDs are `menuShowProviders`, `menuShowProfiles`, `menuShowCatalog`, `menuShowCategories`, `menuShowFavorites`, `menuShowSearch`, `menuShowDescription`, `menuShowPlay`, `menuShowFavorite`, `menuShowRefresh`, `menuShowAudio`, `menuShowSubtitles`, `menuShowQuality`, `menuShowSpeed`, and `menuShowStats`.

All of these fields share the private `nativeMenuVisibility` document. A committed local or remote menu change, including Reset, advances its revision; editing a local draft alone does not. After a conflict, read a new revision before retrying. Do not write the private document or create settings under the public field IDs yourself. Schema discovery is paginated; follow `next_cursor` to see every field.

## Implementation and validation

The existing protected Saved writer owns persistence, readback and acknowledgement. UI clicks carry the compound revision and navigation context and are accepted only while the same menu page is current. Background catalog or programme updates do not invalidate a click. Screen ownership, parental Settings access and kiosk policy are checked before save. A pending save prevents another writer. There is no optimistic visibility update: failed writes leave the last accepted state intact. Uncertain writes require the existing settings reconciliation, and do not apply unverified hidden flags.

The native UI projects accepted settings on its UI thread. Menu construction snapshots that value; paint routines do not read mutable global state. The menu uses stable item IDs for focus after a save and the existing focus fallback when a row is hidden. Nondefault menu visibility is not represented by the current replay input format, so recording is explicitly refused in that case; ordinary playback is unaffected.

Source regressions cover strict document validation, defaults, all item roundtrips, compound revision/Reset behavior, real menu filtering, recovery entries, stale local actions, policy admission, the sole writer and failed/uncertain Saved settlement. Device behavior still requires the normal integration build and UI qualification.
