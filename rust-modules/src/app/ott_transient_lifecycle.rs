//! Local-owner hooks for the shared source lifecycle. No second work queue.
//!
//! Included below `local_remote`, so cancellation settles the same PendingLocal
//! receipt as normal completion. The lifecycle runner owns generation changes,
//! cancel tokens and actual worker retirement.
use super::*;

pub(in crate::app::ott_runtime) fn suspend_for_lifecycle(
    runtime: &mut Runtime,
    app: &mut App,
    previous_generation: u64,
) {
    // Plex cancellation uses its own existing candidate/receipt; its restore
    // helper must obey the same paused-work guard as Local's restore_scan.
    plex_remote::cancel_candidate(runtime, app);
    if let Some(mut candidate) = runtime.transient.take_local_candidate() {
        if matches!(candidate.phase, CandidatePhase::SavingConnection(_)) {
            // The verified Saved writer still owns this operation. Settling it
            // early would turn an in-flight durable write into a false failure.
            runtime.transient.put_local_candidate(candidate);
        } else {
            // Source queries borrow the catalogue slot and temporarily clear
            // catalog_loading. Suspension has already retired their generation,
            // so normal completion must not restore it. Carry only the matching
            // interrupted scan plan through this explicit lifecycle edge; the
            // shared rearm hook still waits for real worker completion before
            // issuing a fresh cancellation token or revalidating the source.
            if candidate.request.op == LocalOp::Sources
                && candidate.owner == OwnerToken(previous_generation)
                && candidate.interrupted.restart
                && runtime.effective_profile() == candidate.interrupted.profile.as_ref()
                && runtime.transient.accepted.as_ref().map(TransientMedia::owner)
                    == candidate.interrupted.owner
                && !runtime.catalog_complete
                && !runtime.catalog_loading
                && runtime.catalog_next.is_none()
            {
                runtime.catalog_next = candidate.interrupted.profile.clone();
                runtime.catalog_loading = runtime.catalog_next.is_some();
            }
            cancel_native_request(runtime, &candidate);
            let result = candidate.pending.fail(
                &candidate_context(runtime, app, &candidate.purpose),
                LocalError::RuntimeChanged,
            );
            // restore_scan preserves only the previous accepted scan while work
            // is paused; it must not mint another generation or uncancel a task.
            if let Admission::Complete { data } = result {
                finish_without_play(runtime, app, candidate, data);
            } else {
                settle_cancelled(runtime, &candidate);
            }
        }
    }
    if matches!(runtime.local_inventory.phase,
        InventoryPhase::Loading(generation) if generation == previous_generation)
    {
        runtime.local_inventory.phase = InventoryPhase::Unloaded;
    }
}

pub(in crate::app::ott_runtime) fn resume_after_lifecycle(runtime: &mut Runtime, _app: &mut App) {
    // A cancel request is not completion. Only the runner may reopen admission
    // after the old shared catalogue producer has actually released its slot.
    if !runtime.source_work_allowed() || !runtime.catalog_done.load(Ordering::Acquire) {
        return;
    }
    runtime.local_inventory.phase = InventoryPhase::Unloaded;
    if matches!(runtime.transient.accepted, Some(TransientMedia::Local(_)))
        && runtime.catalog_loading
        && !runtime.catalog_complete
    {
        runtime.catalog_next = runtime.effective_profile().cloned();
    }
    // launch_catalog takes LocalBindings before catalog_next. It revalidates the
    // existing records, including unremembered temporary roots, before resuming
    // this accepted catalogue. No retired command is replayed here.
}

/// Qualification follows the requested source, not whichever provider happens
/// to be on screen. A failed old Local root must not lock unrelated playback.
pub(in crate::app::ott_runtime) fn lifecycle_target_ready(
    runtime: &Runtime,
    profile: &ProviderProfile,
    selection: Option<&LocalSelection>,
) -> bool {
    if profile.kind != ProviderKind::Local {
        return true;
    }
    if let Some(candidate) = runtime.transient.local_candidate() {
        if candidate.owner == OwnerToken(runtime.generation) {
            if let CandidatePhase::Scanning {
                source,
                profile: candidate_profile,
                ..
            } = &candidate.phase
            {
                if candidate_profile == profile {
                    return capture_source_read(
                        &candidate.inventory,
                        &runtime.saved.profiles,
                        runtime.generation,
                        source,
                    )
                    .is_some_and(|read| {
                        selection.is_none_or(|selected| *selected == read.selection)
                    });
                }
            }
        }
    }
    if !matches!(runtime.local_inventory.phase, InventoryPhase::Ready(_)) {
        return false;
    }
    if let Some(selection) = selection {
        return runtime.capture_local_source(selection).is_some();
    }
    if let Some(TransientMedia::Local(session)) = runtime.transient.accepted.as_ref() {
        if session.profile == *profile {
            return runtime.local_source_read_is_current(&session.read);
        }
    }
    // A durable Local provider which has no temporary root binding keeps its
    // existing worker-side validation. No root grant is manufactured here.
    true
}

fn same_local_source(a: &LocalSelection, b: &LocalSelection) -> bool {
    a.source_id == b.source_id && a.authority == b.authority && a.generation == b.generation
}

/// A refreshed receipt cannot turn a newly opened file into the old source.
/// Keep this independent of optional parental policy. A settled Local session
/// retains its grant; an installed File Favorite retains the exact media handle.
fn resume_grant_matches(runtime: &Runtime, request: &PlaybackRequest, read: &LocalSourceRead) -> bool {
    let Some(selected) = request.local_selection.as_ref() else { return false; };
    if !read.is_live() || !same_local_source(selected, &read.selection) { return false; }
    if let Some(TransientMedia::Local(session)) = runtime.transient.accepted.as_ref() {
        if same_local_source(selected, &session.read.selection) {
            return session.read.is_live() && read.same_grant(&session.read);
        }
    }
    if read.source.descriptor().kind != local::SourceKind::File {
        // Preserve the existing non-session Folder continuation path. File
        // support cannot rely on its descriptor/path-only legacy behavior.
        return true;
    }
    let Some(playing) = runtime.playing.as_ref() else { return false; };
    if playing.selection_id != request.id
        || playing.identity.is_none()
        || playing.identity != request.continuation
        || playing.transient_owner != request.transient_owner
        || !playing.local_selection.as_ref().is_some_and(|bound| same_local_source(bound, selected))
        || playing.profile.kind != ProviderKind::Local
        || read.source.descriptor().canonical_path.as_deref() != Some(playing.profile.source.as_str())
    {
        return false;
    }
    let (Some(plx_media::ff::ExternalSource::HeldLocal(held)), MediaLocator::Local { relative_path }) =
        (playing.source.as_ref(), &playing.entry.media) else { return false; };
    // single_file accepts only this grant's exact basename. Equality compares
    // the opaque retained handle, never path spelling or filesystem identity.
    held.is_live() && read.source.single_file(relative_path).as_ref() == Some(held)
}

/// Keep a waiting continuation in place. Only settled source loss is an error;
/// unavailable validation itself never proves that a different disk was mounted.
pub(in crate::app::ott_runtime) fn lifecycle_resume_ready(
    runtime: &Runtime,
    request: &PlaybackRequest,
) -> Result<bool, &'static str> {
    if let Some(selection) = request.local_selection.as_ref() {
        if !matches!(runtime.local_inventory.phase, InventoryPhase::Ready(_)) {
            return Ok(false);
        }
        let source = runtime
            .current_local_source(selection)
            .ok_or("Local source changed before continuation")?;
        let read = runtime.capture_local_source(selection)
            .ok_or("Local source binding changed before continuation")?;
        if !resume_grant_matches(runtime, request, &read) {
            return Err("Local source capability changed before continuation");
        }
        let profile = source_profile(source)?;
        return if lifecycle_target_ready(runtime, &profile, Some(selection)) {
            Ok(true)
        } else {
            Err("Local source binding changed before continuation")
        };
    }
    if let Some(identity) = request.id.strip_prefix("favorite:") {
        return Ok(runtime
            .saved
            .favorites
            .entries()
            .iter()
            .find(|favorite| favorite.identity() == identity)
            .is_none_or(|favorite| {
                favorite.origin.provider != "local"
                    || matches!(runtime.local_inventory.phase, InventoryPhase::Ready(_))
            }));
    }
    Ok(runtime
        .effective_profile()
        .is_none_or(|profile| lifecycle_target_ready(runtime, profile, None)))
}

pub(in crate::app::ott_runtime) fn lifecycle_metadata_ready(runtime: &Runtime) -> bool {
    if let Some(ready) = runtime.local_asset_work_ready() { return ready; }
    let Some((profile_id, entry_id)) = runtime.metadata_target.as_ref() else {
        return true;
    };
    if let Some(identity) = entry_id.strip_prefix("favorite:") {
        return runtime
            .saved
            .favorites
            .entries()
            .iter()
            .find(|favorite| favorite.identity() == identity)
            .is_none_or(|favorite| {
                favorite.origin.provider != "local"
                    || matches!(runtime.local_inventory.phase, InventoryPhase::Ready(_))
            });
    }
    runtime
        .effective_profile()
        .filter(|profile| profile.id == *profile_id)
        .or_else(|| {
            runtime
                .saved
                .profiles
                .profiles
                .iter()
                .find(|profile| profile.id == *profile_id)
        })
        .is_none_or(|profile| lifecycle_target_ready(runtime, profile, None))
}

/// A retained continuation is user intent, not an old worker response. Only its
/// receipt is refreshed; source identity and the existing live handle stay fixed.
pub(in crate::app::ott_runtime) fn rebind_lifecycle_resume(runtime: &mut Runtime) {
    if !matches!(runtime.local_inventory.phase, InventoryPhase::Ready(_)) {
        return;
    }
    let Some(request) = runtime.resume_pending.as_ref() else { return; };
    let Some(previous) = request.local_selection.as_ref() else { return; };
    let Some(current) = runtime
        .current_local_source(previous)
        .and_then(|source| runtime.bind_local_selection(source).ok())
    else {
        return;
    };
    let Some(read) = runtime.capture_local_source(&current) else { return; };
    if !resume_grant_matches(runtime, request, &read) { return; }
    if let Some(request) = runtime.resume_pending.as_mut() {
        request.local_selection = Some(current);
    }
}
