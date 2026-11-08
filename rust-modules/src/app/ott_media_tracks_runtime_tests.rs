//! Real raw poll -> command dispatcher -> common native selection. Synthetic
//! inventory and an inert host Engine avoid inspection, network and storage IO.
use super::*;
use plx_media::ff::inspect::{InspectedTrack, TrackInventory, TrackKind};
use plx_media::{player, route};

struct Globals {
    state: u8,
    offset: i64,
}
impl Globals {
    fn capture() -> Self {
        Self {
            state: player::swap_state_for_test(player::PlaybackState::Idle),
            offset: player::subtitle_offset_ms(),
        }
    }
}
impl Drop for Globals {
    fn drop(&mut self) {
        player::SHARED.reset_session();
        player::TX.reset();
        route::reset_player_control_for_test(&route::PlaybackSession::IDLE);
        player::restore_state_for_test(self.state);
        player::restore_subtitle_offset(self.offset);
    }
}
fn track(kind: TrackKind, index: i32, ordinal: i32) -> InspectedTrack {
    InspectedTrack {
        kind,
        stream_index: index,
        ordinal,
        codec: if kind == TrackKind::Audio {
            "aac"
        } else {
            "subrip"
        }
        .into(),
        language: if ordinal == 0 { "eng" } else { "spa" }.into(),
        title: format!("Fixture {ordinal}"),
        channels: 2,
        sample_rate: 48000,
        bit_depth: 16,
        decoder_available: true,
        disposition_known: false,
        default: false,
        forced: false,
    }
}
fn install(app: &mut App, inventory: TrackInventory) {
    install_source(app, inventory,
        plx_media::ff::ExternalSource::Http("https://media.invalid/fixture.mkv".into()));
}
fn install_source(app: &mut App, inventory: TrackInventory, source: plx_media::ff::ExternalSource) {
    playback::replace_player(
        &mut app.player,
        &mut app.adapters.player,
        &mut app.repause_at,
        &mut app.refresh_hubs,
        &mut app.pages,
    );
    route::reset_player_control_for_test(&app.player.session);
    let mut info = unsupported_info();
    info.tracks = inventory;
    info.video_codec = "h264".into();
    info.audio_codec = "aac".into();
    info.audio_channels = 2;
    info.audio_sample_rate = 48000;
    info.audio_bit_depth = 16;
    route::request_external_source(
        &mut app.player.session,
        &info,
        source,
        "Fixture",
        "Tracks",
        plx_platform::devcaps::assumed_shared(),
    )
    .unwrap();
    route::reset_player_control_for_test(&app.player.session);
    app.adapters
        .player
        .install(player::engine::engine_after_reload_for_test());
    assert!(player::pause(&mut app.adapters.player));
    player::swap_state_for_test(player::PlaybackState::Paused);
    player::SHARED
        .playpos_ns
        .store(7_000_000_000, Ordering::Release);
    player::SHARED
        .duration_ns
        .store(60_000_000_000, Ordering::Release);
}
fn fixture() -> App {
    let mut app = app_fixture();
    app.running = true;
    {
        let runtime = app.ott.as_mut().unwrap();
        runtime.loaded = true;
        runtime.entropy = Some([9; 16]);
        runtime.write_permit.revoke();
    }
    install(
        &mut app,
        TrackInventory::complete_for_test(vec![
            track(TrackKind::Audio, 1, 0),
            track(TrackKind::Audio, 4, 1),
            track(TrackKind::Subtitle, 2, 0),
            track(TrackKind::Subtitle, 3, 1),
        ])
        .unwrap(),
    );
    mount_player(&mut app);
    assert!(app.adapters.player.is_live());
    app
}

#[test]
fn media_tracks_closed_app_refuses_selection_without_queueing_native_work() {
    let _lock = plx_base::testlock::serial();
    let _globals = Globals::capture();
    let mut app = fixture();
    let listed = list(&mut app);
    let worker = route::worker_ticket();
    app.running = false;
    let result = invoke(&mut app, select(&listed, "audio", Some(1)));
    assert_eq!(result["data"]["error"], "restricted");
    assert_eq!(route::external_tracks(&app.player.session).unwrap().selected_audio(), Some(0));
    assert_eq!(route::worker_ticket(), worker);
    assert!(!commands::track_request_pending(app.ott.as_ref().unwrap()));
    assert!(route::claim_route_action().is_none());
}
fn request(app: &mut App, mut params: Value) -> Invocation {
    let runtime = app.ott.as_mut().unwrap();
    let runtime_id = runtime.remote.runtime().to_owned();
    let now = runtime.now_ms();
    params["runtime"] = json!(runtime_id);
    runtime.remote = Remote::disabled_for_test("dev_fixture", &runtime_id);
    let body=json!({"commands":[],"server_time":1000,"requests":[{"id":"0123456789abcdef0123456789abcdef","expires_at":1020,"action":"media_tracks","params":params}]}).to_string();
    let mut inv = runtime
        .remote
        .raw_poll_for_test(&body, now)
        .pop()
        .expect("raw invocation");
    inv.deadline_ms = runtime.now_ms() + 10000;
    assert!(inv.media_tracks_projection().is_some());
    inv
}
fn dispatch(app: &mut App, inv: &Invocation) -> Value {
    let mut runtime = app.ott.take().unwrap();
    let value = commands::command(&mut runtime, app, inv).expect("immediate track result");
    app.ott = Some(runtime);
    value
}
fn invoke(app: &mut App, params: Value) -> Value {
    let inv = request(app, params);
    dispatch(app, &inv)
}
fn list(app: &mut App) -> Value {
    invoke(app, json!({"op":"list"}))
}
fn select(list: &Value, kind: &str, index: Option<usize>) -> Value {
    let target = match index {
        Some(index) => {
            let rows: Vec<_> = list["data"]["tracks"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|row| row["kind"] == kind)
                .collect();
            json!({"kind":kind,"mode":"track","id":rows[index]["id"]})
        }
        None => json!({"kind":kind,"mode":"off"}),
    };
    json!({"op":"select","playback":list["data"]["playback"],"source_generation":list["data"]["source_generation"],"catalog":list["data"]["catalog"],"target":target})
}

#[test]
fn media_tracks_raw_dispatch_lists_selectable_audio_and_keeps_noop_synchronous() {
    let _lock = plx_base::testlock::serial();
    let _globals = Globals::capture();
    let mut app = fixture();
    let listed = list(&mut app);
    assert_eq!(listed["status"], "ok");
    assert_eq!(listed["data"]["tracks"].as_array().unwrap().len(), 4);
    assert_eq!(listed["data"]["tracks"][0]["selectable"], true);
    let worker = route::worker_ticket();
    let result = invoke(&mut app, select(&listed, "audio", Some(0)));
    assert_eq!(result["data"]["outcome"], "requested");
    assert_eq!(result["data"]["observed"], Value::Null);
    assert_eq!(route::worker_ticket(), worker);
    assert!(!route::pending_user_route_intent(route::UserRouteIntent::NativeAudioReload));
}

#[test]
fn media_tracks_subtitle_and_off_use_common_owner_without_timeline_or_saved_changes() {
    let _lock = plx_base::testlock::serial();
    let _globals = Globals::capture();
    let mut app = fixture();
    let identity = route::playback_identity(&app.player.session);
    let saved = serde_json::to_value(&app.ott.as_ref().unwrap().saved).unwrap();
    let listed = list(&mut app);
    let on = invoke(&mut app, select(&listed, "subtitle", Some(1)));
    assert_eq!(on["data"]["outcome"], "requested");
    assert_eq!(on["data"]["observed"], Value::Null);
    assert_eq!(player::desired_sub_idx(), 1);
    let listed = list(&mut app);
    let off = invoke(&mut app, select(&listed, "subtitle", None));
    assert_eq!(off["data"]["outcome"], "requested");
    assert_eq!(player::desired_sub_idx(), -1);
    assert_eq!(route::playback_identity(&app.player.session), identity);
    assert_eq!(
        player::SHARED.playpos_ns.load(Ordering::Acquire),
        7_000_000_000
    );
    assert_eq!(
        serde_json::to_value(&app.ott.as_ref().unwrap().saved).unwrap(),
        saved
    );
    assert!(!app.ott.as_ref().unwrap().saving);
    assert!(!route::pending_user_route_intent(
        route::UserRouteIntent::NativeAudioReload
    ));
}

#[test]
fn media_tracks_same_off_cannot_bypass_an_existing_manual_intent() {
    let _lock = plx_base::testlock::serial();
    let _globals = Globals::capture();
    let mut app = fixture();
    let listed = list(&mut app);
    route::request_user_route_intent(
        &app.player.session,
        route::UserRouteIntent::NativeAudioReload,
    );
    // The manual intent retires the earlier receipt even for Subtitle Off.
    let stale = invoke(&mut app, select(&listed, "subtitle", None));
    assert_eq!(stale["data"]["error"], "playback_changed");
    assert_eq!(player::desired_sub_idx(), -1);
    let current = list(&mut app);
    assert_eq!(current["data"]["playback"], listed["data"]["playback"]);
    assert_ne!(current["data"]["source_generation"], listed["data"]["source_generation"]);
    let result = invoke(&mut app, select(&current, "subtitle", None));
    assert_eq!(result["data"]["error"], "busy");
    assert_eq!(player::desired_sub_idx(), -1);
    assert!(route::pending_user_route_intent(
        route::UserRouteIntent::NativeAudioReload
    ));
}

#[test]
fn media_tracks_manual_then_remote_share_the_owner_but_same_url_replacement_retires_receipt() {
    let _lock = plx_base::testlock::serial();
    let _globals = Globals::capture();
    let mut app = fixture();
    let listed = list(&mut app);
    let manual = route::external_tracks(&app.player.session)
        .unwrap()
        .choice(TrackKind::Subtitle, Some(1))
        .unwrap();
    route::apply_external_track(&mut app.player.session, &manual).unwrap();
    // Manual and remote edits share an owner, including its intent-generation fence.
    let stale = invoke(&mut app, select(&listed, "subtitle", Some(0)));
    assert_eq!(stale["data"]["error"], "playback_changed");
    assert_eq!(player::desired_sub_idx(), 1);
    let current = list(&mut app);
    assert_eq!(current["data"]["playback"], listed["data"]["playback"]);
    assert_ne!(current["data"]["source_generation"], listed["data"]["source_generation"]);
    let result = invoke(&mut app, select(&current, "subtitle", Some(0)));
    assert_eq!(result["data"]["outcome"], "requested");
    assert_eq!(player::desired_sub_idx(), 0);
    let listed = list(&mut app);
    let inventory = route::external_tracks(&app.player.session)
        .unwrap()
        .inventory()
        .clone();
    install(&mut app, inventory);
    let result = invoke(&mut app, select(&listed, "subtitle", Some(1)));
    assert_eq!(result["data"]["error"], "playback_changed");
    assert_eq!(player::desired_sub_idx(), -1);
}

#[test]
fn media_tracks_unknown_inventory_differs_from_complete_empty_and_missing_nonce() {
    let _lock = plx_base::testlock::serial();
    let _globals = Globals::capture();
    let mut app = fixture();
    install(&mut app, TrackInventory::default());
    let result = list(&mut app);
    assert_eq!(result["data"]["error"], "catalog_incomplete");
    install(&mut app, TrackInventory::complete_for_test(vec![]).unwrap());
    let result = list(&mut app);
    assert_eq!(result["status"], "ok");
    assert_eq!(result["data"]["tracks"], json!([]));
    let inventory = TrackInventory::complete_for_test(vec![]).unwrap();
    install(&mut app, inventory);
    app.ott.as_mut().unwrap().entropy = None;
    assert_eq!(list(&mut app)["data"]["error"], "unavailable");
}

#[test]
fn media_tracks_expired_superseded_and_standby_requests_leave_selection_unchanged() {
    let _lock = plx_base::testlock::serial();
    let _globals = Globals::capture();
    let mut app = fixture();
    let listed = list(&mut app);
    let mut inv = request(&mut app, select(&listed, "subtitle", Some(0)));
    inv.deadline_ms = app.ott.as_ref().unwrap().now_ms();
    assert_eq!(dispatch(&mut app, &inv)["status"], "rejected");
    let inv = request(&mut app, select(&listed, "subtitle", Some(0)));
    assert!(app.ott.as_ref().unwrap().remote.invocation_is_current(&inv));
    // The real cancellation edge rotates control authority without IO.
    app.ott.as_mut().unwrap().remote.cancel_pairing();
    assert!(!app.ott.as_ref().unwrap().remote.invocation_is_current(&inv));
    assert_eq!(
        dispatch(&mut app, &inv)["data"]["error"],
        "runtime_mismatch"
    );
    let inv = request(&mut app, select(&listed, "subtitle", Some(0)));
    app.ott_standby = true;
    assert_eq!(dispatch(&mut app, &inv)["data"]["error"], "restricted");
    assert_eq!(player::desired_sub_idx(), -1);
}

#[test]
fn media_tracks_kiosk_armed_and_malformed_policy_refuse_without_mutation() {
    let _lock = plx_base::testlock::serial();
    let _globals = Globals::capture();
    let mut app = fixture();
    for policy in [
        json!({"version":1,"strict":true,"origin":{"provider":"m3u","profile":"Fixture","subprofile":null,"source_alias":"Fixture"},"target":{"kind":"armed"}}),
        Value::Null,
    ] {
        let listed = list(&mut app);
        let inv = request(&mut app, select(&listed, "subtitle", Some(0)));
        app.ott
            .as_mut()
            .unwrap()
            .saved
            .settings
            .insert("nativeKiosk".into(), policy);
        let result = dispatch(&mut app, &inv);
        assert_eq!(result["status"], "rejected");
        assert_eq!(player::desired_sub_idx(), -1);
        app.ott
            .as_mut()
            .unwrap()
            .saved
            .settings
            .remove("nativeKiosk");
    }
}

#[test]
fn media_tracks_worker_change_retires_receipt_without_changing_logical_playback() {
    let _lock = plx_base::testlock::serial();
    let _globals = Globals::capture();
    let mut app = fixture();
    let listed = list(&mut app);
    let identity = route::playback_identity(&app.player.session);
    route::note_user_seek_intent(8_000_000_000);
    assert!(route::commit_user_seek());
    let mut runtime = app.ott.take().unwrap();
    commands::after_tick(&mut runtime, &mut app);
    app.ott = Some(runtime);
    let result = invoke(&mut app, select(&listed, "subtitle", Some(0)));
    assert_eq!(result["status"], "rejected");
    assert_eq!(player::desired_sub_idx(), -1);
    assert_eq!(route::playback_identity(&app.player.session), identity);
    let fresh = list(&mut app);
    assert_eq!(fresh["data"]["playback"], listed["data"]["playback"]);
    assert_ne!(
        fresh["data"]["source_generation"],
        listed["data"]["source_generation"]
    );
}

#[test]
fn media_tracks_remote_admission_does_not_override_a_later_manual_choice() {
    let _lock = plx_base::testlock::serial();
    let _globals = Globals::capture();
    let mut app = fixture();
    let listed = list(&mut app);
    let result = invoke(&mut app, select(&listed, "subtitle", Some(0)));
    assert_eq!(result["data"]["outcome"], "requested");
    let manual = route::external_tracks(&app.player.session)
        .unwrap()
        .choice(TrackKind::Subtitle, Some(1))
        .unwrap();
    route::apply_external_track(&mut app.player.session, &manual).unwrap();
    assert_eq!(player::desired_sub_idx(), 1);
    assert!(!route::pending_user_route_intent(
        route::UserRouteIntent::NativeAudioReload
    ));
}

#[test]
fn media_tracks_raw_duplicate_keys_never_reach_the_selection_dispatcher() {
    let _lock = plx_base::testlock::serial();
    let _globals = Globals::capture();
    let mut app = fixture();
    let runtime = app.ott.as_mut().unwrap();
    let now = runtime.now_ms();
    let body = r#"{"commands":[],"server_time":1000,"requests":[{"id":"0123456789abcdef0123456789abcdef","expires_at":1020,"action":"media_tracks","params":{"op":"list","op":"select","runtime":"fixture"}}]}"#;
    let actions = runtime.remote.raw_poll_for_test(body, now);
    assert!(actions.is_empty());
    assert_eq!(player::desired_sub_idx(), -1);
}

#[test]
fn media_tracks_missing_playback_and_hls_do_not_advertise_an_actionable_catalog() {
    let _lock = plx_base::testlock::serial();
    let _globals = Globals::capture();
    let mut app = fixture();
    playback::replace_player(
        &mut app.player,
        &mut app.adapters.player,
        &mut app.repause_at,
        &mut app.refresh_hubs,
        &mut app.pages,
    );
    route::forget_external_source(&mut app.player.session);
    assert_eq!(list(&mut app)["data"]["error"], "no_playback");
    assert_eq!(track_capability(&mut app, 1), Value::Null);
    route::reset_player_control_for_test(&app.player.session);
    let mut info = unsupported_info();
    info.video_codec = "h264".into();
    info.audio_codec = "aac".into();
    info.audio_channels = 2;
    info.audio_sample_rate = 48000;
    info.audio_bit_depth = 16;
    info.tracks =
        TrackInventory::complete_for_test(vec![track(TrackKind::Subtitle, 2, 0)]).unwrap();
    route::request_external_source(
        &mut app.player.session,
        &info,
        plx_media::ff::ExternalSource::Hls("https://media.invalid/fixture.m3u8".into()),
        "Fixture",
        "Tracks",
        plx_platform::devcaps::assumed_shared(),
    )
    .unwrap();
    route::reset_player_control_for_test(&app.player.session);
    app.adapters
        .player
        .install(player::engine::engine_after_reload_for_test());
    assert!(player::pause(&mut app.adapters.player));
    player::swap_state_for_test(player::PlaybackState::Paused);
    assert_eq!(list(&mut app)["data"]["error"], "unsupported");
    assert_eq!(track_capability(&mut app, 1), Value::Null);
}


#[test]
fn media_tracks_raw_audio_selection_enters_shared_route_instead_of_rejecting() {
    let _lock = plx_base::testlock::serial();
    let _globals = Globals::capture();
    let mut app = fixture();
    let listed = list(&mut app);
    let inv = request(&mut app, select(&listed, "audio", Some(1)));
    let mut runtime = app.ott.take().unwrap();
    assert!(commands::command(&mut runtime, &mut app, &inv).is_none());
    app.ott = Some(runtime);
    assert!(route::pending_user_route_intent(route::UserRouteIntent::NativeAudioReload));
    assert_eq!(route::external_tracks(&app.player.session).unwrap().selected_audio(), Some(1));
}

fn queue_changed_audio(app: &mut App) -> Invocation {
    let listed = list(app);
    let inv = request(app, select(&listed, "audio", Some(1)));
    let mut runtime = app.ott.take().unwrap();
    assert!(commands::command(&mut runtime, app, &inv).is_none(), "changed audio waits for the real route owner");
    assert!(runtime.remote.response_for_id_for_test(&inv.id).is_none());
    app.ott = Some(runtime);
    assert_eq!(route::external_tracks(&app.player.session).unwrap().selected_audio(), Some(1));
    assert!(route::pending_user_route_intent(route::UserRouteIntent::NativeAudioReload));
    inv
}
fn poll_audio(app: &mut App, now: Option<u64>) {
    let mut runtime = app.ott.take().unwrap();
    commands::before_playback_tick(&mut runtime, app, now);
    app.ott = Some(runtime);
}
fn audio_now(app: &App) -> u64 { app.ott.as_ref().unwrap().now_ms() }
fn audio_response(app: &App, inv: &Invocation) -> Option<Value> {
    app.ott.as_ref().unwrap().remote.response_for_id_for_test(&inv.id)
}
fn claim_audio_start(app: &mut App) -> route::RouteStartAttempt {
    let action = route::claim_route_action().expect("real queued audio action");
    assert_eq!(action.intent, route::RouteIntent::User(route::UserRouteIntent::NativeAudioReload));
    assert!(route::finish_route_action(&mut app.player.session, &action, route::RouteApplyResult::Prepared));
    let transaction = route::pending_route_start().expect("prepared native transaction");
    route::claim_route_start_attempt(transaction).expect("exact native attempt")
}
fn pump_native_answer(app: &mut App, attempt: route::RouteStartAttempt, result: route::RouteStartResult) {
    // This is the native Load mailbox, not a fake command-success callback.
    // The ordinary pump drains it with the inert host Engine installed. No
    // network, decoder, or native worker is started by this host fixture.
    route::publish_route_start_result(attempt, result);
    player::pump(&mut app.player.session, &mut app.adapters.player, &mut app.player.volume, 0);
}

#[test]
fn media_tracks_audio_reply_waits_for_exact_native_mailbox_completion() {
    let _lock = plx_base::testlock::serial();
    let _globals = Globals::capture();
    let mut app = fixture();
    let before = serde_json::to_value(&app.ott.as_ref().unwrap().saved).unwrap();
    let inv = queue_changed_audio(&mut app);
    let now = audio_now(&app);
    poll_audio(&mut app, Some(now));
    assert!(audio_response(&app, &inv).is_none());
    let attempt = claim_audio_start(&mut app);
    poll_audio(&mut app, Some(now));
    assert!(audio_response(&app, &inv).is_none());
    pump_native_answer(&mut app, attempt, route::RouteStartResult::Started);
    assert_eq!(route::route_start_status(attempt), route::RouteStartStatus::Started);
    poll_audio(&mut app, Some(now));
    let response = audio_response(&app, &inv).expect("delivery completed after native acceptance");
    assert_eq!(response["status"], "ok");
    assert_eq!(response["data"]["outcome"], "requested");
    assert_eq!(response["data"]["observed"], Value::Null);
    assert_eq!(serde_json::to_value(&app.ott.as_ref().unwrap().saved).unwrap(), before);
    assert!(!commands::track_request_pending(app.ott.as_ref().unwrap()));
}

#[test]
fn media_tracks_audio_expiry_before_pump_restores_only_its_queued_proposal() {
    let _lock = plx_base::testlock::serial();
    let _globals = Globals::capture();
    let mut app = fixture();
    let inv = queue_changed_audio(&mut app);
    poll_audio(&mut app, Some(inv.deadline_ms));
    assert_eq!(audio_response(&app, &inv).unwrap()["data"]["error"], "unavailable");
    assert_eq!(route::external_tracks(&app.player.session).unwrap().selected_audio(), Some(0));
    assert_eq!(player::SHARED.desired_audio_idx.load(Ordering::Acquire), 0);
    assert!(!player::audio_switching());
    assert!(route::claim_route_action().is_none());
    player::pump(&mut app.player.session, &mut app.adapters.player, &mut app.player.volume, 0);
    assert!(route::pending_route_start().is_none());
}

#[test]
fn media_tracks_audio_clock_failure_and_standby_cancel_before_native_work() {
    let _lock = plx_base::testlock::serial();
    let _globals = Globals::capture();
    for clock_missing in [false, true] {
        let mut app = fixture();
        let inv = queue_changed_audio(&mut app);
        let now = audio_now(&app);
        app.ott_standby = !clock_missing;
        poll_audio(&mut app, if clock_missing { None } else { Some(now) });
        let response = audio_response(&app, &inv).unwrap();
        assert_eq!(response["data"]["error"], if clock_missing { "unavailable" } else { "restricted" });
        assert_eq!(route::external_tracks(&app.player.session).unwrap().selected_audio(), Some(0));
        assert!(route::claim_route_action().is_none());
    }
}

#[test]
fn media_tracks_audio_policy_change_cancels_without_bypassing_strict_mode() {
    let _lock = plx_base::testlock::serial();
    let _globals = Globals::capture();
    let mut app = fixture();
    let inv = queue_changed_audio(&mut app);
    app.ott.as_mut().unwrap().saved.settings.insert("nativeKiosk".into(), Value::Null);
    let now = audio_now(&app);
    poll_audio(&mut app, Some(now));
    assert_eq!(audio_response(&app, &inv).unwrap()["data"]["error"], "restricted");
    assert!(route::claim_route_action().is_none());
}

#[test]
fn media_tracks_audio_native_failure_and_cancelled_claim_never_report_success() {
    let _lock = plx_base::testlock::serial();
    let _globals = Globals::capture();
    for fail_load in [false, true] {
        let mut app = fixture();
        let inv = queue_changed_audio(&mut app);
        if fail_load {
            let attempt = claim_audio_start(&mut app);
            pump_native_answer(&mut app, attempt, route::RouteStartResult::StartFailed);
        } else {
            let claim = route::claim_route_action().unwrap();
            assert!(route::finish_route_action(&mut app.player.session, &claim, route::RouteApplyResult::Cancelled));
        }
        let now = audio_now(&app);
        poll_audio(&mut app, Some(now));
        assert_ne!(audio_response(&app, &inv).unwrap()["status"], "ok");
        assert!(!commands::track_request_pending(app.ott.as_ref().unwrap()));
    }
}

#[test]
fn media_tracks_audio_late_completion_cannot_touch_a_same_url_new_playback() {
    let _lock = plx_base::testlock::serial();
    let _globals = Globals::capture();
    let mut app = fixture();
    let inv = queue_changed_audio(&mut app);
    let attempt = claim_audio_start(&mut app);
    let inventory = route::external_tracks(&app.player.session).unwrap().inventory().clone();
    install(&mut app, inventory);
    let identity = route::playback_identity(&app.player.session);
    pump_native_answer(&mut app, attempt, route::RouteStartResult::Started);
    let now = audio_now(&app);
    poll_audio(&mut app, Some(now));
    assert_eq!(audio_response(&app, &inv).unwrap()["data"]["error"], "playback_changed");
    assert_eq!(route::playback_identity(&app.player.session), identity);
    assert_eq!(route::external_tracks(&app.player.session).unwrap().selected_audio(), Some(0));
}

#[test]
fn media_tracks_audio_late_cancel_preserves_a_newer_manual_track() {
    let _lock = plx_base::testlock::serial();
    let _globals = Globals::capture();
    let mut app = fixture();
    let inv = queue_changed_audio(&mut app);
    let attempt = claim_audio_start(&mut app);
    pump_native_answer(&mut app, attempt, route::RouteStartResult::Started);
    player::swap_state_for_test(player::PlaybackState::Paused);
    let manual = route::external_tracks(&app.player.session).unwrap().choice(TrackKind::Audio, Some(0)).unwrap();
    route::apply_external_track(&mut app.player.session, &manual).unwrap();
    poll_audio(&mut app, Some(inv.deadline_ms));
    assert_eq!(audio_response(&app, &inv).unwrap()["data"]["error"], "unavailable");
    assert_eq!(route::external_tracks(&app.player.session).unwrap().selected_audio(), Some(0));
    assert!(route::pending_user_route_intent(route::UserRouteIntent::NativeAudioReload));
}

#[test]
fn media_tracks_audio_catalog_refresh_does_not_extend_the_original_request_deadline() {
    let _lock = plx_base::testlock::serial();
    let _globals = Globals::capture();
    let mut app = fixture();
    let inv = queue_changed_audio(&mut app);
    let runtime = app.ott.as_mut().unwrap();
    let body = json!({"commands":[],"server_time":1000,"requests":[{
        "id":"1123456789abcdef0123456789abcdef","expires_at":1020,"action":"media_tracks",
        "params":{"op":"list","runtime":runtime.remote.runtime()}
    }]}).to_string();
    let now = runtime.now_ms();
    let mut pending = runtime.remote.raw_poll_for_test(&body, now).pop().unwrap();
    pending.deadline_ms = runtime.now_ms() + 20000;
    let _ = dispatch(&mut app, &pending);
    poll_audio(&mut app, Some(inv.deadline_ms));
    assert_eq!(audio_response(&app, &inv).unwrap()["data"]["error"], "unavailable");
    assert!(route::claim_route_action().is_none());
}


#[test]
fn media_tracks_audio_remote_generation_change_retires_only_the_old_proposal() {
    let _lock = plx_base::testlock::serial();
    let _globals = Globals::capture();
    let mut app = fixture();
    let inv = queue_changed_audio(&mut app);
    app.ott.as_mut().unwrap().remote.cancel_pairing();
    let now = audio_now(&app);
    poll_audio(&mut app, Some(now));
    assert!(audio_response(&app, &inv).is_none(), "old generation cannot publish into new delivery state");
    assert!(!commands::track_request_pending(app.ott.as_ref().unwrap()));
    assert_eq!(route::external_tracks(&app.player.session).unwrap().selected_audio(), Some(0));
    assert!(route::claim_route_action().is_none());
}


#[test]
fn media_tracks_audio_worker_change_before_claim_cannot_reload_a_new_timeline() {
    let _lock = plx_base::testlock::serial();
    let _globals = Globals::capture();
    let mut app = fixture();
    let inv = queue_changed_audio(&mut app);
    route::note_user_seek_intent(15_000_000_000);
    assert!(route::commit_user_seek());
    let timeline = route::worker_ticket();
    let now = audio_now(&app);
    poll_audio(&mut app, Some(now));
    assert_eq!(audio_response(&app, &inv).unwrap()["data"]["error"], "catalog_changed");
    assert_eq!(route::worker_ticket(), timeline);
    assert_eq!(route::external_tracks(&app.player.session).unwrap().selected_audio(), Some(0));
    assert!(route::claim_route_action().is_none());
}

#[test]
fn media_tracks_audio_expiry_after_native_dispatch_cannot_fake_a_route_rollback() {
    let _lock = plx_base::testlock::serial();
    let _globals = Globals::capture();
    let mut app = fixture();
    let inv = queue_changed_audio(&mut app);
    let attempt = claim_audio_start(&mut app);
    poll_audio(&mut app, Some(inv.deadline_ms));
    assert_eq!(audio_response(&app, &inv).unwrap()["data"]["error"], "unavailable");
    assert_eq!(route::route_start_status(attempt), route::RouteStartStatus::Pending);
    assert_eq!(route::external_tracks(&app.player.session).unwrap().selected_audio(), Some(1));
    pump_native_answer(&mut app, attempt, route::RouteStartResult::Started);
    let now = audio_now(&app);
    poll_audio(&mut app, Some(now));
    assert_eq!(audio_response(&app, &inv).unwrap()["data"]["error"], "unavailable");
    assert!(!commands::track_request_pending(app.ott.as_ref().unwrap()));
}

/// Admit another real request without replacing delivery state or invalidating
/// the already pending audio invocation. IDs belong to this test's raw ingress.
fn additional_raw_request(
    app: &mut App,
    id: &str,
    action: &str,
    mut params: Value,
) -> (Invocation, Option<Value>) {
    let mut runtime = app.ott.take().unwrap();
    if action == "media_tracks" {
        params["runtime"] = json!(runtime.remote.runtime());
    }
    let body = json!({"commands":[],"server_time":1000,"requests":[{
        "id":id,"expires_at":1020,"action":action,"params":params
    }]}).to_string();
    let now = runtime.now_ms();
    let mut invocations = runtime.remote.raw_poll_for_test(&body, now);
    assert_eq!(invocations.len(), 1, "new raw request crosses real ingress");
    let mut invocation = invocations.remove(0);
    invocation.deadline_ms = now.saturating_add(10_000);
    let response = commands::command(&mut runtime, app, &invocation);
    app.ott = Some(runtime);
    (invocation, response)
}

#[test]
fn media_tracks_audio_pending_kiosk_writer_cancels_queued_route_before_pump() {
    let _lock = plx_base::testlock::serial();
    let _globals = Globals::capture();
    let mut app = fixture();
    {
        // Bare `kiosk on` uses a real catalog to stage an armed Saved candidate.
        // The native fixture stays the currently playing source; no new media
        // request or synthetic policy mutation is used to create the fence.
        let runtime = app.ott.as_mut().unwrap();
        let profile = synthetic_profile();
        runtime.provider = profile.kind;
        runtime.saved.profiles.profiles = vec![profile.clone()];
        runtime.saved.profiles.active_profile = Some(profile.id.clone());
        runtime.catalog.entries = vec![synthetic_entry()];
        runtime.catalog_complete = true;
        runtime.catalog_loading = false;
    }
    let before = serde_json::to_value(&app.ott.as_ref().unwrap().saved).unwrap();
    let audio = queue_changed_audio(&mut app);
    let (kiosk, response) = additional_raw_request(
        &mut app,
        "2123456789abcdef0123456789abcdef",
        "kiosk",
        json!({"mode":"on"}),
    );
    assert!(response.is_none(), "the actual kiosk writer owns the response");
    let writer_job = {
        let runtime = app.ott.as_ref().unwrap();
        assert!(runtime.saving);
        assert!(commands::kiosk_pending(runtime));
        assert!(super::super::input_policy::pending(runtime));
        assert_eq!(runtime.track_selection_kiosk(), Ok(crate::app::ott_kiosk_policy::State::Off));
        assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), before);
        assert!(runtime.remote.invocation_is_current(&audio));
        // The normal writer really runs with the fixture's revoked permit and
        // returns before file IO. Hold its result: Saved has not been consumed.
        let job = runtime.rx.recv_timeout(Duration::from_secs(5)).expect("actual writer response");
        assert!(matches!(&job, Job::Saved(Err(_), Some(inv), false, _) if inv.id == kiosk.id));
        job
    };
    assert!(route::pending_user_route_intent(route::UserRouteIntent::NativeAudioReload));
    let (_, list_response) = additional_raw_request(
        &mut app,
        "3123456789abcdef0123456789abcdef",
        "media_tracks",
        json!({"op":"list"}),
    );
    let listed = list_response.expect("read-only list remains synchronous");
    assert_eq!(listed["status"], "ok");
    assert_eq!(listed["data"]["tracks"].as_array().unwrap().len(), 4);
    assert!(listed["data"]["tracks"].as_array().unwrap().iter()
        .all(|row| row["selectable"] == false));
    assert_eq!(listed["data"]["subtitle_off_supported"], false);
    let (_, selection_response) = additional_raw_request(
        &mut app,
        "4123456789abcdef0123456789abcdef",
        "media_tracks",
        select(&listed, "subtitle", Some(0)),
    );
    assert_eq!(selection_response.unwrap()["status"], "rejected");
    assert!(audio_response(&app, &audio).is_none());
    // Use the application pre-pump hook, including its fresh clock sample.
    super::super::before_playback_tick(&mut app);
    assert_eq!(audio_response(&app, &audio).unwrap()["data"]["error"], "restricted");
    assert!(!commands::track_request_pending(app.ott.as_ref().unwrap()));
    assert_eq!(route::external_tracks(&app.player.session).unwrap().selected_audio(), Some(0));
    assert_eq!(player::SHARED.desired_audio_idx.load(Ordering::Acquire), 0);
    assert!(!player::audio_switching());
    assert!(route::claim_route_action().is_none());
    player::pump(&mut app.player.session, &mut app.adapters.player, &mut app.player.volume, 0);
    assert!(route::pending_route_start().is_none());
    let runtime = app.ott.as_ref().unwrap();
    assert!(commands::kiosk_pending(runtime), "audio cancellation does not retire the kiosk writer");
    assert!(runtime.saving);
    assert!(runtime.remote.response_for_id_for_test(&kiosk.id).is_none());
    assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), before);
    drop(writer_job);
}

// These calls use unique raw delivery IDs and settle each synchronous response.
// No reset of Remote can disguise a duplicate request or retire an audio request.
fn adapter_query(app: &mut App, serial: u32, action: &str, params: Value) -> Value {
    let id = format!("{serial:032x}");
    let (invocation, response) = additional_raw_request(app, &id, action, params);
    let runtime = app.ott.as_mut().unwrap();
    assert!(runtime.remote.invocation_is_current(&invocation));
    assert!(invocation.deadline_ms > runtime.now_ms());
    let response = response.expect("synchronous adapter query");
    runtime.remote.complete(invocation, response.clone());
    response
}

fn track_capability(app: &mut App, serial: u32) -> Value {
    let response = adapter_query(app, serial, "capabilities", json!({}));
    assert_eq!(response["status"], "ok");
    response["data"]["media_tracks"].clone()
}

fn locked_track_policy() -> Value {
    // A valid persisted intent, not a filesystem grant or a claim of playback.
    crate::app::ott_kiosk_stored::Stored::local_folder(
        plx_data::ott::favorites::Origin {
            provider: "local".into(), profile: "Fixture".into(),
            subprofile: None, source_alias: "Fixture".into(),
        },
        true, "fixture_root".into(), "a".repeat(64),
    ).unwrap().encode().unwrap()
}

fn assert_track_error(response: &Value, invocation: &Invocation, code: &str) {
    let raw = invocation.media_tracks_projection().expect("retained raw projection");
    let raw: Value = serde_json::from_slice(raw).unwrap();
    let params = &raw["params"];
    assert_eq!(response, &json!({"status":"rejected","data":{
        "version":1,"runtime":params["runtime"],"op":"select",
        "state":"error","error":code,"playback":params["playback"],
        "source_generation":params["source_generation"],"catalog":params["catalog"],
        "target":params["target"]
    }}));
}

#[test]
fn media_tracks_capability_is_passive_and_complete_empty_is_available() {
    let _lock = plx_base::testlock::serial();
    let _globals = Globals::capture();
    let mut app = fixture();
    let saved = serde_json::to_value(&app.ott.as_ref().unwrap().saved).unwrap();
    let counter = app.ott.as_ref().unwrap().uuid_counter;
    let worker = route::worker_ticket();
    let identity = route::playback_identity(&app.player.session);
    for serial in 1..=3 {
        assert_eq!(track_capability(&mut app, serial), json!({"version":1}));
    }
    assert_eq!(app.ott.as_ref().unwrap().uuid_counter, counter, "capability mints no IDs");
    let listed = adapter_query(&mut app, 4, "media_tracks", json!({"op":"list"}));
    let counter = app.ott.as_ref().unwrap().uuid_counter;
    assert_eq!(track_capability(&mut app, 5), json!({"version":1}));
    assert_eq!(app.ott.as_ref().unwrap().uuid_counter, counter);
    let again = adapter_query(&mut app, 6, "media_tracks", json!({"op":"list"}));
    assert_eq!(again["data"]["catalog"], listed["data"]["catalog"]);
    assert_eq!(again["data"]["source_generation"], listed["data"]["source_generation"]);
    assert_eq!(route::worker_ticket(), worker);
    assert_eq!(route::playback_identity(&app.player.session), identity);
    assert!(route::claim_route_action().is_none());
    assert!(!commands::track_request_pending(app.ott.as_ref().unwrap()));
    assert!(!app.ott.as_ref().unwrap().saving);
    assert_eq!(serde_json::to_value(&app.ott.as_ref().unwrap().saved).unwrap(), saved);

    install(&mut app, TrackInventory::complete_for_test(vec![]).unwrap());
    assert_eq!(track_capability(&mut app, 7), json!({"version":1}));
    let empty = adapter_query(&mut app, 8, "media_tracks", json!({"op":"list"}));
    assert_eq!(empty["status"], "ok");
    assert_eq!(empty["data"]["tracks"], json!([]));
    assert_eq!(empty["data"]["subtitle_off_supported"], true);
    assert!(route::external_tracks(&app.player.session).unwrap()
        .choice(TrackKind::Subtitle, None).is_ok(), "Off is a real native choice");
}

#[test]
fn media_tracks_capability_rejects_unready_state_and_unreadable_policy() {
    let _lock = plx_base::testlock::serial();
    let _globals = Globals::capture();
    let mut app = fixture();
    for (index, state) in [player::PlaybackState::Idle, player::PlaybackState::Error,
        player::PlaybackState::Resolving, player::PlaybackState::Connecting].into_iter().enumerate() {
        player::swap_state_for_test(state);
        assert_eq!(track_capability(&mut app, index as u32 + 1), Value::Null);
    }
    player::swap_state_for_test(player::PlaybackState::Paused);
    app.ott.as_mut().unwrap().loaded = false;
    assert_eq!(track_capability(&mut app, 5), Value::Null);
    app.ott.as_mut().unwrap().loaded = true;
    app.ott.as_mut().unwrap().persistence_uncertain = true;
    assert_eq!(track_capability(&mut app, 6), Value::Null);
    app.ott.as_mut().unwrap().persistence_uncertain = false;
    app.ott.as_mut().unwrap().saved.settings.insert("nativeKiosk".into(), Value::Null);
    assert_eq!(track_capability(&mut app, 7), Value::Null);
    app.ott.as_mut().unwrap().saved.settings.remove("nativeKiosk");
    assert_eq!(track_capability(&mut app, 8), json!({"version":1}));
    install(&mut app, TrackInventory::default());
    assert_eq!(track_capability(&mut app, 9), Value::Null);
    assert_eq!(adapter_query(&mut app, 10, "media_tracks", json!({"op":"list"}))["data"]["error"],
        "catalog_incomplete");
}

#[test]
fn media_tracks_capability_and_list_share_exact_inventory_bounds() {
    let _lock = plx_base::testlock::serial();
    let _globals = Globals::capture();
    let mut app = fixture();
    let mut serial = 1;
    for (audio, subtitles, eligible) in [(64, 64, true), (65, 0, false), (0, 65, false), (64, 65, false)] {
        let rows = (0..audio).map(|i| track(TrackKind::Audio, i, i))
            .chain((0..subtitles).map(|i| track(TrackKind::Subtitle, audio + i, i))).collect();
        install(&mut app, TrackInventory::complete_for_test(rows).unwrap());
        assert_eq!(track_capability(&mut app, serial), if eligible { json!({"version":1}) } else { Value::Null });
        serial += 1;
        let listed = adapter_query(&mut app, serial, "media_tracks", json!({"op":"list"}));
        serial += 1;
        if eligible {
            assert_eq!(listed["status"], "ok");
            assert_eq!(listed["data"]["tracks"].as_array().unwrap().len(), 128);
        } else {
            assert_eq!(listed["data"]["error"], "catalog_incomplete");
        }
        assert!(route::claim_route_action().is_none());
    }
}

#[test]
fn media_tracks_capability_requires_complete_bounded_adjacent_inventory() {
    let _lock = plx_base::testlock::serial();
    let _globals = Globals::capture();
    let mut app = fixture();
    install_source(&mut app,
        TrackInventory::complete_for_test(vec![track(TrackKind::Subtitle, 2, 0)]).unwrap(),
        plx_media::ff::ExternalSource::Local("/fixture/film.mkv".into()));
    // Synthetic native facts exercise publication only; no pathname is opened
    // and this test does not claim an app-authorized Local discovery occurred.
    let publish = |app: &mut App, count, complete| {
        let playback = route::playback_identity(&app.player.session).unwrap();
        let catalog = route::external_tracks(&app.player.session).unwrap().clone();
        let rows = (0..count).map(|_| route::LocalSubtitleTrack {
            target: Default::default(), format: route::SidecarFormat::Srt,
            title: "Adjacent".into(), language: "eng".into(),
        }).collect();
        route::install_external_local_subtitles(&mut app.player.session,
            &playback, &catalog, rows, complete, 129, 65).unwrap();
    };
    publish(&mut app, 1, false);
    assert_eq!(track_capability(&mut app, 1), Value::Null);
    publish(&mut app, 63, true);
    assert_eq!(track_capability(&mut app, 2), json!({"version":1}));
    let bounded = adapter_query(&mut app, 3, "media_tracks", json!({"op":"list"}));
    assert_eq!(bounded["data"]["tracks"].as_array().unwrap().len(), 64);
    publish(&mut app, 64, true);
    assert_eq!(track_capability(&mut app, 4), Value::Null);
    assert_eq!(adapter_query(&mut app, 5, "media_tracks", json!({"op":"list"}))["data"]["error"],
        "catalog_incomplete");
    assert!(route::claim_route_action().is_none());
}

#[test]
fn media_tracks_readable_restricted_context_projects_no_mutations() {
    let _lock = plx_base::testlock::serial();
    let _globals = Globals::capture();
    for restriction in 0..4 {
        let mut app = fixture();
        match restriction {
            0 => app.running = false,
            1 => app.ott_standby = true,
            2 => { app.ott.as_mut().unwrap().saved.settings.insert("nativeKiosk".into(), locked_track_policy()); }
            _ => {
                bridge::show_page(&mut app.pages, AppArg::Ott);
                settle_pages(&mut app);
                assert!(matches!(app.route(), AppArg::Ott));
            }
        }
        assert_eq!(track_capability(&mut app, 1), json!({"version":1}));
        let listed = adapter_query(&mut app, 2, "media_tracks", json!({"op":"list"}));
        assert_eq!(listed["status"], "ok");
        assert_eq!(listed["data"]["tracks"].as_array().unwrap().len(), 4);
        assert!(listed["data"]["tracks"].as_array().unwrap().iter().all(|row| row["selectable"] == false));
        assert_eq!(listed["data"]["subtitle_off_supported"], false);
        assert!(route::claim_route_action().is_none());
    }
}

#[test]
fn media_tracks_mutability_round_trip_does_not_revive_old_catalog() {
    let _lock = plx_base::testlock::serial();
    let _globals = Globals::capture();
    let mut app = fixture();
    let before = adapter_query(&mut app, 1, "media_tracks", json!({"op":"list"}));
    let playback = route::playback_identity(&app.player.session);
    let worker = route::worker_ticket();
    app.ott_standby = true;
    let restricted = adapter_query(&mut app, 2, "media_tracks", json!({"op":"list"}));
    app.ott_standby = false;
    let after = adapter_query(&mut app, 3, "media_tracks", json!({"op":"list"}));
    assert_eq!(before["data"]["playback"], after["data"]["playback"]);
    for older in [&before, &restricted] {
        assert_ne!(older["data"]["source_generation"], after["data"]["source_generation"]);
        assert_ne!(older["data"]["catalog"], after["data"]["catalog"]);
    }
    let stale = adapter_query(&mut app, 4, "media_tracks", select(&before, "subtitle", Some(0)));
    assert_eq!(stale["data"]["error"], "playback_changed");
    assert_eq!(player::desired_sub_idx(), -1);
    assert_eq!(route::playback_identity(&app.player.session), playback);
    assert_eq!(route::worker_ticket(), worker);
    let valid = adapter_query(&mut app, 5, "media_tracks", select(&after, "subtitle", Some(0)));
    assert_eq!(valid["data"]["outcome"], "requested");
    assert_eq!(valid["data"]["observed"], Value::Null);
}

#[test]
fn media_tracks_pending_audio_does_not_change_passive_mutability_snapshot() {
    let _lock = plx_base::testlock::serial();
    let _globals = Globals::capture();
    let mut app = fixture();
    let pending = queue_changed_audio(&mut app);
    assert_eq!(track_capability(&mut app, 1), json!({"version":1}));
    let listed = adapter_query(&mut app, 2, "media_tracks", json!({"op":"list"}));
    assert!(listed["data"]["tracks"].as_array().unwrap().iter().all(|row| row["selectable"] == true));
    assert_eq!(listed["data"]["subtitle_off_supported"], true);
    let busy = adapter_query(&mut app, 3, "media_tracks", select(&listed, "subtitle", None));
    assert_eq!(busy["data"]["error"], "busy");
    assert!(commands::track_request_pending(app.ott.as_ref().unwrap()));
    assert!(audio_response(&app, &pending).is_none());
    poll_audio(&mut app, Some(pending.deadline_ms));
    assert_eq!(audio_response(&app, &pending).unwrap()["data"]["error"], "unavailable");
    assert_eq!(player::desired_sub_idx(), -1);
    assert!(route::claim_route_action().is_none());
}

#[test]
fn media_tracks_early_expiry_and_policy_errors_echo_retained_raw_bindings() {
    let _lock = plx_base::testlock::serial();
    let _globals = Globals::capture();
    for outer in [false, true] {
        let mut app = fixture();
        let listed = list(&mut app);
        let mut invocation = request(&mut app, select(&listed, "subtitle", Some(0)));
        // The raw validated projection, not this mutable Value, owns the echo.
        invocation.params["catalog"] = json!("untrusted-lossy-catalog");
        invocation.params["target"]["id"] = json!("untrusted-lossy-id");
        app.ott.as_mut().unwrap().saved.settings.insert("nativeKiosk".into(), locked_track_policy());
        let response = if outer {
            // Exact outer Runtime policy branch functions; this is not a
            // transport-flight injection into the whole Runtime tick.
            let runtime = app.ott.as_mut().unwrap();
            let reason = super::super::input_policy::remote(runtime, &invocation).unwrap_err();
            let response = commands::policy_rejection(&invocation, reason);
            runtime.remote.complete(invocation.clone(), response.clone());
            let cached = runtime.remote.response_for_id_for_test(&invocation.id).unwrap();
            assert_eq!(cached["id"], invocation.id);
            assert_eq!(cached["status"], response["status"]);
            assert_eq!(cached["data"], response["data"]);
            response
        } else {
            dispatch(&mut app, &invocation)
        };
        assert_track_error(&response, &invocation, "restricted");
        assert_eq!(player::desired_sub_idx(), -1);
        assert!(route::claim_route_action().is_none());
    }
    let mut app = fixture();
    let listed = list(&mut app);
    let mut invocation = request(&mut app, select(&listed, "subtitle", Some(0)));
    invocation.params["catalog"] = json!("untrusted-lossy-catalog");
    invocation.deadline_ms = app.ott.as_ref().unwrap().now_ms();
    assert_track_error(&dispatch(&mut app, &invocation), &invocation, "unavailable");
    assert_eq!(player::desired_sub_idx(), -1);
    assert!(route::claim_route_action().is_none());
}

#[test]
fn media_tracks_early_error_does_not_repair_missing_raw_projection() {
    let _lock = plx_base::testlock::serial();
    let _globals = Globals::capture();
    let mut app = fixture();
    let runtime = app.ott.as_mut().unwrap();
    let runtime_id = runtime.remote.runtime().to_owned();
    let mut invocation = runtime.remote.request_for_test("media_tracks", json!({"op":"list","runtime":runtime_id}));
    assert!(invocation.media_tracks_projection().is_none());
    invocation.deadline_ms = runtime.now_ms();
    let response = dispatch(&mut app, &invocation);
    assert_eq!(response["status"], "rejected");
    assert!(response["data"].get("version").is_none());
    assert!(response["data"].get("op").is_none());
    assert!(route::claim_route_action().is_none());
}


// Ordinary Plex fixtures use the same raw ingress and command dispatcher as
// external playback above. Only the installed route/metadata are synthetic.
fn plex_track_fixture() -> App {
    let mut app = app_fixture();
    app.running = true;
    let runtime = app.ott.as_mut().unwrap();
    runtime.loaded = true;
    runtime.entropy = Some([9; 16]);
    runtime.write_permit.revoke();
    let sid = plx_plex::plex::register_for_test(
        "plex-track-fixture", "127.0.0.1", 1, "synthetic-token", "track-test-client",
    );
    let mut item = plx_data::metadata::PlayingItem::with_subs(vec![
        plx_data::metadata::Stream {
            id: 203, index: 3, codec: "srt".into(), lang_code: "eng".into(),
            ..Default::default()
        },
    ]);
    item.sid = sid;
    item.rk = "track-fixture-item".into();
    item.part_id = 77;
    // Document order deliberately differs from container order.
    item.audio = vec![
        plx_data::metadata::Stream {
            id: 102, index: 2, codec: "aac".into(), channels: 2, sampling_rate: 48000,
            lang_code: "spa".into(), ..Default::default()
        },
        plx_data::metadata::Stream {
            id: 101, index: 1, codec: "aac".into(), channels: 2, sampling_rate: 48000,
            lang_code: "eng".into(), selected: true, ..Default::default()
        },
    ];
    route::install_plex_track_fixture_for_test(&mut app.player.session, sid, &item.rk, 77,
        route::CarriedAudio::from_stream(&item.audio[1], 0));
    assert!(route::qualify_plex_direct_retry_fixture_for_test(&mut app.player.session,
        plx_plex::plex::client_for(sid).unwrap()));
    assert!(app.bridge.metadata_run(plx_data::stores::metadata::MetadataCmd::InstallPlaying(Some(item))));
    route::reset_player_control_for_test(&app.player.session);
    app.adapters.player.install(player::engine::engine_after_reload_for_test());
    assert!(player::pause(&mut app.adapters.player));
    player::swap_state_for_test(player::PlaybackState::Paused);
    player::SHARED.playpos_ns.store(7_000_000_000, Ordering::Release);
    mount_player(&mut app);
    app
}

struct PlexRegistryReset;
impl Drop for PlexRegistryReset {
    fn drop(&mut self) { plx_plex::plex::reset_servers_for_test(); }
}

#[test]
fn media_tracks_plex_complete_catalog_uses_opaque_ids_from_installed_leaf() {
    let _lock = plx_base::testlock::serial();
    let _globals = Globals::capture();
    let _registry = PlexRegistryReset;
    let mut app = plex_track_fixture();
    let before = serde_json::to_value(&app.ott.as_ref().unwrap().saved).unwrap();
    let listed = list(&mut app);
    assert_eq!(listed["status"], "ok", "{listed}");
    let tracks = listed["data"]["tracks"].as_array().expect("complete Plex catalog");
    assert_eq!(tracks.len(), 3);
    assert!(tracks.iter().all(|row| row["selectable"] == true));
    assert_eq!(listed["data"]["subtitle_off_supported"], true);
    for row in tracks {
        let id = row["id"].as_str().expect("opaque track id");
        assert!(!["101", "102", "203"].contains(&id));
        assert!(!id.contains("plex-track"));
    }
    let wire = listed.to_string();
    assert!(!wire.contains("synthetic-token") && !wire.contains("/library/parts/"));
    assert_eq!(serde_json::to_value(&app.ott.as_ref().unwrap().saved).unwrap(), before);
    assert!(route::claim_route_action().is_none(), "listing must not dispatch playback");
}

#[test]
fn media_tracks_plex_capability_is_passive_and_does_not_mint_a_catalog() {
    let _lock = plx_base::testlock::serial();
    let _globals = Globals::capture();
    let _registry = PlexRegistryReset;
    let mut app = plex_track_fixture();
    let before = app.ott.as_ref().unwrap().uuid_counter;
    let ticket = route::worker_ticket();
    assert_eq!(track_capability(&mut app, 81), json!({"version":1}));
    assert_eq!(app.ott.as_ref().unwrap().uuid_counter, before);
    assert_eq!(route::worker_ticket(), ticket);
    assert!(route::claim_route_action().is_none());
    assert!(!app.ott.as_ref().unwrap().saving);
}

#[test]
fn media_tracks_plex_missing_or_other_leaf_cannot_authorize_selection() {
    let _lock = plx_base::testlock::serial();
    let _globals = Globals::capture();
    let _registry = PlexRegistryReset;
    let mut app = plex_track_fixture();
    assert!(app.bridge.metadata_run(plx_data::stores::metadata::MetadataCmd::InstallPlaying(None)));
    assert_ne!(list(&mut app)["status"], "ok");
    assert_eq!(track_capability(&mut app, 82), Value::Null);
    let mut other = plx_data::metadata::PlayingItem::with_subs(vec![]);
    other.sid = route::cur_sid(&app.player.session);
    other.rk = "different-leaf".into();
    assert!(app.bridge.metadata_run(plx_data::stores::metadata::MetadataCmd::InstallPlaying(Some(other))));
    assert_ne!(list(&mut app)["status"], "ok");
    assert_eq!(track_capability(&mut app, 83), Value::Null);
    assert!(route::claim_route_action().is_none());
}

#[path = "ott_media_tracks_plex_runtime_tests.rs"]
mod plex_adapter;

#[path = "ott_plex_direct_retry_runtime_tests.rs"]
mod direct_retry;
