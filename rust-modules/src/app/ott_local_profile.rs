//! Scalar Local profile edits borrow the existing binding worker and Saved writer.
//! Validation never publishes its staged grant or activates the candidate profile.
use super::*;
use super::super::ott_local_session::{self as local, LocalOp, OwnerToken};
use super::super::ott_local_sources::{FolderDescriptor, LocalSourceKind, RememberedFolders};

#[derive(Clone)]
pub(super) struct Receipt {
    editor: EditorSession,
    profile: ProviderProfile,
    original: Option<ProviderProfile>,
    remembered: RememberedFolders,
    prepared_folders: Option<RememberedFolders>,
    admission_ticket: plx_media::route::PlayRequestTicket,
    runtime: String,
    owner: OwnerToken,
    navigation: Option<u64>,
    playback: u64,
}
#[derive(Default)]
pub(super) struct State {
    cancelled: Option<EditorSession>,
}
pub(super) struct SavedConnection {
    candidate: Option<transient::LocalCandidate>,
    pub(super) ready: bool,
}

pub(super) fn pending(runtime: &Runtime, owner: InstanceId, session: u64) -> bool {
    runtime.transient.local_candidate().is_some_and(|candidate|
        matches!(&candidate.purpose, transient::LocalPurpose::Profile(receipt)
            if receipt.editor.owner == owner && receipt.editor.id == session))
}
fn current(runtime: &Runtime, app: &App, receipt: &Receipt) -> bool {
    runtime.loaded && !runtime.persistence_uncertain && runtime.source_work_allowed()
        && playback_allowed(app) && app.player.pending_exit.is_none()
        && plx_media::route::play_request_is_current(receipt.admission_ticket) && runtime.reply == Some(receipt.editor.owner)
        && current_ui_owner(app) == Some(receipt.editor.owner)
        && runtime.edit_session.as_ref() == Some(&receipt.editor)
        && receipt.owner == OwnerToken(runtime.generation)
        && receipt.navigation == Some(runtime.navigation_epoch)
        && receipt.playback == runtime.playback_epoch
        && !runtime.cancel.load(Ordering::Acquire)
        && matches!(super::super::ott_kiosk_stored::read(&runtime.saved.settings), Ok(None))
        && runtime.saved.profiles.profiles.iter().find(|p| p.id == receipt.profile.id) == receipt.original.as_ref()
        && RememberedFolders::read(&runtime.saved.settings).is_ok_and(|saved| saved == receipt.remembered)
}
pub(super) fn context<'a>(runtime: &'a Runtime, app: &App, receipt: &'a Receipt) -> local::AdmissionContext<'a> {
    local::AdmissionContext { runtime: &receipt.runtime, owner: OwnerToken(runtime.generation),
        invocation_current: current(runtime, app, receipt), now_ms: runtime.now_ms(),
        mutations_allowed: runtime.loaded && !runtime.persistence_uncertain && runtime.source_work_allowed() }
}

pub(super) fn begin(runtime: &mut Runtime, app: &mut App, editor: EditorSession,
    profile: ProviderProfile, original: Option<ProviderProfile>) -> Result<(), &'static str> {
    if runtime.saving || runtime.transient.candidate.is_some() || runtime.usb_remote_waiting_result()
        || runtime.usb_browsing() || !runtime.loaded || runtime.persistence_uncertain
        || !runtime.source_work_allowed() || !playback_allowed(app) || app.player.pending_exit.is_some()
        || runtime.reply != Some(editor.owner) || current_ui_owner(app) != Some(editor.owner)
        || runtime.edit_session.as_ref() != Some(&editor)
        || !matches!(super::super::ott_kiosk_stored::read(&runtime.saved.settings), Ok(None))
    { return Err("Local folder connection is not available now"); }
    if profile.kind != ProviderKind::Local || plx_data::ott::http_url(&profile.source).is_some() {
        return Err("Expected a Local filesystem source");
    }
    if runtime.local_inventory.proof_cleanup != transient::ProofCleanup::Clean {
        if runtime.local_inventory.proof_cleanup == transient::ProofCleanup::Failed {
            runtime.local_inventory.proof_cleanup = transient::ProofCleanup::Pending;
        }
        return Err("Local source proof cleanup is pending");
    }
    runtime.require_parental(parental::Target::Settings)?;
    runtime.profile_candidate(profile.clone(), original.as_ref())?;
    let remembered = RememberedFolders::read(&runtime.saved.settings)?;
    // At most 32 remembered and 32 live IDs: 65 candidates guarantee a free name.
    let new_id = (0..=64).map(|n| format!("local-ui-{n}"))
        .find(|id| !remembered.folders().iter().any(|folder| &folder.id == id)
            && !runtime.local_inventory.sources.iter().any(|source| &source.descriptor().id == id))
        .ok_or("Too many connected Local sources")?;
    // A retry supersedes its undelivered failure. Emitting it here would arrive
    // after the new submission and incorrectly unlock the new pending editor.
    runtime.local_profile.cancelled = None;
    let interrupted = transient::InterruptedScan { profile: runtime.effective_profile().cloned(),
        owner: runtime.transient.accepted.as_ref().map(transient::TransientMedia::owner),
        restart: runtime.catalog_loading && !runtime.catalog_complete };
    runtime.cancel.store(true, Ordering::Release);
    let classic_generation = runtime.generation;
    runtime.generation = runtime.generation.wrapping_add(1).max(1);
    classic_source::rebase(runtime, classic_generation);
    runtime.continuity.rebase(runtime.generation);
    runtime.cancel = Arc::new(AtomicBool::new(false));
    runtime.catalog_next = None;
    runtime.catalog_loading = false;
    if matches!(runtime.local_inventory.phase, transient::InventoryPhase::Loading(_)) {
        runtime.local_inventory.phase = transient::InventoryPhase::Unloaded;
    }
    let owner = OwnerToken(runtime.generation);
    let request = local::LocalRequest { op: LocalOp::Connect, runtime: runtime.remote.runtime().into(),
        selector: Some(local::Selector::Absolute(profile.source.clone())), include_paths: false,
        session: false, progressive: false };
    let binding = ott_catalog::LocalBindingRequest { known: runtime.local_inventory.sources.clone(),
        remembered: remembered.folders().to_vec(), lookup: Some(profile.source.clone()),
        allow_new: true, allow_reconnect: true, restore_remembered: true,
        new_id, generation: runtime.generation };
    let admission_ticket = plx_media::route::play_request_ticket();
    let receipt = Receipt { editor, profile, original, remembered, prepared_folders: None,
        admission_ticket, runtime: request.runtime.clone(),
        owner, navigation: None, playback: runtime.playback_epoch };
    runtime.transient.put_local_candidate(transient::LocalCandidate {
        inventory: Vec::new(), admission_ticket, owner,
        pending: local::PendingLocal::new(request.clone(), owner, runtime.now_ms().saturating_add(35_000)),
        request, purpose: transient::LocalPurpose::Profile(receipt), interrupted,
        phase: transient::CandidatePhase::Binding(Some(binding)),
    });
    runtime.status = "Checking Local source…".into();
    runtime.changed();
    runtime.launch_catalog();
    Ok(())
}
pub(super) fn seal(runtime: &mut Runtime) {
    let navigation = runtime.navigation_epoch;
    let owner = runtime.reply;
    let Some(candidate) = runtime.transient.local_candidate_mut() else { return; };
    if let transient::LocalPurpose::Profile(receipt) = &mut candidate.purpose {
        if receipt.navigation.is_none() && owner == Some(receipt.editor.owner) {
            receipt.navigation = Some(navigation);
        }
    }
}

pub(super) fn remember_failure(runtime: &mut Runtime, receipt: &Receipt) {
    if runtime.edit_session.as_ref() == Some(&receipt.editor)
        && runtime.pending_editor_save.as_ref() != Some(&receipt.editor) {
        if runtime.local_profile.cancelled.is_none() {
            runtime.local_profile.cancelled = Some(receipt.editor.clone());
        }
    }
}
pub(super) fn drain(runtime: &mut Runtime, app: &mut App) {
    if let Some(editor) = runtime.local_profile.cancelled.take() {
        if runtime.edit_session.as_ref() == Some(&editor) {
            runtime.editor_result(app, &editor, Some("Local source could not be connected; check the path and retry".into()));
        }
    }
}
pub(super) fn restore(runtime: &mut Runtime, candidate: &transient::LocalCandidate) {
    let transient::LocalPurpose::Profile(receipt) = &candidate.purpose else { return; };
    if receipt.playback == runtime.playback_epoch
        && plx_media::route::play_request_is_current(receipt.admission_ticket)
        && candidate.owner == OwnerToken(runtime.generation)
        && runtime.effective_profile() == candidate.interrupted.profile.as_ref()
        && runtime.transient.accepted.as_ref().map(transient::TransientMedia::owner) == candidate.interrupted.owner {
        transient::restore_interrupted_scan(runtime, &candidate.interrupted);
    }
}
// Descriptor equality is separate from possession/liveness of the held grant.
// Match the complete source snapshot, as the USB stability check does.
fn same_descriptor(a: &local::BoundSource, b: &local::BoundSource) -> bool {
    a.id == b.id && a.name == b.name && a.kind == b.kind
        && a.available == b.available && a.authority_ref == b.authority_ref
        && a.generation == b.generation && a.canonical_path == b.canonical_path
}
pub(super) fn prepare_bound(runtime: &mut Runtime, app: &App,
    candidate: &mut transient::LocalCandidate, source: &local::BoundSource) -> Result<Saved, &'static str> {
    runtime.require_parental(parental::Target::Settings)?;
    let transient::LocalPurpose::Profile(receipt) = &mut candidate.purpose else { return Err("Expected a Local profile connection"); };
        if !current(runtime, app, receipt) { return Err("Local profile edit is no longer current"); }
        let selected = candidate.inventory.iter().find(|selected| same_descriptor(selected.descriptor(), source) && selected.is_live())
            .ok_or("Local source is no longer available")?;
        let path = source.canonical_path.clone().ok_or("Local source is unavailable")?;
        if source.kind == local::SourceKind::Folder
            && (selected.requires_usb_verification() || super::super::ott_usb::is_usb_path(&path))
            && selected.usb_witness().is_none() { return Err("Select this source through the USB browser"); }
        receipt.profile.source = path.clone();
        let mut saved = runtime.profile_candidate(receipt.profile.clone(), receipt.original.as_ref())?;
        let folder = FolderDescriptor { id: source.id.clone(), name: source.name.clone(), path,
            kind: match source.kind { local::SourceKind::Folder => LocalSourceKind::Folder, local::SourceKind::File => LocalSourceKind::File },
            usb: selected.usb_witness().and_then(|witness| witness.stable_binding()).cloned() };
        let folders = receipt.remembered.clone().with_connected(folder)?;
        folders.put_into(&mut saved.settings)?;
        saved.validate()?;
        receipt.prepared_folders = Some(folders);
        Ok(saved)
}
pub(super) fn save_bound(runtime: &mut Runtime, app: &mut App,
    mut candidate: transient::LocalCandidate, source: local::BoundSource) {
    let prepared = prepare_bound(runtime, app, &mut candidate, &source);
    let transient::LocalPurpose::Profile(receipt) = &candidate.purpose else { return; };
    let editor = receipt.editor.clone();
    match prepared {
        Ok(saved) => {
            candidate.phase = transient::CandidatePhase::SavingConnection(source);
            runtime.transient.put_local_candidate(candidate);
            match runtime.save(saved, None, true) {
                Ok(()) => {
                    runtime.save_ui_owner = Some(editor.owner);
                    runtime.pending_editor_save = Some(editor);
                },
                Err(error) => {
                    let candidate = runtime.transient.take_local_candidate().unwrap();
                    restore(runtime, &candidate);
                    runtime.editor_result(app, &editor, Some(error.into()));
                }
            }
        }
        Err(error) => {
            restore(runtime, &candidate);
            runtime.editor_result(app, &editor, Some(error.into()));
        }
    }
}

/// Called before the normal Saved branch can invalidate the outgoing source.
/// Only the writer owned by this exact editor may consume the staged connection.
pub(super) fn take_saved(runtime: &mut Runtime, app: &App, saved: Option<&Saved>,
    invocation: Option<&Invocation>) -> Option<SavedConnection> {
    if invocation.is_some() { return None; }
    // Only this editor's admitted writer may recheck authorization or consume
    // staging. A denied/expired grant still consumes it for terminal cleanup.
    let authorized = {
        let candidate = runtime.transient.local_candidate()?;
        let transient::LocalPurpose::Profile(receipt) = &candidate.purpose else { return None; };
        if !matches!(candidate.phase, transient::CandidatePhase::SavingConnection(_))
            || runtime.pending_editor_save.as_ref() != Some(&receipt.editor) { return None; }
        runtime.require_parental(parental::Target::Settings).is_ok()
    };
    let candidate = runtime.transient.local_candidate()?;
    let transient::LocalPurpose::Profile(receipt) = &candidate.purpose else { return None; };
    let transient::CandidatePhase::SavingConnection(source) = &candidate.phase else { return None; };
    if runtime.pending_editor_save.as_ref() != Some(&receipt.editor) { return None; }
    let ready = authorized && current(runtime, app, receipt) && saved.is_some_and(|saved|
        saved.profiles.profiles.iter().any(|profile| profile == &receipt.profile)
        && saved.profiles.active_profile.as_ref() == Some(&receipt.profile.id)
        && RememberedFolders::read(&saved.settings).is_ok_and(|folders|
            receipt.prepared_folders.as_ref() == Some(&folders)))
        && candidate.inventory.iter().any(|held| same_descriptor(held.descriptor(), source) && held.is_live());
    Some(SavedConnection { candidate: runtime.transient.take_local_candidate(), ready })
}
/// Settings are durable and applied; publish the real held inventory before the
/// ordinary Local loader asks for its grant. Never reconstruct a grant from text.
pub(super) fn publish(runtime: &mut Runtime, staged: &mut Option<SavedConnection>) -> Result<(), &'static str> {
    let Some(staged) = staged else { return Ok(()); };
    let candidate = staged.candidate.as_ref().ok_or("Local connection was already consumed")?;
    let transient::CandidatePhase::SavingConnection(source) = &candidate.phase else { return Err("Local connection is no longer current"); };
    if !staged.ready || !candidate.inventory.iter().any(|held| same_descriptor(held.descriptor(), source) && held.is_live()) {
        return Err("Local connection is no longer current");
    }
    let candidate = staged.candidate.take().unwrap();
    runtime.local_inventory.install(candidate.inventory, runtime.generation);
    Ok(())
}
pub(super) fn finish(runtime: &mut Runtime, staged: Option<SavedConnection>) {
    if let Some(candidate) = staged.and_then(|staged| staged.candidate) { restore(runtime, &candidate); }
}

/// An uncertain owned write must release staging without claiming NotApplied.
/// The ordinary writer handler delivers the reconciliation-required result.
pub(super) fn uncertain(runtime: &mut Runtime, invocation: Option<&Invocation>) {
    if invocation.is_some() { return; }
    let owned = runtime.transient.local_candidate().is_some_and(|candidate|
        matches!((&candidate.purpose, &candidate.phase),
            (transient::LocalPurpose::Profile(receipt), transient::CandidatePhase::SavingConnection(_))
            if runtime.pending_editor_save.as_ref() == Some(&receipt.editor)));
    if owned { drop(runtime.transient.take_local_candidate()); }
}
