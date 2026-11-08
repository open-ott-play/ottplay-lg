//! Presentation regressions: no player, worker or clock timer is started.
use super::*;
use plx_appkit::player_hud::model::{OsdOpacity, PermanentClock};
use plx_machine::machine::UiMode;
use plx_media::route::PlaybackSession;

fn hidden_clock() -> PlayerScreen {
    let mut page = PlayerScreen::new(EntryId(1));
    page.hud.hide(false);
    page.set_osd_uncovered(true);
    page.model.paused = true;
    page.model.permanent_clock = PermanentClock::Background;
    page.model.wall_now_s = Some(600);
    page
}

#[test]
fn permanent_clock_advances_by_displayed_minute_with_hidden_paused_hud_in_both_modes() {
    let _lock = plx_base::testlock::serial();
    for mode in [UiMode::Classic, UiMode::Modern] {
        let mut page = hidden_clock();
        let ps = PlaybackSession::IDLE;
        assert!(!page.hud_drawn(&ps, 1_000));
        let before = page.clock_fingerprint_in_mode(&ps, 1_000, mode, None);
        page.model.wall_now_s = Some(659);
        assert_eq!(before, page.clock_fingerprint_in_mode(&ps, 1_000, mode, None));
        page.model.wall_now_s = Some(660);
        assert_ne!(before, page.clock_fingerprint_in_mode(&ps, 1_000, mode, None));
        assert!(page.hud.dismissed, "the clock must not pin or reveal transport");
    }
}

#[test]
fn permanent_clock_is_absent_under_other_surfaces_and_restores_latest_time() {
    let _lock = plx_base::testlock::serial();
    let ps = PlaybackSession::IDLE;
    let mut page = hidden_clock();
    let visible = page.permanent_clock_key().unwrap();
    page.set_osd_uncovered(false);
    assert!(page.permanent_clock_key().is_none());
    let before = page.clock_fingerprint_in_mode(&ps, 1_000, UiMode::Modern, None);
    page.model.wall_now_s = Some(720);
    assert_eq!(before, page.clock_fingerprint_in_mode(&ps, 1_000, UiMode::Modern, None));
    page.set_osd_uncovered(true);
    assert_ne!(visible, page.permanent_clock_key().unwrap());
    for policy in [HudPolicy::Lifted, HudPolicy::Hidden] {
        page.set_hud_policy(policy);
        assert!(page.permanent_clock_key().is_none());
    }
    page.set_hud_policy(HudPolicy::Normal);
    assert!(page.permanent_clock_key().is_some());
}

#[test]
fn disabled_or_missing_clock_owes_no_frames_for_wall_time() {
    let _lock = plx_base::testlock::serial();
    let ps = PlaybackSession::IDLE;
    let mut page = hidden_clock();
    page.model.permanent_clock = PermanentClock::Off;
    let before = page.clock_fingerprint_in_mode(&ps, 1_000, UiMode::Modern, None);
    page.model.wall_now_s = Some(720);
    assert_eq!(before, page.clock_fingerprint_in_mode(&ps, 1_000, UiMode::Modern, None));
    page.model.permanent_clock = PermanentClock::Transparent;
    assert!(page.permanent_clock_key().is_some());
    page.model.wall_now_s = None;
    assert!(page.permanent_clock_key().is_none());
    page.model.wall_now_s = Some(-1);
    assert!(page.permanent_clock_key().is_none());
}

#[test]
fn background_changes_invalidate_visible_ground_without_pin_or_transparent_clock_redraw() {
    let _lock = plx_base::testlock::serial();
    let ps = PlaybackSession::IDLE;
    let mut page = hidden_clock();
    for mode in [UiMode::Classic, UiMode::Modern] {
        page.model.permanent_clock = PermanentClock::Off;
        page.hud.dismissed = false;
        page.hud.until = 10_000;
        page.model.osd_opacity = OsdOpacity::default();
        let before = page.clock_fingerprint_in_mode(&ps, 1_000, mode, None);
        page.model.osd_opacity = OsdOpacity::from_level(0).unwrap();
        assert_ne!(before, page.clock_fingerprint_in_mode(&ps, 1_000, mode, None));
        assert_eq!(page.hud.until, 10_000);
    }
    page.hud.hide(false);
    page.model.permanent_clock = PermanentClock::Transparent;
    let before = page.clock_fingerprint_in_mode(&ps, 1_000, UiMode::Modern, None);
    page.model.osd_opacity = OsdOpacity::from_level(10).unwrap();
    assert_eq!(before, page.clock_fingerprint_in_mode(&ps, 1_000, UiMode::Modern, None));
    page.model.permanent_clock = PermanentClock::Background;
    let before = page.clock_fingerprint_in_mode(&ps, 1_000, UiMode::Modern, None);
    page.model.osd_opacity = OsdOpacity::from_level(7).unwrap();
    assert_ne!(before, page.clock_fingerprint_in_mode(&ps, 1_000, UiMode::Modern, None));
}
