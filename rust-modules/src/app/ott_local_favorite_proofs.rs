//! Installation-private proof for ambiguous legacy Local basenames. Portable
//! Favorite documents deliberately contain no proof or receiving-device grant.
use super::{BindingError, SelectionBindings, SelectionOwner};
use crate::app::ott_local_session::{BoundSource, SourceKind};
use crate::app::ott_local_sources::{FolderDescriptor, RememberedFolders};
use crate::app::{ott_local_root::LocalSource, ott_usb::VerifiedRelocation};
use plx_data::ott::{favorites::{Document, Favorite, Locator, ENTRY_COUNT, DOCUMENT_BYTES},
    favorites_wire::hash, CatalogEntry, ProviderKind, ProviderProfile, ProviderProfiles};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::app::ott_runtime) struct LocalFavoriteProofs {
    entries: Vec<Proof>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Proof {
    favorite: Favorite,
    source: Source,
    item_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", deny_unknown_fields)]
enum Source {
    Profile { id: String, fingerprint: String },
    Root { id: String, fingerprint: String },
}

/// A short-lived permission for one lookup, not a persisted grant. The runner
/// must still recheck its existing source-generation/cancellation receipt.
#[derive(Clone)]
pub(in crate::app) struct LocalFavoritePermit {
    favorite: Favorite,
    profile: ProviderProfile,
    item_id: String,
}

impl LocalFavoritePermit {
    pub(in crate::app) fn matches(&self, profile: &ProviderProfile, favorite: &Favorite) -> bool {
        self.profile == *profile && self.favorite == *favorite
    }
    pub(in crate::app) fn item_id(&self) -> &str { &self.item_id }
}

fn single_segment(favorite: &Favorite) -> bool {
    matches!(&favorite.locator, Locator::Local { relative_path, .. } if !relative_path.contains('/'))
}

fn fingerprint(value: &impl Serialize) -> Result<String, BindingError> {
    serde_json::to_vec(value).map(|bytes| hash(&bytes)).map_err(|_| BindingError::InvalidFavorite)
}

/// The existing installation-private root proof identity. Generation and live
/// authority stay in the caller's separate receipt; this hash never grants access.
pub(in crate::app::ott_runtime) fn local_root_fingerprint(root: &BoundSource) -> Result<String, BindingError> {
    let path = root.canonical_path.as_deref().filter(|path| path.starts_with('/'))
        .ok_or(BindingError::SourceUnavailable)?;
    match root.kind {
        // Preserve every byte of the existing Folder proof domain.
        SourceKind::Folder => fingerprint(&("local-root-proof-v1", &root.id, &root.name, path)),
        SourceKind::File => {
            root.single_file_name().ok_or(BindingError::SourceUnavailable)?;
            fingerprint(&("local-file-proof-v1", "file", &root.id, &root.name, path))
        }
    }
}

fn source(owner: SelectionOwner<'_>) -> Result<Source, BindingError> {
    match owner {
        SelectionOwner::Profile(profile) if profile.kind == ProviderKind::Local =>
            Ok(Source::Profile { id: profile.id.clone(), fingerprint: fingerprint(profile)? }),
        SelectionOwner::LocalRoot(root) => {
            Ok(Source::Root { id: root.id.clone(), fingerprint: local_root_fingerprint(root)? })
        }
        _ => Err(BindingError::SourceMismatch),
    }
}

fn profile_matches_owner(owner: SelectionOwner<'_>, profile: &ProviderProfile) -> bool {
    match owner {
        SelectionOwner::Profile(actual) => actual == profile,
        SelectionOwner::M3uAttachment(receipt) => receipt.effective_profile().is_ok_and(|actual| actual == *profile),
        SelectionOwner::LocalRoot(root) => profile.kind == ProviderKind::Local
            && profile.enabled && Some(&profile.source) == root.canonical_path.as_ref()
            && profile.source_alias == root.name,
    }
}

fn captured_proof(owner: SelectionOwner<'_>, catalog: &[CatalogEntry],
    entry: &CatalogEntry, favorite: Favorite) -> Result<Proof, BindingError> {
    let mut matches = catalog.iter().filter(|row| super::matches_entry(&favorite, row));
    let actual = matches.next().ok_or(BindingError::SourceMismatch)?;
    if matches.next().is_some() || actual != entry { return Err(BindingError::SourceMismatch); }
    // capture_bound has already checked a server row's explicit source ID and alias.
    Ok(Proof { favorite, source: source(owner)?, item_id: entry.id.clone() })
}

impl LocalFavoriteProofs {
    /// Keep only pre-existing intent for one exact, still-configured stable USB
    /// folder while it is detached. This never inserts a proof or grants access:
    /// ordinary binding still rejects unavailable roots, and the worker must
    /// acquire a fresh held root before any later relocation can be accepted.
    /// A changed/missing/malformed candidate descriptor preserves revocation.
    pub(in crate::app::ott_runtime) fn invalidate_unavailable_root(
        &mut self,
        root: &BoundSource,
        retained_intent: Option<&FolderDescriptor>,
        candidate_folders: Option<&RememberedFolders>,
    ) {
        let retained_fingerprint = retained_intent.filter(|intent| {
            !root.available
                && root.kind == crate::app::ott_local_session::SourceKind::Folder
                && intent.validate().is_ok()
                && intent.usb.is_some()
                && intent.id == root.id && intent.name == root.name
                && Some(intent.path.as_str()) == root.canonical_path.as_deref()
                && candidate_folders.is_some_and(|folders| {
                    folders.validate().is_ok()
                        && folders.folders().iter().any(|candidate| candidate == *intent)
                })
        }).and_then(|_| local_root_fingerprint(root).ok());
        self.entries.retain(|proof| match &proof.source {
            Source::Root { id, fingerprint } if id == &root.id =>
                retained_fingerprint.as_ref() == Some(fingerprint),
            _ => true,
        });
    }

    /// Restore only baseline attestations whose exact source moved through the
    /// USB worker's verified same-filesystem relocation. Run AFTER ordinary root
    /// invalidation/profile reconciliation, then run ordinary document/pin/profile
    /// pruning again. The caller owns the unchanged Saved intent and commit CAS.
    /// This updates private source fingerprints, never Favorite or item identity.
    pub(in crate::app::ott_runtime) fn restore_verified_relocation(
        &mut self,
        previous: &Self,
        relocation: &VerifiedRelocation,
        current: &LocalSource,
        previous_profiles: &ProviderProfiles,
        candidate_profiles: &ProviderProfiles,
    ) -> Result<usize, BindingError> {
        // Validate complete documents before considering even one eligible row.
        self.validate()?;
        previous.validate()?;
        previous_profiles.validate().map_err(|_| BindingError::SourceMismatch)?;
        candidate_profiles.validate().map_err(|_| BindingError::SourceMismatch)?;
        relocation.stable_binding().validate().map_err(|_| BindingError::SourceMismatch)?;
        if !relocation.is_current(current) {
            return Err(BindingError::SourceUnavailable);
        }
        let old = relocation.old_root();
        let new = relocation.new_root();
        if old.kind != crate::app::ott_local_session::SourceKind::Folder
            || new.kind != crate::app::ott_local_session::SourceKind::Folder
            || old.id != new.id || old.name != new.name
            || old.authority_ref != new.authority_ref
            || old.generation == 0 || old.generation != new.generation
            || !new.available
        {
            return Err(BindingError::SourceMismatch);
        }
        let old_path = old.canonical_path.as_deref().ok_or(BindingError::SourceUnavailable)?;
        let new_path = new.canonical_path.as_deref().ok_or(BindingError::SourceUnavailable)?;
        let old_root = source(SelectionOwner::LocalRoot(old))?;
        let new_root = source(SelectionOwner::LocalRoot(new))?;
        let mut next = self.clone();
        let mut changed = 0;
        for proof in &previous.entries {
            let target = match &proof.source {
                source if source == &old_root => Some(new_root.clone()),
                Source::Profile { id, fingerprint: digest } => {
                    previous_profiles.profiles.iter().find(|profile|
                        profile.id == *id && profile.enabled && profile.kind == ProviderKind::Local
                            && profile.source == old_path
                            && fingerprint(profile).is_ok_and(|value| value == *digest))
                        .and_then(|profile| {
                            let mut expected = profile.clone();
                            expected.source = new_path.to_owned();
                            candidate_profiles.profiles.iter().find(|candidate| **candidate == expected)
                        })
                        .map(|profile| source(SelectionOwner::Profile(profile)))
                        .transpose()?
                }
                _ => None,
            };
            let Some(target) = target else { continue; };
            let mut rebased = proof.clone();
            rebased.source = target;
            if let Some(existing) = next.entries.iter_mut().find(|existing|
                existing.favorite.identity() == proof.favorite.identity())
            {
                // A newer attestation or changed Favorite must not be overwritten
                // by an older relocation receipt, even in a candidate document.
                if *existing == *proof && *existing != rebased {
                    *existing = rebased;
                    changed += 1;
                }
            } else {
                next.entries.push(rebased);
                changed += 1;
            }
        }
        next.validate()?;
        if !relocation.is_current(current) {
            return Err(BindingError::SourceUnavailable);
        }
        *self = next;
        Ok(changed)
    }

    pub(in crate::app::ott_runtime) fn validate(&self) -> Result<(), BindingError> {
        if self.entries.len() > ENTRY_COUNT || serde_json::to_vec(self)
            .map_err(|_| BindingError::InvalidFavorite)?.len() > DOCUMENT_BYTES {
            return Err(BindingError::InvalidFavorite);
        }
        let mut seen = HashSet::new();
        for proof in &self.entries {
            proof.favorite.validate().map_err(|_| BindingError::InvalidFavorite)?;
            let (id, digest) = match &proof.source {
                Source::Profile { id, fingerprint } | Source::Root { id, fingerprint } => (id, fingerprint),
            };
            if !single_segment(&proof.favorite) || !seen.insert(proof.favorite.identity())
                || proof.item_id.is_empty() || proof.item_id.len() > 4096
                || proof.item_id.chars().any(char::is_control)
                || id.is_empty() || id.len() > 256
                || digest.len() != 64 || !digest.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)) {
                return Err(BindingError::InvalidFavorite);
            }
        }
        Ok(())
    }

    /// Called only for a real Add from an admitted catalog. Neither parsing nor
    /// merging a portable Document calls this method. Publication uses the same
    /// Saved after-image as the Favorite; a failed write cannot publish one half.
    // No-pin shorthand for fixtures; production callers retain current kiosk pins.
    #[cfg(test)]
    pub(in crate::app::ott_runtime) fn record_capture(
        &mut self, bindings: &SelectionBindings<'_>, document: &Document,
        owner: SelectionOwner<'_>, catalog: &[CatalogEntry], entry: &CatalogEntry,
    ) -> Result<(), BindingError> {
        self.record_capture_with_pins(bindings, document, &[], owner, catalog, entry)
    }

    /// Capture an explicit Favorite Add while retaining proof used only by the
    /// current kiosk pins. The added Favorite must still belong to the document.
    pub(in crate::app::ott_runtime) fn record_capture_with_pins(
        &mut self, bindings: &SelectionBindings<'_>, document: &Document, pins: &[Favorite],
        owner: SelectionOwner<'_>, catalog: &[CatalogEntry], entry: &CatalogEntry,
    ) -> Result<(), BindingError> {
        let favorite = super::capture_bound(bindings, owner, entry)?;
        if !single_segment(&favorite) { return Ok(()); }
        if !document.entries().iter().any(|row| row == &favorite) {
            return Err(BindingError::InvalidFavorite);
        }
        let proof = captured_proof(owner, catalog, entry, favorite)?;
        self.insert_proof(document, pins, proof)
    }

    /// Capture only from the admitted catalog and an exact pin in the candidate
    /// kiosk state. A portable pin by itself can never create receiving-device proof.
    pub(in crate::app::ott_runtime) fn record_pin_capture(
        &mut self, bindings: &SelectionBindings<'_>, document: &Document, pins: &[Favorite],
        owner: SelectionOwner<'_>, catalog: &[CatalogEntry], entry: &CatalogEntry,
    ) -> Result<(), BindingError> {
        let favorite = super::capture_bound(bindings, owner, entry)?;
        if !pins.iter().any(|row| row == &favorite) {
            return Err(BindingError::InvalidFavorite);
        }
        if !single_segment(&favorite) { return Ok(()); }
        let proof = captured_proof(owner, catalog, entry, favorite)?;
        self.insert_proof(document, pins, proof)
    }

    fn insert_proof(&mut self, document: &Document, pins: &[Favorite], proof: Proof)
        -> Result<(), BindingError> {
        let mut next = self.clone();
        next.prune_with_pins(document, pins);
        next.entries.retain(|old| old.favorite.identity() != proof.favorite.identity());
        next.entries.push(proof);
        next.validate()?;
        *self = next;
        Ok(())
    }

    pub(in crate::app::ott_runtime) fn item_for(
        &self, owner: SelectionOwner<'_>, favorite: &Favorite,
    ) -> Result<Option<&str>, BindingError> {
        if let SelectionOwner::LocalRoot(root) = owner {
            if !super::bindings::local_favorite_path_matches(root, favorite) {
                return Err(BindingError::SourceMismatch);
            }
        }
        if !single_segment(favorite) { return Ok(None); }
        let current = source(owner)?;
        let mut matches = self.entries.iter().filter(|proof| proof.favorite == *favorite && proof.source == current);
        let proof = matches.next().ok_or(BindingError::LocalPathUnproven)?;
        if matches.next().is_some() { return Err(BindingError::InvalidFavorite); }
        Ok(Some(&proof.item_id))
    }

    /// Explicit Add may re-attest an imported same-identity basename. Render,
    /// import, and lookup never call this; the next explicit toggle removes it.
    // No-pin shorthand for fixtures; production callers retain current kiosk pins.
    #[cfg(test)]
    pub(in crate::app::ott_runtime) fn toggle_capture(
        &mut self, document: &mut Document, bindings: &SelectionBindings<'_>,
        owner: SelectionOwner<'_>, catalog: &[CatalogEntry], entry: &CatalogEntry,
    ) -> Result<bool, BindingError> {
        self.toggle_capture_with_pins(document, &[], bindings, owner, catalog, entry)
    }

    pub(in crate::app::ott_runtime) fn toggle_capture_with_pins(
        &mut self, document: &mut Document, pins: &[Favorite], bindings: &SelectionBindings<'_>,
        owner: SelectionOwner<'_>, catalog: &[CatalogEntry], entry: &CatalogEntry,
    ) -> Result<bool, BindingError> {
        let channel = super::capture_channel(bindings, owner, catalog, entry)?;
        self.toggle_capture_with_channel(document, pins, bindings, owner, catalog, entry, channel.as_ref())
    }

    pub(in crate::app::ott_runtime) fn toggle_capture_with_channel(
        &mut self, document: &mut Document, pins: &[Favorite], bindings: &SelectionBindings<'_>,
        owner: SelectionOwner<'_>, catalog: &[CatalogEntry], entry: &CatalogEntry,
        channel: Option<&super::ChannelCapture>,
    ) -> Result<bool, BindingError> {
        let captured = match channel {
            Some(proof) => proof.favorite(bindings, owner, entry)?,
            None => {
                if matches!(owner, SelectionOwner::Profile(profile) if profile.kind == plx_data::ott::ProviderKind::M3u) {
                    return Err(BindingError::SourceUnavailable);
                }
                super::capture_bound(bindings, owner, entry)?
            }
        };
        // A legacy stable ID may match several source rows. Never remove it
        // based on a single row just because the document index finds a match.
        let existing = super::existing_identity_bound(bindings, document, owner, entry)?;
        if let (Some(proof), Some(identity)) = (channel, existing.as_ref()) {
            if document.entries().iter().find(|favorite| favorite.identity() == *identity)
                .is_none_or(|favorite| !proof.allows(favorite)) {
                return Err(BindingError::AmbiguousSource);
            }
        }
        let index = super::FavoriteIndex::new_proven(bindings, document, owner, self)?;
        let mut next_document = document.clone(); let mut next_proofs = self.clone();
        let added = if let Some(identity) = index.identity(entry)? {
            if existing.as_deref() != Some(identity) { return Err(BindingError::SourceMismatch); }
            next_document.remove(identity); next_proofs.prune_with_pins(&next_document, pins); false
        } else {
            let favorite = captured;
            next_document.remove(&favorite.identity());
            next_document.toggle(favorite).map_err(|_| BindingError::InvalidFavorite)?;
            next_proofs.record_capture_with_pins(bindings, &next_document, pins, owner, catalog, entry)?;
            true
        };
        *self = next_proofs; *document = next_document; Ok(added)
    }

    /// Stable paths can rebind normally. A one-segment v1 value cannot prove that
    /// an older sender meant a root file instead of dropping its parent folder.
    pub(in crate::app::ott_runtime) fn authorize(
        &self, bindings: &SelectionBindings<'_>, document: &Document,
        favorite: &Favorite, profile: &ProviderProfile,
    ) -> Result<Option<LocalFavoritePermit>, BindingError> {
        self.authorize_member(bindings, document.entries(), favorite, profile)
    }

    pub(in crate::app::ott_runtime) fn authorize_pin(
        &self, bindings: &SelectionBindings<'_>, pins: &[Favorite],
        favorite: &Favorite, profile: &ProviderProfile,
    ) -> Result<Option<LocalFavoritePermit>, BindingError> {
        if !pins.iter().any(|row| row == favorite)
            || !profile_matches_owner(super::bind_bound(bindings, favorite)?, profile) {
            return Err(BindingError::SourceMismatch);
        }
        self.authorize_member(bindings, pins, favorite, profile)
    }

    fn authorize_member(
        &self, bindings: &SelectionBindings<'_>, members: &[Favorite],
        favorite: &Favorite, profile: &ProviderProfile,
    ) -> Result<Option<LocalFavoritePermit>, BindingError> {
        let owner = super::bind_bound(bindings, favorite)?;
        if !single_segment(favorite) { return Ok(None); }
        self.validate()?;
        let profile_matches = profile_matches_owner(owner, profile);
        if !profile_matches || !members.iter().any(|row| row == favorite) {
            return Err(BindingError::SourceMismatch);
        }
        let item_id = self.item_for(owner, favorite)?.ok_or(BindingError::LocalPathUnproven)?;
        Ok(Some(LocalFavoritePermit { favorite: favorite.clone(), profile: profile.clone(), item_id: item_id.into() }))
    }

    // No-pin shorthand for fixtures; production callers retain current kiosk pins.
    #[cfg(test)]
    pub(in crate::app::ott_runtime) fn prune(&mut self, document: &Document) {
        self.prune_with_pins(document, &[]);
    }

    /// Retain one shared proof if either current owner still holds the exact
    /// Favorite. Neither collection is copied into the other or serialized here.
    pub(in crate::app::ott_runtime) fn prune_with_pins(&mut self, document: &Document, pins: &[Favorite]) {
        self.entries.retain(|proof| document.entries().iter().chain(pins.iter())
            .any(|row| row == &proof.favorite));
    }

    /// Call in the existing Saved transaction on explicit replacement, removal,
    /// or an unproven remount. No guessed volume identity or live grant is saved.
    pub(in crate::app::ott_runtime) fn invalidate_root(&mut self, id: &str) {
        self.entries.retain(|proof| !matches!(&proof.source, Source::Root { id: old, .. } if old == id));
    }

    /// Rebind only an existing exact-profile proof after an attested name edit.
    /// Root grants, locator aliases, item IDs and absent proofs are unchanged.
    pub(in crate::app::ott_runtime) fn rename_profile(
        &mut self, before: &ProviderProfile, after: &ProviderProfile,
        entries: &[(Favorite, Favorite)],
    ) -> Result<(), BindingError> {
        self.validate()?;
        let old = fingerprint(before)?;
        let new = fingerprint(after)?;
        let renamed: std::collections::BTreeMap<_, _> = entries.iter()
            .map(|(old, new)| (old.identity(), (old, new))).collect();
        for proof in &mut self.entries {
            if let Source::Profile { id, fingerprint } = &mut proof.source {
                if id == &before.id && *fingerprint == old {
                    if let Some((old, favorite)) = renamed.get(&proof.favorite.identity()) {
                        if *old == &proof.favorite {
                            proof.favorite = (*favorite).clone();
                            *fingerprint = new.clone();
                        }
                    }
                }
            }
        }
        self.validate()
    }

    pub(in crate::app::ott_runtime) fn reconcile_profiles(&mut self, profiles: &[ProviderProfile]) {
        self.entries.retain(|proof| match &proof.source {
            Source::Profile { id, fingerprint: old } => profiles.iter().any(|profile|
                profile.enabled && profile.id == *id && fingerprint(profile).is_ok_and(|new| new == *old)),
            Source::Root { .. } => true,
        });
    }
}

#[cfg(test)]
#[path = "ott_local_favorite_proofs_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "ott_local_favorite_usb_tests.rs"]
mod usb_tests;
