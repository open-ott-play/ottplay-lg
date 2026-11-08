# Native global Favorites

Favorites is one collection per installed client. The Favorites category mixes configured M3U, VPortal, Plex, and Local sources. Changing a provider, profile, category, or temporary Local/Plex session does not replace that collection. A remembered Favorites selection reopens the list even when no persistent profile is active; unavailable entries stay visible and are checked when explicitly played.

To leave Favorites, open **Categories** and select **All** or a source category.
The selection takes effect after a successful save and does not delete Favorites.
When these menu controls are enabled, an empty unfiltered Favorites list
initially focuses **Categories**; an empty search result focuses **Search** so
the filter can be changed.
In the Classic **Favorites** list, **Yellow** invokes the highlighted item's
available Add/Remove Favorite action. In a regular channel list, it opens the
available **Actions** menu instead.

On the native Mac player, press **F** over the unobstructed video picture to add or remove the item actually playing when it has a supported portable Favorite identity. With the video controls hidden, the desktop input adapter consumes only a fresh, unmodified physical key press; holding the key does not repeatedly toggle it. This shortcut is delivered together with the desktop-controls adapter. Visible video controls and menus retain their existing navigation. Other platforms keep their existing controls. In the channel list, the existing Add/Remove Favorite action and held-OK action remain available for items with a supported portable identity. Torrent files can be selected and played, but cannot currently be added to durable Favorites.

A short notice confirms a successful change only after the protected settings writer confirms it. A failed write does not change Favorites. The notice uses the existing transient notice surface, does not open or extend the transport footer, and cannot replace a confirmation dialog. Leaving the player or changing the native playback identity suppresses an old completion notice.

The fullscreen shortcut uses the admitted playing item rather than whichever row or provider was last browsed. Parental and kiosk restrictions still apply. An attached portal retains its real parent profile; Local media retains its exact live source binding. No shortcut changes the active provider, starts another video, or imports a filesystem permission.

Portable Favorites preserve source/profile identities and media paths or provider item identities. A receiving player needs equivalent profiles and its own source access. Importing Favorites does not transfer passwords, Local filesystem grants, or a private root-file proof. Missing or ambiguous sources stay unavailable until configured; the player does not guess a source from a title or basename.

## Remote list, playback and import

The native player exposes the existing portable collection through `favorites_global_items` version 1. `list` returns bounded pages from the whole collection, including unavailable sources; `current` captures a supported portable identity for the admitted playing item without changing Favorites; a playing item without that identity returns `entry:null` with `no_portable_current_item`. `play` resolves one receiver-issued ID under the listed document and context revisions. A successful dispatch response does not prove that video frames or audio have reached the device.

Receiver-local UUIDv4 metadata is saved alongside the existing document. IDs survive title and order changes while the same portable identity remains present. Imported portable documents do not carry another receiver's IDs or filesystem permissions. Reads do not allocate or save IDs. Existing collections acquire missing IDs through the ordinary guarded load/write paths. A refused or uncertain migration retains the readable original collection; uncertainty continues to block writes.

`favorites_global_recovery` version 1 supplies read-only evidence before a controller starts an import. A complete clean journal can pass that prerequisite, but the normal import admission and durable-write checks still apply. Unresolved operations remain visible and block new mutations. Acknowledgement is intentionally unsupported in this native profile; it must not be simulated by clearing a receipt or replaying its original write.

See [Native Favorites protocol storage profile](native-favorites-protocol.md) for the exact storage hashes, error distinctions and qualification boundaries.
