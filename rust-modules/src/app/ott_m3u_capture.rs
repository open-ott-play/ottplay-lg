//! Portable M3U capture needs a complete source catalogue, not one playing row.
use super::*;

#[derive(Clone)]
pub(in crate::app::ott_runtime) struct ChannelCapture {
    entry_id: String,
    favorite: Favorite,
    aliases: Vec<Locator>,
}

fn is_channel(owner: SelectionOwner<'_>, entry: &CatalogEntry) -> bool {
    matches!(owner, SelectionOwner::Profile(profile) if profile.kind == ProviderKind::M3u)
        && matches!(entry.media, MediaLocator::Http { .. })
}

#[derive(Default)]
struct Counts<'a> {
    ids: HashMap<&'a str, usize>,
    names: HashMap<&'a str, usize>,
    groups: HashMap<(&'a str, &'a str), usize>,
}
impl<'a> Counts<'a> {
    fn new(rows: &'a [CatalogEntry]) -> Self {
        let mut value = Self::default();
        for row in rows {
            if !matches!(row.media, MediaLocator::Http { .. }) { continue; }
            if let Some(id) = row.provider_id.as_deref() { *value.ids.entry(id).or_default() += 1; }
            *value.names.entry(&row.title).or_default() += 1;
            *value.groups.entry((&row.title, &row.category)).or_default() += 1;
        }
        value
    }
    fn unique(&self, locator: &Locator) -> bool {
        match locator {
            Locator::Channel { channel_id } => {
                let id = channel_id.strip_prefix("m3u-epg:").unwrap_or(channel_id);
                self.ids.get(id) == Some(&1)
                    && (!channel_id.starts_with("m3u-epg:") || !self.ids.contains_key(channel_id.as_str()))
            }
            Locator::ChannelName { name, group: None } => self.names.get(name.as_str()) == Some(&1),
            Locator::ChannelName { name, group: Some(group) } =>
                self.groups.get(&(name.as_str(), group.as_str())) == Some(&1),
            _ => false,
        }
    }
}

/// `catalog` must be the complete, successfully loaded catalogue of `owner`.
/// The exact row check also rejects callers borrowing a different browse source.
pub(in crate::app::ott_runtime) fn capture_channel(
    bindings: &SelectionBindings<'_>, owner: SelectionOwner<'_>,
    catalog: &[CatalogEntry], entry: &CatalogEntry,
) -> Result<Option<ChannelCapture>, BindingError> {
    if !is_channel(owner, entry) { return Ok(None); }
    let mut favorite = capture_bound(bindings, owner, entry)?;
    if !catalog.iter().any(|row| row.id == entry.id && row.provider_id == entry.provider_id
        && row.title == entry.title && row.category == entry.category
        && matches!(row.media, MediaLocator::Http { .. })) { return Err(BindingError::SourceMismatch); }
    let counts = Counts::new(catalog);
    let mut aliases = Vec::new();
    if matches!(favorite.locator, Locator::Channel { .. }) && counts.unique(&favorite.locator) {
        aliases.push(favorite.locator.clone());
        if let Some(id) = entry.provider_id.as_ref().filter(|id| !id.starts_with("m3u-epg:")) {
            let legacy = Locator::Channel { channel_id: id.clone() };
            if counts.unique(&legacy) { aliases.push(legacy); }
        }
    }
    let name = Locator::ChannelName { name: entry.title.clone(), group: None };
    if counts.unique(&name) { aliases.push(name); }
    if !entry.category.is_empty() {
        let grouped = Locator::ChannelName { name: entry.title.clone(), group: Some(entry.category.clone()) };
        if counts.unique(&grouped) { aliases.push(grouped); }
    }
    favorite.locator = aliases.first().cloned().ok_or(BindingError::AmbiguousSource)?;
    favorite.validate().map_err(|_| BindingError::InvalidFavorite)?;
    Ok(Some(ChannelCapture { entry_id: entry.id.clone(), favorite, aliases }))
}

impl ChannelCapture {
    /// Only a successful resolver result, subsequently admitted for this exact
    /// request/profile, attests a Favorite without the browsed catalogue.
    pub(in crate::app::ott_runtime) fn resolved(favorite: Favorite, entry: &CatalogEntry) -> Option<Self> {
        if favorite.origin.provider != "m3u"
            || !matches!(favorite.locator, Locator::Channel { .. } | Locator::ChannelName { .. })
            || favorite.validate().is_err() || !matches_entry(&favorite, entry)
            || legacy_id_collision(&favorite, entry) { return None; }
        Some(Self { entry_id: entry.id.clone(), aliases: vec![favorite.locator.clone()], favorite })
    }
    pub(in crate::app::ott_runtime) fn favorite(
        &self, bindings: &SelectionBindings<'_>, owner: SelectionOwner<'_>, entry: &CatalogEntry,
    ) -> Result<Favorite, BindingError> {
        bindings.validate_owner(owner)?;
        if !is_channel(owner, entry) || self.entry_id != entry.id
            || !matches_entry(&self.favorite, entry)
            || !bindings::same_owner(bind_bound(bindings, &self.favorite)?, owner) {
            return Err(BindingError::SourceMismatch);
        }
        Ok(self.favorite.clone())
    }
    pub(in crate::app::ott_runtime) fn allows(&self, favorite: &Favorite) -> bool {
        self.aliases.iter().any(|locator| locator == &favorite.locator)
    }
}

impl FavoriteIndex {
    /// Once-per-view restriction; row star queries remain allocation-free.
    pub(in crate::app::ott_runtime) fn with_channel_catalog(
        mut self, owner: SelectionOwner<'_>, catalog: &[CatalogEntry], complete: bool,
    ) -> Self {
        if !matches!(owner, SelectionOwner::Profile(profile) if profile.kind == ProviderKind::M3u) { return self; }
        let counts = Counts::new(catalog);
        for (id, value) in &mut self.channels {
            if !complete || counts.ids.get(id.as_str()) != Some(&1) { *value = IndexedMatch::Blocked(BindingError::AmbiguousSource); }
        }
        for (name, category) in &mut self.names {
            if !complete || counts.names.get(name.as_str()) != Some(&1) {
                if !matches!(category.any, IndexedMatch::Empty) { category.any = IndexedMatch::Blocked(BindingError::AmbiguousSource); }
            }
            for (group, value) in &mut category.named {
                if !complete || counts.groups.get(&(name.as_str(), group.as_str())) != Some(&1) {
                    *value = IndexedMatch::Blocked(BindingError::AmbiguousSource);
                }
            }
        }
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn setup(body: &str) -> (ProviderProfiles, plx_data::ott::Catalog) {
        let profile = ProviderProfile { id: "receiver".into(), name: "Home".into(), kind: ProviderKind::M3u,
            source: "https://catalog.invalid/list.m3u?token=private".into(), source_alias: String::new(), enabled: true };
        let catalog = plx_data::ott::parse_m3u(body, &profile).unwrap();
        (ProviderProfiles { version: 1, active_profile: Some(profile.id.clone()), profiles: vec![profile] }, catalog)
    }
    fn duplicate_ids() -> &'static str {
        "#EXTM3U\n#EXTINF:-1 tvg-id=\"same\" group-title=\"News\",Alpha\nhttps://media.invalid/a?token=a\n#EXTINF:-1 tvg-id=\"same\" group-title=\"News\",Beta\nhttps://media.invalid/b?token=b\n"
    }
    #[test]
    fn unique_id_stays_canonical_without_private_source_data() {
        let (profiles, catalog) = setup("#EXTM3U\n#EXTINF:-1 tvg-id=\"only\",Alpha\nhttps://media.invalid/a?token=private\n");
        let bindings = SelectionBindings::profiles_only(&profiles);
        let owner = SelectionOwner::Profile(&profiles.profiles[0]);
        let capture = capture_channel(&bindings, owner, &catalog.entries, &catalog.entries[0]).unwrap().unwrap();
        assert_eq!(capture.favorite.locator, Locator::Channel { channel_id: "m3u-epg:only".into() });
        assert!(!serde_json::to_string(&capture.favorite).unwrap().contains("private"));
        assert!(capture.allows(&Favorite { locator: Locator::Channel { channel_id: "only".into() }, ..capture.favorite.clone() }));
    }
    #[test]
    fn duplicate_id_uses_unique_name_with_null_group() {
        let (profiles, catalog) = setup(duplicate_ids());
        assert_eq!(catalog.entries.len(), 2);
        let bindings = SelectionBindings::profiles_only(&profiles);
        let owner = SelectionOwner::Profile(&profiles.profiles[0]);
        for row in &catalog.entries {
            let capture = capture_channel(&bindings, owner, &catalog.entries, row).unwrap().unwrap();
            assert_eq!(capture.favorite.locator, Locator::ChannelName { name: row.title.clone(), group: None });
        }
    }
    #[test]
    fn same_name_uses_unique_group_and_ambiguous_group_refuses() {
        let (profiles, mut catalog) = setup(duplicate_ids());
        catalog.entries[1].title = "Alpha".into(); catalog.entries[1].category = "Movies".into();
        let bindings = SelectionBindings::profiles_only(&profiles);
        let owner = SelectionOwner::Profile(&profiles.profiles[0]);
        let capture = capture_channel(&bindings, owner, &catalog.entries, &catalog.entries[0]).unwrap().unwrap();
        assert_eq!(capture.favorite.locator, Locator::ChannelName { name: "Alpha".into(), group: Some("News".into()) });
        catalog.entries[1].category = "News".into();
        assert!(matches!(capture_channel(&bindings, owner, &catalog.entries, &catalog.entries[0]), Err(BindingError::AmbiguousSource)));
    }
    #[test]
    fn ambiguous_legacy_id_never_deletes_another_channel_favorite() {
        let (profiles, catalog) = setup(duplicate_ids());
        let bindings = SelectionBindings::profiles_only(&profiles);
        let owner = SelectionOwner::Profile(&profiles.profiles[0]);
        for id in ["same", "m3u-epg:same"] {
            let mut old = capture_bound(&bindings, owner, &catalog.entries[0]).unwrap();
            old.locator = Locator::Channel { channel_id: id.into() };
            let mut document = Document::default(); document.toggle(old).unwrap();
            let before = document.clone(); let mut proofs = LocalFavoriteProofs::default();
            assert_eq!(proofs.toggle_capture_with_pins(&mut document, &[], &bindings, owner, &catalog.entries, &catalog.entries[1]), Err(BindingError::AmbiguousSource));
            assert_eq!(document, before);
            let index = FavoriteIndex::new_proven(&bindings, &document, owner, &proofs).unwrap()
                .with_channel_catalog(owner, &catalog.entries, true);
            assert!(!index.contains(&catalog.entries[0])); assert!(!index.contains(&catalog.entries[1]));
        }
    }
    #[test]
    fn duplicate_id_channels_toggle_independently_and_do_not_duplicate() {
        let (profiles, catalog) = setup(duplicate_ids());
        let bindings = SelectionBindings::profiles_only(&profiles);
        let owner = SelectionOwner::Profile(&profiles.profiles[0]);
        let mut document = Document::default(); let mut proofs = LocalFavoriteProofs::default();
        for row in &catalog.entries { assert!(proofs.toggle_capture_with_pins(&mut document, &[], &bindings, owner, &catalog.entries, row).unwrap()); }
        assert_eq!(document.entries().len(), 2);
        assert!(!proofs.toggle_capture_with_pins(&mut document, &[], &bindings, owner, &catalog.entries, &catalog.entries[1]).unwrap());
        assert_eq!(document.entries().len(), 1); assert_eq!(document.entries()[0].title, "Alpha");
    }
    #[test]
    fn unique_legacy_id_is_removed_and_unrelated_catalog_cannot_attest() {
        let (profiles, catalog) = setup("#EXTM3U\n#EXTINF:-1 tvg-id=\"only\",Alpha\nhttps://media.invalid/a\n");
        let bindings = SelectionBindings::profiles_only(&profiles); let owner = SelectionOwner::Profile(&profiles.profiles[0]);
        let mut old = capture_bound(&bindings, owner, &catalog.entries[0]).unwrap(); old.locator = Locator::Channel { channel_id: "only".into() };
        let mut document = Document::default(); document.toggle(old).unwrap(); let mut proofs = LocalFavoriteProofs::default();
        assert!(!proofs.toggle_capture_with_pins(&mut document, &[], &bindings, owner, &catalog.entries, &catalog.entries[0]).unwrap());
        assert!(document.entries().is_empty());
        assert!(matches!(capture_channel(&bindings, owner, &[], &catalog.entries[0]), Err(BindingError::SourceMismatch)));
    }
}
