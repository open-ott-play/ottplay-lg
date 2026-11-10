//! Post-admission evidence through real admission, failure cleanup and status ingress.
//! Only decoder resources and terminal native signals are modeled. No resolver,
//! decoder, network, protected writer or device operation is started.
//! The nullable-context regression uses base APIs; typed-cause cases use the
//! native diagnostic API without starting a producer.
use super::*;
use plx_media::player::{report::IoFailureDetails, FailureKind, PlaybackState};
use plx_telemetry::telemetry::classes::{DemuxExitClass, TraceOutcome, TransportClass};

const PRIVATE_URL: &str = "https://diag.invalid/media?token=diag-token";
const PRIVATE_ERROR: &str = "provider refused private-diagnostic-title at https://diag.invalid/private?token=diag-token";

struct ResetMedia;
impl Drop for ResetMedia {
    fn drop(&mut self) {
        plx_media::player::SHARED.reset_session();
        plx_media::player::TX.reset();
        plx_media::route::reset_player_control_for_test(&plx_media::route::PlaybackSession::IDLE);
    }
}

#[derive(Default)]
struct AcceptedResources { starts: usize }
impl playback::PlaybackResources for AcceptedResources {
    fn request_movie(&mut self, _: &mut plx_media::route::PlaybackSession,
        _: &mut plx_data::stores::metadata::MetadataStore, _: &plx_data::pms::PmsMovie,
        _: Option<plx_plex::plex::MediaId>) -> bool { panic!("external source only") }
    fn request_episode(&mut self, _: &mut plx_media::route::PlaybackSession,
        _: &mut plx_data::stores::metadata::MetadataStore, _: &str,
        _: Option<plx_plex::plex::MediaId>) -> bool { panic!("external source only") }
    fn describe_movie(&mut self, _: &mut plx_data::stores::metadata::MetadataStore,
        _: plx_plex::plex::ServerId, _: &str) { panic!("external source only") }
    fn prepare_start(&mut self, _: &mut plx_media::route::PlaybackSession,
        _: &mut plx_media::player::adapter::PlayerAdapter, _: i64) -> bool {
        self.starts += 1;
        true
    }
}

fn profile() -> ProviderProfile {
    ProviderProfile {
        id: "private-diagnostic-profile".into(), name: "private-diagnostic-name".into(),
        kind: ProviderKind::VPortal,
        source: "portal::[key:diag-token]https://diag.invalid/api".into(),
        source_alias: "private-diagnostic-alias".into(), enabled: true,
    }
}

fn entry(id: &str) -> CatalogEntry {
    CatalogEntry {
        synopsis: None,
        id: id.into(), title: "private-diagnostic-title".into(), category: String::new(),
        description: String::new(), poster: String::new(), duration_ms: Some(10_000),
        media: MediaLocator::VPortal { request: json!({"id": id}) },
        provider_id: Some(id.into()), provider_id_field: Some("id".into()), live: None,
    }
}

fn fixture() -> Box<App> {
    plx_media::player::SHARED.reset_session();
    plx_media::player::TX.reset();
    let mut app = Box::new(ready_app_fixture());
    let runtime = app.ott.as_mut().unwrap();
    runtime.saved.profiles.profiles = vec![profile()];
    runtime.saved.profiles.active_profile = Some(profile().id);
    runtime.provider = ProviderKind::VPortal;
    runtime.catalog = Catalog { entries: vec![entry("first"), entry("second")], hierarchy: None };
    runtime.catalog_loading = false;
    runtime.catalog_complete = true;
    runtime.catalog_done.store(false, Ordering::Release);
    runtime.metadata_done.store(false, Ordering::Release);
    runtime.resolver_busy = true;
    runtime.plex_refresh_pending = true;
    runtime.write_permit.revoke();
    runtime.continuity.suppress_unrequested_start();
    app
}

fn queue(app: &mut App, id: &str) -> u64 {
    let mut runtime = app.ott.take().unwrap();
    runtime.resolver_busy = true;
    let mut request = PlaybackRequest::ordinary(id, 0);
    request.advance_on_failure = false;
    runtime.play_request_with_policy(app, request).unwrap();
    // Occupy the actual resolver slot, then model its taking this exact request.
    let queued = runtime.resolver_next.take().expect("admitted resolver request");
    assert_eq!(queued.generation, runtime.play_generation);
    let generation = queued.generation;
    app.ott = Some(runtime);
    generation
}

fn admit(app: &mut App, id: &str) -> u64 {
    let generation = queue(app, id);
    let mut info = unsupported_info();
    info.video_codec = "h264".into(); info.audio_codec = "aac".into();
    info.audio_channels = 2; info.audio_sample_rate = 48_000; info.audio_bit_depth = 16;
    app.ott.as_ref().unwrap().tx.send(Job::Play(generation, profile(), Ok((entry(id),
        ReadySource::External(plx_media::ff::ExternalSource::Http(PRIVATE_URL.into()), info, None)))))
        .unwrap_or_else(|_| panic!("fixture receiver disappeared"));
    let clock = plx_base::task::time::ClockSample::capture_main(app.adapters.player.mt()).unwrap();
    let mut resources = AcceptedResources::default();
    tick_with_start(app, clock, |runtime, app, profile, entry, source|
        start_resolved_with(runtime, app, profile, entry, source, &mut resources));
    assert_eq!(resources.starts, 1, "real admission must reach only the injected resource seam");
    let runtime = app.ott.as_ref().unwrap();
    assert!(runtime.play_target.is_none());
    let playing = runtime.playing.as_ref().expect("real start committed Playing");
    assert!(playing.identity.is_some());
    assert!(playing_matches(playing, app));
    assert!(!app.adapters.player.is_live(), "no native Engine is started by this fixture");
    assert_eq!(json!(runtime.preplay.snapshot(runtime.now_ms()))["phase"], "admitted");
    generation
}

fn signal_error(app: &App, kind: FailureKind) {
    let shared = &plx_media::player::SHARED;
    shared.demux_failed.store(kind == FailureKind::MediaSource, Ordering::Release);
    shared.demux_io_failed.store(kind == FailureKind::PlaybackInterrupted, Ordering::Release);
    shared.load_failed.store(matches!(kind, FailureKind::TvPipeline | FailureKind::LoadTimeout), Ordering::Release);
    shared.load_timed_out.store(kind == FailureKind::LoadTimeout, Ordering::Release);
    plx_media::player::swap_state_for_test(PlaybackState::Error);
    assert_eq!(plx_media::player::state(&app.player.session), PlaybackState::Error);
    assert_eq!(plx_media::player::error_now(&app.player.session).kind, kind,
        "the code must come from the real native error classifier");
}

// The existing inert Engine is attached to a REAL accepted route-start attempt
// and native callback epoch. Raw SHARED flags or logical identity alone are not
// evidence that the current attempt owns native diagnostics.
fn install_owned_engine(app: &mut App) -> u32 {
    use plx_media::{player, route};
    assert_eq!(unsafe {
        plx_platform::tv::sink::VideoSink::ready(&player::ffi_host::HostSink, app.adapters.player.mt())
    }, 0, "no loaded host object: cleanup must not launch an Unload worker");
    let transaction = route::pending_route_start().expect("real prepared route");
    let attempt = route::claim_route_start_attempt(transaction).expect("current native attempt");
    assert!(route::settle_route_start(&mut app.player.session, attempt, route::RouteStartResult::Started));
    let epoch = player::SHARED.begin_native_session().expect("native callback epoch");
    assert!(player::SHARED.mark_native_load_returned(epoch, None).is_some());
    let mut engine = player::engine::engine_after_reload_for_test();
    engine.route_start = attempt;
    engine.native_epoch = epoch;
    app.adapters.player.install(engine);
    assert!(playing_matches(app.ott.as_ref().unwrap().playing.as_ref().unwrap(), app));
    assert_eq!(player::SHARED.frames.load(Ordering::Acquire), 0);
    epoch
}

fn poison_unowned_details() {
    plx_media::player::SHARED.note_demux_exit(DemuxExitClass::NoVideoStream);
    plx_media::player::SHARED.dg_cb_err_num.store(601, Ordering::Release);
}

fn reconcile(app: &mut App) {
    let mut runtime = app.ott.take().unwrap();
    runtime.reconcile_failure(app);
    app.ott = Some(runtime);
}

fn read(app: &mut App, action: &str, serial: u64) -> Value {
    super::provider_context::read_status(app, action, serial)
}

fn read_params(app: &mut App, action: &str, serial: u64, params: Value) -> Value {
    let mut runtime = app.ott.take().unwrap();
    let saved = serde_json::to_value(&runtime.saved).unwrap();
    let presence = state(app, &runtime);
    let source = (runtime.generation, runtime.catalog_revision, runtime.navigation_epoch,
        runtime.saving, runtime.catalog_loading);
    assert!(runtime.resolver_next.is_none());
    let now = runtime.now_ms();
    let body = json!({"commands":[], "server_time":1000, "requests":[{
        "id":format!("{serial:032x}"), "action":action, "params":params, "expires_at":1020
    }]}).to_string();
    let mut incoming = runtime.remote.raw_poll_for_test(&body, now);
    assert_eq!(incoming.len(), 1);
    let invocation = incoming.pop().unwrap();
    assert!(runtime.remote.invocation_is_current(&invocation));
    let response = commands::command(&mut runtime, app, &invocation)
        .expect("diagnostic queries return synchronously");
    assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), saved);
    assert_eq!(state(app, &runtime), presence);
    assert_eq!((runtime.generation, runtime.catalog_revision, runtime.navigation_epoch,
        runtime.saving, runtime.catalog_loading), source);
    assert!(runtime.resolver_next.is_none());
    runtime.remote.complete(invocation, response.clone());
    app.ott = Some(runtime);
    response
}

fn read_v3(app: &mut App, action: &str, serial: u64) -> Value {
    let response = read_params(app, action, serial, json!({"playback_failure_version":3}));
    assert_eq!(response["status"], "ok");
    response
}

fn failure(reply: &Value) -> &Value {
    reply["data"].get("playback_failure").expect("status must include playback_failure, even when null")
}

fn assert_failure(reply: &Value, generation: u64, kind: FailureKind) {
    assert_failure_context(reply, generation, kind, None);
}

fn assert_failure_context(reply: &Value, generation: u64, kind: FailureKind,
    context: Option<(&str, i32)>) {
    let observed = failure(reply);
    let context = context.map(|(demux_exit, pipeline_error)| json!({"demux_exit":demux_exit, "pipeline_error":pipeline_error}));
    assert_eq!(observed, &json!({"version":2, "play_generation":generation, "code":kind.code(), "context":context}));
    assert!(observed["play_generation"].as_u64().is_some());
    let keys: std::collections::BTreeSet<_> = observed.as_object().unwrap().keys().map(String::as_str).collect();
    assert_eq!(keys, ["version", "play_generation", "code", "context"].into_iter().collect());
}

#[test]
fn native_failure_survives_real_cleanup_with_original_admitted_generation() {
    let _serial = plx_base::testlock::serial();
    let _reset = ResetMedia;
    for kind in [FailureKind::MediaSource, FailureKind::PlaybackInterrupted,
        FailureKind::TvPipeline, FailureKind::LoadTimeout, FailureKind::Unspecified] {
        let mut app = fixture();
        let generation = admit(&mut app, "first");
        let runtime = app.ott.as_mut().unwrap();
        let saved = serde_json::to_value(&runtime.saved).unwrap();
        let preplay = json!(runtime.preplay.snapshot(runtime.now_ms()));
        runtime.cancel_play_request();
        assert_ne!(runtime.play_generation, generation, "generic cancellation advanced the counter");
        assert!(playing_matches(app.ott.as_ref().unwrap().playing.as_ref().unwrap(), &app));
        signal_error(&app, kind);
        reconcile(&mut app);
        let runtime = app.ott.as_ref().unwrap();
        assert!(runtime.playing.is_none());
        assert!(runtime.active_lease.is_none());
        assert!(!plx_media::route::has_url(&app.player.session));
        assert_eq!(plx_media::player::state(&app.player.session), PlaybackState::Idle);
        assert_eq!(runtime.status, "Native playback failed");
        assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), saved);
        assert_eq!(json!(runtime.preplay.snapshot(runtime.now_ms())), preplay);
        let status = read(&mut app, "status", 1);
        assert_failure(&status, generation, kind);
        assert_eq!(status["data"]["playback"]["state"], "idle");
        assert_eq!(status["data"]["playback"]["kind"], "none");
        reconcile(&mut app); // Idle cleanup/read must not erase retained evidence.
        assert_failure(&read(&mut app, "state", 2), generation, kind);
    }
}

#[test]
fn fresh_accepted_request_clears_failure_before_new_resolver_completes() {
    let _serial = plx_base::testlock::serial();
    let _reset = ResetMedia;
    let mut app = fixture();
    let first = admit(&mut app, "first");
    signal_error(&app, FailureKind::TvPipeline);
    reconcile(&mut app);
    assert_failure(&read(&mut app, "status", 1), first, FailureKind::TvPipeline);
    let next = queue(&mut app, "second");
    assert_ne!(first, next);
    assert!(failure(&read(&mut app, "state", 2)).is_null());
    assert!(app.ott.as_ref().unwrap().play_target.is_some());
}

#[test]
fn pending_replacement_cannot_capture_old_native_error_as_the_new_attempt() {
    let _serial = plx_base::testlock::serial();
    let _reset = ResetMedia;
    let mut app = fixture();
    let first = admit(&mut app, "first");
    let next = queue(&mut app, "second");
    assert_ne!(first, next);
    poison_unowned_details();
    signal_error(&app, FailureKind::TvPipeline);
    reconcile(&mut app);
    assert!(failure(&read(&mut app, "status", 1)).is_null());
    // Even after generic retirement drops B's pending fields, A must not borrow B.
    app.ott.as_mut().unwrap().cancel_play_request();
    reconcile(&mut app);
    assert!(failure(&read(&mut app, "state", 2)).is_null());
}

#[test]
fn unowned_native_error_is_not_attributed_to_the_admitted_ott_source() {
    let _serial = plx_base::testlock::serial();
    let _reset = ResetMedia;
    let mut app = fixture();
    admit(&mut app, "first");
    app.ott.as_mut().unwrap().playing.as_mut().unwrap().identity = None;
    assert!(!playing_matches(app.ott.as_ref().unwrap().playing.as_ref().unwrap(), &app));
    poison_unowned_details();
    signal_error(&app, FailureKind::TvPipeline);
    reconcile(&mut app);
    assert!(failure(&read(&mut app, "status", 1)).is_null());
    assert!(app.ott.as_ref().unwrap().playing.is_some(), "existing ownership guard must still refuse cleanup");
}

#[test]
fn retained_failure_does_not_follow_a_different_native_playback_owner() {
    let _serial = plx_base::testlock::serial();
    let _reset = ResetMedia;
    let mut app = fixture();
    let generation = admit(&mut app, "first");
    let original = plx_media::route::playback_identity(&app.player.session).unwrap();
    signal_error(&app, FailureKind::TvPipeline);
    reconcile(&mut app);
    assert_failure(&read(&mut app, "status", 1), generation, FailureKind::TvPipeline);

    let mut info = unsupported_info();
    info.video_codec = "h264".into(); info.audio_codec = "aac".into();
    info.audio_channels = 2; info.audio_sample_rate = 48_000; info.audio_bit_depth = 16;
    let replacement = plx_media::ff::ExternalSource::Http(
        "https://diag.invalid:443/replacement?token=other-token".into());
    plx_media::route::request_external_source(&mut app.player.session, &info,
        replacement.clone(), "Another native owner", "",
        plx_platform::devcaps::assumed_shared()).unwrap();
    let current = plx_media::route::playback_identity(&app.player.session).unwrap();
    assert_ne!(original, current, "the real route API must mint a different playback owner");
    assert!(!app.adapters.player.is_live());
    poison_unowned_details();
    signal_error(&app, FailureKind::MediaSource);
    runtime_tick(&mut app);
    assert!(failure(&read(&mut app, "state", 2)).is_null(),
        "old OTT failure must not be reported as the new native owner's error");
    assert_eq!(plx_media::route::external_source(&app.player.session), Some(replacement));
    assert_eq!(plx_media::player::state(&app.player.session), PlaybackState::Error);
    assert!(app.ott.as_ref().unwrap().playing.is_none());
}

#[test]
fn retained_failure_is_closed_and_does_not_change_legacy_status_or_presence() {
    let _serial = plx_base::testlock::serial();
    let _reset = ResetMedia;
    let mut app = fixture();
    let idle = read(&mut app, "status", 1);
    assert!(failure(&idle).is_null(), "ordinary idle is not a playback failure");
    let generation = admit(&mut app, "first");
    poison_unowned_details();
    plx_media::route::refuse_for_test(&mut app.player.session,
        plx_media::route::PlayVerdict::Server(PRIVATE_ERROR.into(), Default::default()));
    assert_eq!(plx_media::player::state(&app.player.session), PlaybackState::Error);
    let shape = plx_media::player::error_now(&app.player.session);
    assert_eq!(shape.kind, FailureKind::DecisionRefused);
    assert!(shape.detail.contains("diag-token"), "private text really reaches the native classifier");
    reconcile(&mut app);
    app.ott.as_mut().unwrap().status = PRIVATE_ERROR.into();
    let runtime = app.ott.as_ref().unwrap();
    let saved = serde_json::to_value(&runtime.saved).unwrap();
    let preplay = json!(runtime.preplay.snapshot(runtime.now_ms()));
    let presence = state(&app, runtime); // Exact projection passed to Remote::tick.
    assert!(presence.get("playback_failure").is_none());
    assert!(presence.get("playback_error").is_none());
    assert!(presence.get("preplay").is_none());
    let reply = read(&mut app, "state", 2);
    assert_failure(&reply, generation, FailureKind::DecisionRefused);
    assert_eq!(reply["data"]["playback"], presence["playback"]);
    assert_eq!(reply["data"]["preplay"], preplay);
    assert!(reply["data"].get("playback_error").is_none());
    for secret in [PRIVATE_URL, PRIVATE_ERROR, "diag-user", "diag-password", "diag-token",
        "diag.invalid", "private-diagnostic-profile", "private-diagnostic-name",
        "private-diagnostic-alias", "private-diagnostic-title"] {
        assert!(!reply.to_string().contains(secret), "diagnostic status must not expose private inputs");
    }
    let runtime = app.ott.as_ref().unwrap();
    assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), saved);
    assert_eq!(state(&app, runtime), presence, "explicit status is not a periodic-presence mutation");
    assert_eq!(json!(runtime.preplay.snapshot(runtime.now_ms())), preplay);
}

#[test]
fn zero_frame_owned_failure_retains_first_details_through_cleanup_and_counter_drift() {
    let _serial = plx_base::testlock::serial();
    let _reset = ResetMedia;
    for (kind, demux, pipeline_error) in [
        (FailureKind::MediaSource, DemuxExitClass::HttpsOpenFailed, 0),
        (FailureKind::TvPipeline, DemuxExitClass::None, 601),
        (FailureKind::Unspecified, DemuxExitClass::None, 0),
    ] {
        let mut app = fixture();
        let generation = admit(&mut app, "first");
        let epoch = install_owned_engine(&mut app);
        if demux != DemuxExitClass::None {
            plx_media::player::SHARED.fail_demux(demux);
            plx_media::player::SHARED.fail_demux(DemuxExitClass::NoAccessUnits);
        }
        if pipeline_error != 0 {
            plx_media::player::sf_on_event(epoch, 18, i64::from(pipeline_error), std::ptr::null());
        }
        let runtime = app.ott.as_mut().unwrap();
        let saved = serde_json::to_value(&runtime.saved).unwrap();
        let preplay = json!(runtime.preplay.snapshot(runtime.now_ms()));
        runtime.cancel_play_request();
        assert_ne!(runtime.play_generation, generation, "the later counter is not the admitted generation");
        signal_error(&app, kind);
        assert_eq!(plx_media::player::SHARED.frames.load(Ordering::Acquire), 0);
        reconcile(&mut app);
        let runtime = app.ott.as_ref().unwrap();
        assert!(runtime.playing.is_none());
        assert!(!app.adapters.player.is_live());
        assert!(!plx_media::route::has_url(&app.player.session));
        assert_eq!(plx_media::player::state(&app.player.session), PlaybackState::Idle);
        assert_eq!(runtime.status, "Native playback failed");
        assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), saved);
        assert_eq!(json!(runtime.preplay.snapshot(runtime.now_ms())), preplay);
        assert_eq!(plx_media::player::SHARED.dg_demux_exit.load(Ordering::Acquire), 0);
        assert_eq!(plx_media::player::SHARED.dg_cb_err_num.load(Ordering::Acquire), 0);
        assert_failure_context(&read(&mut app, "status", 1), generation, kind,
            Some((demux.code(), pipeline_error)));
        poison_unowned_details();
        reconcile(&mut app);
        assert_failure_context(&read(&mut app, "state", 2), generation, kind,
            Some((demux.code(), pipeline_error)));
        let next = queue(&mut app, "second");
        assert_ne!(generation, next);
        assert!(failure(&read(&mut app, "status", 3)).is_null(), "accepted replacement clears both kind and details");
    }
}

#[test]
fn engine_less_error_does_not_borrow_nonzero_native_details() {
    let _serial = plx_base::testlock::serial();
    let _reset = ResetMedia;
    let mut app = fixture();
    let generation = admit(&mut app, "first");
    assert!(!app.adapters.player.is_live());
    poison_unowned_details();
    signal_error(&app, FailureKind::MediaSource);
    reconcile(&mut app);
    assert_failure(&read(&mut app, "status", 1), generation, FailureKind::MediaSource);
}

#[test]
fn stale_play_result_cannot_restore_context_cleared_by_new_request() {
    let _serial = plx_base::testlock::serial();
    let _reset = ResetMedia;
    let mut app = fixture();
    let first = admit(&mut app, "first");
    install_owned_engine(&mut app);
    plx_media::player::SHARED.fail_demux(DemuxExitClass::HttpsOpenFailed);
    signal_error(&app, FailureKind::MediaSource);
    reconcile(&mut app);
    assert_failure_context(&read(&mut app, "status", 1), first, FailureKind::MediaSource,
        Some(("https_open_failed", 0)));
    let next = queue(&mut app, "second");
    assert_ne!(first, next);
    let snapshot_at = app.ott.as_ref().unwrap().now_ms();
    let pending = json!(app.ott.as_ref().unwrap().preplay.snapshot(snapshot_at));
    assert!(failure(&read(&mut app, "status", 2)).is_null());
    app.ott.as_ref().unwrap().tx.send(Job::Play(first, profile(), Err(PRIVATE_ERROR.into())))
        .unwrap_or_else(|_| panic!("fixture receiver disappeared"));
    runtime_tick(&mut app);
    let runtime = app.ott.as_ref().unwrap();
    assert_eq!(runtime.play_generation, next);
    assert_eq!(runtime.play_target.as_ref().unwrap().id, "second");
    assert!(runtime.resolver_next.is_none(), "the fixture models the new worker taking its request");
    assert_eq!(json!(runtime.preplay.snapshot(snapshot_at)), pending);
    assert!(!app.adapters.player.is_live());
    assert!(failure(&read(&mut app, "state", 3)).is_null());
}

#[test]
fn native_failure_v3_includes_nullable_io_context_after_cleanup() {
    let _serial = plx_base::testlock::serial();
    let _reset = ResetMedia;
    let mut app = fixture();
    let generation = admit(&mut app, "first");
    let epoch = install_owned_engine(&mut app);
    plx_media::player::sf_on_event(epoch, 18, 601, std::ptr::null());
    signal_error(&app, FailureKind::TvPipeline);
    reconcile(&mut app);
    assert!(app.ott.as_ref().unwrap().playing.is_none());
    assert!(!app.adapters.player.is_live());
    assert_eq!(plx_media::player::state(&app.player.session), PlaybackState::Idle);
    let expected = json!({
        "version": 3,
        "play_generation": generation,
        "code": FailureKind::TvPipeline.code(),
        "context": {"demux_exit": "none", "pipeline_error": 601, "io_failure": null},
    });
    assert_eq!(failure(&read_v3(&mut app, "status", 1)), &expected);
    assert_eq!(failure(&read_v3(&mut app, "state", 2)), &expected);
}


#[test]
fn owned_io_failure_v3_keeps_closed_cause_after_cleanup() {
    let _serial = plx_base::testlock::serial();
    let _reset = ResetMedia;
    for (detail, expected_io) in [
        (IoFailureDetails::SourceIo { transport: TransportClass::None },
            json!({"source_io":{"transport":"none"}})),
        (IoFailureDetails::SourceIo { transport: TransportClass::RecvError },
            json!({"source_io":{"transport":"recv_error"}})),
        (IoFailureDetails::Hls { exit: DemuxExitClass::HlsFailed, outcome: None },
            json!({"hls":{"exit":"hls_failed","outcome":null}})),
        (IoFailureDetails::Hls { exit: DemuxExitClass::HlsNotReady, outcome: None },
            json!({"hls":{"exit":"hls_not_ready","outcome":null}})),
        (IoFailureDetails::Hls { exit: DemuxExitClass::HlsShortBody, outcome: None },
            json!({"hls":{"exit":"hls_short_body","outcome":null}})),
        (IoFailureDetails::Hls { exit: DemuxExitClass::Other, outcome: Some(TraceOutcome::Deadline) },
            json!({"hls":{"exit":"other","outcome":"deadline"}})),
        (IoFailureDetails::Hls { exit: DemuxExitClass::HlsTransportOpenFailed, outcome: Some(TraceOutcome::Transport) },
            json!({"hls":{"exit":"hls_transport_open_failed","outcome":"transport"}})),
        (IoFailureDetails::Hls { exit: DemuxExitClass::HlsTransportFailed, outcome: Some(TraceOutcome::ServerState) },
            json!({"hls":{"exit":"hls_transport_failed","outcome":"server_state"}})),
        (IoFailureDetails::Hls { exit: DemuxExitClass::HlsTransportOpenFailed, outcome: Some(TraceOutcome::Refused) },
            json!({"hls":{"exit":"hls_transport_open_failed","outcome":"refused"}})),
        (IoFailureDetails::Hls { exit: DemuxExitClass::HlsTransportFailed, outcome: Some(TraceOutcome::Inconclusive) },
            json!({"hls":{"exit":"hls_transport_failed","outcome":"inconclusive"}})),
        // The public diagnostic enums are wider than this closed wire contract.
        (IoFailureDetails::Hls { exit: DemuxExitClass::NoVideoStream, outcome: Some(TraceOutcome::Started) },
            json!({"hls":{"exit":"other","outcome":null}})),
        (IoFailureDetails::AutoFallback, json!({"auto_fallback":{"outcome":"refused"}})),
        (IoFailureDetails::AudioDecode, json!({"audio_decode":{"outcome":"inconclusive"}})),
        (IoFailureDetails::Unclassified, json!({"unclassified":{"outcome":"inconclusive"}})),
    ] {
        let mut app = fixture();
        let generation = admit(&mut app, "first");
        let epoch = install_owned_engine(&mut app);
        let shared = &plx_media::player::SHARED;
        assert!(shared.note_io_failure_for(epoch, detail));
        assert!(!shared.note_io_failure_for(epoch, IoFailureDetails::AutoFallback),
            "the first complete cause must survive a later observation");
        signal_error(&app, FailureKind::PlaybackInterrupted);
        reconcile(&mut app);
        assert!(!app.adapters.player.is_live());
        assert_eq!(plx_media::player::state(&app.player.session), PlaybackState::Idle);
        assert!(!shared.demux_io_failed.load(Ordering::Acquire), "cleanup clears the live producer-error signal");
        let expected = json!({
            "version":3, "play_generation":generation, "code":"playback_interrupted",
            "context":{"demux_exit":"none", "pipeline_error":0, "io_failure":expected_io},
        });
        assert_failure_context(&read(&mut app, "status", 4), generation,
            FailureKind::PlaybackInterrupted, Some(("none", 0)));
        assert_eq!(failure(&read_v3(&mut app, "status", 1)), &expected);
        poison_unowned_details();
        reconcile(&mut app);
        assert_eq!(failure(&read_v3(&mut app, "state", 2)), &expected,
            "a later read must use the captured cause, not process-wide native state");
        assert_failure_context(&read(&mut app, "state", 5), generation,
            FailureKind::PlaybackInterrupted, Some(("none", 0)));
        let next = queue(&mut app, "second");
        assert_ne!(generation, next);
        assert!(failure(&read_v3(&mut app, "status", 3)).is_null());
    }
}


#[test]
fn playback_failure_version_is_explicit_and_validated() {
    let _serial = plx_base::testlock::serial();
    let _reset = ResetMedia;
    let mut app = fixture();
    let capability = read_params(&mut app, "capabilities", 1, json!({}));
    assert_eq!(capability["status"], "ok");
    assert_eq!(capability["data"]["playback_failure_versions"], json!([2, 3]));
    assert!(failure(&read_v3(&mut app, "status", 2)).is_null());
    let generation = admit(&mut app, "first");
    signal_error(&app, FailureKind::MediaSource);
    reconcile(&mut app);
    let mut serial = 3;
    for action in ["status", "state"] {
        let legacy = read(&mut app, action, serial);
        serial += 1;
        assert_failure(&legacy, generation, FailureKind::MediaSource);
        let explicit = read_params(&mut app, action, serial, json!({"playback_failure_version":2}));
        serial += 1;
        assert_eq!(explicit["status"], "ok");
        assert_eq!(failure(&explicit), failure(&legacy));
        let detailed = read_v3(&mut app, action, serial);
        serial += 1;
        assert_eq!(failure(&detailed), &json!({"version":3, "play_generation":generation,
            "code":"media_source", "context":null}));
        for value in [json!(0), json!(1), json!(4), json!(-1), json!(3.0), json!(2.5),
            json!(true), json!(false), json!(null), json!("3"), json!([]), json!({})] {
            let response = read_params(&mut app, action, serial,
                json!({"playback_failure_version":value}));
            serial += 1;
            assert_eq!(response["status"], "rejected");
            assert!(response["data"].get("playback_failure").is_none());
        }
        for params in [json!({"unknown":3}), json!({"playback_failure_version":3,"unknown":true})] {
            assert_eq!(read_params(&mut app, action, serial, params)["status"], "rejected");
            serial += 1;
        }
        assert_eq!(failure(&read(&mut app, action, serial)), failure(&legacy),
            "version negotiation must not change the retained observation or default");
        serial += 1;
    }
}

#[path = "playback_eof_priority.rs"]
mod eof_priority;

#[path = "playback_video_evidence.rs"]
mod video_evidence;


fn make_live(app: &mut App) {
    let runtime = app.ott.as_mut().unwrap();
    let mut profile = profile();
    profile.kind = ProviderKind::M3u;
    profile.source = "https://diag.invalid/list.m3u".into();
    let mut channel = entry("first");
    channel.duration_ms = None;
    channel.live = Some(Default::default());
    channel.media = MediaLocator::Http { url: PRIVATE_URL.into() };
    runtime.saved.profiles.profiles = vec![profile.clone()];
    runtime.saved.profiles.active_profile = Some(profile.id.clone());
    runtime.provider = ProviderKind::M3u;
    runtime.catalog.entries[0] = channel.clone();
    let playing = runtime.playing.as_mut().unwrap();
    playing.profile = profile;
    playing.entry = channel;
    playing.advance_on_failure = false;
}
fn transient_hls() -> IoFailureDetails {
    IoFailureDetails::Hls { exit: DemuxExitClass::HlsTransportOpenFailed,
        outcome: Some(TraceOutcome::Transport) }
}
fn live_failure() -> Box<App> {
    let mut app = fixture();
    app.ott.as_mut().unwrap().saving = true;
    admit(&mut app, "first");
    make_live(&mut app);
    let epoch = install_owned_engine(&mut app);
    assert!(plx_media::player::SHARED.note_io_failure_for(epoch, transient_hls()));
    signal_error(&app, FailureKind::PlaybackInterrupted);
    reconcile(&mut app);
    // Real admission consumed its Job and freed the slot. Hold the next worker
    // in the fixture, as the queued/late-result tests supply completion by hand.
    app.ott.as_mut().unwrap().resolver_busy = true;
    app
}

#[test]
fn live_transport_recovery_requires_precise_closed_transient_cause() {
    use plx_media::player::report::FailureDetails;
    for exit in [DemuxExitClass::HlsTransportOpenFailed, DemuxExitClass::HlsTransportFailed,
        DemuxExitClass::HlsPlaylistInvalid, DemuxExitClass::Other, DemuxExitClass::NoVideoStream] {
        for outcome in [None, Some(TraceOutcome::Transport), Some(TraceOutcome::Deadline),
            Some(TraceOutcome::ServerState), Some(TraceOutcome::Refused),
            Some(TraceOutcome::Inconclusive), Some(TraceOutcome::Started), Some(TraceOutcome::NoBody)] {
            let expected = matches!(exit, DemuxExitClass::HlsTransportOpenFailed | DemuxExitClass::HlsTransportFailed)
                && matches!(outcome, Some(TraceOutcome::Transport | TraceOutcome::Deadline | TraceOutcome::ServerState));
            assert_eq!(recoverable_live_transport(Some(FailureDetails {
                demux_exit: exit, pipeline_error: 0,
                io_failure: Some(IoFailureDetails::Hls { exit, outcome }),
            })), expected, "{exit:?} {outcome:?}");
        }
    }
    assert!(!recoverable_live_transport(None));
}

#[test]
fn owned_live_transient_failure_retries_only_exact_channel_without_new_viewer_intent() {
    let _serial = plx_base::testlock::serial();
    let _reset = ResetMedia;
    let mut app = live_failure();
    let runtime = app.ott.as_mut().unwrap();
    let generation = runtime.generation;
    let play_generation = runtime.play_generation;
    let selected = runtime.selected.clone();
    assert!(runtime.playing.is_none());
    assert!(runtime.play_target.is_none() && runtime.resolver_next.is_none());
    let retry = runtime.continuity.pending_retry().expect("attributed transient live failure");
    assert_eq!(retry.after, "first");
    let due = retry.not_before_ms;
    assert!(matches!(runtime.retry_continuity(due - 1, &["first", "second"], true), continuity::Decision::Wait));
    let decision = runtime.retry_continuity(due, &["first", "second"], true);
    assert!(matches!(&decision, continuity::Decision::Start { entry_id, reason: continuity::Reason::LiveRetry, .. } if entry_id == "first"));
    runtime.apply_continuity(decision);
    assert_eq!(runtime.generation, generation);
    assert_ne!(runtime.play_generation, play_generation);
    assert_eq!(runtime.selected, selected);
    let request = runtime.play_target.as_ref().unwrap();
    assert_eq!(request.id, "first");
    assert!(!request.advance_on_failure);
    assert_eq!(runtime.resolver_next.as_ref().unwrap().generation, runtime.play_generation);
    // Automatic admission must not reset the shared bounded failure budget.
    assert_eq!(runtime.continuity.live_disconnected(generation, "first", due), continuity::Decision::Wait);
    assert_eq!(runtime.continuity.pending_retry().unwrap().failures, 2);
}

#[test]
fn live_retry_admission_refusal_never_falls_through_to_next_channel() {
    let _serial = plx_base::testlock::serial();
    let _reset = ResetMedia;
    let mut app = live_failure();
    let runtime = app.ott.as_mut().unwrap();
    let due = runtime.continuity.pending_retry().unwrap().not_before_ms;
    let decision = runtime.retry_continuity(due, &["first", "second"], true);
    runtime.catalog.entries.retain(|entry| entry.id != "first");
    runtime.apply_continuity(decision);
    assert!(runtime.play_target.is_none() && runtime.resolver_next.is_none());
    assert!(runtime.continuity.pending_retry().is_none());
    assert_eq!(runtime.continuity.tick(runtime.generation, u64::MAX, &["second"], true), continuity::Decision::Idle);
}

#[test]
fn live_failure_cannot_borrow_unowned_cause_or_recover_finite_video() {
    let _serial = plx_base::testlock::serial();
    let _reset = ResetMedia;
    for live in [false, true] {
        let mut app = fixture();
        app.ott.as_mut().unwrap().saving = true;
        admit(&mut app, "first");
        if live { make_live(&mut app); }
        if !live {
            let epoch = install_owned_engine(&mut app);
            assert!(plx_media::player::SHARED.note_io_failure_for(epoch, transient_hls()));
        } else {
            // Global evidence without this attempt's native Engine is unowned.
            let epoch = plx_media::player::SHARED.begin_native_session().unwrap();
            assert!(plx_media::player::SHARED.note_io_failure_for(epoch, transient_hls()));
        }
        signal_error(&app, FailureKind::PlaybackInterrupted);
        reconcile(&mut app);
        let runtime = app.ott.as_ref().unwrap();
        assert!(runtime.continuity.pending_retry().is_none());
        assert!(runtime.play_target.is_none() && runtime.resolver_next.is_none());
    }
}

#[test]
fn pause_after_live_failure_retires_cooldown_even_without_installed_playing() {
    let _serial = plx_base::testlock::serial();
    let _reset = ResetMedia;
    let mut app = live_failure();
    assert!(app.ott.as_ref().unwrap().continuity.pending_retry().is_some());
    assert!(app.ott.as_ref().unwrap().playing.is_none());
    app.adapters.player.install(plx_media::player::engine::engine_after_reload_for_test());
    assert!(plx_media::player::pause(&mut app.adapters.player));
    assert!(lifecycle::viewer_paused());
    let clock = plx_base::task::time::ClockSample::capture_main(app.adapters.player.mt()).unwrap();
    tick_with_start(&mut app, clock, |_, _, _, _, _| panic!("viewer Pause cannot launch a recovery"));
    let runtime = app.ott.as_mut().unwrap();
    assert!(runtime.continuity.pending_retry().is_none());
    assert_eq!(runtime.continuity.tick(runtime.generation, u64::MAX, &["first", "second"], true), continuity::Decision::Idle);
    assert!(runtime.play_target.is_none() && runtime.resolver_next.is_none());
}


fn issue_live_retry(app: &mut App) -> u64 {
    let runtime = app.ott.as_mut().unwrap();
    let due = runtime.continuity.pending_retry().unwrap().not_before_ms;
    let decision = runtime.retry_continuity(due, &["first", "second"], true);
    runtime.apply_continuity(decision);
    let generation = runtime.play_generation;
    assert_eq!(runtime.live_retry_generation, Some(generation));
    runtime.resolver_next.take().expect("worker takes automatic retry");
    generation
}
fn late_live_result(app: &App, generation: u64) {
    let runtime = app.ott.as_ref().unwrap();
    let profile = runtime.saved.profiles.profiles[0].clone();
    let entry = runtime.catalog.entries[0].clone();
    runtime.tx.send(Job::Play(generation, profile, Ok((entry,
        ReadySource::External(plx_media::ff::ExternalSource::Http(PRIVATE_URL.into()), unsupported_info(), None)))))
        .unwrap_or_else(|_| panic!("fixture receiver disappeared"));
}
fn accepted_pause(app: &mut App) {
    app.adapters.player.install(plx_media::player::engine::engine_after_reload_for_test());
    assert!(plx_media::player::pause(&mut app.adapters.player));
    assert!(lifecycle::viewer_paused());
}
#[test]
fn pause_after_live_retry_was_queued_rejects_late_resolver_completion() {
    let _serial = plx_base::testlock::serial();
    let _reset = ResetMedia;
    let mut app = live_failure();
    let generation = issue_live_retry(&mut app);
    late_live_result(&app, generation);
    accepted_pause(&mut app);
    let clock = plx_base::task::time::ClockSample::capture_main(app.adapters.player.mt()).unwrap();
    tick_with_start(&mut app, clock, |_, _, _, _, _| panic!("Pause must fence the late automatic resolver"));
    let runtime = app.ott.as_ref().unwrap();
    assert_ne!(runtime.play_generation, generation);
    assert!(runtime.live_retry_generation.is_none());
    assert!(runtime.play_target.is_none() && runtime.resolver_next.is_none());
    assert!(runtime.continuity.pending_retry().is_none());
}
#[test]
fn newer_explicit_selection_is_not_cancelled_by_old_live_retry_pause_receipt() {
    let _serial = plx_base::testlock::serial();
    let _reset = ResetMedia;
    let mut app = live_failure();
    let generation = issue_live_retry(&mut app);
    late_live_result(&app, generation);
    let mut runtime = app.ott.take().unwrap();
    let mut next = runtime.catalog.entries[0].clone();
    next.id = "second".into();
    runtime.catalog.entries[1] = next;
    runtime.play_request_with_policy(&mut app, PlaybackRequest::ordinary("second", 0)).unwrap();
    let selected_generation = runtime.play_generation;
    assert_ne!(selected_generation, generation);
    assert!(runtime.live_retry_generation.is_none());
    runtime.resolver_next.take().expect("new selected resolver is held by the fixture");
    app.ott = Some(runtime);
    accepted_pause(&mut app);
    let clock = plx_base::task::time::ClockSample::capture_main(app.adapters.player.mt()).unwrap();
    tick_with_start(&mut app, clock, |_, _, _, _, _| panic!("old resolver cannot start either selection"));
    let runtime = app.ott.as_ref().unwrap();
    assert_eq!(runtime.play_generation, selected_generation);
    assert_eq!(runtime.play_target.as_ref().unwrap().id, "second");
    assert!(runtime.live_retry_generation.is_none());
}

#[path = "player_startup_feedback.rs"]
mod startup_feedback;
