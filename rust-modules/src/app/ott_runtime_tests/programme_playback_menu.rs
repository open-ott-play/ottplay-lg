//! Source-bound More admission through the real Player/More fixture.
//! Workers remain held and the writer revoked; no provider or decoder IO is run.
use super::*;
use plx_appkit::more_menu::PlaybackMenuAction as Action;
use plx_screens::registry::{PlayerMenuRequest, PlayerReq};

fn menu_app(live: bool) -> App {
    let mut app = catalog_app();
    app.ott.as_mut().unwrap().saved.settings.insert("interfaceMode".into(), json!(0));
    app.bridge.apply_ui_mode(plx_machine::machine::UiMode::Classic);
    if live {
        let runtime = app.ott.as_mut().unwrap();
        let mut profile = runtime.playing.as_ref().unwrap().profile.clone();
        profile.kind = ProviderKind::M3u;
        profile.source = "https://media.invalid/live.m3u".into();
        runtime.saved.profiles.profiles = vec![profile.clone()];
        runtime.saved.profiles.active_profile = Some(profile.id.clone());
        runtime.provider = profile.kind;
        for entry in &mut runtime.catalog.entries {
            entry.duration_ms = None;
            entry.live = Some(plx_data::ott::guide::ChannelMetadata::default());
        }
        let playing = runtime.playing.as_mut().unwrap();
        playing.profile = profile;
        playing.entry = runtime.catalog.entries[0].clone();
    }
    {
        let runtime = app.ott.as_mut().unwrap();
        runtime.resume_source_work();
        runtime.metadata_done.store(true, Ordering::Release);
        runtime.rearm_metadata_work();
        runtime.metadata_done.store(false, Ordering::Release);
    }
    super::super::super::super::publish_remote_player_offer(&mut app);
    open_more(&mut app);
    super::super::super::super::publish_remote_player_offer(&mut app);
    app
}

fn request(app: &mut App, action: Action) -> PlayerMenuRequest {
    let wanted = match action { Action::Categories => 40, Action::Guide => 41,
        Action::Records => 42, Action::TogglePause => 43, Action::ReloadStream => 44,
        Action::ReturnLive => 45, Action::Chapters => 46, Action::FromBeginning => 47,
        Action::GoToDetail => 48 };
    // Refresh uses the actual publication before the physical row is selected.
    idle(app);
    for _ in 0..24 {
        let overlay = bridge::player_overlay_mut(&mut app.pages).unwrap();
        let Panel::More(menu) = overlay.panel() else { panic!("More"); };
        if menu.keys().get(menu.sel() as usize) == Some(&wanted) { break; }
        key(app, Key::Down);
    }
    {
        let overlay = bridge::player_overlay_mut(&mut app.pages).unwrap();
        let Panel::More(menu) = overlay.panel() else { panic!("More"); };
        assert_eq!(menu.keys().get(menu.sel() as usize), Some(&wanted));
    }
    let tick = next_tick(app);
    bridge::frame(&mut app.pages, &mut app.bridge, tick, bridge::script_key(Key::Ok, tick));
    let mut requests = app.bridge.take_player_reqs();
    // A full dispatcher frame also ticks the open More panel. Keep its one
    // HUD-linger request explicit before checking the sole action from OK.
    assert_eq!(requests.iter().filter(|request|
        matches!(request, PlayerReq::ExtendDefaultHud)).count(), 1);
    requests.retain(|request| !matches!(request, PlayerReq::ExtendDefaultHud));
    assert_eq!(requests.len(), 1);
    let PlayerReq::Menu(request) = requests.remove(0) else { panic!("typed Menu request"); };
    assert!(request.action == action);
    request
}
fn perform(app: &mut App, request: PlayerMenuRequest) -> Result<Option<PlayerReq>, &'static str> {
    super::super::super::super::player_menu_action(app, request)
}
fn still_more(app: &App) {
    assert_eq!(bridge::player_overlay_kind(&app.pages), Some(OverlayKind::More { page: None }));
    assert!(!app.pages.has_pending_surface_navigation());
}

#[test]
fn menu_capabilities_distinguish_live_from_finite_and_keep_installed_source() {
    let _serial = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    for live in [false, true] {
        let mut app = menu_app(live);
        app.ott.as_mut().unwrap().selected = Some("b".into());
        let offer = app.bridge.playback_menu_offer().unwrap();
        assert_eq!(Some(&offer.source), route::playback_identity(&app.player.session).as_ref());
        assert_eq!(offer.actions.guide, live);
        assert_eq!(offer.actions.toggle_pause, !live);
        assert!(!offer.actions.records && !offer.actions.return_live);
        assert!(offer.actions.reload_stream);
        let source = route::external_source(&app.player.session);
        let mut request = request(&mut app, if live { Action::Guide } else { Action::TogglePause });
        if live { request.action = Action::TogglePause; } // unavailable forged action cannot borrow Guide
        let result = perform(&mut app, request);
        if live {
            assert!(result.is_err());
            still_more(&app);
        } else {
            assert!(matches!(result, Ok(Some(PlayerReq::Transport(None)))));
            assert!(app.pages.has_pending_surface_navigation());
        }
        assert_eq!(route::external_source(&app.player.session), source);
        assert!(app.ott.as_ref().unwrap().play_target.is_none());
    }
}

#[test]
fn menu_guide_uses_playing_channel_not_browse_highlight() {
    let _serial = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    let mut app = menu_app(true);
    app.ott.as_mut().unwrap().selected = Some("b".into());
    let identity = route::playback_identity(&app.player.session);
    let request = request(&mut app, Action::Guide);
    assert!(matches!(perform(&mut app, request), Ok(None)));
    let runtime = app.ott.as_ref().unwrap();
    assert!(runtime.guide.is_some());
    assert_eq!(runtime.metadata_target.as_ref().unwrap().1, "c");
    let (_, MetadataSelection::Entry(entry)) = runtime.metadata_next.as_ref().unwrap() else { panic!("guide entry"); };
    assert_eq!(entry.id, "c");
    assert_eq!(runtime.page, "guide");
    assert!(runtime.play_target.is_none());
    assert!(!runtime.metadata_done.load(Ordering::Acquire));
    assert_eq!(route::playback_identity(&app.player.session), identity);
    assert!(app.pages.has_pending_surface_navigation());
}

#[test]
fn menu_rejects_rebound_channel_even_when_id_and_playback_are_unchanged() {
    let _serial = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    let mut app = menu_app(true);
    let request = request(&mut app, Action::Guide);
    let generation = app.ott.as_ref().unwrap().metadata_generation;
    app.ott.as_mut().unwrap().catalog.entries[0].media = MediaLocator::Http {
        url: "https://media.invalid/replaced-channel.ts".into() };
    assert!(perform(&mut app, request).is_err());
    still_more(&app);
    let runtime = app.ott.as_ref().unwrap();
    assert_eq!(runtime.metadata_generation, generation);
    assert!(runtime.guide.is_none() && runtime.play_target.is_none());
}

#[test]
fn menu_rejects_foreign_owner_pending_surface_and_stale_source() {
    let _serial = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    for case in 0..3 {
        let mut app = menu_app(false);
        let mut request = request(&mut app, Action::TogglePause);
        match case {
            0 => request.owner = plx_machine::machine::InstanceId(request.owner.0 + 1000),
            1 => app.pages.request(plx_machine::machine::MachineId::Nav,
                plx_machine::machine::NavOp::Present(AppArg::Settings(
                    plx_screens::family::SettingsPage::Root))),
            _ => {
                app.ott.as_mut().unwrap().playing.as_mut().unwrap().identity = None;
            }
        }
        assert!(perform(&mut app, request).is_err());
        assert_eq!(bridge::player_overlay_kind(&app.pages), Some(OverlayKind::More { page: None }));
        assert!(app.ott.as_ref().unwrap().play_target.is_none());
        assert!(app.player.pending_exit.is_none());
    }
}

#[test]
fn menu_retains_more_when_metadata_or_malformed_policy_refuses_without_cancelling_playback() {
    let _serial = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    for policy in [false, true] {
        let mut app = menu_app(true);
        let request = request(&mut app, Action::Guide);
        if policy {
            app.ott.as_mut().unwrap().saved.settings.insert("nativeKiosk".into(), "2".into());
        } else {
            let mut runtime = app.ott.take().unwrap();
            runtime.suspend_source_work(&mut app);
            app.ott = Some(runtime);
        }
        let identity = route::playback_identity(&app.player.session);
        assert!(perform(&mut app, request).is_err());
        still_more(&app);
        assert_eq!(route::playback_identity(&app.player.session), identity);
        assert!(app.ott.as_ref().unwrap().play_target.is_none());
    }
}

#[test]
fn menu_return_live_keeps_exact_channel_and_uses_ordinary_guarded_request() {
    let _serial = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    let mut app = menu_app(true);
    {
        use plx_data::ott::archive::{ArchiveInterval, CatchupMode, CatchupSource};
        let runtime = app.ott.as_mut().unwrap();
        runtime.wall_clock_ms = 1_700_000_000_000;
        let playing = runtime.playing.as_mut().unwrap();
        playing.entry.live.as_mut().unwrap().catchup = Some(CatchupSource {
            mode: CatchupMode::Append, source: "?start={utc}&duration={duration}".into(),
            retention_seconds: 86400, correction_seconds: 0 });
        runtime.catalog.entries[0] = playing.entry.clone();
        playing.archive = Some(archive_play::Intent::new(&playing.profile, &playing.entry,
            ArchiveInterval { start: 1_699_999_400, end: 1_699_999_700 }, 1_700_000_000).unwrap());
        runtime.selected = Some("b".into());
    }
    super::super::super::super::publish_remote_player_offer(&mut app);
    let request = request(&mut app, Action::ReturnLive);
    assert!(request.offer.actions.return_live && request.offer.actions.toggle_pause);
    let source = route::playback_identity(&app.player.session);
    assert!(matches!(perform(&mut app, request), Ok(None)));
    let runtime = app.ott.as_ref().unwrap();
    let target = runtime.play_target.as_ref().unwrap();
    assert_eq!(target.id, "c");
    assert_eq!(target.position_ms, 0);
    assert!(target.archive.is_none() && !target.advance_on_failure);
    assert_eq!(route::playback_identity(&app.player.session), source,
        "the held resolver has not replaced the installed archive");
}

#[test]
fn menu_categories_reuses_player_panel_without_replacing_playback() {
    let _serial = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    let mut app = menu_app(false);
    let source = route::playback_identity(&app.player.session);
    let saved = serde_json::to_value(&app.ott.as_ref().unwrap().saved).unwrap();
    let request = request(&mut app, Action::Categories);
    assert!(request.offer.actions.categories);
    assert!(matches!(perform(&mut app, request), Ok(None)));
    idle(&mut app);
    assert!(ott_surface(&app).is_some());
    assert_eq!(app.ott.as_ref().unwrap().page, "categories");
    assert_eq!(route::playback_identity(&app.player.session), source);
    assert_eq!(serde_json::to_value(&app.ott.as_ref().unwrap().saved).unwrap(), saved);
    assert!(app.ott.as_ref().unwrap().play_target.is_none());
}

#[test]
fn menu_guide_allows_display_enrichment_of_the_same_bound_channel() {
    let _serial = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    let mut app = menu_app(true);
    let request = request(&mut app, Action::Guide);
    let row = &mut app.ott.as_mut().unwrap().catalog.entries[0];
    row.title = "Refreshed title".into();
    row.description = "Refreshed synopsis".into();
    row.poster = "https://media.invalid/refreshed-art.png".into();
    assert!(matches!(perform(&mut app, request), Ok(None)));
    let runtime = app.ott.as_ref().unwrap();
    let (_, MetadataSelection::Entry(entry)) = runtime.metadata_next.as_ref().unwrap() else { panic!("guide entry"); };
    assert_eq!(entry.id, "c");
    assert_eq!(entry.title, "Refreshed title");
    assert!(runtime.play_target.is_none());
}

#[test]
fn menu_real_guide_row_reports_refusal_in_more_after_source_binding_changes() {
    let _serial = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    let mut app = menu_app(true);
    for _ in 0..24 {
        let overlay = bridge::player_overlay_mut(&mut app.pages).unwrap();
        let Panel::More(menu) = overlay.panel() else { panic!("More"); };
        if menu.keys().get(menu.sel() as usize) == Some(&41) { break; }
        key(&mut app, Key::Down);
    }
    {
        let overlay = bridge::player_overlay_mut(&mut app.pages).unwrap();
        let Panel::More(menu) = overlay.panel() else { panic!("More"); };
        assert_eq!(menu.keys().get(menu.sel() as usize), Some(&41));
    }
    app.ott.as_mut().unwrap().catalog.entries[0].media = MediaLocator::Http {
        url: "https://media.invalid/replaced-channel.ts".into() };
    key(&mut app, Key::Ok);
    still_more(&app);
    let overlay = bridge::player_overlay_mut(&mut app.pages).unwrap();
    let Panel::More(menu) = overlay.panel() else { panic!("More"); };
    assert_eq!(menu.playback_failure_note(), Some("This action is no longer available for the playing source"));
    assert!(app.ott.as_ref().unwrap().guide.is_none());
    assert!(app.ott.as_ref().unwrap().play_target.is_none());
}
