# Native M3U media, Torrent and Stremio sources

These instructions describe the native integration source. They do not certify an installed
package, a published release, a particular TV, or compatibility with every
provider. The frozen 0.2.1 package evidence in
[the installation guide](ott-native-delivery.md) belongs to an earlier source
snapshot. Verify the installed package's source before expecting these flows.

## Before you start

A normal launch without a usable Plex session opens the OTT provider catalog;
these sources do not require signing in to Plex. This is separate from the
harness's `--guest` option, which selects a test identity and is not a setup step
for an ordinary installation.

For remote control, open **Settings → Remote control → Find command server**
once preferences are ready and access-policy checks permit it. Linux builds
can discover `_ottplay-ctrl._tcp` through the system resolver and network search
domains without a configured public catalog. This is unicast DNS-SD, not
multicast mDNS. Only a successful empty DNS result can fall back to an explicitly
configured, digest-verified public catalog; lookup errors and timeouts do not.
Builds without Linux DNS-SD support use the catalog path.

Review the displayed server address and identity, then select the intended
server, even when there is only one result. Opening Settings does not start
discovery or pairing. If discovery is unavailable, fails or finds no servers,
use manual setup; retry Find after any previous lookup has finished.

For manual setup, open **Remote control server** and save the HTTPS base address
of your command server. After the address save completes, the player discovers
servers; select one if more than one is offered. This manual path remains
available on builds with discovery as well.

Approve the displayed pairing code with your controller and wait for **Remote
control connected**. Saving an address, finding a server or seeing a pairing code
alone does not establish a durable authenticated connection. **Cancel pairing**
stops a pending attempt and preserves the previous saved connection. **Disable
remote control** is the separate action for an enabled connection. A discovery
catalog contains public server identities and addresses; it never supplies or
approves a device token.

Controllers must inspect the installed player's capabilities and setting schema.
Native uses its native decoder, so `players` is read-only. The `interfaceMode`
setting selects Classic (`0`) or Modern (`1`) through the same durable setting
as the local interface selector. Writes remain subject to the installed player's
schema and current settings policy. Shared command names do not imply that every
browser-player setting is writable or that every provider action exists.

## Remote profile activation while editing

When a controller successfully saves and activates a provider or profile, the
player closes the source editor or provider setup form that was open when the
request was accepted, provided that same editing session is still current. Its
unsaved draft is discarded. This includes a new profile's source, a Local source
field, an M3U media-link field and an open field inside provider setup.

A draft opened after the remote request, a search field or an unrelated settings
field is not dismissed by this activation. A failed save or a failure to apply
the saved settings leaves the source draft open. Catalogue metadata and status
updates alone do not close an editor.

## M3U with an attached media portal

An ordinary M3U profile can retain its live playlist and a private VPortal media
attachment. The attachment supports matching media Favorites without replacing
the parent M3U profile. It is resolved only when needed; opening a live M3U
catalog does not eagerly download the attached portal's entire catalog.

Configure the receiving player's M3U profile first. The native authenticated
`player_settings` command exposes the sensitive, provider-scoped setting
`m3uMediaUrl`. Use the normal setting transaction:

1. Select the intended enabled M3U profile on the receiving player.
2. Send `player_settings` with `op: "get"`, the current `runtime`, and
   `id: "m3uMediaUrl"`. The reply reports `configured`, not the private link.
3. Send `op: "set"` with the same setting ID, runtime, returned `context.token`
   as `context`, returned `setting.revision` as `revision`, and the complete
   VPortal link as `value`. Wait for the confirmed save response.
4. Select the matching Favorite again. A changed profile, navigation context,
   runtime or consumed revision requires a fresh `get`; do not reuse stale
   transaction tokens.

You can also edit an attachment on the player: open **Providers**, select
**M3U**, choose **Media library links…**, then select the intended profile.
Enter the VPortal link in the private editor and save it. The steps above
describe the player's authenticated command protocol; availability of an
equivalent command depends on the installed controller or CLI.
The source must be a valid VPortal link accepted by the player, such as the
shape `portal::[key:YOUR_PRIVATE_KEY]https://media.example/api/`. Keep the real
link private. A blank value removes an attachment from a live-playlist profile.

A media-only M3U slot is also supported: configure its source with a valid
VPortal link in place of the playlist URL. The normal parent-profile save
creates its private attachment and uses the portal catalog directly, without
attempting to fetch the link as an M3U playlist. This slot cannot clear its only
source to an empty link; replace the source or delete the profile explicitly.

Favorites keep the parent profile and item identity. Copying Favorites does
not transfer private portal credentials: configure the corresponding profile
and attachment on the receiving player. Missing or ambiguous bindings are
reported instead of silently selecting a similarly named item.

An attached-media Favorite can be pinned as an individual kiosk item by name
or number. An Armed kiosk can also capture that item after an explicit selection
actually starts playing. The pin keeps its M3U parent and the exact attachment
identity; parental authorization is checked against the M3U parent. On restart,
the player resolves the saved item through that attachment even if the Favorite
has since been removed from the global list or no catalog is loaded yet.

The private `nativeKiosk` item stores an `attachment` binding with
`parent_profile_id`, `attachment_id` and `source_fingerprint`. The fingerprint
covers the exact parent and attachment configuration; it does not contain their
URLs or credentials. Current runtime authority is checked separately before
lookup and again before playback. Replacing, clearing or disabling the source
invalidates the old pin instead of resolving the same item number on another
portal. After changing the source configuration, including its credentials,
explicitly pin the desired item again. An old attached-item pin without this
binding is refused; the player does not guess which attachment it meant.
This binding is private native kiosk state, not part of portable Favorites, and
does not enable mixed attached-media kiosk queues.

## Torrent and the local companion

Open **Settings → Local torrent service**. If disabled, choose **Enable**;
**Start / retry service** also starts the configured service. This requires the
matching `bin/ottplay-companion` executable in the application package. Merely
having the settings screen does not prove that the helper is installed or ready.
The helper's actual capability response determines which actions are available.

The configured cache range is **64–512 MiB**, with a default of **256 MiB**.
The supervisor reduces the requested budget when necessary to leave its
96 MiB reserve at startup, and refuses startup if less than 64 MiB remains for the cache.
Torrent playback downloads pieces into this bounded cache; it is different
from Local-folder playback, which reads existing files without importing them.
This is not a permanent offline-download library.

**Providers → Torrent** is always shown. It opens local-service setup when the helper
is unavailable or lacks the required capabilities. Browsing requires a ready helper
with Torrent and local Torznab support. From the browsing screen:

- **Add local indexer** asks for an ID, display name, Torznab endpoint and API
  key. Use the Torznab API endpoint supplied by an indexer service reachable
  from the TV, normally on your LAN. Enter the API key in its separate field;
  query strings and fragments in the endpoint, and public literal IP addresses,
  are rejected. The helper owns the credentials and validates the endpoint.
- Select the configured source, then a child indexer or **All indexers** when
  that capability is available. Choose a category or **Search**. The preferred
  result language orders matching results; it does not change a video's audio.
- Choose a result to prepare it, and select a file when the helper requests a
  choice. **Open torrent link** is the separate direct-link entry point.
- Use **Back / cancel** to leave an operation, or **Refresh / retry** after a
  reported failure. A stale result cannot select a file from a newer session.

The application owns the local helper and its session lifetime. Its loopback
stream address is not a portable playlist URL or an API to expose to the LAN.
A stopped or replaced helper invalidates its previous playback sessions.
Do not enter the remote-control server address or the helper's private playback
URL as the indexer endpoint. The indexer is a separate configured service;
starting the helper does not install an indexer.

## Stremio catalog and stream sources

Open **Settings → Stremio sources → Add source (manifest URL)**. Enter the
provider's complete URL ending in `/manifest.json`. Remote manifests require
HTTPS; HTTP is accepted only for `localhost`, `127.0.0.1` or `[::1]`.
These loopback addresses refer to the TV running the player, not the controlling
computer or a NAS. They require an actual add-on service at that address. Use
the add-on's manifest URL, not the helper's private playback URL. Query-bearing
manifest URLs are rejected. Up to eight configured sources are supported, and the complete
JSON-encoded list of manifest URLs must fit within 4096 bytes. Disabled sources
still count toward both stored configuration limits.

Open **Providers → Stremio**, select a category, then a movie and a playback
source. Catalog and stream sources serve different roles: a catalog may supply
posters and descriptions without offering playable streams. Add a stream-capable
source when the player reports that one is missing. **Search**, **Load more**,
**Retry** and **Cancel / Back** use the same native screen flow.

The optional **Configure built-in catalog service** field is a separate media
metadata endpoint whose URL ends in `/tmdb/s/`. It is not automatically the
paired remote-control server. Without that setting, configured add-on catalogs
remain the available catalog sources. **Catalog language** affects the built-in
catalog and specifically supported add-ons; it is not a universal audio-language
or add-on-translation override.

HTTP(S) playback sources use the native media path. Torrent proposals require
the ready local Torrent helper and explicit source selection. The stream adapter
rejects external-player actions, archive sources, custom-header requirements and
DRM proposals instead of pretending that the native decoder can handle them.
A source appearing in a list is not evidence that its codecs, subtitles or
transport work on a particular TV.

## Verification boundary

These are source-level integration instructions. Source regression tests and
review do not substitute for compiling the exact candidate, running those tests,
checking the packaged helper and dependencies, or exercising the media on a TV.
The desktop simulator does not establish ARM binary compatibility, firmware
symbols, mount permissions or hardware decoding. For Local USB behavior and its
separate identity limitations, see [USB folders](usb-local.md) and
[stable identity](usb-stable-identity.md).
