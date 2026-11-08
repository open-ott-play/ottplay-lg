//! Typed internal Local-folder preparation on the existing transient pipeline.
use super::*;
use crate::app::ott_kiosk_stored::{self as stored, Stored, TargetRef};
use crate::app::ott_runtime::work_lifecycle::{
    PreparationFailure, PreparationOutcome, RestoreStartError,
};

pub(in crate::app::ott_runtime) enum LocalPurpose {
    Remote(Invocation),
    Profile(crate::app::ott_runtime::local_profile::Receipt),
    KioskFolder(LocalKioskPreparation),
}
impl LocalPurpose {
    pub(super) fn invocation(&self) -> Option<&Invocation> {
        match self {
            Self::Remote(invocation) => Some(invocation),
            Self::KioskFolder(_) | Self::Profile(_) => None,
        }
    }
    pub(super) fn admitted_resume(&self, runtime: &Runtime) -> (Option<kiosk_resume::Scope>, Option<continuity::ResumePoint>) {
        match self {
            Self::KioskFolder(value) if value.resume_scope.is_some()
                && kiosk_resume::capture(runtime, &value.read.selection) == value.resume_scope =>
                    (value.resume_scope.clone(), value.resume_point.clone()),
            _ => (None, None),
        }
    }
    pub(super) fn resume_warning(&self) -> Option<&'static str> {
        match self { Self::KioskFolder(value) => value.resume_warning, Self::Remote(_) | Self::Profile(_) => None }
    }
    pub(super) fn note_first(&mut self, id: String) {
        if let Self::KioskFolder(value) = self {
            value.first_id = Some(id);
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Pending,
    Failed,
    NativeAccepted,
}

/// Captures existing ownership only. The common Recovery slot retains a clone;
/// no private path, grant or token is serialized or printed.
#[derive(Clone)]
pub(in crate::app) struct LocalKioskPreparation {
    phase: Phase,
    owner: OwnerToken,
    cancel: Arc<AtomicBool>,
    playback_epoch: u64,
    navigation_epoch: u64,
    observed_generation: u64,
    observed_cancel: Arc<AtomicBool>,
    policy: Stored,
    read: LocalRootRead,
    fingerprint: String,
    runtime: String,
    first_id: Option<String>,
    resume_scope: Option<kiosk_resume::Scope>,
    resume_point: Option<continuity::ResumePoint>,
    resume_warning: Option<&'static str>,
    native_ticket: plx_media::route::PlayRequestTicket,
}
impl LocalKioskPreparation {
    pub(in crate::app) fn same_attempt(&self, other: &Self) -> bool {
        self.owner == other.owner
            && self.playback_epoch == other.playback_epoch
            && Arc::ptr_eq(&self.cancel, &other.cancel)
    }
    fn common(&self) -> KioskPreparation {
        KioskPreparation::from_local(self.clone())
    }
}

fn source_read(runtime: &Runtime, source_id: &str, fingerprint: &str) -> Option<LocalRootRead> {
    if !matches!(runtime.local_inventory.phase, InventoryPhase::Ready(_))
        || runtime.local_inventory.proof_cleanup != ProofCleanup::Clean
    {
        return None;
    }
    let source = runtime
        .local_inventory
        .sources
        .iter()
        .find(|source| source.descriptor().id == source_id)?;
    if selection::local_root_fingerprint(source.descriptor())
        .ok()?
        .as_str()
        != fingerprint
    {
        return None;
    }
    let selection = runtime.bind_local_selection(source.descriptor()).ok()?;
    let read = runtime.capture_local_root(&selection)?;
    read.root.is_live().then_some(read)
}
fn source_current(runtime: &Runtime, value: &LocalKioskPreparation) -> bool {
    source_read(runtime, &value.read.selection.source_id, &value.fingerprint).is_some_and(|read| {
        read.selection.authority == value.read.selection.authority
            && read.selection.generation == value.read.selection.generation
            && Arc::ptr_eq(&read.root, &value.read.root)
    })
}
fn exact_target(runtime: &Runtime, value: &LocalKioskPreparation) -> bool {
    runtime.play_target.as_ref().is_none_or(|target| {
        target.transient_owner == Some(value.owner)
            && target.position_ms == 0
            && target.continuation.is_none()
            && !target.advance_on_failure
            && target.kiosk_selection.is_none()
            && (value.first_id.as_deref() == Some(target.id.as_str())
                || runtime.transient.local_candidate().is_some_and(|candidate| {
                    matches!(&candidate.purpose, LocalPurpose::KioskFolder(current) if current.same_attempt(value))
                        && matches!(&candidate.phase, CandidatePhase::Scanning { proposed: Some(local), .. }
                            if local.order().first() == Some(&target.id))
                }))
            && target.local_selection.as_ref().is_some_and(|local| {
                local.source_id == value.read.selection.source_id
                    && local.authority == value.read.selection.authority
                    && local.generation == value.read.selection.generation
            })
    })
}
fn intent_current(runtime: &Runtime, app: &App, value: &LocalKioskPreparation) -> bool {
    runtime.loaded
        && !runtime.persistence_uncertain
        && runtime.playback_epoch == value.playback_epoch
        && runtime.navigation_epoch == value.navigation_epoch
        && runtime.generation == value.observed_generation
        && Arc::ptr_eq(&runtime.cancel, &value.observed_cancel)
        && app.player.pending_exit.is_none()
        && !lifecycle::viewer_paused()
        && plx_media::route::play_request_is_current(value.native_ticket)
        && exact_target(runtime, value)
        && stored::read(&runtime.saved.settings)
            .ok()
            .flatten()
            .is_some_and(|policy| policy.same_target(&value.policy))
        && source_current(runtime, value)
}

pub(in crate::app::ott_runtime) fn kiosk_preparation_is_current(
    runtime: &Runtime,
    app: &App,
    value: &LocalKioskPreparation,
) -> bool {
    intent_current(runtime, app, value)
        && !value.observed_cancel.load(Ordering::Acquire)
        && match value.phase {
            Phase::Pending | Phase::Failed => true,
            Phase::NativeAccepted => matches!(runtime.transient.accepted.as_ref(),
                Some(TransientMedia::Local(session)) if session.context.owner() == value.owner
                    && runtime.playing.as_ref().is_some_and(|playing|
                        playing.transient_owner == Some(value.owner)
                            && playing_matches(playing, app))),
        }
}
pub(in crate::app::ott_runtime) fn kiosk_preparation_resume_policy(
    runtime: &Runtime,
    app: &App,
    value: &LocalKioskPreparation,
) -> Option<Stored> {
    if !intent_current(runtime, app, value) {
        return None;
    }
    stored::read(&runtime.saved.settings).ok().flatten()
}
pub(in crate::app::ott_runtime) fn retry_kiosk_preparation(
    runtime: &mut Runtime,
    app: &mut App,
    value: &LocalKioskPreparation,
) -> Result<(), RestoreStartError> {
    if !kiosk_preparation_is_current(runtime, app, value) {
        return Err(RestoreStartError::ContextChanged);
    }
    if value.phase == Phase::Pending {
        let Some(candidate) = runtime.transient.take_local_candidate() else {
            return Err(RestoreStartError::ContextChanged);
        };
        if !matches!(&candidate.purpose, LocalPurpose::KioskFolder(current) if current.same_attempt(value))
        {
            runtime.transient.put_local_candidate(candidate);
            return Err(RestoreStartError::ContextChanged);
        }
        // The lifecycle's existing watchdog owns timing. Retire only this
        // candidate; the real worker's occupancy stays set until completion.
        cancel_native_request(runtime, &candidate);
        settle_native_failure(runtime, app, &candidate, LocalError::CatalogLoading);
        // Recovery retains the addressed Failed receipt and schedules its
        // normal retry interval. Never replace that new terminal event here.
        return Err(RestoreStartError::Busy);
    }
    if !runtime.catalog_done.load(Ordering::Acquire) || runtime.resolver_busy {
        return Err(RestoreStartError::Busy);
    }
    kiosk_restore_local_folder(
        runtime,
        app,
        &value.read.selection.source_id,
        &value.fingerprint,
    )
}

pub(in crate::app::ott_runtime) fn kiosk_restore_local_folder(
    runtime: &mut Runtime,
    app: &mut App,
    source_id: &str,
    source_fingerprint: &str,
) -> Result<(), RestoreStartError> {
    if !runtime.loaded || runtime.persistence_uncertain {
        return Err(RestoreStartError::Unavailable);
    }
    if !playback_allowed(app)
        || !runtime.play_work_allowed()
        || runtime.saving
        || runtime.transient.candidate.is_some()
        || runtime.resolver_busy
        || runtime.resolver_next.is_some()
    {
        return Err(RestoreStartError::Busy);
    }
    let policy = stored::read(&runtime.saved.settings)
        .map_err(|_| RestoreStartError::Invalid)?
        .ok_or(RestoreStartError::ContextChanged)?;
    if !matches!(policy.target(), TargetRef::LocalFolder { source_id: id, source_fingerprint: fingerprint }
        if id == source_id && fingerprint == source_fingerprint)
    {
        return Err(RestoreStartError::ContextChanged);
    }
    let read = source_read(runtime, source_id, source_fingerprint)
        .ok_or(RestoreStartError::Unavailable)?;
    let source = runtime
        .local_inventory
        .sources
        .iter()
        .find(|source| source.descriptor().id == source_id)
        .ok_or(RestoreStartError::Unavailable)?
        .descriptor()
        .clone();
    let profile = source_profile(&source).map_err(|_| RestoreStartError::Invalid)?;
    let resume_warning = kiosk_resume::preflight(runtime, source_id).err().map(|error| error.message());
    let resume_scope = kiosk_resume::capture(runtime, &read.selection);
    let resume_point = resume_scope.as_ref().and_then(|scope| kiosk_resume::restore(runtime, scope));
    let interrupted = InterruptedScan {
        profile: runtime.effective_profile().cloned(),
        owner: runtime
            .transient
            .accepted
            .as_ref()
            .map(TransientMedia::owner),
        restart: runtime.catalog_loading && !runtime.catalog_complete,
    };
    runtime.resume_pending = None;
    runtime.playback_epoch = runtime.playback_epoch.wrapping_add(1);
    commands::cancel_pending_play(runtime);
    runtime.cancel_play_request();
    runtime.cancel.store(true, Ordering::Release);
    let classic_generation = runtime.generation;
    runtime.generation = runtime.generation.wrapping_add(1).max(1);
    classic_source::rebase(runtime, classic_generation);
    runtime.continuity.rebase(runtime.generation);
    runtime.cancel = Arc::new(AtomicBool::new(false));
    runtime.catalog_loading = false;
    runtime.catalog_next = Some(profile.clone());
    let receipt = LocalKioskPreparation {
        phase: Phase::Pending,
        owner: OwnerToken(runtime.generation),
        cancel: runtime.cancel.clone(),
        playback_epoch: runtime.playback_epoch,
        navigation_epoch: runtime.navigation_epoch,
        observed_generation: runtime.generation,
        observed_cancel: runtime.cancel.clone(),
        policy,
        read,
        fingerprint: source_fingerprint.into(),
        runtime: runtime.remote.runtime().into(),
        first_id: None,
        resume_scope,
        resume_point,
        resume_warning,
        native_ticket: plx_media::route::play_request_ticket(),
    };
    let request = local::LocalRequest {
        op: LocalOp::Play,
        runtime: receipt.runtime.clone(),
        selector: Some(local::Selector::Name(source_id.into())),
        include_paths: false,
        session: true,
        progressive: true,
    };
    runtime.transient.put_local_candidate(LocalCandidate {
        inventory: runtime.local_inventory.sources.clone(),
        admission_ticket: receipt.native_ticket,
        owner: receipt.owner,
        purpose: LocalPurpose::KioskFolder(receipt.clone()),
        pending: local::PendingLocal::new(
            request.clone(),
            receipt.owner,
            runtime.now_ms().saturating_add(35_000),
        ),
        request,
        interrupted,
        phase: CandidatePhase::Scanning {
            source,
            profile,
            catalog: Catalog::default(),
            state: local::ScanState::Scanning,
            proposed: None,
        },
    });
    runtime.kiosk_preparation_started(receipt.common());
    runtime.status = "Opening local source…".into();
    runtime.changed();
    runtime.launch_catalog();
    Ok(())
}

pub(super) fn candidate_context<'a>(
    runtime: &'a Runtime,
    app: &App,
    purpose: &'a LocalPurpose,
) -> AdmissionContext<'a> {
    match purpose {
        LocalPurpose::Remote(invocation) => context(runtime, app, invocation),
        LocalPurpose::Profile(receipt) => crate::app::ott_runtime::local_profile::context(runtime, app, receipt),
        LocalPurpose::KioskFolder(value) => AdmissionContext {
            runtime: &value.runtime,
            owner: OwnerToken(runtime.generation),
            invocation_current: kiosk_preparation_is_current(runtime, app, value),
            now_ms: runtime.now_ms(),
            mutations_allowed: runtime.loaded
                && !runtime.persistence_uncertain
                && playback_allowed(app)
                && runtime.source_work_allowed(),
        },
    }
}
pub(super) fn candidate_target_matches(runtime: &Runtime, candidate: &LocalCandidate) -> bool {
    let CandidatePhase::Scanning {
        proposed: Some(local),
        ..
    } = &candidate.phase
    else {
        return false;
    };
    runtime.play_target.as_ref().is_some_and(|target| {
        target.transient_owner == Some(candidate.owner)
            && local.order().first().is_some_and(|id| id == &target.id)
            && candidate.purpose.invocation().is_none_or(|expected| {
                runtime
                    .pending_play
                    .as_ref()
                    .is_some_and(|actual| actual.id == expected.id)
            })
    })
}
fn outcome(error: LocalError) -> PreparationOutcome {
    match error {
        LocalError::RuntimeChanged | LocalError::Restricted => PreparationOutcome::Cancelled,
        LocalError::CatalogLoading => PreparationOutcome::Failed(PreparationFailure::Timeout),
        LocalError::EmptySource | LocalError::SourceNotFound => {
            PreparationOutcome::Failed(PreparationFailure::MissingItem)
        }
        LocalError::SourceUnavailable | LocalError::CatalogUnavailable => {
            PreparationOutcome::Failed(PreparationFailure::Unavailable)
        }
        _ => PreparationOutcome::Failed(PreparationFailure::StartFailed),
    }
}
pub(super) fn settle_without_play(runtime: &mut Runtime, app: &App, candidate: &LocalCandidate) {
    settle_native_failure(
        runtime,
        app,
        candidate,
        candidate
            .pending
            .completion()
            .and_then(Result::err)
            .unwrap_or(LocalError::RuntimeChanged),
    );
}
pub(super) fn settle_native_failure(
    runtime: &mut Runtime,
    app: &App,
    candidate: &LocalCandidate,
    error: LocalError,
) {
    let receipt = match &candidate.purpose {
        LocalPurpose::Remote(_) | LocalPurpose::Profile(_) => {
            restore_scan(runtime, candidate);
            return;
        }
        LocalPurpose::KioskFolder(value) => value,
    };
    let outcome = outcome(error);
    let eligible =
        outcome != PreparationOutcome::Cancelled && intent_current(runtime, app, receipt);
    restore_scan(runtime, candidate);
    let mut receipt = receipt.clone();
    if eligible {
        receipt.phase = Phase::Failed;
        receipt.observed_generation = runtime.generation;
        receipt.observed_cancel = runtime.cancel.clone();
    }
    runtime.kiosk_preparation_finished(
        &receipt.common(),
        if eligible {
            outcome
        } else {
            PreparationOutcome::Cancelled
        },
    );
}
pub(super) fn settle_cancelled(runtime: &mut Runtime, candidate: &LocalCandidate) {
    if let LocalPurpose::Profile(receipt) = &candidate.purpose {
        crate::app::ott_runtime::local_profile::remember_failure(runtime, receipt);
    }
    if let LocalPurpose::KioskFolder(value) = &candidate.purpose {
        runtime.kiosk_preparation_finished(&value.common(), PreparationOutcome::Cancelled);
    }
    restore_scan(runtime, candidate);
}
pub(super) fn settle_native_accepted(runtime: &mut Runtime, app: &App, candidate: &LocalCandidate) {
    if let LocalPurpose::KioskFolder(value) = &candidate.purpose {
        let mut terminal = value.clone();
        terminal.phase = Phase::NativeAccepted;
        runtime.kiosk_preparation_finished(
            &terminal.common(),
            if intent_current(runtime, app, value) {
                PreparationOutcome::NativeAccepted
            } else {
                PreparationOutcome::Cancelled
            },
        );
    }
}
pub(super) fn refresh_native_receipt(
    runtime: &Runtime,
    candidate: &mut LocalCandidate,
    result: Result<(), &'static str>,
    native: Option<&NativeStartReceipt>,
) -> Result<(), &'static str> {
    if let LocalPurpose::KioskFolder(value) = &mut candidate.purpose {
        if let Some(receipt) = native {
            if receipt.before != value.native_ticket
                || receipt.generation != runtime.play_generation
                || !plx_media::route::play_request_is_current(receipt.after)
            {
                return Err("Local native admission changed");
            }
            // An addressed synchronous failure may advance the same native
            // ticket too. Preserve its failure while retaining retry identity.
            value.native_ticket = receipt.after;
        } else if result.is_ok() {
            return Err("Missing Local native admission receipt");
        }
    }
    result
}

/// Final source proof for the first staged Local entry. Uses the real candidate
/// inventory/root and request receipt; never falls through to an old catalog.
pub(in crate::app::ott_runtime) fn staged_native_binding(
    runtime: &Runtime,
    request: &PlaybackRequest,
) -> Option<Result<(ProviderProfile, CatalogEntry), &'static str>> {
    let candidate = runtime.transient.local_candidate()?;
    let valid = || {
        let CandidatePhase::Scanning {
            source,
            profile,
            catalog,
            proposed: Some(local),
            state,
        } = &candidate.phase
        else {
            return None;
        };
        if *state == local::ScanState::Failed
            || candidate.owner != OwnerToken(runtime.generation)
            || request.transient_owner != Some(candidate.owner)
            || runtime.cancel.load(Ordering::Acquire)
            || !plx_media::route::play_request_is_current(candidate.admission_ticket)
            || request.kiosk_selection.is_some()
            || request.position_ms != 0
            || request.continuation.is_some()
            || request.advance_on_failure
            || local.order().first() != Some(&request.id)
        {
            return None;
        }
        let read = capture_source_read(
            &candidate.inventory,
            &runtime.saved.profiles,
            runtime.generation,
            source,
        )?;
        if request.local_selection.as_ref() != Some(&read.selection) || !read.is_live() {
            return None;
        }
        let entry = catalog
            .entries
            .iter()
            .find(|entry| entry.id == request.id)?;
        matches!(entry.media, MediaLocator::Local { .. }).then(|| (profile.clone(), entry.clone()))
    };
    Some(valid().ok_or("Staged Local source changed before native admission"))
}
