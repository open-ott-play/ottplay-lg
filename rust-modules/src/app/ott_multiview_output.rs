//! Native composite output owned by an authenticated private service reply.
//!
//! The explicitly paired HTTPS compositor is a trusted manifest/redirect
//! authority, like an operator-configured media service. Generic native HLS
//! does NOT call PrivateOutput::child_url_for_media_worker: signed child lease
//! confinement belongs to the server. No control bearer enters media work.
use super::multiview_connection::{Launcher, NativeClient, NativeLauncher, Retirement};
use super::multiview_service::{Capabilities, NativeExpected, SessionState, StatusResponse};
use super::*;
use plx_base::task::{time::Deadline, OffFrame};
use plx_media::{
    ff::{inspect::MediaInfo, ExternalSource},
    route,
};
use plx_screens::player::input::HudStart;
use std::sync::mpsc::{self, Receiver, TryRecvError};

/// No Debug/Serialize: status has an output HMAC path. The driver produces this
/// only from authenticated server truth, never from a URL or a renderer label.
pub(super) struct ReadyOutput {
    pub connection_generation: u64,
    pub client: Arc<NativeClient>,
    pub capabilities: Capabilities,
    pub status: StatusResponse,
    /// Conservative original lease deadline captured BEFORE the service request.
    /// Neither queuing, inspection nor native Load is allowed to renew it.
    pub lease_deadline: Deadline,
    /// Original user operation deadline; only initial inspection/native Load,
    /// never an active picture's playback lifetime.
    pub admission_deadline: Deadline,
}
impl ReadyOutput {
    fn current(&self, runtime: &Runtime, app: &App) -> bool {
        self.current_with_driver(runtime, app, &runtime.multiview_driver)
    }
    fn current_with_driver(
        &self,
        runtime: &Runtime,
        app: &App,
        driver: &multiview_driver::Driver,
    ) -> bool {
        let Some(output) = self.status.output.as_ref() else {
            return false;
        };
        app.running
            && runtime.loaded
            && !runtime.persistence_uncertain
            && runtime.state_load == StateLoad::Idle
            && playback_allowed(app)
            && runtime
                .clock_sample
                .is_some_and(|sample| !self.lease_deadline.remaining_at_sample(sample).is_zero())
            && self.status.state == SessionState::Ready
            && qualified_geometry(&self.capabilities, &self.status)
            && runtime.multiview_connection.is_current(
                self.connection_generation,
                &self.status.server_epoch,
                &self.client,
            )
            && driver.output_current(
                runtime,
                self.connection_generation,
                &self.status.session_id,
                &self.status.server_epoch,
                &output.output_generation,
            )
            && runtime
                .multiview
                .validate_active_owner(runtime, app)
                .is_ok()
            && native_lifetime_matches(runtime, &self.status.native_expected)
    }
}
fn qualified_geometry(capabilities: &Capabilities, status: &StatusResponse) -> bool {
    capabilities.max_inputs > 0
        && capabilities.output_profiles.iter().any(|profile| {
            profile.canvas == status.geometry.canvas
                && profile.geometry_modes.contains(&status.geometry.mode)
                && profile.max_inputs > 0
                && profile.max_inputs <= capabilities.max_inputs
        })
}

fn native_lifetime_matches(runtime: &Runtime, expected: &NativeExpected) -> bool {
    runtime
        .multiview
        .scene_status(runtime)
        .is_some_and(|scene| lifetime_matches(&scene, expected))
}
fn lifetime_matches(scene: &multiview::SceneStatus, expected: &NativeExpected) -> bool {
    scene.runtime == expected.runtime
        && scene.scene == expected.expected.scene
        && scene.layout_revision == expected.expected.layout_revision
}
fn native_response_matches(runtime: &Runtime, expected: &NativeExpected) -> bool {
    let Some(scene) = runtime.multiview.scene_status(runtime) else {
        return false;
    };
    response_matches(&scene, expected)
}
fn response_matches(scene: &multiview::SceneStatus, expected: &NativeExpected) -> bool {
    lifetime_matches(scene, expected)
        && scene.focus_revision == expected.expected.focus_revision
        && scene.audio_revision == expected.expected.audio_revision
        && scene.slots.iter().any(|slot| {
            slot.slot.index() == usize::from(expected.slot_id)
                && slot.slot_generation == expected.slot_generation
                && slot.source_revision == expected.source_revision
        })
}

pub(super) trait Inspector: Send + Sync + 'static {
    fn inspect(
        &self,
        worker: &OffFrame,
        source: &ExternalSource,
        deadline: Deadline,
        cancelled: Arc<AtomicBool>,
    ) -> Result<MediaInfo, &'static str>;
}
pub(super) struct NativeInspector;
impl Inspector for NativeInspector {
    fn inspect(
        &self,
        worker: &OffFrame,
        source: &ExternalSource,
        deadline: Deadline,
        cancelled: Arc<AtomicBool>,
    ) -> Result<MediaInfo, &'static str> {
        if cancelled.load(Ordering::Acquire) || deadline.expired(worker) {
            return Err("Composite output inspection expired");
        }
        // Existing inspector has bounded per-leg IO and observes this token.
        // Owner tick cancels at the ORIGINAL lease deadline, including queue time;
        // this adapter never claims the inspector accepts a per-read lease hook.
        let result = plx_media::ff::inspect::source(source, Arc::clone(&cancelled));
        if cancelled.load(Ordering::Acquire) || deadline.expired(worker) {
            return Err("Composite output inspection expired");
        }
        result.map_err(|_| "Composite output inspection failed")
    }
}
struct Inspected {
    generation: u64,
    result: Result<MediaInfo, &'static str>,
}
struct Pending {
    proof: Arc<ReadyOutput>,
    generation: u64,
    source: ExternalSource,
    receiver: Receiver<Inspected>,
    cancelled: Arc<AtomicBool>,
    retirement: Box<dyn Retirement>,
    reply: Option<Inspected>,
    disconnected: bool,
    worker: route::WorkerTicket,
    request: route::PlayRequestTicket,
    play_generation: u64,
}
struct NativeAdmission {
    proof: Arc<ReadyOutput>,
    identity: route::PlaybackIdentity,
    source: ExternalSource,
    worker: route::WorkerTicket,
    request: route::PlayRequestTicket,
    play_generation: u64,
    driver_registered: bool,
}
pub(super) struct State {
    pub status: &'static str,
    pub error: Option<&'static str>,
    generation: u64,
    pending: Option<Pending>,
    waiting: Option<Arc<ReadyOutput>>,
    admission: Option<NativeAdmission>,
    shutdown: bool,
    inspector: Arc<dyn Inspector>,
    launcher: Box<dyn Launcher>,
}
impl Default for State {
    fn default() -> Self {
        Self::with_io(Arc::new(NativeInspector), Box::new(NativeLauncher))
    }
}
impl State {
    /// IO injection changes no proof, routing or native admission branch.
    pub fn with_io(inspector: Arc<dyn Inspector>, launcher: Box<dyn Launcher>) -> Self {
        Self {
            status: "idle",
            error: None,
            generation: 1,
            pending: None,
            waiting: None,
            admission: None,
            shutdown: false,
            inspector,
            launcher,
        }
    }
    fn cancel_pending(&mut self) {
        if let Some(pending) = &self.pending {
            pending.cancelled.store(true, Ordering::Release);
        }
    }
    fn advance(&mut self) -> bool {
        self.generation = if self.generation == 0 {
            0
        } else {
            self.generation.checked_add(1).unwrap_or(0)
        };
        self.cancel_pending();
        self.waiting = None;
        self.generation != 0
    }
    fn queue(&mut self, proof: ReadyOutput) -> Result<(), &'static str> {
        if self.shutdown {
            return Err("Composite output is closing");
        }
        if !self.advance() {
            return Err("Composite output generation exhausted");
        }
        self.waiting = Some(Arc::new(proof));
        self.status = "inspecting";
        self.error = None;
        Ok(())
    }
    fn start(
        &mut self,
        proof: Arc<ReadyOutput>,
        worker: route::WorkerTicket,
        request: route::PlayRequestTicket,
        play_generation: u64,
    ) -> Result<(), &'static str> {
        if self.pending.is_some() {
            return Err("Composite output inspection is retiring");
        }
        if self.generation == 0 || !qualified_geometry(&proof.capabilities, &proof.status) {
            return Err("Composite output is not qualified");
        }
        let private = proof
            .client
            .private_output(&proof.capabilities, &proof.status)
            .map_err(|_| "Composite output proof is unavailable")?;
        let source = ExternalSource::Hls(private.playlist_url_for_media_worker());
        let worker_source = source.clone();
        let generation = self.generation;
        let deadline = proof.admission_deadline;
        let lease_deadline = proof.lease_deadline;
        let cancelled = Arc::new(AtomicBool::new(false));
        let worker_cancel = Arc::clone(&cancelled);
        let inspector = Arc::clone(&self.inspector);
        let (sender, receiver) = mpsc::sync_channel(1);
        let work = Box::new(move |worker: &OffFrame| {
            let result = if worker_cancel.load(Ordering::Acquire)
                || deadline.expired(worker)
                || lease_deadline.expired(worker)
            {
                Err("Composite output inspection expired")
            } else {
                inspector.inspect(worker, &worker_source, deadline, Arc::clone(&worker_cancel))
            };
            let result = if worker_cancel.load(Ordering::Acquire)
                || deadline.expired(worker)
                || lease_deadline.expired(worker)
            {
                Err("Composite output inspection expired")
            } else {
                result
            };
            let _ = sender.send(Inspected { generation, result });
        });
        let retirement = self
            .launcher
            .spawn_io(work)
            .ok_or("Composite output worker unavailable")?;
        self.pending = Some(Pending {
            proof,
            generation,
            source,
            receiver,
            cancelled,
            retirement,
            reply: None,
            disconnected: false,
            worker,
            request,
            play_generation,
        });
        Ok(())
    }
    fn take_retired(&mut self) -> Option<Pending> {
        let pending = self.pending.as_mut()?;
        if pending.reply.is_none() && !pending.disconnected {
            match pending.receiver.try_recv() {
                Ok(reply) => pending.reply = Some(reply),
                Err(TryRecvError::Disconnected) => pending.disconnected = true,
                Err(TryRecvError::Empty) => {}
            }
        }
        if !pending.retirement.retired() {
            return None;
        }
        let mut pending = self.pending.take()?;
        if pending.reply.is_none() {
            pending.reply = pending.receiver.try_recv().ok();
        }
        Some(pending)
    }
    fn fail(&mut self, message: &'static str) {
        self.status = "unavailable";
        self.error = Some(message);
    }
}
impl Drop for State {
    fn drop(&mut self) {
        if let Some(pending) = &self.pending {
            pending.cancelled.store(true, Ordering::Release);
        }
    }
}

/// Called only by the authenticated backend driver. No server reply itself
/// changes the picture; queued inspection and actual native admission are later.
pub(super) fn offer(
    runtime: &mut Runtime,
    app: &mut App,
    proof: ReadyOutput,
) -> Result<(), &'static str> {
    if runtime.multiview_output.shutdown {
        return Err("Composite output is closing");
    }
    multiview_driver::authorize_output(runtime, app)?;
    if !proof.current(runtime, app)
        || !native_response_matches(runtime, &proof.status.native_expected)
    {
        return Err("Composite output authority changed");
    }
    // A newly authenticated lease extends only the same still-authorized output.
    // It neither reopens media nor resurrects an expired/replaced picture.
    let renew = runtime
        .multiview_output
        .admission
        .as_ref()
        .is_some_and(|receipt| {
            receipt_current(receipt, runtime, app) && same_output(&receipt.proof, &proof)
        });
    if renew {
        runtime
            .multiview_output
            .admission
            .as_mut()
            .expect("renewed owned output")
            .proof = Arc::new(proof);
        return Ok(());
    }
    if runtime.clock_sample.is_none_or(|sample| {
        proof
            .admission_deadline
            .remaining_at_sample(sample)
            .is_zero()
    }) {
        return Err("Composite output admission expired");
    }
    runtime.multiview_output.queue(proof)
}

pub(super) fn tick(runtime: &mut Runtime, app: &mut App) {
    if runtime.multiview_output.shutdown {
        shutdown_pending(runtime);
        return;
    }
    let needs_policy = runtime.multiview_output.admission.is_some()
        || runtime.multiview_output.pending.is_some()
        || runtime.multiview_output.waiting.is_some();
    let policy = if needs_policy {
        multiview_driver::authorize_output(runtime, app)
    } else {
        Ok(())
    };
    let mut state = std::mem::take(&mut runtime.multiview_output);
    if policy.is_err() {
        state.cancel_pending();
        state.waiting = None;
        state.fail("Composite output policy changed");
    }
    if state.admission.as_ref().is_some_and(|receipt| {
        policy.is_err()
            || state.generation == 0
            || !receipt_route_current(receipt, runtime, app)
            || !receipt.proof.current(runtime, app)
    }) {
        let receipt = state.admission.as_ref().expect("checked admission");
        let pending = state.pending.as_ref().filter(|pending| {
            !pending.cancelled.load(Ordering::Acquire)
                && pending.generation == state.generation
                && pending.worker == route::worker_ticket()
                && route::play_request_is_current(pending.request)
                && pending.play_generation == runtime.play_generation
        });
        let next = state
            .waiting
            .as_ref()
            .or_else(|| pending.map(|pending| &pending.proof));
        // Only the already admitted picture may remain while a checked successor
        // is inspected. This grants no new Load for the stale output generation.
        let keep_old = policy.is_ok()
            && state.generation != 0
            && receipt_route_current(receipt, runtime, app)
            && runtime.clock_sample.is_some_and(|sample| {
                !receipt
                    .proof
                    .lease_deadline
                    .remaining_at_sample(sample)
                    .is_zero()
            })
            && runtime
                .multiview_driver
                .retained_output_sources_current(runtime, &receipt.proof.status)
            && next.is_some_and(|next| {
                same_session(&receipt.proof, next)
                    && next.current(runtime, app)
                    && runtime.clock_sample.is_some_and(|sample| {
                        !next
                            .admission_deadline
                            .remaining_at_sample(sample)
                            .is_zero()
                    })
            });
        if !keep_old {
            let receipt = state.admission.take().expect("checked admission");
            stop_owned(runtime, app, &receipt);
            state.fail("Composite output authority changed");
        }
    }
    if state.pending.as_ref().is_some_and(|pending| {
        !pending.proof.current(runtime, app)
            || runtime.clock_sample.is_none_or(|sample| {
                pending
                    .proof
                    .admission_deadline
                    .remaining_at_sample(sample)
                    .is_zero()
            })
            || pending.play_generation != runtime.play_generation
            || pending.worker != route::worker_ticket()
            || !route::play_request_is_current(pending.request)
    }) {
        state.cancel_pending();
    }
    if let Some(mut pending) = state.take_retired() {
        if !pending.cancelled.load(Ordering::Acquire)
            && pending.generation == state.generation
            && pending.proof.current(runtime, app)
            && runtime.clock_sample.is_some_and(|sample| {
                !pending
                    .proof
                    .admission_deadline
                    .remaining_at_sample(sample)
                    .is_zero()
            })
            && pending.play_generation == runtime.play_generation
            && pending.worker == route::worker_ticket()
            && route::play_request_is_current(pending.request)
        {
            match pending
                .reply
                .take()
                .filter(|reply| reply.generation == pending.generation)
            {
                Some(Inspected {
                    result: Ok(info), ..
                }) => {
                    match admit(
                        runtime,
                        app,
                        Arc::clone(&pending.proof),
                        pending.source,
                        info,
                    ) {
                        Ok(receipt) => {
                            state.admission = Some(receipt);
                            state.status = "admitted";
                            state.error = None;
                        }
                        Err(message) => state.fail(message),
                    }
                }
                _ => state.fail("Composite output inspection failed"),
            }
        } else if state.waiting.is_none() && state.admission.is_none() {
            state.fail("Composite output authority changed");
        }
    }
    if state.pending.is_none() {
        if let Some(proof) = state.waiting.take() {
            if proof.current(runtime, app) {
                if let Err(message) = state.start(
                    proof,
                    route::worker_ticket(),
                    route::play_request_ticket(),
                    runtime.play_generation,
                ) {
                    state.fail(message);
                } else {
                    state.status = "inspecting";
                    state.error = None;
                }
            } else {
                state.fail("Composite output authority changed");
            }
        }
    }
    // A failed/retired inspection cannot leave the old, no-longer-current
    // output retained until another frame merely because it existed above.
    if state.pending.is_none()
        && state.waiting.is_none()
        && state
            .admission
            .as_ref()
            .is_some_and(|receipt| !receipt.proof.current(runtime, app))
    {
        let receipt = state.admission.take().expect("checked stale admission");
        stop_owned(runtime, app, &receipt);
    }
    runtime.multiview_output = state;
    register_picture(runtime, app);
    publish_metadata(runtime, app);
}

/// Called before the frame loop's shared shutdown drain. No source work is
/// admitted; an unrelated successor route is never stopped by this old owner.
pub(super) fn begin_shutdown(runtime: &mut Runtime, app: &mut App) {
    let mut state = std::mem::take(&mut runtime.multiview_output);
    state.shutdown = true;
    state.cancel_pending();
    state.waiting = None;
    if let Some(receipt) = state.admission.take() {
        stop_owned(runtime, app, &receipt);
        if runtime.multiview_picture.as_ref() == Some(&receipt.identity) {
            runtime.multiview_picture = None;
        }
    }
    state.status = "idle";
    state.error = None;
    runtime.multiview_output = state;
    app.bridge.publish_multiview_offer(None);
}
/// Poll only already-owned inspection retirement. No join, source activation,
/// decoder start, or new independent timeout is introduced by shutdown.
pub(super) fn shutdown_pending(runtime: &mut Runtime) -> bool {
    runtime.multiview_output.take_retired();
    runtime.multiview_output.pending.is_some()
}

fn same_session(left: &ReadyOutput, right: &ReadyOutput) -> bool {
    left.connection_generation == right.connection_generation
        && Arc::ptr_eq(&left.client, &right.client)
        && left.status.session_id == right.status.session_id
        && left.status.server_epoch == right.status.server_epoch
        && left.status.native_expected.runtime == right.status.native_expected.runtime
        && left.status.native_expected.expected.scene == right.status.native_expected.expected.scene
}
fn register_picture(runtime: &mut Runtime, app: &mut App) {
    let Some(receipt) = runtime
        .multiview_output
        .admission
        .as_ref()
        .filter(|receipt| !receipt.driver_registered)
    else {
        return;
    };
    let proof = Arc::clone(&receipt.proof);
    let identity = receipt.identity.clone();
    let Some(output) = proof.status.output.as_ref() else {
        return;
    };
    let mut driver = std::mem::take(&mut runtime.multiview_driver);
    let result = driver.picture_admitted(
        runtime,
        app,
        &identity,
        proof.connection_generation,
        &proof.status.session_id,
        &proof.status.server_epoch,
        &output.output_generation,
    );
    runtime.multiview_driver = driver;
    if result.is_ok() {
        if let Some(receipt) = runtime
            .multiview_output
            .admission
            .as_mut()
            .filter(|receipt| receipt.identity == identity)
        {
            receipt.driver_registered = true;
        }
    } else if let Some(receipt) = runtime.multiview_output.admission.take() {
        stop_owned(runtime, app, &receipt);
        runtime
            .multiview_output
            .fail("Composite output picture authority changed");
    }
}

fn same_output(left: &ReadyOutput, right: &ReadyOutput) -> bool {
    left.connection_generation == right.connection_generation
        && Arc::ptr_eq(&left.client, &right.client)
        && left.status.server_id == right.status.server_id
        && left.status.server_epoch == right.status.server_epoch
        && left.status.session_id == right.status.session_id
        && left.status.geometry == right.status.geometry
        && left.status.native_expected.runtime == right.status.native_expected.runtime
        && left.status.native_expected.expected.scene == right.status.native_expected.expected.scene
        && left.status.native_expected.expected.layout_revision
            == right.status.native_expected.expected.layout_revision
        && left
            .status
            .output
            .as_ref()
            .zip(right.status.output.as_ref())
            .is_some_and(|(a, b)| {
                a.output_generation == b.output_generation && a.playlist_path == b.playlist_path
            })
}

fn checked_source(
    source: ExternalSource,
    info: &MediaInfo,
) -> Result<ExternalSource, &'static str> {
    let actual = info.resolved_source.clone().unwrap_or(source);
    if !matches!(&actual, ExternalSource::Hls(url) if url.starts_with("https://"))
        || info.hls_playback.as_ref().is_some_and(
            |playback| !matches!(&actual, ExternalSource::Hls(url) if playback.matches_source(url)),
        )
    {
        return Err("Composite output is not inspected HTTPS HLS");
    }
    Ok(actual)
}
fn admit(
    runtime: &mut Runtime,
    app: &mut App,
    proof: Arc<ReadyOutput>,
    source: ExternalSource,
    info: MediaInfo,
) -> Result<NativeAdmission, &'static str> {
    if !proof.current(runtime, app) {
        return Err("Composite output authority changed");
    }
    multiview_driver::authorize_output(runtime, app)?;
    if !proof.current(runtime, app) {
        return Err("Composite output authority changed");
    }
    let source = checked_source(source, &info)?;
    if u32::from(info.width) != proof.status.geometry.canvas.width
        || u32::from(info.height) != proof.status.geometry.canvas.height
        || info.video_codec != "h264"
        || info.audio_codec != "aac"
        || !info.fps.is_finite()
        || info.fps <= 0.0
    {
        return Err("Composite output differs from the qualified presentation");
    }
    let subtitles = subtitle_settings::Stored::read(&runtime.saved)?;
    // Preserve the previous legitimate source's normal resume bookmark before
    // teardown invalidates its route/position. This does not select a provider.
    runtime.checkpoint(app, true);
    // Only now, after inspection and all owner gates, may the old picture retire.
    playback::replace_player_in_place(
        &mut app.player,
        &mut app.adapters.player,
        &mut app.repause_at,
        &mut app.pages,
    );
    runtime.cancel_play_request();
    // Composite ownership is process-local. Retire old provider retry/restore
    // intent without changing the saved profile, selection or resume bookmark.
    runtime.continuity.stop(runtime.generation);
    runtime.resume_pending = None;
    runtime.resume_floor = None;
    runtime.playing_attachment = None;
    route::request_external_source(
        &mut app.player.session,
        &info,
        source,
        "Multiview",
        "Multiview",
        playback::start_caps(),
    )
    .map_err(|_| "Composite output native request was refused")?;
    let identity = route::playback_identity(&app.player.session)
        .ok_or("Composite output native identity is unavailable")?;
    let source = route::external_source(&app.player.session)
        .ok_or("Composite output native source is unavailable")?;
    let receipt = NativeAdmission {
        proof,
        identity: identity.clone(),
        source,
        worker: route::worker_ticket(),
        request: route::play_request_ticket(),
        play_generation: runtime.play_generation,
        driver_registered: false,
    };
    runtime.playing = None;
    runtime.failed_playback = None;
    runtime.playback_failure.clear();
    runtime.active_lease.take();
    runtime.multiview_picture = Some(identity);
    // Retire the old Plex leaf's tracks/chapters/markers. Generic composite
    // metadata is published only once the real owned route receipt is installed.
    app.bridge
        .metadata_run(plx_data::stores::metadata::MetadataCmd::InstallPlaying(
            None,
        ));
    app.bridge
        .metadata_run(plx_data::stores::metadata::MetadataCmd::SetNowPlaying(None));
    subtitles.apply(app, true);
    if !playback::start_playback(
        &mut app.player.session,
        &mut app.adapters.player,
        0,
        playback::Origin::Here,
        HudStart::Ordinary,
        None,
        &mut app.pages,
        &mut app.bridge,
    ) {
        stop_owned(runtime, app, &receipt);
        return Err("Composite output native start was refused");
    }
    Ok(receipt)
}
fn receipt_route_current(receipt: &NativeAdmission, runtime: &Runtime, app: &App) -> bool {
    route::has_url(&app.player.session)
        && runtime.play_generation == receipt.play_generation
        && route::external_source(&app.player.session).as_ref() == Some(&receipt.source)
        && route::worker_ticket() == receipt.worker
        && route::play_request_is_current(receipt.request)
        && route::playback_identity_matches(&app.player.session, &receipt.identity)
        && runtime.multiview_picture.as_ref() == Some(&receipt.identity)
}
fn receipt_current(receipt: &NativeAdmission, runtime: &Runtime, app: &App) -> bool {
    runtime.multiview_output.generation != 0
        && receipt_route_current(receipt, runtime, app)
        && receipt.proof.current(runtime, app)
}

/// Logical target identity survives Stop. Only the still-installed exact route
/// owns the picture; this intentionally does not require the old service proof
/// to remain current during bounded, authorized successor inspection.
pub(super) fn admitted_route_current(
    runtime: &Runtime,
    app: &App,
    recorded_identity: &route::PlaybackIdentity,
) -> bool {
    !runtime.multiview_output.shutdown
        && runtime.multiview_output.generation != 0
        && runtime
            .multiview_output
            .admission
            .as_ref()
            .is_some_and(|receipt| {
                &receipt.identity == recorded_identity
                    && receipt_route_current(receipt, runtime, app)
            })
}

/// Automatic ordinary-provider recovery must not run behind an installed
/// composite route. A stopped logical target cannot suppress normal recovery.
pub(super) fn owns_picture(runtime: &Runtime, app: &App) -> bool {
    runtime
        .multiview_output
        .admission
        .as_ref()
        .is_some_and(|receipt| admitted_route_current(runtime, app, &receipt.identity))
}

/// A process-local composite descriptor owns the footer only while its exact
/// native route identity does. The OTT marker also fences late Plex Detail
/// publication; no loading/inspection event fabricates a confirmed first frame.
pub(super) fn publish_metadata(runtime: &mut Runtime, app: &mut App) -> bool {
    if !owns_picture(runtime, app) {
        return false;
    }
    runtime.programme_leaf = None;
    runtime.live_hud = Default::default();
    runtime.metadata_playback_identity = None;
    let unchanged = app.bridge.metadata_view().now_playing().is_some_and(|now| {
        now.title == "Multiview"
            && now.ep_title.is_empty()
            && !now.is_episode
            && !now.is_real_episode
            && now.season == 0
            && now.index == 0
            && now.summary.is_empty()
            && now.year == 0
            && now.dur_ms == 0
            && now.rating.is_empty()
            && now.thumb.is_empty()
            && now.detail_rk.is_empty()
            && now.ott.as_ref().is_some_and(|ott| {
                ott.collection_label == "Multiview"
                    && ott.live.is_none()
                    && ott.archive.is_none()
                    && ott.next.is_none()
                    && ott.art.is_none()
                    && ott.confirmed_start_unix_ms.is_none()
            })
    });
    if !unchanged {
        app.bridge
            .metadata_run(plx_data::stores::metadata::MetadataCmd::SetNowPlaying(
                Some(plx_data::metadata::NowPlaying {
                    ott: Some(plx_data::metadata::OttNowPlaying {
                        live: None,
                        archive: None,
                        collection_label: "Multiview".into(),
                        confirmed_start_unix_ms: None,
                        next: None,
                        art: None,
                    }),
                    is_episode: false,
                    is_real_episode: false,
                    title: "Multiview".into(),
                    ep_title: String::new(),
                    season: 0,
                    index: 0,
                    summary: String::new(),
                    year: 0,
                    dur_ms: 0,
                    rating: String::new(),
                    thumb: String::new(),
                    detail_rk: String::new(),
                }),
            ));
    }
    runtime.programme_metadata_generation = Some(app.bridge.metadata_mut().r#gen());
    true
}

/// None means a different owner. A recognized but revoked receipt MUST fail
/// closed and cannot fall through to catalogue, companion or legacy Plex paths.
pub(super) fn native_start_admission(
    runtime: &mut Runtime,
    app: &App,
) -> Option<Result<(), &'static str>> {
    let receipt = runtime.multiview_output.admission.as_ref()?;
    if !route::playback_identity_matches(&app.player.session, &receipt.identity) {
        return None;
    }
    let current = |runtime: &Runtime| {
        runtime
            .multiview_output
            .admission
            .as_ref()
            .is_some_and(|receipt| {
                receipt_current(receipt, runtime, app)
                    && runtime.clock_sample.is_some_and(|sample| {
                        !receipt
                            .proof
                            .admission_deadline
                            .remaining_at_sample(sample)
                            .is_zero()
                    })
            })
    };
    if !current(runtime) {
        return Some(Err("Composite output authority changed before native Load"));
    }
    let policy = multiview_driver::authorize_output(runtime, app);
    Some(policy.and_then(|()| {
        if current(runtime) {
            Ok(())
        } else {
            Err("Composite output authority changed before native Load")
        }
    }))
}
pub(super) fn retire_native_start(
    runtime: &mut Runtime,
    app: &App,
    identity: &route::PlaybackIdentity,
    ticket: route::PlayRequestTicket,
) {
    if !route::play_request_is_current(ticket)
        || !route::playback_identity_matches(&app.player.session, identity)
        || !runtime
            .multiview_output
            .admission
            .as_ref()
            .is_some_and(|receipt| &receipt.identity == identity && receipt.request == ticket)
    {
        return;
    }
    runtime.multiview_output.admission = None;
    if runtime.multiview_picture.as_ref() == Some(identity) {
        runtime.multiview_picture = None;
    }
}
fn stop_owned(runtime: &mut Runtime, app: &mut App, receipt: &NativeAdmission) {
    if route::playback_identity_matches(&app.player.session, &receipt.identity)
        && route::play_request_is_current(receipt.request)
    {
        playback::replace_player_in_place(
            &mut app.player,
            &mut app.adapters.player,
            &mut app.repause_at,
            &mut app.pages,
        );
    }
    // Withdraw only this owner's stale marker; a foreign successor's route and
    // metadata must remain byte-for-byte untouched.
    if runtime.multiview_picture.as_ref() == Some(&receipt.identity) {
        runtime.multiview_picture = None;
    }
}

#[cfg(test)]
#[path = "ott_multiview_output_tests.rs"]
mod tests;
