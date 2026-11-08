//! Physical SDL bytes through the real application ingress; no media/network worker runs.
use super::*;
use plx_ui::consts::{SDLK_DOWN, SDLK_LEFT, SDLK_RIGHT, SDLK_UP};
use std::sync::atomic::Ordering::Relaxed;

struct Reset;
impl Drop for Reset {
    fn drop(&mut self) {
        plx_media::player::SHARED.reset_session();
        plx_media::player::TX.reset();
        plx_media::route::reset_player_control_for_test(&plx_media::route::PlaybackSession::IDLE);
    }
}
fn entry(id: &str) -> CatalogEntry {
    CatalogEntry {
        synopsis: None,
        id: id.into(),
        title: id.into(),
        category: "Videos".into(),
        description: String::new(),
        poster: String::new(),
        media: MediaLocator::Http {
            url: format!("https://example.invalid/{id}.mp4"),
        },
        duration_ms: Some(60_000),
        provider_id: Some(id.into()),
        provider_id_field: None,
        live: None,
    }
}
fn fixture() -> App {
    let mut app = app_fixture();
    app.running = true;
    let profile = ProviderProfile {
        id: "desktop".into(),
        name: "Desktop".into(),
        kind: ProviderKind::M3u,
        source: "https://example.invalid/list.m3u".into(),
        source_alias: String::new(),
        enabled: true,
    };
    let first = entry("first");
    plx_media::route::reset_player_control_for_test(&app.player.session);
    plx_media::route::request_external_source(
        &mut app.player.session,
        &plx_media::ff::inspect::MediaInfo {
            tracks: Default::default(),
            resolved_source: None,
            hls_playback: None,
            video_codec: "h264".into(),
            audio_codec: "aac".into(),
            width: 1280,
            height: 720,
            fps: 24.0,
            duration_ns: 60_000_000_000,
            audio_channels: 2,
            audio_sample_rate: 48_000,
            audio_bit_depth: 16,
            dovi: Default::default(),
        },
        plx_media::ff::ExternalSource::Http("https://example.invalid/first.mp4".into()),
        "First",
        "Videos",
        plx_platform::devcaps::assumed_shared(),
    )
    .unwrap();
    plx_media::route::reset_player_control_for_test(&app.player.session);
    let runtime = app.ott.as_mut().unwrap();
    runtime.loaded = true;
    runtime.resume_source_work();
    runtime.saved.profiles.profiles = vec![profile.clone()];
    runtime.saved.profiles.active_profile = Some(profile.id.clone());
    runtime.provider = ProviderKind::M3u;
    runtime.page = "channels";
    runtime.catalog.entries = vec![first.clone(), entry("second"), entry("third")];
    runtime.selected = Some("first".into());
    runtime.playing = Some(Playing { channel_capture: None,
        kiosk_selection: None,
        archive: None,
        transient_owner: None,
        local_selection: None,
        identity: plx_media::route::playback_identity(&app.player.session),
        profile,
        entry: first,
        selection_id: "first".into(),
        source: plx_media::route::external_source(&app.player.session),
        advance_on_failure: false,
        requires_server_lease: false,
    });
    runtime.saving = true; // Hold persistence and the resolver; neither may perform I/O.
    runtime.resolver_busy = true;
    mount_player(&mut app);
    app.adapters
        .player
        .install(plx_media::player::engine::engine_after_reload_for_test());
    plx_media::player::TX.commit_paused(false);
    plx_media::player::swap_state_for_test(plx_media::player::PlaybackState::Playing);
    plx_media::player::SHARED
        .duration_ns
        .store(60_000_000_000, Relaxed);
    plx_media::player::SHARED
        .playpos_ns
        .store(20_000_000_000, Relaxed);
    plx_media::player::volume::set_volume(
        &mut app.adapters.player,
        &mut app.player.volume,
        plx_media::player::volume::DesiredVolume::new(50, false).unwrap(),
    );
    bridge::player_mut(&mut app.pages).unwrap().hud.hide(false);
    app.inputs.clear();
    app
}
// Re-enter the real bound-key prelude: extend alone cannot undo hide().
fn reveal_hud(app: &mut App) {
    let now = clock::now();
    let paused = plx_media::player::transport::paused();
    let hud = &mut bridge::player_mut(&mut app.pages).unwrap().hud;
    hud.note_fresh_press(&app.player.session, now, paused);
    hud.extend(now, 60_000);
    assert!(hud.visible(&app.player.session, now, paused), "premise: visible HUD");
}

fn physical(sym: u32, modifiers: u16, down: bool, repeat: bool) -> [u8; 128] {
    let mut bytes = [0; 128];
    bytes[0..4].copy_from_slice(&(if down { 0x300_u32 } else { 0x301_u32 }).to_ne_bytes());
    bytes[8..12].copy_from_slice(&1_u32.to_ne_bytes()); // A real SDL window, not SYNTH_WINDOW.
    bytes[12] = u8::from(down);
    bytes[13] = u8::from(repeat);
    bytes[20..24].copy_from_slice(&sym.to_ne_bytes());
    bytes[24..26].copy_from_slice(&modifiers.to_ne_bytes());
    bytes
}
fn ingest(app: &mut App, bytes: [u8; 128]) {
    app.ev = bytes;
    let mut frame = run::Frame {
        ctrl: plx_appkit::player_hud::slot(&app.player.session, app.bridge.metadata_view()),
        now: clock::now(),
        dt: 0.016,
        underlay_moving: false,
        player: true,
        present: false,
        deferred: false,
        rn: "player",
    };
    unsafe {
        run::ingest_sdl_event_with_window(app, &mut frame, |_| {
            panic!("keyboard cannot restore a window")
        });
    }
}
fn tap(app: &mut App, sym: u32, modifiers: u16) {
    ingest(app, physical(sym, modifiers, true, false));
    ingest(app, physical(sym, 0, false, false)); // Modifier can be released first.
}

#[test]
fn mac_physical_video_arrows_change_volume_and_modified_arrows_seek() {
    let _lock = plx_base::testlock::serial();
    let _reset = Reset;
    let mut app = fixture();
    tap(&mut app, SDLK_RIGHT, 0);
    assert_eq!(app.player.volume.desired().level(), 55);
    assert_eq!(plx_media::player::seek_pending(), -1);
    tap(&mut app, SDLK_LEFT, 0);
    assert_eq!(app.player.volume.desired().level(), 50);
    for modifier in [0x0040, 0x0100] {
        tap(&mut app, SDLK_RIGHT, modifier);
    }
    assert_eq!(plx_media::player::seek_pending(), 40_000_000_000);
    assert_eq!(app.player.volume.desired().level(), 50);
    assert!(
        app.inputs.is_empty(),
        "owned arrows must not also enter legacy scrub"
    );
}

#[test]
fn mac_physical_video_step_uses_playing_item_not_browse_selection() {
    let _lock = plx_base::testlock::serial();
    let _reset = Reset;
    for (sym, expected) in [(SDLK_UP, "second"), (SDLK_DOWN, "third")] {
        let mut app = fixture();
        app.ott.as_mut().unwrap().selected = Some("third".into());
        tap(&mut app, sym, 0);
        let runtime = app.ott.as_ref().unwrap();
        let requested = runtime
            .play_target
            .as_ref()
            .expect("one next/previous request");
        assert_eq!(requested.id, expected);
        assert!(
            !requested.advance_on_failure,
            "failed direct selection cannot pick another item"
        );
        assert!(runtime.resolver_busy && runtime.resolver_next.is_some());
        assert!(app.inputs.is_empty());
    }
}

#[test]
fn mac_physical_video_key_release_stays_owned_after_hud_and_source_change() {
    let _lock = plx_base::testlock::serial();
    let _reset = Reset;
    let mut app = fixture();
    ingest(&mut app, physical(SDLK_RIGHT, 0, true, false));
    assert_eq!(app.player.volume.desired().level(), 55);
    plx_media::route::forget_external_source(&mut app.player.session);
    reveal_hud(&mut app);
    ingest(&mut app, physical(SDLK_RIGHT, 0, true, true));
    ingest(&mut app, physical(SDLK_RIGHT, 0, false, false));
    assert_eq!(app.player.volume.desired().level(), 55);
    assert_eq!(app.down_sym, 0);
    assert!(
        app.inputs.is_empty(),
        "stale repeat and release cannot reach the scrubber"
    );
}

#[test]
fn mac_physical_video_keeps_hud_and_injected_remote_navigation() {
    let _lock = plx_base::testlock::serial();
    let _reset = Reset;
    let mut app = fixture();
    reveal_hud(&mut app);
    ingest(&mut app, physical(SDLK_RIGHT, 0, true, false));
    assert_eq!(app.player.volume.desired().level(), 50);
    assert!(
        !app.inputs.is_empty(),
        "visible HUD retains normal focus navigation"
    );
    app.inputs.clear();
    bridge::player_mut(&mut app.pages).unwrap().hud.hide(false);
    ingest(&mut app, crate::app::events::encode_key(SDLK_LEFT, 0, true));
    assert_eq!(app.player.volume.desired().level(), 50);
    assert!(
        !app.inputs.is_empty(),
        "SYNTH_WINDOW must keep the remote mapping"
    );
}

#[test]
fn mac_physical_video_wrong_source_and_pin_fail_without_legacy_fallback() {
    let _lock = plx_base::testlock::serial();
    let _reset = Reset;
    for parental in [false, true] {
        let mut app = fixture();
        let runtime = app.ott.as_mut().unwrap();
        if parental {
            runtime.saved.settings.insert("parentPin".into(), json!("pbkdf2-sha256$600000$000102030405060708090a0b0c0d0e0f$ac310cc8527c6c3a4e20d2a2246ebd77d59766dae1c5fe843d65ecf6a7e20c65"));
        } else {
            runtime.saved.profiles.profiles[0].source = "https://example.invalid/other.m3u".into();
        }
        tap(&mut app, SDLK_DOWN, 0);
        let runtime = app.ott.as_ref().unwrap();
        assert!(runtime.play_target.is_none() && runtime.resolver_next.is_none());
        assert!(app.inputs.is_empty());
        assert_eq!(plx_media::player::seek_pending(), -1);
    }
}

#[test]
fn mac_physical_video_strict_kiosk_still_blocks_arrows() {
    let _lock = plx_base::testlock::serial();
    let _reset = Reset;
    let mut app = fixture();
    let runtime = app.ott.as_mut().unwrap();
    // Malformed policy is deliberately fail-closed, just like a strict lock.
    runtime
        .saved
        .settings
        .insert("nativeKiosk".into(), json!(null));
    tap(&mut app, SDLK_RIGHT, 0);
    tap(&mut app, SDLK_RIGHT, 0x0100);
    tap(&mut app, SDLK_DOWN, 0);
    assert_eq!(app.player.volume.desired().level(), 50);
    assert_eq!(plx_media::player::seek_pending(), -1);
    assert!(app.ott.as_ref().unwrap().play_target.is_none());
    assert!(app.inputs.is_empty());
}

#[test]
fn mac_physical_video_recording_refuses_unreplayable_action() {
    let _lock = plx_base::testlock::serial();
    let _reset = Reset;
    let mut app = fixture();
    let initial = crate::app::bootstrap::Initial::synthetic_home(17, 32517, None).unwrap();
    app.rec = crate::app::recorder::Recplay::recording_with_sink(
        &initial,
        Box::new(plx_ui::rec::MemSink::default()),
    )
    .unwrap();
    assert!(app.rec.failure().is_none());
    tap(&mut app, SDLK_RIGHT, 0);
    assert_eq!(app.player.volume.desired().level(), 55);
    assert_eq!(
        app.rec.failure(),
        Some("Physical desktop playback keys are not bound in this recording format")
    );
    assert!(
        app.rec.outcome_failed(),
        "an incomplete tape must not claim successful recording"
    );
}

#[test]
fn mac_physical_video_release_retires_before_early_input_guards() {
    let _lock = plx_base::testlock::serial();
    let _reset = Reset;
    for boundary in ["kiosk", "standby", "wake"] {
        let mut app = fixture();
        ingest(&mut app, physical(SDLK_RIGHT, 0, true, false));
        assert_eq!(app.player.volume.desired().level(), 55);
        match boundary {
            "kiosk" => {
                app.ott
                    .as_mut()
                    .unwrap()
                    .saved
                    .settings
                    .insert("nativeKiosk".into(), json!(null));
            }
            "standby" => app.ott_standby = true,
            "wake" => app.ott_wake_release = true,
            _ => unreachable!(),
        }
        ingest(&mut app, physical(SDLK_RIGHT, 0, false, false));
        app.ott
            .as_mut()
            .unwrap()
            .saved
            .settings
            .remove("nativeKiosk");
        app.ott_standby = false;
        app.ott_wake_release = false;
        reveal_hud(&mut app);
        app.inputs.clear();
        tap(&mut app, SDLK_RIGHT, 0);
        assert_eq!(
            app.inputs.len(),
            2,
            "{boundary}: a new HUD press owns both of its edges"
        );
        assert_eq!(app.player.volume.desired().level(), 55);
    }
}

#[test]
fn mac_physical_video_new_hud_press_retires_lost_physical_release() {
    let _lock = plx_base::testlock::serial();
    let _reset = Reset;
    let mut app = fixture();
    ingest(&mut app, physical(SDLK_RIGHT, 0, true, false));
    reveal_hud(&mut app);
    app.inputs.clear();
    tap(&mut app, SDLK_RIGHT, 0);
    assert_eq!(
        app.inputs.len(),
        2,
        "a fresh HUD press cannot inherit an old physical owner"
    );
}

#[test]
fn mac_physical_video_f_dispatches_current_favorite_once_per_press() {
    let _lock = plx_base::testlock::serial();
    let _reset = Reset;
    let mut app = fixture();
    // The semantic owner must reject an absent admitted item; it must not fall
    // back to the selected browse row or write Saved through this fixture.
    app.ott.as_mut().unwrap().playing = None;
    ingest(&mut app, physical(b'f' as u32, 0, true, false));
    assert_eq!(
        app.ott.as_ref().unwrap().status,
        "The playing source is no longer current"
    );
    app.ott.as_mut().unwrap().status.clear();
    ingest(&mut app, physical(b'f' as u32, 0, true, true));
    ingest(&mut app, physical(b'f' as u32, 0, false, false));
    assert!(
        app.ott.as_ref().unwrap().status.is_empty(),
        "held F must not toggle twice"
    );
    assert!(app.inputs.is_empty(), "the semantic action owns both edges");
}

#[test]
fn mac_physical_video_modified_f_keeps_its_existing_mapping() {
    let _lock = plx_base::testlock::serial();
    let _reset = Reset;
    for modifier in [0x0001, 0x0040, 0x0100, 0x0400] {
        let mut app = fixture();
        app.ott.as_mut().unwrap().playing = None;
        tap(&mut app, b'f' as u32, modifier);
        assert!(app.ott.as_ref().unwrap().status.is_empty());
        assert_eq!(
            app.inputs.len(),
            2,
            "modified F remains on the page input path"
        );
    }
}


#[test]
fn mac_physical_video_m_toggles_once_and_preserves_the_level() {
    let _lock = plx_base::testlock::serial();
    let _reset = Reset;
    let mut app = fixture();
    let before = bridge::player(&app.pages).unwrap().hud.until;
    ingest(&mut app, physical(b'm' as u32, 0, true, false));
    assert!(app.player.volume.desired().muted());
    assert_eq!(app.player.volume.desired().level(), 50);
    ingest(&mut app, physical(b'm' as u32, 0, true, true));
    assert!(app.player.volume.desired().muted(), "repeat must not toggle mute");
    ingest(&mut app, physical(b'm' as u32, 0, false, false));
    tap(&mut app, b'm' as u32, 0);
    assert!(!app.player.volume.desired().muted());
    assert_eq!(app.player.volume.desired().level(), 50);
    assert_eq!(bridge::player(&app.pages).unwrap().hud.until, before);
    assert!(app.inputs.is_empty(), "mute owns both edges without opening the HUD");
}

#[test]
fn mac_physical_video_command_arrows_seek_in_both_directions() {
    let _lock = plx_base::testlock::serial();
    let _reset = Reset;
    for modifier in [0x0400, 0x0800] {
        for (sym, target) in [(SDLK_LEFT, 10_000_000_000), (SDLK_RIGHT, 30_000_000_000)] {
            let mut app = fixture();
            tap(&mut app, sym, modifier);
            assert_eq!(plx_media::player::seek_pending(), target);
            assert_eq!(app.player.volume.desired().level(), 50);
            assert!(app.inputs.is_empty(), "Command seek cannot also enter legacy scrub");
        }
    }
}

#[test]
fn mac_physical_video_command_combinations_and_modified_m_keep_their_owner() {
    let _lock = plx_base::testlock::serial();
    let _reset = Reset;
    for (sym, modifier) in [
        (SDLK_LEFT, 0x0401), (SDLK_RIGHT, 0x0440), (SDLK_RIGHT, 0x0500),
        (b'm' as u32, 0x0001), (b'm' as u32, 0x0040),
        (b'm' as u32, 0x0100), (b'm' as u32, 0x0400),
    ] {
        let mut app = fixture();
        tap(&mut app, sym, modifier);
        assert_eq!(plx_media::player::seek_pending(), -1);
        assert!(!app.player.volume.desired().muted());
        assert_eq!(app.inputs.len(), 2, "unclaimed combination stays on the page input path");
    }
}

#[test]
fn mac_physical_video_m_and_command_arrows_do_not_take_visible_hud_input() {
    let _lock = plx_base::testlock::serial();
    let _reset = Reset;
    for (sym, modifier) in [(b'm' as u32, 0), (SDLK_RIGHT, 0x0400)] {
        let mut app = fixture();
        reveal_hud(&mut app);
        tap(&mut app, sym, modifier);
        assert!(!app.player.volume.desired().muted());
        assert_eq!(plx_media::player::seek_pending(), -1);
        assert_eq!(app.inputs.len(), 2);
    }
}

#[test]
fn mac_physical_video_new_shortcuts_preserve_kiosk_and_release_fences() {
    let _lock = plx_base::testlock::serial();
    let _reset = Reset;
    for (sym, modifier) in [(b'm' as u32, 0), (SDLK_RIGHT, 0x0400)] {
        let mut app = fixture();
        app.ott.as_mut().unwrap().saved.settings.insert("nativeKiosk".into(), json!(null));
        tap(&mut app, sym, modifier);
        assert!(!app.player.volume.desired().muted());
        assert_eq!(plx_media::player::seek_pending(), -1);
        assert!(app.inputs.is_empty());
    }
    let mut app = fixture();
    ingest(&mut app, physical(b'm' as u32, 0, true, false));
    assert!(app.player.volume.desired().muted());
    plx_media::route::forget_external_source(&mut app.player.session);
    reveal_hud(&mut app);
    ingest(&mut app, physical(b'm' as u32, 0, true, true));
    ingest(&mut app, physical(b'm' as u32, 0, false, false));
    assert!(app.player.volume.desired().muted());
    assert_eq!(app.down_sym, 0);
    assert!(app.inputs.is_empty(), "stale repeat/release cannot leak to the HUD");
}


#[test]
fn mac_physical_video_command_seek_refuses_an_unknown_timeline() {
    let _lock = plx_base::testlock::serial();
    let _reset = Reset;
    let mut app = fixture();
    plx_media::player::SHARED.duration_ns.store(0, Relaxed);
    tap(&mut app, SDLK_RIGHT, 0x0400);
    assert_eq!(plx_media::player::seek_pending(), -1);
    assert_eq!(app.ott.as_ref().unwrap().status, "Seeking requires a finite video timeline");
    assert!(app.inputs.is_empty(), "unsupported seek does not fall back to volume or scrub");
}

#[test]
fn mac_physical_volume_uses_accepted_step_preserves_unmute_and_clamps_output() {
    let _lock = plx_base::testlock::serial();
    for step in [3, 7, 10] {
        // Each case leaves a seek queued; retire it before the next independent App.
        let _reset = Reset;
        let mut app = fixture();
        assert_eq!(plx_media::player::seek_pending(), -1, "fresh volumeStep={step} fixture");
        let runtime = app.ott.as_mut().unwrap();
        runtime.saved = runtime.volume_step_candidate(&step.to_string(), 1).unwrap();
        plx_media::player::volume::set_volume(&mut app.adapters.player, &mut app.player.volume,
            plx_media::player::volume::DesiredVolume::new(50, true).unwrap());
        tap(&mut app, SDLK_RIGHT, 0);
        assert_eq!(app.player.volume.desired().level(), 50 + step);
        assert!(!app.player.volume.desired().muted(), "an explicit level key still unmutes");
        tap(&mut app, SDLK_LEFT, 0);
        assert_eq!(app.player.volume.desired().level(), 50);
        assert_eq!(plx_media::player::seek_pending(), -1);
        for (level, key, expected) in [(99, SDLK_RIGHT, 100), (1, SDLK_LEFT, 0)] {
            plx_media::player::volume::set_volume(&mut app.adapters.player, &mut app.player.volume,
                plx_media::player::volume::DesiredVolume::new(level, false).unwrap());
            tap(&mut app, key, 0);
            assert_eq!(app.player.volume.desired().level(), expected);
        }
        // Modifier seek remains ten seconds, independent of volumeStep.
        tap(&mut app, SDLK_RIGHT, 0x0040);
        assert_eq!(plx_media::player::seek_pending(), 30_000_000_000);
        assert_eq!(app.player.volume.desired().level(), 0);
        assert!(app.inputs.is_empty());
    }
}



fn random_key_fixture() -> App {
    let mut app = fixture();
    let runtime = app.ott.as_mut().unwrap();
    runtime.saving = false;
    runtime.entropy = Some([7; 16]);
    runtime.write_permit.revoke();
    runtime.catalog.entries = vec![entry("second")];
    runtime.catalog_complete = true;
    runtime.catalog_loading = false;
    runtime.sync_saved_context();
    runtime.changed();
    assert!(runtime.raw_view().random_context.is_some(), "one available canonical choice");
    app
}

#[test]
fn mac_physical_video_r_plays_a_canonical_choice_once_and_owns_release() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let mut app = random_key_fixture();
    let before = app.ott.as_ref().unwrap().navigation_epoch;
    let saved = serde_json::to_value(&app.ott.as_ref().unwrap().saved).unwrap();
    ingest(&mut app, physical(b'r' as u32, 0, true, false));
    let runtime = app.ott.as_ref().unwrap();
    let target = runtime.play_target.as_ref().expect("R must reach ordinary Play admission");
    assert_eq!(target.id, "second");
    assert!(!target.advance_on_failure);
    assert_eq!(runtime.navigation_epoch, before + 1);
    assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), saved);
    let playback_epoch = runtime.playback_epoch;
    ingest(&mut app, physical(b'r' as u32, 0, true, true));
    ingest(&mut app, physical(b'r' as u32, 0, false, false));
    let runtime = app.ott.as_ref().unwrap();
    assert_eq!(runtime.navigation_epoch, before + 1, "held R must not draw again");
    assert_eq!(runtime.playback_epoch, playback_epoch);
    assert_eq!(app.down_sym, 0);
    assert!(app.inputs.is_empty(), "owned R must not also reach the page");
}

#[test]
fn mac_physical_video_r_retains_modified_hud_and_injected_key_owners() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    for modifier in [0x0001, 0x0040, 0x0100, 0x0400] {
        let mut app = random_key_fixture();
        tap(&mut app, b'r' as u32, modifier);
        assert!(app.ott.as_ref().unwrap().play_target.is_none());
        assert_eq!(app.inputs.len(), 2);
    }
    let mut app = random_key_fixture();
    reveal_hud(&mut app);
    tap(&mut app, b'r' as u32, 0);
    assert!(app.ott.as_ref().unwrap().play_target.is_none());
    assert_eq!(app.inputs.len(), 2);
    let mut app = random_key_fixture();
    ingest(&mut app, events::encode_key(b'r' as u32, 0, true));
    assert!(app.ott.as_ref().unwrap().play_target.is_none());
    assert!(!app.inputs.is_empty(), "injected R keeps the remote mapping");
}

#[test]
fn mac_physical_video_r_keeps_rejected_actions_out_of_legacy_fallback() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    for gate in ["saving", "kiosk", "empty"] {
        let mut app = random_key_fixture();
        let runtime = app.ott.as_mut().unwrap();
        match gate {
            "saving" => runtime.saving = true,
            "kiosk" => { runtime.saved.settings.insert("nativeKiosk".into(), json!(null)); }
            "empty" => runtime.catalog.entries.clear(),
            _ => unreachable!(),
        }
        tap(&mut app, b'r' as u32, 0);
        let runtime = app.ott.as_ref().unwrap();
        assert!(runtime.play_target.is_none() && runtime.resolver_next.is_none(), "{gate}");
        assert!(app.inputs.is_empty(), "{gate}: no fallback after rejected owned action");
    }
}

#[path = "ott_desktop_quit_tests.rs"]
mod quit;

#[path = "ott_desktop_parity_tests.rs"]
mod parity;
