//! Real More row requests; existing inert engine/source fixtures, no worker IO.
use super::*;
use plx_appkit::more_menu::PlaybackMenuAction as Action;
use plx_screens::registry::{PlayerMenuRequest, PlayerReq};

fn app() -> App {
    let (mut app, _) = native_catalog_app();
    player::SHARED.duration_ns.store(60_000_000_000, Ordering::Release);
    open_more(&mut app);
    publish(&mut app);
    app
}
fn publish(app: &mut App) {
    crate::app::ott_runtime::publish_remote_player_offer(app);
    idle(app);
}
fn select(app: &mut App, action: Action) {
    let wanted = match action {
        Action::Chapters => 46, Action::FromBeginning => 47, Action::GoToDetail => 48,
        _ => panic!("accessibility action"),
    };
    publish(app);
    for _ in 0..24 {
        let overlay = bridge::player_overlay_mut(&mut app.pages).unwrap();
        let Panel::More(menu) = overlay.panel() else { panic!("More"); };
        if menu.keys().get(menu.sel() as usize) == Some(&wanted) { break; }
        key(app, Key::Down);
    }
    let overlay = bridge::player_overlay_mut(&mut app.pages).unwrap();
    let Panel::More(menu) = overlay.panel() else { panic!("More"); };
    assert_eq!(menu.keys().get(menu.sel() as usize), Some(&wanted));
}
fn request(app: &mut App, action: Action) -> PlayerMenuRequest {
    select(app, action);
    let tick = next_tick(app);
    bridge::frame(&mut app.pages, &mut app.bridge, tick, bridge::script_key(Key::Ok, tick));
    // This dispatch also ticks the open panel, which requests its ordinary HUD
    // refresh. Count that exact presentation effect separately from the action.
    let (hud, mut requests): (Vec<_>, Vec<_>) = app.bridge.take_player_reqs()
        .into_iter().partition(|request| matches!(request, PlayerReq::ExtendDefaultHud));
    assert_eq!(hud.len(), 1, "one ordinary panel Tick refresh");
    assert_eq!(requests.len(), 1);
    let PlayerReq::Menu(request) = requests.remove(0) else { panic!("Menu request"); };
    assert!(request.action == action);
    request
}
fn perform(app: &mut App, request: PlayerMenuRequest) -> Result<Option<PlayerReq>, &'static str> {
    crate::app::ott_runtime::player_menu_action(app, request)
}
fn install_leaf(app: &mut App, kind: &str, show: &str, chapters: bool) {
    let mut leaf = app.bridge.metadata_view().playing().unwrap().clone();
    leaf.kind = kind.into(); leaf.show_rk = show.into();
    leaf.chapters = if chapters { vec![plx_data::metadata::Chapter {
        index: 1, start_ms: 3000, title: "Chapter one".into(), thumb: String::new(),
    }] } else { vec![] };
    assert!(app.bridge.metadata_run(plx_data::stores::metadata::MetadataCmd::InstallPlaying(Some(leaf))));
    publish(app);
}
fn retained(app: &App) {
    assert_eq!(bridge::player_overlay_kind(&app.pages), Some(OverlayKind::More { page: None }));
    assert!(!app.pages.has_pending_surface_navigation());
    let runtime = app.ott.as_ref().unwrap();
    assert!(runtime.playing.is_none() && runtime.play_target.is_none()
        && runtime.resolver_next.is_none() && runtime.catalog_next.is_none());
}

#[test]
fn chapters_use_installed_leaf_and_existing_overlay_request() {
    let _serial = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    let _registry = NativeCatalogRegistry;
    let mut app = app();
    install_leaf(&mut app, "episode", "show-42", true);
    let sid = route::cur_sid(&app.player.session);
    land_plex_detail(&mut app, sid, "other", "movie", "Unrelated browsing");
    let identity = route::playback_identity(&app.player.session);
    select(&mut app, Action::Chapters);
    key(&mut app, Key::Ok); // Full bridge -> content -> existing overlay performer.
    assert_eq!(bridge::player_overlay_kind(&app.pages), Some(OverlayKind::Chapters));
    assert_eq!(route::playback_identity(&app.player.session), identity);
    assert_eq!(app.bridge.metadata_view().playing_chapters()[0].start_ms, 3000);
    assert!(app.ott.as_ref().unwrap().playing.is_none());
}

#[test]
fn from_beginning_uses_existing_info_executor_without_adopting_native_playback() {
    let _serial = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    let _registry = NativeCatalogRegistry;
    let mut app = app();
    let identity = route::playback_identity(&app.player.session);
    select(&mut app, Action::FromBeginning);
    player::TX.seek_to_ns.store(-1, Ordering::Release);
    let seeks = player::TX.seek_reqs.load(Ordering::Acquire);
    key(&mut app, Key::Ok); // Full bridge -> content -> existing Info performer.
    assert_eq!(player::seek_pending(), 0);
    assert_eq!(player::TX.seek_reqs.load(Ordering::Acquire), seeks.wrapping_add(1));
    assert!(bridge::player_overlay_kind(&app.pages).is_none());
    // Admission has happened, but the inert fixture has not completed a seek.
    assert_eq!(player::SHARED.playpos_ns.load(Ordering::Acquire), 7_000_000_000);
    assert_eq!(route::playback_identity(&app.player.session), identity);
    assert!(app.ott.as_ref().unwrap().playing.is_none());
    player::abandon_seek();
    player::TX.seek_to_ns.store(-1, Ordering::Release);
}

#[test]
fn detail_target_comes_from_exact_installed_movie_or_episode_not_current_browse() {
    let _serial = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    let _registry = NativeCatalogRegistry;
    for (kind, show, expected) in [("movie", "", "42"), ("episode", "show-42", "show-42")] {
        let mut app = app();
        install_leaf(&mut app, kind, show, false);
        let sid = route::cur_sid(&app.player.session);
        land_plex_detail(&mut app, sid, "unrelated", "movie", "Unrelated browsing");
        let req = request(&mut app, Action::GoToDetail);
        assert!(matches!(perform(&mut app, req), Ok(Some(PlayerReq::Info(
            plx_appkit::info_panel::InfoAction::GoToDetail(rk)))) if rk == expected));
        assert_eq!(route::cur_sid(&app.player.session), sid);
        assert!(app.ott.as_ref().unwrap().catalog_next.is_none());
    }
}

#[test]
fn accessibility_capabilities_refuse_missing_metadata_live_unknown_duration_and_modern() {
    let _serial = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    let _registry = NativeCatalogRegistry;
    let mut app = app();
    install_leaf(&mut app, "episode", "", false);
    assert!(!app.bridge.playback_menu_offer().unwrap().actions.go_to_detail);
    let mut foreign = app.bridge.metadata_view().playing().unwrap().clone();
    foreign.rk = "foreign".into();
    foreign.chapters = vec![plx_data::metadata::Chapter {
        index: 1, start_ms: 0, title: "Foreign".into(), thumb: String::new(),
    }];
    app.bridge.metadata_run(plx_data::stores::metadata::MetadataCmd::InstallPlaying(Some(foreign)));
    publish(&mut app);
    let caps = app.bridge.playback_menu_offer().unwrap().actions;
    assert!(!caps.chapters && !caps.go_to_detail);
    player::SHARED.duration_ns.store(0, Ordering::Release);
    publish(&mut app);
    assert!(!app.bridge.playback_menu_offer().unwrap().actions.from_beginning);
    app.bridge.apply_ui_mode(plx_machine::machine::UiMode::Modern);
    publish(&mut app);
    assert!(app.bridge.playback_menu_offer().is_none());

    let mut app = catalog_app();
    let runtime = app.ott.as_mut().unwrap();
    runtime.saved.settings.insert("interfaceMode".into(), json!(0));
    app.bridge.apply_ui_mode(plx_machine::machine::UiMode::Classic);
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
    player::SHARED.duration_ns.store(60_000_000_000, Ordering::Release);
    publish(&mut app);
    assert!(!app.bridge.playback_menu_offer().unwrap().actions.from_beginning,
        "a positive decoder duration cannot turn ordinary live into finite media");
}

#[test]
fn accessibility_rechecks_current_source_owner_and_pending_surface_before_effect() {
    let _serial = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    let _registry = NativeCatalogRegistry;
    for case in 0..3 {
        let mut app = app();
        let mut req = request(&mut app, Action::FromBeginning);
        match case {
            0 => req.owner = plx_machine::machine::InstanceId(req.owner.0 + 1000),
            1 => bridge::open_settings(&mut app.pages),
            _ => {
                let sid = route::cur_sid(&app.player.session);
                route::install_plex_playback_for_test(&mut app.player.session, sid, "42");
            }
        }
        assert!(perform(&mut app, req).is_err());
        assert_eq!(player::SHARED.playpos_ns.load(Ordering::Acquire), 7_000_000_000);
        assert!(app.ott.as_ref().unwrap().playing.is_none());
        if case != 1 { retained(&app); }
    }
}

#[test]
fn protected_detail_does_not_borrow_unrelated_saved_profile_or_load_catalogue() {
    let _serial = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    let _registry = NativeCatalogRegistry;
    let mut app = app();
    let req = request(&mut app, Action::GoToDetail);
    let unrelated = synthetic_profile();
    let runtime = app.ott.as_mut().unwrap();
    runtime.saved.profiles.profiles = vec![unrelated.clone()];
    runtime.saved.profiles.active_profile = Some(unrelated.id.clone());
    runtime.provider = unrelated.kind;
    runtime.saved.settings.insert("parentPin".into(), json!(
        "pbkdf2-sha256$600000$000102030405060708090a0b0c0d0e0f$ac310cc8527c6c3a4e20d2a2246ebd77d59766dae1c5fe843d65ecf6a7e20c65"));
    let saved = serde_json::to_value(&runtime.saved).unwrap();
    let identity = route::playback_identity(&app.player.session);
    assert!(perform(&mut app, req).is_err());
    retained(&app);
    assert_eq!(route::playback_identity(&app.player.session), identity);
    assert_eq!(serde_json::to_value(&app.ott.as_ref().unwrap().saved).unwrap(), saved);
}
