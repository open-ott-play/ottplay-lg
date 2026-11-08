//! Real held-surface lifecycle, not a manually installed modal or viewport lease.
use super::*;

struct TextReadiness;
impl Drop for TextReadiness {
    fn drop(&mut self) { plx_gfx::text::reset_prewarm_for_test(); }
}

fn focus_channel_list(app: &mut App) {
    open_more(app);
    assert!(more_keys(app).contains(&5));
    for _ in 0..16 {
        let overlay = bridge::player_overlay_mut(&mut app.pages).unwrap();
        let Panel::More(menu) = overlay.panel() else { panic!("More panel") };
        if menu.keys().get(menu.sel() as usize) == Some(&5) { break; }
        key(app, Key::Down);
    }
    let overlay = bridge::player_overlay_mut(&mut app.pages).unwrap();
    let Panel::More(menu) = overlay.panel() else { panic!("More panel") };
    assert_eq!(menu.keys().get(menu.sel() as usize), Some(&5));
}

fn held_catalog_has_layout(app: &App, owner: plx_machine::machine::InstanceId) -> bool {
    let entry = app.pages.nav.entry_of_instance(owner).expect("mounted catalog");
    let surface = app.pages.nav.modals.surfaces.iter()
        .find(|surface| surface.entry.id == entry).expect("real catalog surface");
    assert!(surface.motion.held(), "text readiness must keep the opening surface held");
    assert_eq!(surface.motion.appear, 0.0, "held surface has no visible aperture");
    let body = app.pages.nav.instance(owner).unwrap();
    let screen = body.screen.as_any().unwrap()
        .downcast_ref::<plx_screens::ott::OttScreen>().unwrap();
    screen.viewport_target().is_some()
}

fn held_catalog_for_mode(mode:plx_machine::machine::UiMode) {
    let _serial = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    let _probe = PlacementProbe;
    let _readiness = TextReadiness;
    plx_gfx::text::reset_prewarm_for_test();
    let mut app = placeable_catalog_app();
    if mode != plx_machine::machine::UiMode::Classic {
        let runtime=app.ott.as_mut().unwrap();
        let saved=runtime.interface_mode_candidate(mode);
        runtime.saving=true;runtime.save_navigation=SaveNavigation::Preserve;
        runtime.save_playback_epoch=runtime.playback_epoch;
        runtime.tx.send(Job::Saved(Ok(saved),None,false,json!({"status":"ok"})))
            .unwrap_or_else(|_| panic!("fixture receiver disappeared"));
        runtime_tick(&mut app);idle(&mut app);
        assert_eq!(app.bridge.ui_mode(),mode);
    }
    let identity = route::playback_identity(&app.player.session);
    let source = player::viewport::capture_source(&mut app.adapters.player, &app.player.session)
        .expect("ready native source");
    let page = app.pages.top_page();
    focus_channel_list(&mut app);
    // The same per-frame environmental input gates PopoverMotion in production.
    // Do not call key(Ok): its 32 idle frames would hide the held interval.
    plx_gfx::text::latch_surface_text_pending(true);
    let tick = Tick { ms: app.loop_t.wrapping_add(16), dt_us: 16_000 };
    frame(&mut app, bridge::script_key(Key::Ok, tick));
    for _ in 0..8 {
        if ott_surface(&app).is_some() { break; }
        frame(&mut app, Vec::new());
    }
    let owner = ott_surface(&app).expect("actual Channel list activation mounted catalog");
    let mut had_layout = held_catalog_has_layout(&app, owner);
    // Four 16 ms frames stay below the production 100 ms text-hold deadline,
    // but allow the real Source -> Set -> Submitted queue to run if misadmitted.
    for _ in 0..4 {
        crate::app::player_viewport::sync(&mut app);
        frame(&mut app, Vec::new());
        had_layout |= held_catalog_has_layout(&app, owner);
        assert!(player::ffi_host::place_calls_for_test().is_empty(),
            "a recording-only held catalog must not shrink the native video plane");
        assert!(!crate::app::player_viewport::composite(&mut app));
    }
    assert!(had_layout, "absence of placement must not be explained by missing catalog geometry");
    assert_eq!(ott_surface(&app), Some(owner));
    plx_gfx::text::latch_surface_text_pending(false);
    frame(&mut app, Vec::new());
    let entry = app.pages.nav.entry_of_instance(owner).unwrap();
    let surface = app.pages.nav.modals.surfaces.iter()
        .find(|surface| surface.entry.id == entry).unwrap();
    assert!(!surface.motion.held());
    assert!(surface.motion.appear > 0.0);
    for _ in 0..12 {
        crate::app::player_viewport::sync(&mut app);
        frame(&mut app, Vec::new());
        if !player::ffi_host::place_calls_for_test().is_empty() { break; }
    }
    assert_eq!(player::ffi_host::place_calls_for_test().len(), 1,
        "released catalog must issue the real native placement");
    assert!(!crate::app::player_viewport::composite(&mut app),
        "queued Submitted is not yet an acknowledged screen aperture");
    for _ in 0..4 {
        frame(&mut app, Vec::new());
        if crate::app::player_viewport::composite(&mut app) { break; }
    }
    assert!(crate::app::player_viewport::composite(&mut app));
    assert_eq!(player::ffi_host::place_calls_for_test().len(), 1);
    assert_eq!(route::playback_identity(&app.player.session), identity);
    assert_eq!(player::viewport::capture_source(&mut app.adapters.player, &app.player.session), Some(source));
    assert_eq!(app.pages.top_page(), page);
    assert!(app.adapters.player.is_live());
    assert_eq!(player::SHARED.playpos_ns.load(Ordering::Acquire), 7_000_000_000);
}


#[test]
fn held_catalog_keeps_fullscreen_until_released_and_submitted() {
    held_catalog_for_mode(plx_machine::machine::UiMode::Classic);
}

#[test]
fn modern_held_catalog_keeps_fullscreen_until_released_and_submitted() {
    held_catalog_for_mode(plx_machine::machine::UiMode::Modern);
}
