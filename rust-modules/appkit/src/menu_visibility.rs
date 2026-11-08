//! Presentation-only menu choices. No item here grants or revokes an action.
//! The app projects its accepted Saved document on the UI thread; consumers
//! snapshot it while building their menu, never while drawing it.
use std::cell::Cell;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Item {
    Providers, Profiles, Catalog, Categories, Favorites, Search, Description,
    Play, Favorite, Refresh, Audio, Subtitles, Quality, Speed, Stats,
}

impl Item {
    pub const ALL: [Self; 15] = [Self::Providers, Self::Profiles, Self::Catalog,
        Self::Categories, Self::Favorites, Self::Search, Self::Description,
        Self::Play, Self::Favorite, Self::Refresh, Self::Audio, Self::Subtitles,
        Self::Quality, Self::Speed, Self::Stats];

    /// Stable private document identifiers, independent of translated labels.
    pub fn id(self) -> &'static str { match self {
        Self::Providers => "providers", Self::Profiles => "profiles", Self::Catalog => "catalog",
        Self::Categories => "categories", Self::Favorites => "favorites", Self::Search => "search",
        Self::Description => "description", Self::Play => "play", Self::Favorite => "favorite",
        Self::Refresh => "refresh", Self::Audio => "audio", Self::Subtitles => "subtitles",
        Self::Quality => "quality", Self::Speed => "speed", Self::Stats => "stats",
    }}
    pub fn find(id: &str) -> Option<Self> { Self::ALL.into_iter().find(|item| item.id() == id) }
    pub fn setting(self) -> &'static str { match self {
        Self::Providers => "menuShowProviders", Self::Profiles => "menuShowProfiles",
        Self::Catalog => "menuShowCatalog", Self::Categories => "menuShowCategories",
        Self::Favorites => "menuShowFavorites", Self::Search => "menuShowSearch",
        Self::Description => "menuShowDescription", Self::Play => "menuShowPlay",
        Self::Favorite => "menuShowFavorite", Self::Refresh => "menuShowRefresh",
        Self::Audio => "menuShowAudio", Self::Subtitles => "menuShowSubtitles",
        Self::Quality => "menuShowQuality", Self::Speed => "menuShowSpeed", Self::Stats => "menuShowStats",
    }}
    pub fn title(self) -> &'static str { match self {
        Self::Providers => "Providers", Self::Profiles => "Profiles", Self::Catalog => "Channel list",
        Self::Categories => "Categories", Self::Favorites => "Favorites", Self::Search => "Search",
        Self::Description => "Description", Self::Play => "Play", Self::Favorite => "Add or remove Favorite",
        Self::Refresh => "Refresh", Self::Audio => "Audio tracks", Self::Subtitles => "Subtitles and appearance",
        Self::Quality => "Playback quality", Self::Speed => "Playback speed", Self::Stats => "Stats for nerds",
    }}
    /// Unknown IDs include Settings, Back, Close and Reset: never hide them.
    pub fn navigation(id: &str) -> Option<Self> { match id {
        "nav:providers" => Some(Self::Providers), "nav:profiles" => Some(Self::Profiles),
        "nav:catalog" => Some(Self::Catalog), "nav:categories" => Some(Self::Categories),
        "nav:favorites" => Some(Self::Favorites), "nav:search" | "nav:clear-search" => Some(Self::Search),
        "nav:description" => Some(Self::Description), "nav:play" | "nav:random" => Some(Self::Play),
        "nav:favorite" => Some(Self::Favorite), "nav:refresh" => Some(Self::Refresh),
        _ => None,
    }}
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Visibility(u32);
impl Visibility {
    pub fn visible(self, item: Item) -> bool { self.0 & (1 << item as u8) == 0 }
    pub fn hide(&mut self, item: Item) { self.0 |= 1 << item as u8; }
    pub fn navigation(self, id: &str) -> bool { Item::navigation(id).is_none_or(|item| self.visible(item)) }
    pub fn is_default(self) -> bool { self.0 == 0 }
}

// Menus and Saved projection run on the UI thread. A thread-local snapshot
// avoids sharing a live app's presentation with independent hostsim instances.
thread_local! { static PROJECTED: Cell<Visibility> = const { Cell::new(Visibility(0)) }; }
pub fn projected() -> Visibility { PROJECTED.with(Cell::get) }
pub fn project(value: Visibility) { PROJECTED.with(|current| current.set(value)); }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Change { Set { item: Item, visible: bool }, Reset }

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn default_preserves_every_menu_and_recovery_is_not_configurable() {
        let mut value = Visibility::default();
        for item in Item::ALL { assert!(value.visible(item)); value.hide(item); }
        for id in ["nav:settings", "nav:back", "nav:close", "menu:reset"] {
            assert!(value.navigation(id)); assert!(Item::find(id).is_none());
        }
        for item in Item::ALL { assert!(!value.visible(item)); }
    }
    #[test]
    fn stable_ids_and_remote_ids_are_unique() {
        let mut ids = std::collections::BTreeSet::new();
        let mut settings = std::collections::BTreeSet::new();
        for item in Item::ALL {
            assert!(ids.insert(item.id())); assert!(settings.insert(item.setting()));
            assert_eq!(Item::find(item.id()), Some(item));
            assert!(item.setting().bytes().all(|b| b.is_ascii_alphanumeric()));
        }
    }
}
