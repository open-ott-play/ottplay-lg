//! New semantic API guards; run after the production prerequisite.
use super::*;
use plx_screens::registry::{OttPlayerAction, OttPlayerRequest, PlayerReq};

#[test]
fn info_cycle_rejects_replaced_source_and_stale_close_cannot_hide_new_playback() {
    let _lock = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    let mut app = app_with_description("Description", UiMode::Classic);
    press_right(&mut app); press_right(&mut app); assert!(info(&app));
    let old = route::playback_identity(&app.player.session).unwrap();
    // Model an accepted replacement before the stale overlay receives its next key.
    // Use the existing route fixture; full teardown would correctly remove the panel first.
    route::install_plex_playback_for_test(&mut app.player.session,
        plx_plex::plex::ServerId::from_raw(0), "replacement");
    let new = route::playback_identity(&app.player.session).unwrap();
    assert_ne!(new, old);
    right(&mut app, Edge::Down); right(&mut app, Edge::Up);
    assert!(info(&app), "a stale overlay must not close or control the replacement");
    let now = app.loop_t;
    let page = bridge::player_mut(&mut app.pages).unwrap();
    page.hud.note_fresh_press(&app.player.session, now, true);
    page.hud.extend_default(now);
    assert!(!page.hide_info(&app.player.session, &old));
    assert!(page.hud_up(&app.player.session, now));
    assert_eq!(route::playback_identity(&app.player.session), Some(new));
}

#[test]
fn modern_live_right_keeps_hud_navigation_and_strict_policy_refuses_description() {
    let _lock = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    let mut app = app_with_description("Description", UiMode::Modern);
    press_right(&mut app); press_right(&mut app);
    assert!(!info(&app), "Modern visible-HUD Right remains shared focus navigation");
    app.bridge.apply_ui_mode(UiMode::Classic);
    let source = route::playback_identity(&app.player.session).unwrap();
    let owner = app.pages.nav.top_page().unwrap().inst.as_ref().unwrap().id;
    let mut runtime = app.ott.take().unwrap();
    runtime.saving = false; runtime.entropy = Some([7; 16]);
    let prepared = kiosk_source::prepare(&runtime, &app, Some("1"), true).unwrap();
    runtime.saved = kiosk_source::save_candidate(&runtime, &prepared).unwrap();
    runtime.sync_saved_context(); runtime.saving = true;
    let request = OttPlayerRequest { owner, source: source.into(), action: OttPlayerAction::ShowDescription };
    assert!(player_remote::perform(&mut runtime, &mut app, &request).is_err());
    assert!(!crate::app::ott_runtime::input_policy::player(&runtime, &PlayerReq::HideInfo(request.source.playback().unwrap().clone())));
    app.ott = Some(runtime);
    assert!(!info(&app));
}
