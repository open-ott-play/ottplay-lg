//! An admitted app resolver must not borrow the outgoing native error as its UI verdict.
use super::*;

fn opening() -> Box<App> {
    let mut app = fixture();
    mount_player(&mut app);
    app.running = true;
    queue(&mut app, "second");
    signal_error(&app, FailureKind::MediaSource);
    assert!(
        !plx_media::route::play_pending(),
        "the OTT resolver precedes a native route flight"
    );
    app
}

#[test]
fn startup_pending_covers_outgoing_error_without_clearing_its_evidence() {
    let _serial = plx_base::testlock::serial();
    let _reset = ResetMedia;
    let app = opening();
    assert!(player_startup_pending(&app));
    assert_eq!(
        plx_media::player::state(&app.player.session),
        PlaybackState::Error
    );
    assert_eq!(
        plx_media::player::error_now(&app.player.session).kind,
        FailureKind::MediaSource
    );
}

#[test]
fn startup_pending_is_revoked_by_cancel_generation_ticket_and_source_changes() {
    let _serial = plx_base::testlock::serial();
    let _reset = ResetMedia;
    let mut app = opening();
    assert!(player_startup_pending(&app));
    let generation = app.ott.as_ref().unwrap().play_generation;
    app.ott.as_mut().unwrap().play_generation = generation.wrapping_add(1);
    assert!(
        !player_startup_pending(&app),
        "the old profile cannot cover a newer request"
    );
    app.ott.as_mut().unwrap().play_generation = generation;
    let profile = app.ott.as_ref().unwrap().saved.profiles.profiles[0].clone();
    app.ott.as_mut().unwrap().saved.profiles.profiles[0]
        .source
        .push_str("/edited");
    assert!(
        !player_startup_pending(&app),
        "same ID with new private source is a different owner"
    );
    app.ott.as_mut().unwrap().saved.profiles.profiles[0] = profile;
    assert!(player_startup_pending(&app));
    plx_media::route::cancel_play(&mut app.player.session);
    assert!(
        !player_startup_pending(&app),
        "a native cancellation revokes the admission ticket"
    );
    app.ott.as_mut().unwrap().cancel_play_request();
    assert!(!player_startup_pending(&app));
    assert_eq!(
        plx_media::player::state(&app.player.session),
        PlaybackState::Error
    );
}

#[test]
fn startup_pending_cannot_outlive_foreground_or_a_terminal_request() {
    let _serial = plx_base::testlock::serial();
    let _reset = ResetMedia;
    let mut app = opening();
    app.ott_standby = true;
    assert!(!player_startup_pending(&app));
    app.ott_standby = false;
    assert!(player_startup_pending(&app));
    app.ott
        .as_mut()
        .unwrap()
        .play_cancel
        .store(true, Ordering::Release);
    assert!(!player_startup_pending(&app));
    app.ott.as_mut().unwrap().cancel_play_request();
    assert!(
        !player_startup_pending(&app),
        "a terminal refusal has no pending request to cover it"
    );
}

#[test]
fn startup_pending_covers_only_the_exact_bounded_live_retry() {
    let _serial = plx_base::testlock::serial();
    let _reset = ResetMedia;
    let mut app = live_failure();
    mount_player(&mut app);
    app.running = true;
    assert!(
        player_startup_pending(&app),
        "the retained transient live retry is still working"
    );
    let generation = app.ott.as_ref().unwrap().generation;
    app.ott.as_mut().unwrap().generation = generation.wrapping_add(1);
    assert!(
        !player_startup_pending(&app),
        "an old cooldown cannot cover a new catalogue"
    );
    app.ott.as_mut().unwrap().generation = generation;
    assert!(player_startup_pending(&app));
    app.ott.as_mut().unwrap().continuity.stop(generation);
    assert!(
        !player_startup_pending(&app),
        "no indefinite working notice after cancellation/exhaustion"
    );
}

#[test]
fn stale_failure_cannot_end_new_start_but_matching_terminal_failure_does() {
    let _serial = plx_base::testlock::serial();
    let _reset = ResetMedia;
    let mut app = fixture();
    mount_player(&mut app);
    app.running = true;
    let old = queue(&mut app, "first");
    let current = queue(&mut app, "second");
    signal_error(&app, FailureKind::MediaSource);
    app.ott
        .as_ref()
        .unwrap()
        .tx
        .send(Job::Play(old, profile(), Err("stale refusal")))
        .unwrap();
    let clock = plx_base::task::time::ClockSample::capture_main(app.adapters.player.mt()).unwrap();
    tick_with_start(&mut app, clock, |_, _, _, _, _| {
        panic!("a refusal cannot start media")
    });
    assert!(player_startup_pending(&app));
    assert_eq!(app.ott.as_ref().unwrap().play_generation, current);
    app.ott
        .as_ref()
        .unwrap()
        .tx
        .send(Job::Play(current, profile(), Err("current refusal")))
        .unwrap();
    tick_with_start(&mut app, clock, |_, _, _, _, _| {
        panic!("a refusal cannot start media")
    });
    assert!(app.ott.as_ref().unwrap().play_target.is_none());
    assert!(
        !player_startup_pending(&app),
        "the actual destination's refusal remains visible"
    );
}

#[test]
fn viewer_pause_stops_retry_feedback_before_the_next_policy_tick() {
    let _serial = plx_base::testlock::serial();
    let _reset = ResetMedia;
    let mut app = live_failure();
    mount_player(&mut app);
    app.running = true;
    assert!(player_startup_pending(&app));
    plx_media::player::transport::set_paused(true);
    assert!(app
        .ott
        .as_ref()
        .unwrap()
        .continuity
        .pending_retry()
        .is_some());
    assert!(failed_playback::current(app.ott.as_ref().unwrap(), &app).is_some());
    assert!(!player_startup_pending(&app));
}

#[test]
fn exhausted_live_retry_budget_releases_the_working_feedback() {
    let _serial = plx_base::testlock::serial();
    let _reset = ResetMedia;
    let mut app = live_failure();
    mount_player(&mut app);
    app.running = true;
    assert!(player_startup_pending(&app));
    let runtime = app.ott.as_mut().unwrap();
    let generation = runtime.generation;
    for _ in 0..4 {
        let due = runtime
            .continuity
            .pending_retry()
            .expect("bounded retry")
            .not_before_ms;
        assert!(matches!(
            runtime
                .continuity
                .tick(generation, due, &["first", "second"], true),
            continuity::Decision::Start { .. }
        ));
        runtime
            .continuity
            .live_disconnected(generation, "first", due + 1);
    }
    assert!(runtime.continuity.pending_retry().is_none());
    let runtime = app.ott.as_ref().unwrap();
    assert!(failed_playback::current(runtime, &app).is_some());
    assert!(!player_startup_pending(&app));
    // Recovery cleanup retired the native decoder, but its source-bound
    // failure receipt remains available. Presentation must not keep claiming
    // another attempt exists after the bounded policy has exhausted it.
    assert!(app.ott.as_ref().unwrap().play_target.is_none());
    assert_eq!(
        state(&app, runtime)["playback"]["state"],
        "error",
        "the source-bound failure remains reported after recovery exhausts"
    );
}
