//! Actual native key ingress and the shared Info overlay. Workers stay held.
//! Source-authored regression input: no compiler, renderer or device execution.
use super::*;
use plx_appkit::player_hud;
use plx_machine::machine::{Edge, InputKind, UiMode};
use plx_screens::player::overlay::OverlayKind;
use plx_ui::consts;

fn app_with_description(description: &str, mode: UiMode) -> App {
    app_with_description_path(description, mode, false)
}

fn app_with_description_path(description: &str, mode: UiMode, retained_only: bool) -> App {
    let (mut app, sample) = live_app();
    presented(&mut app, sample);
    arm_playing(&mut app);
    let mut guide = reply(&app, "Current programme");
    guide.result.as_mut().unwrap().programmes[0].description = description.into();
    if retained_only {
        // A completed source-bound result survives a rebuilt HUD owner, whose
        // transient request/completion notice no longer exists.
        let candidate = crate::app::ott_runtime::selected_guide::Candidate::from_reply(
            &guide, (sample.unix_ms() / 1000) as i64);
        app.ott.as_mut().unwrap().live_hud = Default::default();
        publish_without_runtime_change(&mut app);
        assert!(live(&app).unwrap().guide.is_none());
        let runtime = app.ott.as_mut().unwrap();
        let playing = runtime.playing.clone();
        assert!(runtime.retain_selected_guide(Some(candidate), playing.as_ref()));
        publish_without_runtime_change(&mut app);
    } else {
        deliver(&mut app, sample, guide);
    }
    // The display-only Bridge seam changes no profile/source or playback state.
    app.bridge.apply_ui_mode(mode);
    mount_player(&mut app);
    settle(&mut app);
    assert_eq!(app.bridge.ui_mode(), mode);
    bridge::player_mut(&mut app.pages).unwrap().hud.hide(false);
    app.inputs.clear();
    app
}

struct FrameClock(u32);
impl Drop for FrameClock {
    fn drop(&mut self) { crate::app::clock::set_replay(self.0); }
}

fn frame(app: &mut App, bytes: Option<[u8; 128]>) -> usize {
    let tick = next_tick(app);
    // Native ingress uses clock::now(), so its repeat gate must share the frame's time.
    let _clock = FrameClock(crate::app::clock::now());
    crate::app::clock::set_replay(tick.ms);
    let ctrl = player_hud::slot(&app.player.session, app.bridge.metadata_view());
    let mut fr = run::Frame { ctrl, now: tick.ms, dt: 0.016, underlay_moving: false,
        player: matches!(app.route(), AppArg::Player), present: false, deferred: false, rn: "player" };
    if let Some(screen) = bridge::player_mut(&mut app.pages) { screen.slot = ctrl; }
    if let Some(bytes) = bytes {
        app.ev = bytes;
        unsafe { run::ingest_sdl_event_with_window(app, &mut fr,
            |_| panic!("native key must not restore a window")); }
    }
    run::publish_playback_frame(app, fr.player);
    let inputs = std::mem::take(&mut app.inputs);
    let repeats = inputs.iter().filter(|input| matches!(&input.kind,
        InputKind::Key { sym, wcode: 79, edge: Edge::Repeat, .. }
            if *sym == consts::SDLK_RIGHT)).count();
    bridge::frame(&mut app.pages, &mut app.bridge, tick, inputs);
    content::content_requests(app, &fr);
    runtime_tick(app);
    app.ev = [0; 128];
    repeats
}
fn settle(app: &mut App) { for _ in 0..32 { frame(app, None); } }
fn right(app: &mut App, edge: Edge) -> usize {
    let bytes = if edge == Edge::Repeat {
        crate::app::events::encode_key_repeat(consts::SDLK_RIGHT, 79)
    } else { crate::app::events::encode_key(consts::SDLK_RIGHT, 79, edge == Edge::Down) };
    frame(app, Some(bytes))
}
fn press_right(app: &mut App) {
    right(app, Edge::Down); right(app, Edge::Up); settle(app);
}
fn info(app: &App) -> bool { bridge::player_overlay_kind(&app.pages) == Some(OverlayKind::Info) }
fn visible(app: &App) -> bool {
    bridge::player(&app.pages).unwrap().hud_up(&app.player.session, app.loop_t)
}
fn close_animation(app: &mut App) {
    for _ in 0..200 {
        if !app.pages.surface_up() { return; }
        frame(app, None);
    }
    panic!("Info close did not retire through the shared modal stack");
}

#[test]
fn classic_live_right_cycles_footer_description_and_hidden_without_playback_change() {
    let _lock = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    let mut app = app_with_description("Complete current programme synopsis.", UiMode::Classic);
    let identity = route::playback_identity(&app.player.session);
    let page = app.pages.top_page();
    let paused = lifecycle::viewer_paused();
    let position = player::SHARED.playpos_ns.load(Ordering::Acquire);
    let saved = serde_json::to_value(&app.ott.as_ref().unwrap().saved).unwrap();
    assert!(!visible(&app));
    press_right(&mut app);
    assert!(visible(&app)); assert!(!app.pages.surface_up());
    press_right(&mut app);
    assert!(info(&app));
    assert_eq!(app.bridge.metadata_view().now_playing().unwrap().summary,
        "Complete current programme synopsis.");
    press_right(&mut app); close_animation(&mut app);
    assert!(!app.pages.surface_up()); assert!(!visible(&app));
    assert_eq!(route::playback_identity(&app.player.session), identity);
    assert_eq!(app.pages.top_page(), page);
    assert_eq!(lifecycle::viewer_paused(), paused);
    assert_eq!(player::SHARED.playpos_ns.load(Ordering::Acquire), position);
    assert!(app.ott.as_ref().unwrap().play_target.is_none());
    assert_eq!(serde_json::to_value(&app.ott.as_ref().unwrap().saved).unwrap(), saved);
}

#[test]
fn classic_live_current_programme_expands_even_without_synopsis_and_repeat_does_not_close() {
    let _lock = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    for text in ["", "  \n\t", "Available description"] {
        let mut app = app_with_description(text, UiMode::Classic);
        right(&mut app, Edge::Down); settle(&mut app);
        assert_eq!(right(&mut app, Edge::Repeat), 1, "held Right must reach the player");
        settle(&mut app);
        assert_eq!(right(&mut app, Edge::Repeat), 1, "later held Right must reach the player");
        right(&mut app, Edge::Up); settle(&mut app);
        assert!(visible(&app)); assert!(!app.pages.surface_up());
        // Keep this second press held: press_right() would release it before Repeat.
        right(&mut app, Edge::Down); settle(&mut app);
        assert!(info(&app), "a current programme title/time is information even without a synopsis");
        assert_eq!(right(&mut app, Edge::Repeat), 1, "held Right must reach Info");
        settle(&mut app);
        assert!(info(&app), "holding the second Right must not close Info");
        right(&mut app, Edge::Up); settle(&mut app);
        assert!(info(&app), "holding the second Right must not close Info");
    }
}


#[path = "programme_info_close.rs"]
mod close_guards;


fn press_up(app: &mut App) {
    frame(app, Some(crate::app::events::encode_key(consts::SDLK_UP, 82, true)));
    frame(app, Some(crate::app::events::encode_key(consts::SDLK_UP, 82, false)));
    settle(app);
}

#[test]
fn live_up_opens_current_programme_information_without_changing_playback_in_both_themes() {
    let _lock = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    for (mode, retained_only, description) in [UiMode::Classic, UiMode::Modern].into_iter()
        .flat_map(|mode| [false, true].into_iter().flat_map(move |retained|
            ["Complete current programme synopsis.", ""].into_iter().map(move |description|
                (mode, retained, description)))) {
        let mut app = app_with_description_path(description, mode, retained_only);
        let identity = route::playback_identity(&app.player.session);
        let saved = serde_json::to_value(&app.ott.as_ref().unwrap().saved).unwrap();
        press_up(&mut app);
        assert!(visible(&app));
        assert!(!app.pages.surface_up(), "first Up only reveals the information footer");
        press_up(&mut app);
        assert!(info(&app), "second Up expands the current programme information: {mode:?}");
        assert_eq!(app.bridge.metadata_view().now_playing().unwrap().summary, description);
        assert!(plx_appkit::info_panel::live_information_available(app.bridge.metadata_view(),
            (app.ott.as_ref().unwrap().wall_clock_ms / 1000) as i64));
        let guide = live(&app).unwrap().guide.unwrap();
        let expired_at = guide.current.iter().chain(guide.next.iter())
            .map(|programme| programme.end_s).max().unwrap() + 1;
        assert!(!plx_appkit::info_panel::live_information_available(
            app.bridge.metadata_view(), expired_at), "expired guide cannot borrow its old summary");
        assert_eq!(route::playback_identity(&app.player.session), identity);
        assert!(app.ott.as_ref().unwrap().play_target.is_none());
        assert_eq!(serde_json::to_value(&app.ott.as_ref().unwrap().saved).unwrap(), saved);
    }
}

#[test]
fn live_without_guide_or_synopsis_keeps_timed_footer_in_both_themes() {
    let _lock = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    for mode in [UiMode::Classic, UiMode::Modern] {
        let mut app = app_with_description("", mode);
        let runtime = app.ott.as_mut().unwrap();
        runtime.metadata_guide = None;
        runtime.live_hud = Default::default();
        publish_without_runtime_change(&mut app);
        assert!(!plx_appkit::info_panel::live_information_available(app.bridge.metadata_view(),
            (app.ott.as_ref().unwrap().wall_clock_ms / 1000) as i64));
        press_up(&mut app);
        press_up(&mut app);
        assert!(visible(&app));
        assert!(!app.pages.surface_up(), "no programme facts must not fabricate expanded information");
        if mode == UiMode::Classic {
            press_right(&mut app);
            assert!(!app.pages.surface_up());
        }
    }
}
