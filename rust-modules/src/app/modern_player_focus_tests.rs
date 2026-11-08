// Existing Player owner, real application auto-hide phase and FocusEngine.
// The standard control rail is Modern-only. Classic hidden controls and contextual
// rows are covered by screens::player::classic_hidden_focus_tests.
// Synthetic state only: no media engine, renderer, worker or device is started.
use super::*;
use plx_screens::player::input::HUD_LINGER_MS;
use crate::app::bridge;
use plx_appkit::player_hud::{
    command::{HudCommand, MorePage},
    geometry::{GROUP_ROW, GROUP_SCRUB},
    ELEM_SCRUB,
};
use plx_machine::machine::{Delivery, Fx, Key, MachineId, PressId, ScreenEvent, Tick, UiMode};
use plx_screens::registry::PlayerReq;

struct PlaybackStateRestore(u8);
impl PlaybackStateRestore {
    fn set(state: plx_media::player::PlaybackState) -> Self {
        Self(plx_media::player::swap_state_for_test(state))
    }
}
impl Drop for PlaybackStateRestore {
    fn drop(&mut self) {
        plx_media::player::restore_state_for_test(self.0);
    }
}

fn shared_key(app: &mut App, key: Key, now: u32) {
    // Script input deliberately keeps the existing host fixture's prelude;
    // physical/RemoteFifo ordering is covered by programme_remote_keys.
    let paused_now = paused();
    bridge::player_mut(&mut app.pages).unwrap().hud.note_fresh_press(
        &app.player.session, now, paused_now,
    );
    let tick = Tick { ms: now, dt_us: 16_000 };
    bridge::frame(&mut app.pages, &mut app.bridge, tick, bridge::script_key(key, tick));
}

fn focus_more(app: &mut App, mode: UiMode) -> u32 {
    app.bridge.apply_ui_mode(mode);
    bridge::nav_push(&mut app.pages, AppArg::Player);
    for now in [16, 32, 48] { frame(app, now); }
    assert_eq!(app.bridge.ui_mode(), mode);
    bridge::player_mut(&mut app.pages).unwrap().hud.extend(100, HUD_LINGER_MS);
    assert_eq!(app.pages.focus().map(|key| key.elem), Some(ELEM_SCRUB));
    app.bridge.take_player_reqs();
    shared_key(app, Key::Up, 116);
    assert_eq!(bridge::player(&app.pages).unwrap().hud.nav.focus, GROUP_ROW);
    let steps = bridge::player(&app.pages).unwrap().slot.items();
    for index in 0..steps {
        let player = bridge::player(&app.pages).unwrap();
        if player.slot.command(player.hud.nav.btn) == Some(HudCommand::OpenMore(MorePage::Root)) {
            break;
        }
        shared_key(app, Key::Right, 132 + index as u32 * 32);
    }
    let player = bridge::player(&app.pages).unwrap();
    assert_eq!(player.slot.command(player.hud.nav.btn), Some(HudCommand::OpenMore(MorePage::Root)));
    let focused = app.pages.focus().unwrap().elem;
    assert_eq!(plx_appkit::player_hud::row_index(focused), Some(player.hud.nav.btn));
    assert!(app.bridge.take_player_reqs().is_empty(), "focus movement never activates playback or More");
    focused
}

fn auto_hide_phase(app: &mut App, now: u32) {
    let mut fr = Frame::begin(&app.player.session, app.bridge.metadata_view());
    fr.now = now;
    unsafe { playback_tick(app, &mut fr); }
}

#[test]
fn modern_auto_hide_retires_the_engine_rail_before_a_late_commit_and_reveal() {
    let _serial = plx_base::testlock::serial();
    {
        let mode = UiMode::Modern;
        let mut app = app();
        let _paused = TransportPaused::set(false);
        let _state = PlaybackStateRestore::set(plx_media::player::PlaybackState::Playing);
        let clocks = SeekGlobals::set(12_000_000_000, 120_000_000_000);
        let more = focus_more(&mut app, mode);
        let owner = app.pages.top_page().unwrap();
        let until = bridge::player(&app.pages).unwrap().hud.until;
        let identity = plx_media::route::playback_identity(&app.player.session);
        auto_hide_phase(&mut app, until - 1);
        frame(&mut app, until - 1);
        assert_eq!(app.pages.focus().unwrap().elem, more, "visible rail keeps its focus");

        auto_hide_phase(&mut app, until);
        // A delayed control-row commit reaches the same real screen after the
        // auto-hide's queued re-seat. It must see the current engine focus.
        app.pages.emit(MachineId::Input, Fx::Deliver(MachineId::Instance(owner),
            Delivery::Screen(ScreenEvent::PressCommit(PressId(41)))));
        frame(&mut app, until);
        let player = bridge::player(&app.pages).unwrap();
        assert!(!player.hud_up(&app.player.session, until));
        assert_eq!(player.hud.nav.focus, GROUP_SCRUB);
        assert_eq!(app.pages.focus().unwrap().elem, ELEM_SCRUB, "engine, not only HUD mirror, retires the rail");
        assert!(app.bridge.take_player_reqs().is_empty(), "hidden More cannot commit");
        assert!(!app.pages.surface_up());
        assert_eq!(clocks.seek_to(), -1);
        assert_eq!(plx_media::route::playback_identity(&app.player.session), identity);

        shared_key(&mut app, Key::Up, until + 16);
        assert_eq!(app.pages.focus().unwrap().elem, ELEM_SCRUB, "first hidden generic Up only reveals");
        assert!(app.bridge.take_player_reqs().is_empty());
        shared_key(&mut app, Key::Ok, until + 32);
        assert_eq!(app.bridge.take_player_reqs(), vec![PlayerReq::Transport(None)], "OK uses the revealed scrub seat, not the retired More button");
        assert_eq!(app.pages.top_page(), Some(owner));
        assert!(!app.pages.surface_up());
        assert_eq!(clocks.seek_to(), -1, "test collects requests and never executes transport");
    }
}

#[test]
fn modern_expired_timer_preserves_visible_paused_or_buffering_rail_focus() {
    let _serial = plx_base::testlock::serial();
    {
        let mode = UiMode::Modern;
        for is_buffering in [false, true] {
            let mut app = app();
            let _paused = TransportPaused::set(false);
            let _state = PlaybackStateRestore::set(plx_media::player::PlaybackState::Playing);
            let focused = focus_more(&mut app, mode);
            let until = bridge::player(&app.pages).unwrap().hud.until;
            let _held_pause = TransportPaused::set(!is_buffering);
            let _held_state = PlaybackStateRestore::set(if is_buffering {
                plx_media::player::PlaybackState::Buffering
            } else {
                plx_media::player::PlaybackState::Playing
            });
            auto_hide_phase(&mut app, until + 1);
            frame(&mut app, until + 1);
            let player = bridge::player(&app.pages).unwrap();
            assert!(player.hud_up(&app.player.session, until + 1), "pause/buffering keeps the transport visible");
            assert_eq!(player.hud.nav.focus, GROUP_ROW);
            assert_eq!(app.pages.focus().unwrap().elem, focused, "timer expiry alone cannot retire a visible control");
            assert!(app.bridge.take_player_reqs().is_empty());
            assert!(!app.pages.surface_up());
        }
    }
}
