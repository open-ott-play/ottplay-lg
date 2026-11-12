//! Favorites resolve against a complete, owner-supplied source snapshot. This
//! module neither stores sources nor turns imported names/paths into authority.
use super::*;
use crate::app::ott_local_session::{AuthorityRef, BoundSource, SourceKind};
use std::collections::HashSet;
use crate::app::ott_m3u_attachment::{Authorities as AttachmentAuthorities, Receipt as AttachmentReceipt};

/// A source owner from the currently admitted catalog/playing item. A temporary
/// Local root is deliberately not a numbered ProviderProfile.
#[derive(Clone, Copy)]
pub(in crate::app::ott_runtime) enum SelectionOwner<'a> {
    Profile(&'a ProviderProfile),
    LocalRoot(&'a BoundSource),
    M3uAttachment(&'a AttachmentReceipt),
}

pub(in crate::app::ott_runtime) struct SelectionBindings<'a> {
    profiles: &'a ProviderProfiles,
    temporary_profile: Option<&'a ProviderProfile>,
    local_sources: Vec<&'a BoundSource>,
    registry_generation: u64,
    attachments: Option<&'a AttachmentAuthorities>,
}

/// Private receipt for delayed metadata/resolver results. Never serialize it
/// into the portable Favorites document, or persist a live AuthorityRef.
#[derive(Clone, PartialEq, Eq)]
pub(in crate::app::ott_runtime) struct LocalBindingReceipt {
    source_id: String,
    source_kind: SourceKind,
    canonical_path: Option<String>,
    authority: AuthorityRef,
    source_generation: u64,
    registry_generation: u64,
}

impl<'a> SelectionBindings<'a> {
    /// The caller supplies its complete connected-source inventory, not one
    /// path-lookup result. Its existing source runner owns validation and IO.
    pub(in crate::app::ott_runtime) fn new(
        profiles: &'a ProviderProfiles,
        sources: impl IntoIterator<Item = &'a BoundSource>,
        registry_generation: u64,
    ) -> Result<Self, BindingError> {
        let local_sources: Vec<_> = sources.into_iter().take(33).collect();
        if local_sources.len() > 32 || (!local_sources.is_empty() && registry_generation == 0) {
            return Err(BindingError::SourceMismatch);
        }
        let mut ids = HashSet::new();
        for source in &local_sources {
            if source.id.is_empty()
                || source.id.len() > 256
                || !source
                    .id
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"_.:-".contains(&b))
                || !ids.insert(source.id.as_str())
                || source.name.trim().is_empty()
                || source.name.len() > 512
                || source.name.chars().any(char::is_control)
                || source.name.contains(['/', '\\'])
                || matches!(source.name.as_str(), "." | "..")
            {
                return Err(BindingError::SourceMismatch);
            }
        }
        Ok(Self {
            profiles,
            temporary_profile: None,
            local_sources,
            registry_generation,
            attachments: None,
        })
    }

    pub(in crate::app::ott_runtime) fn profiles_only(profiles: &'a ProviderProfiles) -> Self {
        Self {
            profiles,
            temporary_profile: None,
            local_sources: Vec::new(),
            registry_generation: 0,
            attachments: None,
        }
    }

    /// A currently admitted managed Plex source. This reference never enters
    /// ProviderProfiles or Saved; the runtime retires it with its source owner.
    pub(in crate::app::ott_runtime) fn with_temporary_profile(
        mut self, profile: Option<&'a ProviderProfile>,
    ) -> Self {
        self.temporary_profile = profile.filter(|profile|
            profile.enabled && profile.kind == ProviderKind::Plex);
        self
    }

    /// Sources observed by the runtime after durable state admission. This does
    /// not fetch a portal or make an effective profile a numbered profile.
    pub(in crate::app::ott_runtime) fn with_attachments(
        mut self, attachments: &'a AttachmentAuthorities,
    ) -> Self {
        self.attachments = Some(attachments);
        self
    }

    pub(in crate::app::ott_runtime) fn attachment_is_current(
        &self, receipt: &AttachmentReceipt, favorite: &Favorite,
    ) -> bool {
        matches!(self.resolve(favorite), Ok(SelectionOwner::M3uAttachment(current))
            if current == receipt)
    }

    pub(in crate::app::ott_runtime) fn resolve(
        &self,
        favorite: &Favorite,
    ) -> Result<SelectionOwner<'a>, BindingError> {
        if favorite.origin.provider == "m3u" && matches!(favorite.locator, Locator::VPortal { .. }) {
            favorite.validate().map_err(|_| BindingError::InvalidFavorite)?;
            let mut parents = self.profiles.profiles.iter().filter(|profile|
                profile.enabled && profile.kind == ProviderKind::M3u && profile_matches(profile, favorite));
            let parent = parents.next().ok_or(BindingError::MissingProfile)?;
            if parents.next().is_some() { return Err(BindingError::AmbiguousSource); }
            let receipt = self.attachments.and_then(|sources| sources.receipt(&parent.id))
                .ok_or(BindingError::MissingAttachment)?;
            let owner = SelectionOwner::M3uAttachment(receipt);
            self.validate_owner(owner)?;
            return Ok(owner);
        }
        // Keep the complete existing provider/locator validation and named
        // profile semantics, including ambiguity and legacy identities.
        let profile = match resolve_profile(self.profiles, favorite) {
            Ok(profile) => Some(profile),
            Err(BindingError::MissingProfile) => None,
            Err(error) => return Err(error),
        };
        let temporary = self.temporary_profile.filter(|candidate|
            favorite.origin.provider == "plex" && profile_matches(candidate, favorite));
        // The durable resolver above performs the full portable validation even
        // when no Saved row exists. Never guess between matching authorities.
        let profile = match (profile, temporary) {
            (Some(_), Some(_)) => return Err(BindingError::AmbiguousSource),
            (profile, temporary) => profile.or(temporary),
        };
        let mut root = None;
        if favorite.origin.provider == "local" && favorite.origin.profile == "Local videos" {
            let Locator::Local { root_alias, .. } = &favorite.locator else {
                return Err(BindingError::SourceMismatch);
            };
            for candidate in self
                .local_sources
                .iter()
                .filter(|source| source.name == *root_alias)
            {
                // Offline duplicates are still ambiguous. A profile and a root
                // may alias one physical folder, but path text is not proof.
                if root.is_some() || profile.is_some() {
                    return Err(BindingError::AmbiguousSource);
                }
                root = Some(*candidate);
            }
        }
        if let Some(root) = root {
            usable(root)?;
            if !local_favorite_path_matches(root, favorite) {
                return Err(BindingError::SourceMismatch);
            }
            Ok(SelectionOwner::LocalRoot(root))
        } else {
            let owner = profile.map(SelectionOwner::Profile)
                .ok_or(BindingError::MissingProfile)?;
            self.validate_owner(owner)?;
            Ok(owner)
        }
    }

    pub(in crate::app::ott_runtime) fn validate_owner(
        &self,
        owner: SelectionOwner<'_>,
    ) -> Result<(), BindingError> {
        match owner {
            SelectionOwner::Profile(profile) => {
                // A plain Local profile has folder semantics. A known SingleFile
                // must use its exact source owner, never a profile-shaped alias.
                if profile.kind == ProviderKind::Local && self.local_sources.iter().any(|source|
                    source.kind == SourceKind::File
                        && source.canonical_path.as_deref() == Some(profile.source.as_str())) {
                    return Err(BindingError::UnsupportedSource);
                }
                let durable = self.profiles.profiles.iter()
                    .filter(|row| row.enabled && *row == profile).count();
                let temporary = usize::from(self.temporary_profile == Some(profile));
                match durable + temporary {
                    0 => Err(BindingError::MissingProfile),
                    1 => Ok(()),
                    _ => Err(BindingError::AmbiguousSource),
                }
            }
            SelectionOwner::M3uAttachment(receipt) => {
                self.validate_owner(SelectionOwner::Profile(receipt.parent()))?;
                if receipt.parent().kind != ProviderKind::M3u
                    || !self.attachments.is_some_and(|sources| sources.is_current(receipt)) {
                    return Err(BindingError::SourceMismatch);
                }
                receipt.effective_profile().map_err(|_| BindingError::SourceMismatch)?;
                Ok(())
            }
            SelectionOwner::LocalRoot(source) => {
                let current = self
                    .local_sources
                    .iter()
                    .find(|row| row.id == source.id)
                    .ok_or(BindingError::MissingProfile)?;
                if !same_root(current, source)
                    || current.name != source.name
                    || current.kind != source.kind
                    || current.available != source.available
                    || current.canonical_path != source.canonical_path
                {
                    return Err(BindingError::SourceMismatch);
                }
                usable(current)
            }
        }
    }

    pub(in crate::app::ott_runtime) fn receipt(
        &self,
        owner: SelectionOwner<'_>,
    ) -> Result<Option<LocalBindingReceipt>, BindingError> {
        self.validate_owner(owner)?;
        Ok(match owner {
            SelectionOwner::Profile(_) | SelectionOwner::M3uAttachment(_) => None,
            SelectionOwner::LocalRoot(root) => Some(LocalBindingReceipt {
                source_id: root.id.clone(),
                source_kind: root.kind,
                canonical_path: root.canonical_path.clone(),
                authority: root.authority_ref,
                source_generation: root.generation,
                registry_generation: self.registry_generation,
            }),
        })
    }

    /// Rebinding also catches newly ambiguous names. The owner calls this before
    /// it admits an asynchronous result, in addition to its normal play ticket.
    pub(in crate::app::ott_runtime) fn receipt_is_current(
        &self,
        receipt: &LocalBindingReceipt,
        favorite: &Favorite,
    ) -> bool {
        if receipt.registry_generation != self.registry_generation {
            return false;
        }
        matches!(self.resolve(favorite), Ok(SelectionOwner::LocalRoot(root))
            if receipt.source_id == root.id && receipt.source_kind == root.kind
                && receipt.canonical_path == root.canonical_path
                && receipt.authority == root.authority_ref
                && receipt.source_generation == root.generation)
    }
}

fn usable(source: &BoundSource) -> Result<(), BindingError> {
    if source.kind == SourceKind::File && source.single_file_name().is_none() {
        return Err(BindingError::SourceUnavailable);
    }
    if !source.available {
        return Err(BindingError::SourceUnavailable);
    }
    if source.generation == 0 {
        return Err(BindingError::SourceMismatch);
    }
    Ok(())
}

fn same_root(first: &BoundSource, second: &BoundSource) -> bool {
    first.id == second.id
        && first.kind == second.kind
        && first.canonical_path == second.canonical_path
        && first.authority_ref == second.authority_ref
        && first.generation == second.generation
}

pub(in crate::app::ott_runtime) fn same_owner(
    first: SelectionOwner<'_>,
    second: SelectionOwner<'_>,
) -> bool {
    match (first, second) {
        (SelectionOwner::Profile(first), SelectionOwner::Profile(second)) => first == second,
        (SelectionOwner::M3uAttachment(first), SelectionOwner::M3uAttachment(second)) => first == second,
        (SelectionOwner::LocalRoot(first), SelectionOwner::LocalRoot(second)) => {
            same_root(first, second)
        }
        _ => false,
    }
}

/// Descriptor constraint only. A File owner can never widen a portable path
/// into its parent or sibling; the runtime still checks the held read grant.
pub(super) fn local_favorite_path_matches(source: &BoundSource, favorite: &Favorite) -> bool {
    match (&source.kind, &favorite.locator) {
        (SourceKind::Folder, Locator::Local { .. }) => true,
        (SourceKind::File, Locator::Local { relative_path, .. }) =>
            source.single_file_name() == Some(relative_path.as_str()),
        _ => false,
    }
}

pub(in crate::app::ott_runtime) fn owner_matches(
    owner: SelectionOwner<'_>,
    favorite: &Favorite,
) -> bool {
    match owner {
        SelectionOwner::Profile(profile) => {
            favorite.origin.provider == profile.kind.canonical_id()
                && profile_matches(profile, favorite)
        }
        SelectionOwner::M3uAttachment(receipt) => {
            favorite.origin.provider == "m3u"
                && matches!(favorite.locator, Locator::VPortal { .. })
                && profile_matches(receipt.parent(), favorite)
        }
        SelectionOwner::LocalRoot(root) => {
            favorite.origin.provider == "local"
                && favorite.origin.profile == "Local videos"
                && favorite.origin.subprofile.is_none()
                && favorite.origin.source_alias == root.name
                && matches!(&favorite.locator, Locator::Local {root_alias, ..} if *root_alias == root.name)
                && local_favorite_path_matches(root, favorite)
        }
    }
}

pub(in crate::app::ott_runtime) fn bind_bound<'a>(
    bindings: &SelectionBindings<'a>,
    favorite: &Favorite,
) -> Result<SelectionOwner<'a>, BindingError> {
    bindings.resolve(favorite)
}

/// Shared source-to-row proof for capture and bound index/toggle queries.
/// A relative local locator relies on the admitted catalog owner; server rows
/// additionally carry their own source ID and portable alias and must agree.
pub(super) fn local_entry_path<'a>(
    source_id: &str,
    alias: &str,
    entry: &'a CatalogEntry,
) -> Result<&'a str, BindingError> {
    match &entry.media {
        MediaLocator::Local { relative_path } => Ok(relative_path),
        MediaLocator::ServerLibrary {
            source_id: row_source,
            relative_path: Some(path),
            root_alias: Some(row_alias),
            ..
        } if row_source == source_id && row_alias == alias => Ok(path),
        _ => Err(BindingError::SourceMismatch),
    }
}

pub(in crate::app::ott_runtime) fn capture_bound(
    bindings: &SelectionBindings<'_>,
    owner: SelectionOwner<'_>,
    entry: &CatalogEntry,
) -> Result<Favorite, BindingError> {
    bindings.validate_owner(owner)?;
    let favorite = match owner {
        SelectionOwner::Profile(profile) => {
            capture(profile, entry).ok_or(BindingError::InvalidFavorite)?
        }
        SelectionOwner::M3uAttachment(receipt) => {
            let effective = receipt.effective_profile().map_err(|_| BindingError::SourceMismatch)?;
            // The typed owner attests this catalog; the row namespace also must
            // belong to its effective profile, never a UI-selected TV source.
            if !entry.id.strip_prefix(effective.id.as_str()).is_some_and(|rest| rest.starts_with(':')) {
                return Err(BindingError::SourceMismatch);
            }
            let mut favorite = capture(&effective, entry).ok_or(BindingError::InvalidFavorite)?;
            favorite.origin.provider = "m3u".into();
            favorite.origin.profile = receipt.parent().name.clone();
            favorite.origin.source_alias = receipt.parent().alias().to_owned();
            favorite
        }
        SelectionOwner::LocalRoot(root) => {
            let path = local_entry_path(&root.id, &root.name, entry)?;
            if root.kind == SourceKind::File && root.single_file_name() != Some(path) {
                return Err(BindingError::SourceMismatch);
            }
            Favorite {
                title: entry.title.clone(),
                origin: Origin {
                    provider: "local".into(),
                    profile: "Local videos".into(),
                    subprofile: None,
                    source_alias: root.name.clone(),
                },
                locator: Locator::Local {
                    root_alias: root.name.clone(),
                    relative_path: path.to_owned(),
                },
            }
        }
    };
    favorite
        .validate()
        .map_err(|_| BindingError::InvalidFavorite)?;
    if !same_owner(bindings.resolve(&favorite)?, owner) {
        return Err(BindingError::SourceMismatch);
    }
    Ok(favorite)
}

pub(in crate::app::ott_runtime) fn existing_identity_bound(
    bindings: &SelectionBindings<'_>,
    document: &Document,
    owner: SelectionOwner<'_>,
    entry: &CatalogEntry,
) -> Result<Option<String>, BindingError> {
    FavoriteIndex::new_bound(bindings, document, owner)?
        .identity(entry)
        .map(|id| id.map(str::to_owned))
}

#[cfg(test)]
mod managed_profile_tests {
    use super::*;

    fn profile(id: &str, source: &str) -> ProviderProfile {
        ProviderProfile { id: id.into(), name: "Managed library".into(),
            kind: ProviderKind::Plex, source: source.into(), source_alias: String::new(), enabled: true }
    }
    fn entry(source: &str) -> CatalogEntry {
        CatalogEntry { protection: None, synopsis: None, id: "managed:42".into(), title: "Film".into(), category: String::new(),
            description: String::new(), poster: String::new(),
            media: MediaLocator::Plex { server_id: source.into(), rating_key: "42".into() },
            duration_ms: None, provider_id: None, provider_id_field: None, live: None }
    }

    #[test]
    fn managed_profile_capture_and_rebind_are_temporary() {
        let saved = ProviderProfiles::default();
        let temporary = profile("managed-7", "server-7");
        let row = entry("server-7");
        let bindings = SelectionBindings::profiles_only(&saved)
            .with_temporary_profile(Some(&temporary));
        let favorite = capture_bound(&bindings, SelectionOwner::Profile(&temporary), &row).unwrap();
        assert!(matches!(bindings.resolve(&favorite), Ok(SelectionOwner::Profile(bound)) if bound == &temporary));
        assert_eq!(favorite.origin.profile, "Plex");
        assert_eq!(favorite.origin.source_alias, "server-7");
        assert!(saved.profiles.is_empty());
        assert!(saved.active_profile.is_none());
        // Once the runtime owner is absent, the same portable record cannot
        // recreate authority from its source ID or the old temporary name.
        assert!(matches!(SelectionBindings::profiles_only(&saved).resolve(&favorite), Err(BindingError::MissingProfile)));
    }

    #[test]
    fn managed_profile_refuses_durable_ambiguity() {
        let temporary = profile("managed-7", "server-7");
        let saved = ProviderProfiles { profiles: vec![profile("saved-7", "server-7")], ..Default::default() };
        let bindings = SelectionBindings::profiles_only(&saved)
            .with_temporary_profile(Some(&temporary));
        assert!(matches!(capture_bound(&bindings, SelectionOwner::Profile(&temporary), &entry("server-7")),
            Err(BindingError::AmbiguousSource)));
        // Even exact duplicates are ambiguity, never two interchangeable owners.
        let saved = ProviderProfiles { profiles: vec![temporary.clone()], ..Default::default() };
        assert!(matches!(SelectionBindings::profiles_only(&saved).with_temporary_profile(Some(&temporary))
            .validate_owner(SelectionOwner::Profile(&temporary)), Err(BindingError::AmbiguousSource)));
    }

    #[test]
    fn managed_profile_cannot_bind_foreign_or_disabled_source() {
        let saved = ProviderProfiles::default();
        let mut temporary = profile("managed-7", "server-7");
        assert!(matches!(capture_bound(&SelectionBindings::profiles_only(&saved).with_temporary_profile(Some(&temporary)),
            SelectionOwner::Profile(&temporary), &entry("other-server")), Err(BindingError::InvalidFavorite)));
        temporary.enabled = false;
        assert!(matches!(SelectionBindings::profiles_only(&saved).with_temporary_profile(Some(&temporary))
            .validate_owner(SelectionOwner::Profile(&temporary)), Err(BindingError::MissingProfile)));
    }
}
