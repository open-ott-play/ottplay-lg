//! Existing Session commands and a synthetic app fixture. No credentials, disk
//! durability or network success is manufactured by these navigation tests.
use super::*;
use plx_session::auth::owner::{LoginOrigin, ReplyTo};

fn start(app: &mut App, explicit: bool) -> LoginOrigin {
    if explicit {
        let runtime = app.ott.as_mut().unwrap();
        runtime.provider = ProviderKind::Plex;
        runtime.page = "profiles";
        ui(app, OttAction::EditSetting("plex.connect".into()));
    } else {
        ui(app, OttAction::SelectProvider("plex".into()));
    }
    run::loop_requests(app);
    settle_pages(app);
    auth_return::poll(app);
    settle_pages(app);
    assert!(matches!(app.route(), AppArg::Login));
    app.bridge.auth_read().0.origin_cancel.expect("accepted ordinary origin")
}

fn cancel(app: &mut App, origin: LoginOrigin, owner: InstanceId) {
    bridge::execute_session_command(&mut app.pages,
        plx_session::auth::SessionCmd::CancelLoginFrom {
            origin, reply: ReplyTo { instance: owner.0, correlation: 701 },
        });
    settle_pages(app);
    auth_return::poll(app);
    settle_pages(app);
}

#[test]
fn accepted_ott_login_cancel_returns_fresh_profiles_without_writing_or_playing() {
    let _serial = plx_base::testlock::serial();
    let _registry = Registry::empty();
    for explicit in [false, true] {
        let mut app = Box::new(mounted());
        let app = app.as_mut();
        let old = app.pages.top_page().unwrap();
        let before = serde_json::to_value(&app.ott.as_ref().unwrap().saved).unwrap();
        let origin = start(app, explicit);
        let login = app.pages.top_page().unwrap();
        assert_ne!(login, old);
        cancel(app, origin, login);
        assert!(matches!(app.route(), AppArg::Ott));
        assert_ne!(app.pages.top_page(), Some(old), "old page cannot be resurrected");
        let runtime = app.ott.as_ref().unwrap();
        assert_eq!(runtime.view().page_id, "profiles:plex");
        assert_eq!(runtime.view().selected_row.as_deref(), Some("action/plex.connect"));
        assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), before);
        assert!(!runtime.saving && runtime.play_target.is_none() && runtime.resolver_next.is_none());
        assert!(app.bridge.take_session_ready().is_none(), "cancel must not fabricate Ready");
        let returned = app.pages.top_page();
        cancel(app, origin, login);
        assert_eq!(app.pages.top_page(), returned, "consumed return cannot repeat");
    }
}

#[test]
fn accepted_cancel_with_changed_source_uses_current_view_without_old_focus_or_activation() {
    let _serial = plx_base::testlock::serial();
    let _registry = Registry::empty();
    let mut app = Box::new(mounted());
    let app = app.as_mut();
    let origin = start(app, false);
    let login = app.pages.top_page().unwrap();
    // Model an already accepted remote publication through the ordinary context
    // projection. This is not a claim that a remote storage write was exercised.
    let runtime = app.ott.as_mut().unwrap();
    runtime.saved.profiles.active_profile = None;
    runtime.saved.settings.insert("selectedProvider".into(), json!("vportal"));
    runtime.sync_saved_context();
    runtime.page = "profiles";
    runtime.profile_focus = None;
    let current = serde_json::to_value(&runtime.saved).unwrap();
    cancel(app, origin, login);
    assert!(matches!(app.route(), AppArg::Ott));
    let runtime = app.ott.as_ref().unwrap();
    assert_eq!(runtime.view().page_id, "profiles:vportal");
    assert_ne!(runtime.view().selected_row.as_deref(), Some("action/plex.connect"));
    assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), current);
    assert!(runtime.play_target.is_none() && runtime.resolver_next.is_none());
}

#[test]
fn cancel_from_replaced_login_does_not_override_the_new_foreground() {
    let _serial = plx_base::testlock::serial();
    let _registry = Registry::empty();
    let mut app = Box::new(mounted());
    let app = app.as_mut();
    let origin = start(app, false);
    let login = app.pages.top_page().unwrap();
    bridge::nav_root(&mut app.pages, AppArg::Home);
    settle_pages(app);
    let current = app.pages.top_page();
    cancel(app, origin, login);
    assert!(matches!(app.route(), AppArg::Home));
    assert_eq!(app.pages.top_page(), current);
}

#[test]
fn stale_cancel_after_retry_keeps_the_new_flow_on_the_same_login_screen() {
    let _serial = plx_base::testlock::serial();
    let _registry = Registry::empty();
    let mut app = Box::new(mounted());
    let app = app.as_mut();
    let origin = start(app, false);
    let login = app.pages.top_page().unwrap();
    bridge::execute_session_command(&mut app.pages, plx_session::auth::SessionCmd::StartLogin);
    settle_pages(app);
    let epoch = app.bridge.auth_read().0.flow_epoch;
    cancel(app, origin, login);
    assert_eq!(app.pages.top_page(), Some(login));
    assert!(matches!(app.route(), AppArg::Login));
    assert_eq!(app.bridge.auth_read().0.flow_epoch, epoch);
}


#[test]
fn duplicate_cancels_in_one_drain_preserve_the_successful_return() {
    let _serial = plx_base::testlock::serial();
    let _registry = Registry::empty();
    let mut app = Box::new(mounted());
    let app = app.as_mut();
    let origin = start(app, false);
    let login = app.pages.top_page().unwrap();
    for correlation in [801, 802] {
        bridge::execute_session_command(&mut app.pages,
            plx_session::auth::SessionCmd::CancelLoginFrom {
                origin, reply: ReplyTo { instance: login.0, correlation },
            });
    }
    // Both Session verdicts land before the application consumes either one.
    settle_pages(app);
    assert!(auth_return::poll(app));
    settle_pages(app);
    assert!(matches!(app.route(), AppArg::Ott));
    assert_eq!(app.ott.as_ref().unwrap().view().selected_row.as_deref(), Some("action/plex.connect"));
    let returned = app.pages.top_page();
    auth_return::poll(app);
    settle_pages(app);
    assert_eq!(app.pages.top_page(), returned);
    assert!(app.bridge.take_session_ready().is_none());
}

#[test]
fn cancel_at_first_login_mount_returns_once_without_waiting_for_extra_settle_frames() {
    let _serial = plx_base::testlock::serial();
    let _registry = Registry::empty();
    let mut app = Box::new(mounted());
    let app = app.as_mut();
    ui(app, OttAction::SelectProvider("plex".into()));
    run::loop_requests(app);
    settle_pages(app);
    let origin = app.bridge.auth_read().0.origin_cancel.unwrap();
    assert!(auth_return::poll(app));
    // Stop at the first mounted Login, rather than settling its entrance.
    for _ in 0..24 {
        let tick = next_tick(app);
        bridge::frame(&mut app.pages, &mut app.bridge, tick, Vec::new());
        if matches!(app.route(), AppArg::Login) && app.pages.top_page().is_some() { break; }
    }
    assert!(matches!(app.route(), AppArg::Login));
    let login = app.pages.top_page().unwrap();
    bridge::execute_session_command(&mut app.pages,
        plx_session::auth::SessionCmd::CancelLoginFrom {
            origin, reply: ReplyTo { instance: login.0, correlation: 803 },
        });
    for _ in 0..24 {
        let tick = next_tick(app);
        bridge::frame(&mut app.pages, &mut app.bridge, tick, Vec::new());
        auth_return::poll(app);
    }
    assert!(matches!(app.route(), AppArg::Ott));
    assert_eq!(app.ott.as_ref().unwrap().view().selected_row.as_deref(), Some("action/plex.connect"));
    assert!(app.bridge.take_session_ready().is_none());
}

#[test]
fn replaced_start_owner_retires_only_its_accepted_flow_without_navigation() {
    let _serial = plx_base::testlock::serial();
    let _registry = Registry::empty();
    let mut app = Box::new(mounted());
    let app = app.as_mut();
    ui(app, OttAction::SelectProvider("plex".into()));
    run::loop_requests(app);
    settle_pages(app);
    assert!(app.bridge.auth_read().0.origin_cancel.is_some());
    bridge::nav_root(&mut app.pages, AppArg::Home);
    settle_pages(app);
    let current = app.pages.top_page();
    assert!(!auth_return::poll(app));
    settle_pages(app);
    assert!(!auth_return::poll(app));
    assert!(matches!(app.route(), AppArg::Home));
    assert_eq!(app.pages.top_page(), current);
    assert!(app.bridge.auth_read().0.origin_cancel.is_none());
    assert!(app.bridge.take_session_ready().is_none());
}


#[test]
fn network_boundary_precedes_an_already_queued_cancel_for_a_ready_plaintext_session() {
    let _serial = plx_base::testlock::serial();
    let _registry = Registry::empty();
    let mut app = Box::new(mounted());
    let app = app.as_mut();
    let mut persisted = plx_plex::plex::session::Session {
        client_id: "origin-fixture-client".into(), account_token: "origin-fixture-account".into(),
        ..Default::default()
    };
    persisted.server.address = "192.168.0.10".into();
    persisted.server.port = 32400;
    persisted.server.token = "origin-fixture-pms".into();
    persisted.user.uuid = "origin-fixture-user".into();
    persisted.user.token = "origin-fixture-pms".into();
    let mut initial = plx_session::auth::SessionInit::captured(persisted);
    initial.phase = plx_session::auth::Phase::Ready;
    initial.active_profile = Some(initial.persisted.user.clone());
    app.bridge = bridge::Bridge::for_session_test(initial);
    let server = plx_plex::plex::Origin::parse("http://192.168.0.10:32400").unwrap();
    plx_plex::plex::grant::mint(plx_plex::plex::grant::scope(), "origin-fixture-machine", &server,
        &plx_plex::plex::grant::eligible_evidence_for_test()).unwrap();
    assert!(plx_plex::plex::grant::granted_now("origin-fixture-machine", &server));
    let origin = start(app, true);
    let login = app.pages.top_page().unwrap();
    bridge::execute_session_command(&mut app.pages,
        plx_session::auth::SessionCmd::CancelLoginFrom {
            origin, reply: ReplyTo { instance: login.0, correlation: 804 },
        });
    // The exact boundary called by real DID foreground, before the queued
    // cancellation or the later endpoint work is allowed to run.
    run::network_continuity_changed(app);
    assert!(!plx_plex::plex::grant::granted_now("origin-fixture-machine", &server));
    assert!(app.bridge.auth_read().0.origin_cancel.is_none());
    settle_pages(app);
    assert!(!auth_return::poll(app));
    settle_pages(app);
    assert_eq!(app.pages.top_page(), Some(login));
    assert!(matches!(app.route(), AppArg::Login));
    assert!(app.bridge.take_session_ready().is_none());
}

#[test]
fn accepted_return_waits_for_its_modal_to_release_input_without_dismissing_it() {
    let _serial = plx_base::testlock::serial();
    let _registry = Registry::empty();
    let mut app = Box::new(mounted());
    let app = app.as_mut();
    let origin = start(app, false);
    let login = app.pages.top_page().unwrap();
    bridge::execute_session_command(&mut app.pages,
        plx_session::auth::SessionCmd::CancelLoginFrom {
            origin, reply: ReplyTo { instance: login.0, correlation: 805 },
        });
    settle_pages(app);
    bridge::open_settings(&mut app.pages);
    settle_pages(app);
    assert!(app.pages.surface_up());
    assert!(auth_return::poll(app));
    settle_pages(app);
    assert!(app.pages.surface_up(), "the return cannot dismiss a different input owner");
    assert_eq!(app.pages.top_page(), Some(login));
    let surface = app.pages.nav.modals.surfaces.last().expect("Settings modal is mounted").entry.id;
    app.pages.request(MachineId::Nav, plx_machine::machine::NavOp::Dismiss(surface));
    let tick = next_tick(app);
    bridge::frame(&mut app.pages, &mut app.bridge, tick, Vec::new());
    assert_eq!(app.pages.nav.modals.surface(surface).unwrap().phase,
        plx_ui::containers::modal::Phase::Closing);
    assert_eq!(app.pages.nav.input_owner(), Some(plx_machine::machine::InputOwner::Entry(
        app.pages.nav.top_page().unwrap().id)));
    assert_eq!(app.pages.top_page(), Some(login));
    // Settings is Opaque: input returns at Closing, but surface_up remains true
    // until the full spring settles. The generic 24-frame helper is not that bound.
    for _ in 0..120 {
        if !app.pages.surface_up() { break; }
        let tick = next_tick(app);
        bridge::frame(&mut app.pages, &mut app.bridge, tick, Vec::new());
    }
    assert!(!app.pages.surface_up());
    assert!(auth_return::poll(app));
    settle_pages(app);
    assert!(matches!(app.route(), AppArg::Ott));
}

#[test]
fn accepted_return_waits_while_suspended_and_cannot_survive_network_invalidation() {
    let _serial = plx_base::testlock::serial();
    let _registry = Registry::empty();
    let mut app = Box::new(mounted());
    let app = app.as_mut();
    let origin = start(app, false);
    let login = app.pages.top_page().unwrap();
    bridge::execute_session_command(&mut app.pages,
        plx_session::auth::SessionCmd::CancelLoginFrom {
            origin, reply: ReplyTo { instance: login.0, correlation: 806 },
        });
    settle_pages(app);
    bridge::background(&mut app.pages);
    app.window_activity.event(0x104);
    assert!(auth_return::poll(app));
    assert_eq!(app.pages.top_page(), Some(login));
    app.bridge.login_origin_network_changed();
    bridge::foreground(&mut app.pages);
    app.window_activity.event(0x106);
    assert!(!auth_return::poll(app), "an expired returned marker cannot be revived on foreground");
    assert_eq!(app.pages.top_page(), Some(login));
    assert!(app.bridge.take_session_ready().is_none());
}


#[test]
fn accepted_return_waits_for_the_system_keyboard() {
    let _serial = plx_base::testlock::serial();
    let _registry = Registry::empty();
    let mut app = Box::new(mounted());
    let app = app.as_mut();
    let origin = start(app, false);
    let login = app.pages.top_page().unwrap();
    bridge::execute_session_command(&mut app.pages,
        plx_session::auth::SessionCmd::CancelLoginFrom {
            origin, reply: ReplyTo { instance: login.0, correlation: 807 },
        });
    settle_pages(app);
    // The public input state is the dispatcher's authoritative keyboard owner.
    app.pages.input.keyboard = true;
    assert!(auth_return::poll(app));
    assert_eq!(app.pages.top_page(), Some(login));
    app.pages.input.keyboard = false;
    assert!(auth_return::poll(app));
    settle_pages(app);
    assert!(matches!(app.route(), AppArg::Ott));
}

#[test]
fn controlled_replay_return_uses_logical_owner_not_physical_window_activity() {
    use crate::app::{bootstrap::{Initial, Preflight}, recorder::{Recplay, ReplayMode, state_fp}};
    use plx_ui::rec::{Header, Recording};
    let _serial = plx_base::testlock::serial();
    let _registry = Registry::empty();
    let mut app = Box::new(mounted());
    let app = app.as_mut();
    let origin = start(app, false);
    let login = app.pages.top_page().unwrap();
    let initial = Initial::synthetic_home(1, 32517, None).unwrap();
    let recording = Recording { header: Header::new(state_fp(), &initial),
        frames: Vec::new(), metrics: Default::default(), stopped_at: None };
    app.boot_initial = Some(initial.clone());
    app.rec = Recplay::controlled(Preflight::Replay {
        initial: initial.clone(), recording, mode: ReplayMode::Resolve,
    }, &initial).unwrap();
    assert!(run::controlled_replay(app));
    let mut frame = run::Frame {
        ctrl: plx_appkit::player_hud::slot(&app.player.session, app.bridge.metadata_view()),
        now: 0, dt: 0.016, underlay_moving: false, player: false,
        present: false, deferred: false, rn: "login",
    };
    let mut restored_windows = 0;
    app.ev[..4].copy_from_slice(&0x106u32.to_ne_bytes());
    unsafe { run::ingest_sdl_event_with_window(app, &mut frame, |_| restored_windows += 1); }
    assert_eq!(restored_windows, 1, "only physical window safety follows ambient SDL");
    assert_eq!(app.bridge.auth_read().0.origin_cancel, Some(origin),
        "ambient SDL cannot retire the recorded logical origin");
    // Physical presentation remains forbidden; no recorded logical background
    // event has suspended the navigation owner.
    app.window_activity.event(0x104);
    assert!(!app.window_activity.allow_present(true));
    assert!(!app.pages.nav.suspended);
    cancel(app, origin, login);
    assert!(matches!(app.route(), AppArg::Ott));
    assert!(!app.window_activity.allow_present(true));
}


#[test]
fn accepted_start_receipt_after_network_boundary_opens_nonreturnable_login() {
    let _serial = plx_base::testlock::serial();
    let _registry = Registry::empty();
    let mut app = Box::new(mounted());
    let app = app.as_mut();
    let saved = serde_json::to_value(&app.ott.as_ref().unwrap().saved).unwrap();
    ui(app, OttAction::SelectProvider("plex".into()));
    run::loop_requests(app);
    settle_pages(app);
    let origin = app.bridge.auth_read().0.origin_cancel.expect("Session accepted before DID");
    assert!(matches!(app.route(), AppArg::Ott), "Started has not yet reached the app");
    run::network_continuity_changed(app);
    assert!(app.bridge.auth_read().0.origin_cancel.is_none());
    assert_eq!(app.bridge.auth_read().0.flow_epoch, origin.flow_epoch);
    assert!(auth_return::poll(app));
    settle_pages(app);
    assert!(matches!(app.route(), AppArg::Login), "accepted sign-in still has an ordinary UI");
    let login = app.pages.top_page().unwrap();
    cancel(app, origin, login);
    assert_eq!(app.pages.top_page(), Some(login), "retired return proof stays unusable");
    assert!(matches!(app.route(), AppArg::Login));
    let runtime = app.ott.as_ref().unwrap();
    assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), saved);
    assert!(runtime.play_target.is_none() && runtime.resolver_next.is_none());
    assert!(app.bridge.take_session_ready().is_none());
}
