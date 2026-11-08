//! Opaque proof for the existing lifecycle recovery slot. No timer or owner is
//! created here; preparation still lives in the common transient candidate.
use super::*;
use crate::app::ott_kiosk_preparation::KioskPreparation;
use crate::app::ott_runtime::work_lifecycle::{
    PreparationFailure, PreparationOutcome, RestoreStartError,
};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Pending,
    Failed,
    NativeAccepted,
}

#[derive(Clone)]
pub(in crate::app) struct PlexKioskPreparation {
    work: Preparation,
    ticket: plx_media::route::PlayRequestTicket,
    phase: Phase,
}
impl PlexKioskPreparation {
    pub(in crate::app) fn same_attempt(&self, other: &Self) -> bool {
        self.work.owner == other.work.owner
            && self.work.playback_epoch == other.work.playback_epoch
            && Arc::ptr_eq(&self.work.cancel, &other.work.cancel)
    }
}
fn receipt(candidate: &PlexCandidate) -> Option<PlexKioskPreparation> {
    matches!(candidate.work.purpose, Purpose::KioskRestore { .. }).then(|| PlexKioskPreparation {
        work: candidate.work.clone(),
        ticket: candidate.ticket,
        phase: Phase::Pending,
    })
}
fn fields_current(runtime: &Runtime, proof: &PlexKioskPreparation) -> bool {
    let Purpose::KioskRestore { target, .. } = &proof.work.purpose else {
        return false;
    };
    runtime.loaded
        && !runtime.persistence_uncertain
        && runtime.playback_epoch == proof.work.playback_epoch
        && runtime.navigation_epoch == proof.work.navigation_epoch
        && durable_queue_matches(runtime, target)
        && runtime
            .saved
            .profiles
            .profiles
            .iter()
            .any(|p| p == &proof.work.profile && p.enabled)
        && source_current(runtime, &proof.work)
        && proof.work.authority.matches(proof.work.client)
        && plx_plex::plex::client_for(proof.work.authority.server)
            .is_some_and(|client| proof.work.authority.matches(client))
}
fn intent_current(runtime: &Runtime, app: &App, proof: &PlexKioskPreparation) -> bool {
    if !fields_current(runtime, proof)
        || !plx_media::route::play_request_is_current(proof.ticket)
        || lifecycle::viewer_paused()
        || app.player.pending_exit.is_some()
    {
        return false;
    }
    if let Some(candidate) = candidate(runtime) {
        if !receipt(candidate).is_some_and(|current| current.same_attempt(proof)) {
            return false;
        }
    } else if runtime.transient.candidate.is_some() {
        return false;
    }
    if runtime.play_target.as_ref().is_some_and(|target| {
        target.transient_owner != Some(proof.work.owner)
            || target.id != format!("plex-occurrence:{}:0", proof.work.owner.0)
            || target.local_selection.is_some()
            || target.kiosk_selection.is_some()
            || target.continuation.is_some()
            || target.position_ms != 0
            || target.advance_on_failure
    }) {
        return false;
    }
    match proof.phase {
        Phase::Pending => {
            !proof.work.cancel.load(Ordering::Acquire)
                && candidate(runtime).is_some_and(|candidate| {
                    receipt(candidate).is_some_and(|current| current.same_attempt(proof))
                })
        }
        // This provider-issued terminal receipt survives its own timeout cancel.
        // The expired work deadline is deliberately not a retry deadline.
        Phase::Failed => true,
        Phase::NativeAccepted => session(runtime).is_some_and(|session| {
            session.queue.owner().0 == proof.work.owner.0
                && runtime
                    .playing
                    .as_ref()
                    .is_some_and(|playing| session.native_matches(playing, app))
        }),
    }
}
pub(in crate::app::ott_runtime) fn kiosk_preparation_is_current(
    runtime: &Runtime,
    app: &App,
    proof: &PlexKioskPreparation,
) -> bool {
    intent_current(runtime, app, proof)
}
pub(in crate::app::ott_runtime) fn kiosk_preparation_resume_policy(
    runtime: &Runtime,
    app: &App,
    proof: &PlexKioskPreparation,
) -> Option<stored::Stored> {
    if !intent_current(runtime, app, proof) {
        return None;
    }
    stored::read(&runtime.saved.settings).ok().flatten()
}
pub(in crate::app::ott_runtime) fn retry_kiosk_preparation(
    runtime: &mut Runtime,
    app: &mut App,
    proof: &PlexKioskPreparation,
) -> Result<(), RestoreStartError> {
    if !intent_current(runtime, app, proof) {
        return Err(RestoreStartError::ContextChanged);
    }
    // Only the existing lifecycle watchdog chooses when this is due. Pending
    // work gets a real timeout terminal event; its worker slot remains occupied.
    if proof.phase == Phase::Pending {
        retire_candidate(runtime, app, WireError::Queue(QueueError::TimedOut));
        return Err(RestoreStartError::Busy);
    }
    if !runtime.play_work_allowed()
        || !playback_allowed(app)
        || runtime.saving
        || runtime.resolver_busy
        || runtime.resolver_next.is_some()
        || runtime.transient.candidate.is_some()
        || runtime.play_target.is_some()
    {
        return Err(RestoreStartError::Busy);
    }
    let Purpose::KioskRestore { target, .. } = &proof.work.purpose else {
        return Err(RestoreStartError::Invalid);
    };
    let TargetRef::Queue(entries) = target.target() else {
        return Err(RestoreStartError::Invalid);
    };
    kiosk_restore_queue(runtime, app, entries)
}
pub(super) fn started(runtime: &mut Runtime) {
    let Some(proof) = candidate(runtime).and_then(receipt) else {
        return;
    };
    runtime.kiosk_preparation_started(KioskPreparation::from_plex(proof));
}
fn failure(error: WireError) -> PreparationOutcome {
    match error {
        WireError::Queue(QueueError::MissingItem) | WireError::InvalidResult => {
            PreparationOutcome::Failed(PreparationFailure::MissingItem)
        }
        WireError::Queue(QueueError::Unavailable | QueueError::ServerUnavailable) => {
            PreparationOutcome::Failed(PreparationFailure::Unavailable)
        }
        WireError::Queue(QueueError::TimedOut) => {
            PreparationOutcome::Failed(PreparationFailure::Timeout)
        }
        WireError::Queue(QueueError::StartFailed) => {
            PreparationOutcome::Failed(PreparationFailure::StartFailed)
        }
        _ => PreparationOutcome::Cancelled,
    }
}
pub(super) fn failed(runtime: &mut Runtime, candidate: &PlexCandidate, error: WireError) {
    let Some(mut proof) = receipt(candidate) else {
        return;
    };
    let outcome = if fields_current(runtime, &proof)
        && plx_media::route::play_request_is_current(proof.ticket)
    {
        failure(error)
    } else {
        PreparationOutcome::Cancelled
    };
    if matches!(outcome, PreparationOutcome::Failed(_)) {
        proof.phase = Phase::Failed;
    }
    runtime.kiosk_preparation_finished(&KioskPreparation::from_plex(proof), outcome);
}
pub(super) fn invalid_error(runtime: &Runtime, candidate: &PlexCandidate) -> WireError {
    if let Some(proof) = receipt(candidate) {
        if fields_current(runtime, &proof)
            && plx_media::route::play_request_is_current(proof.ticket)
            && runtime.source_work_allowed()
            && !lifecycle::viewer_paused()
            && !proof.work.cancel.load(Ordering::Acquire)
            && matches!(proof.work.purpose, Purpose::KioskRestore { expires_ms, .. } if expires_ms <= runtime.now_ms())
        {
            return WireError::Queue(QueueError::TimedOut);
        }
    }
    WireError::Queue(QueueError::ContextChanged)
}
pub(super) fn native_finished(
    runtime: &mut Runtime,
    app: &App,
    candidate: &PlexCandidate,
    result: Result<(), &'static str>,
    native: Option<&crate::app::ott_runtime::NativeStartReceipt>,
) {
    let Some(mut proof) = receipt(candidate) else {
        return;
    };
    let addressed = native.is_none_or(|native| {
        native.generation == runtime.play_generation
            && native.before == proof.ticket
            && plx_media::route::play_request_is_current(native.after)
    });
    if addressed {
        if let Some(native) = native {
            proof.ticket = native.after;
        }
    }
    let current = addressed
        && fields_current(runtime, &proof)
        && plx_media::route::play_request_is_current(proof.ticket)
        && !lifecycle::viewer_paused()
        && app.player.pending_exit.is_none();
    let outcome = if !current {
        PreparationOutcome::Cancelled
    } else if result.is_err() {
        proof.phase = Phase::Failed;
        PreparationOutcome::Failed(PreparationFailure::StartFailed)
    } else if native.is_some()
        && candidate.staged.as_ref().is_some_and(|(session, _)| {
            runtime
                .playing
                .as_ref()
                .is_some_and(|playing| session.native_matches(playing, app))
        })
    {
        proof.phase = Phase::NativeAccepted;
        PreparationOutcome::NativeAccepted
    } else {
        PreparationOutcome::Cancelled
    };
    runtime.kiosk_preparation_finished(&KioskPreparation::from_plex(proof), outcome);
}

#[cfg(all(test, feature = "hostsim"))]
mod tests {
    use super::*;
    #[test]
    fn remote_cancelled_play_is_rejected_for_each_state_version() {
        let mut runtime =
            Runtime::unloaded(crate::app::ott_companion::Supervisor::inert_for_test());
        runtime.remote = Remote::disabled_for_test("dev_fixture", "fixture");
        for version in [1, 2] {
            let mut remote = Remote::disabled_for_test("dev_fixture", "fixture");
            let mut params = serde_json::json!({"runtime":"fixture","op":"play","ids":["42"]});
            if version == 2 {
                params["state_version"] = serde_json::json!(2);
            }
            let invocation = remote.request_for_test("plex_queue", params.clone());
            let request = queue::parse("plex_queue", &params, "fixture").unwrap();
            let purpose = Purpose::Remote {
                invocation,
                request,
            };
            let response = purpose.error(&runtime, WireError::Queue(QueueError::Cancelled));
            assert_eq!(response, rejected(QueueError::Cancelled.message()));
        }
    }
}
