//! Actual Runtime admission and writer completion boundaries. Native route
//! installation is real; Playing below is a modeled callback, not a decoder.
//! Revoked writes prove pre-I/O failure. Injected Saved is not disk durability.
use super::*;
use plx_media::player::{self, PlaybackState};
struct Globals(u8);
impl Globals {
    fn capture() -> Self {
        Self(player::swap_state_for_test(PlaybackState::Idle))
    }
}
impl Drop for Globals {
    fn drop(&mut self) {
        player::SHARED.reset_session();
        player::TX.reset();
        plx_media::route::reset_player_control_for_test(&plx_media::route::PlaybackSession::IDLE);
        player::restore_state_for_test(self.0);
    }
}
fn fixture() -> App {
    let mut app = app_fixture();
    app.running = true;
    let runtime = app.ott.as_mut().unwrap();
    runtime.loaded = true;
    runtime.entropy = Some([2; 16]);
    runtime.write_permit.revoke();
    runtime.resume_source_work();
    let profile = synthetic_profile();
    let mut entry = synthetic_entry();
    entry.id = "chosen".into();
    entry.title = "Chosen".into();
    runtime.provider = profile.kind;
    runtime.saved.profiles.profiles = vec![profile.clone()];
    runtime.saved.profiles.active_profile = Some(profile.id.clone());
    runtime.catalog.entries = vec![entry];
    runtime.catalog_complete = true;
    runtime.catalog_loading = false;
    runtime.resolver_busy = true;
    let arm = crate::app::ott_kiosk_stored::Stored::armed(
        plx_data::ott::favorites::Origin {
            provider: "vportal".into(),
            profile: profile.name.clone(),
            subprofile: None,
            source_alias: profile.name,
        },
        false,
    )
    .unwrap();
    runtime
        .saved
        .settings
        .insert("nativeKiosk".into(), arm.encode().unwrap());
    app
}
fn select_remote(app: &mut App) {
    let mut runtime = app.ott.take().unwrap();
    // A different request from the strict-only kiosk command below. The real
    // ingress correctly deduplicates request_for_test's fixed default ID.
    let mut admitted = runtime.remote.raw_poll_for_test(
        &json!({"commands":[],"server_time":1000,"requests":[{
            "id":"1123456789abcdef0123456789abcdef","action":"channel",
            "params":{"id":"chosen"},"expires_at":1020
        }]}).to_string(),
        100,
    );
    assert_eq!(admitted.len(), 1, "explicit channel request admitted");
    let mut inv = admitted.remove(0);
    inv.deadline_ms = runtime.now_ms().saturating_add(10_000);
    runtime.play_reply(app, "chosen", &inv).unwrap();
    app.ott = Some(runtime);
}
fn install_playing(app: &mut App) {
    let mut runtime = app.ott.take().unwrap();
    let profile = runtime.effective_profile().unwrap().clone();
    let entry = runtime.catalog.entries[0].clone();
    let mut info = unsupported_info();
    info.video_codec = "h264".into();
    info.audio_codec = "aac".into();
    info.audio_channels = 2;
    info.audio_sample_rate = 48_000;
    info.audio_bit_depth = 16;
    let source = plx_media::ff::ExternalSource::Http("https://media.invalid/kiosk-fixture".into());
    // Match the production replacement boundary: an old URL is still owned
    // until teardown, and must not be silently overwritten by a new install.
    playback::replace_player(
        &mut app.player,
        &mut app.adapters.player,
        &mut app.repause_at,
        &mut app.refresh_hubs,
        &mut app.pages,
    );
    plx_media::route::reset_player_control_for_test(&app.player.session);
    plx_media::route::request_external_source(
        &mut app.player.session,
        &info,
        source.clone(),
        "Chosen",
        "Fixture",
        plx_platform::devcaps::assumed_shared(),
    )
    .unwrap();
    let source = plx_media::route::external_source(&app.player.session)
        .expect("canonical source actually installed by the native route");
    let start = plx_media::route::pending_route_start().expect("prepared native route");
    let attempt = plx_media::route::claim_route_start_attempt(start)
        .expect("addressed native start attempt");
    // Model only the native callback, through its actual transaction owner.
    // This is not decoder or physical playback evidence.
    assert!(plx_media::route::settle_route_start(
        &mut app.player.session, attempt, plx_media::route::RouteStartResult::Started,
    ));
    runtime.play_target = None;
    runtime.resolver_next = None;
    runtime.pending_play = None;
    runtime.resolver_busy = false;
    runtime.playing = Some(Playing { channel_capture: None,
        archive: None,
        kiosk_selection: None,
        transient_owner: None,
        local_selection: None,
        identity: plx_media::route::playback_identity(&app.player.session),
        profile,
        entry: entry.clone(),
        selection_id: entry.id,
        source: Some(source),
        advance_on_failure: false,
        requires_server_lease: false,
    });
    player::swap_state_for_test(PlaybackState::Playing);
    assert_eq!(player::state(&app.player.session), PlaybackState::Playing);
    assert!(kiosk_source::observe(&runtime, app).unwrap().is_some(),
        "fixture owns the exact installed source");
    app.ott = Some(runtime);
}
fn capture_tick(app: &mut App) -> bool {
    let mut runtime = app.ott.take().unwrap();
    let result = kiosk_capture::tick(&mut runtime, app).unwrap();
    app.ott = Some(runtime);
    result
}
fn capture_write(app: &mut App) -> (Job, Saved) {
    assert!(capture_tick(app));
    let runtime = app.ott.as_ref().unwrap();
    assert!(runtime.kiosk_capture.pending());
    let observed = kiosk_source::observe(runtime, app).unwrap().unwrap();
    let strict = crate::app::ott_kiosk_stored::read(&runtime.saved.settings)
        .unwrap()
        .unwrap()
        .strict();
    let prepared = kiosk_source::capture_prepared(runtime, &observed, strict).unwrap();
    let after = kiosk_source::save_candidate(runtime, &prepared).unwrap();
    let job = runtime.rx.recv_timeout(Duration::from_secs(5)).unwrap();
    assert!(matches!(job, Job::Saved(Err(_), None, false, _)));
    (job, after)
}
#[test]
fn old_playing_and_buffering_do_not_claim_a_new_arm() {
    let _serial = plx_base::testlock::serial();
    let _globals = Globals::capture();
    let mut app = fixture();
    install_playing(&mut app);
    assert!(!capture_tick(&mut app));
    assert!(!app.ott.as_ref().unwrap().saving);
    select_remote(&mut app);
    install_playing(&mut app);
    player::swap_state_for_test(PlaybackState::Buffering);
    assert!(!capture_tick(&mut app));
    assert!(!app.ott.as_ref().unwrap().kiosk_capture.pending());
}
#[test]
fn explicit_first_playing_capture_definite_failure_keeps_armed() {
    let _serial = plx_base::testlock::serial();
    let _globals = Globals::capture();
    let mut app = fixture();
    select_remote(&mut app);
    install_playing(&mut app);
    let (job, _) = capture_write(&mut app);
    // This case isolates capture settlement. The admitted-selection checkpoint
    // bypasses the position interval, so mark that independent attempt too.
    // It must not occupy the saving flag after the failed capture releases it.
    let runtime = app.ott.as_mut().unwrap();
    assert!(runtime.saving, "the capture still owns the outstanding writer");
    assert!(runtime.checkpoint_save_epoch.is_none(),
        "the outstanding write must not be a bookmark checkpoint");
    let playing = runtime.playing.as_ref().unwrap();
    runtime.checkpoint_write_selection = Some((
        playing.profile.id.clone(), playing.selection_id.clone(),
        profile_fingerprint(&playing.profile),
    ));
    runtime.last_checkpoint_ms = runtime.now_ms();
    app.ott.as_ref().unwrap().tx.send(job).unwrap();
    runtime_tick(&mut app);
    let runtime = app.ott.as_ref().unwrap();
    assert!(!runtime.kiosk_capture.pending());
    assert!(!runtime.saving);
    assert!(runtime.checkpoint_save_epoch.is_none());
    assert_eq!(
        runtime.saved.settings["nativeKiosk"]["target"]["kind"],
        "armed"
    );
    assert!(runtime.saved.favorites.entries().is_empty());
}
#[test]
fn background_late_playing_cancels_only_unwritten_capture() {
    let _serial = plx_base::testlock::serial();
    let _globals = Globals::capture();
    let mut app = fixture();
    select_remote(&mut app);
    install_playing(&mut app);
    app.window_activity.event(0x104);
    assert!(!playback_allowed(&app));
    assert!(!capture_tick(&mut app));
    app.window_activity.event(0x106);
    assert!(playback_allowed(&app));
    assert!(
        !capture_tick(&mut app),
        "foreground does not rearm an old selection"
    );
    assert!(!app.ott.as_ref().unwrap().saving);
}
#[test]
fn dispatched_capture_still_publishes_after_background_without_wake() {
    let _serial = plx_base::testlock::serial();
    let _globals = Globals::capture();
    let mut app = fixture();
    select_remote(&mut app);
    install_playing(&mut app);
    let (_, after) = capture_write(&mut app);
    app.window_activity.event(0x104);
    assert!(!playback_allowed(&app));
    app.ott
        .as_ref()
        .unwrap()
        .tx
        .send(Job::Saved(
            Ok(after),
            None,
            false,
            ok(json!({"persisted":true})),
        ))
        .unwrap();
    runtime_tick(&mut app);
    let runtime = app.ott.as_ref().unwrap();
    assert!(!runtime.kiosk_capture.pending());
    assert_eq!(
        runtime.saved.settings["nativeKiosk"]["target"]["kind"],
        "item"
    );
    assert!(
        runtime.play_target.is_none(),
        "late durability must not wake playback"
    );
}
#[test]
fn capture_unknown_clears_receipt_but_never_reports_an_empty_off_policy() {
    let _serial = plx_base::testlock::serial();
    let _globals = Globals::capture();
    let mut app = fixture();
    select_remote(&mut app);
    install_playing(&mut app);
    let (_, after) = capture_write(&mut app);
    app.ott
        .as_ref()
        .unwrap()
        .tx
        .send(Job::Uncertain(Some(after), None))
        .unwrap();
    runtime_tick(&mut app);
    let runtime = app.ott.as_ref().unwrap();
    assert!(runtime.persistence_uncertain);
    assert!(!runtime.loaded);
    assert!(!runtime.kiosk_capture.pending());
    assert!(runtime.track_selection_kiosk().is_err());
}
#[test]
fn final_native_admission_rebind_rejects_a_changed_source() {
    let _serial = plx_base::testlock::serial();
    let _globals = Globals::capture();
    let mut app = fixture();
    select_remote(&mut app);
    let runtime = app.ott.as_mut().unwrap();
    let work = runtime.resolver_next.take().unwrap();
    runtime.saved.profiles.profiles[0].source =
        "portal::[key:other]https://other.invalid/api".into();
    let source = ReadySource::External(
        plx_media::ff::ExternalSource::Http("https://media.invalid/old".into()),
        unsupported_info(),
        None,
    );
    runtime
        .tx
        .send(Job::Play(
            work.generation,
            work.profile,
            Ok((work.entry.unwrap(), source)),
        ))
        .unwrap();
    runtime_tick(&mut app);
    assert!(app.ott.as_ref().unwrap().playing.is_none());
    assert!(app.ott.as_ref().unwrap().status.contains("changed"));
}

#[test]
fn protected_first_capture_is_denied_before_writer_admission() {
    let _serial = plx_base::testlock::serial();
    let _globals = Globals::capture();
    let mut app = fixture();
    select_remote(&mut app);
    install_playing(&mut app);
    let mut runtime = app.ott.take().unwrap();
    runtime.saved.settings.insert("parentPin".into(), json!("pbkdf2-sha256$600000$000102030405060708090a0b0c0d0e0f$ac310cc8527c6c3a4e20d2a2246ebd77d59766dae1c5fe843d65ecf6a7e20c65"));
    let before = runtime.saved.clone();
    assert!(kiosk_capture::tick(&mut runtime, &app).is_err());
    assert_eq!(runtime.saved.settings, before.settings);
    assert!(!runtime.saving);
    assert!(!runtime.kiosk_capture.pending());
    assert!(runtime.rx.try_recv().is_err());
    app.ott = Some(runtime);
}

#[test]
fn strict_only_saved_preserves_an_armed_explicit_choice_and_captures_current_strict() {
    let _serial = plx_base::testlock::serial();
    let _globals = Globals::capture();
    let mut app = fixture();
    select_remote(&mut app);
    let mut runtime = app.ott.take().unwrap();
    let mut inv = runtime
        .remote
        .request_for_test("kiosk", json!({"mode":"on","strict":true}));
    inv.deadline_ms = runtime.now_ms().saturating_add(10_000);
    assert!(commands::command(&mut runtime, &mut app, &inv).is_none());
    let Job::Saved(Err(_), invocation, reload, response) =
        runtime.rx.recv_timeout(Duration::from_secs(5)).unwrap()
    else {
        panic!("expected actual revoked strict-only writer");
    };
    let mut saved = runtime.saved.clone();
    saved.settings.get_mut("nativeKiosk").unwrap()["strict"] = json!(true);
    runtime
        .tx
        .send(Job::Saved(Ok(saved), invocation, reload, response))
        .unwrap();
    app.ott = Some(runtime);
    runtime_tick(&mut app);
    install_playing(&mut app);
    let (_, after) = capture_write(&mut app);
    app.ott
        .as_ref()
        .unwrap()
        .tx
        .send(Job::Saved(
            Ok(after),
            None,
            false,
            ok(json!({"persisted":true})),
        ))
        .unwrap();
    runtime_tick(&mut app);
    let runtime = app.ott.as_ref().unwrap();
    assert_eq!(runtime.saved.settings["nativeKiosk"]["strict"], true);
    assert_eq!(
        runtime.saved.settings["nativeKiosk"]["target"]["kind"],
        "item"
    );
    assert!(
        !runtime.persistence_uncertain,
        "captured current strict must match the writer receipt"
    );
}

#[test]
fn changed_policy_then_same_arm_cannot_reuse_the_old_explicit_choice() {
    let _serial = plx_base::testlock::serial();
    let _globals = Globals::capture();
    let mut app = fixture();
    select_remote(&mut app);
    let armed = app.ott.as_ref().unwrap().saved.clone();
    let mut off = armed.clone();
    off.settings.remove("nativeKiosk");
    // Modeled authoritative publication boundaries before the selected source
    // has presented a frame. Restoring identical bytes is not the old intent.
    for saved in [off, armed] {
        app.ott
            .as_ref()
            .unwrap()
            .tx
            .send(Job::Saved(
                Ok(saved),
                None,
                false,
                ok(json!({"persisted":true})),
            ))
            .unwrap();
        runtime_tick(&mut app);
    }
    install_playing(&mut app);
    assert!(!capture_tick(&mut app));
    assert!(!app.ott.as_ref().unwrap().saving);
    assert!(!app.ott.as_ref().unwrap().kiosk_capture.pending());
}
