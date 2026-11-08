//! Real Runtime admission/retirement boundaries. The retry test uses only the
//! existing resolver's in-memory Plex branch, never network/storage/decoder IO.
use super::*;

fn profile() -> ProviderProfile {
    ProviderProfile { id: "lifecycle-source".into(), name: "Fixture".into(),
        kind: ProviderKind::M3u, source: "https://media.invalid/list.m3u".into(),
        source_alias: String::new(), enabled: true }
}

fn entry() -> CatalogEntry {
    let mut entry = synthetic_entry();
    entry.id = "first".into();
    entry.media = MediaLocator::Http { url: "https://media.invalid/first.m3u8".into() };
    entry
}

fn prepared() -> Box<App> {
    let mut app = Box::new(app_fixture());
    let runtime = app.ott.as_mut().unwrap();
    runtime.loaded = true;
    runtime.saved.profiles.profiles = vec![profile()];
    runtime.saved.profiles.active_profile = Some(profile().id);
    runtime.provider = ProviderKind::M3u;
    runtime.catalog.entries = vec![entry()];
    runtime.catalog_loading = true;
    runtime.catalog_complete = false;
    runtime.catalog_dirty = false;
    runtime.local_inventory.phase = transient::InventoryPhase::Ready(runtime.generation);
    runtime.catalog_done.store(false, Ordering::Release);
    runtime.metadata_done.store(false, Ordering::Release);
    runtime.saving = true;
    runtime
        .continuity
        .user_selected(runtime.generation, "first");
    app
}

fn sleep(app: &mut App) {
    super::super::suspend(app);
    crate::app::ott_standby::set(app, true);
}

#[test]
fn standby_blocks_eager_catalogue_metadata_restore_and_resolver_admission() {
    let _serial = plx_base::testlock::serial();
    let mut app = prepared();
    sleep(&mut app);
    let runtime = app.ott.as_mut().unwrap();
    runtime.catalog_next = Some(profile());
    runtime.metadata_next = Some((profile(), MetadataSelection::Entry(entry())));
    runtime.plex_refresh_requested = true;
    runtime.catalog_done.store(true, Ordering::Release);
    runtime.metadata_done.store(true, Ordering::Release);
    runtime.launch_catalog();
    runtime.launch_metadata();
    runtime.refresh_plex_sources();
    assert!(runtime.catalog_next.is_some());
    assert!(runtime.metadata_next.is_some());
    assert!(runtime.plex_refresh_requested && !runtime.plex_refresh_pending);
    let generation = runtime.play_generation;
    assert!(runtime.queue_play_request(PlaybackRequest::ordinary("first", 0),
        profile(), Some(entry()), None, true).is_err());
    assert_eq!(runtime.play_generation, generation);
    assert!(runtime.play_target.is_none() && runtime.resolver_next.is_none());
}

#[test]
fn suspend_preserves_partial_catalogue_and_pending_saved_target() {
    let _serial = plx_base::testlock::serial();
    let mut app = prepared();
    let runtime = app.ott.as_mut().unwrap();
    runtime.continuity = continuity::Continuity::new(runtime.generation,
        Some(continuity::ResumePoint { entry_id: "late".into(), position_ms: 42_000 }));
    assert!(matches!(runtime.continuity.catalog_updated(runtime.generation, &["first"], false),
        continuity::Decision::Start { reason: continuity::Reason::First, .. }));
    let old_cancel = runtime.cancel.clone();
    sleep(&mut app);
    let runtime = app.ott.as_mut().unwrap();
    assert!(old_cancel.load(Ordering::Acquire));
    assert_eq!(runtime.catalog.entries[0].id, "first");
    assert_eq!(runtime.continuity.pending_restore().unwrap().entry_id, "late");
    assert_eq!(runtime.continuity.catalog_updated(runtime.generation, &["first", "late"], true),
        continuity::Decision::Idle);
    assert!(!runtime.catalog_done.load(Ordering::Acquire));
}

#[test]
fn old_catalogue_and_metadata_results_do_not_mutate_suspended_state() {
    let _serial = plx_base::testlock::serial();
    let mut app = prepared();
    let generation = app.ott.as_ref().unwrap().generation;
    let metadata_generation = app.ott.as_ref().unwrap().metadata_generation;
    app.ott.as_mut().unwrap().metadata_target = Some((profile().id, "first".into()));
    sleep(&mut app);
    let runtime = app.ott.as_ref().unwrap();
    let mut late = entry(); late.id = "late".into();
    runtime.catalog_tx.send(CatalogEvent::Batch(generation, Catalog { hierarchy: None, entries: vec![late] })).unwrap();
    runtime.metadata_tx.send(crate::app::ott_metadata::Event::Preview(crate::app::ott_metadata::Preview {
        generation: metadata_generation, profile_id: profile().id, entry_id: "first".into(),
        entry: None, result: Ok(json!({"description":"stale description"})),
    })).unwrap();
    runtime_tick(&mut app);
    let runtime = app.ott.as_ref().unwrap();
    assert_eq!(runtime.catalog.entries.len(), 1);
    assert_ne!(runtime.catalog.entries[0].description, "stale description");
    assert!(runtime.catalog_rx.try_recv().is_err());
    assert!(runtime.metadata_rx.try_recv().is_err());
}

#[test]
fn waking_waits_for_original_worker_retirement_and_keeps_newer_queued_source() {
    let _serial = plx_base::testlock::serial();
    let mut app = prepared();
    sleep(&mut app);
    let mut next = profile(); next.id = "newer-source".into();
    app.ott.as_mut().unwrap().catalog_next = Some(next.clone());
    crate::app::ott_standby::set(&mut app, false);
    super::super::resume(&mut app);
    let mut runtime = app.ott.take().unwrap();
    runtime.rearm_source_work(&mut app);
    assert!(!runtime.catalog_work_allowed());
    assert_eq!(runtime.catalog_next.as_ref().unwrap().id, next.id);
    assert!(!runtime.catalog_done.load(Ordering::Acquire));
    runtime.catalog_done.store(true, Ordering::Release);
    runtime.rearm_source_work(&mut app);
    assert!(runtime.catalog_work_allowed());
    assert_eq!(runtime.catalog_next.as_ref().unwrap().id, next.id);
    app.ott = Some(runtime);
}

#[test]
fn cancelled_plex_result_is_drained_without_publishing_an_old_failure() {
    let _serial = plx_base::testlock::serial();
    let mut app = prepared();
    let runtime = app.ott.as_mut().unwrap();
    let token = runtime.begin_plex_restore();
    runtime.plex_refresh_pending = true;
    sleep(&mut app);
    let runtime = app.ott.as_ref().unwrap();
    runtime.tx.send(Job::PlexRestored(token,
        vec![Err("retired source must not publish")])).unwrap();
    runtime_tick(&mut app);
    let runtime = app.ott.as_ref().unwrap();
    assert!(!runtime.plex_refresh_pending);
    assert!(runtime.plex_refresh_requested);
    assert_ne!(runtime.status, "retired source must not publish");
}

#[test]
fn late_plex_result_does_not_release_a_newer_worker_slot() {
    let _serial = plx_base::testlock::serial();
    let mut runtime = runtime_fixture();
    runtime.resume_source_work();
    let old = runtime.begin_plex_restore();
    old.store(true, Ordering::Release);
    let current = runtime.begin_plex_restore();
    runtime.plex_refresh_pending = true;
    assert!(!runtime.accept_plex_restore(&old));
    assert!(runtime.plex_refresh_pending);
    assert!(runtime.accept_plex_restore(&current));
    assert!(!runtime.plex_refresh_pending);
}

#[test]
fn suspended_metadata_rebuild_waits_for_local_ready_and_retains_only_logical_target() {
    let _serial = plx_base::testlock::serial();
    let mut app = prepared();
    let runtime = app.ott.as_mut().unwrap();
    let favorite = Favorite {
        title: "Film".into(), origin: plx_data::ott::favorites::Origin {
            provider: "local".into(), profile: "Home".into(), subprofile: None,
            source_alias: "Movies".into(),
        }, locator: plx_data::ott::favorites::Locator::Local {
            root_alias: "Movies".into(), relative_path: "Film.mp4".into(),
        },
    };
    let local_profile = ProviderProfile { id: "local-fixture".into(), name: "Home".into(),
        kind: ProviderKind::Local, source: "/fixture/Movies".into(), source_alias: "Movies".into(), enabled: true };
    let mut local_entry = entry();
    local_entry.id = "Film.mp4".into();
    local_entry.title = "Film".into();
    local_entry.media = MediaLocator::Local { relative_path: "Film.mp4".into() };
    runtime.saved.profiles.profiles.push(local_profile.clone());
    runtime.saved.favorites.toggle(favorite.clone()).unwrap();
    let bindings = selection::SelectionBindings::profiles_only(&runtime.saved.profiles);
    runtime.saved.local_favorite_proofs.record_capture(&bindings, &runtime.saved.favorites,
        selection::SelectionOwner::Profile(&local_profile), &[local_entry.clone()], &local_entry).unwrap();
    let permit = runtime.favorite_permit(&local_profile, &favorite).unwrap();
    assert!(permit.is_some(), "the fixture must queue an actual receiving-device permit");
    let target = (local_profile.id.clone(), format!("favorite:{}", favorite.identity()));
    runtime.metadata_target = Some(target.clone());
    runtime.metadata_next = Some((local_profile, MetadataSelection::Favorite(favorite.clone(), permit)));
    sleep(&mut app);
    assert!(app.ott.as_ref().unwrap().metadata_next.is_none());
    crate::app::ott_standby::set(&mut app, false);
    super::super::resume(&mut app);
    let mut runtime = app.ott.take().unwrap();
    runtime.local_inventory.phase = transient::InventoryPhase::Loading(runtime.generation);
    runtime.metadata_done.store(true, Ordering::Release);
    runtime.rearm_metadata_work();
    runtime.launch_metadata();
    assert!(!runtime.metadata_work_allowed());
    assert_eq!(runtime.metadata_target, Some(target));
    assert!(runtime.metadata_next.is_none());
    // The old Favorite was removed before wake. The real refresh path must
    // clear its same-target shortcut and reject rebinding against current Saved.
    runtime.saved.favorites.toggle(favorite).unwrap();
    runtime.local_inventory.phase = transient::InventoryPhase::Ready(runtime.generation);
    runtime.rearm_metadata_work();
    runtime.refresh_metadata();
    assert!(runtime.metadata_target.is_none());
    assert!(runtime.metadata_next.is_none());
    app.ott = Some(runtime);
}

struct ResetControl;
impl ResetControl {
    fn new() -> Self {
        plx_media::player::SHARED.reset_session();
        plx_media::player::TX.reset();
        plx_media::route::reset_player_control_for_test(&plx_media::route::PlaybackSession::IDLE);
        Self
    }
}
impl Drop for ResetControl {
    fn drop(&mut self) {
        // The test's App drops before this guard. The outer serial guard also
        // drains the existing task worker before releasing global test ownership.
        plx_base::task::drain_workers_for_test();
        plx_media::player::SHARED.reset_session();
        plx_media::player::TX.reset();
        plx_media::route::reset_player_control_for_test(&plx_media::route::PlaybackSession::IDLE);
    }
}

fn failed_pinned_request() -> Box<App> {
    let mut app = prepared();
    let mut runtime = app.ott.take().unwrap();
    let mut source = profile();
    source.kind = ProviderKind::Plex;
    source.source = "inert-kiosk-server".into();
    let mut row = entry();
    row.media = MediaLocator::Plex {
        server_id: source.source.clone(), rating_key: "1".into(),
    };
    runtime.provider = ProviderKind::Plex;
    runtime.saved.profiles.profiles = vec![source.clone()];
    runtime.catalog.entries = vec![row];
    runtime.saving = false;
    let prepared = kiosk_source::prepare(&runtime, &app, Some("1"), false).unwrap();
    let saved = kiosk_source::save_candidate(&runtime, &prepared).unwrap();
    // Exercise the real successful Saved boundary; no protected writer is run.
    runtime.saving = true;
    runtime.tx.send(Job::Saved(Ok(saved), None, false, json!({}))).unwrap();
    app.ott = Some(runtime);
    runtime_tick(&mut app);
    runtime = app.ott.take().unwrap();
    // First admission queues behind the held existing slot, without starting IO.
    runtime.resolver_busy = true;
    let attempt = kiosk_source::activate(&mut runtime, &mut app, &prepared).unwrap().unwrap();
    runtime.note_kiosk_attempt(attempt);
    let generation = runtime.play_generation;
    let taken = runtime.resolver_next.take().expect("the admitted worker request exists");
    assert_eq!(taken.generation, generation);
    assert_eq!(taken.profile.id, source.id);
    runtime.tx.send(Job::Play(generation, source, Err("fixture resolve failed"))).unwrap();
    app.ott = Some(runtime);
    runtime_tick(&mut app);
    assert!(plx_media::route::playback_identity(&app.player.session).is_none());
    let runtime = app.ott.as_ref().unwrap();
    assert!(runtime.play_target.is_none() && !runtime.resolver_busy);
    assert_eq!(runtime.kiosk_health(&app).unwrap().health.as_str(), "error");
    app
}

#[test]
fn initial_pinned_resolver_failure_retries_real_admission_after_ten_seconds() {
    let _serial = plx_base::testlock::serial();
    let _reset = ResetControl::new();
    let mut app = failed_pinned_request();
    let mut runtime = app.ott.take().unwrap();
    let now = runtime.now_ms();
    let generation = runtime.play_generation;
    runtime.kiosk_recovery_tick_at(&mut app, now + 9_999);
    assert_eq!(runtime.play_generation, generation);
    assert_eq!(runtime.kiosk_health(&app).unwrap().retries, 0);
    runtime.kiosk_recovery_tick_at(&mut app, now + 10_000);
    assert_ne!(runtime.play_generation, generation);
    assert!(runtime.play_target.is_some());
    assert_eq!(runtime.kiosk_health(&app).unwrap().retries, 1);
    assert!(plx_media::route::playback_identity(&app.player.session).is_none(),
        "retry admission must not be claimed as decoded playback");
    // Do not run a subsequent main tick: the existing worker only creates an
    // inert Plex ReadySource. No native request/decoder/network can be started.
    runtime.cancel_play_request();
    app.ott = Some(runtime);
}

#[test]
fn initial_retry_cannot_cross_a_changed_source() {
    let _serial = plx_base::testlock::serial();
    let _reset = ResetControl::new();
    let mut app = failed_pinned_request();
    let mut runtime = app.ott.take().unwrap();
    let generation = runtime.play_generation;
    let now = runtime.now_ms();
    runtime.saved.profiles.profiles[0].source = "different-inert-server".into();
    runtime.kiosk_recovery_tick_at(&mut app, now + 60_000);
    assert_eq!(runtime.play_generation, generation);
    assert!(runtime.play_target.is_none());
    app.ott = Some(runtime);
}

// Keep whole-Runtime move temporaries out of the test frame while constructing
// the nested App fixture. Both calls still exercise the real retry boundary.
#[inline(never)]
fn assert_initial_retry_stays_blocked(app: &mut App, at_ms: u64) {
    let mut runtime = app.ott.take().unwrap();
    let generation = runtime.play_generation;
    runtime.kiosk_recovery_tick_at(app, at_ms);
    assert_eq!(runtime.play_generation, generation);
    assert!(runtime.play_target.is_none());
    app.ott = Some(runtime);
}

#[test]
fn initial_retry_cannot_cross_viewer_pause_or_standby() {
    let _serial = plx_base::testlock::serial();
    let _reset = ResetControl::new();
    let mut app = failed_pinned_request();
    // Real inert native actuator supplies an accepted Pause; no decoder starts.
    app.adapters.player.install(plx_media::player::engine::engine_after_reload_for_test());
    assert!(plx_media::player::pause(&mut app.adapters.player));
    let now = app.ott.as_ref().unwrap().now_ms();
    assert_initial_retry_stays_blocked(&mut app, now + 60_000);
    sleep(&mut app);
    assert_initial_retry_stays_blocked(&mut app, now + 120_000);
}

#[test]
fn initial_retry_cannot_cross_a_newer_native_admission_ticket() {
    let _serial = plx_base::testlock::serial();
    let _reset = ResetControl::new();
    let mut app = failed_pinned_request();
    let mut runtime = app.ott.take().unwrap();
    let generation = runtime.play_generation;
    let now = runtime.now_ms();
    // The actual route fence invalidates the predecessor's request ticket.
    plx_media::route::cancel_play(&mut app.player.session);
    let ticket = plx_media::route::play_request_ticket();
    runtime.kiosk_recovery_tick_at(&mut app, now + 60_000);
    assert_eq!(runtime.play_generation, generation);
    assert!(plx_media::route::play_request_is_current(ticket));
    assert!(runtime.play_target.is_none());
    app.ott = Some(runtime);
}

#[test]
fn failed_initial_pin_survives_already_backgrounded_suspend_and_rebinds_on_wake() {
    let _serial = plx_base::testlock::serial();
    let _reset = ResetControl::new();
    let mut app = failed_pinned_request();
    // Real WindowActivity receives DID background before Runtime::suspend, as
    // happens at the OS event boundary. Retention must not require foreground.
    app.window_activity.event(0x104);
    assert!(!super::super::playback_allowed(&app));
    super::super::suspend(&mut app);
    let runtime = app.ott.as_ref().unwrap();
    let retained = runtime.resume_pending.as_ref().expect("current admitted pin is retained");
    assert!(retained.kiosk_selection.is_some());
    assert!(runtime.play_target.is_none());
    assert!(!plx_media::route::play_pending());
    app.window_activity.event(0x106);
    super::super::resume(&mut app);
    let mut runtime = app.ott.take().unwrap();
    runtime.catalog_done.store(true, Ordering::Release);
    runtime.rearm_source_work(&mut app);
    let retained = runtime.resume_pending.as_ref().unwrap();
    assert_eq!(transient::lifecycle_resume_ready(&runtime, retained), Ok(true));
    let id = retained.id.clone();
    // The real resume tick uses the binder and queues behind an occupied
    // resolver. Hold the catalogue slot too, so no unrelated IO can start.
    runtime.resolver_busy = true;
    runtime.catalog_done.store(false, Ordering::Release);
    app.ott = Some(runtime);
    runtime_tick(&mut app);
    let runtime = app.ott.as_mut().unwrap();
    assert!(runtime.resume_pending.is_none());
    assert_eq!(runtime.play_target.as_ref().unwrap().id, id);
    let queued = runtime.resolver_next.take().expect("wake actually admitted a fresh request");
    assert!(plx_media::route::playback_identity(&app.player.session).is_none());
    runtime.tx.send(Job::Play(queued.generation, queued.profile,
        Err("fixture resolve failed after wake"))).unwrap();
    runtime_tick(&mut app);
    assert_eq!(app.ott.as_ref().unwrap().kiosk_health(&app).unwrap().health.as_str(), "error",
        "a second failure must retain the new admitted retry, not the cancelled ticket");
}

#[test]
fn retained_initial_pin_rejects_changed_policy_or_source_at_the_real_binder() {
    let _serial = plx_base::testlock::serial();
    let _reset = ResetControl::new();
    for policy_changed in [true, false] {
        let mut app = failed_pinned_request();
        sleep(&mut app);
        let runtime = app.ott.as_mut().unwrap();
        assert!(runtime.resume_pending.as_ref().unwrap().kiosk_selection.is_some());
        if policy_changed {
            runtime.saved.settings.remove("nativeKiosk");
        } else {
            runtime.saved.profiles.profiles[0].source = "replacement-server".into();
        }
        crate::app::ott_standby::set(&mut app, false);
        super::super::resume(&mut app);
        let mut runtime = app.ott.take().unwrap();
        runtime.catalog_done.store(true, Ordering::Release);
        runtime.rearm_source_work(&mut app);
        let retained = runtime.resume_pending.take().unwrap();
        runtime.resolver_busy = true;
        assert!(runtime.play_request(retained).is_err());
        assert!(runtime.play_target.is_none() && runtime.resolver_next.is_none());
        app.ott = Some(runtime);
    }
}

#[test]
fn stale_initial_pin_cannot_be_retained_after_a_new_native_request_ticket() {
    let _serial = plx_base::testlock::serial();
    let _reset = ResetControl::new();
    let mut app = failed_pinned_request();
    plx_media::route::cancel_play(&mut app.player.session);
    app.window_activity.event(0x104);
    super::super::suspend(&mut app);
    assert!(app.ott.as_ref().unwrap().resume_pending.is_none());
}

#[test]
fn failed_initial_pin_never_overwrites_an_existing_continuation() {
    let _serial = plx_base::testlock::serial();
    let _reset = ResetControl::new();
    let mut app = failed_pinned_request();
    let newer = PlaybackRequest::ordinary("newer-choice", 4_000);
    app.ott.as_mut().unwrap().resume_pending = Some(newer.clone());
    super::super::suspend(&mut app);
    assert!(app.ott.as_ref().unwrap().resume_pending.as_ref() == Some(&newer));
}

#[test]
fn strict_only_saved_change_preserves_initial_failure_retry_without_restarting_now() {
    let _serial = plx_base::testlock::serial();
    let _reset = ResetControl::new();
    let mut app = failed_pinned_request();
    let runtime = app.ott.as_mut().unwrap();
    let generation = runtime.play_generation;
    let mut saved = runtime.saved.clone();
    let previous = crate::app::ott_kiosk_stored::read(&saved.settings).unwrap().unwrap();
    saved.settings.insert(crate::app::ott_kiosk_stored::KEY.into(),
        previous.with_strict(!previous.strict()).encode().unwrap());
    runtime.saving = true;
    runtime.tx.send(Job::Saved(Ok(saved), None, false, json!({}))).unwrap();
    runtime_tick(&mut app);
    let mut runtime = app.ott.take().unwrap();
    assert_eq!(runtime.play_generation, generation);
    assert!(runtime.play_target.is_none());
    assert_eq!(runtime.kiosk_health(&app).unwrap().health.as_str(), "error");
    let now = runtime.now_ms();
    runtime.kiosk_recovery_tick_at(&mut app, now + 10_000);
    assert_ne!(runtime.play_generation, generation);
    assert_eq!(runtime.kiosk_health(&app).unwrap().retries, 1);
    runtime.cancel_play_request();
    app.ott = Some(runtime);
}
