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
