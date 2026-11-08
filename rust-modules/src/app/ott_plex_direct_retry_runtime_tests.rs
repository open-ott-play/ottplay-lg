//! Existing retry entrypoints against an installed, source-qualified Direct
//! route. A real NativeRelease stage holds successful starts before any worker.
//! Authored, not run; these tests do not claim physical playback or PMS effects.
use super::*;
use plx_plex::plex::session::DirectPlayMode;

#[derive(Clone, Copy, Debug)]
enum Entry { Failure, Restart }

struct RetryApp(App);
impl Drop for RetryApp {
    fn drop(&mut self) {
        // The existing modeled-Unload fixture owns no worker. Normal stop must
        // still cancel its queued start before the adapter is dropped.
        // No presented media duration/resource exists in this fixture. Prevent
        // an unrelated process-global duration from inventing a stop report.
        player::SHARED.duration_ns.store(0, Ordering::Release);
        player::stop_bufferfeed(&mut self.0.player.session, &mut self.0.adapters.player);
        player::restore_subtitle_offset(0);
    }
}

fn retry(app: &mut App, entry: Entry, mode: Option<DirectPlayMode>) -> bool {
    match entry {
        Entry::Failure => crate::app::playback::retry_failed_playback(
            &mut app.player.session, &mut app.adapters.player, app.bridge.metadata_mut(),
            mode, player::report::AttemptOrigin::UserRetry,
        ),
        Entry::Restart => {
            assert!(mode.is_none());
            player::engine::restart_current(&mut app.player.session, &mut app.adapters.player,
                app.bridge.metadata_mut())
        }
    }
}

fn selected_fixture() -> RetryApp {
    let mut app = plex_track_fixture();
    player::SHARED.duration_ns.store(0, Ordering::Release);
    // The parent fixture qualifies the original Direct URL using the actual
    // Client token-lock capture, before either temporary selection is accepted.
    assert!(route::can_retry_current_play(&app.player.session));
    let listed = list(&mut app);
    let subtitle = invoke(&mut app, select(&listed, "subtitle", Some(0)));
    assert_eq!(subtitle["data"]["outcome"], "requested");
    let listed = list(&mut app);
    let audio = invoke(&mut app, select(&listed, "audio", Some(1)));
    assert_eq!(audio["data"]["outcome"], "requested");
    assert_eq!(route::cur_audio_sid(&app.player.session), 101);
    assert_eq!(route::cur_sub_sid(&app.player.session), 203);
    assert!(route::has_temporary_track_selection(&app.player.session));
    player::restore_subtitle_offset(250);
    RetryApp(app)
}

#[test]
fn both_retry_entrypoints_queue_exact_direct_source_without_resolving_or_losing_pause() {
    let _lock = plx_base::testlock::serial();
    let _globals = Globals::capture();
    let _registry = PlexRegistryReset;
    for entry in [Entry::Failure, Entry::Restart] {
        let mut rig = selected_fixture();
        let app = &mut rig.0;
        let before = serde_json::to_value(&app.ott.as_ref().unwrap().saved).unwrap();
        let url = route::url(&app.player.session);
        let identity = route::playback_identity(&app.player.session);
        let request = route::play_request_ticket();
        let selected = (route::cur_sid(&app.player.session), route::cur_rk(&app.player.session),
            route::cur_part_id(&app.player.session), route::cur_audio_sid(&app.player.session),
            route::cur_sub_sid(&app.player.session));
        // Only remove the inert fixture Engine (it owns no handles), then use
        // the existing real release state machine to defer the upcoming start.
        drop(app.adapters.player.take());
        app.adapters.player.begin_release_for_test(Duration::ZERO);
        assert!(player::transport::viewer_paused());
        assert!(retry(app, entry, None), "{entry:?}");
        assert!(app.adapters.player.release_has_queued_start(), "{entry:?}");
        assert!(!app.adapters.player.is_live(), "no Load worker started");
        let start = route::pending_route_start().expect("one deferred retry transaction");
        let intent = route::track_intent_ticket();
        assert!(!retry(app, entry, None), "a second retry cannot replace the queued request or renew its budget");
        assert_eq!(route::pending_route_start(), Some(start));
        assert!(route::track_intent_is_current(intent));
        assert_eq!(route::url(&app.player.session), url);
        assert_eq!(route::playback_identity(&app.player.session), identity);
        assert_eq!((route::cur_sid(&app.player.session), route::cur_rk(&app.player.session),
            route::cur_part_id(&app.player.session), route::cur_audio_sid(&app.player.session),
            route::cur_sub_sid(&app.player.session)), selected);
        assert_eq!(route::play_request_ticket(), request, "must not enter the PMS resolver");
        assert!(!app.bridge.metadata_mut().has_jobs());
        assert!(route::has_temporary_track_selection(&app.player.session));
        assert!(player::transport::viewer_paused());
        assert_eq!(player::subtitle_offset_ms(), 250);
        assert_eq!(player::SHARED.seek_to_ns.load(Ordering::Acquire), 7_000_000_000);
        assert_eq!(serde_json::to_value(&app.ott.as_ref().unwrap().saved).unwrap(), before);
        player::stop_bufferfeed(&mut app.player.session, &mut app.adapters.player);
        assert!(!app.adapters.player.release_has_queued_start());
    }
}

#[test]
fn both_retry_entrypoints_refuse_changed_part_or_selected_ids_before_stop() {
    let _lock = plx_base::testlock::serial();
    let _globals = Globals::capture();
    let _registry = PlexRegistryReset;
    for entry in [Entry::Failure, Entry::Restart] {
        for changed in ["part", "audio", "subtitle"] {
            let mut rig = selected_fixture();
            let app = &mut rig.0;
            let mut item = app.bridge.metadata_view().playing().unwrap().clone();
            match changed {
                "part" => item.part_id += 1,
                "audio" => item.audio.retain(|row| row.id != 101),
                _ => item.subs.retain(|row| row.id != 203),
            }
            assert!(app.bridge.metadata_run(plx_data::stores::metadata::MetadataCmd::InstallPlaying(Some(item))));
            let url = route::url(&app.player.session);
            let identity = route::playback_identity(&app.player.session);
            let intent = route::track_intent_ticket();
            let request = route::play_request_ticket();
            assert!(!retry(app, entry, None), "{entry:?}: {changed}");
            assert!(app.adapters.player.is_live(), "refusal must precede teardown");
            assert!(!app.adapters.player.release_has_queued_start());
            assert_eq!(route::url(&app.player.session), url);
            assert_eq!(route::playback_identity(&app.player.session), identity);
            assert!(route::track_intent_is_current(intent));
            assert_eq!(route::play_request_ticket(), request);
            assert!(!app.bridge.metadata_mut().has_jobs());
            assert_eq!(player::subtitle_offset_ms(), 250);
            assert!(player::transport::viewer_paused());
            assert!(route::claim_route_action().is_none());
        }
    }
}

#[test]
fn both_retry_entrypoints_refuse_replaced_or_revoked_and_regranted_source() {
    let _lock = plx_base::testlock::serial();
    let _globals = Globals::capture();
    let _registry = PlexRegistryReset;
    for entry in [Entry::Failure, Entry::Restart] {
        for changed in ["address", "revoke", "regrant"] {
            let mut rig = selected_fixture();
            let app = &mut rig.0;
            let sid = route::cur_sid(&app.player.session);
            let original = plx_plex::plex::client_for(sid).unwrap();
            let grant = original.grant_epoch();
            match changed {
                "address" => {
                    let replacement = plx_plex::plex::register_for_test(
                        "plex-track-fixture", "127.0.0.2", 1, "synthetic-token", "track-test-client");
                    assert_eq!(replacement, sid, "same machine retains its logical ServerId");
                    assert!(!std::ptr::eq(original, plx_plex::plex::client_for(sid).unwrap()));
                }
                _ => {
                    assert!(plx_plex::plex::set_current(sid));
                    plx_plex::plex::revoke_for_profile_switch();
                    if changed == "regrant" {
                        let regranted = plx_plex::plex::register_for_test(
                            "plex-track-fixture", "127.0.0.1", 1, "synthetic-token", "track-test-client");
                        assert_eq!(regranted, sid);
                    }
                }
            }
            assert_ne!(original.grant_epoch(), grant, "real production revocation must retire the witness");
            let url = route::url(&app.player.session);
            let identity = route::playback_identity(&app.player.session);
            let request = route::play_request_ticket();
            assert!(!retry(app, entry, None), "{entry:?}: {changed}");
            assert!(app.adapters.player.is_live());
            assert!(!app.adapters.player.release_has_queued_start());
            assert_eq!(route::url(&app.player.session), url);
            assert_eq!(route::playback_identity(&app.player.session), identity);
            assert_eq!(route::play_request_ticket(), request);
            assert!(!app.bridge.metadata_mut().has_jobs());
            assert!(route::claim_route_action().is_none());
        }
    }
}

#[test]
fn failure_retry_does_not_turn_temporary_direct_selection_into_a_mode_change() {
    let _lock = plx_base::testlock::serial();
    let _globals = Globals::capture();
    let _registry = PlexRegistryReset;
    let mut rig = selected_fixture();
    let app = &mut rig.0;
    let url = route::url(&app.player.session);
    let intent = route::track_intent_ticket();
    let request = route::play_request_ticket();
    assert!(!retry(app, Entry::Failure, Some(DirectPlayMode::Disabled)));
    assert!(app.adapters.player.is_live());
    assert_eq!(route::url(&app.player.session), url);
    assert!(route::track_intent_is_current(intent));
    assert_eq!(route::play_request_ticket(), request);
    assert!(!app.bridge.metadata_mut().has_jobs());
    assert!(route::claim_route_action().is_none());
}

#[test]
fn installed_metadata_without_original_source_witness_has_no_remote_choices() {
    let _lock = plx_base::testlock::serial();
    let _globals = Globals::capture();
    let _registry = PlexRegistryReset;
    let mut rig = RetryApp(plex_track_fixture());
    let app = &mut rig.0;
    let item = app.bridge.metadata_view().playing().unwrap().clone();
    // Reinstall only the advertised leaf shape, deliberately omitting the
    // original Client capture. Equal metadata is not source authority.
    app.player.session = route::PlaybackSession::IDLE;
    route::install_plex_track_fixture_for_test(&mut app.player.session, item.sid,
        &item.rk, item.part_id, route::CarriedAudio::from_stream(&item.audio[1], 0));
    route::reset_player_control_for_test(&app.player.session);
    let listed = list(app);
    assert_eq!(listed["status"], "ok");
    assert!(listed["data"]["tracks"].as_array().unwrap().iter()
        .all(|row| row["selectable"] == false));
    assert_eq!(listed["data"]["subtitle_off_supported"], false);
    let intent = route::track_intent_ticket();
    let response = invoke(app, select(&listed, "subtitle", Some(0)));
    assert_eq!(response["data"]["error"], "not_selectable");
    assert!(route::track_intent_is_current(intent));
    assert!(route::claim_route_action().is_none());
}
