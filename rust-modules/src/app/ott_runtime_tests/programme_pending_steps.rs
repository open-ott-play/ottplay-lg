//! Physical step input while a resolver is held: newest selection, not a queue.
use super::*;
use plx_screens::registry::{OttPlayerAction as Action, OttPlayerRequest};

fn rocker(app: &mut App, code: u32) {
    native_edge(app, 0, code, Edge::Down);
    native_edge(app, 0, code, Edge::Up);
}

#[test]
fn rapid_pending_rocker_advances_latest_target_cancels_old_and_ignores_late_result() {
    let _lock = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    for mode in [plx_machine::machine::UiMode::Classic, plx_machine::machine::UiMode::Modern] {
        let mut app = live_m3u_remote_app(mode, false);
        let installed = route::playback_identity(&app.player.session);
        rocker(&mut app, 480);
        assert_eq!(queued(&app), Some("a"));
        let old = app.ott.as_mut().unwrap().resolver_next.take().unwrap();
        let cancelled = old.cancel.clone();
        rocker(&mut app, 480);
        assert_eq!(queued(&app), Some("b"));
        assert!(cancelled.load(Ordering::Acquire));
        assert_eq!(app.ott.as_ref().unwrap().resolver_next.as_ref().unwrap().entry.as_ref().unwrap().id, "b");
        let current_generation = app.ott.as_ref().unwrap().play_generation;
        app.ott.as_ref().unwrap().tx.send(Job::Play(old.generation, old.profile,
            Err("superseded inspection result"))).unwrap();
        runtime_tick(&mut app);
        assert_eq!(queued(&app), Some("b"));
        assert_eq!(app.ott.as_ref().unwrap().play_generation, current_generation);
        assert_eq!(route::playback_identity(&app.player.session), installed);
        assert!(app.ott.as_ref().unwrap().resolver_busy, "one old worker owns the busy slot until it retires");
    }
}

#[test]
fn same_frame_rocker_burst_coalesces_and_reverse_cancels_pending_outgoing_return() {
    let _lock = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    for reverse in [false, true] {
        let mut app = live_m3u_remote_app(plx_machine::machine::UiMode::Classic, false);
        let installed = route::playback_identity(&app.player.session);
        let before = app.ott.as_ref().unwrap().play_generation;
        let down = crate::app::events::encode_key(0, 480, true);
        let release = crate::app::events::encode_key(0, 480, false);
        let second = crate::app::events::encode_key(0, if reverse { 481 } else { 480 }, true);
        let fr = dispatch_batch(&mut app, &[down, release, second]);
        finish_input(&mut app, &fr);
        let runtime = app.ott.as_ref().unwrap();
        assert!(runtime.play_generation.wrapping_sub(before) >= 2);
        if reverse {
            assert!(runtime.play_target.is_none() && runtime.resolver_next.is_none(),
                "returning to the installed outgoing channel cancels the incoming request");
            assert_eq!(runtime.selected.as_deref(), Some("c"));
        } else {
            assert_eq!(queued(&app), Some("b"), "two fresh presses in one dispatch batch are both admitted");
            assert_eq!(runtime.resolver_next.as_ref().unwrap().entry.as_ref().unwrap().id, "b");
        }
        assert_eq!(route::playback_identity(&app.player.session), installed);
    }
}

#[test]
fn pending_step_offer_rejects_transport_menus_and_stale_source_or_later_frame_lineage() {
    let _lock = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    let mut app = live_m3u_remote_app(plx_machine::machine::UiMode::Classic, false);
    rocker(&mut app, 480);
    crate::app::ott_runtime::publish_remote_player_offer(&mut app);
    let source = app.bridge.ott_player_offer().unwrap().source.clone();
    assert!(source.pending());
    let request = OttPlayerRequest { owner: app.pages.top_page().unwrap(), source, action: Action::Next };
    let generation = app.ott.as_ref().unwrap().play_generation;
    for action in [Action::OpenCatalog, Action::OpenMenu, Action::ShowInfo,
        Action::ShowDescription, Action::OpenGuide, Action::SeekForward, Action::SeekBackward, Action::TogglePause] {
        assert!(crate::app::ott_runtime::remote_player_action(&mut app,
            OttPlayerRequest { action, ..request.clone() }).is_err());
    }
    assert_eq!(app.ott.as_ref().unwrap().play_generation, generation);
    crate::app::ott_runtime::remote_player_action(&mut app, request.clone()).unwrap();
    assert_eq!(queued(&app), Some("b"));
    // Refreshing the input publication retires the old same-frame chain too.
    crate::app::ott_runtime::publish_remote_player_offer(&mut app);
    assert!(crate::app::ott_runtime::remote_player_action(&mut app, request).is_err());
    assert_eq!(queued(&app), Some("b"));
}

fn initial_ott_pending() -> App {
    let mut app = live_m3u_remote_app(plx_machine::machine::UiMode::Classic, false);
    player::stop_bufferfeed(&mut app.player.session, &mut app.adapters.player);
    route::cancel_play(&mut app.player.session);
    app.player.session = route::PlaybackSession::IDLE;
    route::reset_player_control_for_test(&app.player.session);
    app.ott.as_mut().unwrap().playing = None;
    bridge::show_page(&mut app.pages, AppArg::Ott);
    settle_pages(&mut app);
    let mut runtime = app.ott.take().unwrap();
    runtime.reply = current_ui_owner(&app);
    runtime.page = "channels";
    runtime.catalog_dirty = false;
    runtime.resolver_busy = true;
    runtime.continuity.stop(runtime.generation);
    apply_ui_action(&mut runtime, &mut app, OttAction::Play("a".into()), true, true, false);
    assert!(runtime.play_target.is_some(), "initial Play refused: {}", runtime.status);
    assert_eq!(runtime.view().pending_play.as_deref(), Some("a"),
        "mode={:?} reply={} owner={} page={}", runtime.interface_mode(), runtime.reply.is_some(), current_ui_owner(&app).is_some(), runtime.page);
    assert!(runtime.view().pending_step.is_some());
    runtime.publish(&mut app);
    app.ott = Some(runtime);
    idle(&mut app);
    assert!(matches!(app.route(), AppArg::Ott));
    assert!(route::playback_identity(&app.player.session).is_none());
    app
}

#[test]
fn initial_ott_loading_cover_handles_physical_rocker_but_not_hidden_list_and_back_cancels() {
    let _lock = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    let mut app = initial_ott_pending();
    let old = app.ott.as_ref().unwrap().play_cancel.clone();
    rocker(&mut app, 480);
    assert_eq!(queued(&app), Some("b"));
    assert!(old.load(Ordering::Acquire));
    assert!(matches!(app.route(), AppArg::Ott), "stepping does not admit a native Player");
    let generation = app.ott.as_ref().unwrap().play_generation;
    native_edge(&mut app, consts::SDLK_DOWN, 81, Edge::Down);
    native_edge(&mut app, consts::SDLK_RETURN, 40, Edge::Down);
    assert_eq!(app.ott.as_ref().unwrap().play_generation, generation, "hidden rows cannot be activated");
    native_edge(&mut app, consts::SDLK_ESCAPE, 41, Edge::Down);
    assert!(app.ott.as_ref().unwrap().play_target.is_none());
    assert!(app.ott.as_ref().unwrap().resolver_next.is_none());
    assert!(app.ott.as_ref().unwrap().view().pending_play.is_none());
}


#[test]
fn three_same_frame_channel_presses_leave_only_the_latest_destination() {
    let _lock = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    let mut app = live_m3u_remote_app(plx_machine::machine::UiMode::Classic, false);
    let runtime = app.ott.as_mut().unwrap();
    let mut last = runtime.catalog.entries[0].clone();
    last.id = "d".into(); last.title = "Fourth".into();
    last.media = MediaLocator::Http { url: "https://media.invalid/d.m3u8".into() };
    runtime.catalog.entries.push(last);
    runtime.catalog_revision += 1;
    let down = crate::app::events::encode_key(0, 480, true);
    let release = crate::app::events::encode_key(0, 480, false);
    let fr = dispatch_batch(&mut app, &[down, release, down, release, down]);
    finish_input(&mut app, &fr);
    assert_eq!(queued(&app), Some("d"));
    assert_eq!(app.ott.as_ref().unwrap().resolver_next.as_ref().unwrap().entry.as_ref().unwrap().id, "d");
}

#[test]
fn replay_namespace_cannot_spend_initial_pending_step_offer() {
    let _lock = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    let mut app = initial_ott_pending();
    let before = app.ott.as_ref().unwrap().play_generation;
    let tick = next_tick(&mut app);
    run::publish_playback_frame(&mut app, false);
    bridge::frame(&mut app.pages, &mut app.bridge, tick, vec![plx_machine::machine::InputEvent {
        kind: plx_machine::machine::InputKind::Key { key: plx_machine::machine::Key::Other,
            sym: 0, wcode: 480, edge: Edge::Down, at_edge: false },
        at: tick, source: plx_machine::machine::Source::Replay,
    }]);
    let fr = run::Frame { ctrl: player_hud::slot(&app.player.session, app.bridge.metadata_view()),
        now: tick.ms, dt: 0.016, underlay_moving: false, player: false,
        present: false, deferred: false, rn: "ott" };
    finish_input(&mut app, &fr);
    assert_eq!(app.ott.as_ref().unwrap().play_generation, before);
    assert_eq!(queued(&app), Some("a"));
}
