//! Accepted viewer transport on the first discovered file revokes a late saved-file restore.
//! Uses the real command performer and catalogue receiver with an inert host-sink Engine.
use super::*;

#[path = "restart.rs"]
mod restart;

fn transport_fixture() -> App {
    let mut app = ready_app_fixture();
    local_playing(
        &mut app,
        Some(continuity::ResumePoint { entry_id: "late".into(), position_ms: 42_000 }),
        8_000,
    );
    let runtime = app.ott.as_ref().unwrap();
    assert!(runtime.loaded && !runtime.persistence_uncertain,
        "ordinary viewer transport requires an accepted settings load");
    assert!(matches!(runtime.track_selection_kiosk(), Ok(crate::app::ott_kiosk_policy::State::Off)));
    let selection = runtime.local_profile_selection(runtime.active().unwrap())
        .expect("the transport fixture must bind its configured Local folder");
    assert!(runtime.capture_local_source(&selection).is_some(),
        "late restore must have the same real Local read authority as explicit play");
    app.adapters.player.install(plx_media::player::engine::engine_after_reload_for_test());
    // A repeated Pause is an accepted viewer instruction too. Start paused so
    // this regression needs no simulated native clock callback or decoder IO.
    assert!(plx_media::player::pause(&mut app.adapters.player));
    plx_media::player::swap_state_for_test(plx_media::player::PlaybackState::Paused);
    plx_media::player::SHARED.duration_ns.store(60_000_000_000, Ordering::Relaxed);
    assert!(app.adapters.player.is_live());
    app
}

fn remote_transport(app: &mut App, params: Value) -> Value {
    let mut runtime = app.ott.take().unwrap();
    // The convenience parser uses one fixed request ID. A fresh transport fixture
    // avoids treating a second synthetic command as a replay of that ID.
    let mut remote = Remote::disabled_for_test("dev_fixture", "fixture");
    let mut invocation = remote.request_for_test("playback", params);
    invocation.deadline_ms = runtime.now_ms().saturating_add(10_000);
    let response = commands::command(&mut runtime, app, &invocation)
        .expect("transport produces its immediate acceptance receipt");
    app.ott = Some(runtime);
    response
}

fn late_batch(app: &mut App) {
    app.ott.as_ref().unwrap().catalog_tx.send(CatalogEvent::Batch(
        7, Catalog { hierarchy: None, entries: vec![local_entry("late")] },
    )).unwrap_or_else(|_| panic!("fixture catalogue receiver disappeared"));
    runtime_tick(app);
}

fn assert_first_kept(app: &App) {
    let runtime = app.ott.as_ref().unwrap();
    assert_eq!(runtime.playing.as_ref().unwrap().entry.id, "first");
    assert!(runtime.resolver_next.is_none() && runtime.play_target.is_none(),
        "accepted viewer transport must prevent a late saved-file restore");
}

#[test]
fn accepted_remote_pause_prevents_late_local_restore() {
    let _serial = plx_base::testlock::serial();
    let _reset = ResetMedia;
    let mut app = transport_fixture();
    let response = remote_transport(&mut app, json!({"operation":"pause"}));
    assert_eq!(response["status"], "ok", "the actual transport command must be accepted: {response}");
    assert_eq!(response["data"]["dispatched"], true);
    late_batch(&mut app);
    assert_first_kept(&app);
}

#[test]
fn accepted_remote_seek_prevents_late_local_restore() {
    let _serial = plx_base::testlock::serial();
    let _reset = ResetMedia;
    let mut app = transport_fixture();
    let response = remote_transport(&mut app, json!({"operation":"seek","position":12}));
    assert_eq!(response["status"], "ok", "the actual seek command must be accepted: {response}");
    assert_eq!(response["data"]["dispatched"], true);
    late_batch(&mut app);
    assert_first_kept(&app);
}

#[test]
fn accepted_remote_resume_prevents_late_local_restore() {
    let _serial = plx_base::testlock::serial();
    let _reset = ResetMedia;
    let mut app = transport_fixture();
    let response = remote_transport(&mut app, json!({"operation":"resume"}));
    assert_eq!(response["status"], "ok", "{response}");
    assert!(!lifecycle::viewer_paused());
    late_batch(&mut app);
    assert_first_kept(&app);
}

#[test]
fn accepted_pause_blocks_eof_but_resume_keeps_normal_next() {
    let _serial = plx_base::testlock::serial();
    let _reset = ResetMedia;
    let mut app = transport_fixture();
    assert_eq!(remote_transport(&mut app, json!({"operation":"pause"}))["status"], "ok");
    late_batch(&mut app);
    assert_first_kept(&app);
    assert_eq!(app.ott.as_mut().unwrap().continuity.ended(7, "first", &["first", "late"], true),
        continuity::Decision::Idle, "a queued EOF must not override viewer Pause");
    assert_eq!(remote_transport(&mut app, json!({"operation":"resume"}))["status"], "ok");
    assert_eq!(app.ott.as_mut().unwrap().continuity.ended(7, "first", &["first", "late"], true),
        continuity::Decision::Start { entry_id: "late".into(), position_ms: 0, reason: continuity::Reason::Next },
        "Resume must retain ordinary continuation, without restoring the old 42s bookmark");
}

#[test]
fn accepted_pause_cancels_already_staged_automatic_restore() {
    let _serial = plx_base::testlock::serial();
    let _reset = ResetMedia;
    let mut app = transport_fixture();
    late_batch(&mut app);
    let runtime = app.ott.as_ref().unwrap();
    assert_eq!(runtime.play_target, Some(PlaybackRequest::ordinary("late", 42_000)));
    let automatic_cancel = runtime.play_cancel.clone();
    assert_eq!(remote_transport(&mut app, json!({"operation":"pause"}))["status"], "ok");
    assert!(automatic_cancel.load(Ordering::Acquire), "already admitted automatic resolver must be retired");
    assert_first_kept(&app);
}

#[test]
fn accepted_pause_preserves_a_newer_explicit_play_request() {
    let _serial = plx_base::testlock::serial();
    let _reset = ResetMedia;
    let mut app = transport_fixture();
    late_batch(&mut app);
    let (generation, explicit_cancel) = {
        let runtime = app.ott.as_mut().unwrap();
        runtime.catalog.entries.push(local_entry("chosen"));
        explicit_play(&mut app, "chosen");
        let runtime = app.ott.as_mut().unwrap();
        (runtime.play_generation, runtime.play_cancel.clone())
    };
    assert_eq!(remote_transport(&mut app, json!({"operation":"pause"}))["status"], "ok");
    let runtime = app.ott.as_ref().unwrap();
    assert_eq!(runtime.play_target, Some(PlaybackRequest::ordinary("chosen", 0)));
    assert_eq!(runtime.play_generation, generation);
    assert!(!explicit_cancel.load(Ordering::Acquire));
    assert!(runtime.resolver_next.as_ref().is_some_and(|request| request.generation == generation));
}

#[test]
fn rejected_seek_does_not_revoke_saved_file_restore() {
    let _serial = plx_base::testlock::serial();
    let _reset = ResetMedia;
    let mut app = transport_fixture();
    let response = remote_transport(&mut app, json!({"operation":"seek","position":-1}));
    assert_ne!(response["status"], "ok", "invalid seek must not claim acceptance");
    late_batch(&mut app);
    assert_eq!(app.ott.as_ref().unwrap().play_target, Some(PlaybackRequest::ordinary("late", 42_000)));
}

#[test]
fn internal_transport_and_seek_do_not_revoke_saved_file_restore() {
    let _serial = plx_base::testlock::serial();
    let _reset = ResetMedia;
    let mut app = transport_fixture();
    assert!(lifecycle::set_transport_paused(&mut app.adapters.player, false));
    playback::request_seek(12_000_000_000);
    late_batch(&mut app);
    assert_eq!(app.ott.as_ref().unwrap().play_target, Some(PlaybackRequest::ordinary("late", 42_000)),
        "low-level reload/hold helpers do not imply new viewer intent");
}

fn native_transport(app: &mut App, request: PlayerReq) {
    app.pages.emit(MachineId::Nav, Fx::App(AppFx::Player(request)));
    let tick = next_tick(app);
    bridge::frame(&mut app.pages, &mut app.bridge, tick, Vec::new());
    let frame = run::Frame {
        ctrl: plx_appkit::player_hud::slot(&app.player.session, app.bridge.metadata_view()),
        now: tick.ms,
        dt: 0.016,
        underlay_moving: false,
        player: true,
        present: false,
        deferred: false,
        rn: "player",
    };
    content::content_requests(app, &frame);
}

#[test]
fn native_commit_seek_prevents_late_local_restore() {
    let _serial = plx_base::testlock::serial();
    let _reset = ResetMedia;
    let mut app = transport_fixture();
    native_transport(&mut app, PlayerReq::CommitSeek(12_000_000_000));
    assert_eq!(playback::intended_pos(&app.player.session), 12_000_000_000,
        "the actual content performer must commit the native seek");
    late_batch(&mut app);
    assert_first_kept(&app);
}

#[test]
fn native_pause_cancels_already_staged_automatic_restore() {
    let _serial = plx_base::testlock::serial();
    let _reset = ResetMedia;
    let mut app = transport_fixture();
    late_batch(&mut app);
    assert_eq!(app.ott.as_ref().unwrap().play_target, Some(PlaybackRequest::ordinary("late", 42_000)));
    native_transport(&mut app, PlayerReq::Transport(Some(false)));
    assert!(lifecycle::viewer_paused());
    assert_first_kept(&app);
}

#[test]
fn rejected_remote_pause_without_live_engine_preserves_restore() {
    let _serial = plx_base::testlock::serial();
    let _reset = ResetMedia;
    let mut app = transport_fixture();
    let _inert_engine = app.adapters.player.take().expect("fixture engine");
    let response = remote_transport(&mut app, json!({"operation":"pause"}));
    assert_eq!(response["status"], "rejected", "no live playback must reject transport");
    late_batch(&mut app);
    assert_eq!(app.ott.as_ref().unwrap().play_target, Some(PlaybackRequest::ordinary("late", 42_000)));
}

#[test]
fn canceled_automatic_restore_cannot_land_after_accepted_pause() {
    let _serial = plx_base::testlock::serial();
    let _reset = ResetMedia;
    let mut app = transport_fixture();
    late_batch(&mut app);
    let (automatic_generation, profile) = {
        let runtime = app.ott.as_ref().unwrap();
        assert_eq!(runtime.play_target, Some(PlaybackRequest::ordinary("late", 42_000)));
        (runtime.play_generation, runtime.active().unwrap().clone())
    };
    assert_eq!(remote_transport(&mut app, json!({"operation":"pause"}))["status"], "ok");
    app.ott.as_ref().unwrap().tx.send(Job::Play(
        automatic_generation,
        profile,
        Err("obsolete automatic resolver failure"),
    )).unwrap_or_else(|_| panic!("fixture resolver receiver disappeared"));
    runtime_tick(&mut app);
    assert_first_kept(&app);
    assert!(app.adapters.player.is_live(), "stale automatic result must not tear down the current decoder");
    assert_ne!(app.ott.as_ref().unwrap().status, "obsolete automatic resolver failure");
    assert!(lifecycle::viewer_paused());
}

#[test]
fn paused_eof_keeps_current_decoder_and_nonzero_checkpoint() {
    let _serial = plx_base::testlock::serial();
    let _reset = ResetMedia;
    let mut app = transport_fixture();
    assert_eq!(remote_transport(&mut app, json!({"operation":"pause"}))["status"], "ok");
    let profile = app.ott.as_ref().unwrap().active().unwrap().clone();
    app.ott.as_mut().unwrap().checkpoints.insert(profile.id.clone(), SavedResume {
        entry_id: "first".into(),
        position_ms: 8_000,
        source_fingerprint: profile_fingerprint(&profile),
    });
    assert!(finish_catalog_playback(&mut app), "OTT must consume its paused stream's stale EOF");
    assert!(app.adapters.player.is_live(), "a queued EOF must not tear down the paused decoder");
    let runtime = app.ott.as_ref().unwrap();
    assert_eq!(runtime.checkpoints.get(&profile.id).unwrap().position_ms, 8_000,
        "a queued EOF must not erase the paused playhead");
    assert_first_kept(&app);
}

#[test]
fn exhausted_unpaused_eof_still_retires_the_current_decoder() {
    let _serial = plx_base::testlock::serial();
    let _reset = ResetMedia;
    let mut app = transport_fixture();
    assert_eq!(remote_transport(&mut app, json!({"operation":"resume"}))["status"], "ok");
    let profile = app.ott.as_ref().unwrap().active().unwrap().clone();
    let runtime = app.ott.as_mut().unwrap();
    // The refreshed source no longer lists the file which is still finishing.
    // This is a genuine EOF with no playable successor, not a rejected event.
    runtime.catalog.entries.clear();
    runtime.catalog_loading = false;
    runtime.catalog_complete = true;
    assert!(finish_catalog_playback(&mut app));
    assert!(!app.adapters.player.is_live(), "an admitted exhausted EOF must retire its decoder");
    let runtime = app.ott.as_ref().unwrap();
    assert_eq!(runtime.checkpoints.get(&profile.id).unwrap().position_ms, 0);
    assert!(runtime.resolver_next.is_none() && runtime.play_target.is_none());
}

#[test]
fn outgoing_local_eof_cannot_override_a_new_native_plex_request() {
    let _serial = plx_base::testlock::serial();
    let _reset = ResetMedia;
    struct ResetServers;
    impl Drop for ResetServers {
        fn drop(&mut self) {
            plx_plex::plex::reset_servers_for_test();
        }
    }
    let _servers = ResetServers;
    plx_plex::plex::reset_servers_for_test();
    let sid = plx_plex::plex::register_for_test(
        "superseding-eof-fixture", "127.0.0.1", 1, "fixture-token", "fixture-client",
    );
    plx_plex::plex::client_for(sid).unwrap().disable_data_io();
    let mut app = transport_fixture();
    assert_eq!(remote_transport(&mut app, json!({"operation":"resume"}))["status"], "ok");
    let profile = app.ott.as_ref().unwrap().active().unwrap().clone();
    let runtime = app.ott.as_mut().unwrap();
    runtime.catalog.entries.push(local_entry("next"));
    runtime.checkpoints.insert(profile.id.clone(), SavedResume {
        entry_id: "first".into(),
        position_ms: 8_000,
        source_fingerprint: profile_fingerprint(&profile),
    });
    let outgoing_source = plx_media::route::external_source(&app.player.session);
    // The native resolver keeps the outgoing Local route while preparing Plex.
    // Deliberately do not tick OTT: real playback_tick sees EOF before that tick.
    plx_media::route::install_plex_request_for_test(&mut app.player.session, sid, "42");
    assert_eq!(plx_media::route::playback_request_identity(&app.player.session), Some((sid, "42")));
    assert_eq!(plx_media::route::external_source(&app.player.session), outgoing_source);
    assert!(finish_catalog_playback(&mut app), "the superseded outgoing EOF must be consumed");
    assert!(app.adapters.player.is_live(), "outgoing EOF must not tear down the decoder while native Plex resolves");
    assert_eq!(plx_media::route::playback_request_identity(&app.player.session), Some((sid, "42")),
        "outgoing EOF must preserve the newer native request");
    let runtime = app.ott.as_ref().unwrap();
    assert_eq!(runtime.checkpoints.get(&profile.id).unwrap().position_ms, 8_000,
        "outgoing EOF must not erase the superseded source's playhead");
    assert!(runtime.resolver_next.is_none() && runtime.play_target.is_none(),
        "outgoing EOF must not stage an OTT successor over the newer native request");

    // EOF may remain asserted until the replacement decoder starts. Retiring
    // OTT ownership at the next tick must not let a repeated EOF escape into
    // the native shelf's fallback exit/advance path.
    runtime_tick(&mut app);
    assert!(finish_catalog_playback(&mut app), "a repeated outgoing EOF must remain consumed while the new native request is pending");
    assert!(app.adapters.player.is_live());
    assert_eq!(plx_media::route::playback_request_identity(&app.player.session), Some((sid, "42")));
    let runtime = app.ott.as_ref().unwrap();
    assert_eq!(runtime.checkpoints.get(&profile.id).unwrap().position_ms, 8_000);
    assert!(runtime.resolver_next.is_none() && runtime.play_target.is_none());
}


#[test]
fn outgoing_eof_preserves_explicit_ott_play_newer_than_retained_native_request() {
    let _serial = plx_base::testlock::serial();
    let _reset = ResetMedia;
    struct ResetServers;
    impl Drop for ResetServers {
        fn drop(&mut self) {
            plx_plex::plex::reset_servers_for_test();
        }
    }
    let _servers = ResetServers;
    plx_plex::plex::reset_servers_for_test();
    let sid = plx_plex::plex::register_for_test(
        "superseding-ott-eof-fixture", "127.0.0.1", 1, "fixture-token", "fixture-client",
    );
    plx_plex::plex::client_for(sid).unwrap().disable_data_io();
    let mut app = transport_fixture();
    assert_eq!(remote_transport(&mut app, json!({"operation":"resume"}))["status"], "ok");
    plx_media::route::install_plex_request_for_test(&mut app.player.session, sid, "42");
    let (generation, cancel) = {
        let runtime = app.ott.as_mut().unwrap();
        runtime.catalog.entries.push(local_entry("chosen"));
        // The newer OTT selection is resolving, so the prior native request
        // identity remains retained until the selected OTT source is installed.
        explicit_play(&mut app, "chosen");
        let runtime = app.ott.as_mut().unwrap();
        (runtime.play_generation, runtime.play_cancel.clone())
    };
    assert!(finish_catalog_playback(&mut app));
    let runtime = app.ott.as_ref().unwrap();
    assert_eq!(runtime.play_target, Some(PlaybackRequest::ordinary("chosen", 0)),
        "retained native identity must not cancel a newer explicit OTT selection");
    assert_eq!(runtime.play_generation, generation);
    assert!(!cancel.load(Ordering::Acquire));
    assert!(app.adapters.player.is_live());
    runtime_tick(&mut app);
    let runtime = app.ott.as_ref().unwrap();
    assert_eq!(runtime.play_target, Some(PlaybackRequest::ordinary("chosen", 0)),
        "reconciliation must preserve the newer explicit OTT selection too");
    assert_eq!(runtime.play_generation, generation);
    assert!(!cancel.load(Ordering::Acquire));
    assert!(app.adapters.player.is_live());
}


struct ResetNativeServers;
impl ResetNativeServers {
    fn fixture() -> (Self, plx_plex::plex::ServerId) {
        plx_plex::plex::reset_servers_for_test();
        let sid = plx_plex::plex::register_for_test(
            "native-eof-boundary-fixture", "127.0.0.1", 1, "fixture-token", "fixture-client",
        );
        plx_plex::plex::client_for(sid).unwrap().disable_data_io();
        (Self, sid)
    }
}
impl Drop for ResetNativeServers {
    fn drop(&mut self) {
        plx_plex::plex::reset_servers_for_test();
    }
}

#[test]
fn outgoing_eof_guard_releases_after_matching_native_plex_lands() {
    let _serial = plx_base::testlock::serial();
    let _reset = ResetMedia;
    let (_servers, sid) = ResetNativeServers::fixture();
    let mut app = transport_fixture();
    plx_media::route::install_plex_request_for_test(&mut app.player.session, sid, "42");
    assert!(finish_catalog_playback(&mut app));
    runtime_tick(&mut app);
    assert!(app.ott.as_ref().unwrap().playing.is_none(), "reconciliation retires the former OTT owner");
    assert!(finish_catalog_playback(&mut app), "the old EOF remains guarded during resolve");

    // Model a completed native landing without starting a decoder or worker:
    // no outgoing External source, and the retained request matches the new item.
    app.player.session = plx_media::route::PlaybackSession::IDLE;
    plx_media::route::install_plex_identity_for_test(&mut app.player.session, sid, "42");
    plx_media::route::install_plex_request_for_test(&mut app.player.session, sid, "42");
    assert!(plx_media::route::external_source(&app.player.session).is_none());
    assert!(!finish_catalog_playback(&mut app), "the landed native item's EOF must return to its legacy handler");
}

#[test]
fn current_ticket_automatic_restore_cannot_outlive_a_foreign_native_request() {
    let _serial = plx_base::testlock::serial();
    let _reset = ResetMedia;
    let (_servers, sid) = ResetNativeServers::fixture();
    let mut app = transport_fixture();
    late_batch(&mut app);
    let runtime = app.ott.as_ref().unwrap();
    let automatic = runtime.play_cancel.clone();
    let ticket = runtime.play_ticket.expect("automatic restore admission ticket");
    assert_eq!(runtime.play_target, Some(PlaybackRequest::ordinary("late", 42_000)));
    assert_eq!(runtime.restore_play_generation, Some(runtime.play_generation));
    // Keep the ticket current deliberately: automatic vs explicit ownership,
    // rather than a stale ticket alone, must decide this reconciliation.
    plx_media::route::install_plex_request_for_test(&mut app.player.session, sid, "42");
    assert!(plx_media::route::play_request_is_current(ticket));
    assert!(finish_catalog_playback(&mut app));
    runtime_tick(&mut app);
    let runtime = app.ott.as_ref().unwrap();
    assert!(automatic.load(Ordering::Acquire));
    assert!(runtime.resolver_next.is_none() && runtime.play_target.is_none());
    assert_eq!(plx_media::route::playback_request_identity(&app.player.session), Some((sid, "42")));
    assert!(app.adapters.player.is_live());
}

#[test]
fn newer_native_request_revokes_an_explicit_ott_play_ticket() {
    let _serial = plx_base::testlock::serial();
    let _reset = ResetMedia;
    let (_servers, sid) = ResetNativeServers::fixture();
    let mut app = transport_fixture();
    let (ticket, cancel) = {
        let runtime = app.ott.as_mut().unwrap();
        runtime.catalog.entries.push(local_entry("chosen"));
        explicit_play(&mut app, "chosen");
        let runtime = app.ott.as_mut().unwrap();
        (runtime.play_ticket.unwrap(), runtime.play_cancel.clone())
    };
    assert!(plx_media::route::play_request_is_current(ticket));
    // Unlike merely installing test identity, cancellation advances the real
    // request fence as an admitted later native request does.
    plx_media::route::cancel_play(&mut app.player.session);
    plx_media::route::install_plex_request_for_test(&mut app.player.session, sid, "42");
    assert!(!plx_media::route::play_request_is_current(ticket));
    assert!(finish_catalog_playback(&mut app));
    runtime_tick(&mut app);
    let runtime = app.ott.as_ref().unwrap();
    assert!(cancel.load(Ordering::Acquire), "a later native request revokes the old OTT resolver");
    assert!(runtime.resolver_next.is_none() && runtime.play_target.is_none());
    assert_eq!(plx_media::route::playback_request_identity(&app.player.session), Some((sid, "42")));
    assert!(app.adapters.player.is_live());
}


#[test]
fn absolute_seek_beyond_duration_is_rejected_without_mutation() {
    let _serial = plx_base::testlock::serial();
    let _reset = ResetMedia;
    let mut app = transport_fixture();
    let before_position = playback::intended_pos(&app.player.session);
    let before_restore = app.ott.as_ref().unwrap().continuity.pending_restore().cloned();
    assert!(before_restore.is_some(), "the fixture must exercise pending saved-file restoration");
    let response = remote_transport(&mut app, json!({"operation":"seek","position":90}));
    assert_eq!(response["status"], "rejected", "an absolute position must not silently clamp: {response}");
    assert_eq!(playback::intended_pos(&app.player.session), before_position,
        "rejection must not mutate the pending native seek");
    assert_eq!(app.ott.as_ref().unwrap().continuity.pending_restore(), before_restore.as_ref(),
        "rejection must not consume saved-file restoration");
    late_batch(&mut app);
    assert_eq!(app.ott.as_ref().unwrap().play_target.as_ref()
        .map(|request| (request.id.as_str(), request.position_ms)), Some(("late", 42_000)));
}

#[test]
fn absolute_seek_to_duration_is_still_accepted_exactly() {
    let _serial = plx_base::testlock::serial();
    let _reset = ResetMedia;
    let mut app = transport_fixture();
    let response = remote_transport(&mut app, json!({"operation":"seek","position":60}));
    assert_eq!(response["status"], "ok", "the finite timeline end is a valid absolute position: {response}");
    assert_eq!(response["data"]["position"].as_f64(), Some(60.0));
    assert_eq!(playback::intended_pos(&app.player.session), 60_000_000_000);
    late_batch(&mut app);
    assert_first_kept(&app);
}

#[test]
fn relative_seek_retains_clamping_at_both_timeline_edges() {
    let _serial = plx_base::testlock::serial();
    for (offset, expected_seconds) in [(-90, 0), (90, 60)] {
        let _reset = ResetMedia;
        let mut app = transport_fixture();
        assert_eq!(playback::intended_pos(&app.player.session), 8_000_000_000);
        let response = remote_transport(&mut app, json!({"operation":"seek_relative","offset":offset}));
        assert_eq!(response["status"], "ok", "relative seek must retain bounded movement: {response}");
        assert_eq!(response["data"], json!({
            "operation":"seek_relative", "dispatched":true,
            "offset":offset as f64, "from":8.0,
            "position":expected_seconds as f64, "clamped":true
        }));
        assert_eq!(playback::intended_pos(&app.player.session), expected_seconds * 1_000_000_000);
        late_batch(&mut app);
        assert_first_kept(&app);
    }
}


#[test]
fn unavailable_kiosk_policy_rejects_transport_without_consuming_restore() {
    let _serial = plx_base::testlock::serial();
    for (loaded, uncertain) in [(false, false), (true, true)] {
        let _reset = ResetMedia;
        let mut app = transport_fixture();
        {
            let runtime = app.ott.as_mut().unwrap();
            // These are the two genuinely unavailable policy states. The
            // ordinary transport fixture must not turn either into Kiosk::Off.
            runtime.loaded = loaded;
            runtime.persistence_uncertain = uncertain;
            assert!(runtime.track_selection_kiosk().is_err());
        }
        let identity = plx_media::route::playback_identity(&app.player.session);
        let position = playback::intended_pos(&app.player.session);
        let restore = app.ott.as_ref().unwrap().continuity.pending_restore().cloned();
        assert!(restore.is_some(), "this control must retain an outstanding saved-file restore");
        assert!(lifecycle::viewer_paused());

        let response = remote_transport(&mut app, json!({"operation":"resume"}));
        assert_eq!(response["status"], "rejected", "unverified policy must reject remote transport: {response}");
        assert!(lifecycle::viewer_paused(), "a refused Resume must not reach the decoder");
        native_transport(&mut app, PlayerReq::CommitSeek(12_000_000_000));
        assert_eq!(playback::intended_pos(&app.player.session), position,
            "the real native request drain must refuse seek while policy is unavailable");
        assert_eq!(plx_media::route::playback_identity(&app.player.session), identity);
        let runtime = app.ott.as_ref().unwrap();
        assert_eq!(runtime.continuity.pending_restore(), restore.as_ref(),
            "neither refused command is accepted viewer intent");
        assert!(runtime.resolver_next.is_none() && runtime.play_target.is_none());
        assert!(app.adapters.player.is_live());
    }
}

// These regressions run the real restore selection, Local authority rebind,
// source admission, checkpoint and input/Exit paths. Only native resource
// acceptance and its later clock samples are supplied by the fixture. The
// existing catalogue, metadata, resolver and writer slots stay held: no media
// bytes, decoder, network or protected-storage write is exercised here.
#[derive(Default)]
struct AutomaticResumeResources { requested_ns: Option<i64> }

impl playback::PlaybackResources for AutomaticResumeResources {
    fn request_movie(&mut self, _: &mut plx_media::route::PlaybackSession,
        _: &mut plx_data::stores::metadata::MetadataStore, _: &plx_data::pms::PmsMovie,
        _: Option<plx_plex::plex::MediaId>) -> bool { panic!("Local restore only") }
    fn request_episode(&mut self, _: &mut plx_media::route::PlaybackSession,
        _: &mut plx_data::stores::metadata::MetadataStore, _: &str,
        _: Option<plx_plex::plex::MediaId>) -> bool { panic!("Local restore only") }
    fn describe_movie(&mut self, _: &mut plx_data::stores::metadata::MetadataStore,
        _: plx_plex::plex::ServerId, _: &str) { panic!("Local restore only") }
    fn prepare_start(&mut self, _: &mut plx_media::route::PlaybackSession,
        _: &mut plx_media::player::adapter::PlayerAdapter, resume_ns: i64) -> bool {
        self.requested_ns = Some(resume_ns);
        true
    }
}

fn automatic_resume_admit(app: &mut App) -> i64 {
    automatic_resume_admit_with_duration(app, 60_000_000_000)
}

fn automatic_resume_admit_with_duration(app: &mut App, duration_ns: i64) -> i64 {
    let runtime = app.ott.as_mut().unwrap();
    let request = runtime.play_target.clone().expect("a real selected request");
    let profile = runtime.active().unwrap().clone();
    let entry = runtime.catalog.entries.iter().find(|entry| entry.id == request.id)
        .expect("selected catalogue row").clone();
    let relative_path = match &entry.media {
        MediaLocator::Local { relative_path } => relative_path,
        _ => panic!("Local restore only"),
    };
    let source = plx_media::ff::ExternalSource::Local(
        std::path::Path::new(&profile.source).join(relative_path));
    let mut info = unsupported_info();
    info.video_codec = "h264".into();
    info.audio_codec = "aac".into();
    info.audio_channels = 2;
    info.audio_sample_rate = 48_000;
    info.audio_bit_depth = 16;
    info.duration_ns = duration_ns;
    let mut resources = AutomaticResumeResources::default();
    let generation = runtime.play_generation;
    let automatic = runtime.restore_play_generation == Some(generation);
    // Model the already-running resolver having taken its work item. The
    // existing busy slot prevents any actual resolver thread from starting.
    let _ = runtime.resolver_next.take();
    // The real Job::Play receiver must keep the automatic-restore marker until
    // start_resolved_with sees it, then settle the request itself. Calling the
    // start helper directly would miss an early marker-clearing regression.
    runtime.tx.send(Job::Play(generation, profile, Ok((entry,
        ReadySource::External(source, info, None)))))
        .unwrap_or_else(|_| panic!("fixture resolver receiver disappeared"));
    let clock = plx_base::task::time::ClockSample::capture_main(app.adapters.player.mt()).unwrap();
    let mut starts = 0;
    tick_with_start(app, clock, |runtime, app, profile, entry, source| {
        starts += 1;
        assert_eq!(runtime.restore_play_generation, automatic.then_some(generation),
            "the addressed resolver reason must survive through native admission");
        start_resolved_with(runtime, app, profile, entry, source, &mut resources)?;
        app.adapters.player.install(plx_media::player::engine::engine_after_reload_for_test());
        assert!(lifecycle::set_transport_paused(&mut app.adapters.player, false));
        plx_media::player::swap_state_for_test(plx_media::player::PlaybackState::Playing);
        plx_media::player::SHARED.duration_ns.store(duration_ns, Ordering::Relaxed);
        plx_media::player::SHARED.seen_frame.store(false, Ordering::Relaxed);
        plx_media::player::SHARED.playpos_ns.store(
            resources.requested_ns.unwrap(), Ordering::Relaxed);
        Ok(())
    });
    assert_eq!(starts, 1, "the addressed real Job must reach native admission once");
    let runtime = app.ott.as_ref().unwrap();
    assert!(runtime.play_target.is_none() && runtime.restore_play_generation.is_none());
    assert!(playing_matches(runtime.playing.as_ref().unwrap(), app));
    // The actual completed job released its resolver slot. Hold it again so a
    // following test's selected successor remains a request, not a new worker.
    app.ott.as_mut().unwrap().resolver_busy = true;
    settle_pages(app);
    resources.requested_ns.expect("the real start supplied a resume offset")
}

fn automatic_resume_select(app: &mut App, id: &str, position_ms: u64) {
    let runtime = app.ott.as_mut().unwrap();
    let profile = runtime.active().unwrap().clone();
    runtime.saved.resume.insert(profile.id.clone(), SavedResume {
        entry_id: id.into(), position_ms, source_fingerprint: profile_fingerprint(&profile),
    });
    runtime.checkpoints.clear();
    runtime.continuity = continuity::Continuity::new(runtime.generation,
        Some(continuity::ResumePoint { entry_id: id.into(), position_ms }));
    let ids = runtime.ordered_ids();
    let refs: Vec<&str> = ids.iter().map(String::as_str).collect();
    let decision = runtime.continuity.catalog_updated(runtime.generation, &refs, true);
    assert_eq!(decision, continuity::Decision::Start {
        entry_id: id.into(), position_ms, reason: continuity::Reason::Restore,
    });
    runtime.apply_continuity(decision);
    assert_eq!(runtime.restore_play_generation, Some(runtime.play_generation));
}

fn automatic_resume_clock(position_ms: u64, credited: bool) {
    // Model the consumed automatic seek and a published callback. This is not
    // a new user seek and does not exercise a decoder or attest displayed media.
    plx_media::player::TX.seek_to_ns.store(-1, Ordering::Relaxed);
    plx_media::player::SHARED.seek_to_ns.store(-1, Ordering::Relaxed);
    plx_media::player::SHARED.seek_target_ns.store(-1, Ordering::Relaxed);
    plx_media::player::SHARED.seek_display_ns.store(-1, Ordering::Relaxed);
    plx_media::player::SHARED.seeking.store(false, Ordering::Relaxed);
    plx_media::player::SHARED.seen_frame.store(credited, Ordering::Relaxed);
    plx_media::player::SHARED.playpos_ns.store((position_ms * 1_000_000) as i64, Ordering::Relaxed);
    plx_media::player::swap_state_for_test(plx_media::player::PlaybackState::Playing);
}

fn automatic_resume_fixture() -> Box<App> {
    automatic_resume_fixture_with_duration(60_000_000_000)
}

fn automatic_resume_fixture_with_duration(duration_ns: i64) -> Box<App> {
    let mut app = automatic_resume_queued_fixture(true);
    let expected_ns = if duration_ns > 0 { duration_ns.min(42_000_000_000) } else { 0 };
    assert_eq!(automatic_resume_admit_with_duration(&mut app, duration_ns), expected_ns);
    automatic_resume_clock((expected_ns.max(0) as u64 / 1_000_000).saturating_sub(4_000), true);
    app
}

fn automatic_resume_queued_fixture(automatic: bool) -> Box<App> {
    let mut app = Box::new(ready_app_fixture());
    local_playing(&mut app, None, 0);
    app.running = true;
    {
        let runtime = app.ott.as_mut().unwrap();
        runtime.catalog_done.store(false, Ordering::Release);
        runtime.metadata_done.store(false, Ordering::Release);
        runtime.catalog_complete = true;
        assert!(runtime.resolver_busy && runtime.saving);
    }
    automatic_resume_select(&mut app, "first", 42_000);
    if !automatic {
        // A newer explicit request with identical item and nonzero position
        // must retire the old automatic origin, even across a later suspend.
        let mut runtime = app.ott.take().unwrap();
        runtime.play_request_with_policy(&mut app,
            PlaybackRequest::ordinary("first", 42_000)).unwrap();
        assert!(runtime.restore_play_generation.is_none());
        app.ott = Some(runtime);
    }
    app
}

fn automatic_resume_checkpoint(app: &mut App) {
    let mut runtime = app.ott.take().unwrap();
    runtime.checkpoint(app, true);
    app.ott = Some(runtime);
}

fn automatic_resume_point(app: &App) -> SavedResume {
    let runtime = app.ott.as_ref().unwrap();
    let key = &runtime.active().unwrap().id;
    runtime.checkpoints.get(key).or_else(|| runtime.saved.resume.get(key))
        .expect("effective checkpoint or original durable bookmark").clone()
}

#[test]
fn automatic_resume_preroll_exit_preserves_the_complete_bookmark() {
    let _serial = plx_base::testlock::serial();
    let _reset = ResetMedia;
    let mut app = automatic_resume_fixture();
    let before = automatic_resume_point(&app);
    let saved = serde_json::to_value(&app.ott.as_ref().unwrap().saved).unwrap();
    automatic_resume_checkpoint(&mut app);
    assert!(automatic_resume_point(&app) == before, "periodic preroll must preserve all bookmark fields");
    exit_through_content(&mut app);
    assert!(app.player.pending_exit.is_some(), "the real Exit must provide its pre-teardown receipt");
    runtime_tick(&mut app);
    assert!(automatic_resume_point(&app) == before, "early Exit must not persist the backward keyframe position");
    assert_eq!(serde_json::to_value(&app.ott.as_ref().unwrap().saved).unwrap(), saved,
        "held writer leaves the entire durable image unchanged");
    assert!(app.ott.as_ref().unwrap().playing.is_none());
}

#[test]
fn automatic_resume_credited_progress_releases_the_floor() {
    let _serial = plx_base::testlock::serial();
    let _reset = ResetMedia;
    let mut app = automatic_resume_fixture();
    automatic_resume_clock(42_000, true);
    automatic_resume_checkpoint(&mut app);
    assert_eq!(automatic_resume_point(&app).position_ms, 42_000);
    automatic_resume_clock(43_500, true);
    automatic_resume_checkpoint(&mut app);
    assert_eq!(automatic_resume_point(&app).position_ms, 43_500);
    // Crossing the resume point returns to ordinary checkpoint semantics; the
    // guard must not become a permanent maximum-of-all-previous-positions rule.
    automatic_resume_clock(40_000, true);
    automatic_resume_checkpoint(&mut app);
    assert_eq!(automatic_resume_point(&app).position_ms, 40_000);
}

#[test]
fn automatic_resume_optimistic_target_is_not_credited_progress() {
    let _serial = plx_base::testlock::serial();
    let _reset = ResetMedia;
    let mut app = automatic_resume_fixture();
    let before = automatic_resume_point(&app);
    automatic_resume_clock(0, false);
    plx_media::player::SHARED.seeking.store(true, Ordering::Relaxed);
    plx_media::player::SHARED.seek_display_ns.store(42_000_000_000, Ordering::Relaxed);
    plx_media::player::swap_state_for_test(plx_media::player::PlaybackState::Seeking);
    assert_eq!(playback::intended_pos(&app.player.session), 42_000_000_000);
    automatic_resume_checkpoint(&mut app);
    // A synthetic initial position/first callback exactly at the target must
    // not retire protection before the backward keyframe is actually observed.
    automatic_resume_clock(42_000, true);
    automatic_resume_checkpoint(&mut app);
    automatic_resume_clock(39_000, true);
    automatic_resume_checkpoint(&mut app);
    assert!(automatic_resume_point(&app) == before);
}

#[test]
fn automatic_resume_floor_uses_the_effective_finite_seek_target() {
    let _serial = plx_base::testlock::serial();
    for (duration_ns, expected_ms) in [(10_000_000_000, 10_000), (0, 8_000)] {
        let _reset = ResetMedia;
        let mut app = automatic_resume_fixture_with_duration(duration_ns);
        automatic_resume_clock(8_000, true);
        automatic_resume_checkpoint(&mut app);
        assert_eq!(automatic_resume_point(&app).position_ms, expected_ms,
            "clamp to an admitted finite seek; unknown duration did not request a resume seek");
    }
}

#[test]
fn automatic_resume_pause_and_resume_preserve_preroll_protection() {
    let _serial = plx_base::testlock::serial();
    let _reset = ResetMedia;
    let mut app = automatic_resume_fixture();
    let before = automatic_resume_point(&app);
    assert_eq!(remote_transport(&mut app, json!({"operation":"pause"}))["status"], "ok");
    assert!(lifecycle::viewer_paused());
    automatic_resume_checkpoint(&mut app);
    assert!(automatic_resume_point(&app) == before);
    assert_eq!(remote_transport(&mut app, json!({"operation":"resume"}))["status"], "ok");
    automatic_resume_checkpoint(&mut app);
    assert!(automatic_resume_point(&app) == before, "Resume is not a request to seek backwards");
}

#[test]
fn automatic_resume_internal_seek_plus_pause_is_not_explicit_seek() {
    let _serial = plx_base::testlock::serial();
    let _reset = ResetMedia;
    let mut app = automatic_resume_fixture();
    let before = automatic_resume_point(&app);
    // Internal reload/speed paths also reach this low-level mailbox. Its
    // presence cannot relabel an unrelated accepted Pause as a viewer seek.
    playback::request_seek(12_000_000_000);
    plx_media::player::swap_state_for_test(plx_media::player::PlaybackState::Seeking);
    assert_eq!(playback::intended_pos(&app.player.session), 12_000_000_000);
    assert_eq!(remote_transport(&mut app, json!({"operation":"pause"}))["status"], "ok");
    automatic_resume_checkpoint(&mut app);
    assert!(automatic_resume_point(&app) == before);
    plx_media::player::abandon_seek();
    automatic_resume_clock(39_000, true);
    automatic_resume_checkpoint(&mut app);
    assert!(automatic_resume_point(&app) == before);
}

#[test]
fn automatic_resume_batched_speed_reload_and_pause_preserve_the_floor() {
    let _serial = plx_base::testlock::serial();
    let _reset = ResetMedia;
    let mut app = automatic_resume_fixture();
    let before = automatic_resume_point(&app);
    // The inert Engine does not publish native-start. A speed change only
    // reloads a started decoder; otherwise it is saved for the next Load.
    plx_media::player::TX.started.store(true, Ordering::Relaxed);
    assert!(plx_media::player::is_started());
    // Model an existing nonnormal speed. Returning to Normal is always a
    // supported menu action and avoids assuming decoder speed capabilities.
    plx_media::player::SHARED.play_rate_permille.store(1500, Ordering::Relaxed);
    app.pages.emit(MachineId::Nav, Fx::App(AppFx::Player(PlayerReq::More(
        plx_appkit::more_menu::Action::SetSpeed(plx_media::player::RATE_NORMAL)))));
    // Both decisions drain through the real content performer before a pump:
    // the internal speed reload leaves TX pending when Pause is accepted.
    native_transport(&mut app, PlayerReq::Transport(Some(false)));
    assert_eq!(plx_media::player::play_rate_permille(), plx_media::player::RATE_NORMAL);
    assert!(plx_media::player::SHARED.rate_reload.load(Ordering::Relaxed));
    assert_eq!(plx_media::player::seek_pending(), 38_000_000_000);
    assert!(lifecycle::viewer_paused());
    automatic_resume_checkpoint(&mut app);
    assert!(automatic_resume_point(&app) == before,
        "the pending internal reload must not turn Pause into an explicit seek");
}

#[test]
fn automatic_resume_accepted_remote_and_native_seek_can_lower_bookmark() {
    let _serial = plx_base::testlock::serial();
    for native in [false, true] {
        let _reset = ResetMedia;
        let mut app = automatic_resume_fixture();
        if native {
            native_transport(&mut app, PlayerReq::CommitSeek(12_000_000_000));
        } else {
            let response = remote_transport(&mut app, json!({"operation":"seek","position":12}));
            assert_eq!(response["status"], "ok", "{response}");
            assert_eq!(response["data"]["dispatched"], true);
        }
        assert_eq!(plx_media::player::seek_pending(), 12_000_000_000,
            "the real input performer must have accepted the explicit seek");
        // App admission already treats this queued explicit seek as viewer
        // intent. No physical seek/decoder completion is claimed by this test.
        automatic_resume_clock(12_000, true);
        automatic_resume_checkpoint(&mut app);
        let point = automatic_resume_point(&app);
        assert_eq!(point.position_ms, 12_000);
        assert_eq!(point.entry_id, "first");
    }
}

#[test]
fn canonical_seek_remote_receipts_preserve_finite_timeline_semantics() {
    let _serial = plx_base::testlock::serial();
    for (params, expected, clamped) in [
        (json!({"operation":"seek","position":12.125}), 12.125, None),
        (json!({"operation":"seek","position":60}), 60.0, None),
        (json!({"operation":"seek_relative","offset":10}), 48.0, Some(false)),
        (json!({"operation":"seek_relative","offset":-100}), 0.0, Some(true)),
        (json!({"operation":"seek_relative","offset":100}), 60.0, Some(true)),
    ] {
        let _reset = ResetMedia;
        let mut app = automatic_resume_fixture();
        let identity = plx_media::route::playback_identity(&app.player.session).unwrap();
        let response = remote_transport(&mut app, params.clone());
        assert_eq!(response["status"], "ok", "{params}: {response}");
        assert_eq!(response["data"]["dispatched"], true);
        assert_eq!(response["data"]["position"], json!(expected));
        assert_eq!(plx_media::player::seek_pending(), (expected * 1e9) as i64,
            "the real performer queues exactly the core's finite position");
        assert!(plx_media::route::playback_identity_matches(&app.player.session, &identity),
            "a seek must not replace the logical playback target");
        if let Some(clamped) = clamped {
            assert_eq!(response["data"]["from"], 38.0);
            assert_eq!(response["data"]["offset"].as_f64(), params["offset"].as_f64());
            assert_eq!(response["data"]["clamped"], clamped);
        } else {
            assert!(response["data"].get("clamped").is_none());
        }
    }
}

#[test]
fn canonical_seek_absolute_overshoot_and_unknown_duration_have_no_effect() {
    let _serial = plx_base::testlock::serial();
    for (duration, params, status) in [
        (60_000_000_000, json!({"operation":"seek","position":60.001}), "rejected"),
        (60_000_000_000, json!({"operation":"seek_relative","offset":0}), "unsupported"),
        (0, json!({"operation":"seek_relative","offset":10}), "unsupported"),
    ] {
        let _reset = ResetMedia;
        let mut app = automatic_resume_fixture_with_duration(duration);
        let before = automatic_resume_point(&app);
        let response = remote_transport(&mut app, params.clone());
        assert_eq!(response["status"], status, "{params}: {response}");
        assert_eq!(plx_media::player::seek_pending(), -1);
        assert!(automatic_resume_point(&app) == before,
            "a refused seek cannot change the retained resume checkpoint");
    }
}

#[test]
fn automatic_resume_batched_explicit_seek_then_exit_keeps_the_lower_target() {
    let _serial = plx_base::testlock::serial();
    let _reset = ResetMedia;
    let mut app = automatic_resume_fixture();
    app.pages.emit(MachineId::Nav, Fx::App(AppFx::Player(
        PlayerReq::CommitSeek(12_000_000_000))));
    // Exit is queued after the accepted seek, in the same content drain and
    // before any media pump. Its intended-position receipt must retain 12s.
    exit_through_content(&mut app);
    assert_eq!(app.player.pending_exit.as_ref().unwrap().position_ns, 12_000_000_000);
    runtime_tick(&mut app);
    assert_eq!(automatic_resume_point(&app).position_ms, 12_000);
    assert!(app.ott.as_ref().unwrap().playing.is_none());
}

#[test]
fn automatic_resume_queued_origin_survives_suspend_without_promoting_explicit_seek() {
    let _serial = plx_base::testlock::serial();
    for automatic in [true, false] {
        let _reset = ResetMedia;
        let mut app = automatic_resume_queued_fixture(automatic);
        // Exercise the Runtime boundary invoked by the OS event path, with
        // the real WindowActivity already closed. No SDL/decoder runs here.
        app.window_activity.event(0x104);
        assert!(!playback_allowed(&app));
        super::super::super::suspend(&mut app);
        assert_eq!(app.ott.as_ref().unwrap().resume_pending.as_ref().unwrap().position_ms, 42_000);
        assert!(app.ott.as_ref().unwrap().play_target.is_none());
        app.window_activity.event(0x106);
        super::super::super::resume(&mut app);
        let mut runtime = app.ott.take().unwrap();
        runtime.catalog_done.store(true, Ordering::Release);
        runtime.rearm_source_work(&mut app);
        // Supply the normal validation result using the same retained live
        // root. The worker slot is held again before any subsequent tick.
        runtime.catalog_done.store(false, Ordering::Release);
        let generation = runtime.generation;
        let sources = runtime.local_inventory.sources.clone();
        runtime.local_inventory.phase = transient::InventoryPhase::Loading(generation);
        transient::binding_result(&mut runtime, &mut app, generation, Ok(ott_catalog::LocalBindings {
            sources, lookup: None, matched: vec![], operation_error: None,
        }));
        assert_eq!(transient::lifecycle_resume_ready(&runtime,
            runtime.resume_pending.as_ref().unwrap()), Ok(true));
        assert!(runtime.resolver_busy);
        app.ott = Some(runtime);
        runtime_tick(&mut app);
        let runtime = app.ott.as_ref().unwrap();
        assert!(runtime.resume_pending.is_none());
        assert_eq!(runtime.play_target.as_ref().unwrap().position_ms, 42_000);
        assert_eq!(runtime.restore_play_generation,
            automatic.then_some(runtime.play_generation));
        assert_eq!(automatic_resume_admit(&mut app), 42_000_000_000);
        automatic_resume_clock(38_000, true);
        automatic_resume_checkpoint(&mut app);
        let expected_ms = if automatic { 42_000 } else { 38_000 };
        assert_eq!(automatic_resume_point(&app).position_ms, expected_ms);
        exit_through_content(&mut app);
        runtime_tick(&mut app);
        assert_eq!(automatic_resume_point(&app).position_ms, expected_ms);
    }
}

#[test]
fn automatic_resume_rejected_seek_does_not_release_the_floor() {
    let _serial = plx_base::testlock::serial();
    for kind in 0..4 {
        let _reset = ResetMedia;
        let mut app = automatic_resume_fixture();
        let before = automatic_resume_point(&app);
        let mut held_engine = None;
        let params = match kind {
            0 => json!({"operation":"seek","position":-1}),
            1 => json!({"operation":"seek","position":90}),
            2 => {
                // Authenticated remote seek is allowed by strict policy, but
                // a background window must reject it before native admission.
                app.window_activity.event(0x104);
                assert!(!playback_allowed(&app));
                json!({"operation":"seek","position":12})
            }
            _ => {
                held_engine = app.adapters.player.take();
                json!({"operation":"seek","position":12})
            }
        };
        let response = remote_transport(&mut app, params);
        assert_eq!(response["status"], if kind == 0 { "unsupported" } else { "rejected" },
            "case {kind}: {response}");
        assert_eq!(plx_media::player::seek_pending(), -1, "refusal must precede seek admission");
        if kind == 2 {
            assert_eq!(response["data"]["error"], "Playback is suspended");
            app.window_activity.event(0x106);
            assert!(playback_allowed(&app));
        }
        if let Some(engine) = held_engine { app.adapters.player.install(engine); }
        automatic_resume_checkpoint(&mut app);
        assert!(automatic_resume_point(&app) == before, "case {kind} must retain the complete bookmark");
    }
}

#[test]
fn automatic_resume_eof_zero_and_next_are_not_clamped() {
    let _serial = plx_base::testlock::serial();
    let _reset = ResetMedia;
    let mut app = automatic_resume_fixture();
    app.ott.as_mut().unwrap().catalog.entries.push(local_entry("second"));
    assert!(finish_catalog_playback(&mut app));
    assert_eq!(automatic_resume_point(&app).position_ms, 0, "accepted EOF retains its reset semantics");
    assert_eq!(app.ott.as_ref().unwrap().play_target, Some(PlaybackRequest::ordinary("second", 0)));
    assert_eq!(automatic_resume_admit(&mut app), 0);
    automatic_resume_clock(3_000, true);
    automatic_resume_checkpoint(&mut app);
    assert_eq!(automatic_resume_point(&app).entry_id, "second");
    assert_eq!(automatic_resume_point(&app).position_ms, 3_000);
}

#[test]
fn automatic_resume_explicit_replacement_does_not_inherit_the_floor() {
    let _serial = plx_base::testlock::serial();
    for id in ["first", "second"] {
        let _reset = ResetMedia;
        let mut app = automatic_resume_fixture();
        let old_identity = plx_media::route::playback_identity(&app.player.session);
        app.ott.as_mut().unwrap().catalog.entries.push(local_entry("second"));
        explicit_play(&mut app, id);
        assert_eq!(automatic_resume_admit(&mut app), 0);
        assert_ne!(plx_media::route::playback_identity(&app.player.session), old_identity);
        automatic_resume_clock(4_000, true);
        automatic_resume_checkpoint(&mut app);
        assert_eq!(automatic_resume_point(&app).entry_id, id);
        assert_eq!(automatic_resume_point(&app).position_ms, 4_000);
    }
}

#[test]
fn automatic_resume_stale_exit_receipt_cannot_clear_a_new_restore_floor() {
    let _serial = plx_base::testlock::serial();
    let _reset = ResetMedia;
    let mut app = automatic_resume_fixture();
    let old = app.ott.as_ref().unwrap().playing.as_ref().unwrap();
    let receipt = plx_media::player::machine::ExitReceipt {
        cause: plx_media::player::machine::ExitCause::RequestedExit,
        playback_identity: old.identity.clone(),
        position_ns: 55_000_000_000,
        sid: plx_plex::plex::ServerId::UNSET,
        rating_key: String::new(),
        source: old.source.clone(),
    };
    app.ott.as_mut().unwrap().catalog.entries.push(local_entry("second"));
    automatic_resume_select(&mut app, "second", 50_000);
    assert_eq!(automatic_resume_admit(&mut app), 50_000_000_000);
    let before = automatic_resume_point(&app);
    assert_ne!(receipt.playback_identity, plx_media::route::playback_identity(&app.player.session));
    let mut runtime = app.ott.take().unwrap();
    runtime.checkpoint_at(&app, true, Some(&receipt));
    app.ott = Some(runtime);
    assert!(automatic_resume_point(&app) == before, "foreign receipt cannot write the new checkpoint");
    automatic_resume_clock(46_000, true);
    automatic_resume_checkpoint(&mut app);
    assert!(automatic_resume_point(&app) == before, "foreign receipt must not retire the newer guard either");
}
