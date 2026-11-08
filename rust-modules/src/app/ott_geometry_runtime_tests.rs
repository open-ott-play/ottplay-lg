//! Actual More row -> addressed Bridge queue -> native geometry runner tests.
//! Sources come from the existing inert native fixture; picture evidence comes
//! from the media test-support guard, not a fabricated opaque Source.
use super::*;
use crate::app::player_geometry;
use plx_machine::machine::{InputEvent, Key, UiMode};
use plx_media::{
    player::{
        self, ffi_host,
        video_geometry::{PictureMode, Transform, Zoom},
        viewport as media_viewport, SHARED, TX,
    },
    route,
};
use plx_screens::{
    player::overlay::{OverlayKind, Panel},
    registry::PlayerGeometryRequest,
};

struct Paused(bool);
impl Paused {
    fn capture() -> Self {
        Self(player::transport::paused())
    }
}
impl Drop for Paused {
    fn drop(&mut self) {
        TX.commit_paused(self.0);
    }
}

fn fixture(mode: UiMode) -> App {
    let mut app = ready_app_fixture();
    let runtime = app.ott.as_mut().unwrap();
    runtime.write_permit.revoke();
    runtime.catalog_done.store(false, Ordering::Release);
    runtime.metadata_done.store(false, Ordering::Release);
    runtime.resolver_busy = true;
    runtime.plex_refresh_pending = true;
    runtime.continuity.stop(runtime.generation);
    let saved = runtime.interface_mode_candidate(mode);
    runtime.saving = true;
    runtime.save_navigation = SaveNavigation::Preserve;
    runtime.save_playback_epoch = runtime.playback_epoch;
    runtime
        .tx
        .send(Job::Saved(Ok(saved), None, false, json!({"status":"ok"})))
        .unwrap_or_else(|_| panic!("fixture receiver disappeared"));
    runtime_tick(&mut app);
    assert_eq!(app.bridge.ui_mode(), mode);
    mount_player(&mut app);
    for _ in 0..180 {
        dispatch(&mut app, Vec::new());
    }
    viewport::native(&mut app);
    app.bridge.publish_playback(&app.player.session, true);
    app
}

fn dispatch(app: &mut App, input: Vec<InputEvent<u32>>) {
    player_geometry::sync(app);
    let tick = next_tick(app);
    bridge::frame(&mut app.pages, &mut app.bridge, tick, input);
}

fn content(app: &mut App) {
    let frame = run::Frame {
        ctrl: plx_appkit::player_hud::slot(&app.player.session, app.bridge.metadata_view()),
        now: app.loop_t,
        dt: 0.016,
        underlay_moving: false,
        player: true,
        present: false,
        deferred: false,
        rn: "player",
    };
    content::content_requests(app, &frame);
    player_geometry::sync(app);
}

fn settle(app: &mut App) {
    for _ in 0..180 {
        dispatch(app, Vec::new());
        content(app);
    }
}

fn key(app: &mut App, key: Key) {
    let tick = Tick {
        ms: app.loop_t.wrapping_add(16),
        dt_us: 16_000,
    };
    dispatch(app, bridge::script_key(key, tick));
    content(app);
    settle(app);
}

fn open_more(app: &mut App) {
    player_geometry::sync(app);
    bridge::open_player_overlay(
        &app.player.session,
        app.bridge.metadata_view(),
        &mut app.pages,
        OverlayKind::More { page: None },
    );
    settle(app);
    assert_eq!(
        bridge::player_overlay_kind(&app.pages),
        Some(OverlayKind::More { page: None })
    );
}

fn menu(app: &mut App) -> &plx_appkit::more_menu::MoreMenuState {
    let Panel::More(menu) = bridge::player_overlay_mut(&mut app.pages)
        .expect("live More")
        .panel()
    else {
        panic!("More panel")
    };
    menu
}

fn focus(app: &mut App, wanted: u32) {
    assert!(menu(app).keys().contains(&wanted), "offered row {wanted}");
    for _ in 0..20 {
        let menu = menu(app);
        if menu.keys().get(menu.sel() as usize) == Some(&wanted) {
            return;
        }
        key(app, Key::Down);
    }
    panic!("D-pad did not reach row {wanted}");
}

fn choose(app: &mut App, wanted: u32) {
    focus(app, wanted);
    key(app, Key::Ok);
}

fn pending(app: &mut App, wanted: u32) -> PlayerGeometryRequest {
    focus(app, wanted);
    let tick = Tick {
        ms: app.loop_t.wrapping_add(16),
        dt_us: 16_000,
    };
    dispatch(app, bridge::script_key(Key::Ok, tick));
    let mut requests =
        app.bridge
            .take_player_reqs()
            .into_iter()
            .filter_map(|request| match request {
                PlayerReq::GeometryAddressed(request) => Some(request),
                _ => None,
            });
    let request = requests.next().expect("actual More choice emits geometry");
    assert!(requests.next().is_none());
    assert!(bridge::player_overlay_mut(&mut app.pages)
        .unwrap()
        .accepts_geometry_request(&request));
    request
}

fn deliver(app: &mut App, request: &PlayerGeometryRequest, from: MachineId, count: usize) {
    for _ in 0..count {
        app.pages.emit(
            from,
            Fx::App(AppFx::Player(PlayerReq::GeometryAddressed(request.clone()))),
        );
    }
    dispatch(app, Vec::new());
    content(app);
}

fn state(app: &mut App) -> media_viewport::ContentState {
    let source =
        media_viewport::capture_source(&mut app.adapters.player, &app.player.session).unwrap();
    media_viewport::content_state(&mut app.adapters.player, &app.player.session, &source).unwrap()
}

fn accept_fill(app: &mut App) -> PlayerGeometryRequest {
    open_more(app);
    choose(app, 6);
    let request = pending(app, 31);
    deliver(app, &request, request.reply.to, 1);
    dispatch(app, Vec::new());
    assert_eq!(state(app).transform.mode, PictureMode::Fill);
    request
}

fn close_more(app: &mut App) {
    bridge::dismiss_player_overlays(&mut app.pages);
    settle(app);
}

/// End this inert source through the real no-Engine media stop path. There is no
/// native object/worker to unload; taking the fixture Engine must still be followed
/// by retiring its Session URL before another logical admission. Deliberately keep
/// the screen owner mounted so stale-menu tests exercise its identity fence.
fn retire_inert_source(app: &mut App) {
    drop(app.adapters.player.take().expect("inert source"));
    route::cancel_play(&mut app.player.session);
    player::stop_bufferfeed(&mut app.player.session, &mut app.adapters.player);
    assert!(!route::has_url(&app.player.session));
}

/// No worker is present in this fixture. Replace its inert Engine with the same
/// real route attempt and logical identity but a fresh native epoch, as a reload
/// does. The Source must change; the application must not key intent by it.
fn reload(app: &mut App) -> media_viewport::Source {
    let old = app.adapters.player.take().unwrap();
    let attempt = old.route_start;
    drop(old);
    player_geometry::sync(app); // the no-engine gap must not erase accepted intent
    SHARED.reset_session_for_reload();
    let epoch = SHARED.begin_native_session().unwrap();
    assert!(SHARED.mark_native_load_returned(epoch, None).is_some());
    let mut engine = player::engine::engine_after_reload_for_test();
    engine.native_epoch = epoch;
    engine.route_start = attempt;
    app.adapters.player.install(engine);
    SHARED.publish_video_raster(1280, 720);
    media_viewport::capture_source(&mut app.adapters.player, &app.player.session).unwrap()
}

#[test]
fn geometry_more_commits_once_in_both_modes_without_changing_transport() {
    let _serial = plx_base::testlock::serial();
    let _paused = Paused::capture();
    for mode in [UiMode::Classic, UiMode::Modern] {
        let _reset = viewport::Reset::new();
        let _picture = media_viewport::picture_for_test();
        let mut app = fixture(mode);
        open_more(&mut app);
        choose(&mut app, 6);
        SHARED.playpos_ns.store(17_000_000_000, Ordering::Relaxed);
        TX.commit_paused(true);
        let identity = route::playback_identity(&app.player.session);
        let plays = ffi_host::play_calls_for_test();
        let request = pending(&mut app, 31);
        assert_eq!(
            state(&mut app).transform,
            Transform::default(),
            "pending choice has not applied"
        );
        deliver(&mut app, &request, MachineId::Player, 1);
        assert!(
            ffi_host::place_calls_for_test().is_empty(),
            "forged emitter cannot apply"
        );
        deliver(&mut app, &request, request.reply.to, 2);
        assert_eq!(
            ffi_host::place_calls_for_test().len(),
            1,
            "same-drain duplicate applies once"
        );
        dispatch(&mut app, Vec::new());
        assert!(!bridge::player_overlay_mut(&mut app.pages)
            .unwrap()
            .accepts_geometry_request(&request));
        assert_eq!(
            state(&mut app).transform,
            Transform {
                mode: PictureMode::Fill,
                zoom: Zoom::X100
            }
        );
        key(&mut app, Key::Back); // inner Picture picker, preserving More
        choose(&mut app, 7);
        choose(&mut app, 33);
        assert_eq!(
            state(&mut app).transform,
            Transform {
                mode: PictureMode::Fill,
                zoom: Zoom::X125
            }
        );
        assert_eq!(route::playback_identity(&app.player.session), identity);
        assert_eq!(SHARED.playpos_ns.load(Ordering::Relaxed), 17_000_000_000);
        assert!(player::transport::paused());
        assert_eq!(ffi_host::play_calls_for_test(), plays);
    }
}

#[test]
fn geometry_closed_owner_and_changed_source_cannot_apply_stale_choice() {
    let _serial = plx_base::testlock::serial();
    let _paused = Paused::capture();
    let _reset = viewport::Reset::new();
    let _picture = media_viewport::picture_for_test();
    let mut app = fixture(UiMode::Modern);
    open_more(&mut app);
    choose(&mut app, 6);
    let request = pending(&mut app, 31);
    key(&mut app, Key::Back); // pending Back cancels and dismisses its actual owner
    deliver(&mut app, &request, request.reply.to, 1);
    assert!(ffi_host::place_calls_for_test().is_empty());
    open_more(&mut app);
    choose(&mut app, 6);
    let request = pending(&mut app, 31);
    let next = reload(&mut app);
    assert_ne!(next, request.offer.source);
    deliver(&mut app, &request, request.reply.to, 1);
    assert_eq!(state(&mut app).transform, Transform::default());
    assert!(
        ffi_host::place_calls_for_test().is_empty(),
        "stale epoch cannot touch new engine"
    );
    dispatch(&mut app, Vec::new());
    assert!(
        !menu(&mut app).keys().contains(&31),
        "changed Source terminally revokes old menu"
    );
}

#[test]
fn geometry_reapplies_same_identity_after_first_picture_but_not_new_logical_source() {
    let _serial = plx_base::testlock::serial();
    let _paused = Paused::capture();
    let _reset = viewport::Reset::new();
    let mut picture = media_viewport::picture_for_test();
    let mut app = fixture(UiMode::Classic);
    let request = accept_fill(&mut app);
    close_more(&mut app);
    let identity = route::playback_identity(&app.player.session);
    picture.clear();
    let source = reload(&mut app);
    assert_ne!(source, request.offer.source);
    ffi_host::reset_place_probe_for_test(false);
    for _ in 0..3 {
        player_geometry::sync(&mut app);
    }
    assert!(app.bridge.geometry_offer().is_none());
    assert!(
        ffi_host::place_calls_for_test().is_empty(),
        "NotReady is passive"
    );
    let _decoded = media_viewport::picture_for_test();
    player_geometry::sync(&mut app);
    assert_eq!(route::playback_identity(&app.player.session), identity);
    assert_eq!(state(&mut app).transform.mode, PictureMode::Fill);
    assert_eq!(
        ffi_host::place_calls_for_test().len(),
        1,
        "first picture releases retained intent"
    );
    player_geometry::sync(&mut app);
    assert_eq!(ffi_host::place_calls_for_test().len(), 1);
    retire_inert_source(&mut app);
    viewport::native(&mut app); // a genuinely new logical admission
    app.bridge.publish_playback(&app.player.session, true);
    assert_ne!(route::playback_identity(&app.player.session), identity);
    player_geometry::sync(&mut app);
    assert_eq!(state(&mut app).transform, Transform::default());
    assert!(ffi_host::place_calls_for_test().is_empty());
}

#[test]
fn geometry_failed_automatic_restore_is_bounded_but_explicit_retry_remains_available() {
    let _serial = plx_base::testlock::serial();
    let _paused = Paused::capture();
    let _reset = viewport::Reset::new();
    let _picture = media_viewport::picture_for_test();
    let mut app = fixture(UiMode::Modern);
    accept_fill(&mut app);
    close_more(&mut app);
    reload(&mut app);
    ffi_host::reset_place_probe_for_test(true);
    for _ in 0..4 {
        player_geometry::sync(&mut app);
    }
    assert_eq!(ffi_host::place_calls_for_test().len(), 1);
    assert_eq!(
        state(&mut app).transform,
        Transform::default(),
        "refusal preserves actual state"
    );
    ffi_host::reset_place_probe_for_test(false);
    open_more(&mut app);
    choose(&mut app, 6);
    choose(&mut app, 31);
    assert_eq!(state(&mut app).transform.mode, PictureMode::Fill);
    assert_eq!(
        ffi_host::place_calls_for_test().len(),
        1,
        "explicit choice is not an automatic retry"
    );
}

#[test]
fn geometry_standby_or_strict_policy_rejects_an_already_queued_choice() {
    let _serial = plx_base::testlock::serial();
    let _paused = Paused::capture();
    for strict in [false, true] {
        let _reset = viewport::Reset::new();
        let _picture = media_viewport::picture_for_test();
        let mut app = fixture(UiMode::Modern);
        open_more(&mut app);
        choose(&mut app, 6);
        let request = pending(&mut app, 31);
        if strict {
            let origin = plx_data::ott::favorites::Origin {
                provider: "local".into(),
                profile: "Fixture".into(),
                source_alias: "Fixture".into(),
                subprofile: None,
            };
            let policy = crate::app::ott_kiosk_stored::Stored::local_folder(
                origin,
                true,
                "fixture-folder".into(),
                "a".repeat(64),
            )
            .unwrap();
            app.ott
                .as_mut()
                .unwrap()
                .saved
                .settings
                .insert("nativeKiosk".into(), policy.encode().unwrap());
            assert!(super::super::kiosk_user_keys_blocked(&app));
        } else {
            app.ott_standby = true;
        }
        deliver(&mut app, &request, request.reply.to, 1);
        assert_eq!(state(&mut app).transform, Transform::default());
        assert!(ffi_host::place_calls_for_test().is_empty());
        assert!(app.bridge.geometry_offer().is_none());
    }
}

fn exit_receipt(app: &App) -> player::machine::ExitReceipt {
    player::machine::ExitReceipt {
        cause: player::machine::ExitCause::RequestedExit,
        playback_identity: route::playback_identity(&app.player.session),
        position_ns: 0,
        sid: route::cur_sid(&app.player.session),
        rating_key: route::cur_rk(&app.player.session),
        source: route::external_source(&app.player.session),
    }
}

#[test]
fn geometry_matching_exit_consumption_and_pending_fallback_clear_retained_intent() {
    let _serial = plx_base::testlock::serial();
    let _paused = Paused::capture();
    for with_runtime in [true, false] {
        let _reset = viewport::Reset::new();
        let _picture = media_viewport::picture_for_test();
        let mut app = fixture(UiMode::Modern);
        accept_fill(&mut app);
        close_more(&mut app);
        app.player.pending_exit = Some(exit_receipt(&app));
        if with_runtime {
            let mut runtime = app.ott.take().unwrap();
            runtime.consume_exit(&mut app);
            app.ott = Some(runtime);
            assert!(app.player.pending_exit.is_none());
        } else {
            app.ott = None;
            player_geometry::sync(&mut app);
            app.player.pending_exit = None;
        }
        reload(&mut app);
        ffi_host::reset_place_probe_for_test(false);
        player_geometry::sync(&mut app);
        assert_eq!(state(&mut app).transform, Transform::default());
        assert!(
            ffi_host::place_calls_for_test().is_empty(),
            "exit must not reapply old intent"
        );
    }
}

#[test]
fn geometry_old_exit_receipt_cannot_clear_a_later_logical_choice() {
    let _serial = plx_base::testlock::serial();
    let _paused = Paused::capture();
    let _reset = viewport::Reset::new();
    let _picture = media_viewport::picture_for_test();
    let mut app = fixture(UiMode::Modern);
    accept_fill(&mut app);
    close_more(&mut app);
    let old_exit = exit_receipt(&app);
    retire_inert_source(&mut app);
    viewport::native(&mut app);
    app.bridge.publish_playback(&app.player.session, true);
    player_geometry::sync(&mut app);
    open_more(&mut app);
    choose(&mut app, 7);
    choose(&mut app, 34);
    assert_eq!(
        state(&mut app).transform,
        Transform {
            mode: PictureMode::Fit,
            zoom: Zoom::X150
        }
    );
    // Exercise the exact receipt hook independently of catalog teardown: that
    // unrelated old receipt does not own the newly accepted logical intent.
    app.player_geometry
        .clear_exit(old_exit.playback_identity.as_ref());
    close_more(&mut app);
    reload(&mut app);
    player_geometry::sync(&mut app);
    assert_eq!(
        state(&mut app).transform,
        Transform {
            mode: PictureMode::Fit,
            zoom: Zoom::X150
        }
    );
}

#[test]
fn geometry_waits_for_first_picture_of_same_item_and_revokes_after_picture_loss() {
    let _serial = plx_base::testlock::serial();
    let _paused = Paused::capture();
    let _reset = viewport::Reset::new();
    let mut picture = media_viewport::picture_for_test();
    let mut app = fixture(UiMode::Modern);
    picture.clear();
    open_more(&mut app);
    assert!(!menu(&mut app).keys().contains(&6));
    let mut decoded = media_viewport::picture_for_test();
    settle(&mut app);
    assert!(
        menu(&mut app).keys().contains(&6),
        "first decoded picture may publish the same logical item"
    );
    decoded.clear();
    settle(&mut app);
    assert!(!menu(&mut app).keys().contains(&6));
    let _returned = media_viewport::picture_for_test();
    settle(&mut app);
    assert!(
        !menu(&mut app).keys().contains(&6),
        "a once-captured offer is terminal after loss"
    );
}

#[test]
fn geometry_uncaptured_menu_cannot_adopt_a_different_logical_item() {
    let _serial = plx_base::testlock::serial();
    let _paused = Paused::capture();
    for identity_gap in [false, true] {
        let _reset = viewport::Reset::new();
        let mut picture = media_viewport::picture_for_test();
        let mut app = fixture(UiMode::Modern);
        let original = route::playback_identity(&app.player.session);
        picture.clear();
        open_more(&mut app);
        assert!(!menu(&mut app).keys().contains(&6));
        retire_inert_source(&mut app);
        if identity_gap {
            app.player.session = route::PlaybackSession::IDLE;
            app.bridge.publish_playback(&app.player.session, true);
            settle(&mut app);
        }
        viewport::native(&mut app);
        assert_ne!(route::playback_identity(&app.player.session), original);
        app.bridge.publish_playback(&app.player.session, true);
        let _decoded = media_viewport::picture_for_test();
        settle(&mut app);
        assert!(
            !menu(&mut app).keys().contains(&6),
            "an old menu cannot bind a later logical item"
        );
    }
}

#[test]
fn geometry_held_more_keeps_offer_but_cannot_apply_a_queued_choice() {
    let _serial = plx_base::testlock::serial();
    let _paused = Paused::capture();
    let _reset = viewport::Reset::new();
    let _picture = media_viewport::picture_for_test();
    let mut app = fixture(UiMode::Modern);
    open_more(&mut app);
    choose(&mut app, 6);
    let request = pending(&mut app, 31);
    let MachineId::Instance(owner) = request.reply.to else {
        panic!("actual More instance");
    };
    let surface = app.pages.nav.modals.surfaces.iter_mut()
        .find(|surface| surface.entry.inst.as_ref().is_some_and(|body| body.id == owner))
        .expect("requesting More surface");
    surface.motion.hold_one_frame();
    assert!(surface.motion.held());
    player_geometry::sync(&mut app);
    assert!(app.bridge.geometry_offer() == Some(&request.offer),
        "a presentation hold does not mean the source lost its picture");
    assert!(player_geometry::request(&mut app, PlayerReq::GeometryAddressed(request.clone())).is_none());
    assert_eq!(state(&mut app).transform, Transform::default());
    assert!(ffi_host::place_calls_for_test().is_empty(),
        "a held surface cannot authorize native placement");
    settle(&mut app);
    assert!(!bridge::player_overlay_mut(&mut app.pages).unwrap().accepts_geometry_request(&request));
    choose(&mut app, 31);
    assert_eq!(state(&mut app).transform.mode, PictureMode::Fill);
    assert_eq!(ffi_host::place_calls_for_test().len(), 1,
        "a fresh explicit choice may apply once the surface is visible");
}


#[test]
fn classic_more_pointer_back_cancels_pending_geometry_without_replaying_the_choice() {
    use plx_machine::machine::{Delivery, ScreenEvent};
    let _serial = plx_base::testlock::serial();
    let _paused = Paused::capture();
    let _reset = viewport::Reset::new();
    let _picture = media_viewport::picture_for_test();
    let mut app = fixture(UiMode::Classic);
    open_more(&mut app);
    choose(&mut app, 6);
    let request = pending(&mut app, 31);
    let identity = route::playback_identity(&app.player.session);
    let before = state(&mut app);
    let plays = ffi_host::play_calls_for_test();
    // Other rows remain blocked while the submitted picture choice is pending.
    app.pages.emit(MachineId::Input,
        Fx::Deliver(request.reply.to, Delivery::Screen(ScreenEvent::Activate(31))));
    dispatch(&mut app, Vec::new());
    assert!(app.bridge.take_player_reqs().iter().all(|request|
        !matches!(request, PlayerReq::GeometryAddressed(_))));
    assert!(bridge::player_overlay_mut(&mut app.pages).unwrap().accepts_geometry_request(&request));
    // This is the actual pointer-only key registered by the Classic footer.
    app.pages.emit(MachineId::Input,
        Fx::Deliver(request.reply.to, Delivery::Screen(ScreenEvent::Activate(plx_ui::page_stack::TITLE_KEY))));
    settle(&mut app);
    assert!(bridge::player_overlay_kind(&app.pages).is_none(), "pending Back must retire and dismiss, not pop a page");
    // The old addressed request cannot apply after the pointer retired its owner.
    deliver(&mut app, &request, request.reply.to, 1);
    assert_eq!(state(&mut app), before);
    assert_eq!(route::playback_identity(&app.player.session), identity);
    assert_eq!(ffi_host::play_calls_for_test(), plays);
    assert!(ffi_host::place_calls_for_test().is_empty());
}
