//! Diagnostic integration only: real continuity policy and App lifecycle wrappers,
//! no network, decoder, storage publication or claim of native first-picture proof.
use super::*;

const IDS: &[&str] = &[
    "https://private.invalid/a?X-Plex-Token=never-log-a",
    "private/path/second-film",
    "private-third-title",
];

fn arm(runtime: &mut Runtime, play_generation: Option<u64>) -> continuity_diagnostics::Pending {
    runtime.generation = 7;
    runtime.continuity = continuity::Continuity::new(7, None);
    runtime.continuity.user_selected(7, IDS[0]);
    assert!(matches!(runtime.continuity_failed(IDS[0], IDS, true, Some(40)),
        continuity::Decision::Start { .. }));
    assert!(runtime.continuity_retry_log.is_none(), "first failure advances without cooldown");
    assert_eq!(runtime.continuity_failed(IDS[1], IDS, true, play_generation), continuity::Decision::Wait);
    let pending = runtime.continuity_retry_log.expect("actual second failure schedules cooldown");
    assert_eq!(pending.row_ordinal, 2);
    assert_eq!(pending.failures, 2);
    assert!(pending.not_before_ms > runtime.now_ms());
    assert!(!runtime.continuity.retry_due(7, runtime.now_ms()));
    pending
}

// The existing thread-scoped read-back seam deliberately delivers synchronously.
// Production continues using the existing bounded asynchronous sink.
fn records() -> Vec<String> {
    let path = plx_base::eventlog::events_log();
    let text = std::fs::read_to_string(&path).expect("private event-log record");
    text.lines().filter_map(|line| line.find("ott-continuity ").map(|at| line[at..].to_owned())).collect()
}

#[test]
fn pending_marker_precedes_deadline_and_duplicates_do_not_rebind_failed_attempt() {
    let _serial = plx_base::testlock::serial();
    plx_base::eventlog::with_private_log(|| {
        let mut app = ready_app_fixture();
        let runtime = app.ott.as_mut().unwrap();
        let pending = arm(runtime, Some(41));
        assert_eq!(runtime.continuity_failed(IDS[1], IDS, true, Some(999)), continuity::Decision::Idle);
        assert_eq!(runtime.continuity_retry_log, Some(pending));
        let reordered = [IDS[1], IDS[2], IDS[0]];
        assert_eq!(runtime.continuity_failed(IDS[1], &reordered, true, Some(999)), continuity::Decision::Idle);
        assert_eq!(runtime.continuity_retry_log, Some(pending),
            "a reordered duplicate retains original row 2 and failed generation 41");
        runtime.reconcile_retry_log();
        let lines = records();
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0], pending.line(continuity_diagnostics::Event::Pending,
            continuity_diagnostics::Reason::None, runtime.now_ms(), 7));
        for private in IDS { assert!(!lines[0].contains(private)); }
        assert!(!lines[0].contains("never-log"));
        assert!(lines[0].contains("failed_play_generation=41 failed_play_generation_known=1 row_ordinal=2"));
    });
}

#[test]
fn due_start_marker_keeps_original_generation_and_is_emitted_once() {
    let _serial = plx_base::testlock::serial();
    plx_base::eventlog::with_private_log(|| {
        let mut app = ready_app_fixture();
        let runtime = app.ott.as_mut().unwrap();
        let pending = arm(runtime, Some(41));
        assert_eq!(runtime.retry_continuity(pending.not_before_ms - 1, IDS, true), continuity::Decision::Wait);
        assert_eq!(runtime.continuity_retry_log, Some(pending));
        assert!(matches!(runtime.retry_continuity(pending.not_before_ms, IDS, true),
            continuity::Decision::Start { ref entry_id, .. } if entry_id == IDS[2]));
        assert_eq!(runtime.retry_continuity(pending.not_before_ms + 1, IDS, true), continuity::Decision::Idle);
        let lines = records();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[1], pending.line(continuity_diagnostics::Event::RetryStart,
            continuity_diagnostics::Reason::None, pending.not_before_ms, 7));
    });
}

#[test]
fn automatic_catalog_retirement_cancels_pending_without_claiming_viewer_stop() {
    let _serial = plx_base::testlock::serial();
    plx_base::eventlog::with_private_log(|| {
        let mut app = ready_app_fixture();
        let mut runtime = app.ott.take().unwrap();
        let pending = arm(&mut runtime, Some(41));
        runtime.stop_catalog(&mut app);
        runtime.stop_catalog(&mut app);
        assert!(runtime.continuity_retry_log.is_none());
        assert_eq!(runtime.retry_continuity(pending.not_before_ms, IDS, true), continuity::Decision::Idle);
        let lines = records();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[1], pending.line(continuity_diagnostics::Event::Cancelled,
            continuity_diagnostics::Reason::Changed, runtime.now_ms(), runtime.generation));
    });
}

#[test]
fn applied_standby_reports_old_and_new_generations_and_wake_does_not_restore_retry() {
    let _serial = plx_base::testlock::serial();
    plx_base::eventlog::with_private_log(|| {
        let mut app = ready_app_fixture();
        let mut runtime = app.ott.take().unwrap();
        let pending = arm(&mut runtime, None);
        runtime.suspend_with_retry_reason(&mut app, continuity_diagnostics::Reason::Standby);
        let suspended_generation = runtime.generation;
        assert_ne!(suspended_generation, pending.catalog_generation);
        runtime.resume_source_work();
        assert!(runtime.continuity_retry_log.is_none());
        assert_eq!(runtime.retry_continuity(pending.not_before_ms, IDS, true), continuity::Decision::Idle);
        let lines = records();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].contains("failed_play_generation=0 failed_play_generation_known=0"));
        assert_eq!(lines[1], pending.line(continuity_diagnostics::Event::Cancelled,
            continuity_diagnostics::Reason::Standby, runtime.now_ms(), suspended_generation));
    });
}

#[test]
fn automatic_refresh_invalidates_old_deadline_without_claiming_viewer_refresh() {
    let _serial = plx_base::testlock::serial();
    plx_base::eventlog::with_private_log(|| {
        let mut app = ready_app_fixture();
        let runtime = app.ott.as_mut().unwrap();
        let pending = arm(runtime, Some(41));
        runtime.refresh_catalog_preserving_playback();
        let refreshed_generation = runtime.generation;
        assert_ne!(refreshed_generation, pending.catalog_generation);
        assert!(runtime.continuity_retry_log.is_none());
        assert_eq!(runtime.retry_continuity(pending.not_before_ms, IDS, true), continuity::Decision::Idle);
        let lines = records();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[1], pending.line(continuity_diagnostics::Event::Cancelled,
            continuity_diagnostics::Reason::Changed, runtime.now_ms(), refreshed_generation));
        // A later explicit viewer selection remains a normal new intent.
        runtime.continuity.user_selected(refreshed_generation, IDS[0]);
        assert!(runtime.continuity.accepts_end(refreshed_generation, IDS[0]));
    });
}

#[test]
fn consumed_cooldown_waits_for_discovery_without_claiming_cancellation() {
    let _serial = plx_base::testlock::serial();
    plx_base::eventlog::with_private_log(|| {
        let mut app = ready_app_fixture();
        let runtime = app.ott.as_mut().unwrap();
        let partial = &IDS[..2];
        runtime.generation = 7;
        runtime.continuity = continuity::Continuity::new(7, None);
        runtime.continuity.user_selected(7, IDS[0]);
        assert!(matches!(runtime.continuity_failed(IDS[0], partial, false, Some(40)),
            continuity::Decision::Start { ref entry_id, .. } if entry_id == IDS[1]));
        assert_eq!(runtime.continuity_failed(IDS[1], partial, false, Some(41)), continuity::Decision::Wait);
        let pending = runtime.continuity_retry_log.expect("second failure owns a real cooldown");
        assert_eq!(pending.row_ordinal, 2);
        assert_eq!(pending.failures, 2);
        assert_eq!(runtime.retry_continuity(pending.not_before_ms, partial, false), continuity::Decision::Wait);
        assert!(runtime.continuity.pending_retry().is_none(), "the deadline was consumed");
        assert!(runtime.continuity_retry_log.is_none(), "only the cooldown observer is retired");
        runtime.reconcile_retry_log();
        assert_eq!(runtime.retry_continuity(pending.not_before_ms + 1, partial, false), continuity::Decision::Idle);
        let lines = records();
        assert_eq!(lines.len(), 2, "one pending and one transition, without repeated markers");
        assert_eq!(lines[1], pending.line(continuity_diagnostics::Event::DiscoveryWait,
            continuity_diagnostics::Reason::None, pending.not_before_ms, 7));
        assert!(lines.iter().all(|line| !line.contains("event=cancelled") && !line.contains("reason=exhausted")));
        assert!(matches!(runtime.continuity.catalog_updated(7, IDS, false),
            continuity::Decision::Start { ref entry_id, .. } if entry_id == IDS[2]),
            "the original continuation still starts a later discovered successor");
        runtime.reconcile_retry_log();
        assert_eq!(records(), lines, "a later discovery Start is outside the consumed cooldown observation");
    });
}


#[test]
fn actual_viewer_refresh_action_attributes_only_its_cancelled_cooldown() {
    let _serial = plx_base::testlock::serial();
    plx_base::eventlog::with_private_log(|| {
        let mut app = ready_app_fixture();
        let mut runtime = app.ott.take().unwrap();
        let profile = ProviderProfile {
            id: "refresh-fixture".into(), name: "Refresh fixture".into(),
            kind: ProviderKind::M3u, source: "https://private.invalid/playlist.m3u".into(),
            source_alias: String::new(), enabled: true,
        };
        runtime.saved.profiles.active_profile = Some(profile.id.clone());
        runtime.saved.profiles.profiles = vec![profile.clone()];
        runtime.provider = ProviderKind::M3u;
        runtime.page = "channels";
        // Keep the real pending catalogue request observable without launching its worker.
        runtime.catalog_done.store(false, Ordering::Release);
        let pending = arm(&mut runtime, Some(41));
        handle_action(&mut runtime, &mut app, OttAction::Refresh).expect("viewer Refresh admitted");
        assert_ne!(runtime.generation, pending.catalog_generation);
        assert_eq!(runtime.catalog_next.as_ref().map(|p| p.id.as_str()), Some(profile.id.as_str()));
        // The real tick settles its observation after dispatching the action.
        // Use that same end-of-tick step with the action's existing clock sample.
        runtime.reconcile_retry_log();
        assert!(runtime.continuity_retry_log.is_none());
        assert_eq!(runtime.retry_continuity(pending.not_before_ms, IDS, true), continuity::Decision::Idle);
        let lines = records();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[1], pending.line(continuity_diagnostics::Event::Cancelled,
            continuity_diagnostics::Reason::Refresh, runtime.now_ms(), runtime.generation));
    });
}


struct PlaybackStateRestore(u8);
impl Drop for PlaybackStateRestore {
    fn drop(&mut self) { plx_media::player::restore_state_for_test(self.0); }
}

fn content_frame(app: &mut App, tick: Tick) {
    let frame = run::Frame {
        ctrl: plx_appkit::player_hud::slot(&app.player.session, app.bridge.metadata_view()),
        now: tick.ms, dt: 0.016, underlay_moving: false, player: true,
        present: false, deferred: false, rn: "player",
    };
    content::content_requests(app, &frame);
}

fn player_request(app: &mut App, request: PlayerReq) {
    app.pages.emit(MachineId::Nav, Fx::App(AppFx::Player(request)));
    let tick = next_tick(app);
    bridge::frame(&mut app.pages, &mut app.bridge, tick, Vec::new());
    content_frame(app, tick);
}

fn player_key(app: &mut App, code: u32) {
    use plx_machine::machine::{Edge, InputKind, Key};
    let tick = next_tick(app);
    crate::app::input::note_global_press(&app.player.session, 0, code, tick.ms,
        bridge::player_mut(&mut app.pages).map(|screen| &mut screen.hud),
        &mut app.ok_armed, &mut app.input.press);
    if let Some(screen) = bridge::player_mut(&mut app.pages) { screen.publish(); }
    run::publish_playback_frame(app, true);
    let mut input = bridge::script_edge(Key::Other, Edge::Down, tick);
    input.kind = InputKind::Key { key: Key::Other, sym: 0, wcode: code,
        edge: Edge::Down, at_edge: false };
    bridge::frame(&mut app.pages, &mut app.bridge, tick, vec![input]);
    content_frame(app, tick);
}

fn assert_retired(app: &mut App, pending: continuity_diagnostics::Pending, reason: &str) {
    assert!(app.player.pending_exit.is_some(), "the actual exit performer must own a receipt");
    runtime_tick(app);
    let runtime = app.ott.as_mut().unwrap();
    assert!(app.player.pending_exit.is_none());
    assert!(runtime.continuity_retry_log.is_none());
    assert_eq!(runtime.retry_continuity(pending.not_before_ms, IDS, true), continuity::Decision::Idle);
    let expected = pending.line(continuity_diagnostics::Event::Cancelled,
        continuity_diagnostics::Reason::Changed, runtime.now_ms(), runtime.generation)
        .replace("reason=changed", &format!("reason={reason}"));
    let lines = records();
    assert_eq!(lines.len(), 2, "one pending and one owned cancellation");
    assert_eq!(lines[1], expected);
    runtime_tick(app);
    assert_eq!(records(), lines, "the consumed exit cannot emit another cancellation");
}

#[test]
fn actual_stop_back_and_failure_back_are_requested_exit_not_stop_ack() {
    let _serial = plx_base::testlock::serial();
    use plx_media::player::PlaybackState;
    for (code, state) in [
        (plx_ui::consts::WCODE_STOP, PlaybackState::Playing),
        (plx_ui::consts::WCODE_BACK, PlaybackState::Playing),
        (plx_ui::consts::WCODE_BACK, PlaybackState::Error),
    ] {
        plx_base::eventlog::with_private_log(|| {
            let mut app = ready_app_fixture();
            mount_player(&mut app);
            let _state = PlaybackStateRestore(plx_media::player::swap_state_for_test(state));
            bridge::player_mut(&mut app.pages).unwrap().hud.hide(false);
            let pending = arm(app.ott.as_mut().unwrap(), Some(41));
            player_key(&mut app, code);
            assert_retired(&mut app, pending, "requested_exit");
        });
    }
}

#[test]
fn automatic_eos_exit_cannot_claim_a_requested_exit() {
    let _serial = plx_base::testlock::serial();
    plx_base::eventlog::with_private_log(|| {
        let mut app = ready_app_fixture();
        mount_player(&mut app);
        let pending = arm(app.ott.as_mut().unwrap(), Some(41));
        assert!(!playback::finish_playback(&mut app),
            "the fixture has no Up Next item");
        assert_retired(&mut app, pending, "changed");
    });
}

#[test]
fn go_to_show_exit_cannot_claim_a_requested_exit() {
    let _serial = plx_base::testlock::serial();
    plx_base::eventlog::with_private_log(|| {
        let mut app = ready_app_fixture();
        mount_player(&mut app);
        let pending = arm(app.ott.as_mut().unwrap(), Some(41));
        // This is the Info card's actual command and real content performer.
        // Do not commit the subsequently queued Detail page (which would fetch it).
        player_request(&mut app, PlayerReq::Info(plx_appkit::info_panel::InfoAction::GoToDetail(
            "private-show-not-a-stop".into())));
        assert_retired(&mut app, pending, "changed");
    });
}

#[test]
fn first_exit_receipt_keeps_its_provenance_across_a_later_exit() {
    let _serial = plx_base::testlock::serial();
    for requested_first in [false, true] {
        plx_base::eventlog::with_private_log(|| {
            let mut app = ready_app_fixture();
            mount_player(&mut app);
            let pending = arm(app.ott.as_mut().unwrap(), Some(41));
            if requested_first { player_request(&mut app, PlayerReq::Exit); }
            playback::exit_player(&mut app.player, &mut app.adapters.player,
                &mut app.repause_at, &mut app.refresh_hubs, &mut app.pages);
            // Dispatch directly to the same production performer: a second frame
            // would legitimately commit the first exit's queued navigation.
            if !requested_first {
                playback::player_requests(&mut app.player, &mut app.adapters.player,
                    &mut app.repause_at, vec![PlayerReq::Exit], app.loop_t,
                    &mut app.refresh_hubs, &mut app.pages, &mut app.bridge,
                    &mut app.ok_armed, &mut app.input.press);
            }
            assert_retired(&mut app, pending, if requested_first { "requested_exit" } else { "changed" });
        });
    }
}
