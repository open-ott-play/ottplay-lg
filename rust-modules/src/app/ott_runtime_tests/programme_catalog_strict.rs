//! Actual HUD catalog plus owned remote strict-only kiosk command admission.
//! The revoked sole writer performs no IO; Saved outcomes are modeled below.
use super::*;
use plx_ui::containers::modal::Phase;

#[path = "programme_catalog_standalone.rs"]
mod standalone;

fn pinned_player() -> App {
    let mut app = placeable_catalog_app();
    let owner = app.pages.top_page().unwrap();
    app.pages.emit(MachineId::Instance(owner),
        Fx::App(AppFx::Player(PlayerReq::Transport(Some(false)))));
    frame(&mut app, Vec::new());
    assert!(lifecycle::viewer_paused());
    let mut runtime = app.ott.take().unwrap();
    runtime.entropy = Some([7; 16]);
    let prepared = kiosk_source::prepare(&runtime, &app, Some("1"), false).unwrap();
    runtime.saved = kiosk_source::save_candidate(&runtime, &prepared).unwrap();
    runtime.sync_saved_context();
    assert!(runtime.current_kiosk_pin_owned(&app), "pin owns the exact running source");
    app.ott = Some(runtime);
    app
}

fn pinned_catalog() -> App {
    let mut app = pinned_player();
    open_more(&mut app);
    choose_catalog(&mut app);
    assert!(ott_surface(&app).is_some());
    app
}

struct PendingStrict { job: Job, candidate: Saved, deadline: u64 }
fn request_strict(app: &mut App, lifetime: u64) -> PendingStrict {
    let mut runtime = app.ott.take().unwrap();
    let body = json!({"commands":[],"server_time":1000,"requests":[{
        "id":"00000000000000000000000000000071","action":"kiosk",
        "params":{"mode":"on","strict":true},"expires_at":1020
    }]}).to_string();
    let now = runtime.now_ms();
    let mut invocations = runtime.remote.raw_poll_for_test(&body, now);
    assert_eq!(invocations.len(), 1);
    let mut invocation = invocations.remove(0);
    invocation.deadline_ms = runtime.now_ms().saturating_add(lifetime);
    let deadline = invocation.deadline_ms;
    let mut candidate = runtime.saved.clone();
    let pin = crate::app::ott_kiosk_stored::read(&candidate.settings).unwrap().unwrap();
    candidate.settings.insert("nativeKiosk".into(), pin.with_strict(true).encode().unwrap());
    assert!(!kiosk_source::policy_target_changed(&runtime.saved.settings, &candidate.settings));
    runtime.write_permit.revoke();
    assert!(commands::command(&mut runtime, app, &invocation).is_none(), "actual command enters sole writer");
    assert!(runtime.saving);
    let job = runtime.rx.recv_timeout(Duration::from_secs(5)).expect("revoked writer completion");
    assert!(matches!(&job, Job::Saved(Err(_), Some(_), false, _)));
    app.ott = Some(runtime);
    PendingStrict { job, candidate, deadline }
}

fn complete(app: &mut App, pending: PendingStrict, outcome: &str) {
    let Job::Saved(_, invocation, reload, response) = pending.job else { unreachable!() };
    let job = match outcome {
        "saved" => Job::Saved(Ok(pending.candidate), invocation, reload, response),
        "failed" => Job::Saved(Err("Fixture write refused"), invocation, reload, response),
        "uncertain" => Job::Uncertain(Some(pending.candidate), invocation),
        _ => unreachable!(),
    };
    app.ott.as_ref().unwrap().tx.send(job).unwrap_or_else(|_| panic!("fixture receiver"));
    runtime_tick(app);
}

fn finish_strict_dismissal(app: &mut App) {
    // Observe the real modal spring/unmount boundary; 32 idle frames alone
    // are not proof that an opaque panel completed its closing animation.
    for _ in 0..200 {
        if !app.pages.surface_up() && !app.pages.has_pending_navigation() { break; }
        frame(app, Vec::new());
    }
    assert!(!app.pages.surface_up(), "strict catalog presentation did not retire");
}

fn assert_same_source(app: &mut App, identity: &Option<plx_media::route::PlaybackIdentity>,
    page: Option<plx_machine::machine::InstanceId>, epoch: u64) {
    assert_eq!(&route::playback_identity(&app.player.session), identity);
    assert_eq!(app.pages.top_page(), page);
    assert!(matches!(app.route(), AppArg::Player));
    assert!(app.adapters.player.is_live());
    assert!(lifecycle::viewer_paused(), "strict input policy must not wake paused playback");
    assert_eq!(app.ott.as_ref().unwrap().playback_epoch, epoch);
    assert!(app.ott.as_ref().unwrap().play_target.is_none());
    assert!(app.ott.as_ref().unwrap().resolver_next.is_none());
    assert!(app.player.pending_exit.is_none());
    assert_eq!(player::SHARED.playpos_ns.load(Ordering::Acquire), 7_000_000_000);
}

#[test]
fn strict_saved_closes_real_catalog_and_retires_its_editor_without_restarting() {
    let _serial = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    let _probe = PlacementProbe;
    let mut app = pinned_catalog();
    surface_action(&mut app, OttAction::BeginSearch);
    let editor = app.ott.as_ref().unwrap().edit_session.clone().expect("actual Search editor");
    let catalog = app.pages.nav.entry_of_instance(editor.owner).expect("catalog entry");
    let identity = route::playback_identity(&app.player.session);
    let page = app.pages.top_page();
    let epoch = app.ott.as_ref().unwrap().playback_epoch;
    let pending = request_strict(&mut app, 60_000);
    assert!(ott_surface(&app).is_some(), "admission alone must not dismiss");
    complete(&mut app, pending, "saved");
    let runtime = app.ott.as_ref().unwrap();
    assert!(runtime.reply.is_none());
    assert!(runtime.setup.is_none());
    assert!(runtime.edit_session.is_none());
    assert!(!app.pages.has_pending_navigation(), "modal dismissal must not move the Player page");
    app.pages.emit(MachineId::Instance(editor.owner), Fx::App(AppFx::Ott(OttOut::Action {
        reply: editor.owner, action: OttAction::SubmitSearch { session: editor.id, value: "stale".into() },
    })));
    // Dismiss is parked, but has_pending_navigation deliberately counts only
    // page moves. Let the real dispatcher commit this surface-only operation.
    frame(&mut app, Vec::new());
    let surface = app.pages.nav.modals.surface(catalog).expect("catalog closes normally");
    assert_eq!(surface.phase, Phase::Closing);
    assert_ne!(app.pages.nav.input_owner(), Some(plx_machine::machine::InputOwner::Entry(catalog)));
    assert!(ott_surface(&app).is_none());
    assert_eq!(app.pages.top_page(), page);
    finish_strict_dismissal(&mut app);
    assert!(!app.pages.surface_up());
    assert!(app.ott.as_ref().unwrap().reply.is_none());
    assert!(!app.ott.as_ref().unwrap().saving);
    assert!(app.ott.as_ref().unwrap().rx.try_recv().is_err());
    assert_same_source(&mut app, &identity, page, epoch);
}

fn covered_saved(expired: bool) {
    let mut app = pinned_catalog();
    bridge::open_player_overlay(&app.player.session, app.bridge.metadata_view(),
        &mut app.pages, OverlayKind::More { page: None });
    idle(&mut app);
    assert!(bridge::ott_visible(&app.pages));
    assert_eq!(bridge::player_overlay_kind(&app.pages), Some(OverlayKind::More { page: None }));
    let identity = route::playback_identity(&app.player.session);
    let page = app.pages.top_page();
    let origin = bridge::player(&app.pages).unwrap().origin;
    let epoch = app.ott.as_ref().unwrap().playback_epoch;
    let pending = request_strict(&mut app, if expired { 20 } else { 60_000 });
    if expired {
        let now = plx_base::task::time::ClockSample::capture_main(app.adapters.player.mt()).unwrap().unix_ms();
        std::thread::sleep(Duration::from_millis(pending.deadline.saturating_sub(now).saturating_add(2)));
    }
    let deadline = pending.deadline;
    complete(&mut app, pending, "saved");
    if expired { assert!(app.ott.as_ref().unwrap().now_ms() >= deadline); }
    finish_strict_dismissal(&mut app);
    assert!(!app.pages.surface_up(), "strict commit closes catalog and covering More");
    assert!(!app.pages.nav.modals.surfaces.iter().any(|s| s.phase != Phase::Hidden));
    assert!(app.ott.as_ref().unwrap().reply.is_none());
    assert!(app.ott.as_ref().unwrap().edit_session.is_none());
    assert!(app.ott.as_ref().unwrap().setup.is_none());
    assert_eq!(bridge::player(&app.pages).unwrap().origin, origin);
    assert_same_source(&mut app, &identity, page, epoch);
}

#[test]
fn strict_saved_closes_catalog_and_covering_more_without_restarting() {
    let _serial = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    let _probe = PlacementProbe;
    covered_saved(false);
}

#[test]
fn expired_strict_saved_still_closes_catalog_and_cover_without_restarting() {
    let _serial = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    let _probe = PlacementProbe;
    covered_saved(true);
}

#[test]
fn strict_failed_save_keeps_catalog_editor_and_current_source() {
    let _serial = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    let _probe = PlacementProbe;
    let mut app = pinned_catalog();
    surface_action(&mut app, OttAction::BeginSearch);
    let editor = app.ott.as_ref().unwrap().edit_session.clone();
    let surface = ott_surface(&app);
    let identity = route::playback_identity(&app.player.session);
    let page = app.pages.top_page();
    let epoch = app.ott.as_ref().unwrap().playback_epoch;
    let pending = request_strict(&mut app, 60_000);
    complete(&mut app, pending, "failed");
    idle(&mut app);
    assert_eq!(ott_surface(&app), surface);
    assert!(app.ott.as_ref().unwrap().edit_session == editor);
    assert!(!crate::app::ott_kiosk_stored::read(&app.ott.as_ref().unwrap().saved.settings).unwrap().unwrap().strict());
    assert_same_source(&mut app, &identity, page, epoch);
}

#[test]
fn strict_uncertain_save_does_not_apply_presentation_cleanup() {
    let _serial = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    let _probe = PlacementProbe;
    let mut app = pinned_catalog();
    let surface = ott_surface(&app);
    let pending = request_strict(&mut app, 60_000);
    complete(&mut app, pending, "uncertain");
    // Existing uncertainty policy retires playback; it is not authority to
    // apply an unverified strict transition to the modal presentation.
    assert!(app.ott.as_ref().unwrap().persistence_uncertain);
    assert_eq!(ott_surface(&app), surface);
    assert!(!app.pages.has_pending_navigation());
}
