//! Late catalogue and persistence replies must not replace a newer user intent.
//! The retained Plex request is injected before its native resolve completes;
//! all catalogue and save replies are synthetic, with their workers held inert.
use super::*;

#[path = "plex_refresh.rs"]
mod plex_refresh;

#[test]
fn native_plex_request_prevents_late_local_resume_from_replacing_it() {
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
        "superseding-plex-fixture",
        "127.0.0.1",
        1,
        "fixture-token",
        "fixture-client",
    );
    plx_plex::plex::client_for(sid).unwrap().disable_data_io();
    let mut app = ready_app_fixture();
    local_playing(
        &mut app,
        Some(continuity::ResumePoint {
            entry_id: "late".into(),
            position_ms: 42_000,
        }),
        8_000,
    );
    let outgoing_source = plx_media::route::external_source(&app.player.session);

    // Native request_play retains the outgoing route during resolution. Inject
    // only its new request identity, without starting the Plex/network worker.
    plx_media::route::install_plex_request_for_test(&mut app.player.session, sid, "42");
    assert_eq!(
        plx_media::route::playback_request_identity(&app.player.session),
        Some((sid, "42")),
        "premise: a different native Plex item was requested"
    );
    assert_eq!(
        plx_media::route::external_source(&app.player.session),
        outgoing_source,
        "premise: Local's outgoing route is still installed during native resolve"
    );
    app.ott
        .as_ref()
        .unwrap()
        .catalog_tx
        .send(CatalogEvent::Batch(
            7,
            Catalog { hierarchy: None,
                entries: vec![local_entry("late")],
            },
        ))
        .unwrap_or_else(|_| panic!("fixture catalogue receiver disappeared"));

    runtime_tick(&mut app);

    let runtime = app.ott.as_ref().unwrap();
    assert!(
        runtime.resolver_next.is_none() && runtime.play_target.is_none(),
        "late Local discovery must not queue a restore over the newer native Plex request"
    );
    assert_eq!(
        plx_media::route::playback_request_identity(&app.player.session),
        Some((sid, "42")),
        "retiring Local ownership must preserve the replacement Plex request"
    );
}

#[test]
fn late_saved_reload_after_exit_cannot_restart_the_first_local_batch() {
    let _serial = plx_base::testlock::serial();
    let _reset = ResetMedia;
    let mut app = ready_app_fixture();
    local_playing(&mut app, None, 8_000);
    let mut saved = app.ott.as_ref().unwrap().saved.clone();
    saved.category = "Fixture folder".into();
    // The fixture represents a save already in flight before Exit. Prevent the
    // later reload from launching a filesystem worker; replies arrive below.
    app.ott
        .as_ref()
        .unwrap()
        .catalog_done
        .store(false, Ordering::Release);

    exit_through_content(&mut app);
    runtime_tick(&mut app);
    assert!(app.player.pending_exit.is_none(), "Exit was consumed");
    assert!(app.ott.as_ref().unwrap().playing.is_none());
    assert!(!matches!(app.route(), AppArg::Player));

    app.ott
        .as_ref()
        .unwrap()
        .tx
        .send(Job::Saved(Ok(saved), None, true, json!({"status":"ok"})))
        .unwrap_or_else(|_| panic!("fixture saved-reply receiver disappeared"));
    runtime_tick(&mut app);
    let runtime = app.ott.as_ref().unwrap();
    assert!(!runtime.saving, "the pending save reply must be accepted");
    assert_eq!(runtime.saved.category, "Fixture folder");
    let generation = runtime.generation;
    runtime
        .catalog_tx
        .send(CatalogEvent::Batch(
            generation,
            Catalog { hierarchy: None,
                entries: vec![local_entry("first")],
            },
        ))
        .unwrap_or_else(|_| panic!("fixture catalogue receiver disappeared"));

    runtime_tick(&mut app);

    let runtime = app.ott.as_ref().unwrap();
    assert!(
        runtime
            .catalog
            .entries
            .iter()
            .any(|entry| entry.id == "first"),
        "the post-save catalogue may refresh while playback stays stopped"
    );
    assert!(
        runtime.resolver_next.is_none() && runtime.play_target.is_none(),
        "a pre-Exit save must not re-enable autoplay when its reload discovers a file"
    );
    assert!(runtime.playing.is_none());
    assert!(!matches!(app.route(), AppArg::Player));
}

fn late_saved_preserves_new_local_play(source_changed: bool) {
    let _serial = plx_base::testlock::serial();
    let _reset = ResetMedia;
    let mut app = ready_app_fixture();
    local_playing(&mut app, None, 8_000);
    // Capture the outgoing position while the fixture's writer is still held.
    // A real save includes these points before dispatching its immutable image.
    // Otherwise the reply below immediately schedules an unrelated checkpoint
    // write, so `saving` no longer describes the old reply being tested.
    {
        let mut runtime = app.ott.take().unwrap();
        assert!(runtime.saving, "checkpoint capture must not start a real writer");
        runtime.checkpoint(&app, true);
        let profile_id = &runtime.active().unwrap().id;
        assert_eq!(runtime.checkpoints.get(profile_id).unwrap().position_ms, 8_000);
        app.ott = Some(runtime);
    }
    let native_source = plx_media::route::external_source(&app.player.session);
    let (saved, generation, cancel, profile_id) = {
        let runtime = app.ott.as_mut().unwrap();
        runtime.catalog_done.store(false, Ordering::Release);
        runtime.catalog.entries.push(local_entry("second"));
        let profile_id = runtime.active().unwrap().id.clone();
        let mut saved = runtime.saved.clone();
        kiosk_resume::merge(&mut saved, &runtime.checkpoints);
        saved.category = "Fixture folder".into();
        if source_changed {
            let mut replacement = runtime.active().unwrap().clone();
            replacement.id = "replacement-folder".into();
            replacement.source = "/media/replacement-fixture".into();
            saved.profiles.active_profile = Some(replacement.id.clone());
            saved.profiles.profiles.push(replacement);
        }
        // Snapshot the writer's admission before the newer user selection.
        // local_playing holds both the writer and resolver boundaries inert.
        runtime.save_playback_epoch = runtime.playback_epoch;
        explicit_play(&mut app, "second");
        let runtime = app.ott.as_mut().unwrap();
        assert_ne!(runtime.save_playback_epoch, runtime.playback_epoch);
        (
            saved,
            runtime.play_generation,
            runtime.play_cancel.clone(),
            profile_id,
        )
    };
    let saved_profile = saved.profiles.active_profile.clone();
    app.ott
        .as_ref()
        .unwrap()
        .tx
        .send(Job::Saved(Ok(saved), None, true, json!({"status":"ok"})))
        .unwrap_or_else(|_| panic!("fixture saved-reply receiver disappeared"));

    runtime_tick(&mut app);

    let runtime = app.ott.as_ref().unwrap();
    assert!(!runtime.saving, "the durable save reply must be accepted");
    assert_eq!(runtime.saved.category, "Fixture folder");
    assert_eq!(runtime.saved.profiles.active_profile, saved_profile);
    assert_eq!(
        plx_media::route::external_source(&app.player.session),
        native_source,
        "a stale save must not tear down the native route while a newer video opens"
    );
    assert_eq!(
        runtime.play_generation, generation,
        "a stale save must not invalidate the newer user's resolver generation"
    );
    assert!(!cancel.load(Ordering::Acquire));
    assert_eq!(runtime.play_target, Some(PlaybackRequest::ordinary("second", 0)));
    assert!(
        runtime.resolver_next.as_ref().is_some_and(|next| {
            next.generation == generation
                && next.profile.id == profile_id
                && next
                    .entry
                    .as_ref()
                    .is_some_and(|entry| entry.id == "second")
        }),
        "the newer request must retain the source captured when it was selected"
    );
    // Catalogue refresh remains allowed, but its first result is not a new
    // playback instruction and must not replace the still-resolving selection.
    runtime
        .catalog_tx
        .send(CatalogEvent::Batch(
            runtime.generation,
            Catalog { hierarchy: None,
                entries: vec![local_entry("refreshed-first")],
            },
        ))
        .unwrap_or_else(|_| panic!("fixture catalogue receiver disappeared"));
    runtime_tick(&mut app);
    let runtime = app.ott.as_ref().unwrap();
    assert_eq!(runtime.play_generation, generation);
    assert_eq!(runtime.play_target, Some(PlaybackRequest::ordinary("second", 0)));
    assert!(!cancel.load(Ordering::Acquire));
}

#[test]
fn late_saved_reload_preserves_a_newer_local_play_request() {
    late_saved_preserves_new_local_play(false);
}

#[test]
fn late_saved_source_change_preserves_a_newer_local_play_request() {
    late_saved_preserves_new_local_play(true);
}
