//! Typed selections for the current durable pin. No provider-controlled string
//! grants this authority. Markers and grants are RAM-only and never serialized.
use super::*;

#[derive(Clone, PartialEq, Eq)]
enum Target {
    Item(Favorite),
    AdmittedFolderMember {
        id: String,
        source_id: String,
        fingerprint: String,
    },
}
#[derive(Clone, PartialEq)]
struct Binding {
    profile: String,
    local: Option<transient::LocalSelection>,
    local_read: Option<LocalReadProof>,
    owner: Option<OwnerToken>,
    attachment: Option<crate::app::ott_m3u_attachment::Receipt>,
}
#[derive(Clone, PartialEq)]
pub(in crate::app::ott_runtime) struct PinnedSelection {
    target: Target,
    binding: Option<Binding>,
}
impl PinnedSelection {
    pub(super) fn request(&self) -> PlaybackRequest {
        let id = match &self.target {
            Target::Item(entry) => format!("kiosk:{}", entry.identity()),
            Target::AdmittedFolderMember { id, .. } => id.clone(),
        };
        let mut request = PlaybackRequest::ordinary(&id, 0);
        request.advance_on_failure = false;
        request.kiosk_selection = Some(self.clone());
        request
    }
}
fn current(runtime: &Runtime, selection: &PinnedSelection) -> Result<(), SourceError> {
    ready(runtime)?;
    if selection.binding.as_ref().and_then(|binding| binding.local_read.as_ref())
        .is_some_and(|read| !read.current(runtime))
    {
        return Err(SourceError::Changed);
    }
    let pin = stored::read(&runtime.saved.settings)
        .map_err(|_| SourceError::Invalid)?
        .ok_or(SourceError::Changed)?;
    match (&selection.target, pin.target()) {
        (Target::Item(expected), TargetRef::Item(entry)) if expected == entry => Ok(()),
        (
            Target::AdmittedFolderMember {
                source_id,
                fingerprint,
                ..
            },
            TargetRef::LocalFolder {
                source_id: id,
                source_fingerprint: hash,
            },
        ) if source_id == id && fingerprint == hash => Ok(()),
        _ => Err(SourceError::Changed),
    }
}
pub(super) fn selection(pin: &Stored) -> Result<PinnedSelection, SourceError> {
    let TargetRef::Item(entry) = pin.target() else {
        return Err(SourceError::Invalid);
    };
    Ok(PinnedSelection {
        target: Target::Item(entry.clone()),
        binding: None,
    })
}
fn bound(
    runtime: &Runtime,
    selection: &PinnedSelection,
) -> Result<
    (
        ProviderProfile,
        Option<transient::LocalSelection>,
        Option<CatalogEntry>,
        Option<Favorite>,
    ),
    SourceError,
> {
    current(runtime, selection)?;
    let (profile, local, entry, favorite) = match &selection.target {
        Target::Item(favorite) => {
            let bindings = runtime
                .favorite_bindings(favorite)
                .map_err(|_| SourceError::Unavailable)?;
            let (profile, local) = match selection::bind_bound(&bindings, favorite)
                .map_err(|_| SourceError::Unavailable)?
            {
                selection::SelectionOwner::Profile(profile) => (profile.clone(), None),
                selection::SelectionOwner::M3uAttachment(receipt) =>
                    (receipt.effective_profile().map_err(|_| SourceError::Unavailable)?, None),
                selection::SelectionOwner::LocalRoot(root) => (
                    transient::source_profile(root).map_err(|_| SourceError::Unavailable)?,
                    Some(
                        runtime
                            .bind_local_selection(root)
                            .map_err(|_| SourceError::Unavailable)?,
                    ),
                ),
            };
            let pin = stored::read(&runtime.saved.settings).map_err(|_| SourceError::Invalid)?
                .ok_or(SourceError::Changed)?;
            attachment_for_pin(runtime, &pin, &profile, favorite)?;
            permit(runtime, selection, &profile)?;
            (profile, local, None, Some(favorite.clone()))
        }
        Target::AdmittedFolderMember {
            id,
            source_id,
            fingerprint,
        } => {
            let (hash, local) = folder_fingerprint(runtime, source_id)?;
            if &hash != fingerprint {
                return Err(SourceError::Changed);
            }
            let profile = runtime
                .effective_profile()
                .cloned()
                .ok_or(SourceError::Unavailable)?;
            let root = runtime
                .current_local_root(&local)
                .ok_or(SourceError::Changed)?;
            if !matches!(runtime.row_owner(),Some(selection::SelectionOwner::LocalRoot(current)) if current.id == root.id)
            {
                return Err(SourceError::Changed);
            }
            let entry = runtime
                .catalog
                .entries
                .iter()
                .find(|entry| &entry.id == id)
                .cloned()
                .ok_or(SourceError::Unavailable)?;
            (profile, Some(local), Some(entry), None)
        }
    };
    let attachment = favorite.as_ref().map(|entry| runtime.attachment_for(entry)).transpose()
        .map_err(|_| SourceError::Unavailable)?.flatten();
    let now = Binding {
        attachment,
        profile: profile_fingerprint(&profile),
        local_read: local_read_proof(runtime, local.as_ref())?,
        local: local.clone(),
        owner: runtime
            .transient
            .accepted
            .as_ref()
            .map(transient::TransientMedia::owner),
    };
    if selection
        .binding
        .as_ref()
        .is_some_and(|expected| expected != &now)
    {
        return Err(SourceError::Changed);
    }
    Ok((profile, local, entry, favorite))
}
pub(super) fn bind(
    runtime: &Runtime,
    selection: &PinnedSelection,
) -> Result<(ProviderProfile, Option<transient::LocalSelection>), SourceError> {
    bound(runtime, selection).map(|(profile, local, _, _)| (profile, local))
}
pub(in crate::app::ott_runtime) fn permit(
    runtime: &Runtime,
    selection: &PinnedSelection,
    profile: &ProviderProfile,
) -> Result<Option<LocalFavoritePermit>, SourceError> {
    current(runtime, selection)?;
    let Target::Item(entry) = &selection.target else {
        return Err(SourceError::Invalid);
    };
    let bindings = runtime
        .favorite_bindings(entry)
        .map_err(|_| SourceError::Unavailable)?;
    runtime
        .saved
        .local_favorite_proofs
        .authorize_pin(&bindings, std::slice::from_ref(entry), entry, profile)
        .map_err(|_| SourceError::Unavailable)
}
pub(in crate::app::ott_runtime) fn bind_request(
    runtime: &Runtime,
    request: &PlaybackRequest,
) -> Result<
    Option<(
        PlaybackRequest,
        ProviderProfile,
        Option<CatalogEntry>,
        Option<Favorite>,
    )>,
    SourceError,
> {
    let Some(selection) = request.kiosk_selection.as_ref() else {
        return Ok(None);
    };
    let (profile, local, entry, favorite) = bound(runtime, selection)?;
    if request.continuation.is_some() && request.local_selection != local {
        return Err(SourceError::Changed);
    }
    let mut selection = selection.clone();
    selection.binding = Some(Binding {
        attachment: favorite.as_ref().map(|entry| runtime.attachment_for(entry)).transpose()
            .map_err(|_| SourceError::Unavailable)?.flatten(),
        profile: profile_fingerprint(&profile),
        local_read: local_read_proof(runtime, local.as_ref())?,
        local: local.clone(),
        owner: runtime
            .transient
            .accepted
            .as_ref()
            .map(transient::TransientMedia::owner),
    });
    let mut rebound = request.clone();
    rebound.local_selection = local;
    rebound.transient_owner = selection.binding.as_ref().and_then(|binding| binding.owner);
    rebound.kiosk_selection = Some(selection);
    Ok(Some((rebound, profile, entry, favorite)))
}
pub(in crate::app::ott_runtime) fn attachment_current(
    runtime: &Runtime, request: &PlaybackRequest,
    receipt: &crate::app::ott_m3u_attachment::Receipt,
) -> bool {
    let Some(selection) = request.kiosk_selection.as_ref() else { return false; };
    request.id == selection.request().id
        && selection.binding.as_ref().and_then(|binding| binding.attachment.as_ref()) == Some(receipt)
        && bind_request(runtime, request).is_ok_and(|bound|
            bound.is_some_and(|(current, _, _, _)| current == *request))
}
pub(super) fn observed_favorite(
    runtime: &Runtime,
    selection: &PinnedSelection,
    entry: &CatalogEntry,
) -> Result<Favorite, SourceError> {
    let (profile, local) = bind(runtime, selection)?;
    match &selection.target {
        Target::Item(favorite) => Ok(favorite.clone()),
        Target::AdmittedFolderMember { .. } => {
            let local = local.ok_or(SourceError::Changed)?;
            let root = runtime
                .current_local_root(&local)
                .ok_or(SourceError::Changed)?;
            let bindings = profile_bindings(runtime, &profile)?;
            selection::capture_bound(&bindings, selection::SelectionOwner::LocalRoot(root), entry)
                .map_err(|_| SourceError::Changed)
        }
    }
}
fn freeze_binding(
    runtime: &Runtime,
    selection: PinnedSelection,
) -> Result<PinnedSelection, SourceError> {
    let request = selection.request();
    let (request, _, _, _) = bind_request(runtime, &request)?.ok_or(SourceError::Invalid)?;
    request.kiosk_selection.ok_or(SourceError::Invalid)
}
/// Startup owner calls this once after authorized Loaded publication. No current
/// catalog or global Favorites membership is required for a stable Item pin.
pub(in crate::app::ott_runtime) fn prepare(runtime: &Runtime) -> Result<Prepared, SourceError> {
    ready(runtime)?;
    let pin = stored::read(&runtime.saved.settings)
        .map_err(|_| SourceError::Invalid)?
        .ok_or(SourceError::NotFound)?;
    let selection = freeze_binding(runtime, selection(&pin)?)?;
    let (profile, local) = bind(runtime, &selection)?;
    Ok(Prepared {
        stored: pin,
        activation: Activation::Pinned(selection.clone()),
        proof: Proof::Pinned(selection, profile, local),
        capture: None,
    })
}
/// May only be called after the source module verifies a current AdmittedAttempt.
/// A folder member is retained verbatim; a new member is never selected on wake.
pub(super) fn resume_request(
    runtime: &Runtime,
    attempt: &AdmittedAttempt,
) -> Result<PlaybackRequest, SourceError> {
    let pin = attempt.prepared.stored();
    let selection = match pin.target() {
        TargetRef::Item(_) => selection(pin)?,
        TargetRef::LocalFolder {
            source_id,
            source_fingerprint,
        } => {
            if let Activation::Pinned(selection) = &attempt.prepared.activation {
                return Ok(freeze_binding(runtime, selection.clone())?.request());
            }
            let Activation::LocalFolder { selection_id, .. } = &attempt.prepared.activation else {
                return Err(SourceError::Invalid);
            };
            PinnedSelection {
                target: Target::AdmittedFolderMember {
                    id: selection_id.clone(),
                    source_id: source_id.into(),
                    fingerprint: source_fingerprint.into(),
                },
                binding: None,
            }
        }
        _ => return Err(SourceError::Invalid),
    };
    Ok(freeze_binding(runtime, selection)?.request())
}
pub(super) fn prepared_for_request(
    runtime: &Runtime,
    request: &PlaybackRequest,
) -> Result<Prepared, SourceError> {
    let (request, profile, _, _) = bind_request(runtime, request)?.ok_or(SourceError::Invalid)?;
    let selection = request.kiosk_selection.ok_or(SourceError::Invalid)?;
    let pin = stored::read(&runtime.saved.settings)
        .map_err(|_| SourceError::Invalid)?
        .ok_or(SourceError::Changed)?;
    Ok(Prepared {
        stored: pin,
        activation: Activation::Pinned(selection.clone()),
        proof: Proof::Pinned(selection, profile, request.local_selection),
        capture: None,
    })
}
pub(super) fn startup_request(runtime: &Runtime) -> Result<Option<PlaybackRequest>, SourceError> {
    ready(runtime)?;
    let Some(pin) = stored::read(&runtime.saved.settings).map_err(|_| SourceError::Invalid)? else {
        return Ok(None);
    };
    if !matches!(pin.target(), TargetRef::Item(_)) {
        return Ok(None);
    }
    match prepare(runtime) {
        Ok(prepared) => match prepared.activation {
            Activation::Pinned(selection) => Ok(Some(selection.request())),
            _ => Err(SourceError::Invalid),
        },
        Err(SourceError::Unavailable | SourceError::NotReady) => {
            Ok(Some(selection(&pin)?.request()))
        }
        Err(error) => Err(error),
    }
}

#[cfg(test)]
#[path = "ott_kiosk_pinned_tests.rs"]
mod tests;
