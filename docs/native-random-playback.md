# Random playback in the native player

Open the channel list or Favorites, then its menu and choose **Play random item**. The player selects one item from the current category and search results. It leaves the list order, provider, profiles and saved settings unchanged. The current item can be chosen again. The **Play** option in Menu items controls visibility of this menu action too.

Already discovered items can be selected while the rest of a folder or catalog loads in the background. If there are no available rows, an editor is open, settings are being committed, or playback work is suspended, the action is unavailable. An old action from another source or view is rejected. Normal parental access and kiosk restrictions still apply. If the chosen item fails admission or resolution, the player does not silently choose a replacement.

The menu action introduces no keyboard or remote-button mapping. Remote clients can use the separately negotiated receipt-scoped version 2 below. The legacy external `random` protocol has a separate classification contract and remains unsupported. Neither the internal choice nor receipt-scoped selection infers classification from names, paths, groups, duration or posters.

## Remote receipt-scoped selection (version 2)

Capabilities advertise `catalog_random: {"version":2,"scope":"receipt","request_field":"receipt_random"}` independently of legacy `random`. List categories, then channels to obtain a channel-page receipt, including category and optional search. Send `play_catalog` with params `{"catalog":"<channel-page receipt>","receipt_random":{"version":2,"scope":"receipt"}}`.

Selection is uniform over the complete receipt population, including rows beyond the current page. Discovery and snapshot must both be complete. Expired, changed, empty or category-only receipts fail. Normal playback admission, parental controls and kiosk apply; a refused item is terminal without replacement. This operation makes no classification claim and does not implement legacy search/range/mode or weighting. Clients must negotiate `catalog_random` explicitly; legacy `random` remains unsupported.
