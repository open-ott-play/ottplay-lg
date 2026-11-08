//! An addressed receipt for the existing external audio reload. It owns no
//! worker, timer or second queue: the shared route controller remains authority.
use super::*;
use crate::ff::inspect::TrackKind;

#[derive(Debug)]
pub struct ExternalTrackRequest {
    playback: PlaybackIdentity,
    choice: ExternalTrackChoice,
    worker: WorkerTicket,
    revision: u64,
    serial: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExternalTrackRequestStatus {
    Queued,
    Pending,
    Started,
    Failed,
    Superseded,
}

/// Apply through the manual track owner. A changed audio selection returns the
/// exact next shared-controller action, while subtitles and no-ops finish here.
pub fn apply_external_track_request(
    ps: &mut PlaybackSession,
    choice: &ExternalTrackChoice,
) -> Result<Option<ExternalTrackRequest>, ExternalTrackApplyError> {
    let playback = playback_identity(ps).ok_or(ExternalTrackApplyError::Inactive)?;
    let changed = apply_external_track(ps, choice)?;
    if !changed || choice.kind() != TrackKind::Audio {
        return Ok(None);
    }
    let control = PLAYER_CONTROL.lock().unwrap_or_else(|e| e.into_inner());
    Ok(Some(ExternalTrackRequest {
        playback,
        choice: choice.clone(),
        worker: worker_ticket_of(&control),
        revision: control.desired_revision,
        serial: next_generation(control.next_action),
    }))
}

/// Observe only the action minted by this request. Its own native reload may
/// change the worker ticket; before claim even a seek/worker change is stale.
/// Started means the exact native attempt was accepted, never audible output.
pub fn external_track_request_status(
    ps: &PlaybackSession,
    request: &ExternalTrackRequest,
) -> ExternalTrackRequestStatus {
    use ExternalTrackRequestStatus as S;
    if playback_identity(ps).as_ref() != Some(&request.playback)
        || !external_tracks(ps).is_some_and(|tracks| {
            tracks.selected_audio() == request.choice.ordinal()
                && tracks.choice(TrackKind::Audio, request.choice.ordinal()).as_ref()
                    == Ok(&request.choice)
        })
    {
        return S::Superseded;
    }
    let control = PLAYER_CONTROL.lock().unwrap_or_else(|e| e.into_inner());
    if control.desired_revision != request.revision {
        return S::Superseded;
    }
    if control.phase == ControlPhase::Stable
        && control.pending_user == Some(UserRouteIntent::NativeAudioReload)
        && next_generation(control.next_action) == request.serial
    {
        return if worker_ticket_of(&control) == request.worker { S::Queued } else { S::Superseded };
    }
    if !matches!(control.claimed_user,
        Some((serial, UserRouteIntent::NativeAudioReload, _)) if serial == request.serial)
    {
        return S::Superseded;
    }
    match control.phase {
        ControlPhase::Applying(serial) | ControlPhase::Preparing(serial)
        | ControlPhase::Prepared(serial) | ControlPhase::Starting(serial, _)
        | ControlPhase::Completing(serial) if serial == request.serial => S::Pending,
        ControlPhase::Failed(serial) if serial == request.serial => S::Failed,
        ControlPhase::Stable => {
            if control.applied_revision == request.revision
                && matches!(control.last_start_result,
                    Some((attempt, RouteStartResult::Started)) if attempt.serial == request.serial)
            {
                S::Started
            } else {
                S::Failed
            }
        }
        _ => S::Superseded,
    }
}

/// Withdraw only an unclaimed proposal from this exact logical playback. Once
/// a Load has been dispatched, cancellation withdraws the caller's reply only;
/// it must not manufacture a rollback over a native effect already in flight.
pub fn cancel_external_track_request(ps: &mut PlaybackSession, request: &ExternalTrackRequest) -> bool {
    if playback_identity(ps).as_ref() != Some(&request.playback) {
        return false;
    }
    let restore = {
        let mut control = PLAYER_CONTROL.lock().unwrap_or_else(|e| e.into_inner());
        if control.phase != ControlPhase::Stable
            || control.desired_revision != request.revision
            || control.pending_user != Some(UserRouteIntent::NativeAudioReload)
            || next_generation(control.next_action) != request.serial
        {
            return false;
        }
        control.pending_user = None;
        control.displaced_pick = false;
        advance_user_contract_locked(&mut control);
        control.applied_projection.clone()
    };
    if let Some(restore) = restore {
        install_route_projection(ps, &restore);
    }
    crate::player::cancel_audio_pick();
    true
}
