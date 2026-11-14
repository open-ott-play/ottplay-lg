//! Raw Remote ingress and the real shared completion path. Worker occupancy and
//! prepared media are modeled explicitly; no network, decoder, or writer runs.
use super::*;
use crate::app::{bridge, playback, run};
use crate::app::ott_remote::{Invocation, Remote as TransportRemote};
use plx_media::{ff::hls_presentation::HlsPlayback, player};
use plx_screens::registry::AppArg;
use serde_json::{json, Value};

struct ExtraGlobals<G> {
    _route: G,
    duration: i64,
    ended: bool,
    pipeline_eos: bool,
    seeking: bool,
    seek_display: i64,
    seek_to: i64,
    seek_reqs: u32,
}
impl<G> Drop for ExtraGlobals<G> {
    fn drop(&mut self) {
        player::SHARED.duration_ns.store(self.duration, Ordering::Relaxed);
        player::SHARED.ended.store(self.ended, Ordering::Relaxed);
        player::SHARED.pipeline_eos.store(self.pipeline_eos, Ordering::Relaxed);
        player::SHARED.seeking.store(self.seeking, Ordering::Relaxed);
        player::SHARED.seek_display_ns.store(self.seek_display, Ordering::Relaxed);
        player::TX.seek_to_ns.store(self.seek_to, Ordering::Relaxed);
        player::TX.seek_reqs.store(self.seek_reqs, Ordering::Relaxed);
    }
}
fn globals() -> impl Drop {
    ExtraGlobals {
        _route: route::HlsTrackRequest::test_globals_guard(),
        duration: player::SHARED.duration_ns.load(Ordering::Relaxed),
        ended: player::SHARED.ended.load(Ordering::Relaxed),
        pipeline_eos: player::SHARED.pipeline_eos.load(Ordering::Relaxed),
        seeking: player::SHARED.seeking.load(Ordering::Relaxed),
        seek_display: player::SHARED.seek_display_ns.load(Ordering::Relaxed),
        seek_to: player::TX.seek_to_ns.load(Ordering::Relaxed),
        seek_reqs: player::TX.seek_reqs.load(Ordering::Relaxed),
    }
}

fn fixture() -> App {
    let mut app = run::lifecycle_regression_tests::app();
    let mut runtime = Runtime::unloaded(crate::app::ott_companion::Supervisor::inert_for_test());
    runtime.remote = TransportRemote::disabled_for_test("dev_fixture", "hls-remote");
    let sample = plx_base::task::time::ClockSample::capture_main(app.adapters.player.mt()).unwrap();
    runtime.clock_anchor = Some(sample);
    runtime.clock_sample = Some(sample);
    runtime.loaded = true;
    runtime.entropy = Some([21; 16]);
    runtime.write_permit.revoke();
    app.ott = Some(Box::new(runtime));
    app.running = true;
    install(&mut app, &HlsPlayback::fixture_for_test());
    bridge::show_page(&mut app.pages, AppArg::Ott);
    settle(&mut app);
    playback::enter_player(&mut app.pages, &mut app.bridge, playback::Origin::Here, None);
    settle(&mut app);
    app
}
fn settle(app: &mut App) {
    for _ in 0..24 {
        app.loop_t = app.loop_t.wrapping_add(16);
        let tick = plx_machine::machine::Tick { ms: app.loop_t, dt_us: 16_000 };
        bridge::frame(&mut app.pages, &mut app.bridge, tick, Vec::new());
    }
}
fn install(app: &mut App, media: &HlsPlayback) {
    playback::replace_player(&mut app.player, &mut app.adapters.player, &mut app.repause_at,
        &mut app.refresh_hubs, &mut app.pages);
    route::reset_player_control_for_test(&app.player.session);
    route::request_external_source(&mut app.player.session, &media.media_info_for_test(),
        plx_media::ff::ExternalSource::Hls("https://example.invalid/master.m3u8".into()),
        "Fixture", "M3U", plx_platform::devcaps::assumed_shared()).unwrap();
    route::reset_player_control_for_test(&app.player.session);
    app.adapters.player.install(player::engine::engine_after_reload_for_test());
    assert!(player::pause(&mut app.adapters.player));
    player::swap_state_for_test(player::PlaybackState::Paused);
    player::SHARED.playpos_ns.store(7_000_000_000, Ordering::Release);
    player::SHARED.duration_ns.store(60_000_000_000, Ordering::Release);
    player::TX.seek_to_ns.store(-1, Ordering::Relaxed);
    player::SHARED.seeking.store(false, Ordering::Relaxed);
    player::SHARED.seek_display_ns.store(-1, Ordering::Relaxed);
}
fn raw(app: &mut App, action: &str, mut params: Value) -> Invocation {
    static IDS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    let rt = app.ott.as_mut().unwrap();
    if action == "media_tracks" { params["runtime"] = json!(rt.remote.runtime()); }
    let id = format!("{:032x}", IDS.fetch_add(1, Ordering::Relaxed));
    let body = json!({"commands":[],"server_time":1000,"requests":[{
        "id":id,"expires_at":1020,"action":action,"params":params}]}).to_string();
    let now = rt.now_ms();
    let mut inv = rt.remote.raw_poll_for_test(&body, now).pop().expect("real raw request admission");
    inv.deadline_ms = now + 10_000;
    inv
}
fn dispatch(app: &mut App, inv: &Invocation) -> Option<Value> {
    let mut rt = app.ott.take().unwrap();
    let result = super::super::commands::command(&mut rt, app, inv);
    if let Some(value) = &result { rt.remote.complete(inv.clone(), value.clone()); }
    app.ott = Some(rt);
    result
}
fn list(app: &mut App) -> Value {
    let inv = raw(app, "media_tracks", json!({"op":"list"}));
    dispatch(app, &inv).unwrap()
}
fn target(list: &Value, index: usize) -> Value {
    json!({"op":"select","playback":list["data"]["playback"],
        "source_generation":list["data"]["source_generation"],"catalog":list["data"]["catalog"],
        "target":{"kind":"audio","mode":"track","id":list["data"]["tracks"][index]["id"]}})
}
fn queue(app: &mut App, index: usize) -> Invocation {
    let listed = list(app);
    let inv = raw(app, "media_tracks", target(&listed, index));
    assert!(dispatch(app, &inv).is_none());
    assert!(has_remote_pending(app.ott.as_ref().unwrap()));
    assert!(answer(app, &inv).is_none());
    inv
}
fn answer(app: &App, inv: &Invocation) -> Option<Value> {
    app.ott.as_ref().unwrap().remote.response_for_id_for_test(&inv.id)
}
fn start(app: &mut App) -> (u64, Arc<AtomicBool>, HlsTrackRequest) {
    let slots = &mut app.ott.as_mut().unwrap().hls_tracks.slots;
    let (id, cancel) = slots.start().unwrap();
    let request = slots.running.as_ref().unwrap().value.as_ref().unwrap().route.clone();
    (id, cancel, request)
}
fn complete(app: &mut App, id: u64, result: Result<PreparedHlsTrack, &'static str>, now: Option<u64>) {
    let mut rt = app.ott.take().unwrap();
    completed_at(&mut rt, app, Completion { id, result }, now);
    app.ott = Some(rt);
}
fn now(app: &App) -> u64 { app.ott.as_ref().unwrap().now_ms() }
fn poll(app: &mut App, at: Option<u64>) {
    let mut rt = app.ott.take().unwrap();
    super::super::commands::before_playback_tick(&mut rt, app, at);
    app.ott = Some(rt);
}
fn selected(app: &App, index: usize) -> bool {
    route::hls_track_catalog(&app.player.session).unwrap().rows()[index].selected
}

#[test]
fn raw_hls_catalog_uses_complete_declared_group_without_preparation_or_fake_codec() {
    let _lock = plx_base::testlock::serial();
    let _globals = globals();
    let mut app = fixture();
    let worker = route::worker_ticket();
    let intent = route::track_intent_ticket();
    let inv = raw(&mut app, "capabilities", json!({}));
    let capabilities = dispatch(&mut app, &inv).unwrap();
    assert_eq!(capabilities["data"]["media_tracks"], json!({"version":1}));
    let listed = list(&mut app);
    let rows = listed["data"]["tracks"].as_array().unwrap();
    assert_eq!(rows.len(), route::hls_track_catalog(&app.player.session).unwrap().rows().len());
    assert_eq!(rows.len(), 2, "existing synthetic master fixture has two real declared rows");
    assert!(rows.iter().all(|r| r["kind"] == "audio" && r["codec"].is_null() && r["selectable"] == true));
    assert_ne!(rows[0]["id"], rows[1]["id"]);
    assert_eq!(listed["data"]["subtitle_off_supported"], false);
    assert_eq!(worker, route::worker_ticket());
    assert_eq!(intent, route::track_intent_ticket());
    assert!(!has_remote_pending(app.ott.as_ref().unwrap()));
    assert!(app.ott.as_ref().unwrap().hls_tracks.slots.running.is_none());
    let mut off = target(&listed, 0);
    off["target"] = json!({"kind":"subtitle","mode":"off"});
    let inv = raw(&mut app, "media_tracks", off);
    assert_eq!(dispatch(&mut app, &inv).unwrap()["data"]["error"], "not_selectable");
}

#[test]
fn raw_hls_subtitle_choice_and_off_share_the_addressed_worker_without_av_reload() {
    let _lock = plx_base::testlock::serial();
    let _globals = globals();
    let mut app = Box::new(fixture());
    install(&mut app, &HlsPlayback::fixture_with_subtitles_for_test());
    let listed = list(&mut app);
    assert_eq!(listed["data"]["subtitle_off_supported"], true);
    let rows = listed["data"]["tracks"].as_array().unwrap();
    let subtitles: Vec<_> = rows.iter().filter(|row| row["kind"] == "subtitle").collect();
    assert_eq!(subtitles.len(), 2);
    assert!(subtitles.iter().all(|row| row["selectable"] == true && row["codec"].is_null()));
    let mut params = target(&listed, 0);
    params["target"] = json!({"kind":"subtitle","mode":"track","id":subtitles[0]["id"]});
    let pick = raw(&mut app, "media_tracks", params);
    assert!(dispatch(&mut app, &pick).is_none());
    let (first, _, request) = start(&mut app);
    let at = now(&app);
    complete(&mut app, first, Ok(request.prepared_for_test()), Some(at));
    let accepted = route::hls_track_catalog(&app.player.session).unwrap();
    assert_eq!(accepted.selected_subtitle(), Some(accepted.subtitle_rows()[0].id));
    assert_eq!(answer(&app, &pick).unwrap()["data"]["outcome"], "requested");
    let worker = route::worker_ticket();
    let position = player::SHARED.playpos_ns.load(Ordering::Acquire);
    let audio = accepted.rows();
    let switching = player::audio_switching();
    let listed = list(&mut app);
    let next_sub = listed["data"]["tracks"].as_array().unwrap().iter()
        .filter(|row| row["kind"] == "subtitle").nth(1).unwrap();
    let mut params = target(&listed, 0);
    params["target"] = json!({"kind":"subtitle","mode":"track","id":next_sub["id"]});
    let pending = raw(&mut app, "media_tracks", params);
    assert!(dispatch(&mut app, &pending).is_none());
    let (old, cancelled, old_request) = start(&mut app);
    let listed = list(&mut app);
    let mut params = target(&listed, 0);
    params["target"] = json!({"kind":"subtitle","mode":"off"});
    let off = raw(&mut app, "media_tracks", params);
    assert!(dispatch(&mut app, &off).is_none());
    assert!(cancelled.load(Ordering::Acquire));
    assert_eq!(answer(&app, &pending).unwrap()["data"]["error"], "catalog_changed");
    assert!(app.ott.as_mut().unwrap().hls_tracks.slots.start().is_none());
    let at = now(&app);
    complete(&mut app, old, Ok(old_request.prepared_for_test()), Some(at));
    assert_eq!(route::hls_track_catalog(&app.player.session).unwrap().selected_subtitle(), accepted.selected_subtitle());
    let (latest, _, request) = start(&mut app);
    complete(&mut app, latest, Ok(request.prepared_for_test()), Some(at));
    assert_eq!(answer(&app, &off).unwrap()["data"]["outcome"], "requested");
    let after = route::hls_track_catalog(&app.player.session).unwrap();
    assert_eq!(after.selected_subtitle(), None);
    assert_eq!(after.rows(), audio);
    assert_eq!(route::worker_ticket(), worker);
    assert_eq!(player::SHARED.playpos_ns.load(Ordering::Acquire), position);
    assert_eq!(player::state(&app.player.session), player::PlaybackState::Paused);
    assert_eq!(player::audio_switching(), switching);
}

#[test]
fn raw_remote_hls_waits_for_guarded_publication_and_reports_requested_not_observed() {
    let _lock = plx_base::testlock::serial();
    let _globals = globals();
    let mut app = fixture();
    let saved = serde_json::to_value(&app.ott.as_ref().unwrap().saved).unwrap();
    let inv = queue(&mut app, 1);
    let (id, _, request) = start(&mut app);
    assert!(selected(&app, 0));
    let at = now(&app);
    complete(&mut app, id, Ok(request.prepared_for_test()), Some(at));
    let response = answer(&app, &inv).unwrap();
    assert_eq!(response["data"]["outcome"], "requested");
    assert_eq!(response["data"]["observed"], Value::Null);
    assert!(selected(&app, 1));
    assert!(!has_remote_pending(app.ott.as_ref().unwrap()));
    assert_eq!(serde_json::to_value(&app.ott.as_ref().unwrap().saved).unwrap(), saved);
    complete(&mut app, id, Err("late duplicate"), Some(at));
    assert_eq!(answer(&app, &inv), Some(response));
}

#[test]
fn remote_supersession_keeps_one_physical_slot_and_one_terminal_per_original_request() {
    let _lock = plx_base::testlock::serial();
    let _globals = globals();
    let mut app = fixture();
    let first = queue(&mut app, 1);
    let (old_id, cancelled, old_request) = start(&mut app);
    let second = queue(&mut app, 0);
    let retired = answer(&app, &first).unwrap();
    assert_eq!(retired["data"]["error"], "catalog_changed");
    assert!(cancelled.load(Ordering::Acquire));
    assert!(app.ott.as_mut().unwrap().hls_tracks.slots.start().is_none());
    assert!(answer(&app, &second).is_none());
    let at = now(&app);
    complete(&mut app, old_id, Ok(old_request.prepared_for_test()), Some(at));
    assert!(selected(&app, 0));
    assert_eq!(answer(&app, &first), Some(retired));
    let (new_id, _, new_request) = start(&mut app);
    complete(&mut app, new_id, Ok(new_request.prepared_for_test()), Some(at));
    assert_eq!(answer(&app, &second).unwrap()["data"]["outcome"], "requested");
}

#[test]
fn queued_and_running_expiry_keep_original_deadline_despite_fresh_list() {
    let _lock = plx_base::testlock::serial();
    let _globals = globals();
    for running in [false, true] {
        let mut app = fixture();
        let inv = queue(&mut app, 1);
        let active = running.then(|| start(&mut app));
        let _renewed = list(&mut app); // same Remote grant, a fresh raw request ID
        poll(&mut app, Some(inv.deadline_ms));
        assert_eq!(answer(&app, &inv).unwrap()["data"]["error"], "unavailable");
        assert!(selected(&app, 0));
        assert!(!has_remote_pending(app.ott.as_ref().unwrap()));
        assert!(!super::super::commands::track_preparation_pending(app.ott.as_ref().unwrap()),
            "retired physical worker no longer fences kiosk inputs");
        if let Some((id, cancel, request)) = active {
            assert!(cancel.load(Ordering::Acquire));
            assert!(app.ott.as_ref().unwrap().hls_tracks.slots.running.is_some());
            complete(&mut app, id, Ok(request.prepared_for_test()), Some(inv.deadline_ms));
            assert!(selected(&app, 0));
        }
    }
}

#[test]
fn successful_preparation_at_deadline_or_without_clock_cannot_publish() {
    let _lock = plx_base::testlock::serial();
    let _globals = globals();
    for clock_available in [true, false] {
        let mut app = fixture();
        let inv = queue(&mut app, 1);
        let (id, _, request) = start(&mut app);
        complete(&mut app, id, Ok(request.prepared_for_test()), clock_available.then_some(inv.deadline_ms));
        assert_eq!(answer(&app, &inv).unwrap()["data"]["error"], "unavailable");
        assert!(selected(&app, 0));
        assert!(app.ott.as_ref().unwrap().hls_tracks.slots.running.is_none());
    }
}

#[test]
fn late_preparation_after_standby_or_closed_app_preserves_current_audio() {
    let _lock = plx_base::testlock::serial();
    let _globals = globals();
    for standby in [false, true] {
        let mut app = fixture();
        let inv = queue(&mut app, 1);
        let (id, _, request) = start(&mut app);
        if standby { app.ott_standby = true; } else { app.running = false; }
        let at = now(&app);
        complete(&mut app, id, Ok(request.prepared_for_test()), Some(at));
        assert_eq!(answer(&app, &inv).unwrap()["data"]["error"], "restricted");
        assert!(selected(&app, 0));
    }
}

#[test]
fn new_same_url_source_or_new_intent_rejects_prepared_old_rendition() {
    let _lock = plx_base::testlock::serial();
    let _globals = globals();
    for replace_source in [false, true] {
        let mut app = fixture();
        let inv = queue(&mut app, 1);
        let (id, _, request) = start(&mut app);
        if replace_source {
            install(&mut app, &HlsPlayback::fixture_for_test());
        } else {
            let catalog = route::hls_track_catalog(&app.player.session).unwrap();
            let choice = catalog.choice(catalog.rows()[0].id).unwrap();
            route::begin_hls_track_request(&mut app.player.session, &choice).unwrap();
        }
        let worker = route::worker_ticket();
        let at = now(&app);
        complete(&mut app, id, Ok(request.prepared_for_test()), Some(at));
        assert_eq!(answer(&app, &inv).unwrap()["data"]["error"],
            if replace_source { "playback_changed" } else { "catalog_changed" });
        assert!(selected(&app, 0));
        assert_eq!(route::worker_ticket(), worker);
    }
}

#[test]
fn revoked_remote_grant_drains_without_reply_into_the_new_generation() {
    let _lock = plx_base::testlock::serial();
    let _globals = globals();
    let mut app = fixture();
    let inv = queue(&mut app, 1);
    let (id, _, request) = start(&mut app);
    app.ott.as_mut().unwrap().remote.disable().unwrap();
    assert!(!app.ott.as_ref().unwrap().remote.invocation_is_current(&inv));
    let at = now(&app);
    complete(&mut app, id, Ok(request.prepared_for_test()), Some(at));
    assert!(answer(&app, &inv).is_none());
    assert!(selected(&app, 0));
    assert!(!has_remote_pending(app.ott.as_ref().unwrap()));
}

#[test]
fn failed_prepare_preserves_selection_pause_position_and_saved_preferences() {
    let _lock = plx_base::testlock::serial();
    let _globals = globals();
    let mut app = fixture();
    let saved = serde_json::to_value(&app.ott.as_ref().unwrap().saved).unwrap();
    let inv = queue(&mut app, 1);
    let (id, _, _) = start(&mut app);
    let at = now(&app);
    complete(&mut app, id, Err("private URL or credential must never enter reply"), Some(at));
    let response = answer(&app, &inv).unwrap();
    assert_eq!(response["data"]["error"], "unavailable");
    assert!(!response.to_string().contains("private URL"));
    assert!(selected(&app, 0));
    assert_eq!(player::state(&app.player.session), player::PlaybackState::Paused);
    assert_eq!(player::SHARED.playpos_ns.load(Ordering::Acquire), 7_000_000_000);
    assert_eq!(serde_json::to_value(&app.ott.as_ref().unwrap().saved).unwrap(), saved);
}

fn menu_choice(app: &mut App) -> TrackCommitRequest {
    use plx_machine::machine::{Delivery, Edge, Fx, Key, MachineId, Tick};
    use plx_ui::screen::ScreenEvent;
    app.bridge.publish_playback(&app.player.session, true);
    bridge::open_player_overlay(&app.player.session,
        plx_data::stores::metadata::MetadataStore::default().view(), &mut app.pages,
        plx_screens::player::overlay::OverlayKind::Tracks { tab: 0 });
    settle(app);
    let instance = app.pages.nav.modals.surfaces.last().unwrap().entry.inst.as_ref().unwrap().id;
    for key in [Key::Down, Key::Ok] {
        app.loop_t = app.loop_t.wrapping_add(16);
        let at = Tick { ms: app.loop_t, dt_us: 16_000 };
        app.pages.emit(MachineId::Nav, Fx::Deliver(MachineId::Instance(instance),
            Delivery::Screen(ScreenEvent::Input(bridge::script_edge(key, Edge::Down, at)))));
        bridge::frame(&mut app.pages, &mut app.bridge, at, vec![]);
    }
    app.bridge.take_player_reqs().into_iter().find_map(|req| match req {
        PlayerReq::CommitTrackAddressed(request) => Some(request), _ => None,
    }).expect("real menu choice")
}

#[test]
fn addressed_ui_and_remote_mutually_supersede_without_freeing_the_old_worker() {
    let _lock = plx_base::testlock::serial();
    let _globals = globals();
    let mut app = fixture();
    let remote = queue(&mut app, 1);
    let (old_id, cancelled, old_request) = start(&mut app);
    let ui = menu_choice(&mut app);
    assert!(player_request(&mut app, PlayerReq::CommitTrackAddressed(ui.clone())).is_none());
    assert_eq!(answer(&app, &remote).unwrap()["data"]["error"], "catalog_changed");
    assert!(owner_pending(&app.pages, Some(&ui)));
    assert!(super::super::commands::track_preparation_pending(app.ott.as_ref().unwrap()));
    assert!(cancelled.load(Ordering::Acquire));
    assert!(app.ott.as_mut().unwrap().hls_tracks.slots.start().is_none());
    let latest = queue(&mut app, 0);
    assert!(!owner_pending(&app.pages, Some(&ui)), "exact UI registration consumed on replacement");
    assert!(answer(&app, &latest).is_none());
    let at = now(&app);
    complete(&mut app, old_id, Ok(old_request.prepared_for_test()), Some(at));
    assert!(selected(&app, 0));
    let (id, _, request) = start(&mut app);
    complete(&mut app, id, Ok(request.prepared_for_test()), Some(at));
    assert_eq!(answer(&app, &latest).unwrap()["data"]["outcome"], "requested");
    assert!(!owner_pending(&app.pages, Some(&ui)));
}

#[test]
fn accepted_stored_kiosk_policy_change_rejects_an_already_prepared_remote_selection() {
    let _lock = plx_base::testlock::serial();
    let _globals = globals();
    let mut app = fixture();
    let inv = queue(&mut app, 1);
    let (id, _, request) = start(&mut app);
    let policy = crate::app::ott_kiosk_stored::Stored::local_folder(
        plx_data::ott::favorites::Origin { provider: "local".into(), profile: "Fixture".into(),
            subprofile: None, source_alias: "Fixture".into() },
        true, "fixture_root".into(), "a".repeat(64),
    ).unwrap().encode().unwrap();
    app.ott.as_mut().unwrap().saved.settings.insert("nativeKiosk".into(), policy);
    let at = now(&app);
    complete(&mut app, id, Ok(request.prepared_for_test()), Some(at));
    assert_eq!(answer(&app, &inv).unwrap()["data"]["error"], "restricted");
    assert!(selected(&app, 0));
}

#[test]
fn strict_kiosk_keeps_saved_automatic_subtitles_and_auto_off_authority() {
    let _lock = plx_base::testlock::serial();
    let _globals = globals();
    for armed in [false, true] {
        let mut app = fixture();
        install(&mut app, &HlsPlayback::fixture_with_subtitles_for_test());
        let origin = plx_data::ott::favorites::Origin { provider: "local".into(), profile: "Fixture".into(),
            subprofile: None, source_alias: "Fixture".into() };
        let kiosk = if armed {
            crate::app::ott_kiosk_stored::Stored::armed(origin, true).unwrap()
        } else {
            crate::app::ott_kiosk_stored::Stored::local_folder(origin, true,
                "fixture_root".into(), "a".repeat(64)).unwrap()
        };
        app.ott.as_mut().unwrap().saved.settings.insert("nativeKiosk".into(), kiosk.encode().unwrap());
        assert!(gate(app.ott.as_ref().unwrap(), &app).is_err(), "manual controls remain restricted");
        assert!(automatic_gate(app.ott.as_ref().unwrap(), &app).is_ok());
        for mode in [route::HlsSubtitleMode::Auto, route::HlsSubtitleMode::Off] {
            route::restore_hls_subtitle_policy(&mut app.player.session, route::HlsSubtitlePolicy {
                mode, language: "en".into(),
            });
            let mut rt = app.ott.take().unwrap();
            enqueue_automatic(&mut rt.hls_tracks, &mut app.player.session);
            app.ott = Some(rt);
            let (id, _, request) = start(&mut app);
            let at = now(&app);
            complete(&mut app, id, Ok(request.prepared_for_test()), Some(at));
            assert_eq!(route::hls_track_catalog(&app.player.session).unwrap().selected_subtitle().is_some(),
                mode == route::HlsSubtitleMode::Auto);
        }
    }
}

#[test]
fn production_completion_resamples_time_instead_of_using_the_old_frame_sample() {
    let _lock = plx_base::testlock::serial();
    let _globals = globals();
    for missing_anchor in [false, true] {
        let mut app = fixture();
        let inv = queue(&mut app, 1);
        let (id, _, request) = start(&mut app);
        let mut rt = app.ott.take().unwrap();
        assert!(rt.now_ms() < inv.deadline_ms);
        // Existing ClockSample seam models a stale frame against a later local
        // time domain, without sleeping or altering a production clock source.
        rt.clock_anchor = if missing_anchor { None } else {
            rt.clock_anchor.map(|a| a.advance_for_test(std::time::Duration::from_secs(20)))
        };
        completed(&mut rt, &mut app, Completion { id, result: Ok(request.prepared_for_test()) });
        app.ott = Some(rt);
        assert_eq!(answer(&app, &inv).unwrap()["data"]["error"], "unavailable");
        assert!(selected(&app, 0));
    }
}

#[test]
fn forged_rendition_id_and_segment_only_hls_never_create_a_preparation() {
    let _lock = plx_base::testlock::serial();
    let _globals = globals();
    let mut app = fixture();
    let listed = list(&mut app);
    let mut params = target(&listed, 1);
    params["target"]["id"] = json!("not_a_catalog_rendition");
    let inv = raw(&mut app, "media_tracks", params);
    assert_eq!(dispatch(&mut app, &inv).unwrap()["data"]["error"], "track_not_found");
    assert!(!has_remote_pending(app.ott.as_ref().unwrap()));
    let mut info = HlsPlayback::fixture_for_test().media_info_for_test();
    info.hls_playback = None;
    playback::replace_player(&mut app.player, &mut app.adapters.player, &mut app.repause_at,
        &mut app.refresh_hubs, &mut app.pages);
    route::reset_player_control_for_test(&app.player.session);
    route::request_external_source(&mut app.player.session, &info,
        plx_media::ff::ExternalSource::Hls("https://example.invalid/media.m3u8".into()),
        "Fixture", "M3U", plx_platform::devcaps::assumed_shared()).unwrap();
    route::reset_player_control_for_test(&app.player.session);
    app.adapters.player.install(player::engine::engine_after_reload_for_test());
    player::swap_state_for_test(player::PlaybackState::Paused);
    let listed = list(&mut app);
    assert_eq!(listed["data"]["error"], "unsupported");
    let inv = raw(&mut app, "capabilities", json!({}));
    assert_eq!(dispatch(&mut app, &inv).unwrap()["data"]["media_tracks"], Value::Null);
    assert!(app.ott.as_ref().unwrap().hls_tracks.slots.running.is_none());
    assert!(!has_remote_pending(app.ott.as_ref().unwrap()));
}

#[test]
fn pending_hls_refuses_kiosk_mutation_but_keeps_status_and_original_request() {
    let _lock = plx_base::testlock::serial();
    let _globals = globals();
    for running in [false, true] {
        let mut app = fixture();
        let saved = serde_json::to_value(&app.ott.as_ref().unwrap().saved).unwrap();
        let audio = queue(&mut app, 1);
        let active = running.then(|| start(&mut app));
        let intent = route::track_intent_ticket();
        let status = raw(&mut app, "kiosk", json!({"mode":"status"}));
        assert_eq!(dispatch(&mut app, &status).unwrap()["status"], "ok");
        for params in [json!({"mode":"on"}), json!({"mode":"off"}), json!({"mode":"on","strict":true})] {
            let inv = raw(&mut app, "kiosk", params);
            assert_eq!(dispatch(&mut app, &inv).unwrap()["status"], "rejected");
            assert!(route::track_intent_is_current(intent));
            assert!(super::super::commands::track_preparation_pending(app.ott.as_ref().unwrap()));
            assert!(!super::super::commands::kiosk_pending(app.ott.as_ref().unwrap()));
            assert!(!app.ott.as_ref().unwrap().saving);
            assert_eq!(serde_json::to_value(&app.ott.as_ref().unwrap().saved).unwrap(), saved);
            assert!(answer(&app, &audio).is_none());
        }
        let (id, _, request) = active.unwrap_or_else(|| start(&mut app));
        let at = now(&app);
        complete(&mut app, id, Ok(request.prepared_for_test()), Some(at));
        assert_eq!(answer(&app, &audio).unwrap()["data"]["outcome"], "requested");
    }
}

fn provider_fixture() -> (super::super::ProviderProfile, super::super::CatalogEntry) {
    use super::super::{CatalogEntry, MediaLocator, ProviderKind, ProviderProfile};
    (ProviderProfile { id: "portal-fixture".into(), name: "Fixture".into(),
        kind: ProviderKind::VPortal, source: "portal::[key:fixture]https://media.invalid/nas/api".into(),
        source_alias: String::new(), enabled: true },
    CatalogEntry { protection: None, synopsis: None, id: "fixture".into(), title: "Fixture".into(),
        category: String::new(), description: String::new(), poster: String::new(),
        media: MediaLocator::VPortal { request: json!({"id":1}) }, duration_ms: Some(10_000),
        provider_id: Some("1".into()), provider_id_field: Some("id".into()), live: None })
}

#[test]
fn pending_hls_defers_unwritten_capture_but_suspension_still_cancels_it() {
    use super::super::{kiosk_capture, kiosk_source, PlaybackRequest};
    let _lock = plx_base::testlock::serial();
    let _globals = globals();
    for suspended in [false, true] {
        let mut app = fixture();
        let audio = queue(&mut app, 1);
        {
            let rt = app.ott.as_mut().unwrap();
            rt.resume_source_work();
            let (profile, entry) = provider_fixture();
            let armed = crate::app::ott_kiosk_stored::Stored::armed(
                plx_data::ott::favorites::Origin { provider: "vportal".into(), profile: profile.name.clone(),
                    subprofile: None, source_alias: profile.name.clone() }, false,
            ).unwrap();
            rt.provider = profile.kind;
            rt.saved.profiles.active_profile = Some(profile.id.clone());
            rt.saved.profiles.profiles = vec![profile];
            rt.play_target = Some(PlaybackRequest::ordinary(&entry.id, 0));
            rt.catalog.entries = vec![entry];
            rt.catalog_complete = true;
            rt.catalog_loading = false;
            rt.saved.settings.insert("nativeKiosk".into(), armed.encode().unwrap());
            kiosk_capture::explicit_admitted(rt);
            // A sentinel exposes whether the existing unwritten intent survives.
            rt.saved.settings.insert("nativeKiosk".into(), json!({"invalid":true}));
        }
        app.ott_standby = suspended;
        let mut rt = app.ott.take().unwrap();
        assert!(!kiosk_capture::tick(&mut rt, &app).unwrap());
        assert!(!rt.saving);
        app.ott = Some(rt);
        app.ott_standby = false;
        let at = now(&app);
        poll(&mut app, Some(at));
        assert_eq!(answer(&app, &audio).unwrap()["data"]["error"], "restricted");
        let mut rt = app.ott.take().unwrap();
        assert!(!super::super::commands::track_preparation_pending(&rt));
        if suspended {
            assert!(!kiosk_capture::tick(&mut rt, &app).unwrap(), "suspension cleared the unwritten intent");
        } else {
            assert!(matches!(kiosk_capture::tick(&mut rt, &app), Err(kiosk_source::SourceError::Invalid)),
                "ordinary pending wait preserved intent until validation");
        }
        assert!(!rt.saving);
        app.ott = Some(rt);
    }
}

#[test]
fn volume_preserves_hls_while_pending_seek_blocks_and_committed_seek_retires() {
    let _lock = plx_base::testlock::serial();
    let _globals = globals();
    for committed in [false, true] {
        let mut app = fixture();
        let audio = queue(&mut app, 1);
        let (id, _, request) = start(&mut app);
        let intent = route::track_intent_ticket();
        let volume = raw(&mut app, "command", json!({"command":"set_volume","volume":30}));
        let volume_response = dispatch(&mut app, &volume).unwrap();
        assert_eq!(volume_response["status"], "rejected");
        assert_eq!(volume_response["data"]["dispatched"], false);
        assert_eq!(volume_response["data"]["desired_volume"], 30);
        assert_eq!(app.player.volume.desired().level(), 30);
        // The inert Engine has epoch zero: no physical setter was confirmed.
        assert!(route::track_intent_is_current(intent));
        assert!(route::hls_track_request_is_current(&app.player.session, &request));
        assert!(answer(&app, &audio).is_none());
        let seek = raw(&mut app, "playback", json!({"operation":"seek","position":8}));
        assert_eq!(dispatch(&mut app, &seek).unwrap()["status"], "ok");
        if committed {
            // Model the existing native timeline commit boundary, not a decoder run.
            assert!(route::commit_user_seek());
        } else {
            assert!(route::hls_track_request_is_current(&app.player.session, &request),
                "a requested seek has not yet changed the physical worker epoch");
        }
        let at = now(&app);
        complete(&mut app, id, Ok(request.prepared_for_test()), Some(at));
        assert_eq!(answer(&app, &audio).unwrap()["data"]["error"],
            if committed { "catalog_changed" } else { "busy" });
        assert!(selected(&app, 0));
    }
}

#[test]
fn server_leased_hls_requires_playback_lease_not_cleanup_while_direct_hls_needs_none() {
    use super::super::{HeldLease, Playing};
    let _lock = plx_base::testlock::serial();
    let _globals = globals();
    for cleanup in [false, true] {
        let mut app = fixture();
        assert!(source_available(app.ott.as_ref().unwrap()), "direct M3U HLS has no server lease");
        let (profile, entry) = provider_fixture();
        let (lease, _release, _finished) = crate::app::ott_vportal_lease::Lease::pending_for_test();
        app.ott.as_mut().unwrap().playing = Some(Playing {
            channel_capture: None, kiosk_selection: None, archive: None, transient_owner: None,
            local_selection: None, identity: route::playback_identity(&app.player.session),
            selection_id: entry.id.clone(), profile, entry,
            source: route::external_source(&app.player.session), advance_on_failure: false,
            requires_server_lease: true,
        });
        app.ott.as_mut().unwrap().active_lease = Some(HeldLease::Playback(lease));
        assert!(source_available(app.ott.as_ref().unwrap()));
        let listed_before_loss = list(&mut app);
        let audio = queue(&mut app, 1);
        let (id, _, request) = start(&mut app);
        let lease = app.ott.as_mut().unwrap().active_lease.take().unwrap().into_lease();
        if cleanup { app.ott.as_mut().unwrap().active_lease = Some(HeldLease::Cleanup(lease)); }
        else { drop(lease); }
        assert!(!source_available(app.ott.as_ref().unwrap()));
        let at = now(&app);
        complete(&mut app, id, Ok(request.prepared_for_test()), Some(at));
        assert_eq!(answer(&app, &audio).unwrap()["data"]["error"], "unavailable");
        assert!(selected(&app, 0));
        let listed = list(&mut app);
        assert_eq!(listed["data"]["error"], "unavailable");
        let intent = route::track_intent_ticket();
        let late = raw(&mut app, "media_tracks", target(&listed_before_loss, 1));
        assert_eq!(dispatch(&mut app, &late).unwrap()["data"]["error"], "unavailable");
        assert!(route::track_intent_is_current(intent));
        assert!(!has_owned_pending(app.ott.as_ref().unwrap()));
    }
}

#[test]
fn rejected_seek_preserves_hls_preparation_only_until_its_original_deadline() {
    let _lock = plx_base::testlock::serial();
    let _globals = globals();
    for expired in [false, true] {
        let mut app = fixture();
        let audio = queue(&mut app, 1);
        let (id, _, request) = start(&mut app);
        let seek = raw(&mut app, "playback", json!({"operation":"seek","position":8}));
        assert_eq!(dispatch(&mut app, &seek).unwrap()["status"], "ok");
        // Model the pump consuming then refusing the seek before a timeline
        // commit; no decoder or server operation is executed by this fixture.
        assert_eq!(player::TX.seek_to_ns.swap(-1, Ordering::Relaxed), 8_000_000_000);
        player::abandon_seek();
        assert!(route::hls_track_request_is_current(&app.player.session, &request));
        let at = if expired { audio.deadline_ms } else { now(&app) };
        complete(&mut app, id, Ok(request.prepared_for_test()), Some(at));
        let response = answer(&app, &audio).unwrap();
        if expired {
            assert_eq!(response["data"]["error"], "unavailable");
            assert!(selected(&app, 0));
        } else {
            assert_eq!(response["data"]["outcome"], "requested");
            assert!(selected(&app, 1));
        }
    }
}
