//! Source-offer gate through the existing keyed menu API; no application navigation.
use super::*;

#[test]
fn settings_row_requires_an_offer_and_retires_with_it() {
    let _lock = plx_base::testlock::serial();
    let ps = plx_media::route::PlaybackSession::IDLE;
    let mut menu = MoreMenuState::new(&ps);
    assert!(!menu.focus_key(36));
    assert!(menu.set_catalog_available(&ps, true));
    assert!(menu.focus_key(36), "an admitted source offers Settings");
    let MoreOk::Action(action) = menu.on_ok(&ps) else { panic!("Settings is an action"); };
    assert_eq!(action.key().0, 36);
    assert_eq!(label(action), plx_platform::i18n::msg::settings_title());
    assert!(menu.set_catalog_available(&ps, false));
    assert!(!menu.focus_key(36), "stale Settings keys cannot retarget another row");
}
