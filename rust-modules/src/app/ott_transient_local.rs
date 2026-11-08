//! Local command transitions execute through the existing catalog and resolver slots.
use super::*;
use crate::app::ott_local_sources::{FolderDescriptor, RememberedFolders};
use local::{Admission, AdmissionContext, LocalError, LocalOp, SourceSelection, SourceSnapshot};

#[path = "ott_transient_lifecycle.rs"]
mod source_lifecycle;
pub(in crate::app::ott_runtime) use source_lifecycle::*;
#[path = "ott_transient_local_kiosk.rs"]
mod kiosk_preparation;
pub(in crate::app) use kiosk_preparation::LocalKioskPreparation;
pub(in crate::app::ott_runtime) use kiosk_preparation::{LocalPurpose,
    kiosk_restore_local_folder, kiosk_preparation_is_current, retry_kiosk_preparation,
    kiosk_preparation_resume_policy, staged_native_binding};
use kiosk_preparation::{candidate_context, candidate_target_matches, settle_without_play,
    settle_native_failure, settle_native_accepted, settle_cancelled, refresh_native_receipt};

fn context<'a>(runtime: &'a Runtime, app: &App, invocation: &Invocation) -> AdmissionContext<'a> {
    AdmissionContext {
        runtime: runtime.remote.runtime(),
        owner: OwnerToken(runtime.generation),
        invocation_current: runtime.remote.invocation_is_current(invocation),
        now_ms: runtime.now_ms(),
        mutations_allowed: playback_allowed(app)
            && runtime.loaded
            && !runtime.persistence_uncertain,
    }
}
fn envelope(data: Value) -> Value {
    if data["state"] == "error" {
        json!({"status":"rejected", "data":data})
    } else {
        ok(data)
    }
}
fn error(request: &local::LocalRequest, error: LocalError) -> Value {
    envelope(request.error_data(error))
}
pub(super) fn is_play(request: &local::LocalRequest) -> bool {
    matches!(request.op, LocalOp::Play | LocalOp::Shuffle)
}

pub(in crate::app::ott_runtime) fn command(
    runtime: &mut Runtime,
    app: &mut App,
    invocation: &Invocation,
) -> Option<Value> {
    command_inner(runtime, app, invocation, None)
}
pub(in crate::app::ott_runtime) fn command_verified_handle(runtime: &mut Runtime, app: &mut App, invocation: &Invocation, proof: LocalSource) -> Option<Value> {
    command_inner(runtime, app, invocation, Some(proof))
}
fn command_inner(runtime: &mut Runtime, app: &mut App, invocation: &Invocation, proof: Option<LocalSource>) -> Option<Value> {
    let mut request = match local::LocalRequest::parse(&invocation.params) {
        Ok(request) => request,
        Err(_) if invocation.params.get("source_handle").is_some() => return Some(local::handle_failure(&invocation.params,runtime.remote.runtime(),LocalError::InvalidRequest)),
        Err(_) => return Some(rejected("Invalid Local request")),
    };
    if request.runtime != runtime.remote.runtime()
        || !runtime.remote.invocation_is_current(invocation)
    {
        return Some(error(&request, LocalError::RuntimeChanged));
    }
    if !runtime.loaded
        || runtime.persistence_uncertain
        || (request.op != LocalOp::Sources && !playback_allowed(app))
    {
        return Some(error(&request, LocalError::Restricted));
    }
    if let Some(local::Selector::Handle { token, .. }) = &request.selector {
        if let Err(reason)=runtime.usb_remote_handle(token) {return Some(error(&request,reason));}
    }
    let handle_based = matches!(&request.selector, Some(local::Selector::Handle { .. }));
    if !handle_based && request.op != LocalOp::Sources && runtime.local_inventory.proof_cleanup != ProofCleanup::Clean {
        // An explicit retry may retry the existing durable cleanup, but cannot
        // grant the source until its verified completion has cleared the proof.
        if runtime.local_inventory.proof_cleanup == ProofCleanup::Failed {
            runtime.local_inventory.proof_cleanup = ProofCleanup::Pending;
        }
        return Some(error(&request, LocalError::OperationInProgress));
    }
    if runtime.transient.candidate.is_some() || (request.op == LocalOp::Connect && runtime.saving) {
        return Some(error(&request, LocalError::OperationInProgress));
    }
    if let Some(local::Selector::Handle { token, source_id }) = &mut request.selector {
        let source = match runtime.usb_remote_handle(token) { Ok(source)=>source, Err(reason)=>return Some(error(&request, reason)) };
        let Some(proof) = proof else { return runtime.usb_remote_consume(invocation, source); };
        if !proof.is_live() || !source.same_grant(&proof) || invocation.deadline_ms<=runtime.now_ms() {return Some(error(&request, LocalError::SourceUnavailable));}
        if runtime.local_inventory.proof_cleanup != ProofCleanup::Clean {
            if runtime.local_inventory.proof_cleanup == ProofCleanup::Failed {runtime.local_inventory.proof_cleanup=ProofCleanup::Pending;}
            return Some(error(&request,LocalError::OperationInProgress));
        }
        *source_id=Some(source.descriptor().id.clone());
    }
    let remembered = match RememberedFolders::read(&runtime.saved.settings) {
        Ok(value) => value,
        Err(_) => return Some(error(&request, LocalError::NativeError)),
    };
    let binding_remembered = match &request.selector {
        Some(local::Selector::Handle { token, .. }) => match runtime.usb_remote_binding_folders(token, &remembered) {
            Ok(folders) => folders,
            Err(error_code) => return Some(error(&request, error_code)),
        },
        _ => remembered.folders().to_vec(),
    };
    if request.op == LocalOp::Sources
        && request.selector.is_none()
        && remembered.folders().is_empty()
        && runtime.local_inventory.sources.is_empty()
    {
        let mut pending = local::PendingLocal::new(
            request.clone(),
            OwnerToken(runtime.generation),
            invocation.deadline_ms,
        );
        return match pending.complete_sources(
            &context(runtime, app, invocation),
            &SourceSnapshot {
                lookup_path: None,
                sources: &[],
            },
        ) {
            Admission::Complete { data } => Some(envelope(data)),
            _ => Some(error(&request, LocalError::NativeError)),
        };
    }
    // Source observation shares the catalogue worker and retires its generation.
    // Do not invalidate an already admitted remote or UI playback request merely
    // to enumerate sources. The caller may retry after native admission settles.
    if request.op == LocalOp::Sources
        && (runtime.play_target.is_some() || runtime.pending_play.is_some())
    {
        return Some(error(&request, LocalError::OperationInProgress));
    }
    let interrupted = InterruptedScan {
        profile: runtime.effective_profile().cloned(),
        owner: runtime
            .transient
            .accepted
            .as_ref()
            .map(TransientMedia::owner),
        restart: runtime.catalog_loading && !runtime.catalog_complete,
    };
    if is_play(&request) {
        runtime.resume_pending = None;
        runtime.playback_epoch = runtime.playback_epoch.wrapping_add(1);
        commands::cancel_pending_play(runtime);
        runtime.cancel_play_request();
    }
    if matches!(runtime.local_inventory.phase, InventoryPhase::Loading(_)) {
        runtime.local_inventory.phase = InventoryPhase::Unloaded;
    }
    runtime.cancel.store(true, Ordering::Release);
    let classic_generation = runtime.generation;
    runtime.generation = runtime.generation.wrapping_add(1).max(1);
    classic_source::rebase(runtime, classic_generation);
    runtime.continuity.rebase(runtime.generation);
    runtime.cancel = Arc::new(AtomicBool::new(false));
    runtime.catalog_next = None;
    runtime.catalog_loading = false;
    let lookup = match &request.selector {
        Some(local::Selector::Absolute(path)) => Some(path.clone()),
        _ => None,
    };
    let task = ott_catalog::LocalBindingRequest {
        known: runtime.local_inventory.sources.clone(),
        remembered: binding_remembered,
        lookup,
        allow_new: !handle_based && request.op != LocalOp::Sources,
        restore_remembered: !handle_based && request.op != LocalOp::Sources,
        allow_reconnect: !handle_based && request.op == LocalOp::Connect,
        new_id: format!("local-{}", invocation.id),
        generation: runtime.generation,
    };
    let reports_opening = request.op != LocalOp::Sources;
    runtime.transient.put_local_candidate(LocalCandidate {
        inventory: Vec::new(),
        admission_ticket: plx_media::route::play_request_ticket(),
        owner: OwnerToken(runtime.generation),
        pending: local::PendingLocal::new(
            request.clone(),
            OwnerToken(runtime.generation),
            invocation.deadline_ms,
        ),
        request,
        purpose: LocalPurpose::Remote(invocation.clone()),
        interrupted,
        phase: CandidatePhase::Binding(Some(task)),
    });
    if reports_opening {
        runtime.status = "Opening local source…".into();
    }
    runtime.changed();
    runtime.launch_catalog();
    None
}

pub(in crate::app::ott_runtime) fn take_binding_task(
    runtime: &mut Runtime,
) -> Option<ott_catalog::Task> {
    // Back retires the old validation but does not request another one. A
    // subsequent explicit load supplies a fresh token; the shared done flag
    // still prevents overlap with the retiring producer.
    if !runtime.loaded || runtime.cancel.load(Ordering::Acquire) {
        return None;
    }
    if let Some(candidate) = runtime.transient.local_candidate_mut() {
        return if let CandidatePhase::Binding(task) = &mut candidate.phase {
            task.take().map(ott_catalog::Task::LocalBindings)
        } else {
            None
        };
    }
    if !matches!(runtime.local_inventory.phase, InventoryPhase::Unloaded) {
        return None;
    }
    let remembered = match RememberedFolders::read(&runtime.saved.settings) {
        Ok(folders) => folders,
        Err(error) => {
            runtime.local_inventory.phase = InventoryPhase::Failed;
            runtime.status = error.into();
            return None;
        }
    };
    if remembered.folders().is_empty() && runtime.local_inventory.sources.is_empty() {
        runtime.local_inventory.phase = InventoryPhase::Ready(runtime.generation);
        runtime.catalog_dirty |= runtime.continuity.has_deferred_start(runtime.generation);
        return None;
    }
    runtime.local_inventory.phase = InventoryPhase::Loading(runtime.generation);
    Some(ott_catalog::Task::LocalBindings(
        ott_catalog::LocalBindingRequest {
            known: runtime.local_inventory.sources.clone(),
            remembered: remembered.folders().to_vec(),
            lookup: None,
            allow_new: false,
            restore_remembered: true,
            allow_reconnect: false,
            new_id: String::new(),
            generation: runtime.generation.max(1),
        },
    ))
}

pub(in crate::app::ott_runtime) fn source_profile(
    source: &BoundSource,
) -> Result<ProviderProfile, &'static str> {
    if !source.available {
        return Err("Local source is unavailable");
    }
    let path = source
        .canonical_path
        .clone()
        .ok_or("Local source has no device path")?;
    Ok(ProviderProfile {
        id: format!("transient:{}", source.id),
        name: "Local videos".into(),
        kind: ProviderKind::Local,
        source: path,
        source_alias: source.name.clone(),
        enabled: true,
    })
}

fn restore_scan(runtime: &mut Runtime, candidate: &LocalCandidate) {
    if matches!(candidate.purpose, LocalPurpose::Profile(_)) {
        crate::app::ott_runtime::local_profile::restore(runtime, candidate);
        return;
    }
    if candidate.request.op == LocalOp::Sources {
        // A read may finish after a newer play or provider choice. Restore only
        // its still-owned interrupted producer, without replacing the current
        // generation/token or clearing a newer catalogue plan.
        let interrupted = &candidate.interrupted;
        let current = candidate.owner == OwnerToken(runtime.generation)
            && runtime.effective_profile() == interrupted.profile.as_ref()
            && runtime.transient.accepted.as_ref().map(TransientMedia::owner) == interrupted.owner
            && !runtime.cancel.load(Ordering::Acquire);
        if !current { return; }
        if interrupted.restart && !runtime.catalog_complete
            && !runtime.catalog_loading && runtime.catalog_next.is_none()
        {
            runtime.catalog_next = interrupted.profile.clone();
            runtime.catalog_loading = runtime.catalog_next.is_some();
        }
        runtime.changed();
        return;
    }
    restore_interrupted_scan(runtime, &candidate.interrupted);
}
pub(in crate::app::ott_runtime) fn restore_interrupted_scan(runtime: &mut Runtime, interrupted: &InterruptedScan) {
    // A late command/Saved completion may restore the planned accepted scan,
    // but cannot reopen work or mint a generation while lifecycle admission is
    // closed. The lifecycle runner re-arms after the real worker has retired.
    if runtime.catalog_work_allowed() {
        runtime.cancel.store(true, Ordering::Release);
        let classic_generation = runtime.generation;
        runtime.generation = runtime.generation.wrapping_add(1).max(1);
        classic_source::rebase(runtime, classic_generation);
        runtime.continuity.rebase(runtime.generation);
        runtime.cancel = Arc::new(AtomicBool::new(false));
    }
    runtime.catalog_next = None;
    if runtime.effective_profile() == interrupted.profile.as_ref()
        && runtime
            .transient
            .accepted
            .as_ref()
            .map(TransientMedia::owner)
            == interrupted.owner
        && interrupted.restart
    {
        runtime.catalog_next = interrupted.profile.clone();
        runtime.catalog_loading = true;
        runtime.catalog_complete = false;
    }
    runtime.changed();
}
fn finish_without_play(runtime: &mut Runtime, app: &App, candidate: LocalCandidate, data: Value) {
    settle_without_play(runtime, app, &candidate);
    if matches!(runtime.local_inventory.phase, InventoryPhase::Ready(_)) {
        runtime.validate_bound_resume();
    }
    match candidate.purpose {
        LocalPurpose::Remote(invocation) => runtime.remote.complete(invocation, envelope(data)),
        LocalPurpose::Profile(receipt) => crate::app::ott_runtime::local_profile::remember_failure(runtime, &receipt),
        LocalPurpose::KioskFolder(_) => {}
    }
}
fn fail_candidate(
    runtime: &mut Runtime,
    app: &mut App,
    mut candidate: LocalCandidate,
    failure: LocalError,
) {
    runtime.retire_closed_source(app);
    let response = candidate
        .pending
        .fail(&candidate_context(runtime, app, &candidate.purpose), failure);
    if let Admission::Complete { data } = response {
        finish_without_play(runtime, app, candidate, data);
    } else {
        settle_cancelled(runtime, &candidate);
    }
}

pub(in crate::app::ott_runtime) fn binding_result(
    runtime: &mut Runtime,
    app: &mut App,
    generation: u64,
    result: Result<ott_catalog::LocalBindings, LocalError>,
) {
    let Some(mut candidate) = runtime.transient.take_local_candidate() else {
        if matches!(runtime.local_inventory.phase,InventoryPhase::Loading(current) if current==generation)
            && generation == runtime.generation
        {
            match result {
                Ok(value) => {
                    observe_local_loss(runtime, app, &value.sources);
                    if value.operation_error.is_none() {
                        runtime.local_inventory.install(value.sources, generation.max(1));
                        runtime.catalog_dirty |= runtime.continuity.has_deferred_start(runtime.generation);
                        rebind_lifecycle_resume(runtime);
                    } else {
                        runtime.local_inventory.phase = InventoryPhase::Failed;
                        runtime.status = "Local sources could not be restored".into();
                    }
                }
                Err(_) => {
                    runtime.local_inventory.phase = InventoryPhase::Failed;
                    runtime.status = "Local sources could not be restored".into();
                }
            }
            runtime.validate_bound_resume();
            runtime.changed();
        }
        return;
    };
    if candidate.owner != OwnerToken(generation) || generation != runtime.generation {
        runtime.transient.put_local_candidate(candidate);
        return;
    }
    // Sources may include remembered-only, unacquired intent in its response.
    // It has not observed loss for those rows and must not install them or revoke
    // their private proof. Only pre-command inventory members were observed.
    let query_observations = (candidate.request.op == LocalOp::Sources).then(|| {
        result.as_ref().map(|value| value.sources.iter().filter(|source|
            runtime.local_inventory.sources.iter().any(|known|
                known.descriptor().id == source.descriptor().id))
            .cloned().collect::<Vec<_>>()).unwrap_or_default()
    });
    // A current worker's observed loss remains relevant even if the command
    // expired while it was checking. A stale generation never reaches here.
    if let Ok(value) = &result {
        observe_local_loss(runtime, app, query_observations.as_deref().unwrap_or(&value.sources));
    }
    if let Admission::Complete { data } =
        candidate
            .pending
            .check_pending(&candidate_context(runtime, app, &candidate.purpose))
    {
        finish_without_play(runtime, app, candidate, data);
        return;
    }
    let validated = match result {
        Ok(value) => value,
        Err(error) => {
            fail_candidate(runtime, app, candidate, error);
            return;
        }
    };
    if let Some(error) = validated.operation_error {
        fail_candidate(runtime, app, candidate, error);
        return;
    }
    let descriptors: Vec<_> = validated.sources.iter().map(|source| source.descriptor().clone()).collect();
    let snapshot = SourceSnapshot {
        lookup_path: validated.lookup.as_deref(),
        sources: if validated.lookup.is_some() {
            &validated.matched
        } else {
            &descriptors
        },
    };
    if candidate.request.op == LocalOp::Sources {
        let result = candidate
            .pending
            .complete_sources(&candidate_context(runtime, app, &candidate.purpose), &snapshot);
        if let Admission::Complete { data } = result {
            if data["state"] == "ready" {
                runtime
                    .local_inventory
                    .install(query_observations.expect("Sources observations were scoped before publication"), generation);
                runtime.catalog_dirty |= runtime.continuity.has_deferred_start(runtime.generation);
                rebind_lifecycle_resume(runtime);
            }
            finish_without_play(runtime, app, candidate, data);
        } else {
            fail_candidate(runtime, app, candidate, LocalError::NativeError);
        }
        return;
    }
    let source = match local::select_source(&candidate.request, &snapshot) {
        Ok(SourceSelection::Selected(source)) => source,
        // An absolute mutation is already canonicalized and, when necessary, connected in RAM.
        _ => {
            fail_candidate(runtime, app, candidate, LocalError::SourceNotFound);
            return;
        }
    };
    let handle_folders = if let Some(local::Selector::Handle { token, .. }) = &candidate.request.selector {
        let current = runtime.usb_remote_handle(token).ok();
        let verified = validated.sources.iter().find(|s| s.descriptor().id == source.id && s.is_live());
        match (current, verified) {
            (Some(current), Some(verified)) if current.same_grant(verified) => {
                if candidate.request.op == LocalOp::Connect {
                    match runtime.usb_remote_connect_candidate(token, verified) {
                        Ok(folders) => Some(folders),
                        Err(error) => { fail_candidate(runtime, app, candidate, error); return; }
                    }
                } else { None }
            },
            _ => { fail_candidate(runtime, app, candidate, LocalError::SourceUnavailable); return; }
        }
    } else { None };
    candidate.inventory = validated.sources;
    if matches!(candidate.purpose, LocalPurpose::Profile(_)) {
        crate::app::ott_runtime::local_profile::save_bound(runtime, app, candidate, source);
        return;
    }
    if candidate.request.op == LocalOp::Connect {
        let Some(path) = source.canonical_path.clone() else {
            fail_candidate(runtime, app, candidate, LocalError::NativeError);
            return;
        };
        let prepared = if let Some(folders) = handle_folders { Ok(folders) }
        else { RememberedFolders::read(&runtime.saved.settings).and_then(|folders| {
            folders.with_connected(connection_descriptor(&source, path, None))
        }) };
        let mut saved = runtime.saved.clone();
        let prepared = prepared.and_then(|folders| folders.put_into(&mut saved.settings));
        if prepared.is_err() {
            fail_candidate(runtime, app, candidate, LocalError::NativeError);
            return;
        }
        candidate.phase = CandidatePhase::SavingConnection(source);
        let Some(invocation) = candidate.purpose.invocation().cloned() else {
            fail_candidate(runtime, app, candidate, LocalError::InvalidRequest);
            return;
        };
        runtime.transient.put_local_candidate(candidate);
        if runtime.save(saved, Some(invocation), false).is_err() {
            let candidate = runtime.transient.take_local_candidate().unwrap();
            fail_candidate(runtime, app, candidate, LocalError::NativeError);
        }
        return;
    }
    let profile = ProviderProfile {
        id: format!("transient:{}", source.id),
        name: "Local videos".into(),
        kind: ProviderKind::Local,
        source: source.canonical_path.clone().unwrap_or_default(),
        source_alias: source.name.clone(),
        enabled: true,
    };
    candidate.phase = CandidatePhase::Scanning {
        source,
        profile: profile.clone(),
        catalog: Catalog::default(),
        state: local::ScanState::Scanning,
        proposed: None,
    };
    runtime.catalog_next = Some(profile);
    runtime.transient.put_local_candidate(candidate);
    runtime.launch_catalog();
}

fn cancel_native_request(runtime: &mut Runtime, candidate: &LocalCandidate) {
    if candidate_target_matches(runtime, candidate) {
        if candidate.purpose.invocation().is_some() { runtime.pending_play = None; }
        runtime.cancel_play_request();
    }
}

pub(in crate::app::ott_runtime) fn pending_tick(runtime: &mut Runtime, app: &mut App) {
    plex_remote::pending_tick(runtime, app);
    let Some(mut candidate) = runtime.transient.take_local_candidate() else {
        return;
    };
    if matches!(candidate.phase, CandidatePhase::SavingConnection(_)) {
        runtime.transient.put_local_candidate(candidate);
        return;
    }
    let result = match candidate.phase {
        CandidatePhase::Failed(failure) => candidate
            .pending
            .fail(&candidate_context(runtime, app, &candidate.purpose), failure),
        _ => candidate
            .pending
            .check_pending(&candidate_context(runtime, app, &candidate.purpose)),
    };
    match result {
        Admission::Complete { data } => {
            runtime.retire_closed_source(app);
            cancel_native_request(runtime, &candidate);
            finish_without_play(runtime, app, candidate, data);
        }
        Admission::Retired => {
            runtime.retire_closed_source(app);
            cancel_native_request(runtime, &candidate);
            settle_cancelled(runtime, &candidate);
        }
        _ => runtime.transient.put_local_candidate(candidate),
    }
}

pub(in crate::app::ott_runtime) fn launch_failed(runtime: &mut Runtime) {
    if let Some(candidate) = runtime.transient.local_candidate_mut() {
        candidate.phase = CandidatePhase::Failed(LocalError::NativeError);
    } else if matches!(runtime.local_inventory.phase, InventoryPhase::Loading(_)) {
        runtime.local_inventory.phase = InventoryPhase::Failed;
        runtime.validate_bound_resume();
    }
}

pub(in crate::app::ott_runtime) fn scan_result(
    runtime: &mut Runtime,
    app: &mut App,
    incoming: &mut Catalog,
    finished: &mut Option<Result<(), &'static str>>,
) -> bool {
    if incoming.entries.is_empty() && finished.is_none() {
        return false;
    }
    let Some(mut candidate) = runtime.transient.take_local_candidate() else {
        return false;
    };
    let CandidatePhase::Scanning {
        source,
        profile,
        catalog,
        state,
        proposed,
    } = &mut candidate.phase
    else {
        runtime.transient.put_local_candidate(candidate);
        return false;
    };
    if catalog.merge(std::mem::take(incoming)).is_err() {
        *state = local::ScanState::Failed;
    }
    if let Some(result) = finished.take() {
        *state = if result.is_ok() {
            local::ScanState::Complete
        } else {
            local::ScanState::Failed
        };
    }
    let items: Vec<_> = catalog
        .entries
        .iter()
        .map(|entry| local::LocalItem {
            id: entry.id.clone(),
            playable: true,
        })
        .collect();
    let snapshot = local::CatalogSnapshot {
        source_id: &source.id,
        authority_ref: source.authority_ref,
        source_generation: source.generation,
        state: *state,
        items: &items,
    };
    let seed = candidate.purpose.invocation().and_then(|invocation|
        u32::from_str_radix(invocation.id.get(..8).unwrap_or("1"), 16).ok()).unwrap_or(1).max(1);
    let result = candidate.pending.evaluate(
        &candidate_context(runtime, app, &candidate.purpose),
        source,
        &snapshot,
        seed,
    );
    match result {
        Admission::Propose(mut local) => {
            let Some(id) = local.order().first().cloned() else {
                fail_candidate(runtime, app, candidate, LocalError::EmptySource);
                return true;
            };
            let Some(entry) = catalog.entries.iter().find(|entry| entry.id == id).cloned() else {
                fail_candidate(runtime, app, candidate, LocalError::NativeError);
                return true;
            };
            if !plx_media::route::play_request_is_current(candidate.admission_ticket) {
                fail_candidate(runtime, app, candidate, LocalError::RuntimeChanged);
                return true;
            }
            let _ = local.note_intent(candidate.owner, local::LocalIntent::Selected(&id));
            let bindings = selection::SelectionBindings::new(
                &runtime.saved.profiles,
                candidate.inventory.iter().map(LocalSource::descriptor),
                runtime.generation,
            );
            let binding = bindings.and_then(|bindings| {
                bindings.receipt(selection::SelectionOwner::LocalRoot(source))
            });
            let Ok(Some(receipt)) = binding else {
                fail_candidate(runtime, app, candidate, LocalError::SourceUnavailable);
                return true;
            };
            let request = PlaybackRequest {
                kiosk_selection: None,
                archive: None,
                id,
                position_ms: 0,
                advance_on_failure: false,
                continuation: None,
                transient_owner: Some(candidate.owner),
                local_selection: Some(LocalSelection {
                    source_id: source.id.clone(),
                    authority: source.authority_ref,
                    generation: source.generation,
                    receipt,
                }),
            };
            let profile = profile.clone();
            *proposed = Some(local);
            let invocation = candidate.purpose.invocation().cloned();
            candidate.purpose.note_first(request.id.clone());
            // Admission checks the same staged inventory before it becomes
            // accepted. Keep its sole owner visible while the existing queue
            // and resolver inspect the exact Local selection receipt.
            runtime.transient.put_local_candidate(candidate);
            let result = runtime.queue_play_request(request, profile, Some(entry), None, false);
            if result.is_err() {
                runtime.cancel_play_request();
                if let Some(candidate) = runtime.transient.take_local_candidate() {
                    fail_candidate(runtime, app, candidate, LocalError::NativeError);
                }
                return true;
            }
            runtime.pending_play = invocation;
            return true;
        }
        Admission::Complete { data } => {
            runtime.retire_closed_source(app);
            cancel_native_request(runtime, &candidate);
            finish_without_play(runtime, app, candidate, data);
            return true;
        }
        Admission::Retired => {
            runtime.retire_closed_source(app);
            cancel_native_request(runtime, &candidate);
            settle_cancelled(runtime, &candidate);
            return true;
        }
        _ => {}
    }
    runtime.transient.put_local_candidate(candidate);
    true
}

pub(in crate::app::ott_runtime) fn before_native(
    runtime: &mut Runtime,
    app: &App,
) -> Result<(), &'static str> {
    if let Some(result) = plex_remote::before_native(runtime) {
        return result;
    }
    let Some(mut candidate) = runtime.transient.take_local_candidate() else {
        return Ok(());
    };
    let matches = candidate_target_matches(runtime, &candidate);
    let result = if !matches {
        Ok(())
    } else {
        let check = candidate
            .pending
            .check_pending(&candidate_context(runtime, app, &candidate.purpose));
        if !matches!(check, Admission::AwaitingAdmission) {
            Err("Local admission is no longer current")
        } else if !matches!(&candidate.phase,CandidatePhase::Scanning{state,..} if *state!=local::ScanState::Failed)
        {
            Err("Local catalog is unavailable")
        } else if !matches!(&candidate.phase, CandidatePhase::Scanning{source,..}
            if capture_source_read(&candidate.inventory, &runtime.saved.profiles,
                runtime.generation, source).is_some()) {
            Err("Local root was withdrawn")
        } else {
            Ok(())
        }
    };
    runtime.transient.put_local_candidate(candidate);
    result
}

pub(in crate::app::ott_runtime) fn finish_native(
    runtime: &mut Runtime,
    app: &mut App,
    result: Result<(), &'static str>,
    native: Option<&NativeStartReceipt>,
) -> Option<Value> {
    if let Some(value) = plex_remote::finish_native(runtime, app, result, native) {
        return Some(value);
    }
    let candidate = runtime.transient.local_candidate()?;
    if !candidate_target_matches(runtime, candidate) { return None; }
    let mut candidate = runtime.transient.take_local_candidate().unwrap();
    let result = refresh_native_receipt(runtime, &mut candidate, result, native);
    let CandidatePhase::Scanning { source, state, .. } = &candidate.phase else {
        settle_native_failure(runtime, app, &candidate, LocalError::NativeError);
        return Some(error(&candidate.request, LocalError::NativeError));
    };
    let read = capture_source_read(&candidate.inventory, &runtime.saved.profiles,
        runtime.generation, source);
    let result = result.and_then(|()| read.as_ref().map(|_| ()).ok_or("Local root was withdrawn"));
    let response = candidate.pending.finish_admission(
        &candidate_context(runtime, app, &candidate.purpose),
        source,
        *state,
        result.map_err(|_| LocalError::NativeError),
    );
    let Admission::Complete { data } = response else {
        settle_native_failure(runtime, app, &candidate, LocalError::RuntimeChanged);
        return Some(error(&candidate.request, LocalError::RuntimeChanged));
    };
    if candidate.pending.completion() != Some(Ok(())) {
        settle_without_play(runtime, app, &candidate);
        return Some(envelope(data));
    }
    settle_native_accepted(runtime, app, &candidate);
    let (resume_scope, resume_point) = candidate.purpose.admitted_resume(runtime);
    let resume_warning = candidate.purpose.resume_warning();
    let CandidatePhase::Scanning {
        profile,
        catalog,
        state,
        proposed: Some(local),
        ..
    } = candidate.phase
    else {
        return Some(error(&candidate.request, LocalError::NativeError));
    };
    runtime
        .local_inventory
        .install(candidate.inventory, runtime.generation);
    rebind_lifecycle_resume(runtime);
    runtime.catalog = catalog;
    runtime.catalog_complete = state == local::ScanState::Complete;
    runtime.catalog_loading = state == local::ScanState::Scanning;
    runtime.catalog_revision = runtime.catalog_revision.wrapping_add(1);
    runtime.catalog_dirty = true;
    runtime.transient.accepted = Some(TransientMedia::Local(LocalSession {
        read: read.expect("ready admission retained its root"),
        context: local,
        profile,
        borrowed: None,
        kiosk_resume: resume_scope,
    }));
    runtime.parental_source_installed();
    runtime.continuity = continuity::Continuity::new(runtime.generation, resume_point);
    if let Some(playing) = runtime.playing.as_ref() {
        runtime.selected = Some(playing.selection_id.clone());
        runtime
            .continuity
            .initial_admitted(runtime.generation, &playing.selection_id);
    }
    runtime.page = if runtime.saved.favorites_selected { "favorites" } else { "channels" };
    if let Some(warning) = resume_warning {
        if runtime.status.is_empty() || runtime.status == "Opening local source…" {
            runtime.status = warning.into();
        }
    }
    runtime.changed();
    Some(envelope(data))
}

pub(in crate::app::ott_runtime) fn connection_descriptor(source: &BoundSource, path: String, usb: Option<crate::app::ott_local_sources::UsbBinding>) -> FolderDescriptor {
    FolderDescriptor { id: source.id.clone(), name: source.name.clone(), path, usb,
        kind: match source.kind { local::SourceKind::Folder=>crate::app::ott_local_sources::LocalSourceKind::Folder,local::SourceKind::File=>crate::app::ott_local_sources::LocalSourceKind::File } }
}

pub(in crate::app::ott_runtime) fn connection_saved(
    runtime: &mut Runtime,
    app: &App,
    invocation: &Invocation,
    saved: bool,
) -> Option<Value> {
    let candidate = runtime.transient.local_candidate()?;
    if candidate.purpose.invocation().is_none_or(|current| current.id != invocation.id)
        || !matches!(candidate.phase, CandidatePhase::SavingConnection(_))
    {
        return None;
    }
    let mut candidate = runtime.transient.take_local_candidate().unwrap();
    let CandidatePhase::SavingConnection(source) = &candidate.phase else {
        unreachable!()
    };
    let handle_current = match &candidate.request.selector {
        Some(local::Selector::Handle { token, .. }) => runtime.usb_remote_handle(token).is_ok_and(|live|
            candidate.inventory.iter().any(|verified| verified.descriptor().id == source.id && verified.same_grant(&live)
                && runtime.usb_remote_connect_candidate(token, verified).is_ok())),
        _ => true,
    };
    let current = handle_current && capture_source_read(&candidate.inventory, &runtime.saved.profiles,
        runtime.generation, source).is_some();
    let result = if saved && current {
        candidate
            .pending
            .complete_connect(&context(runtime, app, invocation), source)
    } else {
        candidate
            .pending
            .fail(&context(runtime, app, invocation), LocalError::NativeError)
    };
    if let Admission::Complete { data } = result {
        if data["state"] == "ready" {
            runtime
                .local_inventory
                .install(std::mem::take(&mut candidate.inventory), runtime.generation);
            runtime.catalog_dirty |= runtime.continuity.has_deferred_start(runtime.generation);
            rebind_lifecycle_resume(runtime);
        }
        restore_scan(runtime, &candidate);
        Some(envelope(data))
    } else {
        restore_scan(runtime, &candidate);
        Some(error(&candidate.request, LocalError::RuntimeChanged))
    }
}

pub(in crate::app::ott_runtime) fn accepted_catalog_update(runtime: &mut Runtime) {
    let Some(TransientMedia::Local(session)) = runtime.transient.accepted.as_mut() else {
        return;
    };
    let state = if runtime.catalog_complete {
        local::ScanState::Complete
    } else if runtime.catalog_loading {
        local::ScanState::Scanning
    } else {
        local::ScanState::Failed
    };
    let items: Vec<_> = runtime
        .catalog
        .entries
        .iter()
        .map(|entry| local::LocalItem {
            id: entry.id.clone(),
            playable: true,
        })
        .collect();
    let source = session.context.source().clone();
    let snapshot = local::CatalogSnapshot {
        source_id: &source.id,
        authority_ref: source.authority_ref,
        source_generation: source.generation,
        state,
        items: &items,
    };
    if let Ok(update) = session.context.update(session.context.owner(), &snapshot) {
        if !update.continuation_allowed
            || (session.borrowed.is_some() && !runtime.saved.favorites_selected)
        {
            runtime.catalog_dirty = false;
        }
    }
}

pub(in crate::app::ott_runtime) fn set_view(
    runtime: &mut Runtime,
    category: &str,
    favorites: bool,
) -> bool {
    if plex_remote::set_view(runtime, category, favorites) {
        return true;
    }
    let Some(TransientMedia::Local(session)) = runtime.transient.accepted.as_mut() else {
        return false;
    };
    let _ = session
        .context
        .set_view_selection(session.context.owner(), category, favorites);
    runtime.changed();
    true
}

pub(in crate::app::ott_runtime) fn note_stop(runtime: &mut Runtime) {
    plex_remote::note_stop(runtime);
    if let Some(TransientMedia::Local(session)) = runtime.transient.accepted.as_mut() {
        let _ = session
            .context
            .note_intent(session.context.owner(), local::LocalIntent::Stop);
    }
    // The caller owns native retirement; no App-less candidate cancellation here.
}

pub(in crate::app::ott_runtime) fn discard(runtime: &mut Runtime) {
    note_stop(runtime);
    if runtime.transient.accepted.take().is_some() {
        runtime.retire_parental_source();
    }
}

pub(in crate::app::ott_runtime) fn note_pause(runtime: &mut Runtime, paused: bool) {
    if let Some(TransientMedia::Local(session)) = runtime.transient.accepted.as_mut() {
        let intent = if paused {
            local::LocalIntent::Pause
        } else {
            local::LocalIntent::Resume
        };
        let _ = session.context.note_intent(session.context.owner(), intent);
    }
}

pub(in crate::app::ott_runtime) fn target_current(runtime: &Runtime) -> bool {
    if let Some(current) = plex_remote::target_current(runtime) {
        return current;
    }
    let Some(target) = runtime.play_target.as_ref() else {
        return true;
    };
    let Some(binding) = target.local_selection.as_ref() else {
        return true;
    };
    if let Some(candidate) = runtime.transient.local_candidate() {
        if target.transient_owner == Some(candidate.owner) {
            return candidate
                .inventory
                .iter()
                .find(|root| root.descriptor().id == binding.source_id)
                .is_some_and(|root| {
                    root.is_live()
                        && root.descriptor().authority_ref == binding.authority
                        && root.descriptor().generation == binding.generation
                });
        }
    }
    let Some(root) = runtime.current_local_source(binding) else {
        return false;
    };
    if runtime.bind_local_selection(root).as_ref().ok() != Some(binding)
        || runtime.capture_local_source(binding).is_none() {
        return false;
    }
    if let Some(identity) = target.id.strip_prefix("favorite:") {
        let Some(favorite) = runtime
            .saved
            .favorites
            .entries()
            .iter()
            .find(|f| f.identity() == identity)
        else {
            return false;
        };
        return runtime
            .selection_bindings()
            .is_ok_and(|bindings| bindings.receipt_is_current(&binding.receipt, favorite));
    }
    true
}

pub(in crate::app::ott_runtime) fn bookmark_position(
    runtime: &Runtime,
    playing: &Playing,
) -> Option<u64> {
    if let Some(position) = plex_remote::bookmark_position(runtime, playing) {
        return Some(position);
    }
    let owner = playing.transient_owner?;
    let Some(TransientMedia::Local(session)) = runtime
        .transient
        .accepted
        .as_ref()
        .filter(|value| value.owner() == owner)
    else {
        return None;
    };
    if let Some(id) = runtime.folder_row_for(playing) {
        return (session.context.current() == Some(id.as_str()))
            .then(|| session.context.position_ms());
    }
    session
        .borrowed
        .as_ref()
        .filter(|value| {
            value.selection_id == playing.selection_id
                && value.profile_fingerprint == profile_fingerprint(&playing.profile)
        })
        .map(|value| value.position_ms)
}

pub(in crate::app::ott_runtime) fn refresh_effective(runtime: &mut Runtime, app: &mut App) {
    if plex_remote::refresh(runtime, app) {
        return;
    }
    cancel_play_candidate(runtime, app);
    let Some(owner) = runtime.transient.accepted.as_ref() else {
        runtime.load_catalog();
        return;
    };
    let profile = owner.profile().clone();
    runtime.cancel.store(true, Ordering::Release);
    runtime.generation = runtime.generation.wrapping_add(1).max(1);
    runtime.continuity.rebase(runtime.generation);
    runtime.cancel = Arc::new(AtomicBool::new(false));
    runtime.catalog_next = Some(profile);
    runtime.catalog_loading = true;
    runtime.catalog_complete = false;
    runtime.launch_catalog();
    runtime.changed();
}

pub(in crate::app::ott_runtime) fn cancel_play_candidate(runtime: &mut Runtime, app: &mut App) {
    plex_remote::cancel_candidate(runtime, app);
    if !runtime
        .transient
        .local_candidate()
        .is_some_and(|candidate| is_play(&candidate.request))
    {
        return;
    }
    if let Some(candidate) = runtime.transient.take_local_candidate() {
        runtime.retire_closed_source(app);
        cancel_native_request(runtime, &candidate);
        let data = candidate.request.error_data(LocalError::RuntimeChanged);
        finish_without_play(runtime, app, candidate, data);
    }
}

/// Remove proof for observed loss, except exact configured stable-USB intent.
/// Retained proof is dormant: live reads still require the current held source,
/// and relocation requires the worker's receipt. The caller owns the candidate.
pub(in crate::app::ott_runtime) fn revoke_unavailable_proofs(runtime: &Runtime, saved: &mut Saved) {
    let folders = RememberedFolders::read(&saved.settings).ok();
    for source in runtime.local_inventory.sources.iter().map(LocalSource::descriptor).filter(|source| !source.available) {
        let retained = runtime.retained_usb_intent(source);
        saved.local_favorite_proofs.invalidate_unavailable_root(source, retained.as_ref(), folders.as_ref());
    }
}
fn cleanup_needed(runtime: &Runtime) -> bool {
    let mut candidate = runtime.saved.clone();
    revoke_unavailable_proofs(runtime, &mut candidate);
    candidate.local_favorite_proofs != runtime.saved.local_favorite_proofs
}

pub(in crate::app::ott_runtime) fn observe_local_loss(runtime: &mut Runtime, app: &mut App, sources: &[LocalSource]) {
    let unavailable: Vec<_> = sources.iter().filter(|source| !source.is_live())
        .map(LocalSource::descriptor).collect();
    if unavailable.is_empty() { return; }
    for lost in &unavailable {
        if let Some(current) = runtime.local_inventory.sources.iter_mut().find(|old| old.descriptor().id == lost.id) {
            current.withdraw();
        } else {
            // Initial restoration can discover an unavailable remembered root.
            // Retain its refusal, never an available grant, before proof cleanup.
            runtime.local_inventory.sources.push(LocalSource::unavailable((*lost).clone()));
        }
    }
    let lost = |binding: &LocalSelection| unavailable.iter().any(|root| root.id == binding.source_id);
    let playing_lost = runtime.playing.as_ref().is_some_and(|playing|
        playing.local_selection.as_ref().is_some_and(&lost));
    if playing_lost {
        if runtime.playing.as_ref().is_some_and(|playing| playing_matches(playing, app)) {
            plx_media::player::stop_bufferfeed(&mut app.player.session, &mut app.adapters.player);
            plx_media::route::cancel_play(&mut app.player.session);
            plx_media::route::forget_external_source(&mut app.player.session);
        }
        runtime.playing = None;
        runtime.release_active_lease();
    }
    if runtime.play_target.as_ref().is_some_and(|target| target.local_selection.as_ref().is_some_and(&lost)) {
        runtime.cancel_play_request();
    }
    if runtime.resume_pending.as_ref().is_some_and(|target| target.local_selection.as_ref().is_some_and(&lost)) {
        runtime.resume_pending = None;
    }
    let accepted_lost = runtime.transient.accepted.as_ref()
        .and_then(TransientMedia::root)
        .is_some_and(|owned_root| unavailable.iter().any(|root| root.id == owned_root.id));
    if accepted_lost {
        discard(runtime);
        runtime.catalog = Catalog::default();
        runtime.catalog_revision = runtime.catalog_revision.wrapping_add(1);
        runtime.catalog_loading = false;
        runtime.catalog_complete = false;
        runtime.catalog_dirty = false;
        runtime.catalog_next = None;
        runtime.selected = None;
        runtime.metadata_cancel.store(true, Ordering::Release);
        runtime.metadata_generation = runtime.metadata_generation.wrapping_add(1);
        runtime.metadata_next = None;
        runtime.metadata_target = None;
        runtime.favorite_preview = None;
    }
    if playing_lost || accepted_lost { runtime.continuity.stop(runtime.generation); }
    if cleanup_needed(runtime) && runtime.local_inventory.proof_cleanup == ProofCleanup::Clean {
        runtime.local_inventory.proof_cleanup = ProofCleanup::Pending;
    }
    runtime.status = "Local source was disconnected; connect it again before playback".into();
    runtime.changed();
}

/// Every successful Saved completion is checked, including a writer which was
/// admitted before the loss. Its old after-image cannot reopen the refused root.
pub(in crate::app::ott_runtime) fn local_save_finished(runtime: &mut Runtime, saved: bool) {
    if saved {
        runtime.local_inventory.proof_cleanup = if cleanup_needed(runtime) {
            ProofCleanup::Pending
        } else { ProofCleanup::Clean };
    } else if runtime.local_inventory.proof_cleanup == ProofCleanup::Writing {
        runtime.local_inventory.proof_cleanup = ProofCleanup::Failed;
        runtime.status = "Local source proof cleanup failed; reconnect to retry".into();
    }
}

/// Share the normal single verified writer. A busy writer defers cleanup; a
/// failure requires an explicit retry and never permits background reactivation.
pub(in crate::app::ott_runtime) fn cleanup_local_proofs(runtime: &mut Runtime) {
    if runtime.local_inventory.proof_cleanup != ProofCleanup::Pending || runtime.saving
        || !runtime.loaded || runtime.persistence_uncertain { return; }
    if !cleanup_needed(runtime) {
        runtime.local_inventory.proof_cleanup = ProofCleanup::Clean;
        return;
    }
    let mut candidate = runtime.saved.clone();
    revoke_unavailable_proofs(runtime, &mut candidate);
    runtime.local_inventory.proof_cleanup = ProofCleanup::Writing;
    if runtime.save(candidate, None, false).is_err() {
        local_save_finished(runtime, false);
        runtime.changed();
    }
}
