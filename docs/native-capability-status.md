# Native LG capability status

This inventory describes the 0.4.23 source candidate. It is not physical-TV
acceptance or a stable-release qualification. The historical `parity-gaps.md`
compares an earlier client and must not be used as the current product inventory.

## Shared user experience

Classic and Modern render one catalogue, playback, selection and persistence
state. A fresh installation defaults to Classic, matching the PC client. An
explicit saved choice of Classic or Modern is retained.

- Channel rows carry numbers, source logos, current-programme information,
  progress and archive availability when those data exist. Missing guide data
  must not be replaced by invented programmes.
- Categories include global Favorites. Category restoration is bound to the
  profile and source identity; temporary playback does not replace that saved
  choice. Favorites can resolve across configured sources.
- Search and Add/Remove Favorite use the same guarded actions in both modes.
  Classic exposes them in its contextual footer. A measured narrow layout keeps
  overflow actions accessible through Browse rather than drawing overlapping
  controls or inventing unsupported remote shortcuts.
- Fullscreen Up reveals the information footer, then expands available programme
  information. Physical CH keys, numeric tuning, Back, desktop volume and modified
  seek have separate input contracts. PC shortcuts do not imply equivalent TV
  scancodes.
- Settings, descriptions, notifications, parental admission, track selection and
  kiosk controls use runtime capabilities and source ownership, not renderer
  policy. See `native-settings.md` for supported settings and import boundaries.

## Playback ownership and recovery

HLS terminal settlement and unrecovered source-read failures require their exact
native playback epoch. Their details, transport diagnostics and terminal flags
are published under the same epoch lock; a retired completion at these boundaries
cannot fail a replacement session.

Live HLS retries remain bounded and target the original selected channel. A
trusted segment-availability or body-transport failure may qualify for that
existing policy; authentication refusals, malformed media and arbitrary error
text do not. Cancellation, pause, source replacement and admission restrictions
retain priority. Playback failure does not authorize advancing to another live
channel. Completed catalogue reuse after Back requires matching source and
generation proof.

An optional `command_activity` field in a status response records the last eight
selected remote command families in RAM, with local sequence, age and local
completion status. Status reads do not evict evidence. Parameters, raw request
IDs, source addresses, filenames and user-entered text are absent. The records
reset when control authority changes or is erased. This diagnoses incoming
requests; it does not identify a human actor, acknowledge server delivery, prove
playback occurred, or account for local UI/internal actions. No persistent audit
log or background network request is added.

## Providers and platform boundaries

The dispatcher includes M3U, Xtream, Stalker, VPortal, Local, Plex and the named
operator adapters in the current provider inventory. Device filesystem sources
and HTTP server libraries are distinct Local transports. Branded login,
catalogue and archive support must be checked per adapter, not inferred from an
M3U export. Archive admission requires an explicitly supported catchup source.

System television and LG Channels use the controller bridge and its permissions;
they are not decoder streams. Their physical acceptance is separate from host
tests. Torrent playback requires a verified companion bound to its source and
package receipt; adapter presence alone is not autonomous engine delivery.

Finite/live track menus reflect the active transport's actual inventory and
switching support. Burned-in text and authored bitmap/ASS subtitles have different
appearance capabilities. Native PiP has no verified backend and is not advertised
as a working action. Reminders require an active player and matching source; they
do not promise operating-system wake alarms.

Full equivalence is not yet certified. Browser buffer/recovery preferences and
legacy manual-lock import have platform/migration gaps documented in the settings
inventory; unsupported keys must not be described as applied. Actual EPG feed
coverage, Magic Remote response, sustained decoding, picture/sound synchronism and
error recovery require later acceptance on the exact television package.

## Verification record

Source tests cover real screen dispatch, saved interface choices, category and
Favorites ownership, input and footer behavior, retired-producer failure fencing
and bounded HLS transport classification. Host tests, ARM ABI checks, a compiled
IPK and physical-TV observations are separate evidence classes. Record the exact
commit, package hash, build profile and tested scenarios for each delivered
candidate. Do not carry an older package's acceptance into a new version.
