//! Locale refresh is presentation only, including while a key-repeat detent is active.
use super::*;
use plx_platform::i18n::{language_on_this_thread_for_test, Preference};

#[test]
fn relabel_retains_offset_limits_repeat_and_detent() {
    let _lock = plx_base::testlock::serial();
    let _en = language_on_this_thread_for_test(Preference::En);
    let mut cap = TimingCapsule::new(-100, -60_000, 60_000);
    assert_eq!(cap.key(Key::Right { alt: false }, Edge::Repeat, 110), Some(CapsuleOut::Step(0)));
    let state = (cap.offset_ms, cap.lo, cap.hi, cap.repeats, cap.detent_until, cap.shake.pos, cap.shake.vel);
    let old = cap.text().to_string();
    let _ru = language_on_this_thread_for_test(Preference::Ru);
    cap.relabel();
    assert_ne!(cap.text(), old);
    assert_eq!(cap.text(), text_in(0, plx_platform::i18n::current()));
    assert_eq!((cap.offset_ms, cap.lo, cap.hi, cap.repeats, cap.detent_until, cap.shake.pos, cap.shake.vel), state);
    assert_eq!(cap.key(Key::Right { alt: false }, Edge::Repeat, 300), None);
    assert_eq!(cap.key(Key::Right { alt: false }, Edge::Down, 350), Some(CapsuleOut::Step(100)));
}
