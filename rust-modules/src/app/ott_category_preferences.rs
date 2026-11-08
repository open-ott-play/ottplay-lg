//! Pure category preference projection for the existing OTT Saved image.
//! Global Favorites selection stays in Saved.favorites_selected; this module
//! owns no writer and never changes the Favorites document or playback owner.
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

const KEY: &str = "nativeCategoryPreferencesV1";
const PROFILE_COUNT: usize = 90;
const CATEGORY_BYTES: usize = 4096;
const ENCODED_BYTES: usize = 512 * 1024;

#[derive(Clone, Copy)]
pub(super) struct Source<'a> {
    pub profile_id: &'a str,
    /// The caller supplies the existing profile_fingerprint, never a URL/path.
    pub fingerprint: &'a str,
}
impl Source<'_> {
    fn valid(self) -> bool {
        !self.profile_id.is_empty()
            && self.profile_id.len() <= 128
            && self
                .profile_id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"-_:.".contains(&b))
            && self.fingerprint.len() == 64
            && self
                .fingerprint
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Entry {
    fingerprint: String,
    category: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Preferences {
    version: u8,
    profiles: BTreeMap<String, Entry>,
}
impl Default for Preferences {
    fn default() -> Self {
        Self {
            version: 1,
            profiles: BTreeMap::new(),
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Selection {
    /// Remembered choice, not necessarily present in a partial/current catalog.
    pub category: String,
    pub favorites_selected: bool,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Error {
    Invalid,
    TooLarge,
}

impl Preferences {
    pub fn read(settings: &BTreeMap<String, Value>) -> Result<Self, Error> {
        let Some(value) = settings.get(KEY) else {
            return Ok(Self::default());
        };
        if serde_json::to_vec(value).map_err(|_| Error::Invalid)?.len() > ENCODED_BYTES {
            return Err(Error::TooLarge);
        }
        let parsed: Self = serde_json::from_value(value.clone()).map_err(|_| Error::Invalid)?;
        parsed.validate()?;
        Ok(parsed)
    }
    fn validate(&self) -> Result<(), Error> {
        if self.version != 1 {
            return Err(Error::Invalid);
        }
        if self.profiles.len() > PROFILE_COUNT {
            return Err(Error::TooLarge);
        }
        for (profile_id, entry) in &self.profiles {
            if !(Source {
                profile_id,
                fingerprint: &entry.fingerprint,
            })
            .valid()
            {
                return Err(Error::Invalid);
            }
            if entry.category.len() > CATEGORY_BYTES {
                return Err(Error::TooLarge);
            }
        }
        if serde_json::to_vec(self).map_err(|_| Error::Invalid)?.len() > ENCODED_BYTES {
            return Err(Error::TooLarge);
        }
        Ok(())
    }
    /// Changes only a candidate Saved.settings map. The existing Saved writer
    /// must confirm durability before publishing this candidate.
    pub fn write_into(&self, settings: &mut BTreeMap<String, Value>) -> Result<(), Error> {
        self.validate()?;
        let value = serde_json::to_value(self).map_err(|_| Error::Invalid)?;
        settings.insert(KEY.into(), value);
        Ok(())
    }
    /// Call only with the complete validated Saved.profiles image, never a
    /// filtered provider list or a catalog batch. Source edits retain one slot;
    /// only explicitly absent profile IDs are pruned.
    pub fn prune_deleted(
        &mut self,
        profiles: &plx_data::ott::ProviderProfiles,
    ) -> Result<usize, Error> {
        profiles.validate().map_err(|_| Error::Invalid)?;
        let before = self.profiles.len();
        self.profiles
            .retain(|id, _| profiles.profiles.iter().any(|profile| profile.id == *id));
        Ok(before - self.profiles.len())
    }
    pub fn remembered<'a>(&'a self, source: Source<'_>) -> Result<Option<&'a str>, Error> {
        if !source.valid() {
            return Err(Error::Invalid);
        }
        Ok(self
            .profiles
            .get(source.profile_id)
            .filter(|entry| entry.fingerprint == source.fingerprint)
            .map(|entry| entry.category.as_str()))
    }
    /// A provider/profile switch does not end the global Favorites preference.
    /// Seed the old single-category field only when that profile has no history;
    /// a known absent category must never be overwritten with its All fallback.
    pub fn switch_source(
        &mut self,
        from: Option<Source<'_>>,
        to: Option<Source<'_>>,
        legacy_category: &str,
        favorites_selected: bool,
    ) -> Result<Selection, Error> {
        if legacy_category.len() > CATEGORY_BYTES {
            return Err(Error::TooLarge);
        }
        if from.is_some_and(|s| !s.valid()) || to.is_some_and(|s| !s.valid()) {
            return Err(Error::Invalid);
        }
        let mut next = self.clone();
        if let Some(source) = from {
            if !next.profiles.contains_key(source.profile_id) {
                next.profiles.insert(
                    source.profile_id.into(),
                    Entry {
                        fingerprint: source.fingerprint.into(),
                        category: legacy_category.into(),
                    },
                );
            }
        }
        next.validate()?;
        let category = match to {
            Some(source) => next.remembered(source)?.unwrap_or("").to_owned(),
            None => String::new(),
        };
        *self = next;
        Ok(Selection {
            category,
            favorites_selected,
        })
    }
    /// Remap one exact remembered category after an explicit organization edit.
    /// This is metadata maintenance and never changes the global Favorites flag.
    pub fn remap_category(&mut self, source: Source<'_>, from: &str, to: &str) -> Result<bool, Error> {
        if !source.valid() { return Err(Error::Invalid); }
        if to.len() > CATEGORY_BYTES { return Err(Error::TooLarge); }
        let Some(entry) = self.profiles.get(source.profile_id)
            .filter(|entry| entry.fingerprint == source.fingerprint && entry.category == from) else { return Ok(false); };
        if entry.category == to { return Ok(false); }
        let mut next = self.clone();
        next.profiles.get_mut(source.profile_id).ok_or(Error::Invalid)?.category = to.into();
        next.validate()?;
        *self = next;
        Ok(true)
    }

    /// This explicit user choice is the only operation here that clears the
    /// installation-wide Favorites preference. Empty category explicitly means All.
    pub fn choose_category(
        &mut self,
        source: Source<'_>,
        category: &str,
    ) -> Result<Selection, Error> {
        if !source.valid() {
            return Err(Error::Invalid);
        }
        if category.len() > CATEGORY_BYTES {
            return Err(Error::TooLarge);
        }
        let mut next = self.clone();
        next.profiles.insert(
            source.profile_id.into(),
            Entry {
                fingerprint: source.fingerprint.into(),
                category: category.into(),
            },
        );
        next.validate()?;
        *self = next;
        Ok(Selection {
            category: category.into(),
            favorites_selected: false,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum CatalogSelection<'a> {
    All,
    Category(&'a str),
    Awaiting(&'a str),
    Missing(&'a str),
}
impl<'a> CatalogSelection<'a> {
    /// Fallback is a view projection only. Do not persist this over the choice.
    pub fn effective(self) -> &'a str {
        match self {
            Self::Category(category) => category,
            _ => "",
        }
    }
}
pub(super) fn catalog_selection<'a, 'b>(
    remembered: &'a str,
    available: impl IntoIterator<Item = &'b str>,
    complete: bool,
) -> CatalogSelection<'a> {
    if remembered.is_empty() {
        CatalogSelection::All
    } else if available.into_iter().any(|category| category == remembered) {
        CatalogSelection::Category(remembered)
    } else if complete {
        CatalogSelection::Missing(remembered)
    } else {
        CatalogSelection::Awaiting(remembered)
    }
}

#[cfg(test)]
#[path = "ott_category_preferences_tests.rs"]
mod tests;
