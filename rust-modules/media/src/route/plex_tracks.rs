//! Temporary, addressed track picks for an already installed Plex leaf.
//! No new owner: effects and receipts use the shared route controller.
use super::*;
use crate::ff::inspect::TrackKind;
use plx_base::task::{MainThread, time::Deadline};
use plx_data::metadata::{self, PlayingItem};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TrackIntentTicket(pub(super) u64);
pub fn track_intent_ticket() -> TrackIntentTicket {
    TrackIntentTicket(PLAYER_CONTROL.lock().unwrap_or_else(|e| e.into_inner()).desired_revision)
}
pub fn track_intent_is_current(ticket: TrackIntentTicket) -> bool { track_intent_ticket() == ticket }

#[derive(Clone, Copy, Debug)]
pub(super) struct TemporaryTracks {
    pub audio: bool,
    pub subtitle: bool,
    pub(super) pending: Option<PendingTrackEffect>,
}
#[derive(Clone, Copy, Debug)]
pub(super) struct PendingTrackEffect { pub revision: u64, pub serial: u64, pub deadline: Deadline }
impl TemporaryTracks {
    pub const NONE: Self = Self { audio: false, subtitle: false, pending: None };
    pub fn any(self) -> bool { self.audio || self.subtitle }
}

#[derive(Clone, Debug, PartialEq)]
enum Pick { Audio(CarriedAudio), Subtitle { id: i64, ordinal: i32 } }
#[derive(Clone, Debug, PartialEq)]
pub struct PlexTrackChoice {
    playback: PlaybackIdentity,
    worker: WorkerTicket,
    intent: TrackIntentTicket,
    sid: ServerId,
    rk: String,
    part: i64,
    pick: Pick,
}
#[derive(Clone, Debug, PartialEq)]
pub struct PlexTrackRow {
    pub kind: TrackKind,
    pub label: String,
    pub language: String,
    pub codec: String,
    pub choice: Option<PlexTrackChoice>,
    id: i64,
    index: i64,
    external: bool,
    key: String,
}
#[derive(Clone, Debug, PartialEq)]
pub struct PlexTracks {
    playback: PlaybackIdentity,
    worker: WorkerTicket,
    sid: ServerId,
    rk: String,
    part: i64,
    rows: Vec<PlexTrackRow>,
    off: Option<PlexTrackChoice>,
    intent: TrackIntentTicket,
    audio: i64,
    subtitle: i64,
    temporary: (bool, bool),
}
impl PlexTracks {
    /// Passive: the exact installed leaf is the only inventory authority. An
    /// absent Stream array is indistinguishable from empty metadata, so is not
    /// advertised as a complete empty catalog.
    pub fn capture(ps: &PlaybackSession, item: &PlayingItem) -> Option<Self> {
        if external_source(ps).is_some() || !has_url(ps) || cur_part_id(ps) <= 0
            || item.part_id != cur_part_id(ps) || item.sid != cur_sid(ps) || item.rk != cur_rk(ps)
            || item.audio.len() > 64 || item.subs.len() > 64
            || item.audio.is_empty() && item.subs.is_empty() { return None; }
        let playback = playback_identity(ps)?;
        let worker = worker_ticket();
        let intent = track_intent_ticket();
        let direct = direct_temporary_route(ps);
        let make = |pick| PlexTrackChoice { playback: playback.clone(), worker: worker.clone(), intent,
            sid: item.sid, rk: item.rk.clone(), part: cur_part_id(ps), pick };
        let mut ids = std::collections::HashSet::new();
        let mut rows = Vec::new();
        for (kind, streams) in [(TrackKind::Audio, &item.audio), (TrackKind::Subtitle, &item.subs)] {
            let mut indices = std::collections::HashSet::new();
            for (i, stream) in streams.iter().enumerate() {
                if stream.id <= 0 || !ids.insert(stream.id) || (!stream.external && (stream.index < 0 || !indices.insert(stream.index)))
                    || stream.title.len() > 2048 || stream.lang.len() > 2048 || stream.codec.len() > 2048
                    || stream.key.len() > 2048 { return None; }
                let pick = match kind {
                    TrackKind::Audio => Pick::Audio(CarriedAudio::from_stream(stream, metadata::audio_ordinal(streams, i))),
                    TrackKind::Subtitle => Pick::Subtitle { id: stream.id, ordinal: metadata::sub_render_ordinal(streams, i) },
                };
                let eligible = direct && match &pick {
                    Pick::Audio(a) => !stream.external && ps.sub_mode != SubMode::ForcedOnly
                        && audio_track_direct_plays(ps, &a.codec, a.channels, a.sample_rate),
                    Pick::Subtitle { ordinal, .. } => !stream.external && *ordinal >= 0
                        && matches!(stream.codec.to_ascii_lowercase().as_str(),
                            "srt" | "subrip" | "ass" | "ssa" | "webvtt" | "mov_text" | "text" |
                            "hdmv_pgs_subtitle" | "pgs" | "dvd_subtitle" | "dvb_subtitle"),
                };
                rows.push(PlexTrackRow { kind, label: if stream.title.is_empty() { format!("Track {}", i + 1) } else { stream.title.clone() },
                    language: stream.lang.clone(), codec: stream.codec.clone(), choice: eligible.then(|| make(pick)),
                    id: stream.id, index: stream.index, external: stream.external, key: stream.key.clone() });
            }
        }
        Some(Self { playback: playback.clone(), worker: worker.clone(), sid: item.sid, rk: item.rk.clone(), part: cur_part_id(ps), rows, off: direct.then(|| make(Pick::Subtitle { id: 0, ordinal: -1 })),
            intent, audio: cur_audio_sid(ps), subtitle: cur_sub_sid(ps),
            temporary: (ps.temporary_tracks.audio, ps.temporary_tracks.subtitle) })
    }
    pub(super) fn retains_native_selection(&self, ps: &PlaybackSession) -> bool {
        let audio = match &ps.cur_audio {
            Some(selected) => self.rows.iter().any(|row| matches!(&row.choice,
                Some(PlexTrackChoice { pick: Pick::Audio(audio), .. }) if audio == selected)),
            None => !self.rows.iter().any(|row| row.kind == TrackKind::Audio),
        };
        let subtitle = if ps.cur_sub_sid == 0 {
            self.off.is_some() && !ps.cur_sub_sidecar
        } else {
            !ps.cur_sub_sidecar && self.rows.iter().any(|row| matches!(&row.choice,
                Some(PlexTrackChoice { pick: Pick::Subtitle { id, ordinal }, .. })
                    if *id == ps.cur_sub_sid && *ordinal == ps.cur_sub_ordinal))
        };
        audio && subtitle
    }
    pub fn rows(&self) -> &[PlexTrackRow] { &self.rows }
    pub fn subtitle_off(&self) -> Option<PlexTrackChoice> { self.off.clone() }
}
fn direct_temporary_route(ps: &PlaybackSession) -> bool {
    external_source(ps).is_none() && has_url(ps) && !is_transcoding(ps)
        && enhancement_family(ps) == RouteFamily::Direct && !ps.cur_contract.audio.any()
        && !original_recovery_pending()
        && ps.direct_source.as_ref().is_some_and(|source| source.current(ps))
}

pub(super) fn note_temporary_track_refusal() {
    crate::player::log("Temporary tracks cannot be sent to Plex. Choose them manually or start a new playback before changing the server route.");
}
pub(super) fn queue_current_selection(ps: &PlaybackSession) {
    let audio = if ps.temporary_tracks.audio { 0 } else { cur_audio_sid(ps) };
    let subtitle = (!ps.temporary_tracks.subtitle).then(|| cur_sub_sid(ps));
    if audio != 0 || subtitle.is_some() { queue_selection(cur_sid(ps), cur_part_id(ps), audio, subtitle, false); }
}
/// The manual wrapper clears only its own kind; an untouched temporary kind
/// must not be persisted by a coupled server conversion.
pub(super) fn manual_subtitle_allowed(ps: &PlaybackSession, id: i64, drawable: bool, ordinal: i32) -> bool {
    !ps.temporary_tracks.audio || (direct_temporary_route(ps) && (id == 0 || drawable && ordinal >= 0))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlexTrackApplyError { Inactive, Busy, Stale, NotSelectable, Expired }
#[derive(Debug)]
pub struct PlexTrackRequest { choice: PlexTrackChoice, revision: u64, serial: u64, worker: WorkerTicket }

pub fn apply_plex_track_request(ps: &mut PlaybackSession, choice: &PlexTrackChoice, deadline: Deadline,
    main: &MainThread) -> Result<Option<PlexTrackRequest>, PlexTrackApplyError> {
    use PlexTrackApplyError as E;
    if deadline.on_main(main).remaining().is_zero() { return Err(E::Expired); }
    if playback_identity(ps).as_ref() != Some(&choice.playback) { return Err(E::Inactive); }
    if cur_sid(ps) != choice.sid || cur_rk(ps) != choice.rk || cur_part_id(ps) != choice.part
        || worker_ticket() != choice.worker || !track_intent_is_current(choice.intent) { return Err(E::Stale); }
    {
        let c = PLAYER_CONTROL.lock().unwrap_or_else(|e| e.into_inner());
        if c.phase != ControlPhase::Stable || c.pending_user.is_some() || c.flight.is_some() { return Err(E::Busy); }
    }
    if !direct_temporary_route(ps) { return Err(E::NotSelectable); }
    if let Pick::Audio(a) = &choice.pick {
        if ps.sub_mode == SubMode::ForcedOnly || !audio_track_direct_plays(ps, &a.codec, a.channels, a.sample_rate) { return Err(E::NotSelectable); }
    }
    // Credential and deadline are observed again before the first accepted mutation.
    if !direct_temporary_route(ps) { return Err(E::Stale); }
    if deadline.on_main(main).remaining().is_zero() { return Err(E::Expired); }
    let _edit = begin_user_contract_boundary();
    // A parked remembered write must not later persist an equal-valued temporary pick.
    ps.write_back = None;
    match &choice.pick {
        Pick::Subtitle { id, ordinal } => {
            ps.temporary_tracks.subtitle = true;
            commit_subtitle_selection_inner(ps, *ordinal, *id, *ordinal >= 0, false);
            crate::player::sidecar::deselect();
            Ok(None)
        }
        Pick::Audio(a) => {
            let changed = ps.cur_audio.as_ref() != Some(a);
            ps.temporary_tracks.audio = true;
            ps.cur_audio = Some(a.clone());
            ps.selection_known = true;
            if let Some(candidate) = ps.auto_original.as_mut() { candidate.retarget_audio(a, true); }
            if !changed { commit_in_place_route_projection(ps, false); return Ok(None); }
            crate::player::request_audio_track(ps, a.ordinal, &a.codec);
            crate::player::note_audio_pick();
            drop(_edit);
            let c = PLAYER_CONTROL.lock().unwrap_or_else(|e| e.into_inner());
            ps.temporary_tracks.pending = Some(PendingTrackEffect { revision: c.desired_revision, serial: next_generation(c.next_action), deadline });
            Ok(Some(PlexTrackRequest { choice: choice.clone(), revision: c.desired_revision,
                serial: next_generation(c.next_action), worker: worker_ticket_of(&c) }))
        }
    }
}

pub fn plex_track_request_status(ps: &PlaybackSession, request: &PlexTrackRequest) -> ExternalTrackRequestStatus {
    use ExternalTrackRequestStatus as S;
    if playback_identity(ps).as_ref() != Some(&request.choice.playback)
        || !matches!(&request.choice.pick, Pick::Audio(a) if ps.cur_audio.as_ref() == Some(a)) { return S::Superseded; }
    let c = PLAYER_CONTROL.lock().unwrap_or_else(|e| e.into_inner());
    if c.desired_revision != request.revision { return S::Superseded; }
    if c.phase == ControlPhase::Stable && c.pending_user == Some(UserRouteIntent::NativeAudioReload)
        && next_generation(c.next_action) == request.serial {
        return if worker_ticket_of(&c) == request.worker { S::Queued } else { S::Superseded };
    }
    if !matches!(c.claimed_user, Some((serial, UserRouteIntent::NativeAudioReload, _)) if serial == request.serial) { return S::Superseded; }
    match c.phase {
        ControlPhase::Applying(serial) | ControlPhase::Preparing(serial) | ControlPhase::Prepared(serial)
        | ControlPhase::Starting(serial, _) | ControlPhase::Completing(serial) if serial == request.serial => S::Pending,
        ControlPhase::Failed(serial) if serial == request.serial => S::Failed,
        ControlPhase::Stable if c.applied_revision == request.revision && matches!(c.last_start_result,
            Some((attempt, RouteStartResult::Started)) if attempt.serial == request.serial) => S::Started,
        ControlPhase::Stable => S::Failed,
        _ => S::Superseded,
    }
}
pub fn cancel_plex_track_request(ps: &mut PlaybackSession, request: &PlexTrackRequest) -> bool {
    if playback_identity(ps).as_ref() != Some(&request.choice.playback) { return false; }
    if cancel_queued_temporary(ps, request.revision) { return true; }
    let mut c = PLAYER_CONTROL.lock().unwrap_or_else(|e| e.into_inner());
    if c.desired_revision != request.revision || !matches!(c.claimed_user,
        Some((serial, UserRouteIntent::NativeAudioReload, _)) if serial == request.serial) { return false; }
    // Do not restore over a dispatched Load. Revoke only its still-unperformed
    // effects; the captured worker guard observes this same intent revision.
    advance_user_contract_locked(&mut c);
    true
}
fn cancel_queued_temporary(ps: &mut PlaybackSession, revision: u64) -> bool {
    let restore = {
        let mut c = PLAYER_CONTROL.lock().unwrap_or_else(|e| e.into_inner());
        if c.phase != ControlPhase::Stable || c.desired_revision != revision
            || c.pending_user != Some(UserRouteIntent::NativeAudioReload) { return false; }
        c.pending_user = None; c.displaced_pick = false;
        advance_user_contract_locked(&mut c);
        c.applied_projection.clone()
    };
    if let Some(restore) = restore { install_route_projection(ps, &restore); }
    ps.temporary_tracks.pending = None;
    crate::player::cancel_audio_pick();
    true
}
/// Called by the real pump before claiming any action, using its MainThread.
pub fn expire_plex_track_request(ps: &mut PlaybackSession, main: &MainThread) {
    if let Some(effect) = ps.temporary_tracks.pending {
        if effect.deadline.on_main(main).remaining().is_zero() { cancel_queued_temporary(ps, effect.revision); }
    }
}
/// Rechecked after claim immediately before the native reload effect. It never
/// borrows a fresh allowance, and a later manual intent does not inherit it.
pub fn plex_track_reload_allowed(ps: &PlaybackSession, main: &MainThread) -> bool {
    ps.temporary_tracks.pending.is_none_or(|effect|
        track_intent_is_current(TrackIntentTicket(effect.revision)) && !effect.deadline.on_main(main).remaining().is_zero())
}

pub(super) fn supersede_pending_plex_track(ps: &mut PlaybackSession) {
    if let Some(effect) = ps.temporary_tracks.pending {
        cancel_queued_temporary(ps, effect.revision);
    }
    ps.temporary_tracks.pending = None;
}

pub fn has_temporary_track_selection(ps: &PlaybackSession) -> bool { ps.temporary_tracks.any() }

#[derive(Clone, Copy, Debug)]
pub struct PlexTrackLoadGuard { deadline: Deadline, intent: TrackIntentTicket, worker_epochs: Option<(u64, u64, u64, u64)>, source: Option<plx_plex::plex::DirectSourceWitness> }
// The intent is minted once for this immutable guard. Identity equality must
// not sample a clock; duplicate queued starts retain the first complete guard.
impl PartialEq for PlexTrackLoadGuard { fn eq(&self, other: &Self) -> bool { self.intent == other.intent } }
impl Eq for PlexTrackLoadGuard {}
impl PlexTrackLoadGuard {
    pub(super) fn retained_retry(deadline: Deadline, intent: TrackIntentTicket,
        source: plx_plex::plex::DirectSourceWitness) -> Self {
        Self { deadline, intent, worker_epochs: None, source: Some(source) }
    }
    pub fn allows_main(&self, main: &MainThread) -> bool {
        self.source.is_none_or(|source| source.current())
            && track_intent_is_current(self.intent) && !self.deadline.on_main(main).remaining().is_zero()
    }
    pub fn allows(&self, worker: &plx_base::task::OffFrame) -> bool {
        if self.source.is_some_and(|source| !source.current()) { return false; }
        let c = PLAYER_CONTROL.lock().unwrap_or_else(|e| e.into_inner());
        c.desired_revision == self.intent.0
            && self.worker_epochs.is_none_or(|epochs| epochs == (c.active.epoch, c.engine_epoch, c.media_epoch, c.applied_revision))
            && !self.deadline.expired(worker)
    }
    /// Bind only after this request's own teardown and native attempt claim.
    /// A later suspend still changes these existing worker epochs.
    pub fn bind_worker(mut self, attempt: RouteStartAttempt) -> Option<Self> {
        let c = PLAYER_CONTROL.lock().unwrap_or_else(|e| e.into_inner());
        if c.desired_revision != self.intent.0 || c.phase != ControlPhase::Starting(attempt.serial, attempt.attempt) { return None; }
        self.worker_epochs = Some((c.active.epoch, c.engine_epoch, c.media_epoch, c.applied_revision));
        Some(self)
    }
}
pub fn plex_track_load_guard(ps: &PlaybackSession) -> Option<PlexTrackLoadGuard> {
    ps.temporary_tracks.pending
        .map(|effect| PlexTrackLoadGuard { deadline: effect.deadline, intent: TrackIntentTicket(effect.revision), worker_epochs: None, source: ps.direct_source.as_ref().map(|source| source.witness) })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    // Hold the real queue in its already-running state while testing manual
    // write admission. Nothing drains it and therefore no worker/HTTP starts.
    struct HeldSelectionQueue;
    impl HeldSelectionQueue {
        fn new() -> Self {
            let mut queue = selection_queue();
            assert!(!queue.worker_running && queue.pending.is_empty());
            queue.worker_running = true;
            Self
        }
    }
    impl Drop for HeldSelectionQueue {
        fn drop(&mut self) {
            let mut queue = selection_queue();
            queue.pending.clear();
            queue.worker_running = false;
        }
    }
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            reset_player_control_for_test(&PlaybackSession::IDLE);
            crate::player::reset_audio_track();
            crate::player::reset_subtitle();
            plx_plex::plex::reset_servers_for_test();
        }
    }
    fn fixture() -> (PlaybackSession, PlayingItem, Reset) {
        let mut item = PlayingItem::with_subs(vec![metadata::Stream {
            id: 303, index: 3, codec: "srt".into(), ..Default::default()
        }]);
        item.sid = plx_plex::plex::register_for_test("direct-retry-route", "127.0.0.1", 1, "test-token", "test-client");
        item.rk = "bound-leaf".into();
        item.part_id = 77;
        item.audio = vec![
            metadata::Stream { id: 202, index: 2, codec: "aac".into(), channels: 2, sampling_rate: 48000, ..Default::default() },
            metadata::Stream { id: 201, index: 1, codec: "aac".into(), channels: 2, sampling_rate: 48000, ..Default::default() },
        ];
        let mut ps = PlaybackSession::IDLE;
        install_plex_track_fixture_for_test(&mut ps, item.sid, &item.rk, 77, CarriedAudio::from_stream(&item.audio[1], 0));
        assert!(qualify_plex_direct_retry_fixture_for_test(&mut ps, plx_plex::plex::client_for(item.sid).unwrap()));
        reset_player_control_for_test(&ps);
        (ps, item, Reset)
    }
    fn main() -> MainThread { unsafe { MainThread::assume() } }
    fn deadline() -> Deadline { Deadline::after(&plx_base::task::OffFrame::for_test(), Duration::from_secs(60)) }
    fn audio(ps: &PlaybackSession, item: &PlayingItem) -> PlexTrackChoice {
        let tracks = PlexTracks::capture(ps, item).expect("complete bound fixture");
        tracks.rows()[0].choice.clone().expect("fixture AAC is directly playable")
    }
    fn admit(ps: &mut PlaybackSession, item: &PlayingItem) -> PlexTrackRequest {
        apply_plex_track_request(ps, &audio(ps, item), deadline(), &main()).unwrap().expect("different audio queues a native reload")
    }

    #[test]
    fn plex_tracks_bind_part_and_container_ordinals_without_guessing_empty_metadata() {
        let _serial = plx_base::testlock::serial();
        let (ps, mut item, _reset) = fixture();
        let tracks = PlexTracks::capture(&ps, &item).unwrap();
        assert!(matches!(&tracks.rows()[0].choice.as_ref().unwrap().pick, Pick::Audio(a) if a.sid == 202 && a.ordinal == 1));
        item.part_id = 88;
        assert!(PlexTracks::capture(&ps, &item).is_none());
        item.part_id = 77;
        item.audio.clear(); item.subs.clear();
        assert!(PlexTracks::capture(&ps, &item).is_none());
    }

    #[test]
    fn unsupported_external_rows_do_not_hide_the_bound_native_audio() {
        let _serial = plx_base::testlock::serial();
        let (ps, mut item, _reset) = fixture();
        item.subs.push(metadata::Stream { id: 304, index: -1, codec: "srt".into(), external: true,
            key: "/library/streams/304".into(), ..Default::default() });
        let tracks = PlexTracks::capture(&ps, &item).unwrap();
        assert!(tracks.rows()[0].choice.is_some());
        assert!(tracks.rows().last().unwrap().choice.is_none());
        assert!(tracks.subtitle_off().is_some());
    }

    #[test]
    fn expired_plex_selection_is_atomic_and_does_not_cross_the_intent_boundary() {
        let _serial = plx_base::testlock::serial();
        let (mut ps, item, _reset) = fixture();
        let choice = audio(&ps, &item);
        let before = PlexTracks::capture(&ps, &item);
        let expired = Deadline::after(&plx_base::task::OffFrame::for_test(), Duration::ZERO);
        assert_eq!(apply_plex_track_request(&mut ps, &choice, expired, &main()).unwrap_err(), PlexTrackApplyError::Expired);
        assert_eq!(PlexTracks::capture(&ps, &item), before);
        assert!(!pending_user_route_intent(UserRouteIntent::NativeAudioReload));
    }

    #[test]
    fn temporary_audio_and_opposite_kind_manual_selection_preserve_provenance() {
        let _serial = plx_base::testlock::serial();
        let _queued = HeldSelectionQueue::new();
        let (mut ps, item, _reset) = fixture();
        let request = admit(&mut ps, &item);
        let action = claim_route_action().unwrap();
        assert!(finish_route_action(&mut ps, &action, RouteApplyResult::Prepared));
        let attempt = claim_route_start_attempt(begin_route_start().unwrap()).unwrap();
        assert!(settle_route_start(&mut ps, attempt, RouteStartResult::Started));
        assert_eq!(plex_track_request_status(&ps, &request), ExternalTrackRequestStatus::Started);
        assert!(ps.temporary_tracks.audio && ps.temporary_tracks.pending.is_none());
        assert!(commit_subtitle_pick(&mut ps, 0, 303, true, None));
        assert!(ps.temporary_tracks.audio && !ps.temporary_tracks.subtitle);
        {
            let queue = selection_queue();
            let write = queue.pending.back().expect("manual subtitle write");
            assert_eq!((write.aud, write.sub), (0, Some(303)), "temporary audio is omitted");
        }
        // Only the ordinary manual write is queued; the held queue starts no IO.
        let picked = ps.cur_audio.clone().unwrap();
        let prior = track_intent_ticket();
        assert!(commit_audio_pick(&mut ps, picked, None));
        assert!(!ps.temporary_tracks.any());
        assert!(!track_intent_is_current(prior));
        assert!(!pending_user_route_intent(UserRouteIntent::NativeAudioReload));
    }

    #[test]
    fn temporary_selection_survives_rollback_and_retry_refuses_before_server_effects() {
        let _serial = plx_base::testlock::serial();
        let (mut ps, item, _reset) = fixture();
        let off = PlexTracks::capture(&ps, &item).unwrap().subtitle_off().unwrap();
        assert!(apply_plex_track_request(&mut ps, &off, deadline(), &main()).unwrap().is_none());
        let snapshot = route_projection(&ps);
        ps.temporary_tracks = TemporaryTracks::NONE;
        install_route_projection(&mut ps, &snapshot);
        assert!(ps.temporary_tracks.subtitle);
        let retry = current_retry_context(&ps, 7_000_000_000);
        let mut next = PlaybackSession::IDLE;
        begin_track_selection(&mut next, Some(retry));
        assert!(next.temporary_tracks.subtitle);
        let store = plx_data::stores::metadata::MetadataStore::default();
        let mut env = ResolveEnv::snapshot(&ps, store.view(), item.sid, &item.rk);
        apply_retry(&mut env, retry);
        let plan = build_stream(&plx_base::task::OffFrame::for_test(), &item.rk, "/library/parts/77/file.mkv", "h264", "aac", &env);
        assert_eq!(plan.verdict, Some(PlayVerdict::TemporaryTracksNeedDirectPlay));
        assert!(plan.url.is_empty());
        assert_eq!(retry.resume_ns, 7_000_000_000);
        begin_track_selection(&mut next, None);
        assert!(!next.temporary_tracks.any());
    }

    #[test]
    fn queued_cancellation_restores_the_previous_audio_and_does_not_steal_a_newer_intent() {
        let _serial = plx_base::testlock::serial();
        let (mut ps, item, _reset) = fixture();
        let request = admit(&mut ps, &item);
        assert!(cancel_plex_track_request(&mut ps, &request));
        assert_eq!(cur_audio_sid(&ps), 201);
        assert!(!ps.temporary_tracks.any());
        let later = track_intent_ticket();
        assert!(!cancel_plex_track_request(&mut ps, &request));
        assert!(track_intent_is_current(later));
    }

    #[test]
    fn claimed_policy_cancellation_revokes_the_existing_media_worker_guard() {
        let _serial = plx_base::testlock::serial();
        let (mut ps, item, _reset) = fixture();
        let request = admit(&mut ps, &item);
        let action = claim_route_action().unwrap();
        assert!(finish_route_action(&mut ps, &action, RouteApplyResult::Prepared));
        let guard = plex_track_load_guard(&ps).unwrap();
        let worker = plx_base::task::OffFrame::for_test();
        assert!(guard.allows(&worker));
        assert!(cancel_plex_track_request(&mut ps, &request));
        assert!(!guard.allows(&worker));
        assert!(!plex_track_reload_allowed(&ps, &main()));
        let attempt = claim_route_start_attempt(begin_route_start().unwrap()).unwrap();
        assert!(settle_route_start(&mut ps, attempt, RouteStartResult::StartFailed));
        assert!(ps.temporary_tracks.pending.is_none());
        assert!(ps.temporary_tracks.audio, "failure must not erase no-PUT provenance");
    }

    #[test]
    fn rejected_manual_conversion_keeps_the_pending_request_and_intent() {
        let _serial = plx_base::testlock::serial();
        let (mut ps, item, _reset) = fixture();
        let request = admit(&mut ps, &item);
        let before = track_intent_ticket();
        assert!(!commit_subtitle_pick(&mut ps, -1, 999, false, None));
        assert!(track_intent_is_current(before));
        assert_eq!(plex_track_request_status(&ps, &request), ExternalTrackRequestStatus::Queued);
        assert!(cancel_plex_track_request(&mut ps, &request));
    }
    #[test]
    fn a_deferred_start_keeps_its_original_guard_after_manual_same_target_intent() {
        let _serial = plx_base::testlock::serial();
        let _queued = HeldSelectionQueue::new();
        let (mut ps, item, _reset) = fixture();
        let _request = admit(&mut ps, &item);
        let action = claim_route_action().unwrap();
        assert!(finish_route_action(&mut ps, &action, RouteApplyResult::Prepared));
        let start = begin_route_start().unwrap();
        let queued = crate::player::release::QueuedStart { route_start: start, track_guard: plex_track_load_guard(&ps) };
        let mut release = crate::player::release::NativeRelease::new();
        let mut effects = Vec::new();
        release.step(&crate::player::release::ReleaseEvent::UnloadHandedOff { epoch: 7, at: std::time::Instant::now() }, &mut effects);
        release.step(&crate::player::release::ReleaseEvent::StartRequested(queued), &mut effects);
        assert!(effects.is_empty());
        // The UI accepts an explicit Off even though Off was already displayed.
        assert!(commit_subtitle_pick(&mut ps, -1, 0, false, None));
        release.step(&crate::player::release::ReleaseEvent::UnloadReturned { epoch: 7 }, &mut effects);
        let delayed = effects.iter().find_map(|effect| match effect {
            crate::player::release::ReleaseEffect::Start(start) => Some(start), _ => None,
        }).expect("the real release machine returns the original request");
        assert!(!delayed.track_guard.unwrap().allows_main(&main()));
        assert!(!delayed.track_guard.unwrap().allows(&plx_base::task::OffFrame::for_test()));
    }

    #[test]
    fn a_later_suspend_revokes_the_post_teardown_media_worker_epoch() {
        let _serial = plx_base::testlock::serial();
        let (mut ps, item, _reset) = fixture();
        let _request = admit(&mut ps, &item);
        let action = claim_route_action().unwrap();
        assert!(finish_route_action(&mut ps, &action, RouteApplyResult::Prepared));
        let attempt = claim_route_start_attempt(begin_route_start().unwrap()).unwrap();
        let guard = plex_track_load_guard(&ps).unwrap().bind_worker(attempt).unwrap();
        let worker = plx_base::task::OffFrame::for_test();
        assert!(guard.allows(&worker));
        let intent = track_intent_ticket();
        begin_engine_teardown(true);
        assert!(track_intent_is_current(intent), "suspend preserves desired intent");
        assert!(!guard.allows(&worker), "the existing engine epoch still revokes its Load");
    }

    #[test]
    fn synchronous_native_abort_retires_only_its_own_deadline() {
        let _serial = plx_base::testlock::serial();
        let (mut ps, item, _reset) = fixture();
        let _request = admit(&mut ps, &item);
        let action = claim_route_action().unwrap();
        assert!(finish_route_action(&mut ps, &action, RouteApplyResult::Prepared));
        assert!(abort_route_start(begin_route_start().unwrap(), RouteStartResult::StartFailed));
        retire_terminal_plex_track_request(&mut ps);
        assert!(ps.temporary_tracks.pending.is_none());
        assert!(ps.temporary_tracks.audio);
    }

}

/// Retire only a terminal attempt's deadline, never its temporary selection.
pub fn retire_terminal_plex_track_request(ps: &mut PlaybackSession) {
    let Some(effect) = ps.temporary_tracks.pending else { return; };
    let mut c = PLAYER_CONTROL.lock().unwrap_or_else(|e| e.into_inner());
    let terminal = match (c.phase, c.claimed_user) {
        (ControlPhase::Failed(a), Some((b, UserRouteIntent::NativeAudioReload, _))) => a == b && a == effect.serial,
        (ControlPhase::Stable, Some((serial, UserRouteIntent::NativeAudioReload, _))) =>
            serial == effect.serial && c.last_start_result.is_some_and(|(attempt, _)| attempt.serial == serial),
        _ => false,
    };
    if terminal {
        ps.temporary_tracks.pending = None;
        if let Some(applied) = c.applied_projection.as_mut() {
            if applied.temporary_tracks.pending.is_some_and(|pending| pending.serial == effect.serial) { applied.temporary_tracks.pending = None; }
        }
    }
}
