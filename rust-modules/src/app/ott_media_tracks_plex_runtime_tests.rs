//! Installed Plex leaf fixtures through real raw ingress, Catalog and route
//! receipts. No PMS, decoder or persistence worker is started. Authored, unrun.
use super::*;

fn saved(app: &App) -> Value {
    serde_json::to_value(&app.ott.as_ref().unwrap().saved).unwrap()
}

fn queue_audio(app: &mut App) -> Invocation {
    let listed = list(app);
    // Fixture document order is Spanish then currently selected English.
    let invocation = request(app, select(&listed, "audio", Some(0)));
    let mut runtime = app.ott.take().unwrap();
    assert!(commands::command(&mut runtime, app, &invocation).is_none());
    app.ott = Some(runtime);
    assert!(route::pending_user_route_intent(route::UserRouteIntent::NativeAudioReload));
    invocation
}

// Cached Remote::complete responses include the original delivery ID.
fn assert_completed_track_error(response: &Value, invocation: &Invocation, code: &str) {
    let raw = invocation.media_tracks_projection().expect("retained raw projection");
    let raw: Value = serde_json::from_slice(raw).unwrap();
    let params = &raw["params"];
    assert_eq!(response, &json!({"id":invocation.id,"status":"rejected","data":{
        "version":1,"runtime":params["runtime"],"op":"select",
        "state":"error","error":code,"playback":params["playback"],
        "source_generation":params["source_generation"],"catalog":params["catalog"],
        "target":params["target"]
    }}));
}

#[test]
fn plex_subtitle_and_off_are_temporary_requested_results_without_playback_changes() {
    let _lock = plx_base::testlock::serial();
    let _globals = Globals::capture();
    let _registry = PlexRegistryReset;
    let mut app = plex_track_fixture();
    let before = saved(&app);
    let identity = route::playback_identity(&app.player.session);
    let first_intent = route::track_intent_ticket();
    let listed = list(&mut app);
    let selected = invoke(&mut app, select(&listed, "subtitle", Some(0)));
    assert_eq!(selected["data"]["outcome"], "requested");
    assert!(selected["data"]["observed"].is_null());
    assert!(!route::track_intent_is_current(first_intent));
    let listed = list(&mut app);
    let off = invoke(&mut app, select(&listed, "subtitle", None));
    assert_eq!(off["data"]["outcome"], "requested");
    assert!(off["data"]["observed"].is_null());
    assert_eq!(player::desired_sub_idx(), -1);
    assert_eq!(route::playback_identity(&app.player.session), identity);
    assert_eq!(player::state(&app.player.session), player::PlaybackState::Paused);
    assert_eq!(player::SHARED.playpos_ns.load(Ordering::Acquire), 7_000_000_000);
    assert_eq!(saved(&app), before);
    assert!(!app.ott.as_ref().unwrap().saving);
    assert!(route::claim_route_action().is_none());
}

#[test]
fn plex_accepted_audio_noop_retires_the_old_catalog_intent() {
    let _lock = plx_base::testlock::serial();
    let _globals = Globals::capture();
    let _registry = PlexRegistryReset;
    let mut app = plex_track_fixture();
    let listed = list(&mut app);
    let before = saved(&app);
    let intent = route::track_intent_ticket();
    let reply = invoke(&mut app, select(&listed, "audio", Some(1)));
    assert_eq!(reply["data"]["outcome"], "requested");
    assert!(!route::track_intent_is_current(intent));
    assert!(!commands::track_request_pending(app.ott.as_ref().unwrap()));
    assert!(route::claim_route_action().is_none());
    let stale = invoke(&mut app, select(&listed, "subtitle", Some(0)));
    assert_eq!(stale["data"]["error"], "playback_changed");
    assert_eq!(saved(&app), before);
}

#[test]
fn plex_audio_reply_waits_for_owned_native_start_and_never_claims_confirmation() {
    let _lock = plx_base::testlock::serial();
    let _globals = Globals::capture();
    let _registry = PlexRegistryReset;
    let mut app = plex_track_fixture();
    let before = saved(&app);
    let invocation = queue_audio(&mut app);
    let now = audio_now(&app);
    poll_audio(&mut app, Some(now));
    assert!(audio_response(&app, &invocation).is_none());
    let attempt = claim_audio_start(&mut app);
    poll_audio(&mut app, Some(now));
    assert!(audio_response(&app, &invocation).is_none());
    pump_native_answer(&mut app, attempt, route::RouteStartResult::Started);
    poll_audio(&mut app, Some(now));
    let response = audio_response(&app, &invocation).expect("owned terminal receipt");
    assert_eq!(response["data"]["outcome"], "requested");
    assert!(response["data"]["observed"].is_null());
    assert_eq!(saved(&app), before);
    assert!(!commands::track_request_pending(app.ott.as_ref().unwrap()));
}

#[test]
fn plex_catalog_renewal_cannot_extend_an_admitted_audio_deadline() {
    let _lock = plx_base::testlock::serial();
    let _globals = Globals::capture();
    let _registry = PlexRegistryReset;
    let mut app = plex_track_fixture();
    let before = saved(&app);
    let invocation = queue_audio(&mut app);
    let _ = adapter_query(&mut app, 44, "media_tracks", json!({"op":"list"}));
    poll_audio(&mut app, Some(invocation.deadline_ms));
    let response = audio_response(&app, &invocation).expect("expired request receipt");
    assert_eq!(response["data"]["error"], "unavailable");
    assert!(!commands::track_request_pending(app.ott.as_ref().unwrap()));
    assert!(route::claim_route_action().is_none());
    assert_eq!(saved(&app), before);
}

#[test]
fn plex_old_catalog_cannot_target_reordered_metadata_or_new_same_item_playback() {
    let _lock = plx_base::testlock::serial();
    let _globals = Globals::capture();
    let _registry = PlexRegistryReset;
    for replace_playback in [false, true] {
        let mut app = plex_track_fixture();
        let listed = list(&mut app);
        let mut item = app.bridge.metadata_view().playing().unwrap().clone();
        if replace_playback {
            route::install_plex_track_fixture_for_test(&mut app.player.session, item.sid, &item.rk, 77,
                route::CarriedAudio::from_stream(&item.audio[1], 0));
            assert!(route::qualify_plex_direct_retry_fixture_for_test(&mut app.player.session,
                plx_plex::plex::client_for(item.sid).unwrap()));
        } else {
            item.audio.reverse();
            assert!(app.bridge.metadata_run(plx_data::stores::metadata::MetadataCmd::InstallPlaying(Some(item))));
        }
        let identity = route::playback_identity(&app.player.session);
        let response = invoke(&mut app, select(&listed, "audio", Some(0)));
        assert_eq!(response["data"]["error"], "playback_changed");
        assert_eq!(route::playback_identity(&app.player.session), identity);
        assert!(route::claim_route_action().is_none());
    }
}

#[test]
fn plex_same_leaf_other_part_cannot_reuse_stale_installed_metadata() {
    let _lock = plx_base::testlock::serial();
    let _globals = Globals::capture();
    let _registry = PlexRegistryReset;
    let mut app = plex_track_fixture();
    let listed = list(&mut app);
    let mut item = app.bridge.metadata_view().playing().unwrap().clone();
    assert_eq!(item.part_id, 77);
    route::install_plex_track_fixture_for_test(&mut app.player.session, item.sid, &item.rk, 88,
        route::CarriedAudio::from_stream(&item.audio[1], 0));
    assert_eq!(list(&mut app)["data"]["error"], "catalog_incomplete");
    assert_eq!(track_capability(&mut app, 71), Value::Null);
    let response = invoke(&mut app, select(&listed, "audio", Some(0)));
    assert_eq!(response["data"]["error"], "catalog_incomplete");
    assert!(route::claim_route_action().is_none());
    item.part_id = 88;
    assert!(app.bridge.metadata_run(plx_data::stores::metadata::MetadataCmd::InstallPlaying(Some(item))));
    let fresh = list(&mut app);
    assert_eq!(fresh["status"], "ok");
    assert_ne!(fresh["data"]["playback"], listed["data"]["playback"]);
    let stale = invoke(&mut app, select(&listed, "audio", Some(0)));
    assert_eq!(stale["data"]["error"], "playback_changed");
    assert!(route::claim_route_action().is_none());
}

#[test]
fn plex_sidecar_inventory_is_visible_but_cannot_borrow_a_native_subtitle_choice() {
    let _lock = plx_base::testlock::serial();
    let _globals = Globals::capture();
    let _registry = PlexRegistryReset;
    let mut app = plex_track_fixture();
    let mut item = app.bridge.metadata_view().playing().unwrap().clone();
    item.subs[0].external = true;
    item.subs[0].index = -1;
    item.subs[0].key = "/library/streams/203".into();
    assert!(app.bridge.metadata_run(plx_data::stores::metadata::MetadataCmd::InstallPlaying(Some(item))));
    let listed = list(&mut app);
    assert_eq!(listed["status"], "ok");
    let rows = listed["data"]["tracks"].as_array().unwrap();
    let subtitle = rows.iter().find(|row| row["kind"] == "subtitle").unwrap();
    assert_eq!(subtitle["selectable"], false);
    let intent = route::track_intent_ticket();
    let response = invoke(&mut app, select(&listed, "subtitle", Some(0)));
    assert_eq!(response["data"]["error"], "not_selectable");
    assert!(route::track_intent_is_current(intent));
    assert!(route::claim_route_action().is_none());
}

#[test]
fn plex_late_audio_completion_cannot_acknowledge_a_new_logical_playback() {
    let _lock = plx_base::testlock::serial();
    let _globals = Globals::capture();
    let _registry = PlexRegistryReset;
    let mut app = plex_track_fixture();
    let invocation = queue_audio(&mut app);
    let attempt = claim_audio_start(&mut app);
    let item = app.bridge.metadata_view().playing().unwrap().clone();
    route::install_plex_track_fixture_for_test(&mut app.player.session, item.sid, &item.rk, 77,
        route::CarriedAudio::from_stream(&item.audio[1], 0));
    assert!(route::qualify_plex_direct_retry_fixture_for_test(&mut app.player.session,
        plx_plex::plex::client_for(item.sid).unwrap()));
    let identity = route::playback_identity(&app.player.session);
    pump_native_answer(&mut app, attempt, route::RouteStartResult::Started);
    let now = audio_now(&app);
    poll_audio(&mut app, Some(now));
    assert_eq!(audio_response(&app, &invocation).unwrap()["data"]["error"], "playback_changed");
    assert_eq!(route::playback_identity(&app.player.session), identity);
}

#[test]
fn plex_old_receipt_does_not_retire_catalog_from_newer_accepted_intent() {
    let _lock = plx_base::testlock::serial();
    let _globals = Globals::capture();
    let _registry = PlexRegistryReset;
    let mut app = plex_track_fixture();
    let invocation = queue_audio(&mut app);
    let attempt = claim_audio_start(&mut app);
    pump_native_answer(&mut app, attempt, route::RouteStartResult::Started);
    player::swap_state_for_test(player::PlaybackState::Paused);
    // Another accepted core caller wins before the old adapter polls its receipt.
    // This exercises shared intent ownership without invoking any persistent menu.
    let item = app.bridge.metadata_view().playing().unwrap().clone();
    let tracks = route::PlexTracks::capture(&app.player.session, &item).unwrap();
    let choice = tracks.subtitle_off().unwrap();
    let runtime = app.ott.as_ref().unwrap();
    let deadline = runtime.clock_anchor.unwrap().deadline_at_unix_ms(runtime.now_ms() + 10_000);
    assert!(route::apply_plex_track_request(&mut app.player.session, &choice, deadline, app.adapters.player.mt()).unwrap().is_none());
    let fresh = adapter_query(&mut app, 61, "media_tracks", json!({"op":"list"}));
    assert_eq!(fresh["status"], "ok");
    let now = audio_now(&app);
    poll_audio(&mut app, Some(now));
    assert_eq!(audio_response(&app, &invocation).unwrap()["data"]["error"], "catalog_changed");
    let response = adapter_query(&mut app, 62, "media_tracks", select(&fresh, "subtitle", None));
    assert_eq!(response["data"]["outcome"], "requested", "{response}");
}

#[test]
fn plex_claimed_audio_cancellation_cannot_ack_late_start_or_poison_future_intent() {
    let _lock = plx_base::testlock::serial();
    let _globals = Globals::capture();
    let _registry = PlexRegistryReset;
    let mut app = plex_track_fixture();
    let before = saved(&app);
    let invocation = queue_audio(&mut app);
    let attempt = claim_audio_start(&mut app);
    let admitted_intent = route::track_intent_ticket();
    app.ott_standby = true;
    let now = audio_now(&app);
    poll_audio(&mut app, Some(now));
    assert_completed_track_error(&audio_response(&app, &invocation).unwrap(), &invocation, "restricted");
    assert!(!commands::track_request_pending(app.ott.as_ref().unwrap()));
    assert!(!route::track_intent_is_current(admitted_intent), "claimed effect must be revoked synchronously");
    let cancelled_intent = route::track_intent_ticket();
    app.ott_standby = false;
    // Feed the actual mailbox a late result. This proves receipt retirement,
    // not that an already performed physical Load can be undone.
    pump_native_answer(&mut app, attempt, route::RouteStartResult::Started);
    poll_audio(&mut app, Some(now));
    assert!(route::track_intent_is_current(cancelled_intent));
    assert_completed_track_error(&audio_response(&app, &invocation).unwrap(), &invocation, "restricted");
    player::swap_state_for_test(player::PlaybackState::Paused);
    let fresh = adapter_query(&mut app, 81, "media_tracks", json!({"op":"list"}));
    assert_eq!(fresh["status"], "ok");
    let response = adapter_query(&mut app, 82, "media_tracks", select(&fresh, "subtitle", None));
    assert_eq!(response["data"]["outcome"], "requested", "future selection after resume: {response}");
    let newer_intent = route::track_intent_ticket();
    pump_native_answer(&mut app, attempt, route::RouteStartResult::Started);
    poll_audio(&mut app, Some(now));
    assert!(route::track_intent_is_current(newer_intent), "duplicate old result cannot supersede new intent");
    assert_eq!(player::desired_sub_idx(), -1);
    assert_completed_track_error(&audio_response(&app, &invocation).unwrap(), &invocation, "restricted");
    assert_eq!(saved(&app), before);
    assert!(!app.ott.as_ref().unwrap().saving);
}

#[test]
fn plex_pending_track_refuses_kiosk_mutation_but_keeps_status_and_request() {
    let _lock = plx_base::testlock::serial();
    let _globals = Globals::capture();
    let _registry = PlexRegistryReset;
    for claimed in [false, true] {
        let mut app = plex_track_fixture();
        let before = saved(&app);
        let invocation = queue_audio(&mut app);
        let attempt = claimed.then(|| claim_audio_start(&mut app));
        let intent = route::track_intent_ticket();
        assert!(commands::plex_track_request_pending(app.ott.as_ref().unwrap()));
        let status = adapter_query(&mut app, 91, "kiosk", json!({"mode":"status"}));
        assert_eq!(status["status"], "ok");
        for (i, params) in [json!({"mode":"on"}), json!({"mode":"off"}), json!({"mode":"on","strict":true})].into_iter().enumerate() {
            let rejected = adapter_query(&mut app, 92 + i as u32, "kiosk", params);
            assert_eq!(rejected["status"], "rejected", "{rejected}");
            assert!(route::track_intent_is_current(intent));
            assert!(commands::plex_track_request_pending(app.ott.as_ref().unwrap()));
            assert!(!commands::kiosk_pending(app.ott.as_ref().unwrap()));
            assert!(!app.ott.as_ref().unwrap().saving);
            assert!(audio_response(&app, &invocation).is_none());
            assert_eq!(saved(&app), before);
        }
        let attempt = attempt.unwrap_or_else(|| claim_audio_start(&mut app));
        pump_native_answer(&mut app, attempt, route::RouteStartResult::Started);
        let now = audio_now(&app);
        poll_audio(&mut app, Some(now));
        assert_eq!(audio_response(&app, &invocation).unwrap()["data"]["outcome"], "requested");
        assert!(!commands::plex_track_request_pending(app.ott.as_ref().unwrap()));
    }
}

#[test]
fn plex_pending_track_defers_capture_without_consuming_its_unwritten_intent() {
    let _lock = plx_base::testlock::serial();
    let _globals = Globals::capture();
    let _registry = PlexRegistryReset;
    let mut app = plex_track_fixture();
    let invocation = queue_audio(&mut app);
    {
        let runtime = app.ott.as_mut().unwrap();
        runtime.resume_source_work();
        let profile = synthetic_profile();
        let entry = synthetic_entry();
        let armed = crate::app::ott_kiosk_stored::Stored::armed(
            plx_data::ott::favorites::Origin {
                provider: "vportal".into(), profile: profile.name.clone(),
                subprofile: None, source_alias: profile.name.clone(),
            }, false,
        ).unwrap();
        runtime.provider = profile.kind;
        runtime.saved.profiles.active_profile = Some(profile.id.clone());
        runtime.saved.profiles.profiles = vec![profile];
        runtime.play_target = Some(PlaybackRequest::ordinary(&entry.id, 0));
        runtime.catalog.entries = vec![entry];
        runtime.catalog_complete = true;
        runtime.catalog_loading = false;
        runtime.saved.settings.insert("nativeKiosk".into(), armed.encode().unwrap());
        // Model a late policy publication over the already admitted track
        // request, and retain an actual explicit-choice capture Intent.
        kiosk_capture::explicit_admitted(runtime);
        // A sentinel makes later Intent consumption observable without invoking
        // a capture writer or pretending the Plex fixture owns this catalog.
        runtime.saved.settings.insert("nativeKiosk".into(), json!({"invalid":true}));
    }
    let before = saved(&app);
    let mut runtime = app.ott.take().unwrap();
    assert!(commands::plex_track_request_pending(&runtime));
    assert_eq!(kiosk_capture::tick(&mut runtime, &app).unwrap(), false);
    assert!(!runtime.saving);
    assert!(!runtime.kiosk_capture.pending());
    app.ott = Some(runtime);
    let now = audio_now(&app);
    poll_audio(&mut app, Some(now));
    assert_completed_track_error(&audio_response(&app, &invocation).unwrap(), &invocation, "restricted");
    let mut runtime = app.ott.take().unwrap();
    assert!(!commands::plex_track_request_pending(&runtime));
    assert!(matches!(kiosk_capture::tick(&mut runtime, &app), Err(kiosk_source::SourceError::Invalid)),
        "deferred Intent reaches validation once the track request settles");
    assert!(!runtime.saving);
    assert!(!runtime.kiosk_capture.pending());
    app.ott = Some(runtime);
    assert_eq!(saved(&app), before);
}
