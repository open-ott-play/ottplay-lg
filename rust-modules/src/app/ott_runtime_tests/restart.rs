//! Restart admission must commit its actual source, not the pending saved-file target.
use super::*;

struct RestartFixture(App);
impl std::ops::Deref for RestartFixture {
    type Target = App;
    fn deref(&self) -> &App { &self.0 }
}
impl std::ops::DerefMut for RestartFixture {
    fn deref_mut(&mut self) -> &mut App { &mut self.0 }
}
impl Drop for RestartFixture {
    fn drop(&mut self) {
        // Use the normal typed stop to cancel any queued deferred native start.
        plx_media::player::stop_bufferfeed(&mut self.0.player.session, &mut self.0.adapters.player);
    }
}

fn restart_fixture() -> RestartFixture {
    let mut app = transport_fixture();
    app.running = true;
    assert!(lifecycle::set_transport_paused(&mut app.adapters.player, false));
    plx_media::player::swap_state_for_test(plx_media::player::PlaybackState::Playing);
    RestartFixture(app)
}

fn defer_native_restart(app: &mut App) {
    let _engine = app.adapters.player.take().expect("inert fixture Engine");
    app.adapters.player.begin_release_for_test(std::time::Duration::ZERO);
}

fn command(app: &mut App, action: &str, params: Value) -> Option<Value> {
    let mut runtime = app.ott.take().unwrap();
    let mut remote = Remote::disabled_for_test("dev_fixture", "fixture");
    let mut invocation = remote.request_for_test(action, params);
    invocation.deadline_ms = runtime.now_ms().saturating_add(10_000);
    let response = commands::command(&mut runtime, app, &invocation);
    app.ott = Some(runtime);
    response
}

fn remote_restart(app: &mut App) -> Value {
    command(app, "lifecycle", json!({"operation":"restart_stream"}))
        .expect("restart returns an admission receipt")
}

fn restart_offered(app: &mut App) -> bool {
    command(app, "capabilities", json!({})).unwrap()["data"]["lifecycle"]
        .as_array().unwrap().iter().any(|value| value == "restart_stream")
}

fn overdue_native_restart(app: &mut App) {
    let _engine = app.adapters.player.take().expect("inert fixture Engine");
    app.adapters.player.begin_release_for_test(std::time::Duration::from_secs(11));
}

#[test]
fn accepted_restart_commits_fallback_before_saved_file_arrives() {
    let _serial = plx_base::testlock::serial();
    let _reset = ResetMedia;
    let mut app = restart_fixture();
    defer_native_restart(&mut app);
    let response = remote_restart(&mut app);
    assert_eq!(response["status"], "ok", "real deferred restart must be admitted: {response}");
    assert_eq!(response["data"]["accepted"], true);
    assert!(app.adapters.player.release_has_queued_start(), "real native reload must be queued behind Unload");
    late_batch(&mut app);
    assert_first_kept(&app);
}

#[test]
fn accepted_restart_reanchors_eof_after_canceling_staged_restore() {
    let _serial = plx_base::testlock::serial();
    let _reset = ResetMedia;
    let mut app = restart_fixture();
    late_batch(&mut app);
    assert_eq!(app.ott.as_ref().unwrap().play_target, Some(PlaybackRequest::ordinary("late", 42_000)));
    defer_native_restart(&mut app);
    let response = remote_restart(&mut app);
    assert_eq!(response["status"], "ok", "real deferred restart must be admitted: {response}");
    assert!(app.adapters.player.release_has_queued_start());
    assert!(finish_catalog_playback(&mut app));
    assert_eq!(app.ott.as_ref().unwrap().play_target, Some(PlaybackRequest::ordinary("late", 0)),
        "EOF after accepted restart must advance from the restarted first file");
}

#[test]
fn rejected_native_restart_preserves_the_staged_restore() {
    let _serial = plx_base::testlock::serial();
    let _reset = ResetMedia;
    let mut app = restart_fixture();
    late_batch(&mut app);
    let (generation, cancel) = {
        let runtime = app.ott.as_ref().unwrap();
        assert_eq!(runtime.play_target, Some(PlaybackRequest::ordinary("late", 42_000)));
        (runtime.play_generation, runtime.play_cancel.clone())
    };
    overdue_native_restart(&mut app);
    let response = remote_restart(&mut app);
    assert_eq!(response["status"], "rejected", "overdue release must refuse real native reload");
    assert!(app.adapters.player.release_overdue(), "the actual release machine must reject the reload");
    let runtime = app.ott.as_ref().unwrap();
    assert_eq!(runtime.play_generation, generation, "a refused restart must not consume an existing resolver");
    assert_eq!(runtime.play_target, Some(PlaybackRequest::ordinary("late", 42_000)));
    assert!(!cancel.load(Ordering::Acquire));
}

#[test]
fn rejected_restart_preserves_a_real_deferred_channel_command() {
    let _serial = plx_base::testlock::serial();
    let _reset = ResetMedia;
    let mut app = restart_fixture();
    app.ott.as_mut().unwrap().catalog.entries.clear();
    assert!(command(&mut app, "command", json!({
        "command":"channel_by_name", "channel_name":"chosen.mp4"
    })).is_none(), "legacy channel command must wait for its first catalogue batch");
    overdue_native_restart(&mut app);
    assert_eq!(remote_restart(&mut app)["status"], "rejected");
    let runtime = app.ott.as_ref().unwrap();
    let before_revision = runtime.catalog_revision;
    // Discovery must cross the real catalog receiver: its revision also
    // invalidates the empty channel projection cached by the deferred command.
    runtime.catalog_tx.send(CatalogEvent::Batch(runtime.generation, Catalog {
        hierarchy: None, entries: vec![local_entry("chosen")],
    })).unwrap_or_else(|_| panic!("fixture catalogue receiver disappeared"));
    runtime_tick(&mut app);
    let runtime = app.ott.as_ref().unwrap();
    assert_ne!(runtime.catalog_revision, before_revision,
        "the first discovered batch must invalidate the cached empty channel list");
    assert_eq!(runtime.play_target, Some(PlaybackRequest::ordinary("chosen", 0)),
        "the refused restart must leave the actual deferred request able to dispatch");
    let pending = runtime.pending_play.as_ref()
        .expect("the original deferred remote request must own this playback admission");
    assert_eq!(pending.action, "play");
    assert_eq!(pending.params["query"], "chosen.mp4");
    assert_eq!(commands::play_response(runtime, pending)["data"]["channel"]["id"], "chosen",
        "automatic continuation alone must not satisfy the deferred command regression");
}

#[test]
fn newer_explicit_ott_selection_is_not_advertised_or_canceled_by_restart() {
    let _serial = plx_base::testlock::serial();
    let _reset = ResetMedia;
    let mut app = restart_fixture();
    let (generation, cancel) = {
        let runtime = app.ott.as_mut().unwrap();
        runtime.catalog.entries.push(local_entry("chosen"));
        explicit_play(&mut app, "chosen");
        let runtime = app.ott.as_mut().unwrap();
        (runtime.play_generation, runtime.play_cancel.clone())
    };
    defer_native_restart(&mut app);
    assert!(!restart_offered(&mut app), "capability must use the same ownership guard as the command");
    assert_eq!(remote_restart(&mut app)["status"], "rejected");
    let runtime = app.ott.as_ref().unwrap();
    assert_eq!(runtime.play_target, Some(PlaybackRequest::ordinary("chosen", 0)));
    assert_eq!(runtime.play_generation, generation);
    assert!(!cancel.load(Ordering::Acquire));
}

#[test]
fn newer_native_selection_is_not_advertised_or_restarted_as_the_old_source() {
    let _serial = plx_base::testlock::serial();
    let _reset = ResetMedia;
    let (_servers, sid) = ResetNativeServers::fixture();
    let mut app = restart_fixture();
    defer_native_restart(&mut app);
    plx_media::route::install_plex_request_for_test(&mut app.player.session, sid, "42");
    assert!(!restart_offered(&mut app));
    assert_eq!(remote_restart(&mut app)["status"], "rejected");
    assert_eq!(plx_media::route::playback_request_identity(&app.player.session), Some((sid, "42")));
    assert!(app.ott.as_ref().unwrap().continuity.pending_restore().is_some(),
        "the refusal must not manufacture new OTT playback intent");
}

#[test]
fn accepted_restart_preserves_pause_and_blocks_late_restore_and_eof() {
    let _serial = plx_base::testlock::serial();
    let _reset = ResetMedia;
    let mut app = restart_fixture();
    assert!(plx_media::player::pause(&mut app.adapters.player));
    plx_media::player::swap_state_for_test(plx_media::player::PlaybackState::Paused);
    defer_native_restart(&mut app);
    assert_eq!(remote_restart(&mut app)["status"], "ok");
    assert!(lifecycle::viewer_paused());
    late_batch(&mut app);
    assert_first_kept(&app);
    assert!(finish_catalog_playback(&mut app));
    assert!(app.adapters.player.release_has_queued_start(),
        "stale EOF while paused must not cancel the accepted deferred reload");
    assert!(app.ott.as_ref().unwrap().play_target.is_none());
}

#[test]
fn first_failed_native_resolve_offers_restart_without_an_installed_predecessor() {
    let _serial = plx_base::testlock::serial();
    let _reset = ResetMedia;
    let (_servers, sid) = ResetNativeServers::fixture();
    let mut app = app_fixture();
    app.running = true;
    // ResetMedia leaves the shared route controller Stable. Use the real empty
    // teardown to establish initial Idle; it has no Engine or URL to touch.
    plx_media::player::stop_bufferfeed(&mut app.player.session, &mut app.adapters.player);
    plx_media::route::install_plex_request_for_test(&mut app.player.session, sid, "first-failed");
    plx_media::route::fail_initial_resolve_spawn_for_test(&mut app.player.session);

    assert!(crate::app::ott_runtime::playback_allowed(&app));
    assert_eq!(plx_media::route::cur_sid(&app.player.session), plx_plex::plex::ServerId::UNSET);
    assert!(plx_media::route::cur_rk(&app.player.session).is_empty());
    assert!(plx_media::route::external_source(&app.player.session).is_none());
    assert!(!app.adapters.player.is_live());
    assert!(plx_media::route::can_retry_current_play(&app.player.session));
    assert!(!plx_media::route::play_pending());
    assert!(plx_media::route::play_resolution_failed(&app.player.session));
    assert_eq!(plx_media::player::state(&app.player.session), plx_media::player::PlaybackState::Error,
        "Error must be derived from real failed-spawn settlement, not swapped into pump state");
    assert!(restart_offered(&mut app),
        "the first failed retained request is retryable; there is no older installed source to protect");
}

#[test]
fn first_panicked_native_resolve_offers_restart_after_its_default_plan_lands() {
    let _serial = plx_base::testlock::serial();
    let _reset = ResetMedia;
    let (_servers, sid) = ResetNativeServers::fixture();
    let mut app = app_fixture();
    app.running = true;
    plx_media::player::stop_bufferfeed(&mut app.player.session, &mut app.adapters.player);
    plx_media::route::install_plex_request_for_test(&mut app.player.session, sid, "first-panicked");
    plx_media::route::land_initial_resolve_panic_for_test(
        &mut app.player.session, app.bridge.metadata_mut(),
    );

    assert!(crate::app::ott_runtime::playback_allowed(&app));
    assert_eq!(plx_media::route::cur_sid(&app.player.session), plx_plex::plex::ServerId::UNSET);
    assert_eq!(plx_media::route::cur_rk(&app.player.session), "first-panicked",
        "the real default-plan installer retains the attempted rating key");
    assert_eq!(plx_media::route::playback_request_identity(&app.player.session), Some((sid, "first-panicked")));
    assert!(!plx_media::route::has_url(&app.player.session));
    assert!(plx_media::route::external_source(&app.player.session).is_none());
    assert!(!app.adapters.player.is_live());
    assert!(plx_media::route::can_retry_current_play(&app.player.session));
    assert!(!plx_media::route::play_pending());
    assert!(plx_media::route::play_resolution_failed(&app.player.session));
    assert_eq!(plx_media::player::state(&app.player.session), plx_media::player::PlaybackState::Error,
        "the default failed plan must derive Error without replacing the pump state");
    assert!(restart_offered(&mut app),
        "a failed initial plan has no older installed source to protect from retry");
}

// Actual catalogue admission and terminal reconciliation, with only decoder
// resources injected. The resolver is held: these tests never fetch a source.
fn failed_live_restart_fixture() -> RestartFixture {
    struct Accepted;
    impl playback::PlaybackResources for Accepted {
        fn request_movie(&mut self, _: &mut plx_media::route::PlaybackSession,
            _: &mut plx_data::stores::metadata::MetadataStore, _: &plx_data::pms::PmsMovie,
            _: Option<plx_plex::plex::MediaId>) -> bool { panic!("external source only") }
        fn request_episode(&mut self, _: &mut plx_media::route::PlaybackSession,
            _: &mut plx_data::stores::metadata::MetadataStore, _: &str,
            _: Option<plx_plex::plex::MediaId>) -> bool { panic!("external source only") }
        fn describe_movie(&mut self, _: &mut plx_data::stores::metadata::MetadataStore,
            _: plx_plex::plex::ServerId, _: &str) { panic!("external source only") }
        fn prepare_start(&mut self, _: &mut plx_media::route::PlaybackSession,
            _: &mut plx_media::player::adapter::PlayerAdapter, _: i64) -> bool { true }
    }
    plx_media::player::SHARED.reset_session();
    plx_media::player::TX.reset();
    plx_media::route::reset_player_control_for_test(&plx_media::route::PlaybackSession::IDLE);
    let mut app = ready_app_fixture();
    app.running = true;
    assert!(lifecycle::set_transport_paused(&mut app.adapters.player, false));
    let profile = ProviderProfile {
        id: "restart-live".into(), name: "Restart live".into(), kind: ProviderKind::M3u,
        source: "https://restart.invalid/list.m3u".into(), source_alias: String::new(), enabled: true,
    };
    let mut entry = local_entry("first");
    entry.media = MediaLocator::Http { url: "https://restart.invalid/live.m3u8".into() };
    entry.live = Some(Default::default());
    entry.duration_ms = None;
    let mut runtime = app.ott.take().unwrap();
    runtime.saved.profiles.profiles = vec![profile.clone()];
    runtime.saved.profiles.active_profile = Some(profile.id.clone());
    runtime.provider = ProviderKind::M3u;
    runtime.catalog = Catalog { entries: vec![entry.clone()], hierarchy: None };
    runtime.catalog_loading = false;
    runtime.catalog_complete = true;
    runtime.catalog_done.store(false, Ordering::Release);
    runtime.metadata_done.store(false, Ordering::Release);
    runtime.resolver_busy = true;
    runtime.resolver_hold_for_test = true;
    runtime.plex_refresh_pending = true;
    runtime.saving = true;
    runtime.write_permit.revoke();
    runtime.continuity.suppress_unrequested_start();
    let mut request = PlaybackRequest::ordinary("first", 0);
    request.advance_on_failure = false;
    runtime.play_request_with_policy(&mut app, request).unwrap();
    let queued = runtime.resolver_next.take().unwrap();
    let generation = queued.generation;
    app.ott = Some(runtime);
    let mut info = unsupported_info();
    info.video_codec = "h264".into(); info.audio_codec = "aac".into();
    info.audio_channels = 2; info.audio_sample_rate = 48_000; info.audio_bit_depth = 16;
    info.duration_ns = 0;
    // A real successful playlist inspection supplies this typed source. Passing
    // a raw Http(.m3u8) correctly fails admission as an uninspected playlist.
    info.resolved_source = Some(plx_media::ff::ExternalSource::Hls(
        "https://restart.invalid/live.m3u8".into()));
    app.ott.as_ref().unwrap().tx.send(Job::Play(generation, profile,
        Ok((entry, ReadySource::External(plx_media::ff::ExternalSource::Http(
            "https://restart.invalid/live.m3u8".into()), info, None)))))
        .unwrap_or_else(|_| panic!("fixture receiver disappeared"));
    let clock = plx_base::task::time::ClockSample::capture_main(app.adapters.player.mt()).unwrap();
    tick_with_start(&mut app, clock, |runtime, app, profile, entry, source|
        start_resolved_with(runtime, app, profile, entry, source, &mut Accepted));
    {
        let runtime = app.ott.as_ref().unwrap();
        assert!(runtime.playing.as_ref().is_some_and(|playing| playing_matches(playing, &app)),
            "real live fixture admission failed: status={}, target={:?}", runtime.status, runtime.play_target);
    }
    // Attach the existing inert Engine to the real admitted start transaction.
    // Its native callback epoch owns the same closed failure seen on the TV;
    // no decoder producer or loaded HostSink object exists in this fixture.
    assert_eq!(unsafe {
        plx_platform::tv::sink::VideoSink::ready(&plx_media::player::ffi_host::HostSink,
            app.adapters.player.mt())
    }, 0);
    let transaction = plx_media::route::pending_route_start().unwrap();
    let attempt = plx_media::route::claim_route_start_attempt(transaction).unwrap();
    assert!(plx_media::route::settle_route_start(&mut app.player.session, attempt,
        plx_media::route::RouteStartResult::Started));
    let epoch = plx_media::player::SHARED.begin_native_session().unwrap();
    assert!(plx_media::player::SHARED.mark_native_load_returned(epoch, None).is_some());
    let mut engine = plx_media::player::engine::engine_after_reload_for_test();
    engine.route_start = attempt;
    engine.native_epoch = epoch;
    app.adapters.player.install(engine);
    assert!(plx_media::player::SHARED.note_io_failure_for(epoch,
        plx_media::player::report::IoFailureDetails::Hls {
            exit: plx_telemetry::telemetry::classes::DemuxExitClass::HlsTransportFailed,
            outcome: None,
        }));
    plx_media::player::SHARED.demux_io_failed.store(true, Ordering::Release);
    let mut runtime = app.ott.take().unwrap();
    assert!(runtime.playing.as_ref().is_some_and(|playing| playing_matches(playing, &app)));
    plx_media::player::swap_state_for_test(plx_media::player::PlaybackState::Error);
    runtime.reconcile_failure(&mut app);
    runtime.reconcile_owner(&mut app);
    assert!(runtime.playing.is_none());
    assert!(failed_playback::admissible(&runtime, &app).is_some());
    assert!(!app.adapters.player.is_live());
    // Stop clears the playable URL and native work, but intentionally retains
    // the bound external descriptor as session metadata for ownership checks.
    assert!(plx_media::route::external_source(&app.player.session).is_some());
    assert!(!plx_media::route::has_url(&app.player.session));
    assert!(plx_media::route::pending_route_start().is_none());
    assert!(!app.adapters.player.release_has_queued_start());
    assert_eq!(plx_media::player::state(&app.player.session), plx_media::player::PlaybackState::Idle);
    assert!(runtime.play_target.is_none() && runtime.resolver_next.is_none());
    app.ott = Some(runtime);
    RestartFixture(app)
}

#[test]
fn explicit_restart_of_retired_live_channel_queues_fresh_exact_admission() {
    let _serial = plx_base::testlock::serial();
    let _reset = ResetMedia;
    let mut app = failed_live_restart_fixture();
    // CLI restart also works from the catalogue, not only the fullscreen HUD.
    bridge::show_page(&mut app.pages, AppArg::Ott);
    settle_pages(&mut app);
    let before = app.ott.as_ref().unwrap();
    let saved = serde_json::to_value(&before.saved).unwrap();
    let generation = before.play_generation;
    let retired_source = plx_media::route::external_source(&app.player.session);
    assert!(restart_offered(&mut app), "retired decoder is not a lost current channel");
    assert_eq!(remote_restart(&mut app)["status"], "ok");
    let runtime = app.ott.as_ref().unwrap();
    let request = runtime.play_target.as_ref().expect("fresh policy admission");
    assert_eq!(request.id, "first");
    assert_eq!(request.position_ms, 0);
    assert!(!request.advance_on_failure);
    assert_ne!(runtime.play_generation, generation);
    assert_eq!(runtime.resolver_next.as_ref().unwrap().generation, runtime.play_generation);
    assert_eq!(runtime.resolver_next.as_ref().unwrap().entry.as_ref().unwrap().id, "first");
    assert!(runtime.failed_playback.is_none());
    assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), saved);
    assert_eq!(plx_media::route::external_source(&app.player.session), retired_source,
        "accepting a resolve must not replace the retained source metadata");
    assert!(!plx_media::route::has_url(&app.player.session),
        "accepting a new resolve must not reinstall a retired native URL");
    assert!(plx_media::route::pending_route_start().is_none());
    assert!(!app.adapters.player.release_has_queued_start());
    assert!(!app.adapters.player.is_live());
    assert_eq!(runtime.resolver_launches_for_test, 0);
}

#[test]
fn retired_live_restart_rejects_changed_owner_or_suspended_intent_without_work() {
    let _serial = plx_base::testlock::serial();
    let _reset = ResetMedia;
    for mutation in 0..8 {
        let mut app = failed_live_restart_fixture();
        match mutation {
            0 => app.ott.as_mut().unwrap().playback_epoch += 1,
            1 => app.ott.as_mut().unwrap().generation += 1,
            2 => app.ott.as_mut().unwrap().saved.profiles.profiles[0].source.push_str("?changed"),
            3 => app.ott.as_mut().unwrap().catalog.entries[0].media =
                MediaLocator::Http { url: "https://restart.invalid/replaced.m3u8".into() },
            4 => app.ott_standby = true,
            // There is intentionally no Engine after terminal cleanup. Model
            // retained viewer intent, not a fabricated native Pause success.
            5 => plx_media::player::transport::set_paused(true),
            6 => app.ott.as_mut().unwrap().persistence_uncertain = true,
            7 => { app.ott.as_mut().unwrap().saved.settings.insert("nativeKiosk".into(), json!({"version":999})); },
            _ => unreachable!(),
        }
        let before = app.ott.as_ref().unwrap();
        let generation = before.play_generation;
        let saved = serde_json::to_value(&before.saved).unwrap();
        assert!(!restart_offered(&mut app), "mutation {mutation}");
        assert_eq!(remote_restart(&mut app)["status"], "rejected", "mutation {mutation}");
        let runtime = app.ott.as_ref().unwrap();
        assert_eq!(runtime.play_generation, generation);
        assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), saved);
        assert!(runtime.play_target.is_none() && runtime.resolver_next.is_none());
        assert!(!plx_media::route::has_url(&app.player.session));
        assert!(plx_media::route::pending_route_start().is_none());
        assert!(!app.adapters.player.release_has_queued_start());
    }
}

#[test]
fn retired_live_restart_rechecks_parental_policy_before_consuming_failure() {
    let _serial = plx_base::testlock::serial();
    let _reset = ResetMedia;
    let mut app = failed_live_restart_fixture();
    // Enrichment may mark a previously admitted row as protected without
    // changing its media identity. A recovery receipt is not parental consent.
    app.ott.as_mut().unwrap().catalog.entries[0].protection = Some(true);
    let generation = app.ott.as_ref().unwrap().play_generation;
    assert_eq!(remote_restart(&mut app)["status"], "rejected");
    let runtime = app.ott.as_ref().unwrap();
    assert!(runtime.failed_playback.is_some());
    assert_eq!(runtime.play_generation, generation);
    assert!(runtime.play_target.is_none() && runtime.resolver_next.is_none());
}

#[test]
fn retired_live_restart_cannot_cancel_a_newer_admitted_selection() {
    let _serial = plx_base::testlock::serial();
    let _reset = ResetMedia;
    let mut app = failed_live_restart_fixture();
    explicit_play(&mut app, "first");
    let runtime = app.ott.as_ref().unwrap();
    let request = runtime.play_target.clone();
    let generation = runtime.play_generation;
    let cancel = runtime.play_cancel.clone();
    assert!(request.is_some() && runtime.resolver_next.is_some());
    assert!(!restart_offered(&mut app));
    assert_eq!(remote_restart(&mut app)["status"], "rejected");
    let runtime = app.ott.as_ref().unwrap();
    assert_eq!(runtime.play_target, request);
    assert_eq!(runtime.play_generation, generation);
    assert!(!cancel.load(Ordering::Acquire));
    assert_eq!(runtime.resolver_next.as_ref().unwrap().generation, generation);
}

#[test]
fn retired_live_restart_revalidates_same_strict_pin_instead_of_denying_all_kiosks() {
    let _serial = plx_base::testlock::serial();
    let _reset = ResetMedia;
    let mut app = failed_live_restart_fixture();
    let runtime = app.ott.as_mut().unwrap();
    let profile = runtime.effective_profile().unwrap();
    let bindings = selection::SelectionBindings::profiles_only(&runtime.saved.profiles);
    let favorite = selection::capture_bound(&bindings,
        selection::SelectionOwner::Profile(profile), &runtime.catalog.entries[0]).unwrap();
    let pin = crate::app::ott_kiosk_stored::Stored::item(favorite, true).unwrap();
    runtime.saved.settings.insert("nativeKiosk".into(), pin.encode().unwrap());
    let saved = serde_json::to_value(&runtime.saved).unwrap();
    assert!(restart_offered(&mut app), "a valid strict pin permits restarting that same source");
    assert_eq!(remote_restart(&mut app)["status"], "ok");
    let runtime = app.ott.as_ref().unwrap();
    let request = runtime.play_target.as_ref().unwrap();
    assert_eq!(request.id, "first");
    assert!(!request.advance_on_failure);
    assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), saved);
    assert!(!plx_media::route::has_url(&app.player.session));
    assert!(!app.adapters.player.is_live());
}
