//! Actual Saved -> Classic screen -> addressed request -> content runner boundary.
//! The Engine is the existing inert host fixture, with real logical/native owners.
//! No decoder worker, Load, protected storage, network or device is started.
use super::*;
use crate::app::player_viewport;
use plx_machine::machine::{Delivery, ScreenEvent, UiMode};
use plx_media::player::{self, ffi_host, viewport, PlaybackState, SHARED, TX};
use plx_media::route;
use plx_screens::registry::{PlayerViewportAction as Action, PlayerViewportRequest};

pub(super) struct Reset {
    state: u8,
    font: plx_machine::machine::FontScale,
}
impl Reset {
    pub(super) fn new() -> Self {
        Self { state: player::swap_state_for_test(PlaybackState::Idle), font: plx_ui::typography::current() }
    }
}
impl Drop for Reset {
    fn drop(&mut self) {
        SHARED.reset_session();
        TX.reset();
        route::reset_player_control_for_test(&route::PlaybackSession::IDLE);
        ffi_host::reset_place_probe_for_test(false);
        ffi_host::force_clocksink_for_test(false);
        plx_gfx::text::set_ui_typography(self.font);
        player::restore_state_for_test(self.state);
    }
}

fn frame(app: &mut App) {
    let tick = next_tick(app);
    bridge::frame(&mut app.pages, &mut app.bridge, tick, Vec::new());
}

fn commit_mode(app: &mut App, mode: UiMode) {
    let runtime = app.ott.as_mut().unwrap();
    let saved = runtime.interface_mode_candidate(mode);
    runtime.saving = true;
    runtime.save_navigation = SaveNavigation::Preserve;
    runtime.save_playback_epoch = runtime.playback_epoch;
    runtime.tx.send(Job::Saved(Ok(saved), None, false, json!({"status":"ok"})))
        .unwrap_or_else(|_| panic!("fixture receiver disappeared"));
    runtime_tick(app);
    for _ in 0..4 { frame(app); }
    assert_eq!(app.bridge.ui_mode(), mode, "only the accepted Saved publishes mode");
}

fn classic() -> App {
    let mut app = app_fixture();
    let runtime = app.ott.as_mut().unwrap();
    runtime.loaded = true;
    runtime.entropy = Some([1; 16]);
    runtime.write_permit.revoke();
    runtime.catalog_done.store(false, Ordering::Release);
    runtime.metadata_done.store(false, Ordering::Release);
    runtime.resolver_busy = true;
    runtime.plex_refresh_pending = true;
    runtime.continuity.stop(runtime.generation);
    runtime.page = "settings";
    bridge::show_page(&mut app.pages, AppArg::Ott);
    for _ in 0..180 { frame(&mut app); }
    commit_mode(&mut app, UiMode::Classic);
    app
}

pub(super) fn native(app: &mut App) -> viewport::Source {
    assert!(!app.adapters.player.is_live());
    SHARED.reset_session();
    TX.reset();
    route::reset_player_control_for_test(&app.player.session);
    ffi_host::force_clocksink_for_test(true);
    ffi_host::reset_place_probe_for_test(false);
    let info = plx_media::ff::inspect::MediaInfo {
        tracks: Default::default(), resolved_source: None, hls_playback: None,
        video_codec: "h264".into(), audio_codec: "aac".into(),
        width: 1280, height: 720, fps: 24.0, duration_ns: 60_000_000_000,
        audio_channels: 2, audio_sample_rate: 48_000, audio_bit_depth: 16,
        dovi: Default::default(),
    };
    route::request_external(&mut app.player.session, &info,
        "https://example.invalid/viewport-app.mp4", "Viewport", "Fixture",
        plx_platform::devcaps::assumed_shared()).unwrap();
    let transaction = route::pending_route_start().expect("real prepared route");
    let attempt = route::claim_route_start_attempt(transaction).expect("real native attempt");
    assert!(route::settle_route_start(&mut app.player.session, attempt, route::RouteStartResult::Started));
    let epoch = SHARED.begin_native_session().expect("native epoch");
    assert!(SHARED.mark_native_load_returned(epoch, None).is_some());
    let mut engine = player::engine::engine_after_reload_for_test();
    engine.native_epoch = epoch;
    engine.route_start = attempt;
    app.adapters.player.install(engine);
    SHARED.publish_video_raster(1280, 720);
    player::swap_state_for_test(PlaybackState::Playing);
    viewport::capture_source(&mut app.adapters.player, &app.player.session).expect("real ready Source")
}

fn emitted(app: &mut App) -> PlayerViewportRequest {
    for _ in 0..8 {
        player_viewport::sync(app);
        frame(app);
        if let Some(request) = app.bridge.take_player_reqs().into_iter().find_map(|request| match request {
            PlayerReq::ViewportAddressed(request) => Some(request), _ => None,
        }) { return request; }
    }
    panic!("the real Classic screen did not emit a viewport request");
}

fn content(app: &mut App) {
    let tick = next_tick(app);
    let fr = run::Frame {
        ctrl: plx_appkit::player_hud::slot(&app.player.session, app.bridge.metadata_view()),
        now: tick.ms, dt: 0.016, underlay_moving: false, player: false,
        present: false, deferred: false, rn: "ott",
    };
    content::content_requests(app, &fr);
}

fn deliver(app: &mut App, from: MachineId, request: PlayerViewportRequest, copies: usize) {
    for _ in 0..copies {
        app.pages.emit(from, Fx::App(AppFx::Player(PlayerReq::ViewportAddressed(request.clone()))));
    }
    frame(app);
    content(app);
}

fn accepted(app: &mut App) -> PlayerViewportRequest {
    let request = emitted(app);
    assert!(matches!(request.action, Action::Set(_)));
    deliver(app, request.reply.to, request.clone(), 1);
    assert_eq!(ffi_host::place_calls_for_test().len(), 1);
    assert_eq!(ffi_host::place_calls_for_test()[0].1, 1);
    frame(app); // consume the actual addressed result
    request
}

#[test]
fn classic_settings_places_the_existing_player_and_preserves_pause_position_identity() {
    let _serial = plx_base::testlock::serial(); let _reset = Reset::new();
    let mut app = classic(); let source = native(&mut app);
    SHARED.playpos_ns.store(17_000_000_000, Ordering::Relaxed);
    TX.commit_paused(true);
    let plays = ffi_host::play_calls_for_test();
    accepted(&mut app);
    assert_eq!(app.ott.as_ref().unwrap().page, "settings");
    assert_eq!(viewport::capture_source(&mut app.adapters.player, &app.player.session), Some(source));
    assert_eq!(SHARED.playpos_ns.load(Ordering::Relaxed), 17_000_000_000);
    assert!(player::transport::paused());
    assert_eq!(ffi_host::play_calls_for_test(), plays);
    assert!(player_viewport::composite(&mut app), "acknowledged Classic aperture must reach host composition");
}

#[test]
fn host_composition_requires_consumed_ack_and_disappears_on_editor_entry() {
    let _serial = plx_base::testlock::serial(); let _reset = Reset::new();
    let mut app = classic(); native(&mut app);
    let request = emitted(&mut app);
    assert!(!player_viewport::composite(&mut app));
    deliver(&mut app, request.reply.to, request.clone(), 1);
    assert!(!player_viewport::composite(&mut app), "queued Submitted is not an acknowledged aperture");
    frame(&mut app);
    assert!(player_viewport::composite(&mut app));
    app.pages.emit(MachineId::Ott, Fx::Deliver(request.reply.to, Delivery::Screen(ScreenEvent::App(
        plx_screens::registry::AppMsg::OttEditor(std::sync::Arc::new(plx_screens::ott::OttEditor { text_policy: plx_screens::ott::OttTextPolicy::Predictive,
            session: 78, kind: plx_screens::ott::OttEditorKind::Setting, key: "fixture".into(),
            title: "Fixture".into(), value: String::new(), secret: false, limit: 64,
        }))))));
    frame(&mut app);
    assert!(!player_viewport::composite(&mut app), "editor is ineligible even before restore reconciliation");
}

#[test]
fn forged_viewport_emitter_cannot_retarget_the_real_pending_request() {
    let _serial = plx_base::testlock::serial(); let _reset = Reset::new();
    let mut app = classic(); native(&mut app);
    let request = emitted(&mut app);
    for wrong in [MachineId::Player, MachineId::Instance(plx_machine::machine::InstanceId(999_999))] {
        deliver(&mut app, wrong, request.clone(), 1);
    }
    assert!(ffi_host::place_calls_for_test().is_empty());
    deliver(&mut app, request.reply.to, request, 1);
    assert_eq!(ffi_host::place_calls_for_test().len(), 1, "the real pending request remains usable");
}

#[test]
fn duplicate_same_batch_viewport_request_cannot_retry_a_refused_native_call() {
    let _serial = plx_base::testlock::serial(); let _reset = Reset::new();
    let mut app = classic(); native(&mut app);
    let request = emitted(&mut app);
    ffi_host::reset_place_probe_for_test(true);
    deliver(&mut app, request.reply.to, request, 2);
    assert_eq!(ffi_host::place_calls_for_test().len(), 1, "ACK is still queued: exact pending alone cannot deduplicate");
    assert_eq!(ffi_host::place_calls_for_test()[0].1, 0);
}

#[test]
fn editor_retires_queued_classic_placement_before_it_can_reach_native_sink() {
    let _serial = plx_base::testlock::serial(); let _reset = Reset::new();
    let mut app = classic(); native(&mut app);
    let request = emitted(&mut app);
    app.pages.emit(MachineId::Ott, Fx::Deliver(request.reply.to, Delivery::Screen(ScreenEvent::App(
        plx_screens::registry::AppMsg::OttEditor(std::sync::Arc::new(plx_screens::ott::OttEditor { text_policy: plx_screens::ott::OttTextPolicy::Predictive,
            session: 77, kind: plx_screens::ott::OttEditorKind::Setting, key: "fixture".into(),
            title: "Fixture".into(), value: String::new(), secret: false, limit: 64,
        }))))));
    frame(&mut app);
    deliver(&mut app, request.reply.to, request, 1);
    assert!(ffi_host::place_calls_for_test().is_empty());
}

#[test]
fn an_unmounted_owner_restores_fullscreen_and_retains_a_refused_restore_obligation() {
    let _serial = plx_base::testlock::serial(); let _reset = Reset::new();
    let mut app = classic(); native(&mut app);
    let request = accepted(&mut app);
    let MachineId::Instance(owner) = request.reply.to else { panic!("instance owner") };
    app.pages.reset_for_profile();
    for _ in 0..180 { frame(&mut app); }
    assert!(app.pages.nav.instance(owner).is_none(), "the source screen is actually unmounted");
    ffi_host::reset_place_probe_for_test(true);
    player_viewport::sync(&mut app);
    assert_eq!(ffi_host::place_calls_for_test(), vec![([1280, 720, 0, 0, 1920, 1080], 0)]);
    ffi_host::reset_place_probe_for_test(false);
    player_viewport::sync(&mut app);
    assert_eq!(ffi_host::place_calls_for_test(), vec![([1280, 720, 0, 0, 1920, 1080], 1)]);
    player_viewport::sync(&mut app);
    assert_eq!(ffi_host::place_calls_for_test().len(), 1, "completed restoration is not repeated");
}

#[test]
fn current_unready_native_owner_does_not_erase_fullscreen_restoration() {
    let _serial = plx_base::testlock::serial(); let _reset = Reset::new();
    let mut app = classic(); native(&mut app); accepted(&mut app);
    app.window_activity.event(0x103);
    app.adapters.player.engine().unwrap().preview_abandon = true;
    ffi_host::reset_place_probe_for_test(false);
    player_viewport::sync(&mut app);
    assert!(ffi_host::place_calls_for_test().is_empty());
    app.adapters.player.engine().unwrap().preview_abandon = false;
    player_viewport::sync(&mut app);
    assert_eq!(ffi_host::place_calls_for_test(), vec![([1280, 720, 0, 0, 1920, 1080], 1)]);
}

#[test]
fn replaced_native_epoch_refuses_queued_source_and_publishes_fresh_capability() {
    let _serial = plx_base::testlock::serial(); let _reset = Reset::new();
    let mut app = classic(); native(&mut app);
    let old = emitted(&mut app);
    let old_epoch = app.adapters.player.engine().unwrap().native_epoch;
    assert!(SHARED.retire_native_session(old_epoch));
    let next_epoch = SHARED.begin_native_session().unwrap();
    assert!(SHARED.mark_native_load_returned(next_epoch, None).is_some());
    app.adapters.player.engine().unwrap().native_epoch = next_epoch;
    deliver(&mut app, old.reply.to, old.clone(), 1);
    assert!(ffi_host::place_calls_for_test().is_empty());
    let current = emitted(&mut app);
    assert_ne!(current.source, old.source);
    assert!(current.reply.req.0 > old.reply.req.0);
    deliver(&mut app, current.reply.to, current, 1);
    assert_eq!(ffi_host::place_calls_for_test().len(), 1);
}

#[test]
fn delayed_release_cannot_reset_the_same_instances_newer_placement_claim() {
    let _serial = plx_base::testlock::serial(); let _reset = Reset::new();
    let mut app = classic(); native(&mut app);
    let first = accepted(&mut app);
    commit_mode(&mut app, UiMode::Modern);
    let release = app.bridge.take_player_reqs().into_iter().find_map(|request| match request {
        PlayerReq::ViewportAddressed(request) if matches!(request.action, Action::Release { .. }) => Some(request),
        _ => None,
    }).expect("real mode change emits the release");
    assert_eq!(release.action, Action::Release { claim: first.reply.req });
    deliver(&mut app, release.reply.to, release.clone(), 1);
    frame(&mut app);
    commit_mode(&mut app, UiMode::Classic);
    ffi_host::reset_place_probe_for_test(false);
    let current = accepted(&mut app);
    assert_eq!(current.reply.to, first.reply.to);
    assert!(current.reply.req.0 > release.reply.req.0);
    ffi_host::reset_place_probe_for_test(false);
    deliver(&mut app, release.reply.to, release, 1);
    assert!(ffi_host::place_calls_for_test().is_empty());
}

#[test]
fn preparing_a_replacement_does_not_drop_the_old_decoders_restore_obligation() {
    let _serial = plx_base::testlock::serial(); let _reset = Reset::new();
    let mut app = classic(); let original = native(&mut app); accepted(&mut app);
    let preparing = route::begin_route_start().expect("old route is still recoverable");
    ffi_host::reset_place_probe_for_test(false);
    player_viewport::sync(&mut app);
    assert_eq!(ffi_host::place_calls_for_test(), vec![([1280, 720, 0, 0, 1920, 1080], 1)]);
    assert!(route::reject_route_start_preparation(preparing));
    assert_eq!(viewport::capture_source(&mut app.adapters.player, &app.player.session), Some(original));
}


fn modern_catalogue() -> App {
    let mut app=classic();
    commit_mode(&mut app,UiMode::Modern);
    let owner=crate::app::ott_runtime::current_ui_owner(&app).expect("mounted OTT page");
    let view=plx_screens::ott::OttView {
        page_id:"channels:fixture".into(),revision:1_000_000,
        status:"Loaded 1 / 1".into(),selected_row:Some("fixture".into()),
        rows:vec![plx_screens::ott::OttRow {
            current_programme: None, id:"fixture".into(),title:"Current file".into(),detail:"Local videos".into(),
            description:"Complete description remains in the reader.".into(),
            poster:None,content_title:None,guide:None,guide_available:false,records_available:false,media:None,
            action:plx_screens::ott::OttAction::Play("fixture".into()),enabled:true,favorite:false,
        }],..Default::default()
    };
    app.pages.emit(MachineId::Ott,Fx::Deliver(MachineId::Instance(owner),Delivery::Screen(ScreenEvent::App(
        plx_screens::registry::AppMsg::OttSnapshot(std::sync::Arc::new(view))))));
    frame(&mut app);
    app
}

#[test]
fn modern_catalogue_reuses_the_current_player_and_waits_for_exact_ack() {
    let _serial=plx_base::testlock::serial();let _reset=Reset::new();
    let mut app=modern_catalogue();let source=native(&mut app);
    SHARED.playpos_ns.store(17_000_000_000,Ordering::Relaxed);TX.commit_paused(true);
    let identity=route::playback_identity(&app.player.session);
    let plays=ffi_host::play_calls_for_test();
    let request=emitted(&mut app);
    assert!(matches!(request.action,Action::Set(viewport::Target::Inline{..})));
    assert!(!player_viewport::composite(&mut app));
    deliver(&mut app,request.reply.to,request.clone(),1);
    assert!(!player_viewport::composite(&mut app),"unconsumed ACK must not punch through metadata");
    frame(&mut app);assert!(player_viewport::composite(&mut app));
    assert_eq!(viewport::capture_source(&mut app.adapters.player,&app.player.session),Some(source));
    assert_eq!(route::playback_identity(&app.player.session),identity);
    assert_eq!(SHARED.playpos_ns.load(Ordering::Relaxed),17_000_000_000);
    assert!(player::transport::paused());assert_eq!(ffi_host::play_calls_for_test(),plays);
    app.pages.emit(MachineId::Ott,Fx::Deliver(request.reply.to,Delivery::Screen(ScreenEvent::Cover)));
    frame(&mut app);assert!(!player_viewport::composite(&mut app));
    ffi_host::reset_place_probe_for_test(false);player_viewport::sync(&mut app);
    assert_eq!(ffi_host::place_calls_for_test(),vec![([1280,720,0,0,1920,1080],1)]);
}

#[test]
fn modern_catalogue_discards_late_ack_after_source_replacement() {
    let _serial=plx_base::testlock::serial();let _reset=Reset::new();
    let mut app=modern_catalogue();native(&mut app);
    let old=emitted(&mut app);
    deliver(&mut app,old.reply.to,old.clone(),1); // ACK is queued, not consumed.
    let old_epoch=app.adapters.player.engine().unwrap().native_epoch;
    assert!(SHARED.retire_native_session(old_epoch));
    let epoch=SHARED.begin_native_session().unwrap();
    assert!(SHARED.mark_native_load_returned(epoch,None).is_some());
    app.adapters.player.engine().unwrap().native_epoch=epoch;
    player_viewport::sync(&mut app);frame(&mut app);
    assert!(!player_viewport::composite(&mut app),"old ACK cannot expose the replacement source");
    ffi_host::reset_place_probe_for_test(false);
    let current=emitted(&mut app);assert_ne!(current.source,old.source);
    deliver(&mut app,current.reply.to,current,1);frame(&mut app);
    assert!(player_viewport::composite(&mut app));
}

#[test]
fn modern_catalogue_releases_preview_when_large_type_no_longer_fits() {
    let _serial=plx_base::testlock::serial();let _reset=Reset::new();
    let mut app=modern_catalogue();native(&mut app);let request=accepted(&mut app);
    app.bridge.apply_ui_font_scale(200).expect("supported UI scale");
    assert_eq!(app.bridge.ui_font_scale_percent(), 200, "layout measurement receives the new scale");
    assert_eq!(plx_ui::typography::current().percent(), 200, "paint receives the same scale");
    // A long title tests measured text, not an arbitrary font-scale threshold.
    let view=plx_screens::ott::OttView {
        page_id:"channels:fixture".into(),revision:1_000_001,
        status:"Loaded 1 / 1".into(),selected_row:Some("fixture".into()),
        rows:vec![plx_screens::ott::OttRow {
            current_programme: None, id:"fixture".into(),title:"A long programme title with several translated words ".repeat(4),
            detail:"Local videos".into(),description:"Complete description".into(),
            poster:None,content_title:None,guide:None,guide_available:false,records_available:false,media:None,
            action:plx_screens::ott::OttAction::Play("fixture".into()),enabled:true,favorite:false,
        }],..Default::default()
    };
    app.pages.emit(MachineId::Ott,Fx::Deliver(request.reply.to,Delivery::Screen(ScreenEvent::App(
        plx_screens::registry::AppMsg::OttSnapshot(std::sync::Arc::new(view))))));
    frame(&mut app);assert!(!player_viewport::composite(&mut app));
    ffi_host::reset_place_probe_for_test(false);player_viewport::sync(&mut app);
    assert_eq!(ffi_host::place_calls_for_test(),vec![([1280,720,0,0,1920,1080],1)]);
}

#[test]
fn modern_guide_growth_retires_the_acknowledged_claim_and_ignores_its_late_ack() {
    fn screen(app: &App, owner: plx_machine::machine::InstanceId) -> &plx_screens::ott::OttScreen {
        app.pages.nav.instance(owner).unwrap().screen.as_any().unwrap().downcast_ref().unwrap()
    }
    let _serial=plx_base::testlock::serial();let _reset=Reset::new();
    let mut app=modern_catalogue();
    // Keep the same large font before and after admission: only new guide text
    // changes the measured layout, not a font, title, source or page transition.
    app.bridge.apply_ui_font_scale(200).expect("supported UI scale");
    frame(&mut app);
    let source=native(&mut app);
    SHARED.playpos_ns.store(17_000_000_000,Ordering::Relaxed);TX.commit_paused(true);
    let identity=route::playback_identity(&app.player.session);
    let plays=ffi_host::play_calls_for_test();
    let first=accepted(&mut app);
    let MachineId::Instance(owner)=first.reply.to else {panic!("addressed screen owner")};
    let Action::Set(target)=first.action else {panic!("actual initial placement")};
    assert!(screen(&app,owner).has_submitted_viewport(&source,first.reply.req,target));
    assert!(player_viewport::composite(&mut app),"the original ACK is really consumed");

    let view=plx_screens::ott::OttView {
        page_id:"channels:fixture".into(),revision:1_000_001,
        status:"Loaded 1 / 1".into(),selected_row:Some("fixture".into()),
        rows:vec![plx_screens::ott::OttRow {
            current_programme: None, id:"fixture".into(),title:"Current file".into(),detail:"Local videos".into(),
            description:"Complete description remains in the reader.".into(),
            poster:None,content_title:None,guide_available:false,records_available:false,media:None,
            guide:Some(plx_screens::ott::OttNowNext { now_s:1_700_000_010,
                current:Some(plx_screens::ott::OttGuideProgramme {
                    title:"A long current programme title with several translated words ".repeat(4),
                    description:"Current programme synopsis".into(),start_s:1_700_000_000,end_s:1_700_003_600,
                }),
                next:Some(plx_screens::ott::OttGuideProgramme {
                    title:"A long next programme title with several translated words ".repeat(4),
                    description:"Next programme synopsis".into(),start_s:1_700_003_600,end_s:1_700_007_200,
                }),
            }),
            action:plx_screens::ott::OttAction::Play("fixture".into()),enabled:true,favorite:false,
        }],..Default::default()
    };
    app.pages.emit(MachineId::Ott,Fx::Deliver(first.reply.to,Delivery::Screen(ScreenEvent::App(
        plx_screens::registry::AppMsg::OttSnapshot(std::sync::Arc::new(view))))));
    frame(&mut app);
    assert!(screen(&app,owner).viewport_target().is_none(),"guide text needs the video area");
    assert!(!screen(&app,owner).has_submitted_viewport(&source,first.reply.req,target));
    assert!(!player_viewport::composite(&mut app),"layout retirement applies before native reconciliation");
    let release=app.bridge.take_player_reqs().into_iter().find_map(|request| match request {
        PlayerReq::ViewportAddressed(request) if matches!(request.action,Action::Release {claim} if claim==first.reply.req) =>Some(request),
        _=>None,
    }).expect("the screen releases its acknowledged claim");
    assert_eq!(release.source,source);assert_eq!(release.reply.to,first.reply.to);
    assert!(release.reply.req.0>first.reply.req.0);
    assert!(screen(&app,owner).accepts_viewport_request(&release));

    app.pages.emit(MachineId::Player,Fx::Deliver(first.reply.to,Delivery::Screen(ScreenEvent::Async(
        first.reply.req,plx_screens::registry::AppMsg::PlayerViewportReply {
            outcome:plx_screens::registry::PlayerViewportOutcome::Submitted,
        }))));
    frame(&mut app);
    assert!(screen(&app,owner).accepts_viewport_request(&release),"late ACK cannot consume the newer release");
    assert!(!screen(&app,owner).has_submitted_viewport(&source,first.reply.req,target));
    assert!(!player_viewport::composite(&mut app));

    ffi_host::reset_place_probe_for_test(false);player_viewport::sync(&mut app);frame(&mut app);
    assert_eq!(ffi_host::place_calls_for_test(),vec![([1280,720,0,0,1920,1080],1)]);
    assert!(!screen(&app,owner).accepts_viewport_request(&release),"Source retirement revokes the old release");
    deliver(&mut app,first.reply.to,first,1);
    assert_eq!(ffi_host::place_calls_for_test().len(),1,"an old Set cannot reopen the retired aperture");
    assert!(!player_viewport::composite(&mut app));
    assert_eq!(viewport::capture_source(&mut app.adapters.player,&app.player.session),Some(source));
    assert_eq!(route::playback_identity(&app.player.session),identity);
    assert_eq!(SHARED.playpos_ns.load(Ordering::Relaxed),17_000_000_000);
    assert!(player::transport::paused());assert_eq!(ffi_host::play_calls_for_test(),plays);
}

#[test]
fn root_exit_prompt_keeps_viewport_source_and_ack_live_through_cancel_fade() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset::new();
    let mut app = classic();
    let owner = crate::app::ott_runtime::current_ui_owner(&app).expect("mounted OTT page");
    let to = MachineId::Instance(owner);
    let mut view = plx_screens::ott::OttView {
        page_id: "providers".into(), revision: 1_000_000, ..Default::default()
    };
    app.pages.emit(MachineId::Ott, Fx::Deliver(to, Delivery::Screen(ScreenEvent::App(
        AppMsg::OttSnapshot(std::sync::Arc::new(view.clone()))))));
    frame(&mut app);
    app.pages.emit(MachineId::Ott, Fx::Deliver(to, Delivery::Screen(ScreenEvent::App(
        AppMsg::OttExitPrompt { token: 71 }))));
    for _ in 0..60 { frame(&mut app); }
    let source = native(&mut app);
    let first = emitted(&mut app);
    assert_eq!(first.source, source, "Source delivered while the prompt is open must be consumed");
    assert!(!player_viewport::composite(&mut app));
    deliver(&mut app, first.reply.to, first, 1);
    frame(&mut app);
    assert!(player_viewport::composite(&mut app), "open prompt must consume the actual placement ACK");

    // A different native epoch creates a genuine new Source, without a decoder or network.
    let old_epoch = app.adapters.player.engine().unwrap().native_epoch;
    assert!(SHARED.retire_native_session(old_epoch));
    let epoch = SHARED.begin_native_session().unwrap();
    assert!(SHARED.mark_native_load_returned(epoch, None).is_some());
    app.adapters.player.engine().unwrap().native_epoch = epoch;
    let next = emitted(&mut app);
    assert_ne!(next.source, source);
    let tick = next_tick(&mut app);
    bridge::frame(&mut app.pages, &mut app.bridge, tick,
        vec![plx_ui::fixture::key(plx_machine::machine::Key::Back, tick)]);
    assert!(app.bridge.take_ott_reqs().iter().any(|request| matches!(request,
        plx_screens::ott::OttOut::Action { reply, action: OttAction::ExitAnswer { token: 71, accept: false } }
            if *reply == owner)), "Back must cancel the current question before the next ACK");
    deliver(&mut app, next.reply.to, next, 1);
    frame(&mut app);
    assert!(player_viewport::composite(&mut app), "Cancel fade must not strand an actual placement ACK");

    view.page_id = "channels".into();
    view.revision += 1;
    app.pages.emit(MachineId::Ott, Fx::Deliver(to, Delivery::Screen(ScreenEvent::App(
        AppMsg::OttSnapshot(std::sync::Arc::new(view))))));
    for _ in 0..60 { frame(&mut app); }
    player_viewport::sync(&mut app);
    frame(&mut app);
    if !player_viewport::composite(&mut app) {
        let request = emitted(&mut app);
        deliver(&mut app, request.reply.to, request, 1);
        frame(&mut app);
    }
    assert!(player_viewport::composite(&mut app), "the following catalogue still owns a usable viewport");
}
