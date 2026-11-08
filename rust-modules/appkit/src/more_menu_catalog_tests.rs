use super::*;

#[test]
fn catalog_offer_is_opt_in_and_keeps_a_stable_action_key() {
    let _lock = plx_base::testlock::serial();
    let ps = plx_media::route::PlaybackSession::default();
    let mut menu = MoreMenuState::new(&ps);
    let key = Action::OpenCatalog.key().0;
    assert_eq!(key, 5);
    assert!(!menu.keys().contains(&key));
    assert!(menu.set_catalog_available(&ps, true));
    assert!(menu.keys().contains(&key));
    assert!(menu.focus_key(key));
    assert_eq!(menu.on_ok(&ps), MoreOk::Action(Action::OpenCatalog));
    assert!(!menu.set_catalog_available(&ps, true));
    assert!(menu.set_catalog_available(&ps, false));
    assert!(!menu.keys().contains(&key));
    assert!(!menu.focus_key(key));
    assert_ne!(menu.on_ok(&ps), MoreOk::Action(Action::OpenCatalog));
}

#[test]
fn catalog_offer_refresh_preserves_an_existing_options_focus() {
    let _lock = plx_base::testlock::serial();
    let ps = plx_media::route::PlaybackSession::default();
    let mut menu = MoreMenuState::new(&ps);
    let stats = MoreRow::Act(Action::ToggleStats);
    assert!(menu.focus_key(stats.key().0));
    let before = menu.keys();
    menu.set_catalog_available(&ps, true);
    assert_eq!(menu.sel_id(), Some(stats));
    for key in &before { assert!(menu.keys().contains(key)); }
    menu.set_catalog_available(&ps, false);
    assert_eq!(menu.keys(), before);
    assert_eq!(menu.sel_id(), Some(stats));
}
