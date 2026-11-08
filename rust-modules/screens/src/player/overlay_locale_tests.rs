//! Exercise the real pure screen hook while an addressed More action is pending.
use super::*;

#[test]
fn locale_hook_keeps_the_exact_pending_menu_receipt_and_source() {
    let _serial = plx_base::testlock::serial();
    let _en = plx_platform::i18n::language_on_this_thread_for_test(plx_platform::i18n::Preference::En);
    let mut f = Fixture::new();
    let request = f.request();
    let mut before = Canon::new(); f.page.write(&mut before);
    let before = before.finish();
    let _ru = plx_platform::i18n::language_on_this_thread_for_test(plx_platform::i18n::Preference::Ru);
    <PlayerOverlayScreen as Screen<TestHost>>::locale_changed(&mut f.page, 9);
    let mut after = Canon::new(); f.page.write(&mut after);
    assert_eq!(after.finish(), before);
    let pending = f.page.menu_pending.as_ref().unwrap();
    assert_eq!(pending.id, request.id);
    assert_eq!(pending.owner, request.owner);
    assert_eq!(pending.offer, request.offer);
    assert_eq!(pending.action, request.action);
    assert!(f.step(ScreenEvent::Activate(40), ENTRY).is_empty());
    f.step(ScreenEvent::Async(request.id, AppMsg::PlayerMenuResult { request, outcome: Err("still addressed") }), ENTRY);
    assert_eq!(f.note(), Some("still addressed"));
    assert!(f.page.menu_pending.is_none());
}
