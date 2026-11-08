//! Real Loaded/preparation/suspend boundaries using the existing synthetic Plex
//! authority. The prior resolver slot is held, so no private/network IO runs.
use super::*;
use crate::app::ott_kiosk_stored::{self as stored, Stored};

fn loaded_queue(background: bool, newer_navigation: bool) -> App {
    loaded_queue_from(background, newer_navigation, None)
}
fn loaded_queue_from(background: bool, newer_navigation: bool,
    endpoint: Option<&super::kiosk_hooks::RetryEndpoint>) -> App
{
    let mut app = setup();
    if let Some(endpoint) = endpoint {
        endpoint.register_named(&mut app, "boot-queue");
    } else {
        register_source(&mut app, "boot-queue");
    }
    let entries = kiosk_entries(&app, "boot-queue", &["42", "42", "7"]);
    if background { app.window_activity.event(0x104); }
    let runtime = app.ott.as_mut().unwrap();
    let mut saved = runtime.saved.clone();
    saved.settings.insert(stored::KEY.into(), Stored::queue(entries, true).unwrap().encode().unwrap());
    runtime.loaded = false;
    runtime.plex_refresh_pending = true;
    if newer_navigation { runtime.navigation_epoch = 1; }
    runtime.tx.send(Job::Loaded(Ok((saved, [1;16])))).unwrap();
    runtime_tick(&mut app);
    app
}
fn owner(app: &App) -> OwnerToken {
    match app.ott.as_ref().unwrap().transient.candidate.as_ref().expect("real candidate") {
        transient::Candidate::Plex(c) => c.work.owner,
        _ => panic!("Plex preparation required"),
    }
}
fn fail_preparation(app: &mut App, expected: OwnerToken) {
    let runtime = app.ott.as_mut().unwrap();
    let queued = runtime.resolver_next.take().expect("real queued work");
    assert!(queued.plex.is_some());
    runtime.tx.send(Job::PlexPrepared(expected,
        transient::plex_remote::PreparationId::KioskRestore,
        Err(queue::WireError::Queue(queue::QueueError::MissingItem)))).unwrap();
    runtime_tick(app);
}
fn wake_without_releasing_worker(app: &mut App) {
    app.window_activity.event(0x106);
    super::super::super::resume(app);
    let mut runtime = app.ott.take().unwrap();
    runtime.catalog_done.store(true, Ordering::Release);
    runtime.rearm_source_work(app);
    runtime.catalog_done.store(false, Ordering::Release);
    app.ott = Some(runtime);
}

#[test]
fn loaded_queue_uses_actual_typed_restore_and_existing_single_flight_slot() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let app = loaded_queue(false, false);
    let runtime = app.ott.as_ref().unwrap();
    let _ = owner(&app);
    assert!(runtime.resolver_busy, "predecessor slot must remain owned until its result");
    assert!(runtime.resolver_next.as_ref().unwrap().plex.is_some());
    assert!(runtime.play_target.is_none());
    assert!(runtime.transient.accepted.is_none());
    assert_ne!(runtime.kiosk_health(&app).unwrap().health.as_str(), "playing");
}

#[test]
fn newer_navigation_blocks_loaded_queue_startup() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let app = loaded_queue(false, true);
    let runtime = app.ott.as_ref().unwrap();
    assert!(runtime.transient.candidate.is_none());
    assert!(runtime.resolver_next.is_none());
}

#[test]
fn background_loaded_queue_waits_then_starts_after_foreground_revalidation() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let mut app = loaded_queue(true, false);
    assert!(app.ott.as_ref().unwrap().transient.candidate.is_none());
    wake_without_releasing_worker(&mut app);
    runtime_tick(&mut app);
    let _ = owner(&app);
    assert!(app.ott.as_ref().unwrap().resolver_busy);
}

#[test]
fn actual_terminal_preparation_failure_retries_after_ten_seconds() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let endpoint = super::kiosk_hooks::RetryEndpoint::new();
    let mut app = loaded_queue_from(false, false, Some(&endpoint));
    let old = owner(&app);
    fail_preparation(&mut app, old);
    assert_eq!(app.ott.as_ref().unwrap().kiosk_health(&app).unwrap().health.as_str(), "error");
    let mut runtime = app.ott.take().unwrap();
    let now = runtime.now_ms();
    // The addressed failure already released the only resolver slot. Keeping
    // it artificially busy would correctly refuse a retry, not test its timer.
    assert!(!runtime.resolver_busy);
    runtime.kiosk_recovery_tick_at(&mut app, now + 9_999);
    assert!(runtime.transient.candidate.is_none());
    runtime.kiosk_recovery_tick_at(&mut app, now + 10_000);
    let retries = runtime.kiosk_health(&app).ok().map(|health| health.retries);
    let next_owner = match runtime.transient.candidate.as_ref() {
        Some(transient::Candidate::Plex(candidate)) => Some(candidate.work.owner),
        _ => None,
    };
    // This is a real retry worker, limited to the inert loopback endpoint.
    // Cancel and drain it before assertions can unwind the serial registry guard.
    transient::plex_remote::cancel_shutdown(&runtime);
    let drained = if runtime.resolver_busy {
        matches!(runtime.rx.recv_timeout(Duration::from_secs(10)), Ok(Job::PlexPrepared(..)))
    } else {
        true
    };
    runtime.resolver_busy = false;
    assert!(drained, "the loopback preparation worker must finish");
    assert_eq!(retries, Some(1));
    assert!(next_owner.is_some_and(|next| next != old));
    app.ott = Some(runtime);
}

#[test]
fn suspend_during_preparation_retains_intent_and_wake_queues_a_fresh_attempt() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let mut app = loaded_queue(false, false);
    let old = owner(&app);
    app.window_activity.event(0x104);
    super::super::super::suspend(&mut app);
    assert!(app.ott.as_ref().unwrap().transient.candidate.is_none());
    assert!(app.ott.as_ref().unwrap().resolver_busy);
    wake_without_releasing_worker(&mut app);
    runtime_tick(&mut app);
    assert!(owner(&app) != old);
    assert!(app.ott.as_ref().unwrap().resolver_busy,
        "fresh work is queued; original producer was not falsely marked done");
}

#[test]
fn newer_native_intent_after_preparation_suspend_prevents_wake_restore() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let mut app = loaded_queue(false, false);
    app.window_activity.event(0x104);
    super::super::super::suspend(&mut app);
    plx_media::route::cancel_play(&mut app.player.session);
    wake_without_releasing_worker(&mut app);
    runtime_tick(&mut app);
    assert!(app.ott.as_ref().unwrap().transient.candidate.is_none());
    assert!(app.ott.as_ref().unwrap().resolver_next.is_none());
}

fn first_request_queued() -> App {
    let mut app = loaded_queue(false, false);
    let first_owner = owner(&app);
    let sid = plx_plex::plex::id_of_machine("boot-queue").unwrap();
    let mut runtime = app.ott.take().unwrap();
    let work = runtime.resolver_next.take().unwrap();
    assert!(work.plex.is_some());
    transient::plex_remote::prepared(&mut runtime, &mut app, first_owner,
        transient::plex_remote::PreparationId::KioskRestore,
        Ok(proposal(first_owner, sid, prepare::Prepared::Queue {
            kind: queue::PrepareKind::Play,
            selection: selection(&["42", "42", "7"]),
        })));
    assert!(runtime.play_target.as_ref().is_some_and(|request|
        request.transient_owner == Some(first_owner)));
    assert!(runtime.resolver_next.is_some());
    assert!(runtime.playing.is_none());
    app.ott = Some(runtime);
    app
}

#[test]
fn suspend_after_first_queue_request_retains_preparation_not_stale_occurrence() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let mut app = first_request_queued();
    let old = owner(&app);
    app.window_activity.event(0x104);
    super::super::super::suspend(&mut app);
    let runtime = app.ott.as_ref().unwrap();
    assert!(runtime.transient.candidate.is_none());
    assert!(runtime.play_target.is_none());
    assert!(runtime.resume_pending.is_none(),
        "the old staged occurrence must not shadow retained preparation intent");
    wake_without_releasing_worker(&mut app);
    runtime_tick(&mut app);
    assert!(owner(&app) != old);
    assert!(app.ott.as_ref().unwrap().resolver_busy);
}

#[test]
fn unrelated_target_is_not_replaced_by_preparation_retention() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let mut app = first_request_queued();
    let mut runtime = app.ott.take().unwrap();
    let mut row = live_entry();
    row.id = "newer-target".into();
    let request = PlaybackRequest::ordinary(&row.id, 5_000);
    // Actual shared queue admission, still behind the occupied resolver slot.
    // Its distinct owner must not be mistaken for the prior staged occurrence.
    runtime.queue_play_request(request.clone(), permanent(), Some(row), None, false).unwrap();
    app.ott = Some(runtime);
    app.window_activity.event(0x104);
    super::super::super::suspend(&mut app);
    assert!(app.ott.as_ref().unwrap().resume_pending.as_ref() == Some(&request));
    assert!(app.ott.as_ref().unwrap().kiosk_preparation_receipt_for_test().is_none());
}

#[test]
fn pending_readiness_timeout_never_fakes_cancellation_or_starts_second_worker() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let mut app = loaded_queue(false, false);
    let mut runtime = app.ott.take().unwrap();
    let receipt = runtime.kiosk_preparation_receipt_for_test().unwrap();
    let now = runtime.now_ms();
    runtime.kiosk_recovery_tick_at(&mut app, now + 59_999);
    assert!(runtime.kiosk_preparation_receipt_for_test().unwrap().same_attempt(&receipt));
    runtime.kiosk_recovery_tick_at(&mut app, now + 60_000);
    // The owner may time out its exact operation, but a legitimately occupied
    // worker must answer Busy, not ContextChanged merely because it is pending.
    assert!(runtime.kiosk_preparation_receipt_for_test().is_some(),
        "a readiness deadline must not silently disable recovery");
    assert!(runtime.resolver_busy);
    assert_eq!(runtime.kiosk_health(&app).unwrap().retries, 0);
    assert_ne!(runtime.kiosk_health(&app).unwrap().health.as_str(), "playing");
    app.ott = Some(runtime);
}

#[test]
fn native_request_and_exit_while_loaded_is_pending_revoke_all_kiosk_startup_paths() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    for queue_pin in [false, true] {
        // Runtime::unloaded captured the original native ticket in setup.
        let mut app = setup();
        let sid = register_source(&mut app, "boot-queue");
        let entries = kiosk_entries(&app, "boot-queue", &["42"]);
        let runtime = app.ott.as_mut().unwrap();
        let mut saved = runtime.saved.clone();
        saved.profiles.active_profile = Some("plex-boot-queue".into());
        let pin = if queue_pin { Stored::queue(entries, true) }
            else { Stored::item(entries[0].clone(), true) }.unwrap();
        saved.settings.insert(stored::KEY.into(), pin.encode().unwrap());
        runtime.loaded = false;
        runtime.plex_refresh_pending = true;
        // The existing native request fence and retained-request fixture model
        // the newer choice without starting a resolver or accessing a server.
        plx_media::route::cancel_play(&mut app.player.session);
        plx_media::route::install_plex_request_for_test(&mut app.player.session, sid, "77");
        assert!(plx_media::route::playback_request_identity(&app.player.session).is_some());
        playback::replace_player(&mut app.player, &mut app.adapters.player,
            &mut app.repause_at, &mut app.refresh_hubs, &mut app.pages);
        assert!(!plx_media::route::play_pending());
        assert!(!app.adapters.player.is_live());
        // Teardown may retain historical retry metadata, but no active native
        // request or Engine remains; only the original ticket detects history.
        app.ott.as_ref().unwrap().tx.send(Job::Loaded(Ok((saved, [1;16])))).unwrap();
        runtime_tick(&mut app);
        let runtime = app.ott.as_ref().unwrap();
        assert!(runtime.resume_pending.is_none());
        assert!(runtime.transient.candidate.is_none());
        assert!(runtime.resolver_next.is_none());
        // A subsequent matching row must not bypass the revoked startup owner
        // through normal catalogue auto-start, either.
        let mut row = live_entry();
        row.id = "42".into();
        row.media = MediaLocator::Plex { server_id: "boot-queue".into(), rating_key: "42".into() };
        runtime.catalog_tx.send(CatalogEvent::Batch(runtime.generation,
            Catalog { hierarchy: None, entries: vec![row] })).unwrap();
        runtime_tick(&mut app);
        assert!(app.ott.as_ref().unwrap().play_target.is_none());
        assert!(app.ott.as_ref().unwrap().resolver_next.is_none());
    }
}
