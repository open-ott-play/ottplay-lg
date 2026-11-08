//! Retained Direct retry, addressed to the original admitted source.
//! No resolver, registry lookup, selection PUT or encoder is reachable here.
use super::*;
use plx_base::task::{MainThread, time::Deadline};
use plx_data::metadata::PlayingItem;
use plx_plex::plex::DirectSourceWitness;

#[derive(Clone, Debug)]
pub(super) struct DirectSource {
    pub witness: DirectSourceWitness,
    pub playback: Option<PlaybackIdentity>,
    pub sid: ServerId,
    pub rk: String,
    pub part: i64,
    pub media_index: usize,
}
impl DirectSource {
    pub(super) fn current(&self, ps: &PlaybackSession) -> bool {
        self.playback.is_some() && self.playback == ps.playback_identity
            && self.sid == ps.cur_sid && self.rk == ps.cur_rk
            && self.part > 0 && self.part == ps.cur_part_id
            && self.media_index == ps.cur_media_index && self.witness.current_for(&ps.url)
    }
}

/// One already-admitted start in the existing route/release owner. The guard
/// travels unchanged through deferred release, source open and decoder Load.
#[derive(Clone, Copy, Debug)]
pub struct RetainedDirectRetry {
    pub(crate) start: RouteStartTransaction,
    pub(crate) guard: PlexTrackLoadGuard,
}

pub fn prepare_retained_direct_retry(
    ps: &mut PlaybackSession, item: &PlayingItem,
    direct_play: Option<DirectPlayMode>, deadline: Deadline, main: &MainThread,
) -> Result<RetainedDirectRetry, PlexTrackApplyError> {
    use PlexTrackApplyError as E;
    if deadline.on_main(main).remaining().is_zero() { return Err(E::Expired); }
    if !ps.temporary_tracks.any() || direct_play.is_some_and(|mode| mode != ps.direct_play_mode) {
        return Err(E::NotSelectable);
    }
    let source = ps.direct_source.as_ref().filter(|source| source.current(ps)).ok_or(E::Stale)?;
    let witness = source.witness;
    let tracks = PlexTracks::capture(ps, item).ok_or(E::Stale)?;
    if !tracks.retains_native_selection(ps) { return Err(E::NotSelectable); }
    let mut control = PLAYER_CONTROL.lock().unwrap_or_else(|e| e.into_inner());
    if !matches!(control.phase, ControlPhase::Stable | ControlPhase::Failed(_) | ControlPhase::Idle)
        || control.pending_user.is_some() || control.pending_auto.is_some()
        || control.flight.is_some() || control.pending_original.is_some() {
        return Err(E::Busy);
    }
    // No mutation, including an intent bump, precedes the final checks.
    if !witness.current_for(&ps.url) { return Err(E::Stale); }
    if deadline.on_main(main).remaining().is_zero() { return Err(E::Expired); }
    advance_user_contract_locked(&mut control);
    control.next_action = next_generation(control.next_action);
    let serial = control.next_action;
    control.phase = ControlPhase::Prepared(serial);
    control.applied_revision = control.desired_revision;
    control.applied_quality = control.desired_quality;
    control.claimed_user = None;
    control.last_start_result = None;
    // A completed previous track request has no authority over this explicit
    // retry. The per-kind no-PUT provenance remains part of the projection.
    ps.temporary_tracks.pending = None;
    ps.write_back = None;
    let retained_resume = ps.requested_resume_ns;
    clear_play_verdict(ps);
    ps.requested_resume_ns = retained_resume;
    ps.finished = false;
    control.applied_projection = Some(route_projection(ps));
    Ok(RetainedDirectRetry {
        start: RouteStartTransaction { serial },
        guard: PlexTrackLoadGuard::retained_retry(deadline,
            TrackIntentTicket(control.desired_revision), witness),
    })
}

/// The fixture uses the production URL/witness constructor on a registered
/// synthetic client. It does not bypass any retry/capture admission predicate.
#[cfg(any(test, feature = "test-support"))]
pub fn qualify_plex_direct_retry_fixture_for_test(
    ps: &mut PlaybackSession, client: &'static plx_plex::plex::Client,
) -> bool {
    if ps.cur_sid != client.id() || ps.cur_part_id <= 0 { return false; }
    let part = format!("/library/parts/{}/file.mkv", ps.cur_part_id);
    let Some((url, witness)) = client.direct_play_source(&part, &ps.sess) else { return false; };
    ps.url = url.to_url();
    ps.direct_source = Some(DirectSource {
        witness, playback: ps.playback_identity.clone(), sid: ps.cur_sid, rk: ps.cur_rk.clone(),
        part: ps.cur_part_id, media_index: ps.cur_media_index,
    });
    install_plex_request_for_test(ps, client.id(), &ps.cur_rk.clone());
    commit_in_place_route_projection(ps, false);
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use plx_data::metadata::Stream;
    use plx_base::task::OffFrame;
    use std::time::Duration;
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            reset_player_control_for_test(&PlaybackSession::IDLE);
            crate::player::reset_audio_track();
            crate::player::reset_subtitle();
            plx_plex::plex::reset_servers_for_test();
        }
    }
    fn fixture() -> (PlaybackSession, PlayingItem, &'static plx_plex::plex::Client, Reset) {
        let sid = plx_plex::plex::register_for_test("retained-retry-test", "127.0.0.1", 1, "fixture-token", "fixture-client");
        let client = plx_plex::plex::client_for(sid).unwrap();
        let mut item = PlayingItem::with_subs(vec![Stream { id: 303, index: 3, codec: "srt".into(), ..Default::default() }]);
        item.sid = sid; item.rk = "same-item".into(); item.part_id = 77;
        item.audio = vec![Stream { id: 201, index: 1, codec: "aac".into(), channels: 2, sampling_rate: 48000, ..Default::default() }];
        let mut ps = PlaybackSession::IDLE;
        install_plex_track_fixture_for_test(&mut ps, sid, &item.rk, 77, CarriedAudio::from_stream(&item.audio[0], 0));
        assert!(qualify_plex_direct_retry_fixture_for_test(&mut ps, client));
        reset_player_control_for_test(&ps);
        let off = PlexTracks::capture(&ps, &item).unwrap().subtitle_off().unwrap();
        assert!(apply_plex_track_request(&mut ps, &off, deadline(), &main()).unwrap().is_none());
        (ps, item, client, Reset)
    }
    fn main() -> MainThread { unsafe { MainThread::assume() } }
    fn deadline() -> Deadline { Deadline::after(&OffFrame::for_test(), Duration::from_secs(60)) }
    #[test]
    fn retained_retry_uses_new_attempt_without_losing_selection_or_source() {
        let _serial = plx_base::testlock::serial();
        let (mut ps, item, _, _reset) = fixture();
        let before = (ps.url.clone(), ps.cur_audio.clone(), ps.cur_sub_sid, ps.playback_identity.clone());
        let old_intent = track_intent_ticket();
        // A terminal prior attempt's expired deadline is not this new request.
        ps.temporary_tracks.pending = Some(plex_tracks::PendingTrackEffect { revision: old_intent.0, serial: 500, deadline: Deadline::after(&OffFrame::for_test(), Duration::ZERO) });
        PLAYER_CONTROL.lock().unwrap().phase = ControlPhase::Failed(500);
        let retry = prepare_retained_direct_retry(&mut ps, &item, None, deadline(), &main()).unwrap();
        assert_ne!(track_intent_ticket(), old_intent);
        assert_eq!(before, (ps.url.clone(), ps.cur_audio.clone(), ps.cur_sub_sid, ps.playback_identity.clone()));
        assert!(ps.temporary_tracks.subtitle);
        assert!(ps.temporary_tracks.pending.is_none());
        assert!(ps.write_back.is_none());
        assert!(retry.guard.allows_main(&main()));
        let attempt = claim_route_start_attempt(retry.start).unwrap();
        let guard = retry.guard.bind_worker(attempt).unwrap();
        assert!(guard.allows(&OffFrame::for_test()));
        assert!(settle_route_start(&mut ps, attempt, RouteStartResult::Started));
        assert!(ps.temporary_tracks.subtitle);
        assert!(prepare_retained_direct_retry(&mut ps, &item, None, deadline(), &main()).is_ok());
    }
    #[test]
    fn expired_or_changed_part_and_missing_selection_refuse_before_intent() {
        let _serial = plx_base::testlock::serial();
        let (mut ps, mut item, _, _reset) = fixture();
        let before = (ps.url.clone(), track_intent_ticket(), pending_route_start());
        assert!(matches!(prepare_retained_direct_retry(&mut ps, &item, None, Deadline::after(&OffFrame::for_test(), Duration::ZERO), &main()), Err(PlexTrackApplyError::Expired)));
        item.part_id = 88;
        assert!(prepare_retained_direct_retry(&mut ps, &item, None, deadline(), &main()).is_err());
        item.part_id = 77; item.audio[0].id = 999;
        assert!(prepare_retained_direct_retry(&mut ps, &item, None, deadline(), &main()).is_err());
        assert_eq!(before, (ps.url.clone(), track_intent_ticket(), pending_route_start()));
    }
    #[test]
    fn deferred_retry_guard_is_revoked_before_effects_by_credential_or_new_intent() {
        let _serial = plx_base::testlock::serial();
        let (mut ps, item, client, _reset) = fixture();
        let retry = prepare_retained_direct_retry(&mut ps, &item, None, deadline(), &main()).unwrap();
        let queued = crate::player::release::QueuedStart { route_start: retry.start, track_guard: Some(retry.guard) };
        assert!(queued.track_guard.unwrap().allows_main(&main()));
        client.set_token(""); client.set_token("fixture-token");
        assert!(!queued.track_guard.unwrap().allows_main(&main()));
        assert!(!queued.track_guard.unwrap().allows(&OffFrame::for_test()));
        // A new explicit intent independently revokes the original guard.
        let _edit = begin_user_contract_boundary();
        assert!(!retry.guard.allows_main(&main()));
    }
    #[test]
    fn missing_source_authority_is_nonselectable_without_losing_initial_url() {
        let _serial = plx_base::testlock::serial();
        let (mut ps, item, _, _reset) = fixture();
        let url = ps.url.clone();
        ps.direct_source = None;
        let catalog = PlexTracks::capture(&ps, &item).unwrap();
        assert!(catalog.rows().iter().all(|row| row.choice.is_none()));
        assert!(catalog.subtitle_off().is_none());
        assert!(prepare_retained_direct_retry(&mut ps, &item, None, deadline(), &main()).is_err());
        assert_eq!(ps.url, url);
    }
    #[test]
    fn accepted_retry_retires_real_failure_but_refusal_preserves_it_and_resume() {
        let _serial = plx_base::testlock::serial();
        let (mut ps, item, _, _reset) = fixture();
        note_engine_failure(&mut ps, crate::player::RuntimeFailure::MediaSource, 9_000_000_000);
        assert_eq!(engine_failure(&ps), Some(crate::player::RuntimeFailure::MediaSource));
        let intent = track_intent_ticket();
        assert!(prepare_retained_direct_retry(&mut ps, &item, None,
            Deadline::after(&OffFrame::for_test(), Duration::ZERO), &main()).is_err());
        assert_eq!(engine_failure(&ps), Some(crate::player::RuntimeFailure::MediaSource));
        assert!(track_intent_is_current(intent));
        assert!(prepare_retained_direct_retry(&mut ps, &item, None, deadline(), &main()).is_ok());
        assert_eq!(engine_failure(&ps), None);
        assert!(!play_refused(&ps));
        assert!(!play_resolution_failed(&ps));
        assert_eq!(unpresented_resume_ns(&ps), 9_000_000_000);
    }

}
