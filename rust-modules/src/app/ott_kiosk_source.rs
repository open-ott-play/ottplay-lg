//! One source-binding path for kiosk designation, capture and recovery. This is
//! a Runtime child: it owns no source registry, persistence worker or player.
use super::*;
use crate::app::ott_kiosk_stored::{self as stored, BorrowedTarget, Stored, TargetRef};
use plx_data::ott::favorites::{Locator, Origin};
#[path = "ott_kiosk_pinned.rs"]
mod pinned;
pub(super) use pinned::{
    attachment_current as pinned_attachment_current, bind_request as bind_pinned_request,
    permit as pinned_permit, PinnedSelection,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum SourceError {
    NotReady,
    Busy,
    Unavailable,
    NotFound,
    Ambiguous,
    Changed,
    Invalid,
    Native,
    ResumeCapacity,
}
impl SourceError {
    pub(super) fn message(self) -> &'static str {
        match self {
            Self::NotReady => "Wait for protected settings and a playable source",
            Self::Busy => "A kiosk source transition is still pending",
            Self::Unavailable => "The kiosk source is unavailable",
            Self::NotFound => "No matching playable item is available",
            Self::Ambiguous => "The kiosk source is ambiguous",
            Self::Changed => "The kiosk source changed before admission",
            Self::Invalid => "The kiosk target cannot be represented safely",
            Self::Native => "The native player refused the kiosk target",
            Self::ResumeCapacity => "Playback resume storage is full",
        }
    }
}

/// Fresh immutable snapshot supplied by the existing explicit queue owner.
#[derive(Clone, PartialEq)]
pub(super) struct QueuePin {
    pub owner: OwnerToken,
    pub entries: Vec<Favorite>,
    pub index: usize,
    pub profile_fingerprint: String,
    pub current_identity: plx_media::route::PlaybackIdentity,
}

#[derive(Clone, PartialEq)]
pub(super) enum Activation {
    None,
    Pinned(PinnedSelection),
    Item {
        selection_id: String,
    },
    LocalFolder {
        source_id: String,
        selection_id: String,
    },
    Queue(QueuePin),
}
#[derive(Clone, PartialEq, Eq)]
struct Context {
    profile: String,
    generation: u64,
    transient: Option<OwnerToken>,
}
#[derive(Clone, PartialEq)]
pub(super) struct Item {
    pub(super) id: String,
    favorite: Favorite,
    pub(super) profile: ProviderProfile,
    entry: Option<CatalogEntry>,
    local: Option<transient::LocalSelection>,
    local_read: Option<LocalReadProof>,
    pub(super) attachment: Option<crate::app::ott_m3u_attachment::Receipt>,
}
/// A prepared designation keeps the admitted capability, not just its path.
/// Equality is structural; fresh use separately checks liveness and receipts.
#[derive(Clone)]
struct LocalReadProof(transient::LocalSourceRead);
impl PartialEq for LocalReadProof {
    fn eq(&self, other: &Self) -> bool {
        self.0.selection == other.0.selection && self.0.same_grant(&other.0)
    }
}
impl Eq for LocalReadProof {}
impl LocalReadProof {
    fn current(&self, runtime: &Runtime) -> bool {
        runtime.capture_local_source(&self.0.selection)
            .is_some_and(|read| read.is_live() && read.same_grant(&self.0))
    }
    fn verify_file(&self, profile: &ProviderProfile, favorite: &Favorite) -> Result<(), SourceError> {
        let source = self.0.source.descriptor();
        if source.kind != crate::app::ott_local_session::SourceKind::File { return Ok(()); }
        let Locator::Local { root_alias, relative_path } = &favorite.locator else {
            return Err(SourceError::Changed);
        };
        if profile.kind != ProviderKind::Local
            || source.canonical_path.as_deref() != Some(profile.source.as_str())
            || root_alias != &source.name
            || source.single_file_name() != Some(relative_path.as_str())
            || !self.0.is_live()
        {
            return Err(SourceError::Changed);
        }
        Ok(())
    }
}
fn local_read_proof(runtime: &Runtime, selected: Option<&transient::LocalSelection>)
    -> Result<Option<LocalReadProof>, SourceError>
{
    selected.map(|selected| runtime.capture_local_source(selected)
        .map(LocalReadProof).ok_or(SourceError::Changed)).transpose()
}
#[derive(Clone, PartialEq)]
enum Proof {
    Context(Context),
    Pinned(
        PinnedSelection,
        ProviderProfile,
        Option<transient::LocalSelection>,
    ),
    Item(Context, Item),
    Observed(Context, Item, u64),
    Folder(Context, String, String, transient::LocalSelection),
    Queue(QueuePin),
}
#[derive(Clone, PartialEq)]
pub(super) struct Prepared {
    stored: Stored,
    activation: Activation,
    proof: Proof,
    capture: Option<Item>,
}
impl Prepared {
    pub(super) fn stored(&self) -> &Stored {
        &self.stored
    }
    pub(super) fn encoded(&self) -> Result<Value, SourceError> {
        self.stored.encode().map_err(|_| SourceError::Invalid)
    }
}

fn ready(runtime: &Runtime) -> Result<(), SourceError> {
    if !runtime.loaded || runtime.persistence_uncertain {
        return Err(SourceError::NotReady);
    }
    Ok(())
}
fn context(runtime: &Runtime) -> Result<Context, SourceError> {
    ready(runtime)?;
    Ok(Context {
        profile: profile_fingerprint(
            runtime
                .effective_profile()
                .ok_or(SourceError::Unavailable)?,
        ),
        generation: runtime.generation,
        transient: runtime
            .transient
            .accepted
            .as_ref()
            .map(transient::TransientMedia::owner),
    })
}
fn profile_bindings<'a>(
    runtime: &'a Runtime,
    profile: &ProviderProfile,
) -> Result<selection::SelectionBindings<'a>, SourceError> {
    if profile.kind == ProviderKind::Local {
        runtime
            .selection_bindings()
            .map_err(|_| SourceError::Unavailable)
    } else {
        Ok(selection::SelectionBindings::profiles_only(
            &runtime.saved.profiles,
        ))
    }
}
fn origin(profile: &ProviderProfile) -> Origin {
    let plex = profile.kind == ProviderKind::Plex;
    Origin {
        provider: profile.kind.canonical_id().into(),
        profile: if plex {
            "Plex".into()
        } else {
            profile.name.clone()
        },
        source_alias: if plex {
            profile.source.clone()
        } else if profile.kind == ProviderKind::Local {
            if profile.source_alias.is_empty() {
                profile.name.clone()
            } else {
                profile.source_alias.clone()
            }
        } else {
            profile.name.clone()
        },
        subprofile: None,
    }
}
fn item(runtime: &Runtime, id: &str) -> Result<Item, SourceError> {
    let (request, profile, entry, existing) = runtime
        .prepare_play_request(PlaybackRequest::ordinary(id, 0))
        .map_err(|_| SourceError::Unavailable)?;
    let favorite = if let Some(favorite) = existing {
        favorite
    } else {
        let entry = entry.as_ref().ok_or(SourceError::NotFound)?;
        let bindings = profile_bindings(runtime, &profile)?;
        let owner = if let Some(local) = &request.local_selection {
            selection::SelectionOwner::LocalRoot(
                runtime
                    .current_local_source(local)
                    .ok_or(SourceError::Changed)?,
            )
        } else {
            selection::SelectionOwner::Profile(&profile)
        };
        selection::capture_bound(&bindings, owner, entry).map_err(|_| SourceError::Invalid)?
    };
    let local_read = local_read_proof(runtime, request.local_selection.as_ref())?;
    if let Some(read) = &local_read { read.verify_file(&profile, &favorite)?; }
    let attachment = runtime.attachment_for(&favorite).map_err(|_| SourceError::Unavailable)?;
    Ok(Item {
        id: id.into(),
        favorite,
        profile,
        entry,
        local: request.local_selection,
        local_read,
        attachment,
    })
}
fn same_item(a: &Item, b: &Item) -> bool {
    a.id == b.id && a.favorite == b.favorite && a.profile == b.profile && a.local == b.local
        && a.local_read == b.local_read && a.attachment == b.attachment
}
fn stored_item(item: &Item, strict: bool) -> Result<Stored, SourceError> {
    match &item.attachment {
        Some(receipt) => Stored::attached_item(item.favorite.clone(), strict,
            receipt.kiosk_binding().map_err(|_| SourceError::Invalid)?),
        None => Stored::item(item.favorite.clone(), strict),
    }.map_err(|_| SourceError::Invalid)
}
/// The durable identity selects one attached source; the fresh receipt grants
/// this attempt authority. Neither a portable Favorite nor a kiosk ID grants it.
fn attachment_for_pin(runtime: &Runtime, pin: &Stored, profile: &ProviderProfile,
    favorite: &Favorite) -> Result<Option<crate::app::ott_m3u_attachment::Receipt>, SourceError>
{
    let receipt = runtime.attachment_for(favorite).map_err(|_| SourceError::Unavailable)?;
    match (&receipt, pin.attachment_binding()) {
        (Some(receipt), Some(expected)) if runtime.m3u_attachments.is_current(receipt)
            && receipt.effective_profile().ok().as_ref() == Some(profile)
            && receipt.kiosk_binding().ok().as_ref() == Some(expected) => Ok(Some(receipt.clone())),
        (None, None) => Ok(None),
        _ => Err(SourceError::Changed),
    }
}
fn folder_fingerprint(
    runtime: &Runtime,
    id: &str,
) -> Result<(String, transient::LocalSelection), SourceError> {
    let root = runtime
        .local_inventory
        .sources
        .iter()
        .find(|source| source.descriptor().id == id)
        .ok_or(SourceError::Unavailable)?;
    // The descriptor alone is not authority after source withdrawal.
    root.root_handle().ok_or(SourceError::Unavailable)?;
    let root = root.descriptor();
    let folders = crate::app::ott_local_sources::RememberedFolders::read(&runtime.saved.settings)
        .map_err(|_| SourceError::Unavailable)?;
    if !root.available
        || !folders.folders().iter().any(|saved| {
            saved.id == root.id
                && saved.name == root.name
                && Some(&saved.path) == root.canonical_path.as_ref()
        })
    {
        return Err(SourceError::Unavailable);
    }
    let fingerprint =
        selection::local_root_fingerprint(root).map_err(|_| SourceError::Unavailable)?;
    let binding = runtime
        .bind_local_selection(root)
        .map_err(|_| SourceError::Unavailable)?;
    runtime.capture_local_root(&binding).ok_or(SourceError::Unavailable)?;
    Ok((fingerprint, binding))
}

/// Existing explicit queues are direct designation. A bare ordinary catalogue
/// only Arms; its already-playing item must not become a pin here.
pub(super) fn prepare(
    runtime: &Runtime,
    app: &App,
    query: Option<&str>,
    strict: bool,
) -> Result<Prepared, SourceError> {
    ready(runtime)?;
    if runtime.saving || runtime.transient.owns_playback() || runtime.play_target.is_some() {
        return Err(SourceError::Busy);
    }
    if query.is_none() {
        if let Some(queue) =
            transient::kiosk_queue(runtime, app).map_err(|_| SourceError::Unavailable)?
        {
            if queue.index >= queue.entries.len() {
                return Err(SourceError::Invalid);
            }
            let stored =
                Stored::queue(queue.entries.clone(), strict).map_err(|_| SourceError::Invalid)?;
            return Ok(Prepared {
                stored,
                activation: Activation::Queue(queue.clone()),
                proof: Proof::Queue(queue),
                capture: None,
            });
        }
    }
    let context = context(runtime)?;
    let profile = runtime
        .effective_profile()
        .ok_or(SourceError::Unavailable)?;
    let Some(query) = query else {
        if runtime.ordered_ids().is_empty() {
            return Err(SourceError::NotReady);
        }
        return Ok(Prepared {
            stored: Stored::armed(origin(profile), strict).map_err(|_| SourceError::Invalid)?,
            activation: Activation::None,
            proof: Proof::Context(context),
            capture: None,
        });
    };
    if let Some(name) = query.strip_prefix("folder:") {
        let key = plx_base::unicode::caseless_key(name.trim());
        if key.is_empty() {
            return Err(SourceError::NotFound);
        }
        let mut roots = runtime
            .local_inventory
            .sources
            .iter()
            .map(|source| source.descriptor())
            .filter(|root| plx_base::unicode::caseless_key(&root.name) == key);
        let root = roots.next().ok_or(SourceError::NotFound)?;
        if roots.next().is_some() {
            return Err(SourceError::Ambiguous);
        }
        let (fingerprint, local) = folder_fingerprint(runtime, &root.id)?;
        kiosk_resume::preflight(runtime, &root.id)?;
        // Use the currently admitted root's incremental catalogue. A different
        // root must go through the existing Local selector before designation.
        if !matches!(runtime.row_owner(),Some(selection::SelectionOwner::LocalRoot(current)) if current.id == root.id)
        {
            return Err(SourceError::Unavailable);
        }
        let selection_id = runtime
            .catalog
            .entries
            .first()
            .ok_or(SourceError::NotFound)?
            .id
            .clone();
        if item(runtime, &selection_id)?.local.as_ref() != Some(&local) {
            return Err(SourceError::Changed);
        }
        let stored = Stored::local_folder(
            Origin {
                provider: "local".into(),
                profile: "Local videos".into(),
                subprofile: None,
                source_alias: root.name.clone(),
            },
            strict,
            root.id.clone(),
            fingerprint.clone(),
        )
        .map_err(|_| SourceError::Invalid)?;
        return Ok(Prepared {
            stored,
            activation: Activation::LocalFolder {
                source_id: root.id.clone(),
                selection_id,
            },
            proof: Proof::Folder(context, root.id.clone(), fingerprint, local),
            capture: None,
        });
    }
    let query = query.trim();
    if query.is_empty() {
        return Err(SourceError::NotFound);
    }
    let ids = runtime.ordered_ids();
    let selected = if query.bytes().all(|b| b.is_ascii_digit()) {
        query
            .parse::<usize>()
            .ok()
            .and_then(|number| number.checked_sub(1))
            .and_then(|index| ids.get(index))
    } else {
        // Per-command indexes, never O(rows²) repeated catalog scans. Numeric
        // selection above does not inspect any preceding title at all.
        let titles: std::collections::HashMap<&str, &str> = runtime
            .catalog
            .entries
            .iter()
            .map(|entry| (entry.id.as_str(), entry.title.as_str()))
            .collect();
        let favorites: std::collections::HashMap<String, &str> = runtime
            .saved
            .favorites
            .entries()
            .iter()
            .map(|entry| (entry.identity(), entry.title.as_str()))
            .collect();
        let key = plx_base::unicode::caseless_key(query);
        ids.iter().find(|id| {
            let title = if let Some(identity) = id.strip_prefix("favorite:") {
                favorites.get(identity).copied()
            } else {
                titles.get(id.as_str()).copied()
            };
            title.is_some_and(|title| plx_base::unicode::caseless_key(title).contains(&key))
        })
    }
    .ok_or(SourceError::NotFound)?;
    let candidate = item(runtime, selected)?;
    let stored = stored_item(&candidate, strict)?;
    Ok(Prepared {
        stored,
        activation: Activation::Item {
            selection_id: selected.clone(),
        },
        proof: Proof::Item(context, candidate.clone()),
        capture: Some(candidate),
    })
}

pub(super) fn still_current(runtime: &Runtime, app: &App, prepared: &Prepared) -> bool {
    if ready(runtime).is_err() {
        return false;
    }
    match &prepared.proof {
        Proof::Context(expected) => context(runtime).as_ref() == Ok(expected),
        Proof::Pinned(selection, profile, local) => pinned::bind(runtime, selection)
            .is_ok_and(|(now, binding)| &now == profile && &binding == local),
        Proof::Item(expected, old) => {
            context(runtime).as_ref() == Ok(expected)
                && item(runtime, &old.id).is_ok_and(|new| same_item(old, &new))
        }
        Proof::Observed(expected, old, generation) => {
            context(runtime).as_ref() == Ok(expected)
                && observe(runtime, app).is_ok_and(|observed| observed.is_some_and(|current|
                    current.play_generation == *generation && same_item(old, &current.item)))
        }
        Proof::Folder(expected, id, hash, local) => {
            context(runtime).as_ref() == Ok(expected)
                && folder_fingerprint(runtime, id)
                    .is_ok_and(|(now, binding)| now == *hash && binding == *local)
        }
        Proof::Queue(queue) => transient::kiosk_queue_is_current(runtime, app, queue),
    }
}

/// Authorize the actual designated source before admitting a Saved image.
/// An ordinary bare Arm has no item yet; its first Playing capture is checked.
pub(super) fn authorize_designation(
    runtime: &mut Runtime,
    app: &App,
    prepared: &Prepared,
) -> Result<(), &'static str> {
    if !still_current(runtime, app, prepared) {
        return Err(SourceError::Changed.message());
    }
    let profile = match &prepared.proof {
        Proof::Context(_) if matches!(prepared.stored.target(), TargetRef::Armed) => return Ok(()),
        Proof::Context(_) => return Err(SourceError::Invalid.message()),
        Proof::Item(_, item) | Proof::Observed(_, item, _) => item.profile.clone(),
        Proof::Pinned(_, profile, _) => profile.clone(),
        Proof::Folder(context, ..) => {
            let profile = runtime
                .effective_profile()
                .ok_or(SourceError::Unavailable.message())?;
            if profile_fingerprint(profile) != context.profile {
                return Err(SourceError::Changed.message());
            }
            profile.clone()
        }
        Proof::Queue(queue) => {
            let profile = runtime
                .effective_profile()
                .ok_or(SourceError::Unavailable.message())?;
            if profile_fingerprint(profile) != queue.profile_fingerprint {
                return Err(SourceError::Changed.message());
            }
            profile.clone()
        }
    };
    let favorite = match prepared.stored.target() { TargetRef::Item(entry) => Some(entry), _ => None };
    let policy_profile = runtime.attachment_policy_profile(&profile, favorite)?;
    runtime.require_parental_playback(&policy_profile)
}

/// Include the pin and any real root-file attestation in the SAME Saved image.
/// This never adds/removes a global Favorite.
pub(super) fn save_candidate(runtime: &Runtime, prepared: &Prepared) -> Result<Saved, SourceError> {
    ready(runtime)?;
    let mut saved = runtime.saved.clone();
    saved
        .settings
        .insert(stored::KEY.into(), prepared.encoded()?);
    if let Some(candidate) = &prepared.capture {
        if candidate.attachment != attachment_for_pin(runtime, &prepared.stored, &candidate.profile, &candidate.favorite)? {
            return Err(SourceError::Changed);
        }
        if candidate.local.is_some()
            && !candidate.local_read.as_ref().is_some_and(|read| read.current(runtime))
        {
            return Err(SourceError::Changed);
        }
        if let Some(entry) = &candidate.entry {
            let bindings = runtime
                .favorite_bindings(&candidate.favorite)
                .map_err(|_| SourceError::Unavailable)?;
            let owner = if let Some(local) = &candidate.local {
                selection::SelectionOwner::LocalRoot(
                    runtime
                        .current_local_source(local)
                        .ok_or(SourceError::Changed)?,
                )
            } else if let Some(receipt) = &candidate.attachment {
                selection::SelectionOwner::M3uAttachment(receipt)
            } else {
                selection::SelectionOwner::Profile(&candidate.profile)
            };
            saved
                .local_favorite_proofs
                .record_pin_capture(
                    &bindings,
                    &saved.favorites,
                    prepared.stored.pins(),
                    owner,
                    &runtime.catalog.entries,
                    entry,
                )
                .map_err(|_| SourceError::Invalid)?;
        }
    }
    saved
        .local_favorite_proofs
        .prune_with_pins(&saved.favorites, prepared.stored.pins());
    if let Activation::LocalFolder { source_id, selection_id } = &prepared.activation {
        kiosk_resume::preflight(runtime, source_id)?;
        kiosk_resume::reserve(runtime, &mut saved, selection_id);
    }
    saved.validate().map_err(|_| SourceError::Invalid)?;
    Ok(saved)
}

pub(super) fn activate(
    runtime: &mut Runtime,
    app: &mut App,
    prepared: &Prepared,
) -> Result<Option<AdmittedAttempt>, SourceError> {
    if !playback_allowed(app) || lifecycle::viewer_paused() {
        return Err(SourceError::NotReady);
    }
    // Durable policy is authoritative even if activation became stale. Caller
    // enforces that policy separately; do not overwrite it or claim rollback.
    if stored::read(&runtime.saved.settings)
        .ok()
        .flatten()
        .as_ref()
        != Some(&prepared.stored)
        || !still_current(runtime, app, prepared)
    {
        return Err(SourceError::Changed);
    }
    match &prepared.activation {
        Activation::None => Ok(None),
        Activation::Pinned(selection) => {
            let request = selection.request();
            runtime
                .play_request_with_policy(app, request)
                .map_err(|_| SourceError::Native)?;
            Ok(admitted_attempt(runtime, prepared))
        }
        Activation::Item { selection_id } => {
            runtime
                .play_with_policy(app, selection_id, false)
                .map_err(|_| SourceError::Native)?;
            Ok(admitted_attempt(runtime, prepared))
        }
        Activation::LocalFolder {
            source_id,
            selection_id,
        } => {
            folder_fingerprint(runtime, source_id)?;
            runtime
                .play_with_policy(app, selection_id, false)
                .map_err(|_| SourceError::Native)?;
            kiosk_resume::activated(runtime, source_id);
            Ok(admitted_attempt(runtime, prepared))
        }
        Activation::Queue(queue) => {
            transient::kiosk_repeat_all(runtime, app, queue).map_err(|_| SourceError::Changed)?;
            Ok(None)
        }
    }
}

/// Build the no-query Arm's after-image only from an observed, explicitly
/// selected native source. Caller owns the explicit-intent and Playing fences.
pub(super) fn capture_prepared(
    runtime: &Runtime,
    observed: &OwnedItem,
    strict: bool,
) -> Result<Prepared, SourceError> {
    let stored = stored_item(&observed.item, strict)?;
    Ok(Prepared {
        stored,
        activation: Activation::None,
        proof: if observed.item.attachment.is_some() {
            Proof::Observed(context(runtime)?, observed.item.clone(), observed.play_generation)
        } else {
            Proof::Item(context(runtime)?, observed.item.clone())
        },
        capture: Some(observed.item.clone()),
    })
}

/// Borrowed output of the central binder. Admission consumes this output;
/// it must never recursively invoke prepare_play_request.
pub(super) struct BoundCandidate<'a> {
    pub request: &'a PlaybackRequest,
    pub profile: &'a ProviderProfile,
    pub entry: Option<&'a CatalogEntry>,
    pub favorite: Option<&'a Favorite>,
}
/// A first kiosk-folder request carries its staged inventory receipt. It must
/// not be rebound through the older accepted inventory before native admission.
/// All other uses retain the ordinary accepted-source proof.
fn admission_local_read(
    runtime: &Runtime,
    app: &App,
    candidate: &BoundCandidate<'_>,
) -> Result<Option<LocalReadProof>, SourceError> {
    let preparation = runtime.transient.local_candidate().and_then(|staged| {
        if let transient::LocalPurpose::KioskFolder(value) = &staged.purpose {
            Some(value)
        } else {
            None
        }
    });
    let Some(preparation) = preparation else {
        return local_read_proof(runtime, candidate.request.local_selection.as_ref());
    };
    let preparation = transient::KioskPreparation::from_local(preparation.clone());
    if !transient::kiosk_preparation_is_current(runtime, app, &preparation) {
        return Err(SourceError::Changed);
    }
    let (profile, entry) = transient::staged_native_binding(runtime, candidate.request)
        .ok_or(SourceError::Changed)?
        .map_err(|_| SourceError::Changed)?;
    if &profile != candidate.profile
        || !candidate.entry.is_some_and(|bound| bound.id == entry.id && bound.media == entry.media)
    {
        return Err(SourceError::Changed);
    }
    transient::capture_video_root(runtime, candidate.request, candidate.profile)
        .map(|read| read.map(LocalReadProof))
        .map_err(|_| SourceError::Changed)
}

/// Both explicit switches and continuation/recovery stay inside the pin. Call
/// after common binding at queue admission and again before native admission.
pub(super) fn admit_bound(
    runtime: &Runtime,
    app: &App,
    candidate: BoundCandidate<'_>,
) -> Result<(), SourceError> {
    ready(runtime)?;
    let Some(pin) = stored::read(&runtime.saved.settings).map_err(|_| SourceError::Invalid)? else {
        return Ok(());
    };
    if matches!(pin.target(), TargetRef::Armed) {
        return Ok(());
    }
    let captured;
    let favorite = if let Some(favorite) = candidate.favorite {
        favorite
    } else {
        let entry = candidate.entry.ok_or(SourceError::NotFound)?;
        let bindings = profile_bindings(runtime, candidate.profile)?;
        let owner = if let Some(local) = &candidate.request.local_selection {
            selection::SelectionOwner::LocalRoot(
                runtime
                    .current_local_source(local)
                    .ok_or(SourceError::Changed)?,
            )
        } else {
            selection::SelectionOwner::Profile(candidate.profile)
        };
        captured = selection::capture_bound(&bindings, owner, entry)
            .map_err(|_| SourceError::Unavailable)?;
        &captured
    };
    attachment_for_pin(runtime, &pin, candidate.profile, favorite)?;
    let read = admission_local_read(runtime, app, &candidate)?;
    if let Some(read) = &read {
        read.verify_file(candidate.profile, favorite)?;
        if read.0.source.descriptor().kind == crate::app::ott_local_session::SourceKind::File {
            runtime.saved.local_favorite_proofs.authorize_pin(
                &runtime.favorite_bindings(favorite).map_err(|_| SourceError::Unavailable)?,
                pin.pins(), favorite, candidate.profile,
            ).map_err(|_| SourceError::Unavailable)?;
        }
    }
    let local = if matches!(pin.target(), TargetRef::LocalFolder { .. }) { candidate
        .request
        .local_selection
        .as_ref()
        .map(|binding| {
            folder_fingerprint(runtime, &binding.source_id)
                .map(|(hash, _)| (binding.source_id.clone(), hash))
        })
        .transpose()? } else { None };
    if !matches_pin(
        &pin,
        favorite,
        local
            .as_ref()
            .map(|(id, hash)| (id.as_str(), hash.as_str())),
    ) {
        return Err(SourceError::Changed);
    }
    if matches!(pin.target(), TargetRef::Queue(_))
        && !matches!(stored::borrow_validated(&runtime.saved.settings), Ok(Some(view))
            if matches!(view.target(), BorrowedTarget::Queue(entries)
                if (transient::kiosk_restore_allows(runtime, &candidate.request.id, entries) || transient::kiosk_queue_allows(runtime, app, &candidate.request.id, entries))))
    {
        return Err(SourceError::Changed);
    }
    Ok(())
}

/// Retained by the existing lifecycle recovery owner, never by another Runtime
/// slot. Only a successfully admitted concrete request can construct this.
#[derive(Clone)]
pub(super) struct AdmittedAttempt {
    pub play_generation: u64,
    playback_epoch: u64,
    ticket: plx_media::route::PlayRequestTicket,
    prepared: Prepared,
}
impl AdmittedAttempt {
    pub(super) fn same_admission(&self, other: &Self) -> bool {
        self.play_generation == other.play_generation
            && self.playback_epoch == other.playback_epoch
            && self.ticket == other.ticket
            && self.prepared.stored.same_target(&other.prepared.stored)
            && self.prepared.activation == other.prepared.activation
            && self.prepared.proof == other.prepared.proof
    }
}
pub(super) fn admitted_attempt(runtime: &Runtime, prepared: &Prepared) -> Option<AdmittedAttempt> {
    let pinned_id;
    let id = match &prepared.activation {
        Activation::Pinned(selection) => {
            pinned_id = selection.request().id;
            &pinned_id
        }
        Activation::Item { selection_id } | Activation::LocalFolder { selection_id, .. } => {
            selection_id
        }
        _ => return None,
    };
    let target = runtime.play_target.as_ref()?;
    if target.archive.is_some() { return None; }
    let ticket = runtime.play_ticket?;
    if target.id != *id || !plx_media::route::play_request_is_current(ticket) {
        return None;
    }
    Some(AdmittedAttempt {
        play_generation: runtime.play_generation,
        playback_epoch: runtime.playback_epoch,
        ticket,
        prepared: prepared.clone(),
    })
}
pub(super) fn attempt_current(runtime: &Runtime, app: &App, attempt: &AdmittedAttempt) -> bool {
    ready(runtime).is_ok()
        && playback_allowed(app)
        && !lifecycle::viewer_paused()
        && runtime.play_generation == attempt.play_generation
        && runtime.playback_epoch == attempt.playback_epoch
        && plx_media::route::play_request_is_current(attempt.ticket)
        && runtime.playing.is_none()
        && app.player.pending_exit.is_none()
        && stored::read(&runtime.saved.settings)
            .ok()
            .flatten()
            .is_some_and(|pin| pin.same_target(attempt.prepared.stored()))
        && still_current(runtime, app, &attempt.prepared)
}
impl Runtime {
    /// Initial-load provenance is sampled at Runtime construction, not when a
    /// delayed result lands. A newer native play followed by Exit also revokes it.
    pub(super) fn initial_kiosk_startup_allowed(&self, app: &App) -> bool {
        self.loaded
            && !self.persistence_uncertain
            && self.navigation_epoch == 0
            && self.playback_epoch == 0
            && plx_media::route::play_request_is_current(self.initial_native_ticket)
            && self.playing.is_none()
            && self.play_target.is_none()
            && self.resume_pending.is_none()
            && self.transient.candidate.is_none()
            && app.player.pending_exit.is_none()
            && !plx_media::route::play_pending()
            && !app.adapters.player.is_live()
            && !lifecycle::viewer_paused()
    }
}

/// Only the initial Loaded owner may stage a persisted Item. The request is
/// unbound until the ordinary source-ready path supplies fresh credentials and
/// Local grants. New viewer intent cancels this existing resume_pending slot.
pub(super) fn stage_startup_item(runtime: &mut Runtime, app: &App) -> Result<bool, SourceError> {
    if !runtime.initial_kiosk_startup_allowed(app) {
        return Ok(false);
    }
    let Some(request) = pinned::startup_request(runtime)? else {
        return Ok(false);
    };
    runtime.continuity.stop(runtime.generation);
    runtime.resume_pending = Some(request);
    Ok(true)
}
pub(super) fn admitted_pinned_request(runtime: &Runtime) -> Option<AdmittedAttempt> {
    let request = runtime.play_target.as_ref()?;
    if request.archive.is_some() { return None; }
    let prepared = if request.kiosk_selection.is_some() {
        pinned::prepared_for_request(runtime, request).ok()?
    } else {
        let pin = stored::read(&runtime.saved.settings).ok()??;
        let candidate = item(runtime, &request.id).ok()?;
        attachment_for_pin(runtime, &pin, &candidate.profile, &candidate.favorite).ok()?;
        match pin.target() {
            TargetRef::Item(entry)
                if entry.origin == candidate.favorite.origin
                    && entry.locator == candidate.favorite.locator =>
            {
                Prepared {
                    stored: pin,
                    activation: Activation::Item {
                        selection_id: request.id.clone(),
                    },
                    proof: Proof::Item(context(runtime).ok()?, candidate),
                    capture: None,
                }
            }
            TargetRef::LocalFolder {
                source_id,
                source_fingerprint,
            } => {
                let (hash, local) = folder_fingerprint(runtime, source_id).ok()?;
                if &hash != source_fingerprint || candidate.local.as_ref() != Some(&local) {
                    return None;
                }
                let id = source_id.to_owned();
                Prepared {
                    stored: pin,
                    activation: Activation::LocalFolder {
                        source_id: id.clone(),
                        selection_id: request.id.clone(),
                    },
                    proof: Proof::Folder(context(runtime).ok()?, id, hash, local),
                    capture: None,
                }
            }
            _ => return None,
        }
    };
    admitted_attempt(runtime, &prepared)
}

/// Preserve only a still-owned failed admission at the suspend edge. The
/// current foreground bit may already be false, but no other fence is relaxed.
pub(super) fn admitted_resume_request(
    runtime: &Runtime,
    app: &App,
    attempt: &AdmittedAttempt,
) -> Option<PlaybackRequest> {
    if ready(runtime).is_err()
        || lifecycle::viewer_paused()
        || runtime.resume_pending.is_some()
        || runtime.play_target.is_some()
        || runtime.playing.is_some()
        || app.player.pending_exit.is_some()
        || runtime.play_generation != attempt.play_generation
        || runtime.playback_epoch != attempt.playback_epoch
        || !plx_media::route::play_request_is_current(attempt.ticket)
        || !stored::read(&runtime.saved.settings)
            .ok()
            .flatten()
            .is_some_and(|pin| pin.same_target(attempt.prepared.stored()))
        || !still_current(runtime, app, &attempt.prepared)
    {
        return None;
    }
    pinned::resume_request(runtime, attempt).ok()
}
/// Main-thread evidence taken immediately before the exact synchronous
/// start_resolved call. Never retain across a tick or use for resolver errors.
pub(super) struct NativeAdmissionFence {
    generation: u64,
    playback_epoch: u64,
    ticket: plx_media::route::PlayRequestTicket,
}
pub(super) fn begin_native_admission(
    runtime: &Runtime,
    app: &App,
    attempt: &AdmittedAttempt,
) -> Option<NativeAdmissionFence> {
    if !attempt_current(runtime, app, attempt) {
        return None;
    }
    Some(NativeAdmissionFence {
        generation: attempt.play_generation,
        playback_epoch: attempt.playback_epoch,
        ticket: attempt.ticket,
    })
}
/// Consume immediately after that same start_resolved call. Its own synchronous
/// cancel/replace may advance the native ticket; application intent must not.
pub(super) fn finish_native_admission(
    runtime: &Runtime,
    app: &App,
    attempt: &mut AdmittedAttempt,
    fence: NativeAdmissionFence,
) -> bool {
    let same_target =
        runtime
            .play_target
            .as_ref()
            .is_some_and(|request| match &attempt.prepared.activation {
                Activation::Item { selection_id }
                | Activation::LocalFolder { selection_id, .. } => request.id == *selection_id,
                Activation::Pinned(selection) => {
                    request.kiosk_selection.as_ref() == Some(selection)
                }
                _ => false,
            });
    if fence.generation != attempt.play_generation
        || fence.playback_epoch != attempt.playback_epoch
        || fence.ticket != attempt.ticket
        || runtime.play_generation != fence.generation
        || runtime.playback_epoch != fence.playback_epoch
        || !same_target
        || !playback_allowed(app)
        || lifecycle::viewer_paused()
        || app.player.pending_exit.is_some()
        || !stored::read(&runtime.saved.settings)
            .ok()
            .flatten()
            .is_some_and(|pin| pin.same_target(attempt.prepared.stored()))
        || !still_current(runtime, app, &attempt.prepared)
    {
        return false;
    }
    attempt.ticket = plx_media::route::play_request_ticket();
    true
}
pub(super) fn retry_attempt(
    runtime: &mut Runtime,
    app: &mut App,
    attempt: &AdmittedAttempt,
) -> Result<AdmittedAttempt, SourceError> {
    if !attempt_current(runtime, app, attempt) {
        return Err(SourceError::Changed);
    }
    if runtime.resolver_busy
        || runtime.resolver_next.is_some()
        || runtime.play_target.is_some()
        || runtime.saving
        || runtime.transient.owns_playback()
    {
        return Err(SourceError::Busy);
    }
    let current = stored::read(&runtime.saved.settings)
        .map_err(|_| SourceError::Invalid)?
        .ok_or(SourceError::Changed)?;
    if !current.same_target(attempt.prepared.stored()) {
        return Err(SourceError::Changed);
    }
    let mut prepared = attempt.prepared.clone();
    prepared.stored = current;
    activate(runtime, app, &prepared)?.ok_or(SourceError::Changed)
}

/// Observe only an exact installed source. A borrowed Favorite does not become
/// a retained Local/Plex queue merely because it inherited its owner token.
#[derive(Clone)]
pub(super) struct OwnedItem {
    pub favorite: Favorite,
    pub play_generation: u64,
    pub(super) item: Item,
}
pub(super) fn observe(runtime: &Runtime, app: &App) -> Result<Option<OwnedItem>, SourceError> {
    ready(runtime)?;
    if runtime.play_target.is_some()
        || runtime.transient.owns_playback()
        || app.player.pending_exit.is_some()
    {
        return Ok(None);
    }
    let Some(playing) = runtime
        .playing
        .as_ref()
        .filter(|p| playing_matches(p, app) && !playing_superseded(p, app))
    else {
        return Ok(None);
    };
    if playing.identity.is_none() {
        return Ok(None);
    }
    let favorite = if let Some(selection) = playing.kiosk_selection.as_ref() {
        let (profile, local) = pinned::bind(runtime, selection)?;
        if profile != playing.profile || local != playing.local_selection {
            return Err(SourceError::Changed);
        }
        pinned::observed_favorite(runtime, selection, &playing.entry)?
    } else if runtime.playing_attachment.is_some() {
        // The installed source owns this row even after global Favorites change.
        // Reuse the exact playing-source capture, including its held receipt.
        playing_favorite::capture(runtime, playing).map_err(|_| SourceError::Changed)?
    } else if playing.selection_id.starts_with("favorite:") {
        let rebound = item(runtime, &playing.selection_id)?;
        if rebound.profile != playing.profile || rebound.local != playing.local_selection {
            return Err(SourceError::Changed);
        }
        rebound.favorite
    } else {
        let bindings = profile_bindings(runtime, &playing.profile)?;
        let owner = if let Some(local) = &playing.local_selection {
            selection::SelectionOwner::LocalRoot(
                runtime
                    .current_local_source(local)
                    .ok_or(SourceError::Changed)?,
            )
        } else {
            selection::SelectionOwner::Profile(&playing.profile)
        };
        selection::capture_bound(&bindings, owner, &playing.entry)
            .map_err(|_| SourceError::Unavailable)?
    };
    let attachment = runtime.attachment_for(&favorite).map_err(|_| SourceError::Unavailable)?;
    // Compare the receipt held by this actual playback, including its revision.
    // Rebinding an equal-looking A after A -> B -> A cannot authorize old media.
    if attachment != runtime.playing_attachment {
        return Err(SourceError::Changed);
    }
    if attachment.as_ref().is_some_and(|receipt|
        !runtime.m3u_attachments.is_current(receipt)
            || receipt.effective_profile().ok().as_ref() != Some(&playing.profile)) {
        return Err(SourceError::Changed);
    }
    let local_read = local_read_proof(runtime, playing.local_selection.as_ref())?;
    if let Some(read) = &local_read {
        read.verify_file(&playing.profile, &favorite)?;
        if read.0.source.descriptor().kind == crate::app::ott_local_session::SourceKind::File {
            let (Some(plx_media::ff::ExternalSource::HeldLocal(held)), MediaLocator::Local { relative_path }) =
                (playing.source.as_ref(), &playing.entry.media) else { return Err(SourceError::Changed); };
            if !held.is_live() || read.0.source.single_file(relative_path).as_ref() != Some(held) {
                return Err(SourceError::Changed);
            }
        }
    }
    Ok(Some(OwnedItem {
        favorite: favorite.clone(),
        play_generation: runtime.play_generation,
        item: Item {
            id: playing.selection_id.clone(),
            favorite,
            profile: playing.profile.clone(),
            entry: Some(playing.entry.clone()),
            local: playing.local_selection.clone(),
            local_read,
            attachment,
        },
    }))
}

pub(super) fn matches_pin(
    stored: &Stored,
    favorite: &Favorite,
    local: Option<(&str, &str)>,
) -> bool {
    match stored.target() {
        TargetRef::Armed => false,
        TargetRef::Item(entry) => entry.identity() == favorite.identity(),
        TargetRef::Queue(entries) => entries
            .iter()
            .any(|entry| entry.identity() == favorite.identity()),
        TargetRef::LocalFolder {
            source_id,
            source_fingerprint,
        } => {
            local == Some((source_id, source_fingerprint))
                && matches!(favorite.locator, Locator::Local { .. })
        }
    }
}
/// Select the admitted folder's traversal domain without changing the viewer's
/// global Favorites/category preference. The held source, not an optional resume
/// checkpoint or an outgoing decoder, owns this scope across EOF and discovery.
pub(super) fn strict_folder_traversal(runtime: &Runtime) -> bool {
    let Ok(Some(pin)) = stored::borrow_validated(&runtime.saved.settings) else { return false; };
    if !matches!(pin.state(), crate::app::ott_kiosk_policy::State::Locked { strict: true }) {
        return false;
    }
    let BorrowedTarget::LocalFolder { source_id, source_fingerprint } = pin.target() else {
        return false;
    };
    let Some(transient::TransientMedia::Local(session)) = &runtime.transient.accepted else {
        return false;
    };
    if !runtime.local_source_read_is_current(&session.read) {
        return false;
    }
    let Some(root) = runtime.current_local_root(&session.read.selection) else { return false; };
    let accepted = session.context.source();
    root.id == source_id && root.id == accepted.id
        && root.authority_ref == accepted.authority_ref
        && root.generation == accepted.generation
        && selection::local_root_fingerprint(root).is_ok_and(|value| value == source_fingerprint)
}

pub(super) fn owned_pinned_target(
    runtime: &Runtime,
    app: &App,
) -> Result<Option<OwnedItem>, SourceError> {
    let Some(pin) =
        stored::borrow_validated(&runtime.saved.settings).map_err(|_| SourceError::Invalid)?
    else {
        return Ok(None);
    };
    if matches!(pin.target(), BorrowedTarget::Armed) {
        return Ok(None);
    }
    let Some(observed) = observe(runtime, app)? else {
        return Ok(None);
    };
    if observed.item.attachment.is_some() {
        let stored = stored::read(&runtime.saved.settings).map_err(|_| SourceError::Invalid)?
            .ok_or(SourceError::Changed)?;
        attachment_for_pin(runtime, &stored, &observed.item.profile, &observed.favorite)?;
    }
    let matched = match pin.target() {
        BorrowedTarget::Armed => false,
        BorrowedTarget::Item(entry) => entry.matches(&observed.favorite),
        BorrowedTarget::Queue(entries) => {
            entries
                .iter()
                .any(|entry| entry.matches(&observed.favorite))
                && transient::kiosk_queue_matches_current(runtime, app, entries)
        }
        BorrowedTarget::LocalFolder {
            source_id,
            source_fingerprint,
        } => {
            observed
                .item
                .local
                .as_ref()
                .is_some_and(|binding| binding.source_id == source_id)
                && folder_fingerprint(runtime, source_id)
                    .is_ok_and(|(hash, _)| hash == source_fingerprint)
                && runtime
                    .playing
                    .as_ref()
                    .is_some_and(|playing| runtime.folder_row_for(playing).is_some())
        }
    };
    if !matched {
        return Ok(None);
    }
    if matches!(observed.favorite.locator, Locator::Local { .. })
        && !matches!(pin.target(), BorrowedTarget::LocalFolder { .. })
    {
        // This exact entry was matched against the validated durable pin above.
        // A single borrowed pin suffices for its proof; never decode the queue.
        runtime
            .saved
            .local_favorite_proofs
            .authorize_pin(
                &runtime
                    .selection_bindings()
                    .map_err(|_| SourceError::Unavailable)?,
                std::slice::from_ref(&observed.favorite),
                &observed.favorite,
                &observed.item.profile,
            )
            .map_err(|_| SourceError::Unavailable)?;
    }
    Ok(Some(observed))
}

impl Runtime {
    pub(super) fn current_kiosk_pin_owned(&self, app: &App) -> bool {
        match stored::borrow_validated(&self.saved.settings) {
            Ok(Some(pin)) => match pin.target() {
                BorrowedTarget::Queue(entries) => {
                    ready(self).is_ok()
                        && transient::kiosk_queue_matches_current(self, app, entries)
                }
                _ => owned_pinned_target(self, app).is_ok_and(|item| item.is_some()),
            },
            _ => false,
        }
    }
}

/// The portable identity contains private locator and profile data. Expose a
/// stable-in-this-runtime digest, never the identity itself or an unsalted hash.
/// Existing boot entropy keeps low-entropy paths from being guessed offline.
fn status_channel(runtime: &Runtime, favorite: &Favorite) -> Result<Value, SourceError> {
    let entropy = runtime.entropy.ok_or(SourceError::NotReady)?;
    let input = json!([
        "ott-kiosk-status-id-v1", entropy, runtime.remote.runtime(), favorite.identity()
    ]);
    let id = format!("kiosk:{}", plx_data::ott::favorites_wire::hash(input.to_string().as_bytes()));
    Ok(json!({"id":id,"name":favorite.title}))
}

pub(super) fn status_target(
    runtime: &Runtime,
    app: &App,
    pin: &Stored,
) -> Result<Value, SourceError> {
    match pin.target() {
        TargetRef::Armed => Ok(json!({"channel":null})),
        TargetRef::Item(entry) => Ok(json!({"channel":status_channel(runtime, entry)?})),
        TargetRef::Queue(entries) => {
            // A failed decoder retains an already pinned queue's identity.
            // The source owner validates current ownership for this read only;
            // capturing or installing a new pin still requires healthy playback.
            let current = transient::kiosk_queue_for_status(runtime, app)
                .map_err(|_| SourceError::Unavailable)?
                .filter(|queue| {
                    queue.entries.as_slice() == entries && queue.index < entries.len()
                });
            let current = current.ok_or(SourceError::Unavailable)?;
            Ok(json!({"channel":null,"media":{"index":current.index,
                "title":entries[current.index].title,"total":entries.len()}}))
        }
        TargetRef::LocalFolder {
            source_id,
            source_fingerprint,
        } => {
            let folders =
                crate::app::ott_local_sources::RememberedFolders::read(&runtime.saved.settings)
                    .map_err(|_| SourceError::Unavailable)?;
            let name = folders
                .folders()
                .iter()
                .find(|folder| folder.id == source_id)
                .map(|folder| folder.name.as_str());
            let owned = owned_pinned_target(runtime, app)?.ok_or(SourceError::Unavailable)?;
            let valid = folder_fingerprint(runtime, source_id)
                .is_ok_and(|(hash, _)| hash == source_fingerprint);
            Ok(
                json!({"channel":status_channel(runtime, &owned.favorite)?,
                "folder":{"name":name,"available":valid}}),
            )
        }
    }
}

/// Enforce an already durable target without implicitly starting or waking it.
/// Source activation is a separate, revalidated operation. Off/Armed never stop.
pub(super) fn apply_durable_policy(runtime: &mut Runtime, app: &mut App) {
    let pin = stored::borrow_validated(&runtime.saved.settings);
    if matches!(pin, Ok(None)) {
        transient::plex_remote::set_kiosk_repeat(runtime, false);
        return;
    }
    if matches!(&pin, Ok(Some(pin)) if matches!(pin.target(), BorrowedTarget::Armed)) {
        return;
    }
    if runtime.current_kiosk_pin_owned(app) {
        // A verified Save still owns policy after the initiating RPC expires.
        // Enforce its already-running queue without admitting fresh playback.
        if matches!(&pin, Ok(Some(pin)) if matches!(pin.target(), BorrowedTarget::Queue(_))) {
            transient::plex_remote::set_kiosk_repeat(runtime, true);
        }
        return;
    }
    // Enforcement must not create a checkpoint writer after the policy commit.
    runtime.retire_catalog_playback(app);
    playback::replace_player(
        &mut app.player,
        &mut app.adapters.player,
        &mut app.repause_at,
        &mut app.refresh_hubs,
        &mut app.pages,
    );
}

pub(super) fn arm_matches(pin: &Stored, profile: &ProviderProfile) -> bool {
    let expected = pin.origin();
    let actual = origin(profile);
    matches!(pin.target(), TargetRef::Armed)
        && expected.provider == actual.provider
        && expected.profile == actual.profile
        && expected.source_alias == actual.source_alias
}

#[cfg(test)]
#[path = "ott_kiosk_source_tests.rs"]
mod tests;

#[cfg(all(test, feature = "hostsim"))]
#[path = "ott_kiosk_file_tests.rs"]
mod file_tests;

/// A strict-only update changes input policy, not playback ownership.
pub(super) fn policy_target_changed(
    before: &BTreeMap<String, Value>,
    after: &BTreeMap<String, Value>,
) -> bool {
    match (stored::read(before), stored::read(after)) {
        (Ok(None), Ok(None)) => false,
        (Ok(Some(before)), Ok(Some(after))) => !before.same_target(&after),
        _ => true,
    }
}
