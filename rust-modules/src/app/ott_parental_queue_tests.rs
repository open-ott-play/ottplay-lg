//! Legacy queue controls use the exact same final source/PIN gate as native cards.
//! The positive actuator seam observes admission without creating a PMS worker.
use super::*;
use plx_appkit::player_hud::command::HudCommand;
use plx_media::route::QueueStep;

struct MediaReset;
impl Drop for MediaReset {
    fn drop(&mut self) {
        plx_media::route::reset_player_control_for_test(&plx_media::route::PlaybackSession::IDLE);
        plx_media::player::SHARED.reset_session();
        plx_media::player::TX.reset();
    }
}

fn queued() -> (App, plx_plex::plex::ServerId) {
    let (mut app, sid) = fixture();
    plx_media::player::SHARED.reset_session();
    plx_media::player::TX.reset();
    plx_media::route::reset_player_control_for_test(&app.player.session);
    let current = app.ott.as_ref().unwrap().catalog.entries[0].clone();
    for key in ["before", "next", "later"] {
        let mut entry = current.clone();
        entry.id = key.into();
        entry.provider_id = Some(key.into());
        entry.media = MediaLocator::Plex { server_id: "foreign-parental".into(), rating_key: key.into() };
        app.ott.as_mut().unwrap().catalog.entries.push(entry);
    }
    let rows = ["before", "item", "next", "later"].into_iter().enumerate().map(|(i, key)|
        plx_plex::plex::QueueRow { sid, item_id: i as i64 + 10, rk: key.into(), kind: "episode".into(),
            title: "Ordinary presentation label".into(), part: format!("/library/parts/{i}"),
            vcodec: "h264".into(), acodec: "aac".into(), dur_ms: 10_000, ..Default::default() }
    ).collect();
    plx_media::route::install_plex_queue_for_test(&mut app.player.session, sid, "item", rows);
    crate::app::bridge::show_page(&mut app.pages, plx_screens::registry::AppArg::Player);
    for i in 1..=16 {
        crate::app::bridge::frame(&mut app.pages, &mut app.bridge,
            plx_machine::machine::Tick { ms: i * 16, dt_us: 16_000 }, Vec::new());
    }
    assert!(crate::app::bridge::player(&app.pages).is_some());
    (app, sid)
}

fn protect(app: &mut App, key: &str) {
    let entry = app.ott.as_mut().unwrap().catalog.entries.iter_mut().find(|entry|
        matches!(&entry.media, MediaLocator::Plex { rating_key, .. } if rating_key == key)).unwrap();
    entry.protection = Some(true);
}

fn credits() -> plx_data::metadata::Marker {
    plx_data::metadata::Marker { kind: plx_data::metadata::MarkerKind::Credits,
        start_ms: 9_000, end_ms: 10_000, final_seg: true }
}

#[test]
fn queue_final_gate_admits_only_the_exact_ordinary_source_owned_target() {
    let _serial = plx_base::testlock::serial();
    let _registry = Registry::new();
    let _media = MediaReset;
    let (mut app, sid) = queued();
    let identity = plx_media::route::playback_identity(&app.player.session);
    for (direction, expected) in [(Some(QueueStep::Next), "next"),
        (Some(QueueStep::Previous), "before"), (None, "next")] {
        assert_eq!(crate::app::playback::queue_target_for_test(&mut app, direction),
            Some((sid, expected.into())));
        assert_eq!(plx_media::route::playback_identity(&app.player.session), identity);
        assert_eq!(plx_media::route::cur_rk(&app.player.session), "item");
        assert!(!plx_media::route::play_pending());
    }
}

#[test]
fn actual_next_previous_and_hud_next_cannot_trust_an_ordinary_queue_label() {
    let _serial = plx_base::testlock::serial();
    let _registry = Registry::new();
    let _media = MediaReset;
    for command in [HudCommand::Next, HudCommand::Previous] {
        let (mut app, _) = queued();
        protect(&mut app, if command == HudCommand::Next { "next" } else { "before" });
        let identity = plx_media::route::playback_identity(&app.player.session);
        crate::app::playback::player_requests_checked(&mut app,
            vec![plx_screens::registry::PlayerReq::Command(command)], 300);
        assert_eq!(plx_media::route::playback_identity(&app.player.session), identity);
        assert_eq!(plx_media::route::cur_rk(&app.player.session), "item");
        assert!(!plx_media::route::play_pending());
        assert!(!app.player.session.finished);
    }
    let (mut app, _) = queued();
    protect(&mut app, "next");
    let identity = plx_media::route::playback_identity(&app.player.session);
    crate::app::playback::activate_ctrl_row_checked(&mut app,
        plx_appkit::player_hud::ControlSlot::UpNext(credits()), plx_appkit::up_next::BTN_NEXT);
    assert_eq!(plx_media::route::playback_identity(&app.player.session), identity);
    assert_eq!(plx_media::route::cur_rk(&app.player.session), "item");
    assert!(!plx_media::route::play_pending());
    assert!(!app.player.session.finished);
}

#[test]
fn queue_unknown_duplicate_and_rebound_catalogues_are_not_authority() {
    let _serial = plx_base::testlock::serial();
    let _registry = Registry::new();
    let _media = MediaReset;
    for kind in ["unknown", "duplicate", "rebound"] {
        let (mut app, _) = queued();
        let runtime = app.ott.as_mut().unwrap();
        match kind {
            "unknown" => runtime.catalog.entries.retain(|entry| entry.id != "next"),
            "duplicate" => {
                let entry = runtime.catalog.entries.iter().find(|entry| entry.id == "next").unwrap().clone();
                runtime.catalog.entries.push(entry);
            }
            "rebound" => {
                let mut init = app.bridge.snapshot_session_init();
                init.epoch += 1;
                app.bridge = crate::app::bridge::Bridge::for_session_test(init);
            }
            _ => unreachable!(),
        }
        assert_eq!(crate::app::playback::queue_target_for_test(&mut app, Some(QueueStep::Next)), None,
            "{kind} must fail before the actuator");
        assert_eq!(plx_media::route::cur_rk(&app.player.session), "item");
        assert!(!plx_media::route::play_pending());
    }
}

#[test]
fn automatic_up_next_refuses_protected_target_without_skipping_or_opening_pin() {
    let _serial = plx_base::testlock::serial();
    let _registry = Registry::new();
    let _media = MediaReset;
    let (mut app, _) = queued();
    protect(&mut app, "next");
    let identity = plx_media::route::playback_identity(&app.player.session);
    assert!(!crate::app::playback::play_up_next(&mut app,
        plx_screens::player::input::HudStart::Ordinary, false));
    assert_eq!(plx_media::route::playback_identity(&app.player.session), identity);
    assert_eq!(plx_media::route::cur_rk(&app.player.session), "item");
    assert!(!plx_media::route::play_pending());
    assert!(app.ott.as_ref().unwrap().edit_session.is_none());
}

#[test]
fn automatic_eos_cannot_skip_a_protected_successor_to_an_ordinary_later_item() {
    let _serial = plx_base::testlock::serial();
    let _registry = Registry::new();
    let _media = MediaReset;
    let prior = plx_media::route::next_episode_mode();
    struct Mode(plx_media::route::NextEpisodeMode);
    impl Drop for Mode { fn drop(&mut self) { plx_media::route::restore_next_episode_mode(self.0); } }
    let _mode = Mode(prior);
    let (mut app, _) = queued();
    plx_media::route::restore_next_episode_mode(plx_media::route::NextEpisodeMode::AfterCredits);
    assert!(plx_media::route::up_next(&app.player.session).is_some());
    protect(&mut app, "next");
    assert!(!crate::app::playback::finish_playback(&mut app));
    assert!(!plx_media::route::play_pending());
    assert!(!["next", "later"].contains(&plx_media::route::cur_rk(&app.player.session).as_str()));
    assert!(app.ott.as_ref().unwrap().edit_session.is_none());
}

#[test]
fn queue_explicit_disable_is_authoritative_but_missing_runtime_is_not() {
    let _serial = plx_base::testlock::serial();
    let _registry = Registry::new();
    let _media = MediaReset;
    let (mut app, sid) = queued();
    app.ott.as_mut().unwrap().saved.settings.insert("parentPin".into(), json!("*"));
    app.ott.as_mut().unwrap().catalog.entries.clear();
    assert_eq!(crate::app::playback::queue_target_for_test(&mut app, Some(QueueStep::Next)),
        Some((sid, "next".into())));
    app.ott = None;
    assert_eq!(crate::app::playback::queue_target_for_test(&mut app, Some(QueueStep::Next)), None);
    assert_eq!(plx_media::route::cur_rk(&app.player.session), "item");
}

#[test]
fn queued_next_rechecks_pin_expiry_on_the_actual_dispatch_frame() {
    let _serial = plx_base::testlock::serial();
    let _registry = Registry::new();
    let _media = MediaReset;
    let (mut app, sid) = queued();
    protect(&mut app, "next");
    parental_runtime::grant_current_profile_for_test(&mut app);
    assert_eq!(crate::app::playback::queue_target_for_test(&mut app, Some(QueueStep::Next)),
        Some((sid, "next".into())));
    let request = plx_screens::registry::PlayerReq::Command(HudCommand::Next);
    parental_runtime::expire_current_profile_for_test(&mut app);
    let identity = plx_media::route::playback_identity(&app.player.session);
    crate::app::playback::player_requests_checked(&mut app, vec![request], 400);
    assert_eq!(plx_media::route::playback_identity(&app.player.session), identity);
    assert_eq!(plx_media::route::cur_rk(&app.player.session), "item");
    assert!(!plx_media::route::play_pending());
    assert!(!app.player.session.finished);
}

#[test]
fn delayed_disc_activation_and_final_skip_use_the_current_target_gate() {
    let _serial = plx_base::testlock::serial();
    let _registry = Registry::new();
    let _media = MediaReset;
    let prior = plx_media::route::next_episode_mode();
    struct Mode(plx_media::route::NextEpisodeMode);
    impl Drop for Mode { fn drop(&mut self) { plx_media::route::restore_next_episode_mode(self.0); } }
    let _mode = Mode(prior);
    for skip in [false, true] {
        let (mut app, _) = queued();
        plx_media::route::restore_next_episode_mode(plx_media::route::NextEpisodeMode::AfterCredits);
        protect(&mut app, "next");
        let identity = plx_media::route::playback_identity(&app.player.session);
        if skip {
            crate::app::playback::activate_ctrl_row_checked(&mut app,
                plx_appkit::player_hud::ControlSlot::Skip(plx_appkit::skip_pill::Prompt {
                    marker: credits(), kind: plx_data::metadata::MarkerKind::Credits,
                    action: plx_appkit::skip_pill::SkipAction::Finish,
                }), 0);
        } else {
            crate::app::bridge::player_mut(&mut app.pages).unwrap().hud.nav.btn = plx_appkit::player_hud::BTN_NEXT;
            crate::app::playback::activate_player_row_checked(&mut app,
                plx_appkit::player_hud::ControlSlot::Discs(
                    plx_appkit::player_hud::Controls::for_item(true, true, true, false)), 400);
        }
        assert_eq!(plx_media::route::playback_identity(&app.player.session), identity);
        assert_eq!(plx_media::route::cur_rk(&app.player.session), "item");
        assert!(!plx_media::route::play_pending());
        assert!(!app.player.session.finished);
    }
}

#[test]
fn split_field_transport_cannot_start_an_unadmitted_queue_leaf() {
    let _serial = plx_base::testlock::serial();
    let _registry = Registry::new();
    let _media = MediaReset;
    let (mut app, _) = queued();
    protect(&mut app, "next");
    let identity = plx_media::route::playback_identity(&app.player.session);
    crate::app::playback::perform_hud_command(HudCommand::Next, &mut app.player,
        &mut app.adapters.player, &mut app.repause_at, 300, &mut app.refresh_hubs,
        &mut app.pages, &mut app.bridge);
    assert_eq!(plx_media::route::playback_identity(&app.player.session), identity);
    assert_eq!(plx_media::route::cur_rk(&app.player.session), "item");
    assert!(!plx_media::route::play_pending());
}
