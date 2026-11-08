//! One cancellable HLS preparation worker; route publication stays on the App thread.
use super::{App, Job, Runtime};
use super::commands::media_tracks::hls::Remote;
use plx_appkit::track_menu::{TrackCommit, TrackCommitFailure as Failure, TrackCommitOutcome as Outcome};
use plx_media::route::{self, HlsTrackRequest, PreparedHlsTrack};
use plx_screens::registry::{PlayerReq, TrackCommitRequest};
use std::sync::{Arc, atomic::{AtomicBool, Ordering}};

struct Active<T> { id: u64, cancelled: Arc<AtomicBool>, value: Option<T> }
struct Slots<T> { running: Option<Active<T>>, queued: Option<T>, next: u64 }
impl<T> Default for Slots<T> {
    fn default() -> Self { Self { running: None, queued: None, next: 0 } }
}
impl<T> Slots<T> {
    // Cancellation retires authority immediately, but retains the worker slot
    // until its completion. A rapid sequence never creates unbounded workers.
    fn replace(&mut self, value: T) -> Vec<T> {
        let mut retired = Vec::new();
        if let Some(active) = &mut self.running {
            active.cancelled.store(true, Ordering::Release);
            retired.extend(active.value.take());
        }
        retired.extend(self.queued.replace(value));
        retired
    }
    fn start(&mut self) -> Option<(u64, Arc<AtomicBool>)> {
        if self.running.is_some() { return None; }
        let value = self.queued.take()?;
        self.next = self.next.wrapping_add(1);
        let cancelled = Arc::new(AtomicBool::new(false));
        self.running = Some(Active { id: self.next, cancelled: cancelled.clone(), value: Some(value) });
        Some((self.next, cancelled))
    }
    fn finish(&mut self, id: u64) -> Option<T> {
        if self.running.as_ref().is_none_or(|active| active.id != id) { return None; }
        self.running.take()?.value
    }
    fn retire_if(&mut self, mut invalid: impl FnMut(&T) -> bool) -> Vec<T> {
        let mut retired = Vec::new();
        if let Some(active) = &mut self.running {
            if active.value.as_ref().is_some_and(&mut invalid) {
                active.cancelled.store(true, Ordering::Release);
                retired.extend(active.value.take());
            }
        }
        if self.queued.as_ref().is_some_and(invalid) { retired.extend(self.queued.take()); }
        retired
    }
}
impl<T> Drop for Slots<T> {
    fn drop(&mut self) {
        if let Some(active) = &self.running { active.cancelled.store(true, Ordering::Release); }
    }
}
enum Recipient { Ui(Option<TrackCommitRequest>), Remote(Remote) }
struct Pending { route: HlsTrackRequest, reply: Recipient }
#[derive(Default)]
pub(super) struct State { slots: Slots<Pending> }
pub(super) struct Completion { id: u64, result: Result<PreparedHlsTrack, &'static str> }

fn failure(error: route::ExternalTrackApplyError) -> Failure {
    match error {
        route::ExternalTrackApplyError::Busy => Failure::Busy,
        route::ExternalTrackApplyError::InvalidChoice(route::ExternalTrackError::StaleSession) => Failure::StalePlayback,
        _ => Failure::Unavailable,
    }
}
pub(super) fn source_available(runtime: &Runtime) -> bool {
    !runtime.playing.as_ref().is_some_and(|playing| playing.requires_server_lease)
        || runtime.active_lease.as_ref().is_some_and(super::HeldLease::is_playback)
}
fn gate(runtime: &Runtime, app: &App) -> Result<(), Failure> {
    if !source_available(runtime) { return Err(Failure::Unavailable); }
    if !super::playback_allowed(app) || !runtime.track_selection_kiosk()
        .is_ok_and(super::super::ott_kiosk_policy::remote_track_selection_allowed)
    { return Err(Failure::Unavailable); }
    use plx_media::player::PlaybackState;
    match plx_media::player::state(&app.player.session) {
        PlaybackState::Idle | PlaybackState::Error => Err(Failure::Unavailable),
        PlaybackState::Resolving | PlaybackState::Connecting => Err(Failure::Busy),
        _ => Ok(()),
    }
}
fn reply(app: &mut App, request: Option<TrackCommitRequest>, outcome: Outcome) {
    if let Some(request) = request {
        super::super::bridge::finish_track_commit(&mut app.pages, &request, outcome);
    } else if matches!(outcome, Outcome::Rejected(_)) {
        plx_base::eventlog::log("HLS track choice was not applied");
    }
}
fn retire(runtime: &mut Runtime, app: &mut App, pending: Vec<Pending>, why: Failure,
    now_ms: Option<u64>)
{
    for pending in pending {
        match pending.reply {
            Recipient::Ui(request) => reply(app, request, Outcome::Rejected(why)),
            Recipient::Remote(remote) => match remote.current(runtime, app, &pending.route, now_ms) {
                Err(error) => remote.reject(runtime, error),
                Ok(()) => remote.finish(runtime, Outcome::Rejected(why)),
            },
        }
    }
}
fn remote_now(runtime: &Runtime, app: &App) -> Option<u64> {
    plx_base::task::time::ClockSample::capture_main(app.adapters.player.mt()).ok()
        .zip(runtime.clock_anchor)
        .map(|(sample, anchor)| sample.steady_from(anchor).unix_ms())
}
fn finish(runtime: &mut Runtime, app: &mut App, recipient: Recipient, outcome: Outcome) {
    match recipient {
        Recipient::Ui(request) => reply(app, request, outcome),
        Recipient::Remote(remote) => remote.finish(runtime, outcome),
    }
}
// Reads only the addressed remote owner. It is also called before the playback
// pump, where a failed clock read must retire authority instead of renewing TTL.
pub(super) fn poll_remote(runtime: &mut Runtime, app: &mut App, now_ms: Option<u64>) {
    let mut state = std::mem::take(&mut runtime.hls_tracks);
    let retired = state.slots.retire_if(|pending| match &pending.reply {
        Recipient::Remote(remote) => remote.current(runtime, app, &pending.route, now_ms).is_err(),
        Recipient::Ui(_) => false,
    });
    runtime.hls_tracks = state;
    retire(runtime, app, retired, Failure::StalePlayback, now_ms);
}
pub(super) fn has_owned_pending(runtime: &Runtime) -> bool {
    let slots = &runtime.hls_tracks.slots;
    slots.queued.is_some() || slots.running.as_ref().is_some_and(|active| active.value.is_some())
}
pub(super) fn has_remote_pending(runtime: &Runtime) -> bool {
    let slots = &runtime.hls_tracks.slots;
    slots.queued.iter().chain(slots.running.iter().filter_map(|active| active.value.as_ref()))
        .any(|pending| matches!(&pending.reply, Recipient::Remote(_)))
}
pub(super) fn enqueue_remote(runtime: &mut Runtime, app: &mut App,
    route: HlsTrackRequest, remote: Remote)
{
    let retired = runtime.hls_tracks.slots.replace(Pending { route, reply: Recipient::Remote(remote) });
    let now = remote_now(runtime, app);
    retire(runtime, app, retired, Failure::StalePlayback, now);
    // The ordinary HLS tick starts the single worker after checking the owner.
}


fn perform(app: &mut App, choice: route::HlsTrackChoice, reply_to: Option<TrackCommitRequest>) {
    let Some(mut runtime) = app.ott.take() else {
        reply(app, reply_to, Outcome::Rejected(Failure::Unavailable)); return;
    };
    let request = gate(&runtime, app).and_then(|()|
        route::begin_hls_track_request(&mut app.player.session, &choice).map_err(failure));
    match request {
        Ok(route) => {
            let retired = runtime.hls_tracks.slots.replace(Pending { route, reply: Recipient::Ui(reply_to) });
            let now = remote_now(&runtime, app);
            retire(&mut runtime, app, retired, Failure::StalePlayback, now);
            pump(&mut runtime, app);
        }
        Err(why) => reply(app, reply_to, Outcome::Rejected(why)),
    }
    app.ott = Some(runtime);
}
pub(super) fn trusted_commit(app: &mut App, commit: TrackCommit) -> Result<(), TrackCommit> {
    match commit {
        TrackCommit::Hls(choice) => { perform(app, choice, None); Ok(()) }
        other => Err(other),
    }
}
pub(super) fn player_request(app: &mut App, request: PlayerReq) -> Option<PlayerReq> {
    match request {
        PlayerReq::CommitTrack(TrackCommit::Hls(choice)) => { perform(app, choice, None); None }
        PlayerReq::CommitTrackAddressed(request) if matches!(&request.commit, TrackCommit::Hls(_)) => {
            use super::super::bridge::{self, TrackCommitAdmission};
            match bridge::begin_track_commit(&mut app.pages, &app.player.session, &request) {
                TrackCommitAdmission::Ignore => {}
                TrackCommitAdmission::Reject(why) => reply(app, Some(request), Outcome::Rejected(why)),
                TrackCommitAdmission::Ready => {
                    let TrackCommit::Hls(choice) = request.commit.clone() else { unreachable!() };
                    perform(app, choice, Some(request));
                }
            }
            None
        }
        other => Some(other),
    }
}
fn pump(runtime: &mut Runtime, app: &mut App) {
    if has_remote_pending(runtime) {
        let now = remote_now(runtime, app);
        poll_remote(runtime, app, now);
    }
    let Some((id, cancelled)) = runtime.hls_tracks.slots.start() else { return; };
    let request = runtime.hls_tracks.slots.running.as_ref().unwrap().value.as_ref().unwrap().route.clone();
    let sender = runtime.tx.clone();
    let spawned = plx_base::task::spawn_off_frame("hls-tracks", move |worker| {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            if cancelled.load(Ordering::Acquire) { return Err("HLS preparation cancelled"); }
            request.prepare(worker, cancelled)
        })).unwrap_or(Err("HLS preparation failed"));
        let _ = sender.send(Job::HlsTrack(Completion { id, result }));
        plx_machine::idle::invalidate();
    });
    if !spawned {
        if let Some(pending) = runtime.hls_tracks.slots.finish(id) {
            finish(runtime, app, pending.reply, Outcome::Rejected(Failure::Busy));
        }
    }
}
fn owner_pending(pages: &plx_ui::dispatch::Dispatcher<super::super::bridge::AppHost>,
    reply: Option<&TrackCommitRequest>) -> bool
{
    reply.is_none_or(|request| super::super::bridge::track_commit_is_pending(pages, request))
}
fn apply_owned_ready(pages: &plx_ui::dispatch::Dispatcher<super::super::bridge::AppHost>,
    ps: &mut route::PlaybackSession, pending: &Pending,
    result: Result<PreparedHlsTrack, &'static str>) -> Outcome
{
    let Recipient::Ui(request) = &pending.reply else {
        return Outcome::Rejected(Failure::StalePlayback);
    };
    if !owner_pending(pages, request.as_ref()) {
        return Outcome::Rejected(Failure::StalePlayback);
    }
    apply_ready(ps, &pending.route, result)
}
// Kept separate so tests use the same real route admission/publication path
// without constructing an App, fetching media or creating a background thread.
fn apply_ready(ps: &mut route::PlaybackSession, request: &HlsTrackRequest,
    result: Result<PreparedHlsTrack, &'static str>) -> Outcome
{
    if !route::hls_track_request_is_current(ps, request) { return Outcome::Rejected(Failure::StalePlayback); }
    match result {
        Ok(prepared) => match route::complete_hls_track_request(ps, request, prepared) {
            Ok(true) => Outcome::Admitted, Ok(false) => Outcome::Unchanged,
            Err(error) => Outcome::Rejected(failure(error)),
        },
        Err(_) => Outcome::Rejected(Failure::Unavailable),
    }
}
fn completed_at(runtime: &mut Runtime, app: &mut App, completion: Completion, now_ms: Option<u64>) {
    if let Some(pending) = runtime.hls_tracks.slots.finish(completion.id) {
        match &pending.reply {
            Recipient::Remote(remote) => {
                // This is the last gate before publication, not a later polling
                // check: a successful preparation may arrive at its deadline.
                match remote.current(runtime, app, &pending.route, now_ms) {
                    Err(error) => {
                        let Recipient::Remote(remote) = pending.reply else { unreachable!() };
                        remote.reject(runtime, error);
                    }
                    Ok(()) => {
                        let outcome = apply_ready(&mut app.player.session, &pending.route, completion.result);
                        finish(runtime, app, pending.reply, outcome);
                    }
                }
            }
            Recipient::Ui(_) => {
                let outcome = match gate(runtime, app) {
                    Ok(()) => apply_owned_ready(&app.pages, &mut app.player.session, &pending, completion.result),
                    Err(why) => Outcome::Rejected(why),
                };
                finish(runtime, app, pending.reply, outcome);
            }
        }
    }
}
pub(super) fn completed(runtime: &mut Runtime, app: &mut App, completion: Completion) {
    let now = remote_now(runtime, app);
    completed_at(runtime, app, completion, now);
    tick(runtime, app);
}
pub(super) fn tick(runtime: &mut Runtime, app: &mut App) {
    let now = has_remote_pending(runtime).then(|| remote_now(runtime, app)).flatten();
    poll_remote(runtime, app, now);
    let allowed = gate(runtime, app).is_ok();
    let retired = runtime.hls_tracks.slots.retire_if(|pending| match &pending.reply {
        Recipient::Ui(request) => !allowed || !owner_pending(&app.pages, request.as_ref())
            || !route::hls_track_request_is_current(&app.player.session, &pending.route),
        Recipient::Remote(_) => false,
    });
    retire(runtime, app, retired, Failure::StalePlayback, now);
    pump(runtime, app);
}
pub(super) fn shutdown(runtime: &mut Runtime, app: &mut App) {
    let retired = runtime.hls_tracks.slots.retire_if(|_| true);
    let now = remote_now(runtime, app);
    retire(runtime, app, retired, Failure::Unavailable, now);
}

#[cfg(test)]
#[path = "ott_hls_tracks_tests.rs"]
mod tests;

#[cfg(all(test, feature = "hostsim"))]
#[path = "ott_hls_tracks_remote_tests.rs"]
mod remote_tests;
