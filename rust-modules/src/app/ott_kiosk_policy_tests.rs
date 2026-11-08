use super::*;

#[test]
fn remote_track_selection_is_allowed_when_kiosk_is_off() {
    assert!(remote_track_selection_allowed(State::Off));
}

#[test]
fn remote_track_selection_is_denied_for_every_enabled_kiosk_state() {
    for strict in [false, true] {
        assert!(!remote_track_selection_allowed(State::Armed { strict }));
        assert!(!remote_track_selection_allowed(State::Locked { strict }));
    }
}
