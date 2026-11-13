//! Actual connection -> ServiceClient -> Driver -> PrivateOutput -> output route
//! -> deferred native-start gate. Synthetic service and inspected media only:
//! no FOSS router, TLS, media decoding, socket, firmware or physical-TV claim.
use super::multiview_service as service;
use super::*;
use plx_base::task::{self, time::Deadline, OffFrame};
use plx_media::{
    ff::{inspect::MediaInfo, ExternalSource},
    player, route,
};
use std::{
    collections::VecDeque,
    sync::{atomic::AtomicUsize, Mutex},
    time::{Duration, Instant},
};

type Work = Box<dyn FnOnce(&OffFrame) + Send>;
type Jobs = Arc<Mutex<VecDeque<(Work, Arc<AtomicBool>)>>>;
struct Done(Arc<AtomicBool>);
impl multiview_connection::Retirement for Done {
    fn retired(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
}
struct ManualLauncher(Jobs);
impl multiview_connection::Launcher for ManualLauncher {
    fn spawn_io(&mut self, work: Work) -> Option<Box<dyn multiview_connection::Retirement>> {
        let done = Arc::new(AtomicBool::new(false));
        let mut jobs = self.0.lock().unwrap();
        assert!(
            jobs.is_empty(),
            "one owner may admit only one inspection task"
        );
        jobs.push_back((work, done.clone()));
        Some(Box::new(Done(done)))
    }
}
fn finish_jobs(jobs: &Jobs) {
    loop {
        let job = jobs.lock().unwrap().pop_front();
        let Some((work, done)) = job else {
            return;
        };
        work(&OffFrame::for_test());
        done.store(true, Ordering::Release);
    }
}
#[derive(Default)]
struct ServiceState {
    epoch: String,
    status: Option<service::StatusResponse>,
    paths: Vec<String>,
    operations: Vec<String>,
    audio_target: Option<u8>,
}
struct SyntheticTransport(Arc<Mutex<ServiceState>>);
impl service::Transport for SyntheticTransport {
    fn send(
        &self,
        _: &OffFrame,
        binding: &service::ServiceBinding,
        request: &service::ServiceRequest,
        _: Deadline,
        cancelled: &AtomicBool,
    ) -> Result<Vec<u8>, service::ServiceError> {
        if cancelled.load(Ordering::Acquire) {
            return Err(service::ServiceError::Cancelled);
        }
        let url = request.url(binding);
        assert!(url.starts_with("https://service.invalid/"));
        let body: Value = serde_json::from_slice(request.body().unwrap_or(b"{}")).unwrap();
        let mut server = self.0.lock().unwrap();
        server.paths.push(url.clone());
        let response = if url.ends_with("/capabilities") {
            json!({"version":1,"server_id":"server_fixture","server_epoch":server.epoch,
                "max_inputs":4,"qualified_input_adapters":["http_checked"],
                "output_auth":"hmac_path_v1","catalog_revision":"catalog_fixture",
                "output_profiles":[{"profile_id":"profile_fixture","canvas":{"width":320,"height":180,"fps":10},
                    "max_inputs":4,"geometry_modes":["disjoint","pip"]}]})
        } else if url.ends_with("/sessions") {
            // The explicitly synthetic service can render an empty qualified grid.
            // This does not advertise capacity in the real production broker.
            let geometry: service::Geometry =
                serde_json::from_value(body["geometry"].clone()).unwrap();
            let status = service::StatusResponse {
                version: 1,
                server_id: "server_fixture".into(),
                server_epoch: server.epoch.clone(),
                session_id: "session_fixture".into(),
                native_expected: serde_json::from_value(body["native_expected"].clone()).unwrap(),
                backend_expected: service::BackendExpected {
                    runtime: 1,
                    scene: "backend_scene".into(),
                    geometry_revision: 1,
                    focus_revision: 1,
                    audio_revision: 1,
                },
                state: service::SessionState::Ready,
                slots: geometry
                    .rects
                    .iter()
                    .map(|rect| service::SlotSnapshot {
                        slot_id: rect.slot_id,
                        source_revision: 1,
                        state: service::SlotState::Empty,
                        error: None,
                        frames: 0,
                        position_ms: None,
                        duration_ms: None,
                        source_ref: None,
                        item_ref: None,
                    })
                    .collect(),
                geometry,
                audio: service::AudioState {
                    policy: service::AudioPolicy::Muted,
                    slot_id: None,
                },
                output: Some(service::Output {
                    session_id: "session_fixture".into(),
                    server_epoch: server.epoch.clone(),
                    output_generation: "output_fixture".into(),
                    playlist_path: format!(
                        "/multiview/v1/media/{}.{}/index.m3u8",
                        "a".repeat(32),
                        "b".repeat(64)
                    ),
                    lease_expires_in_ms: 60_000,
                }),
            };
            server.status = Some(status.clone());
            serde_json::to_value(status).unwrap()
        } else if url.ends_with("/status") {
            let status = server.status.as_mut().unwrap();
            status.native_expected =
                serde_json::from_value(body["native_expected"].clone()).unwrap();
            serde_json::to_value(status).unwrap()
        } else if url.ends_with("/control") {
            let operation = body["op"]["name"].as_str().unwrap().to_owned();
            server.operations.push(operation.clone());
            let mut context: service::Binding =
                serde_json::from_value(body["context"].clone()).unwrap();
            let status = server.status.as_ref().unwrap();
            if context.server_epoch != status.server_epoch
                || context.session_id != status.session_id
                || context.native_expected != status.native_expected
                || context.backend_expected != status.backend_expected
                || status
                    .slots
                    .iter()
                    .find(|slot| slot.slot_id == context.native_expected.slot_id)
                    .is_none_or(|slot| slot.source_revision != context.backend_slot_revision)
            {
                return Err(service::ServiceError::AuthorityRevoked);
            }
            if !body["next_native"].is_null() {
                context.native_expected =
                    serde_json::from_value(body["next_native"].clone()).unwrap();
            }
            match operation.as_str() {
                "audio" => {
                    let policy: service::AudioPolicy =
                        serde_json::from_value(body["op"]["policy"].clone()).unwrap();
                    let target = (policy == service::AudioPolicy::Slot)
                        .then_some(context.native_expected.slot_id);
                    let status = server.status.as_ref().unwrap();
                    if status.audio.policy != policy || server.audio_target != target {
                        context.backend_expected.audio_revision += 1;
                    }
                    let owner = match policy {
                        service::AudioPolicy::Muted => None,
                        service::AudioPolicy::Slot | service::AudioPolicy::FollowsFocus => status
                            .slots
                            .iter()
                            .find(|slot| {
                                slot.slot_id == context.native_expected.slot_id
                                    && slot.state == service::SlotState::Playing
                            })
                            .map(|slot| slot.slot_id),
                    };
                    server.audio_target = target;
                    server.status.as_mut().unwrap().audio = service::AudioState {
                        policy,
                        slot_id: owner,
                    };
                }
                "off" => {
                    let status = server.status.as_mut().unwrap();
                    status.state = service::SessionState::Closed;
                    status.output = None;
                }
                _ => return Err(service::ServiceError::InvalidRequest),
            }
            let status = server.status.as_mut().unwrap();
            status.native_expected = context.native_expected.clone();
            status.backend_expected = context.backend_expected.clone();
            json!({"version":1,"context":context,"accepted":true,
                "operation_id":"operation_fixture","state":status.state})
        } else {
            return Err(service::ServiceError::InvalidRequest);
        };
        Ok(serde_json::to_vec(&response).unwrap())
    }
}
struct SyntheticInspector(Arc<AtomicUsize>);
impl multiview_output::Inspector for SyntheticInspector {
    fn inspect(
        &self,
        worker: &OffFrame,
        source: &ExternalSource,
        deadline: Deadline,
        cancelled: Arc<AtomicBool>,
    ) -> Result<MediaInfo, &'static str> {
        assert!(!cancelled.load(Ordering::Acquire));
        assert!(!deadline.expired(worker));
        assert!(
            matches!(source, ExternalSource::Hls(url) if url.starts_with("https://service.invalid/multiview/v1/media/")
            && !url.contains("fixture-bearer"))
        );
        self.0.fetch_add(1, Ordering::AcqRel);
        Ok(media())
    }
}
fn media() -> MediaInfo {
    MediaInfo {
        resolved_source: None,
        hls_playback: None,
        tracks: Default::default(),
        video_codec: "h264".into(),
        audio_codec: "aac".into(),
        width: 320,
        height: 180,
        fps: 10.0,
        duration_ns: 0,
        audio_channels: 2,
        audio_sample_rate: 48_000,
        audio_bit_depth: 16,
        dovi: plx_data::metadata::Dovi::NONE,
    }
}
struct Rig {
    app: Box<App>,
    server: Arc<Mutex<ServiceState>>,
    output_jobs: Jobs,
    inspected: Arc<AtomicUsize>,
    saved_before: Value,
}
impl Drop for Rig {
    fn drop(&mut self) {
        // Synthetic Unload has no worker to finish. Normal stop MUST retire its
        // queued native start before dropping the inert adapter.
        player::SHARED.duration_ns.store(0, Ordering::Release);
        player::stop_bufferfeed(&mut self.app.player.session, &mut self.app.adapters.player);
        player::TX.reset();
        route::reset_player_control_for_test(&self.app.player.session);
        assert!(!self.app.adapters.player.release_has_queued_start());
        assert!(!self.app.adapters.player.is_live());
    }
}
fn setup() -> Rig {
    let mut app = Box::new(crate::app::run::lifecycle_regression_tests::app());
    app.running = true;
    player::TX.reset();
    route::reset_player_control_for_test(&app.player.session);
    let mut runtime = Runtime::unloaded(crate::app::ott_companion::Supervisor::inert_for_test());
    runtime.loaded = true;
    runtime.state_load = StateLoad::Idle;
    runtime.entropy = Some([1; 16]);
    runtime.resume_source_work();
    runtime.remote = Remote::disabled_for_test("dev_admission", "runtime_admission");
    let sample = task::time::ClockSample::capture_main(app.adapters.player.mt()).unwrap();
    runtime.clock_sample = Some(sample);
    runtime.clock_anchor = Some(sample);
    let protected = json!(
        json!({"server_id":"server_fixture","origin":"https://service.invalid",
        "audience":service::AUDIENCE,"installation_id":"dev_admission",
        "control_bearer":"fixture-bearer-abcdefghijklmnopqrstuvwxyz"})
        .to_string()
    );
    runtime
        .saved
        .settings
        .insert("nativeMultiviewService".into(), protected.clone());
    let server = Arc::new(Mutex::new(ServiceState {
        epoch: "epoch_fixture".into(),
        ..Default::default()
    }));
    let connection_jobs: Jobs = Default::default();
    runtime.multiview_connection = multiview_connection::State::with_io(
        Arc::new(SyntheticTransport(server.clone())),
        Box::new(ManualLauncher(connection_jobs.clone())),
    );
    runtime.multiview_connection.tick(
        true,
        true,
        Some(&protected),
        runtime.remote.persisted_device_id(),
    );
    finish_jobs(&connection_jobs);
    runtime.multiview_connection.tick(
        true,
        true,
        Some(&protected),
        runtime.remote.persisted_device_id(),
    );
    assert!(
        runtime.multiview_connection.qualified().is_some(),
        "actual ServiceClient capabilities must validate"
    );
    let output_jobs: Jobs = Default::default();
    let inspected = Arc::new(AtomicUsize::new(0));
    runtime.multiview_output = multiview_output::State::with_io(
        Arc::new(SyntheticInspector(inspected.clone())),
        Box::new(ManualLauncher(output_jobs.clone())),
    );
    let saved_before = serde_json::to_value(&runtime.saved).unwrap();
    app.ott = Some(runtime);
    Rig {
        app,
        server,
        output_jobs,
        inspected,
        saved_before,
    }
}
fn queue_layout(rig: &mut Rig) {
    let mut runtime = rig.app.ott.take().unwrap();
    let params = json!({"version":1,"runtime":runtime.remote.runtime(),"op":"layout","layout":"grid4",
        "expected":{"scene":"off","layout_revision":"off","focus_revision":"off","audio_revision":"off"}});
    let now = runtime.now_ms();
    let invocation=runtime.remote.raw_poll_for_test(&json!({"commands":[],"server_time":1000,
        "requests":[{"id":format!("{:032x}",17),"action":"multiview","params":params,"expires_at":1100}]}).to_string(),now).remove(0);
    let mut driver = std::mem::take(&mut runtime.multiview_driver);
    assert!(driver
        .command(&mut runtime, &rig.app, &invocation)
        .is_none());
    runtime.multiview_driver = driver;
    rig.app.ott = Some(runtime);
    let end = Instant::now() + Duration::from_secs(3);
    while rig.output_jobs.lock().unwrap().is_empty() {
        assert!(
            Instant::now() < end,
            "owned Driver service work must settle"
        );
        let mut runtime = rig.app.ott.take().unwrap();
        let mut driver = std::mem::take(&mut runtime.multiview_driver);
        driver.tick(&mut runtime, &mut rig.app);
        let ready = driver.take_ready_output(&runtime);
        runtime.multiview_driver = driver;
        if let Some(ready) = ready {
            multiview_output::offer(&mut runtime, &mut rig.app, ready).unwrap();
        }
        multiview_output::tick(&mut runtime, &mut rig.app);
        rig.app.ott = Some(runtime);
        std::thread::sleep(Duration::from_millis(1));
    }
    assert!(!route::has_url(&rig.app.player.session));
    assert!(!rig.app.adapters.player.is_live());
    let server = rig.server.lock().unwrap();
    assert_eq!(
        server
            .operations
            .iter()
            .filter(|operation| *operation == "audio")
            .count(),
        1,
        "actual initial audio policy must be acknowledged before output inspection"
    );
    let status = server.status.as_ref().unwrap();
    assert_eq!(status.audio.policy, service::AudioPolicy::FollowsFocus);
    assert_eq!(status.audio.slot_id, None, "empty grid has no audio owner");
    assert_eq!(status.backend_expected.audio_revision, 2);
}
fn admitted() -> Rig {
    admit_rig(setup())
}
fn admit_rig(mut rig: Rig) -> Rig {
    queue_layout(&mut rig);
    // This is a machine event only, not a worker or a sleeping thread. Age zero
    // gives the real deferred-start path a short bounded held native release.
    rig.app
        .adapters
        .player
        .begin_release_for_test(Duration::ZERO);
    finish_jobs(&rig.output_jobs);
    let mut runtime = rig.app.ott.take().unwrap();
    multiview_output::tick(&mut runtime, &mut rig.app);
    assert_eq!(runtime.multiview_output.status, "admitted");
    assert!(runtime.multiview_picture.is_some());
    assert!(route::playback_identity(&rig.app.player.session).is_some());
    assert!(multiview_output::owns_picture(&runtime, &rig.app));
    assert_eq!(
        serde_json::to_value(&runtime.saved).unwrap(),
        rig.saved_before
    );
    rig.app.ott = Some(runtime);
    assert_eq!(rig.inspected.load(Ordering::Acquire), 1);
    assert!(rig.app.adapters.player.release_has_queued_start());
    assert!(!rig.app.adapters.player.is_live());
    rig
}
#[test]
fn actual_composite_adapter_mints_native_identity_and_passes_deferred_final_gate() {
    let _serial = plx_base::testlock::serial();
    let mut rig = admitted();
    assert!(foreign_native_start_allowed(&mut rig.app));
    crate::app::run::publish_playback_frame(&mut rig.app, false);
    assert!(
        rig.app.adapters.player.release_has_queued_start(),
        "valid owner must keep the held start"
    );
    assert!(
        !rig.app.adapters.player.is_live(),
        "fixture must not start a decoder"
    );
    assert!(rig
        .server
        .lock()
        .unwrap()
        .paths
        .iter()
        .any(|url| url.ends_with("/sessions")));
}
#[test]
fn actual_deferred_composite_gate_revokes_expired_lease_without_loading() {
    let _serial = plx_base::testlock::serial();
    let mut rig = admitted();
    let runtime = rig.app.ott.as_mut().unwrap();
    runtime.clock_sample = Some(
        runtime
            .clock_sample
            .unwrap()
            .advance_for_test(Duration::from_secs(61)),
    );
    assert!(!foreign_native_start_allowed(&mut rig.app));
    crate::app::run::publish_playback_frame(&mut rig.app, false);
    assert!(!rig.app.adapters.player.release_has_queued_start());
    assert!(!rig.app.adapters.player.is_live());
}
#[test]
fn actual_deferred_composite_gate_revokes_changed_installation_without_loading() {
    let _serial = plx_base::testlock::serial();
    let mut rig = admitted();
    let runtime = rig.app.ott.as_mut().unwrap();
    runtime.multiview_connection.tick(
        true,
        true,
        runtime.saved.settings.get("nativeMultiviewService"),
        Some("different_device"),
    );
    assert!(!foreign_native_start_allowed(&mut rig.app));
    crate::app::run::publish_playback_frame(&mut rig.app, false);
    assert!(!rig.app.adapters.player.release_has_queued_start());
    assert!(!rig.app.adapters.player.is_live());
}
#[test]
fn actual_deferred_composite_gate_rejects_newer_route_identity_without_loading() {
    let _serial = plx_base::testlock::serial();
    let mut rig = admitted();
    let old = route::playback_identity(&rig.app.player.session).unwrap();
    // Ordinary replacement retires the old queued Load and native route through
    // the actual production ritual, not by clearing a URL or resetting globals.
    playback::replace_player_in_place(
        &mut rig.app.player,
        &mut rig.app.adapters.player,
        &mut rig.app.repause_at,
        &mut rig.app.pages,
    );
    assert!(!rig.app.adapters.player.release_has_queued_start());
    assert!(!rig.app.adapters.player.is_live());
    let newer_source = ExternalSource::Hls("https://other.invalid/new.m3u8".into());
    route::request_external_source(
        &mut rig.app.player.session,
        &media(),
        newer_source.clone(),
        "New source",
        "",
        playback::start_caps(),
    )
    .unwrap();
    let newer = route::playback_identity(&rig.app.player.session).unwrap();
    let accepted_source = route::external_source(&rig.app.player.session).unwrap();
    assert_eq!(
        accepted_source,
        ExternalSource::Hls(plx_data::ott::http_url("https://other.invalid/new.m3u8").unwrap())
    );
    assert_ne!(newer, old);
    let mut runtime = rig.app.ott.take().unwrap();
    multiview_output::tick(&mut runtime, &mut rig.app);
    assert!(!multiview_output::owns_picture(&runtime, &rig.app));
    rig.app.ott = Some(runtime);
    assert_eq!(
        route::playback_identity(&rig.app.player.session),
        Some(newer)
    );
    assert_eq!(
        route::external_source(&rig.app.player.session),
        Some(accepted_source)
    );
    // No ordinary catalogue proof was installed for this synthetic successor.
    // Its legacy refusal is not a claim of positive ordinary-source authority.
    assert!(!foreign_native_start_allowed(&mut rig.app));
    crate::app::run::publish_playback_frame(&mut rig.app, false);
    assert!(!rig.app.adapters.player.release_has_queued_start());
    assert!(!rig.app.adapters.player.is_live());
}

fn assert_composite_metadata(app: &App) {
    let view = app.bridge.metadata_view();
    assert!(
        view.playing().is_none(),
        "old Plex track/chapter owner must retire"
    );
    let now = view
        .now_playing()
        .expect("composite owns its generic descriptor");
    assert_eq!(now.title, "Multiview");
    assert!(!now.is_episode && !now.is_real_episode);
    assert!(now.ep_title.is_empty() && now.summary.is_empty() && now.thumb.is_empty());
    assert!(now.detail_rk.is_empty() && now.rating.is_empty());
    assert_eq!((now.season, now.index, now.year, now.dur_ms), (0, 0, 0, 0));
    let ott = now
        .ott
        .as_ref()
        .expect("OTT marker fences late Detail publication");
    assert_eq!(ott.collection_label, "Multiview");
    assert!(ott.live.is_none() && ott.archive.is_none() && ott.next.is_none() && ott.art.is_none());
    assert!(
        ott.confirmed_start_unix_ms.is_none(),
        "admission is not a confirmed frame"
    );
}

#[test]
fn actual_composite_admission_retires_previous_plex_metadata_and_fences_late_detail() {
    let _serial = plx_base::testlock::serial();
    let mut rig = setup();
    let sid = plx_plex::plex::ServerId::UNSET;
    let mut leaf = plx_data::metadata::PlayingItem::with_subs(vec![]);
    leaf.sid = sid;
    leaf.rk = "previous_episode".into();
    leaf.kind = "episode".into();
    rig.app
        .bridge
        .metadata_run(plx_data::stores::metadata::MetadataCmd::InstallPlaying(
            Some(leaf),
        ));
    rig.app
        .bridge
        .metadata_run(plx_data::stores::metadata::MetadataCmd::SetNowPlaying(
            Some(plx_data::metadata::NowPlaying {
                ott: None,
                is_episode: true,
                is_real_episode: true,
                title: "Previous Plex show".into(),
                ep_title: "Previous episode".into(),
                season: 2,
                index: 3,
                summary: "Previous programme summary".into(),
                year: 2026,
                dur_ms: 90_000,
                rating: "fixture_rating".into(),
                thumb: "/previous_poster".into(),
                detail_rk: "previous_show".into(),
            }),
        ));
    assert_eq!(
        rig.app.bridge.metadata_view().now_playing().unwrap().title,
        "Previous Plex show"
    );
    let adapter = rig.app.bridge.metadata_mut().adapter_for_test();
    let generation = plx_data::metadata::begin_detail_for_test(&adapter, sid, "previous_episode");
    let mut rig = admit_rig(rig);
    assert_composite_metadata(&rig.app);
    assert!(foreign_native_start_allowed(&mut rig.app));
    {
        let (state, owning_adapter) = rig.app.bridge.metadata_mut().split_for_test();
        assert!(Arc::ptr_eq(owning_adapter, &adapter));
        assert!(plx_data::metadata::land_detail_for_test(
            state,
            owning_adapter,
            sid,
            "previous_episode",
            generation,
            Some(plx_data::metadata::Detail {
                sid,
                rk: "previous_episode".into(),
                kind: "movie".into(),
                title: "Late previous film".into(),
                summary: "Late previous summary".into(),
                // Empty GUID prevents any alternate-source lookup worker.
                ..Default::default()
            })
        ));
    }
    assert_eq!(
        rig.app.bridge.metadata_view().current().unwrap().title,
        "Late previous film",
        "the actual Detail landed; it was not silently discarded"
    );
    assert_composite_metadata(&rig.app);
    let mut runtime = rig.app.ott.take().unwrap();
    publish_programme(&mut runtime, &mut rig.app, true);
    rig.app.ott = Some(runtime);
    assert_composite_metadata(&rig.app);
    assert!(rig.app.adapters.player.release_has_queued_start());
    assert!(!rig.app.adapters.player.is_live());
}

#[test]
fn actual_output_shutdown_cancels_inspection_and_waits_for_owned_task_retirement() {
    let _serial = plx_base::testlock::serial();
    let mut rig = setup();
    queue_layout(&mut rig);
    assert_eq!(rig.inspected.load(Ordering::Acquire), 0);
    let mut runtime = rig.app.ott.take().unwrap();
    multiview_output::begin_shutdown(&mut runtime, &mut rig.app);
    assert!(multiview_output::shutdown_pending(&mut runtime));
    assert!(!route::has_url(&rig.app.player.session));
    finish_jobs(&rig.output_jobs);
    assert!(!multiview_output::shutdown_pending(&mut runtime));
    multiview_output::tick(&mut runtime, &mut rig.app);
    rig.app.ott = Some(runtime);
    assert_eq!(
        rig.inspected.load(Ordering::Acquire),
        0,
        "cancelled job must not inspect media"
    );
    assert!(rig.output_jobs.lock().unwrap().is_empty());
    assert!(!rig.app.adapters.player.is_live());
    assert!(!rig.app.adapters.player.release_has_queued_start());
}

fn tick_real_driver_and_output(rig: &mut Rig) {
    let mut runtime = rig.app.ott.take().unwrap();
    let mut driver = std::mem::take(&mut runtime.multiview_driver);
    driver.tick(&mut runtime, &mut rig.app);
    let ready = driver.take_ready_output(&runtime);
    runtime.multiview_driver = driver;
    if let Some(ready) = ready {
        multiview_output::offer(&mut runtime, &mut rig.app, ready).unwrap();
    }
    multiview_output::tick(&mut runtime, &mut rig.app);
    rig.app.ott = Some(runtime);
}
#[test]
fn admitted_picture_foreign_successor_retires_original_service_without_reoffering() {
    let _serial = plx_base::testlock::serial();
    let mut rig = admitted();
    playback::replace_player_in_place(
        &mut rig.app.player,
        &mut rig.app.adapters.player,
        &mut rig.app.repause_at,
        &mut rig.app.pages,
    );
    route::request_external_source(
        &mut rig.app.player.session,
        &media(),
        ExternalSource::Hls("https://successor.invalid/new.m3u8".into()),
        "Ordinary successor",
        "",
        playback::start_caps(),
    )
    .unwrap();
    let identity = route::playback_identity(&rig.app.player.session);
    let source = route::external_source(&rig.app.player.session);
    let end = Instant::now() + Duration::from_secs(3);
    loop {
        assert!(Instant::now() < end, "original service Off must settle");
        tick_real_driver_and_output(&mut rig);
        assert_eq!(route::playback_identity(&rig.app.player.session), identity);
        assert_eq!(route::external_source(&rig.app.player.session), source);
        if rig.server.lock().unwrap().status.as_ref().unwrap().state
            == service::SessionState::Closed
        {
            // Consume the actual final closed response and cleanup barriers.
            for _ in 0..5 {
                tick_real_driver_and_output(&mut rig);
                std::thread::sleep(Duration::from_millis(1));
            }
            break;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(
        rig.server
            .lock()
            .unwrap()
            .operations
            .iter()
            .filter(|name| *name == "off")
            .count(),
        1
    );
    assert_eq!(
        rig.inspected.load(Ordering::Acquire),
        1,
        "old output must never be reoffered"
    );
    assert!(rig.output_jobs.lock().unwrap().is_empty());
    assert!(rig.app.ott.as_ref().unwrap().multiview_picture.is_none());
    assert!(!rig.app.adapters.player.is_live());
}
#[test]
fn owned_output_replacement_keeps_picture_until_inspected_successor_is_admitted() {
    let _serial = plx_base::testlock::serial();
    let mut rig = admitted();
    let old = route::playback_identity(&rig.app.player.session).unwrap();
    rig.server
        .lock()
        .unwrap()
        .status
        .as_mut()
        .unwrap()
        .output
        .as_mut()
        .unwrap()
        .output_generation = "output_successor".into();
    let end = Instant::now() + Duration::from_secs(3);
    while rig.output_jobs.lock().unwrap().is_empty() {
        assert!(
            Instant::now() < end,
            "actual status poll must offer replacement"
        );
        tick_real_driver_and_output(&mut rig);
        std::thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(
        route::playback_identity(&rig.app.player.session).as_ref(),
        Some(&old)
    );
    let old_source = route::external_source(&rig.app.player.session);
    assert!(route::has_url(&rig.app.player.session));
    tick_real_driver_and_output(&mut rig);
    assert_eq!(
        route::playback_identity(&rig.app.player.session).as_ref(),
        Some(&old)
    );
    assert_eq!(route::external_source(&rig.app.player.session), old_source);
    assert!(route::has_url(&rig.app.player.session));
    {
        let mut runtime = rig.app.ott.take().unwrap();
        assert!(
            matches!(
                multiview_output::native_start_admission(&mut runtime, &rig.app),
                Some(Err(_))
            ),
            "retaining the existing picture must not authorize a fresh Load from its stale proof"
        );
        rig.app.ott = Some(runtime);
    }
    assert!(!rig
        .server
        .lock()
        .unwrap()
        .operations
        .iter()
        .any(|op| op == "off"));
    finish_jobs(&rig.output_jobs);
    tick_real_driver_and_output(&mut rig);
    let new = route::playback_identity(&rig.app.player.session).unwrap();
    assert_ne!(new, old);
    assert!(foreign_native_start_allowed(&mut rig.app));
    tick_real_driver_and_output(&mut rig);
    assert!(!rig
        .server
        .lock()
        .unwrap()
        .operations
        .iter()
        .any(|op| op == "off"));
    assert_eq!(rig.inspected.load(Ordering::Acquire), 2);
    assert!(rig.app.adapters.player.release_has_queued_start());
    assert!(!rig.app.adapters.player.is_live());
}

#[test]
fn replacement_inspection_cannot_retain_picture_after_playback_policy_revocation() {
    let _serial = plx_base::testlock::serial();
    let mut rig = admitted();
    rig.server
        .lock()
        .unwrap()
        .status
        .as_mut()
        .unwrap()
        .output
        .as_mut()
        .unwrap()
        .output_generation = "output_successor".into();
    let end = Instant::now() + Duration::from_secs(3);
    while rig.output_jobs.lock().unwrap().is_empty() {
        assert!(Instant::now() < end, "actual poll must offer replacement");
        tick_real_driver_and_output(&mut rig);
        std::thread::sleep(Duration::from_millis(1));
    }
    assert!(route::has_url(&rig.app.player.session));
    rig.app.running = false;
    let mut runtime = rig.app.ott.take().unwrap();
    multiview_output::tick(&mut runtime, &mut rig.app);
    assert!(!route::has_url(&rig.app.player.session));
    assert!(runtime.multiview_picture.is_none());
    rig.app.ott = Some(runtime);
    finish_jobs(&rig.output_jobs);
    let mut runtime = rig.app.ott.take().unwrap();
    multiview_output::tick(&mut runtime, &mut rig.app);
    assert!(!route::has_url(&rig.app.player.session));
    assert!(runtime.multiview_picture.is_none());
    assert_eq!(
        rig.inspected.load(Ordering::Acquire),
        1,
        "cancelled successor must not inspect or revive the revoked picture"
    );
    assert!(!rig.app.adapters.player.is_live());
    rig.app.ott = Some(runtime);
}

fn assert_stopped_logical_target_retires_composite_service(exit: bool) {
    let mut rig = admitted();
    let identity = route::playback_identity(&rig.app.player.session).unwrap();
    let request = route::play_request_ticket();
    let worker = route::worker_ticket();
    if exit {
        playback::exit_player(
            &mut rig.app.player,
            &mut rig.app.adapters.player,
            &mut rig.app.repause_at,
            &mut rig.app.refresh_hubs,
            &mut rig.app.pages,
        );
        assert!(rig.app.player.pending_exit.is_some());
    } else {
        playback::replace_player_in_place(
            &mut rig.app.player,
            &mut rig.app.adapters.player,
            &mut rig.app.repause_at,
            &mut rig.app.pages,
        );
    }
    assert_eq!(
        route::playback_identity(&rig.app.player.session).as_ref(),
        Some(&identity),
        "real stop retains the logical retry target"
    );
    assert!(!route::has_url(&rig.app.player.session));
    assert!(
        worker != route::worker_ticket() || !route::play_request_is_current(request),
        "real stop must retire route tickets"
    );
    {
        let runtime = rig.app.ott.as_ref().unwrap();
        assert!(!multiview_output::admitted_route_current(
            runtime, &rig.app, &identity
        ));
        assert!(
            !multiview_output::owns_picture(runtime, &rig.app),
            "stopped composite must not suppress ordinary recovery"
        );
    }
    let end = Instant::now() + Duration::from_secs(3);
    while !rig
        .app
        .ott
        .as_ref()
        .unwrap()
        .multiview_driver
        .shutdown_confirmed()
    {
        assert!(
            Instant::now() < end,
            "stopped composite original Off must settle"
        );
        tick_real_driver_and_output(&mut rig);
        assert!(!route::has_url(&rig.app.player.session));
        std::thread::sleep(Duration::from_millis(1));
    }
    let settled_paths = rig.server.lock().unwrap().paths.len();
    for _ in 0..10 {
        tick_real_driver_and_output(&mut rig);
        assert!(!route::has_url(&rig.app.player.session));
    }
    let server = rig.server.lock().unwrap();
    assert_eq!(
        server
            .operations
            .iter()
            .filter(|operation| *operation == "off")
            .count(),
        1
    );
    assert_eq!(
        server.paths.len(),
        settled_paths,
        "closed scene must not poll or renew"
    );
    assert_eq!(
        rig.inspected.load(Ordering::Acquire),
        1,
        "stopped scene must not reoffer output"
    );
    assert!(rig.output_jobs.lock().unwrap().is_empty());
    assert!(rig.app.ott.as_ref().unwrap().multiview_picture.is_none());
    assert!(!rig.app.adapters.player.is_live());
    assert!(!rig.app.adapters.player.release_has_queued_start());
}
#[test]
fn real_route_stop_retires_service_despite_retained_logical_identity() {
    let _serial = plx_base::testlock::serial();
    assert_stopped_logical_target_retires_composite_service(false);
}
#[test]
fn real_player_exit_retires_service_despite_retained_logical_identity() {
    let _serial = plx_base::testlock::serial();
    assert_stopped_logical_target_retires_composite_service(true);
}
