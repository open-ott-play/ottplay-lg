//! Actual pointer dispatcher -> source-bound app admission -> More. No OS window or renderer.
use super::*;
use plx_machine::machine::{InputEvent, UiMode};

fn app(native: bool, mode: UiMode) -> App {
    let mut app = if native { native_catalog_app().0 } else { remote_app() };
    let runtime = app.ott.as_mut().unwrap();
    runtime.saved.settings.insert("interfaceMode".into(), json!(if mode == UiMode::Classic { 0 } else { 1 }));
    app.bridge.apply_ui_mode(mode);
    app.inputs.clear();
    app
}
fn dispatch_pointer(app: &mut App, input: Vec<InputEvent<u32>>) -> run::Frame {
    let tick = next_tick(app);
    let ctrl = player_hud::slot(&app.player.session, app.bridge.metadata_view());
    let fr = run::Frame { ctrl, now: tick.ms, dt: 0.016, underlay_moving: false,
        player: true, present: false, deferred: false, rn: "player" };
    run::publish_playback_frame(app, true);
    bridge::frame(&mut app.pages, &mut app.bridge, tick, input);
    fr
}
fn point(app: &mut App, motion: bool, x: f32, y: f32) {
    let tick = Tick { ms: app.loop_t.wrapping_add(16), dt_us: 16_000 };
    let input = if motion { bridge::pointer_input(x, y, tick) } else { bridge::click_input(x, y, tick) };
    let fr = dispatch_pointer(app, vec![input]);
    finish_input(app, &fr);
}
fn more(app: &App) -> bool {
    bridge::player_overlay_kind(&app.pages) == Some(OverlayKind::More { page: None })
}

#[test]
fn top_video_click_opens_more_before_and_after_pointer_reveals_hud() {
    let _lock = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    let _registry = NativeCatalogRegistry;
    for native in [false, true] {
        for reveal in [false, true] {
            let mut app = app(native, UiMode::Classic);
            bridge::player_mut(&mut app.pages).unwrap().hud.hide(false);
            let identity = route::playback_identity(&app.player.session);
            let saved = serde_json::to_value(&app.ott.as_ref().unwrap().saved).unwrap();
            let paused = lifecycle::viewer_paused();
            let epoch = app.ott.as_ref().unwrap().playback_epoch;
            if reveal { point(&mut app, true, 500.0, 100.0); }
            assert!(app.pages.input.hit.top_at(500.0, 100.0).is_none(), "background, not a control");
            point(&mut app, false, 500.0, 100.0);
            idle(&mut app);
            assert!(more(&app));
            assert_eq!(route::playback_identity(&app.player.session), identity);
            assert_eq!(lifecycle::viewer_paused(), paused);
            let runtime = app.ott.as_ref().unwrap();
            assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), saved);
            assert_eq!(runtime.playback_epoch, epoch);
            assert_eq!(runtime.playing.is_none(), native);
            assert!(runtime.play_target.is_none() && runtime.catalog_next.is_none());
        }
    }
}

#[test]
fn top_video_click_does_not_fall_through_a_settings_surface_or_policy_refusal() {
    let _lock = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    let _registry = NativeCatalogRegistry;
    for surface in [false, true] {
        let mut app = app(true, UiMode::Classic);
        if surface { bridge::open_settings(&mut app.pages); idle(&mut app); }
        else { app.ott.as_mut().unwrap().saved.settings.insert("nativeKiosk".into(), json!("malformed")); }
        let identity = route::playback_identity(&app.player.session);
        let paused = lifecycle::viewer_paused();
        point(&mut app, false, 500.0, 100.0);
        idle(&mut app);
        assert!(!more(&app));
        assert_eq!(route::playback_identity(&app.player.session), identity);
        assert_eq!(lifecycle::viewer_paused(), paused);
        assert!(app.ott.as_ref().unwrap().play_target.is_none());
    }
}

#[test]
fn queued_top_video_click_cannot_open_menu_on_a_replacement_source() {
    let _lock = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    let _registry = NativeCatalogRegistry;
    let mut app = app(true, UiMode::Classic);
    let tick = Tick { ms: app.loop_t.wrapping_add(16), dt_us: 16_000 };
    let fr = dispatch_pointer(&mut app, vec![bridge::click_input(500.0, 100.0, tick)]);
    let sid = route::cur_sid(&app.player.session);
    route::install_plex_playback_for_test(&mut app.player.session, sid, "replacement");
    let replacement = route::playback_identity(&app.player.session);
    finish_input(&mut app, &fr);
    idle(&mut app);
    assert!(!more(&app));
    assert_eq!(route::playback_identity(&app.player.session), replacement);
    assert!(app.ott.as_ref().unwrap().playing.is_none());
}
