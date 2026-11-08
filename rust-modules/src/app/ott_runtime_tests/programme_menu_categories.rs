//! Baseline-compatible causal regression: use existing More input and shared PlayerPanel.
use super::*;

#[test]
fn classic_more_categories_row_opens_existing_player_panel() {
    let _serial = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    let mut app = catalog_app();
    app.ott.as_mut().unwrap().saved.settings.insert("interfaceMode".into(), json!(0));
    app.bridge.apply_ui_mode(plx_machine::machine::UiMode::Classic);
    super::super::super::super::publish_remote_player_offer(&mut app);
    let identity = route::playback_identity(&app.player.session);
    open_more(&mut app);
    super::super::super::super::publish_remote_player_offer(&mut app);
    idle(&mut app);
    assert!(more_keys(&mut app).contains(&40), "Classic More must contain Categories");
    for _ in 0..24 {
        let overlay = bridge::player_overlay_mut(&mut app.pages).unwrap();
        let Panel::More(menu) = overlay.panel() else { panic!("More"); };
        if menu.keys().get(menu.sel() as usize) == Some(&40) { break; }
        key(&mut app, Key::Down);
    }
    key(&mut app, Key::Ok);
    assert!(ott_surface(&app).is_some());
    assert_eq!(app.ott.as_ref().unwrap().page, "categories");
    assert_eq!(route::playback_identity(&app.player.session), identity);
    assert!(app.ott.as_ref().unwrap().play_target.is_none());
    assert!(app.player.pending_exit.is_none());
}
