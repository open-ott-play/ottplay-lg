//! Pure projections between configured sources and portable Favorites. Neither a
//! receiving client's slot number nor its private URL/root is portable identity.
use plx_data::ott::favorites::{Document, Favorite, Locator, Origin};
use plx_data::ott::{CatalogEntry, MediaLocator, ProviderKind, ProviderProfile, ProviderProfiles};
use std::collections::HashMap;

#[path = "ott_selection_bindings.rs"]
mod bindings;
#[path = "ott_local_favorite_proofs.rs"]
mod local_proofs;
#[path = "ott_m3u_capture.rs"]
mod m3u_capture;
pub(super) use m3u_capture::{capture_channel, ChannelCapture};
pub(super) use local_proofs::{local_root_fingerprint, LocalFavoriteProofs};
pub(in crate::app) use local_proofs::LocalFavoritePermit;
pub(super) use bindings::{
    bind_bound, capture_bound, existing_identity_bound, LocalBindingReceipt,
    SelectionBindings, SelectionOwner,
};

#[cfg(test)]
#[path = "ott_selection_bindings_tests.rs"]
mod bindings_tests;

#[cfg(test)]
#[path = "ott_selection_attachment_tests.rs"]
mod attachment_tests;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum BindingError {
    InvalidFavorite,
    UnsupportedProvider,
    UnsupportedSource,
    MissingProfile,
    MissingAttachment,
    AmbiguousSource,
    SourceMismatch,
    SourceUnavailable,
    LocalPathUnproven,
}

impl BindingError {
    pub(super) const fn message(self) -> &'static str {
        match self {
            Self::InvalidFavorite => "Invalid Favorite identity",
            Self::UnsupportedProvider => "Favorite provider is unavailable",
            Self::UnsupportedSource => "Favorite source is not supported by this player",
            Self::MissingProfile => "Favorite profile is not configured on this player",
            Self::MissingAttachment => "Favorite M3U media attachment is not configured on this player",
            Self::AmbiguousSource => "Favorite source is ambiguous",
            Self::SourceMismatch => "Favorite source does not match its media",
            Self::SourceUnavailable => "Favorite source is currently unavailable",
            Self::LocalPathUnproven => "Select this local file on this player and add it to Favorites again",
        }
    }
}

/// Imported root names are independent of the receiver's mount path. The import
/// adapter retains that name in source_alias; direct folder selection uses its
/// basename, as the web folder picker does. This never reads the filesystem.
fn local_alias(profile: &ProviderProfile) -> &str {
    if !profile.source_alias.is_empty() {
        &profile.source_alias
    } else if profile.source.starts_with('/') {
        profile
            .source
            .trim_end_matches('/')
            .rsplit('/')
            .next()
            .filter(|name| !name.is_empty())
            .unwrap_or(&profile.name)
    } else {
        // A server library needs an explicit root label, not its private URL.
        &profile.name
    }
}

fn portable_brand(profile:&ProviderProfile)->Option<&'static str> {
    let name=match profile.kind {ProviderKind::Edem=>"Edem.tv / iLook.tv",ProviderKind::Operator(id)=>id.name(),_=>return None};
    let default_name=profile.name==name || profile.name==format!("{name} 1") || profile.name==format!("{} 1",profile.kind.canonical_id()) || profile.name==format!("{} 1",profile.kind.canonical_id().replace('/',"_"))
        || (profile.kind==ProviderKind::Edem && matches!(profile.name.as_str(),"Edem / iLook TV 1"|"Edem / iLook TV"));
    (default_name&&(profile.source_alias.is_empty()||profile.source_alias==profile.name||profile.source_alias==name)).then_some(name)
}
fn playlist_origin(profile:&ProviderProfile)->Option<String> {
    match profile.kind {
        ProviderKind::Edem=>plx_data::ott::edem::Config::parse(&profile.source).ok().map(|config|format!("Playlist {}",config.list+1)),
        ProviderKind::Operator(id) if id.canonical_id()=="kb-team"=>plx_data::ott::operators::Config::parse(id,&profile.source).ok().map(|config|format!("Playlist {}",config.list+1)),
        _=>None,
    }
}

fn origin_matches(profile:&ProviderProfile,origin:&Origin)->bool {
    (origin.profile==profile.name && (origin.source_alias==profile.alias()
        // Older native captures used the display name even when an explicit
        // alias was configured. Keep that exact legacy form readable; the
        // resolver still rejects every set with more than one matching source.
        || (origin.source_alias==profile.name && matches!(profile.kind,
            ProviderKind::M3u | ProviderKind::Xtream | ProviderKind::Stalker | ProviderKind::VPortal))))
        || portable_brand(profile).is_some_and(|name|origin.profile==name&&origin.source_alias==name)
}

fn profile_matches(profile: &ProviderProfile, favorite: &Favorite) -> bool {
    let origin = &favorite.origin;
    match (&profile.kind, &favorite.locator) {
        // Embedded portals are separately attested authorities. Their explicit
        // parent alias cannot be replaced by a legacy ordinary-profile name.
        (ProviderKind::M3u, Locator::VPortal { .. }) => {
            origin.profile == profile.name && origin.source_alias == profile.alias()
        }
        (ProviderKind::Plex, Locator::Plex { server_id, .. }) => {
            // Web Plex has one logical profile and several authenticated servers.
            // Accept old native names only against their exact original source.
            profile.source == *server_id
                && ((origin.profile == "Plex" && origin.source_alias == *server_id)
                    || (origin.profile == profile.name && origin.source_alias == profile.alias()))
        }
        (ProviderKind::Edem | ProviderKind::Operator(plx_data::ott::OperatorId::KbTeam), Locator::Channel { .. } | Locator::ChannelName { .. }) => {
            let expected=playlist_origin(profile);
            origin_matches(profile,origin) && origin.subprofile.as_ref()==expected.as_ref()
        }
        (ProviderKind::Local, Locator::Local { root_alias, .. }) => {
            origin.profile == profile.name
                && origin.source_alias == *root_alias
                && (plx_data::ott::http_url(&profile.source).is_some()
                    || root_alias == local_alias(profile)
                    || root_alias == profile.alias())
        }
        _ => {
            origin_matches(profile,origin)
        }
    }
}

/// Missing and unsupported entries remain in the document. Do not pick the first
/// candidate or search another profile by title when the binding is ambiguous.
pub(super) fn resolve_profile<'a>(
    profiles: &'a ProviderProfiles,
    favorite: &Favorite,
) -> Result<&'a ProviderProfile, BindingError> {
    favorite
        .validate()
        .map_err(|_| BindingError::InvalidFavorite)?;
    let kind = match favorite.origin.provider.as_str() {
        "m3u" => ProviderKind::M3u,
        "xtream" => ProviderKind::Xtream,
        "edem" => ProviderKind::Edem,
        "stalker" => ProviderKind::Stalker,
        "local" => ProviderKind::Local,
        "plex" => ProviderKind::Plex,
        "vportal" => ProviderKind::VPortal,
        provider => ProviderKind::from_id(provider).ok_or(BindingError::UnsupportedProvider)?,
    };
    match (&kind, &favorite.locator) {
        (ProviderKind::Local, Locator::Local { root_alias, .. }) => {
            if favorite.origin.source_alias != *root_alias {
                return Err(BindingError::SourceMismatch);
            }
            if favorite.origin.subprofile.is_some() {
                return Err(BindingError::UnsupportedSource);
            }
        }
        (ProviderKind::Plex, Locator::Plex { .. }) => {
            if favorite.origin.subprofile.is_some() {
                return Err(BindingError::UnsupportedSource);
            }
        }
        (ProviderKind::VPortal | ProviderKind::Edem, Locator::VPortal { .. }) => {}
        (ProviderKind::M3u, Locator::VPortal { .. }) => {
            // An ordinary M3U URL cannot resolve a separate embedded portal.
            return Err(BindingError::UnsupportedSource);
        }
        (ProviderKind::Edem | ProviderKind::Operator(plx_data::ott::OperatorId::KbTeam), Locator::Channel { .. } | Locator::ChannelName { .. }) => {
            if !favorite.origin.subprofile.as_deref().is_some_and(|slot|slot.strip_prefix("Playlist ").and_then(|n|n.parse::<u8>().ok()).is_some_and(|n|(1..=4).contains(&n))) {
                return Err(BindingError::UnsupportedSource);
            }
        }
        (ProviderKind::M3u, Locator::Channel { .. } | Locator::ChannelName { .. })
        | (ProviderKind::Xtream | ProviderKind::Stalker | ProviderKind::Operator(_), Locator::ChannelName { .. }) => {
            if favorite.origin.subprofile.is_some() {
                return Err(BindingError::UnsupportedSource);
            }
        }
        (ProviderKind::Xtream | ProviderKind::Stalker, Locator::Channel { .. }) => {
            // Native numeric IDs are not the web provider's attested media keys.
            return Err(BindingError::UnsupportedSource);
        }
        _ => return Err(BindingError::SourceMismatch),
    }
    let mut matches = profiles.profiles.iter().filter(|profile| {
        profile.enabled && profile.kind == kind && profile_matches(profile, favorite)
    });
    let profile = matches.next().ok_or(BindingError::MissingProfile)?;
    if matches.next().is_some() {
        return Err(BindingError::AmbiguousSource);
    }
    if profile.kind == ProviderKind::M3u
        && plx_data::ott::m3u_source_kind(&profile.source) != Ok(plx_data::ott::M3uSourceKind::Playlist)
    {
        return Err(BindingError::UnsupportedSource);
    }
    Ok(profile)
}

/// Validate portable parent identity before the worker uses the captured portal.
/// Currentness is deliberately separate: the runtime rechecks its Authorities
/// receipt and request/navigation tickets before launch and before admission.
pub(in crate::app) fn attachment_profile(
    receipt: &crate::app::ott_m3u_attachment::Receipt,
    favorite: &Favorite,
) -> Result<ProviderProfile, &'static str> {
    favorite.validate().map_err(|_| BindingError::InvalidFavorite.message())?;
    let parent = receipt.parent();
    if !parent.enabled || parent.kind != ProviderKind::M3u
        || favorite.origin.provider != "m3u"
        || !matches!(favorite.locator, Locator::VPortal { .. })
        || !profile_matches(parent, favorite) {
        return Err(BindingError::SourceMismatch.message());
    }
    receipt.effective_profile().map_err(|_| BindingError::SourceMismatch.message())
}

fn channel_name(entry: &CatalogEntry) -> Locator {
    Locator::ChannelName {
        name: entry.title.clone(),
        group: (!entry.category.is_empty()).then(|| entry.category.clone()),
    }
}

fn portable_id(id: &str) -> bool {
    id.len() <= 256
        && id.bytes().next().is_some_and(|b| b.is_ascii_alphanumeric())
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._:-".contains(&b))
}

/// Pass the source owning the row or playing item, not the currently selected UI
/// profile during a mixed-Favorites handoff. No credentials enter the result.
pub(super) fn capture(profile: &ProviderProfile, entry: &CatalogEntry) -> Option<Favorite> {
    if !profile.enabled {
        return None;
    }
    let mut origin = Origin {
        provider: profile.kind.canonical_id().into(),
        profile: profile.name.clone(),
        subprofile: None,
        source_alias: profile.alias().to_owned(),
    };
    if let Some(name)=portable_brand(profile){origin.profile=name.into();origin.source_alias=name.into();}
    let locator = match (&profile.kind, &entry.media) {
        (ProviderKind::Local, MediaLocator::Local { relative_path }) => {
            origin.source_alias = local_alias(profile).into();
            Locator::Local {
                root_alias: origin.source_alias.clone(),
                relative_path: relative_path.clone(),
            }
        }
        (
            ProviderKind::Local,
            MediaLocator::ServerLibrary {
                relative_path: Some(relative_path),
                root_alias: Some(root_alias),
                ..
            },
        ) if plx_data::ott::http_url(&profile.source).is_some() => {
            // The server adapter attests this root alias and relative path
            // against one unique folder source. URLs and item IDs stay local.
            origin.source_alias = root_alias.clone();
            Locator::Local {
                root_alias: root_alias.clone(),
                relative_path: relative_path.clone(),
            }
        }
        (
            ProviderKind::Plex,
            MediaLocator::Plex {
                server_id,
                rating_key,
            },
        ) => {
            if *server_id != profile.source {
                return None;
            }
            origin.profile = "Plex".into();
            origin.source_alias = server_id.clone();
            Locator::Plex {
                server_id: server_id.clone(),
                rating_key: rating_key.clone(),
            }
        }
        (ProviderKind::VPortal, MediaLocator::VPortal { .. } | MediaLocator::Http { .. })
        | (ProviderKind::Edem, MediaLocator::VPortal { .. } | MediaLocator::Http { .. })
            if profile.kind == ProviderKind::VPortal || entry.provider_id_field.is_some() => {
            origin.subprofile = (!entry.category.is_empty()).then(|| entry.category.clone());
            Locator::VPortal {
                id_field: entry.provider_id_field.clone()?,
                id: entry.provider_id.clone()?,
            }
        }
        (ProviderKind::M3u, MediaLocator::Http { .. }) => entry
            .provider_id
            .as_deref()
            .filter(|id| portable_id(id))
            .map(|id| format!("m3u-epg:{id}"))
            .filter(|id| portable_id(id))
            .map(|channel_id| Locator::Channel { channel_id })
            .unwrap_or_else(|| channel_name(entry)),
        // Web Xtream falls back to name+group. Stalker media keys require source
        // attestation; a native numeric ID is not that same portable identity.
        (ProviderKind::Edem, _) => {
            let config=plx_data::ott::edem::Config::parse(&profile.source).ok()?;
            origin.subprofile=Some(format!("Playlist {}",config.list + 1));
            channel_name(entry)
        }
        (ProviderKind::Operator(id), _) if id.canonical_id()=="kb-team" => {
            origin.subprofile=playlist_origin(profile);
            channel_name(entry)
        }
        (ProviderKind::Xtream | ProviderKind::Stalker | ProviderKind::Operator(_), _) => channel_name(entry),
        _ => return None,
    };
    let favorite = Favorite {
        title: entry.title.clone(),
        origin,
        locator,
    };
    favorite.validate().ok()?;
    Some(favorite)
}

/// Match inside a catalogue already bound with resolve_profile. Catalogue IDs
/// include the receiver's profile ID and cannot identify an imported Favorite.
/// The caller rejects multiple distinct matches, legacy_id_collision, and stale
/// source generations before reporting an item as resolved.
pub(in crate::app) fn matches_entry(favorite: &Favorite, entry: &CatalogEntry) -> bool {
    match (&favorite.locator, &entry.media) {
        (
            Locator::Local { relative_path, .. },
            MediaLocator::Local {
                relative_path: path,
            },
        ) => relative_path == path,
        (
            Locator::Local {
                root_alias,
                relative_path,
            },
            MediaLocator::ServerLibrary {
                root_alias: Some(alias),
                relative_path: Some(path),
                ..
            },
        ) => root_alias == alias && relative_path == path,
        (
            Locator::Plex {
                server_id,
                rating_key,
            },
            MediaLocator::Plex {
                server_id: server,
                rating_key: key,
            },
        ) => server_id == server && rating_key == key,
        (
            Locator::VPortal { id_field, id },
            MediaLocator::VPortal { .. } | MediaLocator::Http { .. },
        ) => {
            entry.provider_id.as_ref() == Some(id)
                && entry.provider_id_field.as_ref() == Some(id_field)
                && favorite
                    .origin
                    .subprofile
                    .as_ref()
                    .is_none_or(|category| category == &entry.category)
        }
        (Locator::Channel { channel_id }, MediaLocator::Http { .. })
            if favorite.origin.provider == "m3u" =>
        {
            // Explicit namespace is always canonical, never retried as a raw ID:
            // tvg-id "x" and tvg-id "m3u-epg:x" are different channels.
            let wanted = channel_id.strip_prefix("m3u-epg:").unwrap_or(channel_id);
            entry.provider_id.as_deref() == Some(wanted)
        }
        (Locator::ChannelName { name, group }, _) => {
            // The web resolver and its post-play verifier both treat null as
            // unspecified, not as an assertion that the item's group is empty.
            entry.title == *name
                && group
                    .as_ref()
                    .is_none_or(|category| category == &entry.category)
        }
        _ => false,
    }
}

/// Older native documents exported raw tvg-id values. A literal tvg-id carrying
/// the canonical prefix is therefore ambiguous with a web document. Never fall
/// back to that literal ID, even if the unprefixed target is currently missing.
pub(in crate::app) fn legacy_id_collision(favorite: &Favorite, entry: &CatalogEntry) -> bool {
    match (&favorite.locator, &entry.media) {
        (Locator::Channel { channel_id }, MediaLocator::Http { .. })
            if favorite.origin.provider == "m3u" && channel_id.starts_with("m3u-epg:") =>
        {
            entry.provider_id.as_ref() == Some(channel_id)
        }
        _ => false,
    }
}

/// Reference scan for indexed star/toggle parity tests. Old native records may
/// resolve to this item while differing from its newly canonical capture. Remove
/// this returned identity; only capture/add when the result is `Ok(None)`.
///
/// `profile` must own `entry` and be part of this exact configuration snapshot.
/// For fullscreen actions it is the playing profile, not the UI's active one.
/// Unresolved records are left untouched. Ambiguous records cannot be deleted by
/// a toggle; the caller must show the error instead of guessing or adding a copy.
#[cfg(test)]
pub(super) fn existing_identity(
    profiles: &ProviderProfiles,
    document: &Document,
    profile: &ProviderProfile,
    entry: &CatalogEntry,
) -> Result<Option<String>, BindingError> {
    let configured = profiles
        .profiles
        .iter()
        .filter(|candidate| candidate.enabled && *candidate == profile)
        .count();
    match configured {
        0 => return Err(BindingError::MissingProfile),
        1 => {}
        _ => return Err(BindingError::AmbiguousSource),
    }
    let mut found = None;
    for favorite in document.entries() {
        // Cheap item/source filters avoid resolving every unrelated Favorite
        // for every visible catalogue row. They are not a binding proof.
        if favorite.origin.provider != profile.kind.canonical_id()
            || !profile_matches(profile, favorite)
        {
            continue;
        }
        let collision = legacy_id_collision(favorite, entry);
        if !collision && !matches_entry(favorite, entry) {
            continue;
        }
        match resolve_profile(profiles, favorite) {
            Ok(bound) if bound == profile => {
                if collision || found.is_some() {
                    return Err(BindingError::AmbiguousSource);
                }
                found = Some(favorite.identity());
            }
            Err(BindingError::AmbiguousSource) => return Err(BindingError::AmbiguousSource),
            Err(BindingError::InvalidFavorite) => return Err(BindingError::InvalidFavorite),
            _ => {}
        }
    }
    Ok(found)
}

/// A bucket needs to distinguish zero, one and multiple identities, never an
/// unbounded list of candidates. A blocked source stays blocked when combined.
#[derive(Clone, Copy, Default)]
enum IndexedMatch {
    #[default]
    Empty,
    One(usize),
    Blocked(BindingError),
}

impl IndexedMatch {
    fn combine(self, other: Self) -> Self {
        match (self, other) {
            (Self::Blocked(error), _) | (_, Self::Blocked(error)) => Self::Blocked(error),
            (Self::Empty, found) | (found, Self::Empty) => found,
            (Self::One(first), Self::One(second)) if first == second => Self::One(first),
            (Self::One(_), Self::One(_)) => Self::Blocked(BindingError::AmbiguousSource),
        }
    }
}

type ExactFavorites = HashMap<String, IndexedMatch>;
type PairedFavorites = HashMap<String, ExactFavorites>;

fn insert_match(index: &mut ExactFavorites, key: &str, value: IndexedMatch) {
    let current = index.entry(key.to_owned()).or_default();
    *current = current.combine(value);
}

fn exact_match(index: &ExactFavorites, key: &str) -> IndexedMatch {
    index.get(key).copied().unwrap_or_default()
}

fn paired_match(index: &PairedFavorites, first: &str, second: &str) -> IndexedMatch {
    index
        .get(first)
        .map_or(IndexedMatch::Empty, |rows| exact_match(rows, second))
}

#[derive(Default)]
struct CategoryFavorites {
    any: IndexedMatch,
    named: ExactFavorites,
}

impl CategoryFavorites {
    fn insert(&mut self, category: Option<&str>, value: IndexedMatch) {
        match category {
            Some(name) => insert_match(&mut self.named, name, value),
            None => self.any = self.any.combine(value),
        }
    }

    fn get(&self, category: &str) -> IndexedMatch {
        self.any.combine(exact_match(&self.named, category))
    }
}

/// Immutable, owned index for one exact profile configuration and document
/// snapshot. Build once per view (or cache by both revisions); row queries do no
/// profile resolution, Favorite capture, document scan or string allocation.
#[derive(Default)]
pub(super) struct FavoriteIndex {
    // This index is a projection of one already validated owner, not a source
    // registry. Legacy profile-only indexes deliberately carry no root proof.
    bound_local_root: Option<(String, String)>,
    bound_attachment: Option<String>,
    identities: Vec<String>,
    channels: ExactFavorites,
    legacy_collisions: ExactFavorites,
    local_paths: ExactFavorites,
    local_proof_items: HashMap<String, HashMap<String, String>>,
    local_roots: PairedFavorites,
    plex: PairedFavorites,
    portals: HashMap<String, HashMap<String, CategoryFavorites>>,
    names: HashMap<String, CategoryFavorites>,
}

impl FavoriteIndex {
    #[cfg(test)]
    pub(super) fn new(
        profiles: &ProviderProfiles,
        document: &Document,
        profile: &ProviderProfile,
    ) -> Result<Self, BindingError> {
        Self::new_bound(
            &SelectionBindings::profiles_only(profiles),
            document,
            SelectionOwner::Profile(profile),
        )
    }

    /// The same index builder serves numbered profiles and temporary roots.
    pub(super) fn new_bound(
        bindings: &SelectionBindings<'_>,
        document: &Document,
        owner: SelectionOwner<'_>,
    ) -> Result<Self, BindingError> {
        bindings.validate_owner(owner)?;
        let mut index = Self::with_resolution(document, |favorite| {
            if !bindings::owner_matches(owner, favorite) {
                return None;
            }
            Some(bindings.resolve(favorite).map(|bound| bindings::same_owner(bound, owner)))
        });
        if let SelectionOwner::LocalRoot(root) = owner {
            index.bound_local_root = Some((root.id.clone(), root.name.clone()));
        }
        if let SelectionOwner::M3uAttachment(receipt) = owner {
            index.bound_attachment = Some(receipt.attachment().id().to_owned());
        }
        Ok(index)
    }

    /// Proof-aware projection shared by row stars and explicit capture/toggle.
    /// Basename proof is checked once per Favorite; row lookups stay bounded.
    pub(super) fn new_proven(
        bindings: &SelectionBindings<'_>, document: &Document, owner: SelectionOwner<'_>,
        proofs: &LocalFavoriteProofs,
    ) -> Result<Self, BindingError> {
        bindings.validate_owner(owner)?;
        proofs.validate()?;
        let mut items = HashMap::new();
        let mut index = Self::with_resolution(document, |favorite| {
            if !bindings::owner_matches(owner, favorite) { return None; }
            Some(bindings.resolve(favorite).and_then(|bound| {
                if !bindings::same_owner(bound, owner) { return Ok(false); }
                match proofs.item_for(owner, favorite) {
                    Ok(Some(id)) => {
                        if let Locator::Local { root_alias, relative_path } = &favorite.locator {
                            items.entry(root_alias.clone()).or_insert_with(HashMap::new)
                                .insert(relative_path.clone(), id.to_owned());
                        }
                        Ok(true)
                    }
                    Ok(None) => Ok(true),
                    Err(BindingError::LocalPathUnproven) => Ok(false),
                    Err(error) => Err(error),
                }
            }))
        });
        index.local_proof_items = items;
        if let SelectionOwner::LocalRoot(root) = owner {
            index.bound_local_root = Some((root.id.clone(), root.name.clone()));
        }
        if let SelectionOwner::M3uAttachment(receipt) = owner {
            index.bound_attachment = Some(receipt.attachment().id().to_owned());
        }
        Ok(index)
    }

    fn with_resolution(
        document: &Document,
        mut resolve: impl FnMut(&Favorite) -> Option<Result<bool, BindingError>>,
    ) -> Self {
        let mut index = Self::default();
        for favorite in document.entries() {
            // One resolution per relevant Favorite, regardless of catalogue size.
            let value = match resolve(favorite) {
                Some(Ok(true)) => {
                    let id = index.identities.len();
                    index.identities.push(favorite.identity());
                    IndexedMatch::One(id)
                }
                Some(Err(error @ (BindingError::AmbiguousSource | BindingError::InvalidFavorite))) => {
                    IndexedMatch::Blocked(error)
                }
                _ => continue,
            };
            match &favorite.locator {
                Locator::Channel { channel_id } => {
                    let canonical = channel_id.strip_prefix("m3u-epg:");
                    insert_match(&mut index.channels, canonical.unwrap_or(channel_id), value);
                    if canonical.is_some() {
                        insert_match(
                            &mut index.legacy_collisions,
                            channel_id,
                            value.combine(IndexedMatch::Blocked(BindingError::AmbiguousSource)),
                        );
                    }
                }
                Locator::ChannelName { name, group } => index
                    .names
                    .entry(name.clone())
                    .or_default()
                    .insert(group.as_deref(), value),
                Locator::Local {
                    root_alias,
                    relative_path,
                } => {
                    insert_match(&mut index.local_paths, relative_path, value);
                    insert_match(
                        index.local_roots.entry(root_alias.clone()).or_default(),
                        relative_path,
                        value,
                    );
                }
                Locator::Plex {
                    server_id,
                    rating_key,
                } => insert_match(
                    index.plex.entry(server_id.clone()).or_default(),
                    rating_key,
                    value,
                ),
                Locator::VPortal { id_field, id } => index
                    .portals
                    .entry(id_field.clone())
                    .or_default()
                    .entry(id.clone())
                    .or_default()
                    .insert(favorite.origin.subprofile.as_deref(), value),
            }
        }
        index
    }

    pub(super) fn contains(&self, entry: &CatalogEntry) -> bool {
        matches!(self.identity(entry), Ok(Some(_)))
    }

    pub(super) fn identity(&self, entry: &CatalogEntry) -> Result<Option<&str>, BindingError> {
        if self.bound_attachment.as_ref().is_some_and(|id|
            !entry.id.strip_prefix(id.as_str()).is_some_and(|rest| rest.starts_with(':'))) {
            return Err(BindingError::SourceMismatch);
        }
        if let Some((source_id, alias)) = &self.bound_local_root {
            bindings::local_entry_path(source_id, alias, entry)?;
        }
        let proof_mismatch = match &entry.media {
            MediaLocator::Local { relative_path } => {
                let mut ids = self.local_proof_items.values().filter_map(|paths| paths.get(relative_path));
                ids.next().is_some_and(|id| *id != entry.id || ids.any(|other| *other != entry.id))
            }
            MediaLocator::ServerLibrary { root_alias: Some(root), relative_path: Some(path), .. } =>
                self.local_proof_items.get(root).and_then(|paths| paths.get(path))
                    .is_some_and(|id| *id != entry.id),
            _ => false,
        };
        if proof_mismatch { return Ok(None); }
        let named = self
            .names
            .get(entry.title.as_str())
            .map_or(IndexedMatch::Empty, |groups| groups.get(&entry.category));
        let portal = || {
            entry
                .provider_id_field
                .as_deref()
                .and_then(|field| self.portals.get(field))
                .and_then(|ids| entry.provider_id.as_deref().and_then(|id| ids.get(id)))
                .map_or(IndexedMatch::Empty, |groups| groups.get(&entry.category))
        };
        let located = match &entry.media {
            MediaLocator::Http { .. } => {
                let channel = entry
                    .provider_id
                    .as_deref()
                    .map_or(IndexedMatch::Empty, |id| {
                        exact_match(&self.channels, id)
                            .combine(exact_match(&self.legacy_collisions, id))
                    });
                channel.combine(portal())
            }
            MediaLocator::VPortal { .. } => portal(),
            MediaLocator::Local { relative_path } => exact_match(&self.local_paths, relative_path),
            MediaLocator::ServerLibrary {
                root_alias: Some(root),
                relative_path: Some(path),
                ..
            } => paired_match(&self.local_roots, root, path),
            MediaLocator::Plex {
                server_id,
                rating_key,
            } => paired_match(&self.plex, server_id, rating_key),
            _ => IndexedMatch::Empty,
        };
        match named.combine(located) {
            IndexedMatch::Empty => Ok(None),
            IndexedMatch::One(index) => Ok(Some(&self.identities[index])),
            IndexedMatch::Blocked(error) => Err(error),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn server_folder_favorites_use_portable_paths_not_receiver_item_ids() {
        let mut server = profile(ProviderKind::Local);
        server.source = "https://example.invalid/library".into();
        let item = entry(MediaLocator::ServerLibrary {
            source_id: "receiver-root-id".into(),
            item_id: "receiver-item-id".into(),
            root_alias: Some("Movies".into()),
            relative_path: Some("Season 1/Episode.mp4".into()),
        });
        let favorite = capture(&server, &item).unwrap();
        assert_eq!(favorite.origin.source_alias, "Movies");
        assert_eq!(
            favorite.locator,
            Locator::Local {
                root_alias: "Movies".into(),
                relative_path: "Season 1/Episode.mp4".into(),
            }
        );
        let configured = profiles(server.clone());
        assert_eq!(
            resolve_profile(&configured, &favorite),
            Ok(&configured.profiles[0])
        );
        assert!(matches_entry(&favorite, &item));
        let other_root = entry(MediaLocator::ServerLibrary {
            source_id: "other-root".into(),
            item_id: "other-item".into(),
            root_alias: Some("Other".into()),
            relative_path: Some("Season 1/Episode.mp4".into()),
        });
        assert!(!matches_entry(&favorite, &other_root));
        let old_server = entry(MediaLocator::ServerLibrary {
            source_id: "root".into(),
            item_id: "item".into(),
            root_alias: None,
            relative_path: None,
        });
        assert!(capture(&server, &old_server).is_none());
        let disk = profile(ProviderKind::Local);
        assert!(capture(&disk, &item).is_none());
        let mut document = Document::default();
        document.toggle(favorite.clone()).unwrap();
        let index = FavoriteIndex::new(&configured, &document, &server).unwrap();
        assert_eq!(
            index.identity(&item),
            Ok(Some(favorite.identity().as_str()))
        );
        assert!(!index.contains(&other_root));
        assert!(!index.contains(&old_server));
    }
    fn profile(kind: ProviderKind) -> ProviderProfile {
        ProviderProfile {
            id: "receiver-slot".into(),
            name: "Home".into(),
            kind,
            source: if kind == ProviderKind::M3u {
                "https://playlist.invalid/fixture.m3u".into()
            } else {
                "/media/Movies".into()
            },
            source_alias: String::new(),
            enabled: true,
        }
    }
    fn profiles(profile: ProviderProfile) -> ProviderProfiles {
        ProviderProfiles {
            version: 1,
            active_profile: None,
            profiles: vec![profile],
        }
    }
    fn entry(media: MediaLocator) -> CatalogEntry {
        CatalogEntry {
            synopsis: None,
            id: "receiver-row".into(),
            title: "Film".into(),
            category: "Series / Season 2".into(),
            description: String::new(),
            poster: String::new(),
            media,
            duration_ms: None,
            provider_id: Some("42".into()),
            provider_id_field: Some("id".into()),
            live: None,
        }
    }
    fn local() -> (ProviderProfiles, Favorite) {
        let p = profile(ProviderKind::Local);
        let f = capture(
            &p,
            &entry(MediaLocator::Local {
                relative_path: "Season 2/Film.mp4".into(),
            }),
        )
        .unwrap();
        (profiles(p), f)
    }
    #[test]
    fn local_rebind_uses_portable_root_not_sender_mount_or_slot() {
        let (mut profiles, favorite) = local();
        profiles.profiles[0].source = "/different/mount".into();
        profiles.profiles[0].source_alias = "Movies".into();
        assert_eq!(
            resolve_profile(&profiles, &favorite).unwrap().source,
            "/different/mount"
        );
        assert_eq!(favorite.origin.source_alias, "Movies");
        assert!(!favorite.identity().contains("/media/"));
    }
    #[test]
    fn missing_ambiguous_disabled_and_other_profile_are_distinct() {
        let (mut profiles, mut favorite) = local();
        let mut duplicate = profiles.profiles[0].clone();
        duplicate.id = "other-slot".into();
        profiles.profiles.push(duplicate);
        assert_eq!(
            resolve_profile(&profiles, &favorite),
            Err(BindingError::AmbiguousSource)
        );
        profiles.profiles[1].enabled = false;
        assert!(resolve_profile(&profiles, &favorite).is_ok());
        profiles.profiles[0].enabled = false;
        assert_eq!(
            resolve_profile(&profiles, &favorite),
            Err(BindingError::MissingProfile)
        );
        profiles.profiles[0].enabled = true;
        favorite.origin.profile = "Elsewhere".into();
        assert_eq!(
            resolve_profile(&profiles, &favorite),
            Err(BindingError::MissingProfile)
        );
    }
    #[test]
    fn local_root_alias_disagreement_cannot_pick_a_source() {
        let (profiles, mut favorite) = local();
        favorite.origin.source_alias = "Another root".into();
        assert_eq!(
            resolve_profile(&profiles, &favorite),
            Err(BindingError::SourceMismatch)
        );
    }
    #[test]
    fn plex_uses_verified_server_identity_not_friendly_name() {
        let mut p = profile(ProviderKind::Plex);
        p.source = "server-123".into();
        let e = entry(MediaLocator::Plex {
            server_id: p.source.clone(),
            rating_key: "42".into(),
        });
        let favorite = capture(&p, &e).unwrap();
        assert_eq!(favorite.origin.profile, "Plex");
        assert_eq!(favorite.origin.source_alias, "server-123");
        p.name = "Renamed server".into();
        assert!(resolve_profile(&profiles(p.clone()), &favorite).is_ok());
        p.source = "different-server".into();
        assert_eq!(
            resolve_profile(&profiles(p.clone()), &favorite),
            Err(BindingError::MissingProfile)
        );
        assert!(capture(&p, &e).is_none());
    }
    #[test]
    fn old_native_plex_and_local_keep_exact_bindings() {
        let (source_profiles, mut local) = local();
        local.origin.source_alias = "Home".into();
        if let Locator::Local { root_alias, .. } = &mut local.locator {
            *root_alias = "Home".into();
        }
        assert!(resolve_profile(&source_profiles, &local).is_ok());
        let mut p = profile(ProviderKind::Plex);
        p.source = "server-123".into();
        let mut f = capture(
            &p,
            &entry(MediaLocator::Plex {
                server_id: p.source.clone(),
                rating_key: "42".into(),
            }),
        )
        .unwrap();
        f.origin.profile = "Home".into();
        f.origin.source_alias = "Home".into();
        assert!(resolve_profile(&profiles(p), &f).is_ok());
    }
    #[test]
    fn portal_subprofile_and_id_field_are_part_of_item_identity() {
        let p = profile(ProviderKind::VPortal);
        let mut e = entry(MediaLocator::VPortal {
            request: serde_json::json!({"id":42}),
        });
        let f = capture(&p, &e).unwrap();
        assert_eq!(f.origin.subprofile.as_deref(), Some("Series / Season 2"));
        assert!(resolve_profile(&profiles(p), &f).is_ok());
        assert!(matches_entry(&f, &e));
        e.category = "Series / Season 1".into();
        assert!(!matches_entry(&f, &e));
        e.category = "Series / Season 2".into();
        e.provider_id_field = Some("fid".into());
        assert!(!matches_entry(&f, &e));
    }
    #[test]
    fn unsupported_embedded_portal_is_retained_not_bound_to_m3u_url() {
        let p = profile(ProviderKind::VPortal);
        let mut f = capture(
            &p,
            &entry(MediaLocator::VPortal {
                request: serde_json::json!({}),
            }),
        )
        .unwrap();
        f.origin.provider = "m3u".into();
        let before = f.clone();
        assert_eq!(
            resolve_profile(&profiles(profile(ProviderKind::M3u)), &f),
            Err(BindingError::UnsupportedSource)
        );
        assert_eq!(f, before);
    }
    #[test]
    fn m3u_tvg_id_namespace_roundtrips_without_prefix_collision() {
        let p = profile(ProviderKind::M3u);
        let mut e = entry(MediaLocator::Http {
            url: "https://private.invalid/video".into(),
        });
        let f = capture(&p, &e).unwrap();
        assert_eq!(
            f.locator,
            Locator::Channel {
                channel_id: "m3u-epg:42".into()
            }
        );
        assert!(matches_entry(&f, &e));
        e.provider_id = Some("m3u-epg:42".into());
        assert!(!matches_entry(&f, &e));
        assert!(legacy_id_collision(&f, &e));
        let literal_prefix = capture(&p, &e).unwrap();
        assert!(matches_entry(&literal_prefix, &e));
        assert!(!legacy_id_collision(&literal_prefix, &e));
        e.id = "different-slot-row".into();
        assert!(matches_entry(&literal_prefix, &e));
    }
    #[test]
    fn nonportable_channel_id_uses_exact_title_and_group() {
        let p = profile(ProviderKind::M3u);
        let mut e = entry(MediaLocator::Http {
            url: "https://private.invalid/video".into(),
        });
        e.provider_id = Some("invalid id".into());
        let f = capture(&p, &e).unwrap();
        assert!(matches!(f.locator, Locator::ChannelName { .. }));
        assert!(matches_entry(&f, &e));
        e.category = "Other".into();
        assert!(!matches_entry(&f, &e));
    }
    #[test]
    fn xtream_and_stalker_do_not_export_raw_ids_as_web_keys() {
        for kind in [ProviderKind::Xtream, ProviderKind::Stalker] {
            let p = profile(kind);
            let e = entry(MediaLocator::Http {
                url: "https://private.invalid/video".into(),
            });
            let f = capture(&p, &e).unwrap();
            assert!(matches!(f.locator, Locator::ChannelName { .. }));
            assert!(resolve_profile(&profiles(p), &f).is_ok());
            assert!(matches_entry(&f, &e));
        }
    }

    fn assert_old_record_toggles_off(profile: ProviderProfile, entry: CatalogEntry, old: Favorite) {
        let canonical = capture(&profile, &entry).unwrap();
        assert_ne!(old.identity(), canonical.identity());
        let mut document = Document::default();
        assert!(document.toggle(old.clone()).unwrap());
        let source_profiles = profiles(profile.clone());
        let selected = existing_identity(&source_profiles, &document, &profile, &entry).unwrap();
        assert_eq!(selected, Some(old.identity()));
        let index = FavoriteIndex::new(&source_profiles, &document, &profile).unwrap();
        assert_eq!(index.identity(&entry).unwrap(), selected.as_deref());
        assert!(index.contains(&entry));
        assert!(document.remove(selected.as_deref().unwrap()));
        assert!(document.entries().is_empty());
        assert_eq!(
            existing_identity(&source_profiles, &document, &profile, &entry),
            Ok(None)
        );
    }

    #[test]
    fn legacy_display_name_aliases_rebind_and_toggle_without_rewriting_portable_identity() {
        for kind in [ProviderKind::M3u, ProviderKind::Xtream, ProviderKind::Stalker, ProviderKind::VPortal] {
            let mut p = profile(kind);
            p.name = "FamilyTV".into();
            p.source_alias = "home".into();
            let mut e = entry(MediaLocator::Http { url: "https://private.invalid/video".into() });
            if kind == ProviderKind::VPortal {
                e.provider_id = Some("42".into());
                e.provider_id_field = Some("id".into());
            }
            let canonical = capture(&p, &e).unwrap();
            assert_eq!(canonical.origin.source_alias, "home");
            let mut legacy = canonical.clone();
            legacy.origin.source_alias = "FamilyTV".into();
            let configured = profiles(p.clone());
            assert_eq!(resolve_profile(&configured, &legacy).unwrap(), &p);
            assert_eq!(legacy.origin.source_alias, "FamilyTV", "reading must not rewrite imported identity");
            assert_old_record_toggles_off(p, e, legacy);
        }
    }

    #[test]
    fn legacy_name_alias_counts_all_same_name_sources_while_modern_alias_is_specific() {
        let mut home = profile(ProviderKind::M3u);
        home.name = "FamilyTV".into(); home.source_alias = "home".into();
        let e = entry(MediaLocator::Http { url: "https://private.invalid/video".into() });
        let modern = capture(&home, &e).unwrap();
        let mut legacy = modern.clone(); legacy.origin.source_alias = home.name.clone();
        let mut other = home.clone();
        other.id = "other-slot".into(); other.source_alias = "holiday".into();
        other.source = "https://other.invalid/list.m3u".into();
        let mut configured = profiles(home.clone()); configured.profiles.push(other);
        for reverse in [false, true] {
            if reverse { configured.profiles.reverse(); }
            assert_eq!(resolve_profile(&configured, &legacy), Err(BindingError::AmbiguousSource));
            assert_eq!(resolve_profile(&configured, &modern).unwrap(), &home);
        }
        let mut document = Document::default(); document.toggle(legacy.clone()).unwrap();
        assert_eq!(existing_identity(&configured, &document, &home, &e), Err(BindingError::AmbiguousSource));
        assert_eq!(FavoriteIndex::new(&configured, &document, &home).unwrap().identity(&e),
            Err(BindingError::AmbiguousSource), "row stars must preserve the same ambiguity refusal");
        let mut invalid = modern.clone(); invalid.origin.source_alias = "unknown-alias".into();
        assert_eq!(resolve_profile(&configured, &invalid), Err(BindingError::MissingProfile));
        configured.profiles.iter_mut().find(|p| p.id == "other-slot").unwrap().enabled = false;
        assert_eq!(resolve_profile(&configured, &legacy).unwrap(), &home,
            "disabled accounts are not receiver candidates");
    }

    #[test]
    fn legacy_name_alias_never_prefers_an_exact_name_alias_over_another_matching_source() {
        let mut explicit = profile(ProviderKind::M3u);
        explicit.name = "FamilyTV".into(); explicit.source_alias = "home".into();
        let e = entry(MediaLocator::Http { url: "https://private.invalid/video".into() });
        let mut legacy = capture(&explicit, &e).unwrap(); legacy.origin.source_alias = explicit.name.clone();
        let mut exact = explicit.clone(); exact.id = "exact-slot".into(); exact.source_alias = exact.name.clone();
        let mut configured = profiles(exact); configured.profiles.push(explicit);
        assert_eq!(resolve_profile(&configured, &legacy), Err(BindingError::AmbiguousSource),
            "legacy fallback cannot guess between exact and historical name matches");
    }

    #[test]
    fn old_m3u_plex_and_local_records_toggle_off_without_canonical_duplicates() {
        let p = profile(ProviderKind::M3u);
        let e = entry(MediaLocator::Http {
            url: "https://private.invalid/video".into(),
        });
        let mut old = capture(&p, &e).unwrap();
        old.locator = Locator::Channel {
            channel_id: "42".into(),
        };
        assert_old_record_toggles_off(p, e, old);

        let mut p = profile(ProviderKind::Plex);
        p.source = "server-123".into();
        let e = entry(MediaLocator::Plex {
            server_id: p.source.clone(),
            rating_key: "42".into(),
        });
        let mut old = capture(&p, &e).unwrap();
        old.origin.profile = p.name.clone();
        old.origin.source_alias = p.name.clone();
        assert_old_record_toggles_off(p, e, old);

        let p = profile(ProviderKind::Local);
        let e = entry(MediaLocator::Local {
            relative_path: "Season 2/Film.mp4".into(),
        });
        let mut old = capture(&p, &e).unwrap();
        old.origin.source_alias = p.name.clone();
        if let Locator::Local { root_alias, .. } = &mut old.locator {
            *root_alias = p.name.clone();
        }
        assert_old_record_toggles_off(p, e, old);
    }

    #[test]
    fn a_matching_channel_in_another_profile_is_not_removed() {
        let first = profile(ProviderKind::M3u);
        let mut second = first.clone();
        second.id = "second-slot".into();
        second.name = "Second".into();
        let e = entry(MediaLocator::Http {
            url: "https://private.invalid/video".into(),
        });
        let mut document = Document::default();
        let first_favorite = capture(&first, &e).unwrap();
        document.toggle(first_favorite.clone()).unwrap();
        let mut source_profiles = profiles(first);
        source_profiles.profiles.push(second.clone());
        assert_eq!(
            existing_identity(&source_profiles, &document, &second, &e),
            Ok(None)
        );
        document.toggle(capture(&second, &e).unwrap()).unwrap();
        assert_eq!(document.entries().len(), 2);
        assert!(document.contains(&first_favorite));
    }

    #[test]
    fn semantic_duplicates_and_ambiguous_profiles_do_not_choose_a_record() {
        let p = profile(ProviderKind::M3u);
        let e = entry(MediaLocator::Http {
            url: "https://private.invalid/video".into(),
        });
        let canonical = capture(&p, &e).unwrap();
        let mut old = canonical.clone();
        old.locator = Locator::Channel {
            channel_id: "42".into(),
        };
        let mut document = Document::default();
        document.toggle(canonical).unwrap();
        document.toggle(old.clone()).unwrap();
        let mut source_profiles = profiles(p.clone());
        assert_eq!(
            existing_identity(&source_profiles, &document, &p, &e),
            Err(BindingError::AmbiguousSource)
        );
        assert_eq!(document.entries().len(), 2);
        document.remove(&old.identity());
        let mut duplicate = p.clone();
        duplicate.id = "other-slot".into();
        source_profiles.profiles.push(duplicate);
        assert_eq!(
            existing_identity(&source_profiles, &document, &p, &e),
            Err(BindingError::AmbiguousSource)
        );
    }

    #[test]
    fn prefix_collision_does_not_erase_or_duplicate_a_saved_record() {
        let p = profile(ProviderKind::M3u);
        let mut e = entry(MediaLocator::Http {
            url: "https://private.invalid/video".into(),
        });
        let saved = capture(&p, &e).unwrap();
        let mut document = Document::default();
        document.toggle(saved.clone()).unwrap();
        e.provider_id = Some("m3u-epg:42".into());
        assert_eq!(
            existing_identity(&profiles(p.clone()), &document, &p, &e),
            Err(BindingError::AmbiguousSource)
        );
        assert_eq!(document.entries(), &[saved]);
    }

    #[test]
    fn null_group_is_unspecified_and_overlapping_saved_groups_are_ambiguous() {
        let p = profile(ProviderKind::M3u);
        let mut e = entry(MediaLocator::Http {
            url: "https://private.invalid/video".into(),
        });
        e.provider_id = None;
        let scoped = capture(&p, &e).unwrap();
        let mut unscoped = scoped.clone();
        if let Locator::ChannelName { group, .. } = &mut unscoped.locator {
            *group = None;
        }
        assert!(matches_entry(&unscoped, &e));
        let mut document = Document::default();
        document.toggle(unscoped.clone()).unwrap();
        let source_profiles = profiles(p.clone());
        assert_eq!(
            existing_identity(&source_profiles, &document, &p, &e),
            Ok(Some(unscoped.identity()))
        );
        document.toggle(scoped).unwrap();
        assert_eq!(
            existing_identity(&source_profiles, &document, &p, &e),
            Err(BindingError::AmbiguousSource)
        );
    }

    #[test]
    fn stale_profile_snapshot_cannot_remove_from_a_reconfigured_source() {
        let p = profile(ProviderKind::M3u);
        let e = entry(MediaLocator::Http {
            url: "https://private.invalid/video".into(),
        });
        let mut document = Document::default();
        document.toggle(capture(&p, &e).unwrap()).unwrap();
        let mut stale = p.clone();
        stale.source = "https://old.invalid/list.m3u".into();
        assert_eq!(
            existing_identity(&profiles(p), &document, &stale, &e),
            Err(BindingError::MissingProfile)
        );
        assert_eq!(document.entries().len(), 1);
    }

    fn assert_index_matches_scan(
        source_profiles: &ProviderProfiles,
        document: &Document,
        profile: &ProviderProfile,
        entries: &[CatalogEntry],
    ) {
        let index = FavoriteIndex::new(source_profiles, document, profile).unwrap();
        for entry in entries {
            let expected = existing_identity(source_profiles, document, profile, entry);
            assert_eq!(
                index.identity(entry).map(|id| id.map(str::to_owned)),
                expected
            );
            assert_eq!(index.contains(entry), matches!(expected, Ok(Some(_))));
        }
    }

    #[test]
    fn index_combines_ids_and_scoped_or_wildcard_names_without_choosing_duplicates() {
        let p = profile(ProviderKind::M3u);
        let e = entry(MediaLocator::Http {
            url: "https://private.invalid/video".into(),
        });
        let mut another_group = e.clone();
        another_group.category = "Other".into();
        let canonical = capture(&p, &e).unwrap();
        let mut scoped = canonical.clone();
        scoped.locator = channel_name(&e);
        let mut wildcard = scoped.clone();
        if let Locator::ChannelName { group, .. } = &mut wildcard.locator {
            *group = None;
        }
        let source_profiles = profiles(p.clone());
        let entries = [e.clone(), another_group.clone()];
        let mut document = Document::default();
        document.toggle(canonical.clone()).unwrap();
        document.toggle(scoped.clone()).unwrap();
        assert_index_matches_scan(&source_profiles, &document, &p, &entries);
        let index = FavoriteIndex::new(&source_profiles, &document, &p).unwrap();
        assert_eq!(index.identity(&e), Err(BindingError::AmbiguousSource));
        assert!(index.contains(&another_group));
        document.remove(&canonical.identity());
        document.toggle(wildcard.clone()).unwrap();
        assert_index_matches_scan(&source_profiles, &document, &p, &entries);
        let index = FavoriteIndex::new(&source_profiles, &document, &p).unwrap();
        assert_eq!(index.identity(&e), Err(BindingError::AmbiguousSource));
        assert_eq!(
            index.identity(&another_group),
            Ok(Some(wildcard.identity().as_str()))
        );
    }

    #[test]
    fn index_keeps_portal_id_field_and_optional_category_exact() {
        let p = profile(ProviderKind::VPortal);
        let first = entry(MediaLocator::VPortal {
            request: serde_json::json!({"id":42}),
        });
        let mut second = first.clone();
        second.category = "Other season".into();
        let mut foreign_field = first.clone();
        foreign_field.provider_id_field = Some("fid".into());
        let first_favorite = capture(&p, &first).unwrap();
        let second_favorite = capture(&p, &second).unwrap();
        let mut document = Document::default();
        document.toggle(first_favorite.clone()).unwrap();
        document.toggle(second_favorite.clone()).unwrap();
        let source_profiles = profiles(p.clone());
        let entries = [first.clone(), second.clone(), foreign_field.clone()];
        assert_index_matches_scan(&source_profiles, &document, &p, &entries);
        let index = FavoriteIndex::new(&source_profiles, &document, &p).unwrap();
        assert_eq!(
            index.identity(&first),
            Ok(Some(first_favorite.identity().as_str()))
        );
        assert_eq!(
            index.identity(&second),
            Ok(Some(second_favorite.identity().as_str()))
        );
        assert!(!index.contains(&foreign_field));
        let mut unscoped = first_favorite;
        unscoped.origin.subprofile = None;
        document.toggle(unscoped).unwrap();
        assert_index_matches_scan(&source_profiles, &document, &p, &entries);
        let index = FavoriteIndex::new(&source_profiles, &document, &p).unwrap();
        assert_eq!(index.identity(&first), Err(BindingError::AmbiguousSource));
        assert_eq!(index.identity(&second), Err(BindingError::AmbiguousSource));
    }

    #[test]
    fn index_blocks_only_matching_ambiguous_sources_and_legacy_prefixes() {
        let p = profile(ProviderKind::M3u);
        let e = entry(MediaLocator::Http {
            url: "https://private.invalid/video".into(),
        });
        let mut literal = e.clone();
        literal.provider_id = Some("m3u-epg:42".into());
        let mut unrelated = e.clone();
        unrelated.provider_id = Some("999".into());
        unrelated.title = "Other".into();
        let mut document = Document::default();
        document.toggle(capture(&p, &e).unwrap()).unwrap();
        let mut source_profiles = profiles(p.clone());
        let entries = [e.clone(), literal.clone(), unrelated.clone()];
        assert_index_matches_scan(&source_profiles, &document, &p, &entries);
        let index = FavoriteIndex::new(&source_profiles, &document, &p).unwrap();
        assert_eq!(index.identity(&literal), Err(BindingError::AmbiguousSource));
        let mut duplicate = p.clone();
        duplicate.id = "second-slot".into();
        source_profiles.profiles.push(duplicate);
        assert_index_matches_scan(&source_profiles, &document, &p, &entries);
        let index = FavoriteIndex::new(&source_profiles, &document, &p).unwrap();
        assert_eq!(index.identity(&e), Err(BindingError::AmbiguousSource));
        assert_eq!(index.identity(&unrelated), Ok(None));
        let mut stale = p;
        stale.source = "https://changed.invalid/list".into();
        assert!(matches!(
            FavoriteIndex::new(&source_profiles, &document, &stale),
            Err(BindingError::MissingProfile)
        ));
    }

    #[test]
    fn index_retains_one_owned_revision_without_borrowing_mutable_profile_state() {
        let p = profile(ProviderKind::M3u);
        let e = entry(MediaLocator::Http {
            url: "https://private.invalid/video".into(),
        });
        let favorite = capture(&p, &e).unwrap();
        let mut document = Document::default();
        document.toggle(favorite.clone()).unwrap();
        let old = FavoriteIndex::new(&profiles(p.clone()), &document, &p).unwrap();
        document.remove(&favorite.identity());
        let new = FavoriteIndex::new(&profiles(p.clone()), &document, &p).unwrap();
        assert!(old.contains(&e));
        assert!(!new.contains(&e));
    }

    #[test]
    fn one_index_projects_fifty_thousand_rows_against_a_full_favorites_document() {
        let p = profile(ProviderKind::M3u);
        let mut row = entry(MediaLocator::Http {
            url: "https://private.invalid/video".into(),
        });
        let favorites: Vec<_> = (0..1000)
            .map(|number| {
                row.provider_id = Some(number.to_string());
                let mut favorite = capture(&p, &row).unwrap();
                if number % 2 == 0 {
                    favorite.locator = Locator::Channel {
                        channel_id: number.to_string(),
                    };
                }
                favorite
            })
            .collect();
        let document = Document::parse(
            &serde_json::to_vec(&serde_json::json!({"version":1,"entries":favorites})).unwrap(),
        )
        .unwrap();
        let index = FavoriteIndex::new(&profiles(p.clone()), &document, &p).unwrap();
        let mut starred = 0;
        for number in 0..50_000 {
            row.provider_id = Some(number.to_string());
            if index.contains(&row) {
                starred += 1;
            }
        }
        assert_eq!(starred, 1000);
    }
}

#[cfg(test)]
mod edem_favorite_tests {
    use super::*;
    #[test]
    fn edem_live_template_and_portal_favorites_keep_distinct_portable_identity() {
        let profile=ProviderProfile{id:"edem-1".into(),name:"Edem".into(),kind:ProviderKind::Edem,source:r#"{"key":"test_key","list":2}"#.into(),source_alias:String::new(),enabled:true};
        let template=plx_data::ott::edem::Config::parse(&profile.source).unwrap().template_profile(&profile);
        let mut row=plx_data::ott::parse_m3u("#EXTM3U\n#EXTINF:-1 group-title=\"News\",Channel\nhttp://media.invalid/live\n",&template).unwrap().entries.remove(0);
        let live=capture(&profile,&row).unwrap();
        assert_eq!(live.origin.subprofile.as_deref(),Some("Playlist 3"));
        let profiles=ProviderProfiles{version:1,active_profile:Some(profile.id.clone()),profiles:vec![profile.clone()]};
        assert!(resolve_profile(&profiles,&live).is_ok());
        row.live=None;row.provider_id=Some("47".into());row.provider_id_field=Some("id".into());row.category="Movies / Comedy".into();
        let movie=capture(&profile,&row).unwrap();
        assert!(matches!(movie.locator,Locator::VPortal{..}));
        assert_eq!(movie.origin.subprofile.as_deref(),Some("Movies / Comedy"));
        assert!(resolve_profile(&profiles,&movie).is_ok());
        assert!(matches_entry(&movie,&row));
    }
}

#[cfg(test)]
mod operator_default_identity_tests {
    use super::*;
    #[test]
    fn default_brand_names_are_portable_and_duplicate_accounts_remain_ambiguous() {
        let id=plx_data::ott::OperatorId::from_id("great").unwrap();
        let profile=ProviderProfile{id:"great-1".into(),name:"great 1".into(),kind:ProviderKind::Operator(id),source:r#"{"m3u":"https://playlist.invalid/list.m3u"}"#.into(),source_alias:String::new(),enabled:true};
        let mut template=profile.clone();template.kind=ProviderKind::M3u;template.source="https://playlist.invalid/list.m3u".into();
        let row=plx_data::ott::parse_m3u("#EXTM3U\n#EXTINF:-1,Fixture\nhttps://media.invalid/live.ts\n",&template).unwrap().entries.remove(0);
        let favorite=capture(&profile,&row).unwrap();assert_eq!(favorite.origin.profile,id.name());
        let mut receiver=profile.clone();receiver.name=id.name().into();
        let mut profiles=ProviderProfiles{version:1,active_profile:None,profiles:vec![receiver.clone()]};assert!(resolve_profile(&profiles,&favorite).is_ok());
        let mut isolated=receiver.clone();isolated.source_alias="account-B".into();
        let isolated_profiles=ProviderProfiles{version:1,active_profile:None,profiles:vec![isolated.clone()]};
        assert!(resolve_profile(&isolated_profiles,&favorite).is_err(),"Default brand must not select another explicit account");
        let explicit=capture(&isolated,&row).unwrap();assert_eq!(explicit.origin.source_alias,"account-B");
        assert!(resolve_profile(&isolated_profiles,&explicit).is_ok());
        receiver.id="great-duplicate".into();profiles.profiles.push(receiver);
        assert_eq!(resolve_profile(&profiles,&favorite).err(),Some(BindingError::AmbiguousSource));
        let mut custom=profile.clone();custom.name="Account two".into();custom.source_alias="Account two".into();
        assert_eq!(capture(&custom,&row).unwrap().origin.profile,"Account two");
    }
}
