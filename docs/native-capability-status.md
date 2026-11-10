# Native LG capability status

This inventory distinguishes the published `v0.3.3-beta.1` source
(`14c5af1f09d35d71af5c179338c5deeb088ffb21`) from subsequent work. A source-level
implementation or successful host test does not establish physical-TV acceptance.
The historical `parity-gaps.md` compares an earlier Plex client and is not this
application's current capability inventory.

## Published beta baseline

The catalogue dispatcher connects M3U, Xtream, Stalker, VPortal, Local and Plex.
Local supports device filesystem sources and HTTP server libraries. Provider
profiles have independent credentials and source identities. Classic and Modern
share the same catalogue, selection, playback and persistence state.

Search, category selection, unified Favorites, descriptions, logos, guide rows,
remote commands, kiosk policy, notifications and subtitle appearance settings are
connected to the runtime. Archive admission is restricted to explicit M3U catchup
sources. MAG and Xtream short guides do not advertise arbitrary day browsing.

The generic providers do not implement every branded JavaScript provider's login,
catalogue, archive or playback API. An M3U export can provide compatibility, but
must not be represented as native support for that operator's specific protocol.

Torrent discovery and playback integration expects a verified companion. The
published beta workflow did not build or stage that executable. Runtime adapter
modules alone are not an autonomous torrent engine.

Finite HLS tracks have supported routes, while embedded/live track inventories,
live WebVTT refreshing and automatic forced/default subtitle selection have
remaining restrictions. Track-menu availability must reflect actual transport
support. Burned-in subtitles and authored bitmap/ASS rendering have different
appearance capabilities.

## Subsequent integration work

- The new receipt-scoped random operation has explicit version 2 semantics;
  legacy classification-based random is a separate protocol. Controllers must
  negotiate the new capability before sending its request.
- Homebrew metadata is generated from the approved RC bytes at stable promotion.
  External catalogue admission and actual television qualification remain distinct.
- The Edem/iLook adapter must follow the current operator driver, including its
  fixed stream host and key/list/portal settings; older saved CDN/template fields
  must not override current stream resolution.
- Packaged companion delivery must bind its source, dependency inputs, ELF/link
  checks and receipt to the final IPK. Source-completeness claims require all
  enclosing runtime source obligations to be resolved.

## Platform boundaries

Reminders currently require the player and matching source to remain active.
They do not claim an operating-system wake alarm. Picture-in-picture and zoom
must be offered only when the active engine supports them. A Homebrew catalogue
entry requires external review; a CI success cannot substitute for that review.

For a candidate release record the exact commit, package hash, provider scenarios
and host/ARM/physical-TV results separately. Do not carry a device observation
forward to a different candidate merely because its version label is unchanged.
