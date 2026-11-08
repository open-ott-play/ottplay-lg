//! The real delete request must retire OTT before Session begins its erase.
//! Only in-memory native routes and held producer slots are used here. Durable
//! private-record removal is covered separately at the storage boundary.
use super::*;
use plx_screens::registry::LoopReq;

struct ResetMedia;
impl Drop for ResetMedia {
    fn drop(&mut self) {
        plx_media::player::SHARED.reset_session();
        plx_media::player::TX.reset();
        plx_media::route::reset_player_control_for_test(&plx_media::route::PlaybackSession::IDLE);
    }
}

#[test]
fn delete_all_local_data_retires_ott_before_session_erasure() {
    let _serial = plx_base::testlock::serial();
    let _media = ResetMedia;
    let mut app = app_fixture();
    mount_player(&mut app);
    let profile = synthetic_profile();
    let mut entry = synthetic_entry();
    entry.id = "erase-fixture".into();
    entry.title = "Synthetic erase fixture".into();
    let source = plx_media::ff::ExternalSource::Http(
        "https://media.invalid/nas/stream/fixture.token/media.mp4".into(),
    );
    let mut info = unsupported_info();
    info.video_codec = "h264".into();
    info.audio_codec = "aac".into();
    info.audio_channels = 2;
    info.audio_sample_rate = 48_000;
    info.audio_bit_depth = 16;
    plx_media::route::request_external_source(
        &mut app.player.session,
        &info,
        source.clone(),
        &entry.title,
        "Fixture",
        plx_platform::devcaps::assumed_shared(),
    )
    .expect("the synthetic native route must be installed before erasure");
    // Admission canonicalizes HTTP addresses. Track the actual native owner,
    // just as the production runtime does, rather than the pre-admission text.
    let source = plx_media::route::external_source(&app.player.session)
        .expect("admission must retain the synthetic native source");
    let (lease, release, _finished) = crate::app::ott_vportal_lease::Lease::pending_for_test();
    let (tx, catalog_tx, metadata_tx, cancel, play_cancel, metadata_cancel, saved) = {
        let runtime = app.ott.as_mut().unwrap();
        runtime.saved.profiles.profiles = vec![profile.clone()];
        runtime.saved.profiles.active_profile = Some(profile.id.clone());
        runtime
            .saved
            .settings
            .insert("erase-marker".into(), json!("old state"));
        runtime.loaded = true;
        runtime.provider = profile.kind;
        runtime.catalog.entries = vec![entry.clone()];
        runtime.generation = 7;
        runtime.continuity = continuity::Continuity::new(7, None);
        runtime.continuity.user_selected(7, &entry.id);
        runtime.playing = Some(Playing { channel_capture: None,
            kiosk_selection: None,
        archive: None,
        transient_owner: None,
        local_selection: None,
            identity: plx_media::route::playback_identity(&app.player.session),
            profile,
            entry: entry.clone(),
            selection_id: entry.id.clone(),
            source: Some(source.clone()),
            advance_on_failure: true,
            requires_server_lease: true,
        });
        runtime.active_lease = Some(lease.into());
        runtime.catalog_loading = true;
        runtime.catalog_done.store(false, Ordering::Release);
        runtime.metadata_done.store(false, Ordering::Release);
        runtime.resolver_busy = true;
        runtime.saving = true;
        // Admission alone starts no transport. If the old Runtime survives, its
        // next tick would still be able to start this pairing producer.
        runtime
            .remote
            .begin_pairing("https://control.invalid", "fixture", 0)
            .unwrap();
        assert_eq!(runtime.remote.snapshot()["state"], "pairing");
        (
            runtime.tx.clone(),
            runtime.catalog_tx.clone(),
            runtime.metadata_tx.clone(),
            runtime.cancel.clone(),
            runtime.play_cancel.clone(),
            runtime.metadata_cancel.clone(),
            runtime.saved.clone(),
        )
    };
    assert_eq!(
        plx_media::route::external_source(&app.player.session),
        Some(source)
    );
    assert!(playing_matches(
        app.ott.as_ref().unwrap().playing.as_ref().unwrap(),
        &app
    ));
    assert!(!cancel.load(Ordering::Acquire));
    assert!(!play_cancel.load(Ordering::Acquire));
    assert!(!metadata_cancel.load(Ordering::Acquire));
    let write_permit = app.ott.as_ref().unwrap().write_permit.clone();
    assert!(write_permit.is_valid());

    app.pages.emit(
        MachineId::Nav,
        Fx::App(AppFx::Loop(LoopReq::DeleteAllLocalData)),
    );
    let tick = next_tick(&mut app);
    bridge::frame(&mut app.pages, &mut app.bridge, tick, Vec::new());
    run::loop_requests(&mut app);

    // No further frame is driven: the Session erase is only queued. Retirement
    // must already have happened, before it can acknowledge a successful clear.
    assert!(
        tx.send(Job::Saved(Ok(saved), None, true, json!({"status":"ok"})))
            .is_err(),
        "Delete all local data left the old OTT receiver open to a late durable save"
    );
    assert!(
        !write_permit.is_valid(),
        "Delete all local data left outstanding writes authorized"
    );
    assert!(
        app.ott.is_none(),
        "OTT must stay retired until erasure is acknowledged"
    );
    assert!(
        cancel.load(Ordering::Acquire),
        "catalogue producer was not cancelled"
    );
    assert!(
        play_cancel.load(Ordering::Acquire),
        "playback resolver was not cancelled"
    );
    assert!(
        metadata_cancel.load(Ordering::Acquire),
        "metadata producer was not cancelled"
    );
    assert!(
        catalog_tx
            .try_send(CatalogEvent::Finished(7, Ok(())))
            .is_err(),
        "the old catalogue receiver survived erasure admission"
    );
    assert!(
        metadata_tx
            .send(crate::app::ott_metadata::Event::Preview(crate::app::ott_metadata::Preview {
                generation: 0,
                profile_id: "portal-fixture".into(),
                entry_id: entry.id,
                entry: None,
                result: Ok(json!({"description":"old metadata"})),
            }))
            .is_err(),
        "the old metadata receiver survived erasure admission"
    );
    assert!(
        plx_media::route::external_source(&app.player.session).is_none(),
        "Delete all local data left the native media source installed"
    );
    assert!(!plx_media::route::has_url(&app.player.session));
    assert!(!plx_media::route::can_retry_current_play(&app.player.session));
    assert!(!plx_media::player::engine::restart_current(
        &mut app.player.session,
        &mut app.adapters.player,
        app.bridge.metadata_mut(),
    ), "an erased source must not be reconstructed by restart");
    assert_eq!(
        release.recv_timeout(Duration::from_millis(100)),
        Ok(()),
        "the active NAS session was not told to stop"
    );
}

#[test]
fn controlled_replay_cannot_retire_ott_or_revoke_its_writes() {
    let _serial = plx_base::testlock::serial();
    let _media = ResetMedia;
    let mut app = app_fixture();
    let initial = crate::app::bootstrap::Initial::synthetic_home(17, 32517, None).unwrap();
    app.bridge = bridge::Bridge::controlled_home(|| 0, &initial, app.adapters.player.mt(), true);
    let (tx, write_permit) = {
        let runtime = app.ott.as_mut().unwrap();
        runtime
            .saved
            .settings
            .insert("erase-marker".into(), json!("keep"));
        (runtime.tx.clone(), runtime.write_permit.clone())
    };
    assert!(app.bridge.is_controlled_replay());
    assert!(write_permit.is_valid());
    assert!(app.bridge.snapshot_session_init().pending_erase.is_none());

    // The actual effect is refused at the controlled Home boundary, before it
    // can enter the loop queue. Also exercise the loop's own erasure guard.
    app.pages.emit(
        MachineId::Nav,
        Fx::App(AppFx::Loop(LoopReq::DeleteAllLocalData)),
    );
    let tick = next_tick(&mut app);
    bridge::frame(&mut app.pages, &mut app.bridge, tick, Vec::new());
    assert_eq!(
        app.bridge.controlled_failure(),
        Some("unsupported Home application effect")
    );
    run::loop_requests(&mut app);
    assert!(!run::request_local_erasure(
        &mut app.rec,
        &mut app.bridge,
        &mut app.pages,
    ));

    assert!(
        write_permit.is_valid(),
        "replay revoked a live write permit"
    );
    assert_eq!(
        app.ott.as_ref().unwrap().saved.settings["erase-marker"],
        json!("keep")
    );
    assert!(tx.send(Job::Loaded(Err("synthetic late result"))).is_ok());
    assert!(app.bridge.snapshot_session_init().pending_erase.is_none());
}

#[test]
fn account_sign_out_preserves_the_independent_ott_runtime() {
    let _serial = plx_base::testlock::serial();
    let _media = ResetMedia;
    let mut app = app_fixture();
    let (tx, write_permit) = {
        let runtime = app.ott.as_mut().unwrap();
        runtime.saved.profiles.profiles = vec![synthetic_profile()];
        runtime
            .saved
            .settings
            .insert("erase-marker".into(), json!("keep"));
        runtime
            .remote
            .begin_pairing("https://control.invalid", "fixture", 0)
            .unwrap();
        (runtime.tx.clone(), runtime.write_permit.clone())
    };
    let epoch = app.bridge.snapshot_session_init().epoch;
    assert!(write_permit.is_valid());
    app.pages.emit(
        MachineId::Nav,
        Fx::App(AppFx::Loop(LoopReq::AccountSignOut)),
    );
    let tick = next_tick(&mut app);
    bridge::frame(&mut app.pages, &mut app.bridge, tick, Vec::new());
    run::loop_requests(&mut app);
    // The ordinary host Bridge uses the real Session machine with its inert
    // resource adapter, so SignOut's all_local=false path executes without IO.
    settle_pages(&mut app);

    assert_ne!(
        app.bridge.snapshot_session_init().epoch,
        epoch,
        "SignOut was not executed"
    );
    assert!(
        write_permit.is_valid(),
        "account SignOut revoked independent OTT writes"
    );
    let runtime = app
        .ott
        .as_ref()
        .expect("account SignOut retired the OTT runtime");
    assert_eq!(runtime.saved.settings["erase-marker"], json!("keep"));
    assert_eq!(runtime.saved.profiles.profiles.len(), 1);
    assert_eq!(runtime.remote.snapshot()["state"], "pairing");
    assert!(tx.send(Job::Loaded(Err("synthetic late result"))).is_ok());
}
