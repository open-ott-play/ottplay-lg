//! Admission and completion of a track edit for its original live overlay.
use super::*;
use plx_appkit::track_menu::{TrackCommitFailure, TrackCommitOutcome};
use plx_screens::registry::TrackCommitRequest;
use plx_ui::containers::modal::Phase;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TrackCommitAdmission { Ignore, Reject(TrackCommitFailure), Ready }

fn exact_track_overlay(d: &Dispatcher<AppHost>, request: &TrackCommitRequest) -> bool {
    exact_track_overlay_in(d, request, false)
}

fn exact_track_overlay_in(d: &Dispatcher<AppHost>, request: &TrackCommitRequest, include_covered: bool) -> bool {
    let MachineId::Instance(instance) = request.reply.to else { return false };
    d.nav.modals.surfaces.iter()
        .chain(d.nav.covered_modals.iter().filter(|_| include_covered).flat_map(|(_, stack)| stack.surfaces.iter()))
        .any(|surface| {
        matches!(surface.phase, Phase::Opening | Phase::Open)
            && surface.entry.inst.as_ref().is_some_and(|body| {
                body.id == instance && body.screen.as_any()
                    .and_then(|a| a.downcast_ref::<plx_screens::player::overlay::PlayerOverlayScreen>())
                    .is_some_and(|overlay| overlay.accepts_track_commit(request))
            })
    })
}

/// Must be called before choosing native/Plex behavior or starting any write.
/// The caller subsequently checks its existing pending/terminal playback gate,
/// also before classification; ordinary buffering is not itself a refusal.
pub(crate) fn begin_track_commit(
    d: &mut Dispatcher<AppHost>, ps: &plx_media::route::PlaybackSession, request: &TrackCommitRequest,
) -> TrackCommitAdmission {
    if !exact_track_overlay_in(d, request, true) || d.nav.is_deliverable(&request.reply) {
        // A duplicate queued request must not cancel the first one's eventual reply.
        return TrackCommitAdmission::Ignore;
    }
    let MachineId::Instance(instance) = request.reply.to else { return TrackCommitAdmission::Ignore };
    d.track_inflight(instance, request.reply.req);
    if !exact_track_overlay(d, request) || !request.source.matches(ps) {
        return TrackCommitAdmission::Reject(TrackCommitFailure::StalePlayback);
    }
    TrackCommitAdmission::Ready
}

/// Non-mutating authority check for an admitted asynchronous edit. A covered
/// but still-live owner may complete; Back, closing, retirement and consumed
/// registrations revoke publication as well as eventual reply delivery.
pub(crate) fn track_commit_is_pending(d: &Dispatcher<AppHost>, request: &TrackCommitRequest) -> bool {
    d.nav.is_deliverable(&request.reply) && exact_track_overlay_in(d, request, true)
}

/// A late result never addresses whichever menu happens to be on top now.
/// Consumes its registration before queueing delivery; duplicate results are inert.
pub(crate) fn finish_track_commit(
    d: &mut Dispatcher<AppHost>, request: &TrackCommitRequest, outcome: TrackCommitOutcome,
) {
    if !d.nav.is_deliverable(&request.reply) { return; }
    let live = exact_track_overlay_in(d, request, true);
    let MachineId::Instance(instance) = request.reply.to else { return };
    if let Some(body) = d.nav.instance_mut(instance) {
        body.inflight.retain(|req| *req != request.reply.req);
    }
    if !live { return; }
    d.emit(MachineId::Player, Fx::Deliver(request.reply.to,
        Delivery::Screen(ScreenEvent::Async(request.reply.req, AppMsg::TrackCommitReply { outcome }))));
}

#[cfg(test)]
#[path = "bridge_track_feedback_tests.rs"]
mod tests;
