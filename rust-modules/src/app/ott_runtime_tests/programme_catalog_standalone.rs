//! Additive coverage of strict commits without any OTT catalog surface.
use super::*;

fn standalone_player() -> App {
    // Begin without ever presenting a catalog. The same real paused-player
    // and soft-pin setup also underlies pinned_catalog's tested HUD entry.
    let app = pinned_player();
    assert!(!bridge::ott_visible(&app.pages));
    assert!(!app.pages.surface_up());
    assert!(lifecycle::viewer_paused());
    app
}

fn settings_alone() -> App {
    let mut app = standalone_player();
    open_more(&mut app);
    key(&mut app, Key::Back);
    finish_strict_dismissal(&mut app);
    // More has no Settings row. Use the existing shared Settings presenter,
    // also called by the ordinary LoopReq::AccountSettings ingress.
    assert!(plx_plex::plex::account::PreferenceRequest::capture().is_none(),
        "generic Settings fixture must have no account request capable of IO");
    bridge::open_settings(&mut app.pages);
    idle(&mut app);
    assert!(bridge::settings_up(&app.pages));
    assert!(!bridge::ott_visible(&app.pages));
    assert!(!bridge::player_overlay_up(&app.pages));
    app
}

fn flush_dismissals(app: &mut App) {
    frame(app, Vec::new());
    for _ in 0..200 {
        if !app.pages.has_pending_navigation() && !app.pages.nav.modals.surfaces.iter()
            .any(|surface| surface.phase == Phase::Closing) { return; }
        frame(app, Vec::new());
    }
    panic!("dismissals did not complete through the real modal lifecycle");
}

#[test]
fn strict_saved_closes_standalone_settings_without_changing_paused_player() {
    let _serial = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    let _probe = PlacementProbe;
    let mut app = settings_alone();
    let identity = route::playback_identity(&app.player.session);
    let page = app.pages.top_page();
    let origin = bridge::player(&app.pages).unwrap().origin;
    let epoch = app.ott.as_ref().unwrap().playback_epoch;
    let pending = request_strict(&mut app, 60_000);
    assert!(bridge::settings_up(&app.pages), "admission is not durable publication");
    complete(&mut app, pending, "saved");
    flush_dismissals(&mut app);
    assert!(!bridge::settings_up(&app.pages));
    assert!(!app.pages.surface_up());
    assert_eq!(bridge::player(&app.pages).unwrap().origin, origin);
    assert_same_source(&mut app, &identity, page, epoch);
}

#[test]
fn strict_saved_closes_standalone_more_without_changing_paused_player() {
    let _serial = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    let _probe = PlacementProbe;
    let mut app = standalone_player();
    open_more(&mut app);
    let identity = route::playback_identity(&app.player.session);
    let page = app.pages.top_page();
    let epoch = app.ott.as_ref().unwrap().playback_epoch;
    let pending = request_strict(&mut app, 60_000);
    complete(&mut app, pending, "saved");
    flush_dismissals(&mut app);
    assert!(!app.pages.surface_up());
    assert_same_source(&mut app, &identity, page, epoch);
}

#[test]
fn strict_saved_keeps_info_while_dismissing_covering_more() {
    let _serial = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    let _probe = PlacementProbe;
    let mut app = standalone_player();
    bridge::open_player_overlay(&app.player.session, app.bridge.metadata_view(),
        &mut app.pages, OverlayKind::Info);
    idle(&mut app);
    let info_owner = input_owner(&app);
    bridge::open_player_overlay(&app.player.session, app.bridge.metadata_view(),
        &mut app.pages, OverlayKind::More { page: None });
    idle(&mut app);
    assert_ne!(input_owner(&app), info_owner);
    let identity = route::playback_identity(&app.player.session);
    let page = app.pages.top_page();
    let epoch = app.ott.as_ref().unwrap().playback_epoch;
    let pending = request_strict(&mut app, 60_000);
    complete(&mut app, pending, "saved");
    flush_dismissals(&mut app);
    assert_eq!(bridge::player_overlay_kind(&app.pages), Some(OverlayKind::Info));
    assert_eq!(input_owner(&app), info_owner, "permitted Info retains its actual instance");
    assert_eq!(app.pages.nav.modals.surfaces.iter().filter(|s| s.phase != Phase::Hidden).count(), 1);
    assert_same_source(&mut app, &identity, page, epoch);
}

#[test]
fn strict_saved_keeps_standalone_info_without_dismissal() {
    let _serial = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    let _probe = PlacementProbe;
    let mut app = standalone_player();
    bridge::open_player_overlay(&app.player.session, app.bridge.metadata_view(),
        &mut app.pages, OverlayKind::Info);
    idle(&mut app);
    let owner = input_owner(&app);
    let pending = request_strict(&mut app, 60_000);
    complete(&mut app, pending, "saved");
    assert!(!app.pages.has_pending_navigation());
    idle(&mut app);
    assert_eq!(input_owner(&app), owner);
    assert_eq!(bridge::player_overlay_kind(&app.pages), Some(OverlayKind::Info));
}

#[test]
fn strict_failed_save_keeps_standalone_settings_and_paused_player() {
    let _serial = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    let _probe = PlacementProbe;
    let mut app = settings_alone();
    let owner = input_owner(&app);
    let identity = route::playback_identity(&app.player.session);
    let page = app.pages.top_page();
    let epoch = app.ott.as_ref().unwrap().playback_epoch;
    let pending = request_strict(&mut app, 60_000);
    complete(&mut app, pending, "failed");
    idle(&mut app);
    assert!(bridge::settings_up(&app.pages));
    assert_eq!(input_owner(&app), owner);
    assert_same_source(&mut app, &identity, page, epoch);
}

#[test]
fn strict_uncertain_save_keeps_standalone_settings_presentation() {
    let _serial = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    let _probe = PlacementProbe;
    let mut app = settings_alone();
    let owner = input_owner(&app);
    let pending = request_strict(&mut app, 60_000);
    complete(&mut app, pending, "uncertain");
    assert!(app.ott.as_ref().unwrap().persistence_uncertain);
    assert!(bridge::settings_up(&app.pages));
    assert_eq!(input_owner(&app), owner);
    assert!(!app.pages.has_pending_navigation());
}
