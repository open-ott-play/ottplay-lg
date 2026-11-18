# Using the app

Installation, first-run behaviour and the settings a television owner changes
directly in OTTPlay LG (PlxNative).

- [Install Plex on LG webOS TV with PlxNative](../install-and-verify.md) — Developer Mode installation on a retail set, without root.
- [Installation and acceptance](../ott-native-delivery.md) — matching a package to its source revision through executable and package checksums.
- [Compatibility reports](../compatibility.md) — the televisions owners have reported this running on, and the known failures.
- [Classic and Modern native migration](../port-plan.md) — the migration plan towards one shared experience rendered with SDL.
- [Two installs on one television](../two-installs.md) — the stable and separate installation identities, written before the nightly flavour landed.
- [Nightly builds](../nightly-builds.md) — the build published beside the regular app with its own launcher tile and sign-in.
- [Distribution: what it takes to ship this publicly](../distribution.md) — dated research on public distribution, retained from before the GPL-3.0-or-later licence.
- [Installing an OTTPlay IPK with the LG webOS CLI](../lg-cli-install-troubleshooting.md) — the working steps for a Developer Mode install and the CLI issue seen with 0.3.3.
- [Troubleshooting](../troubleshooting.md) — what to do when the tile does nothing or playback fails.
- [OTTPlay Native for macOS](../macos-app-readme.md) — opening the shared-core desktop build for the first time.
- [`OTTPlay Native.app`](../macos-app.md) — the self-contained macOS bundle `make macapp` produces.
- [Classic list and numeric playback settings](../classic-list-settings.md) — choosing Classic and the numeric playback preferences.

## Sources

- [Native M3U media, Torrent and Stremio sources](../native-media-sources.md) — the source types the native integration accepts.
- [VPortal in the native player](../vportal-categories.md) — the VPortal source with folder categories.
- [System television channels](../system-television.md) — the LG scanned channels and system player, which OTTPlay does not decode.
- [USB folders in the native player](../usb-local.md) — explicit removable-folder selection for the Local provider.
- [Optional stable USB filesystem identity](../usb-stable-identity.md) — the identity evidence used to reattach a folder.
- [Local artwork, descriptions and subtitles](../local-assets.md) — optional sidecar assets beside a Local video.
- [Explicit metadata for user-prepared Local files](../local-metadata-sidecar.md) — the optional `.ottplay/metadata.json` reader.
- [Native hosted EPG fallback](../native-hosted-epg.md) — the configured guide service used when a playlist advertises no XMLTV.
- [Programme guide, archives and reminders](../ott-guide-and-reminders.md) — the guide, archive routes and reminder implementation.

## Settings and collections

- [Native LG settings](../native-settings.md) — the `ott PLAYER settings schema` transport and its 45 entries.
- [Native menu visibility](../native-menu-visibility.md) — staging visible or hidden menu rows and saving them.
- [Native global Favorites](../native-favorites.md) — the single installation-wide Favorites collection across providers.
- [Native Favorites protocol storage profile](../native-favorites-protocol.md) — read-only recovery inspection of the protected Favorites state.
- [Native multi-view integration](../native-multiview.md) — the experimental layout and what geometry checks do not establish.
- [Editing saved provider profiles](../native-profile-editing.md) — editing a profile row without loading it.
- [First native connection and Home preset](../native-first-connection.md) — pairing the app with a command server and approving the receiver.
- [Random playback in the native player](../native-random-playback.md) — selecting from the current category and search results.
- [Home Assistant notifications](../home-assistant-notifications.md) — receiving banners through a paired OTT controller.
- [Modern provider browsing](../modern-provider-browsing.md) — provider, profile and category choices as native collection cards.
- [Search](../search.md) — the Search screen and the research into the television's on-screen keyboard.
- [Launch links](../launch-links.md) — opening a Plex title from another app through launch parameters.
- [Watchlist](../watchlist.md) — the plex.tv Watchlist as the shipped code implements it.
- [Watchlist — implementation plan](../watchlist-plan.md) — the approved plan and its appendices behind that implementation.
- [UX Scenario — PlxNative](../ux-scenario.md) — the 2026-08-23 walkthrough of every behaviour, read out of the source.
