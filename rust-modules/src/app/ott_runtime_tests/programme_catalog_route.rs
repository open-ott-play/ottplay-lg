//! Real shared HUD/form input over an admitted OTT player. Native engine and
//! presentation observations use the existing programme fixture; no IO runs.
use super::*;
use plx_appkit::player_hud::{self, command::{HudCommand, MorePage}};
use plx_machine::machine::{InputEvent, Key};
use plx_screens::player::overlay::{OverlayKind, Panel};

#[path = "programme_catalog_held_viewport.rs"]
mod held_viewport;
#[path = "programme_catalog_strict.rs"]
mod strict;
#[path = "programme_vportal_resolved.rs"]
mod vportal_resolved;

fn catalog_app() -> App {
    let mut app = fixture();
    let runtime = app.ott.as_mut().unwrap();
    runtime.saving = false;
    runtime.page = "channels";
    runtime.selected = Some("c".into());
    runtime.catalog_done.store(false, Ordering::Release);
    runtime.metadata_done.store(false, Ordering::Release);
    runtime.resolver_busy = true;
    runtime.cancel.store(false, Ordering::Release);
    app.refresh_hubs = false;
    player::SHARED.start_queued.store(false, Ordering::Release);
    player::SHARED.seen_frame.store(true, Ordering::Release);
    mount_player(&mut app);
    let now = app.loop_t;
    bridge::player_mut(&mut app.pages).unwrap().hud.extend(now, 60_000);
    idle(&mut app);
    app
}

fn frame(app: &mut App, mut input: Vec<InputEvent<u32>>) {
    let tick = next_tick(app);
    // The real ingress samples HUD visibility before dispatching a fresh bound
    // press. Scripted canonical keys must cross that same SDL-free helper.
    for event in &mut input {
        let plx_machine::machine::InputKind::Key { key, sym, wcode, edge, .. } = &mut event.kind else { continue; };
        if *sym == 0 && *wcode == 0 {
            *sym = match key {
                Key::Up => plx_ui::consts::SDLK_UP,
                Key::Down => plx_ui::consts::SDLK_DOWN,
                Key::Left => plx_ui::consts::SDLK_LEFT,
                Key::Right => plx_ui::consts::SDLK_RIGHT,
                Key::Ok => plx_ui::consts::SDLK_RETURN,
                Key::Back => plx_ui::consts::SDLK_ESCAPE,
                Key::Other => 0,
            };
        }
        if *edge == plx_machine::machine::Edge::Down {
            crate::app::input::note_global_press(&app.player.session, *sym, *wcode,
                tick.ms, bridge::player_mut(&mut app.pages).map(|screen| &mut screen.hud),
                &mut app.ok_armed, &mut app.input.press);
            if let Some(screen) = bridge::player_mut(&mut app.pages) { screen.publish(); }
        }
    }
    // The ordinary loop supplies this same once-per-frame slot before dispatch.
    let ctrl = player_hud::slot(&app.player.session, app.bridge.metadata_view());
    if let Some(screen) = bridge::player_mut(&mut app.pages) { screen.slot = ctrl; }
    bridge::frame(&mut app.pages, &mut app.bridge, tick, input);
    let frame = run::Frame { ctrl, now: tick.ms, dt: 0.016, underlay_moving: false,
        player: matches!(app.route(), AppArg::Player), present: false,
        deferred: false, rn: "player" };
    content::content_requests(app, &frame);
    runtime_tick(app);
}

fn idle(app: &mut App) {
    for _ in 0..32 { frame(app, Vec::new()); }
}

fn key(app: &mut App, key: Key) {
    let tick = Tick { ms: app.loop_t.wrapping_add(16), dt_us: 16_000 };
    frame(app, bridge::script_key(key, tick));
    idle(app);
    retire_closing_surfaces(app);
}

fn retire_closing_surfaces(app: &mut App) {
    // The modal's exact rest includes velocity, so 24/32 fixed frames need not
    // finish a close. Pump the ordinary dispatcher until its own prune retires
    // the closing entries, with the same bounded budget as the modal fixtures.
    for _ in 0..200 {
        if !app.pages.nav.modals.surfaces.iter().any(|surface|
            surface.phase == plx_ui::containers::modal::Phase::Closing) { return; }
        frame(app, Vec::new());
    }
    panic!("closing surfaces did not retire through the dispatcher within 200 frames");
}

fn open_more(app: &mut App) {
    if app.bridge.ui_mode() == plx_machine::machine::UiMode::Classic {
        // Classic no longer paints the disc row. Use the real native Menu key
        // through the same SDL-byte ingress and content runner as production.
        // This is native evdev/SDL Yellow (488), not browser keyCode 405.
        for down in [true, false] {
            let tick = next_tick(app);
            let ctrl = player_hud::slot(&app.player.session, app.bridge.metadata_view());
            let mut fr = run::Frame { ctrl, now: tick.ms, dt: 0.016, underlay_moving: false,
                player: matches!(app.route(), AppArg::Player), present: false,
                deferred: false, rn: "player" };
            if let Some(screen) = bridge::player_mut(&mut app.pages) { screen.slot = ctrl; }
            app.ev = crate::app::events::encode_key(0, plx_ui::consts::WCODE_YELLOW, down);
            unsafe {
                run::ingest_sdl_event_with_window(app, &mut fr,
                    |_| panic!("Menu key must not restore a window"));
            }
            run::publish_playback_frame(app, fr.player);
            let inputs = std::mem::take(&mut app.inputs);
            bridge::frame(&mut app.pages, &mut app.bridge, tick, inputs);
            content::content_requests(app, &fr);
            runtime_tick(app);
            app.ev = [0; 128];
        }
        idle(app);
        assert_eq!(bridge::player_overlay_kind(&app.pages), Some(OverlayKind::More { page: None }),
            "the real Classic Yellow key must mount the More surface");
        return;
    }
    key(app, Key::Up);
    let wanted = HudCommand::OpenMore(MorePage::Root);
    for _ in 0..8 {
        let screen = bridge::player(&app.pages).unwrap();
        if screen.slot.command(screen.hud.nav.btn) == Some(wanted) { break; }
        key(app, Key::Right);
    }
    let screen = bridge::player(&app.pages).unwrap();
    assert_eq!(screen.slot.command(screen.hud.nav.btn), Some(wanted),
        "the real HUD focus must reach More before testing its menu");
    key(app, Key::Ok);
    assert_eq!(bridge::player_overlay_kind(&app.pages), Some(OverlayKind::More { page: None }),
        "the shared HUD press must mount its actual More surface");
}

fn more_keys(app: &mut App) -> Vec<u32> {
    let overlay = bridge::player_overlay_mut(&mut app.pages).expect("More owns input");
    let Panel::More(menu) = overlay.panel() else { panic!("More panel") };
    menu.keys()
}

#[test]
fn playing_ott_more_offers_channel_list_through_the_shared_hud() {
    let _serial = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    let mut app = catalog_app();
    let identity = route::playback_identity(&app.player.session);
    let source = route::external_source(&app.player.session);
    let page = app.pages.top_page();
    open_more(&mut app);
    assert!(more_keys(&mut app).contains(&5), "Options must offer Channel list for admitted OTT playback");
    assert_eq!(route::playback_identity(&app.player.session), identity);
    assert_eq!(route::external_source(&app.player.session), source);
    assert_eq!(app.pages.top_page(), page);
    assert!(app.adapters.player.is_live());
    assert!(app.player.pending_exit.is_none());
}

fn choose_catalog(app: &mut App) {
    assert!(more_keys(app).contains(&5), "Channel list is a real offered row");
    for _ in 0..16 {
        let overlay = bridge::player_overlay_mut(&mut app.pages).unwrap();
        let Panel::More(menu) = overlay.panel() else { panic!("More panel") };
        if menu.keys().get(menu.sel() as usize) == Some(&5) { break; }
        key(app, Key::Down);
    }
    let overlay = bridge::player_overlay_mut(&mut app.pages).unwrap();
    let Panel::More(menu) = overlay.panel() else { panic!("More panel") };
    assert_eq!(menu.keys().get(menu.sel() as usize), Some(&5));
    key(app, Key::Ok);
    assert!(ott_surface(app).is_some(), "the real offered row opens the shared OTT surface");
}

fn input_owner(app: &App) -> plx_machine::machine::InstanceId {
    let Some(plx_machine::machine::InputOwner::Entry(entry)) = app.pages.nav.input_owner() else {
        panic!("screen owns input")
    };
    app.pages.nav.entry(entry).unwrap().inst.as_ref().unwrap().id
}

fn ott_surface(app: &App) -> Option<plx_machine::machine::InstanceId> {
    let plx_machine::machine::InputOwner::Entry(entry) = app.pages.nav.input_owner()? else { return None; };
    if !app.pages.nav.is_surface(entry) { return None; }
    let entry = app.pages.nav.entry(entry)?;
    matches!(entry.arg, AppArg::Ott).then(|| entry.inst.as_ref().unwrap().id)
}

// Internal navigation uses the same addressed effect the mounted OTT form emits.
// Native key traversal and the More activation are tested independently above.
fn surface_action(app: &mut App, action: OttAction) {
    let reply = ott_surface(app).expect("current OTT surface");
    app.pages.emit(MachineId::Instance(reply), Fx::App(AppFx::Ott(OttOut::Action { reply, action })));
    frame(app, Vec::new());
    idle(app);
}

#[test]
fn catalog_menu_and_back_keep_the_same_player_and_decoder() {
    let _serial = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    let mut app = catalog_app();
    let identity = route::playback_identity(&app.player.session);
    let source = route::external_source(&app.player.session);
    let page = app.pages.top_page();
    let origin = bridge::player(&app.pages).unwrap().origin;
    let position = player::SHARED.playpos_ns.load(Ordering::Acquire);
    let generation = app.ott.as_ref().unwrap().generation;
    open_more(&mut app);
    choose_catalog(&mut app);
    assert!(matches!(app.route(), AppArg::Player));
    assert_eq!(app.ott.as_ref().unwrap().page, "channels");
    assert_eq!(route::playback_identity(&app.player.session), identity);
    assert_eq!(route::external_source(&app.player.session), source);
    assert!(app.adapters.player.is_live());
    assert_eq!(app.pages.top_page(), page);
    assert_eq!(bridge::player(&app.pages).unwrap().origin, origin);
    assert!(bridge::ott_visible(&app.pages));
    bridge::open_player_overlay(&app.player.session, app.bridge.metadata_view(),
        &mut app.pages, OverlayKind::More { page: None });
    idle(&mut app);
    assert!(bridge::ott_visible(&app.pages), "covered catalog still suppresses private input recording");
    assert!(super::super::super::current_ui_owner(&app).is_none(), "cover owns interaction");
    key(&mut app, Key::Back);
    assert!(ott_surface(&app).is_some());
    assert!(bridge::ott_visible(&app.pages));
    key(&mut app, Key::Back);
    assert!(ott_surface(&app).is_none());
    assert!(!bridge::ott_visible(&app.pages));
    assert!(!app.pages.surface_up());
    assert!(matches!(app.route(), AppArg::Player));
    assert_eq!(app.pages.top_page(), page, "return reuses Player, without PopTo its launch page");
    assert_eq!(route::playback_identity(&app.player.session), identity);
    assert_eq!(route::external_source(&app.player.session), source);
    assert_eq!(player::SHARED.playpos_ns.load(Ordering::Acquire), position);
    assert_eq!(app.ott.as_ref().unwrap().generation, generation);
    assert!(app.adapters.player.is_live());
    assert!(app.player.pending_exit.is_none());
    assert!(!app.refresh_hubs);
}

#[test]
fn catalog_categories_and_providers_do_not_cancel_the_admitted_stream() {
    let _serial = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    let mut app = catalog_app();
    let identity = route::playback_identity(&app.player.session);
    let page = app.pages.top_page();
    let epoch = app.ott.as_ref().unwrap().playback_epoch;
    open_more(&mut app);
    choose_catalog(&mut app);
    let surface = ott_surface(&app);
    surface_action(&mut app, OttAction::ShowCategories);
    assert_eq!(app.ott.as_ref().unwrap().page, "categories");
    key(&mut app, Key::Back);
    assert_eq!(app.ott.as_ref().unwrap().page, "channels");
    assert_eq!(ott_surface(&app), surface);
    surface_action(&mut app, OttAction::ShowProviders);
    assert_eq!(app.ott.as_ref().unwrap().page, "providers");
    surface_action(&mut app, OttAction::ShowProfiles);
    assert_eq!(app.ott.as_ref().unwrap().page, "profiles");
    key(&mut app, Key::Back);
    assert_eq!(app.ott.as_ref().unwrap().page, "providers");
    assert_eq!(ott_surface(&app), surface);
    key(&mut app, Key::Back);
    assert!(ott_surface(&app).is_none());
    assert_eq!(app.pages.top_page(), page);
    assert_eq!(route::playback_identity(&app.player.session), identity);
    assert_eq!(app.ott.as_ref().unwrap().playback_epoch, epoch);
    assert!(!app.ott.as_ref().unwrap().cancel.load(Ordering::Acquire));
    assert!(app.adapters.player.is_live());
    assert!(app.player.pending_exit.is_none());
}

#[test]
fn stale_catalog_offer_cannot_open_for_a_replacement_source() {
    let _serial = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    let mut app = catalog_app();
    open_more(&mut app);
    let request = plx_screens::registry::PlayerCatalogRequest {
        owner: input_owner(&app), source: route::playback_identity(&app.player.session).unwrap(),
    };
    let page = app.pages.top_page();
    let origin = bridge::player(&app.pages).unwrap().origin;
    // Use the real successor teardown: programme::install uses exit-style
    // replacement, which queues PopTo(Ott) and would destroy this test's Player.
    playback::replace_player_in_place(&mut app.player, &mut app.adapters.player,
        &mut app.repause_at, &mut app.pages);
    route::reset_player_control_for_test(&app.player.session);
    let mut info = unsupported_info();
    info.video_codec = "h264".into();
    info.audio_codec = "aac".into();
    info.audio_channels = 2;
    info.audio_sample_rate = 48_000;
    info.audio_bit_depth = 16;
    info.duration_ns = 600_000_000_000;
    route::request_external_source(&mut app.player.session, &info,
        plx_media::ff::ExternalSource::Http("https://media.invalid/programme.mkv".into()),
        "Current file", "Collection", plx_platform::devcaps::assumed_shared()).unwrap();
    route::reset_player_control_for_test(&app.player.session);
    app.adapters.player.install(player::engine::engine_after_reload_for_test());
    assert!(player::pause(&mut app.adapters.player));
    player::swap_state_for_test(player::PlaybackState::Paused);
    let replacement = route::playback_identity(&app.player.session);
    assert_ne!(replacement.as_ref(), Some(&request.source));
    // Model the accepted replacement receipt, so it is the captured offer that
    // is stale, not an unrelated mismatch in the current Runtime.playing value.
    let playing = app.ott.as_mut().unwrap().playing.as_mut().unwrap();
    playing.identity = replacement.clone();
    playing.source = route::external_source(&app.player.session);
    idle(&mut app);
    assert!(matches!(app.route(), AppArg::Player));
    assert_eq!(app.pages.top_page(), page);
    assert_eq!(bridge::player(&app.pages).unwrap().origin, origin);
    let generation = app.ott.as_ref().unwrap().generation;
    assert!(super::super::super::open_player_catalog(&mut app, request).is_err());
    idle(&mut app);
    assert!(ott_surface(&app).is_none());
    assert!(!bridge::ott_visible(&app.pages));
    assert!(matches!(app.route(), AppArg::Player));
    assert_eq!(route::playback_identity(&app.player.session), replacement);
    assert_eq!(app.pages.top_page(), page);
    assert_eq!(bridge::player(&app.pages).unwrap().origin, origin);
    assert_eq!(app.ott.as_ref().unwrap().generation, generation);
    assert!(app.adapters.player.is_live());
    assert!(app.player.pending_exit.is_none());
}

#[test]
fn a_player_instance_cannot_borrow_the_current_more_menu_offer() {
    let _serial = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    let mut app = catalog_app();
    let player_owner = app.pages.top_page().unwrap();
    open_more(&mut app);
    assert_ne!(input_owner(&app), player_owner);
    let request = plx_screens::registry::PlayerCatalogRequest {
        owner: player_owner, source: route::playback_identity(&app.player.session).unwrap(),
    };
    let menu = input_owner(&app);
    assert!(super::super::super::open_player_catalog(&mut app, request).is_err());
    idle(&mut app);
    assert_eq!(input_owner(&app), menu, "wrong owner refusal preserves the active menu");
    assert!(ott_surface(&app).is_none());
    assert!(app.adapters.player.is_live());
    assert!(app.player.pending_exit.is_none());
}

#[test]
fn foreign_playback_keeps_more_without_catalog_and_stop_still_exits() {
    let _serial = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    let mut app = catalog_app();
    app.ott.as_mut().unwrap().playing = None;
    idle(&mut app);
    open_more(&mut app);
    assert!(!more_keys(&mut app).contains(&5), "a foreign source has no OTT catalog offer");
    key(&mut app, Key::Back);
    assert!(!app.pages.surface_up());
    let tick = Tick { ms: app.loop_t.wrapping_add(16), dt_us: 16_000 };
    frame(&mut app, vec![InputEvent { at: tick, source: plx_machine::machine::Source::Sdl,
        kind: plx_machine::machine::InputKind::Key { key: Key::Other,
            sym: 0, wcode: plx_ui::consts::WCODE_STOP,
            edge: plx_machine::machine::Edge::Down, at_edge: false } }]);
    idle(&mut app);
    assert!(!route::has_url(&app.player.session));
    assert!(!app.adapters.player.is_live());
    assert!(!matches!(app.route(), AppArg::Player));
}

#[test]
fn explicit_player_exit_retires_the_owned_catalog_surface() {
    let _serial = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    let mut app = catalog_app();
    open_more(&mut app);
    choose_catalog(&mut app);
    // Existing real content performer, shared with Stop. This is an exit,
    // unlike the native Back tested above which only closes the browse surface.
    exit_through_content(&mut app);
    runtime_tick(&mut app);
    settle_pages(&mut app);
    assert!(!route::has_url(&app.player.session));
    assert!(!app.adapters.player.is_live());
    assert!(!matches!(app.route(), AppArg::Player));
    assert!(!app.pages.nav.modals.surfaces.iter().any(|surface|
        matches!(surface.entry.arg, AppArg::Ott)));
}

struct PlacementProbe;
impl Drop for PlacementProbe {
    fn drop(&mut self) {
        player::ffi_host::reset_place_probe_for_test(false);
        player::ffi_host::force_clocksink_for_test(false);
    }
}

// Same admitted native-route fixture as ott_viewport_runtime_tests::native.
// The engine itself remains the existing inert host fixture, not a decoder.
fn placeable_catalog_app() -> App {
    let mut app = catalog_app();
    playback::replace_player(&mut app.player, &mut app.adapters.player,
        &mut app.repause_at, &mut app.refresh_hubs, &mut app.pages);
    player::SHARED.reset_session();
    player::TX.reset();
    route::reset_player_control_for_test(&app.player.session);
    player::ffi_host::force_clocksink_for_test(true);
    player::ffi_host::reset_place_probe_for_test(false);
    let mut info = unsupported_info();
    info.video_codec = "h264".into(); info.audio_codec = "aac".into();
    info.width = 1280; info.height = 720; info.fps = 24.0;
    info.duration_ns = 60_000_000_000; info.audio_channels = 2;
    info.audio_sample_rate = 48_000; info.audio_bit_depth = 16;
    route::request_external(&mut app.player.session, &info,
        "https://media.invalid/placement.mp4", "Current file", "Collection",
        plx_platform::devcaps::assumed_shared()).unwrap();
    let transaction = route::pending_route_start().expect("real prepared route");
    let attempt = route::claim_route_start_attempt(transaction).expect("native attempt");
    assert!(route::settle_route_start(&mut app.player.session, attempt, route::RouteStartResult::Started));
    let epoch = player::SHARED.begin_native_session().expect("native epoch");
    assert!(player::SHARED.mark_native_load_returned(epoch, None).is_some());
    let mut engine = player::engine::engine_after_reload_for_test();
    engine.native_epoch = epoch;
    engine.route_start = attempt;
    app.adapters.player.install(engine);
    player::SHARED.publish_video_raster(1280, 720);
    player::swap_state_for_test(player::PlaybackState::Playing);
    player::SHARED.playpos_ns.store(7_000_000_000, Ordering::Release);
    let runtime = app.ott.as_mut().unwrap();
    let playing = runtime.playing.as_mut().unwrap();
    playing.identity = route::playback_identity(&app.player.session);
    playing.source = route::external_source(&app.player.session);
    playing.entry.media = MediaLocator::Http { url: "https://media.invalid/placement.mp4".into() };
    runtime.catalog.entries[0] = playing.entry.clone();
    // The existing accepted-Saved seam, not a direct renderer mode override.
    let saved = runtime.interface_mode_candidate(plx_machine::machine::UiMode::Classic);
    runtime.saving = true;
    runtime.save_navigation = SaveNavigation::Preserve;
    runtime.save_playback_epoch = runtime.playback_epoch;
    runtime.tx.send(Job::Saved(Ok(saved), None, false, json!({"status":"ok"})))
        .unwrap_or_else(|_| panic!("fixture receiver disappeared"));
    runtime_tick(&mut app);
    mount_player(&mut app);
    let now = app.loop_t;
    bridge::player_mut(&mut app.pages).unwrap().hud.extend(now, 60_000);
    app.refresh_hubs = false;
    idle(&mut app);
    assert_eq!(app.bridge.ui_mode(), plx_machine::machine::UiMode::Classic);
    app
}

#[test]
fn real_catalog_surface_ack_places_and_restores_the_same_native_source() {
    let _serial = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    let _probe = PlacementProbe;
    let mut app = placeable_catalog_app();
    let identity = route::playback_identity(&app.player.session);
    let source = player::viewport::capture_source(&mut app.adapters.player, &app.player.session)
        .expect("ready native source");
    let page = app.pages.top_page();
    open_more(&mut app);
    choose_catalog(&mut app);
    assert!(!crate::app::player_viewport::composite(&mut app), "no placement before acknowledged request");
    for _ in 0..12 {
        crate::app::player_viewport::sync(&mut app);
        frame(&mut app, Vec::new());
    }
    assert!(crate::app::player_viewport::composite(&mut app), "real screen consumed the actual Submitted reply");
    assert_eq!(player::ffi_host::place_calls_for_test().len(), 1);
    assert_eq!(player::ffi_host::place_calls_for_test()[0].1, 1);
    assert_eq!(route::playback_identity(&app.player.session), identity);
    player::ffi_host::reset_place_probe_for_test(false);
    key(&mut app, Key::Back);
    crate::app::player_viewport::sync(&mut app);
    assert_eq!(player::ffi_host::place_calls_for_test(), vec![([1280, 720, 0, 0, 1920, 1080], 1)]);
    assert!(!crate::app::player_viewport::composite(&mut app));
    assert!(super::super::super::current_ui_owner(&app).is_none());
    assert_eq!(player::viewport::capture_source(&mut app.adapters.player, &app.player.session), Some(source));
    assert_eq!(route::playback_identity(&app.player.session), identity);
    assert_eq!(app.pages.top_page(), page);
    assert!(app.adapters.player.is_live());
    assert_eq!(player::SHARED.playpos_ns.load(Ordering::Acquire), 7_000_000_000);
}

#[derive(Default)]
struct CatalogAcceptedResources { starts: usize }

impl playback::PlaybackResources for CatalogAcceptedResources {
    fn request_movie(&mut self, _: &mut route::PlaybackSession,
        _: &mut plx_data::stores::metadata::MetadataStore, _: &plx_data::pms::PmsMovie,
        _: Option<plx_plex::plex::MediaId>) -> bool { panic!("external source only") }
    fn request_episode(&mut self, _: &mut route::PlaybackSession,
        _: &mut plx_data::stores::metadata::MetadataStore, _: &str,
        _: Option<plx_plex::plex::MediaId>) -> bool { panic!("external source only") }
    fn describe_movie(&mut self, _: &mut plx_data::stores::metadata::MetadataStore,
        _: plx_plex::plex::ServerId, _: &str) { panic!("external source only") }
    fn prepare_start(&mut self, _: &mut route::PlaybackSession,
        _: &mut player::adapter::PlayerAdapter, _: i64) -> bool {
        self.starts += 1;
        true
    }
}

#[test]
fn native_eos_under_catalog_admits_one_successor_without_exiting_player() {
    let _serial = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    let mut app = catalog_app();
    // The common programme fixture is paused. Resume through the same native
    // request dispatcher that commits viewer transport and continuity intent.
    assert!(lifecycle::viewer_paused());
    let transport_epoch = app.ott.as_ref().unwrap().playback_epoch;
    let player_owner = app.pages.top_page().expect("mounted Player");
    app.pages.emit(MachineId::Instance(player_owner),
        Fx::App(AppFx::Player(plx_screens::registry::PlayerReq::Transport(Some(true)))));
    frame(&mut app, Vec::new());
    assert!(!lifecycle::viewer_paused(), "the native Play request must resume the admitted source");
    assert_eq!(app.ott.as_ref().unwrap().playback_epoch, transport_epoch.wrapping_add(1),
        "accepted native transport must commit the catalogue's viewer intent");
    open_more(&mut app);
    choose_catalog(&mut app);
    let page = app.pages.top_page();
    let original_identity = route::playback_identity(&app.player.session);
    let origin = bridge::player(&app.pages).unwrap().origin;
    let depth = app.pages.nav.tabs.stack.depth();
    let surface = ott_surface(&app).expect("catalog owns input");
    let runtime = app.ott.as_mut().unwrap();
    assert_eq!(runtime.ordered_ids(), ["c", "a", "b"]);
    assert!(runtime.continuity.accepts_end(runtime.generation, "c"));
    // Match the existing catalogue EOF fixture: checkpoint admission cannot
    // start a protected writer while this test follows only the EOS transition.
    runtime.saving = true;
    assert!(!player::is_started(), "existing inert engine must not run a decoder pump");
    assert!(app.adapters.player.is_live());
    // This is the native observation consumed by the real loop, not a direct
    // call to the catalogue's finish function or to its continuity model.
    player::SHARED.ended.store(true, Ordering::Release);
    let tick = next_tick(&mut app);
    let mut fr = run::Frame {
        ctrl: player_hud::slot(&app.player.session, app.bridge.metadata_view()),
        now: tick.ms, dt: 0.016, underlay_moving: false, player: true,
        present: false, deferred: false, rn: "player",
    };
    unsafe { run::playback_tick(&mut app, &mut fr); }
    let runtime = app.ott.as_ref().unwrap();
    assert_eq!(runtime.play_target.as_ref().map(|request| request.id.as_str()), Some("a"));
    assert_eq!(runtime.resolver_next.as_ref().and_then(|request| request.entry.as_ref())
        .map(|entry| entry.id.as_str()), Some("a"));
    let generation = runtime.play_generation;
    assert!(!app.adapters.player.is_live(), "old decoder is released once at EOS");
    assert!(!player::ended(), "old EOS observation is consumed by teardown");
    assert_eq!(app.pages.top_page(), page);
    assert_eq!(ott_surface(&app), Some(surface));
    assert!(matches!(app.route(), AppArg::Player));
    assert!(app.player.pending_exit.is_none(), "catalogue EOS must not take generic Exit/PopTo");
    assert!(!app.pages.has_pending_navigation());
    assert!(!app.refresh_hubs);
    fr.now = fr.now.wrapping_add(16);
    unsafe { run::playback_tick(&mut app, &mut fr); }
    assert_eq!(app.ott.as_ref().unwrap().play_generation, generation,
        "a repeated loop cannot consume the old EOS again");
    assert_eq!(app.ott.as_ref().unwrap().play_target.as_ref().map(|request| request.id.as_str()), Some("a"));
    assert_eq!(app.pages.top_page(), page);
    assert_eq!(ott_surface(&app), Some(surface));

    // Follow the already-qualified successor admission seam: the real queued
    // request and start_resolved_with, substituting only resource acceptance.
    // No resolver, decoder or network worker is started by this test.
    let mut runtime = app.ott.take().unwrap();
    let request = runtime.play_target.clone().expect("EOS queued the successor");
    let profile = runtime.effective_profile().unwrap().clone();
    let entry = runtime.catalog.entries.iter().find(|entry| entry.id == request.id).unwrap().clone();
    runtime.rebind_native_admission(&mut app, &profile, &entry).unwrap();
    let mut info = unsupported_info();
    info.video_codec = "h264".into(); info.audio_codec = "aac".into();
    info.audio_channels = 2; info.audio_sample_rate = 48_000; info.audio_bit_depth = 16;
    let MediaLocator::Http { url } = &entry.media else { panic!("external successor") };
    let ready = ReadySource::External(plx_media::ff::ExternalSource::Http(
        plx_data::ott::http_url(url).unwrap()), info, None);
    let _ = runtime.play_ticket.take();
    let _ = runtime.resolver_next.take();
    let mut resources = CatalogAcceptedResources::default();
    start_resolved_with(&mut runtime, &mut app, profile, entry, ready, &mut resources).unwrap();
    let _ = runtime.take_completed_play_target();
    app.ott = Some(runtime);
    settle_pages(&mut app);
    retire_closing_surfaces(&mut app);
    assert_eq!(resources.starts, 1);
    assert_eq!(app.ott.as_ref().unwrap().playing.as_ref().unwrap().entry.id, "a");
    assert_ne!(route::playback_identity(&app.player.session), original_identity);
    assert_eq!(app.pages.top_page(), page, "successor reuses the retained Player instance");
    assert_eq!(app.pages.nav.tabs.stack.depth(), depth, "no duplicate Player or exit PopTo");
    assert_eq!(bridge::player(&app.pages).unwrap().origin, origin);
    assert!(ott_surface(&app).is_none(), "accepted successor closes the old catalog surface");
    assert!(!app.pages.surface_up());
    assert!(app.player.pending_exit.is_none());
    assert!(app.ott.as_ref().unwrap().play_target.is_none());
}

#[path = "programme_remote_keys.rs"]
mod remote_keys;

#[path = "programme_settings_back.rs"]
mod settings_back;

// Publish the mode through the accepted settings path. The inherited native
// engine and held metadata/resolver slots remain inert; no writer is admitted.
fn continuity_catalog_app(mode: plx_machine::machine::UiMode) -> App {
    let mut app = catalog_app();
    let runtime = app.ott.as_mut().unwrap();
    let saved = runtime.interface_mode_candidate(mode);
    runtime.last_checkpoint_ms = runtime.now_ms();
    runtime.saving = true;
    runtime.save_navigation = SaveNavigation::Preserve;
    runtime.save_playback_epoch = runtime.playback_epoch;
    runtime.tx.send(Job::Saved(Ok(saved), None, false, json!({"status":"ok"})))
        .unwrap_or_else(|_| panic!("fixture receiver disappeared"));
    runtime_tick(&mut app);
    assert_eq!(app.bridge.ui_mode(), mode);
    app.ott.as_mut().unwrap().saving = true;
    mount_player(&mut app);
    let now = app.loop_t;
    bridge::player_mut(&mut app.pages).unwrap().hud.extend(now, 60_000);
    idle(&mut app);
    app
}

fn assert_catalog_card_focus(app: &mut App) {
    let focus = app.pages.focus().expect("catalogue focus");
    assert_eq!(app.pages.nav.entry_of_instance(ott_surface(app).unwrap()), Some(focus.entry));
    let parts = app.pages.parts(Tick { ms: app.loop_t, dt_us: 16_000 });
    let split = plx_ui::dispatch::Rig::<bridge::AppHost>::split(&mut app.bridge);
    let cx = parts.cx::<bridge::AppHost>(split.views, split.measure);
    let screen = &app.pages.nav.entry(focus.entry).unwrap().inst.as_ref().unwrap().screen;
    assert_eq!(screen.press_kind(&focus.elem, &cx), Some(plx_ui::screen::ElemKind::Card),
        "Enter must address a real media row, not a navigation header");
}

#[test]
fn classic_enter_current_row_returns_to_the_retained_player_without_restarting() {
    let _serial = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    let mut app = continuity_catalog_app(plx_machine::machine::UiMode::Classic);
    open_more(&mut app);
    choose_catalog(&mut app);
    assert_eq!(app.ott.as_ref().unwrap().selected.as_deref(), Some("c"));
    assert_catalog_card_focus(&mut app);
    let identity = route::playback_identity(&app.player.session);
    let source = route::external_source(&app.player.session);
    let position = player::SHARED.playpos_ns.load(Ordering::Acquire);
    let state = player::state(&app.player.session);
    let owner = app.pages.top_page();
    let depth = app.pages.nav.tabs.stack.depth();
    let runtime = app.ott.as_ref().unwrap();
    let epochs = (runtime.playback_epoch, runtime.play_generation, runtime.generation);
    key(&mut app, Key::Ok);
    let runtime = app.ott.as_ref().unwrap();
    assert!(ott_surface(&app).is_none(), "same-row Enter closes only the catalogue panel");
    assert!(runtime.play_target.is_none() && runtime.resolver_next.is_none());
    assert_eq!((runtime.playback_epoch, runtime.play_generation, runtime.generation), epochs);
    assert_eq!(route::playback_identity(&app.player.session), identity);
    assert_eq!(route::external_source(&app.player.session), source);
    assert_eq!(player::SHARED.playpos_ns.load(Ordering::Acquire), position);
    assert_eq!(player::state(&app.player.session), state);
    assert_eq!(app.pages.top_page(), owner);
    assert_eq!(app.pages.nav.tabs.stack.depth(), depth);
    assert!(app.adapters.player.is_live() && app.player.pending_exit.is_none());
}

#[test]
fn classic_back_from_a_different_highlight_does_not_play_it() {
    let _serial = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    let mut app = continuity_catalog_app(plx_machine::machine::UiMode::Classic);
    open_more(&mut app);
    choose_catalog(&mut app);
    assert_catalog_card_focus(&mut app);
    let initial_focus = app.pages.focus();
    let identity = route::playback_identity(&app.player.session);
    let position = player::SHARED.playpos_ns.load(Ordering::Acquire);
    let epoch = app.ott.as_ref().unwrap().playback_epoch;
    key(&mut app, Key::Down);
    assert_catalog_card_focus(&mut app);
    assert_ne!(app.pages.focus(), initial_focus, "a different media row is highlighted");
    key(&mut app, Key::Back);
    assert!(ott_surface(&app).is_none());
    let runtime = app.ott.as_ref().unwrap();
    assert!(runtime.play_target.is_none() && runtime.resolver_next.is_none());
    assert_eq!(runtime.playing.as_ref().unwrap().selection_id, "c");
    assert_eq!(runtime.playback_epoch, epoch);
    assert_eq!(route::playback_identity(&app.player.session), identity);
    assert_eq!(player::SHARED.playpos_ns.load(Ordering::Acquire), position);
    assert!(app.adapters.player.is_live() && app.player.pending_exit.is_none());
}

#[test]
fn classic_enter_a_different_row_queues_the_existing_playback_path_once() {
    let _serial = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    let mut app = continuity_catalog_app(plx_machine::machine::UiMode::Classic);
    open_more(&mut app);
    choose_catalog(&mut app);
    key(&mut app, Key::Down);
    assert_catalog_card_focus(&mut app);
    let identity = route::playback_identity(&app.player.session);
    let epoch = app.ott.as_ref().unwrap().playback_epoch;
    key(&mut app, Key::Ok);
    let runtime = app.ott.as_ref().unwrap();
    assert_eq!(runtime.play_target.as_ref().map(|request| request.id.as_str()), Some("a"));
    assert_eq!(runtime.resolver_next.as_ref().and_then(|request| request.entry.as_ref())
        .map(|entry| entry.id.as_str()), Some("a"));
    assert_eq!(runtime.playback_epoch, epoch.wrapping_add(1));
    assert_eq!(route::playback_identity(&app.player.session), identity,
        "the old source remains until the existing asynchronous admission completes");
    assert!(ott_surface(&app).is_some());
}

#[test]
fn classic_current_favorite_uses_its_bound_source_without_restarting() {
    let _serial = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    let mut app = continuity_catalog_app(plx_machine::machine::UiMode::Classic);
    let runtime = app.ott.as_mut().unwrap();
    let playing = runtime.playing.as_ref().unwrap();
    let favorite = selection::capture(&playing.profile, &playing.entry).expect("portable fixture");
    let id = format!("favorite:{}", favorite.identity());
    runtime.saved.favorites.toggle(favorite).unwrap();
    runtime.saved.favorites_selected = true;
    runtime.selected = Some(id.clone());
    assert_eq!(runtime.playing.as_ref().unwrap().selection_id, "c",
        "the same current item was started from Channels, not Favorites");
    let (_, bound, entry, favorite) = runtime.prepare_play_request(PlaybackRequest::ordinary(&id, 0)).unwrap();
    assert!(entry.is_none() && favorite.is_some(), "Favorites resolve through their saved binding");
    assert_eq!(bound, runtime.playing.as_ref().unwrap().profile);
    open_more(&mut app);
    choose_catalog(&mut app);
    assert_eq!(app.ott.as_ref().unwrap().page, "favorites");
    let identity = route::playback_identity(&app.player.session);
    let epoch = app.ott.as_ref().unwrap().playback_epoch;
    surface_action(&mut app, OttAction::Play(id));
    retire_closing_surfaces(&mut app);
    let runtime = app.ott.as_ref().unwrap();
    assert!(ott_surface(&app).is_none());
    assert!(runtime.play_target.is_none() && runtime.resolver_next.is_none());
    assert_eq!(runtime.playback_epoch, epoch);
    assert_eq!(route::playback_identity(&app.player.session), identity);
}

#[test]
fn classic_current_return_requires_the_current_source_and_panel_owner() {
    let _serial = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    for changed in ["profile", "media", "identity", "pending", "reply", "covered"] {
        let mut app = continuity_catalog_app(plx_machine::machine::UiMode::Classic);
        open_more(&mut app);
        choose_catalog(&mut app);
        let mut runtime = app.ott.take().unwrap();
        let identity = route::playback_identity(&app.player.session);
        let epoch = runtime.playback_epoch;
        match changed {
            "profile" => runtime.saved.profiles.profiles[0].source.push_str("/changed"),
            "media" => runtime.catalog.entries[0].media = MediaLocator::Http { url: "https://media.invalid/changed.mkv".into() },
            "identity" => runtime.playing.as_mut().unwrap().identity = None,
            "pending" => runtime.play_target = Some(PlaybackRequest::ordinary("a", 0)),
            "reply" => runtime.reply = None,
            "covered" => app.pages.request(MachineId::Nav,
                plx_machine::machine::NavOp::Present(AppArg::PlayerOverlay(
                    plx_screens::player::overlay::PlayerOverlayArg { kind: OverlayKind::Info }))),
            _ => unreachable!(),
        }
        assert!(!player_browse::play_current(&mut runtime, &mut app, "c").unwrap(), "{changed}");
        assert_eq!(runtime.playback_epoch, epoch, "{changed}");
        assert_eq!(route::playback_identity(&app.player.session), identity, "{changed}");
        assert!(ott_surface(&app).is_some(), "{changed}");
        app.ott = Some(runtime);
    }
}

#[test]
fn modern_same_row_retains_the_existing_explicit_play_behavior() {
    let _serial = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    let mut app = continuity_catalog_app(plx_machine::machine::UiMode::Modern);
    open_more(&mut app);
    choose_catalog(&mut app);
    let epoch = app.ott.as_ref().unwrap().playback_epoch;
    surface_action(&mut app, OttAction::Play("c".into()));
    let runtime = app.ott.as_ref().unwrap();
    assert_eq!(runtime.play_target.as_ref().map(|request| request.id.as_str()), Some("c"));
    assert_eq!(runtime.playback_epoch, epoch.wrapping_add(1));
    assert!(ott_surface(&app).is_some());
}

struct NativeCatalogRegistry;
impl Drop for NativeCatalogRegistry {
    fn drop(&mut self) {
        plx_plex::plex::reset_servers_for_test();
        plx_plex::plex::grant::reset_for_test();
    }
}

fn native_catalog_app() -> (App, plx_plex::plex::ServerId) {
    plx_plex::plex::reset_servers_for_test();
    plx_plex::plex::grant::reset_for_test();
    let origin = plx_plex::plex::Origin::parse("https://native-catalog.invalid:443").unwrap();
    let sid = plx_plex::plex::register_pinned_with_client_id("native-catalog", &origin,
        "synthetic-token", None, "synthetic-client", Default::default());
    assert!(plx_plex::plex::set_current(sid));
    let mut app = app_fixture();
    app.running = true;
    let mut init = app.bridge.snapshot_session_init();
    init.epoch = 31;
    init.phase = plx_session::auth::Phase::Ready;
    init.apply_pending = false;
    init.pending_commit = None;
    init.persistence_warning = None;
    app.bridge = bridge::Bridge::for_session_test(init);
    let runtime = app.ott.as_mut().unwrap();
    runtime.loaded = true;
    runtime.public_preferences.accept(None);
    runtime.entropy = Some([7; 16]);
    // Readiness shares the public preference fence. Keep its permit live;
    // no Playing/checkpoint owner exists and all worker slots stay held.
    runtime.catalog_done.store(false, Ordering::Release);
    runtime.metadata_done.store(false, Ordering::Release);
    runtime.resolver_busy = true;
    runtime.plex_refresh_pending = true;
    runtime.last_checkpoint_ms = runtime.now_ms();
    let saved = runtime.interface_mode_candidate(plx_machine::machine::UiMode::Classic);
    runtime.saving = true;
    runtime.save_navigation = SaveNavigation::Preserve;
    runtime.save_playback_epoch = runtime.playback_epoch;
    runtime.tx.send(Job::Saved(Ok(saved), None, false, json!({"status":"ok"})))
        .unwrap_or_else(|_| panic!("fixture receiver disappeared"));
    runtime_tick(&mut app);
    assert_eq!(app.bridge.ui_mode(), plx_machine::machine::UiMode::Classic);
    let audio = plx_data::metadata::Stream { codec: "aac".into(), channels: 2,
        sampling_rate: 48_000, ..Default::default() };
    route::install_plex_track_fixture_for_test(&mut app.player.session, sid, "42", 77,
        route::CarriedAudio::from_stream(&audio, 0));
    route::reset_player_control_for_test(&app.player.session);
    app.adapters.player.install(player::engine::engine_after_reload_for_test());
    assert!(player::pause(&mut app.adapters.player));
    player::swap_state_for_test(player::PlaybackState::Paused);
    player::SHARED.start_queued.store(false, Ordering::Release);
    player::SHARED.seen_frame.store(true, Ordering::Release);
    player::SHARED.playpos_ns.store(7_000_000_000, Ordering::Release);
    let mut leaf = plx_data::metadata::PlayingItem::with_subs(vec![]);
    leaf.sid = sid; leaf.rk = "42".into(); leaf.kind = "movie".into();
    assert!(app.bridge.metadata_run(plx_data::stores::metadata::MetadataCmd::InstallPlaying(Some(leaf))));
    land_plex_detail(&mut app, sid, "42", "movie", "Native current film");
    mount_player(&mut app);
    let now = app.loop_t;
    bridge::player_mut(&mut app.pages).unwrap().hud.extend(now, 60_000);
    app.refresh_hubs = false;
    idle(&mut app);
    assert!(app.ott.as_ref().unwrap().playing.is_none());
    assert!(player_browse::ready(&app));
    (app, sid)
}

fn native_catalog_rows(app: &mut App) -> (String, String) {
    let runtime = app.ott.as_mut().unwrap();
    let profile = runtime.effective_profile().unwrap().clone();
    let current = format!("{}:42", profile.id);
    let next = format!("{}:43", profile.id);
    let rows = [(current.clone(), "42"), (next.clone(), "43")].into_iter().map(|(id, rk)| {
        let mut entry = synthetic_entry();
        entry.id = id; entry.title = format!("Film {rk}");
        entry.media = MediaLocator::Plex { server_id: profile.source.clone(), rating_key: rk.into() };
        entry
    }).collect();
    runtime.catalog_tx.send(CatalogEvent::Batch(runtime.generation,
        Catalog { entries: rows, hierarchy: None })).unwrap();
    runtime.catalog_tx.send(CatalogEvent::Finished(runtime.generation, Ok(()))).unwrap();
    frame(app, vec![]);
    idle(app);
    (current, next)
}

#[test]
fn classic_native_plex_catalogue_is_explicit_and_never_adopts_playback() {
    let _serial = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    let _registry = NativeCatalogRegistry;
    let (mut app, _) = native_catalog_app();
    let saved = serde_json::to_value(&app.ott.as_ref().unwrap().saved).unwrap();
    let identity = route::playback_identity(&app.player.session);
    let epoch = app.ott.as_ref().unwrap().playback_epoch;
    let page = app.pages.top_page();
    assert!(app.ott.as_ref().unwrap().catalog_next.is_none());
    open_more(&mut app);
    assert!(more_keys(&mut app).contains(&5));
    assert!(app.ott.as_ref().unwrap().catalog_next.is_none(), "offering is not loading");
    choose_catalog(&mut app);
    assert!(app.ott.as_ref().unwrap().catalog_next.is_some());
    let _ = native_catalog_rows(&mut app);
    key(&mut app, Key::Back);
    let runtime = app.ott.as_ref().unwrap();
    assert!(runtime.playing.is_none() && runtime.play_target.is_none() && runtime.resolver_next.is_none());
    assert_eq!(runtime.playback_epoch, epoch);
    assert_eq!(route::playback_identity(&app.player.session), identity);
    assert_eq!(app.pages.top_page(), page);
    assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), saved);
    assert!(ott_surface(&app).is_none() && app.adapters.player.is_live());
}

#[test]
fn classic_native_current_row_closes_panel_without_creating_ott_playing() {
    let _serial = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    let _registry = NativeCatalogRegistry;
    let (mut app, _) = native_catalog_app();
    open_more(&mut app); choose_catalog(&mut app);
    let (current, _) = native_catalog_rows(&mut app);
    let identity = route::playback_identity(&app.player.session);
    let epoch = app.ott.as_ref().unwrap().playback_epoch;
    surface_action(&mut app, OttAction::Play(current));
    retire_closing_surfaces(&mut app);
    let runtime = app.ott.as_ref().unwrap();
    assert!(ott_surface(&app).is_none());
    assert!(runtime.playing.is_none() && runtime.play_target.is_none() && runtime.resolver_next.is_none());
    assert_eq!(runtime.playback_epoch, epoch);
    assert_eq!(route::playback_identity(&app.player.session), identity);
    assert_eq!(player::SHARED.playpos_ns.load(Ordering::Acquire), 7_000_000_000);
    assert_eq!(player::state(&app.player.session), player::PlaybackState::Paused);
}

#[test]
fn classic_native_different_row_uses_normal_pending_admission() {
    let _serial = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    let _registry = NativeCatalogRegistry;
    let (mut app, _) = native_catalog_app();
    open_more(&mut app); choose_catalog(&mut app);
    let (_, next) = native_catalog_rows(&mut app);
    let identity = route::playback_identity(&app.player.session);
    surface_action(&mut app, OttAction::Play(next.clone()));
    let runtime = app.ott.as_ref().unwrap();
    assert!(runtime.playing.is_none());
    assert_eq!(runtime.play_target.as_ref().map(|request| request.id.as_str()), Some(next.as_str()));
    assert!(runtime.resolver_next.is_some());
    assert_eq!(route::playback_identity(&app.player.session), identity);
    assert!(ott_surface(&app).is_some());
}

#[test]
fn classic_native_catalogue_offer_requires_ready_installed_source_without_pending_replacement() {
    let _serial = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    let _registry = NativeCatalogRegistry;
    for gate in ["modern", "empty-rk", "request", "session", "stopped", "client", "pending-modal"] {
        let (mut app, sid) = native_catalog_app();
        match gate {
            "modern" => app.bridge.apply_ui_mode(plx_machine::machine::UiMode::Modern),
            "empty-rk" => route::install_plex_identity_for_test(&mut app.player.session, sid, ""),
            "request" => route::install_plex_request_for_test(&mut app.player.session, sid, "43"),
            "session" => {
                let mut init = app.bridge.snapshot_session_init();
                init.phase = plx_session::auth::Phase::Profiles;
                app.bridge = bridge::Bridge::for_session_test(init);
                app.bridge.apply_ui_mode(plx_machine::machine::UiMode::Classic);
            }
            "stopped" => { app.decoder_parked = true; }
            "client" => plx_plex::plex::client_for(sid).unwrap().disable_data_io(),
            "pending-modal" => app.pages.request(MachineId::Nav,
                plx_machine::machine::NavOp::Present(AppArg::PlayerOverlay(
                    plx_screens::player::overlay::PlayerOverlayArg { kind: OverlayKind::Info }))),
            _ => unreachable!(),
        }
        assert!(player_browse::offer(app.ott.as_ref().unwrap(), &app).is_none(), "{gate}");
        assert!(app.ott.as_ref().unwrap().catalog_next.is_none(), "{gate}");
        assert!(app.ott.as_ref().unwrap().playing.is_none(), "{gate}");
    }
}

#[test]
fn classic_native_same_item_replacement_cannot_borrow_the_old_panel_receipt() {
    let _serial = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    let _registry = NativeCatalogRegistry;
    let (mut app, sid) = native_catalog_app();
    open_more(&mut app); choose_catalog(&mut app);
    let (current, _) = native_catalog_rows(&mut app);
    let old = route::playback_identity(&app.player.session);
    route::install_plex_playback_for_test(&mut app.player.session, sid, "42");
    assert_ne!(route::playback_identity(&app.player.session), old);
    let mut runtime = app.ott.take().unwrap();
    assert!(!player_browse::play_current(&mut runtime, &mut app, &current).unwrap());
    assert!(runtime.playing.is_none() && runtime.play_target.is_none());
    assert!(ott_surface(&app).is_some());
    app.ott = Some(runtime);
}


#[test]
fn classic_native_episode_catalogue_uses_installed_source_not_parent_detail() {
    let _serial = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    let _registry = NativeCatalogRegistry;
    for has_playing_metadata in [true, false] {
        let (mut app, sid) = native_catalog_app();
        // A real show-page episode start installs the episode route while the
        // metadata Detail stays on the show. Neither that Detail nor a missing
        // metadata leaf is authority for the already-installed Player source.
        let mut episode = plx_data::metadata::PlayingItem::with_subs(vec![]);
        episode.sid = sid;
        episode.rk = "42".into();
        episode.kind = "episode".into();
        episode.show_rk = "show-parent".into();
        assert!(app.bridge.metadata_run(plx_data::stores::metadata::MetadataCmd::InstallPlaying(
            Some(episode))));
        if !has_playing_metadata {
            assert!(app.bridge.metadata_run(plx_data::stores::metadata::MetadataCmd::InstallPlaying(None)));
        }
        land_plex_detail(&mut app, sid, "show-parent", "show", "Parent show");
        assert_eq!(app.bridge.metadata_view().current().unwrap().rk, "show-parent");
        assert!(app.bridge.metadata_view().current_leaf_now_playing(sid, "42").is_none());
        assert_eq!(route::cur_sid(&app.player.session), sid);
        assert_eq!(route::cur_rk(&app.player.session), "42");
        let identity = route::playback_identity(&app.player.session);
        let epoch = app.ott.as_ref().unwrap().playback_epoch;
        let saved = serde_json::to_value(&app.ott.as_ref().unwrap().saved).unwrap();
        assert!(player_browse::offer(app.ott.as_ref().unwrap(), &app).is_some());
        assert!(app.ott.as_ref().unwrap().catalog_next.is_none(), "offer must not load a catalogue");

        open_more(&mut app);
        assert!(more_keys(&mut app).contains(&5));
        choose_catalog(&mut app);
        let (current, _) = native_catalog_rows(&mut app);
        assert_eq!(app.bridge.metadata_view().current().unwrap().rk, "show-parent");
        surface_action(&mut app, OttAction::Play(current));
        retire_closing_surfaces(&mut app);

        let runtime = app.ott.as_ref().unwrap();
        assert!(ott_surface(&app).is_none());
        assert!(runtime.playing.is_none() && runtime.play_target.is_none() && runtime.resolver_next.is_none());
        assert_eq!(runtime.playback_epoch, epoch);
        assert_eq!(route::playback_identity(&app.player.session), identity);
        assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), saved);
        assert_eq!(player::SHARED.playpos_ns.load(Ordering::Acquire), 7_000_000_000);
        assert_eq!(player::state(&app.player.session), player::PlaybackState::Paused);
        assert_eq!(app.bridge.metadata_view().current().unwrap().rk, "show-parent");
        assert!(app.bridge.metadata_view().current_leaf_now_playing(sid, "42").is_none());
    }
}

#[test]
fn classic_native_pending_display_keeps_only_the_previously_offered_source() {
    let _serial = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    let _registry = NativeCatalogRegistry;
    for change in ["same", "no-prior", "replaced"] {
        let (mut app, sid) = native_catalog_app();
        let offered = player_browse::offer(app.ott.as_ref().unwrap(), &app).unwrap();
        assert_eq!(app.bridge.ott_catalog_offer(), Some(&offered));
        app.pages.request(MachineId::Nav,
            plx_machine::machine::NavOp::Present(AppArg::PlayerOverlay(
                plx_screens::player::overlay::PlayerOverlayArg {
                    kind: OverlayKind::More { page: None },
                })));
        match change {
            "no-prior" => app.bridge.publish_ott_catalog_offer(None),
            "replaced" => {
                route::install_plex_playback_for_test(&mut app.player.session, sid, "42");
                assert_ne!(route::playback_identity(&app.player.session), Some(offered.clone()));
            }
            _ => {}
        }
        assert!(app.pages.has_pending_surface_navigation());
        let runtime = app.ott.as_ref().unwrap();
        assert!(player_browse::offer(runtime, &app).is_none(), "pending actions stay blocked");
        let display = player_browse::display_offer(runtime, &app);
        if change == "same" {
            assert_eq!(display, Some(offered));
        } else {
            assert!(display.is_none(), "{change}");
        }
        assert!(runtime.playing.is_none() && runtime.play_target.is_none()
            && runtime.resolver_next.is_none() && runtime.catalog_next.is_none());
    }
}

#[path = "programme_native_remote.rs"]
mod native_remote;

#[path = "programme_category_return.rs"]
mod category_return;

#[path = "programme_menu_settings.rs"]
mod menu_settings;

#[path = "programme_playback_menu.rs"]
mod playback_menu;

#[path = "programme_menu_categories.rs"]
mod menu_categories;

#[path = "programme_menu_accessibility.rs"]
mod menu_accessibility;
