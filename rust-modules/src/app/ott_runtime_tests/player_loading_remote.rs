//! Real Player catalogue modal ownership while its selected source is resolving.
use super::*;
use super::player_remote::{forty_channels, open_fullscreen_catalog};
use plx_machine::machine::UiMode;
use plx_screens::registry::{OttPlayerAction as Action, OttPlayerRequest};

fn loading_catalog() -> (App, plx_machine::machine::InstanceId) {
    let mut app = forty_channels(UiMode::Classic);
    let panel = open_fullscreen_catalog(&mut app);
    key(&mut app, Key::Down);
    key(&mut app, Key::Ok);
    assert!(matches!(app.route(), AppArg::Player));
    assert_eq!(ott_surface(&app), Some(panel));
    assert_eq!(queued(&app), Some("2"));
    assert_eq!(app.ott.as_ref().unwrap().view().pending_play.as_deref(), Some("2"));
    (app, panel)
}

#[test]
fn loading_catalog_modal_rocker_replaces_latest_target_and_rejects_late_completion() {
    let _lock = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    let (mut app, panel) = loading_catalog();
    let old = app.ott.as_mut().unwrap().resolver_next.take().unwrap();
    let installed = route::playback_identity(&app.player.session);
    native_edge(&mut app, 0, 480, Edge::Down);
    native_edge(&mut app, 0, 480, Edge::Up);
    assert_eq!(queued(&app), Some("3"));
    assert!(old.cancel.load(Ordering::Acquire));
    assert_eq!(app.ott.as_ref().unwrap().view().pending_play.as_deref(), Some("3"),
        "a replacement keeps the cover bound to its new generation and modal owner");
    native_edge(&mut app, 0, 480, Edge::Down);
    native_edge(&mut app, 0, 480, Edge::Up);
    assert_eq!(queued(&app), Some("4"));
    app.ott.as_ref().unwrap().tx.send(Job::Play(old.generation, old.profile,
        Err("superseded modal resolver"))).unwrap();
    runtime_tick(&mut app);
    assert_eq!(queued(&app), Some("4"));
    assert_eq!(app.ott.as_ref().unwrap().view().selected_row.as_deref(), Some("4"));
    assert_eq!(ott_surface(&app), Some(panel));
    assert_eq!(route::playback_identity(&app.player.session), installed);
    native_edge(&mut app, 0, 481, Edge::Down);
    native_edge(&mut app, 0, 481, Edge::Up);
    assert_eq!(queued(&app), Some("3"), "reverse starts from the latest pending row");
}

#[test]
fn loading_catalog_pending_capability_is_step_only_and_requires_its_exact_modal_owner() {
    let _lock = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    let (mut app, panel) = loading_catalog();
    crate::app::ott_runtime::publish_remote_player_offer(&mut app);
    let source = app.bridge.ott_player_offer().unwrap().source.clone();
    let request = OttPlayerRequest { owner: panel, source, action: Action::Next };
    assert!(request.source.pending());
    for action in [Action::OpenCatalog, Action::OpenMenu, Action::OpenGuide, Action::TogglePause,
        Action::SeekForward, Action::SeekBackward] {
        assert!(crate::app::ott_runtime::remote_player_action(&mut app,
            OttPlayerRequest { action, ..request.clone() }).is_err());
    }
    let player = app.pages.top_page().unwrap();
    assert_ne!(player, panel);
    assert!(crate::app::ott_runtime::remote_player_action(&mut app,
        OttPlayerRequest { owner: player, ..request.clone() }).is_err());
    assert_eq!(queued(&app), Some("2"));
    key(&mut app, Key::Back);
    assert!(queued(&app).is_none());
    assert!(app.ott.as_ref().unwrap().view().pending_play.is_none());
    assert!(crate::app::ott_runtime::remote_player_action(&mut app, request).is_err(),
        "Back retires the pending ticket even while its catalogue remains mounted");
}

fn awaiting_native_load() -> App {
    let mut app = forty_channels(UiMode::Classic);
    let mut effects = Vec::new();
    app.player.lifecycle.step(&lifecycle::ForegroundEvent::Park {
        saved_ns: 0, viewer_paused: false,
    }, &mut effects);
    assert!(app.player.lifecycle.awaiting_load());
    assert!(effects.is_empty());
    assert!(!app.decoder_parked && playback_allowed(&app));
    crate::app::ott_runtime::publish_remote_player_offer(&mut app);
    app
}

#[test]
fn installed_channel_awaiting_native_load_accepts_only_a_current_ticket_bound_step() {
    let _lock = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    let mut app = awaiting_native_load();
    let source = app.bridge.ott_player_offer().expect("loading source has CH-only authority").source.clone();
    assert!(source.pending());
    let request = OttPlayerRequest { owner: app.pages.top_page().unwrap(), source, action: Action::Next };
    for action in [Action::OpenCatalog, Action::OpenMenu, Action::TogglePause, Action::SeekForward] {
        assert!(crate::app::ott_runtime::remote_player_action(&mut app,
            OttPlayerRequest { action, ..request.clone() }).is_err());
    }
    let installed = route::playback_identity(&app.player.session);
    crate::app::ott_runtime::remote_player_action(&mut app, request).unwrap();
    assert_eq!(queued(&app), Some("2"));
    assert_eq!(route::playback_identity(&app.player.session), installed);
    assert!(!app.ott.as_ref().unwrap().play_target.as_ref().unwrap().advance_on_failure);
}

#[test]
fn installed_loading_step_refuses_cancelled_ticket_background_and_changed_identity() {
    let _lock = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    for change in ["ticket", "identity", "standby", "parked"] {
        let mut app = awaiting_native_load();
        let source = app.bridge.ott_player_offer().unwrap().source.clone();
        let request = OttPlayerRequest { owner: app.pages.top_page().unwrap(), source, action: Action::Next };
        match change {
            "ticket" => route::cancel_play(&mut app.player.session),
            "identity" => app.ott.as_mut().unwrap().playing.as_mut().unwrap().identity = None,
            "standby" => app.ott_standby = true,
            "parked" => app.decoder_parked = true,
            _ => unreachable!(),
        }
        assert!(crate::app::ott_runtime::remote_player_action(&mut app, request).is_err(), "{change}");
        assert!(queued(&app).is_none(), "{change}");
    }
}
