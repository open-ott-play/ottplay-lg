# Random playback in the native player

Open the channel list or Favorites, then its menu and choose **Play random item**. The player selects one item from the current category and search results. It leaves the list order, provider, profiles and saved settings unchanged. The current item can be chosen again. The **Play** option in Menu items controls visibility of this menu action too.

Already discovered items can be selected while the rest of a folder or catalog loads in the background. If there are no available rows, an editor is open, settings are being committed, or playback work is suspended, the action is unavailable. An old action from another source or view is rejected. Normal parental access and kiosk restrictions still apply. If the chosen item fails admission or resolution, the player does not silently choose a replacement.

This action is internal to the native menu. It introduces no keyboard or remote-button mapping and no new remote protocol capability. The existing external random protocol has a separate legacy classification contract; it remains unsupported by this implementation. The internal choice does not infer any classification from names, paths, groups, duration or posters.
