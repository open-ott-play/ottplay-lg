//! Real Runtime and root-handle boundaries. Existing catalogue/resolver slots
//! remain held: these regressions do not start a scanner, decoder or network IO.
use super::*;
use crate::app::ott_kiosk_stored::{self as stored, Stored};
use crate::app::ott_runtime::work_lifecycle::RestoreStartError;
use plx_data::ott::favorites::Origin;

fn fixture() -> (TempDir, Box<App>, local::BoundSource, String) {
    let (directory, mut app, root) = setup();
    let sample = plx_base::task::time::ClockSample::capture_main(app.adapters.player.mt()).unwrap();
    let runtime = app.ott.as_mut().unwrap();
    runtime.clock_sample = Some(sample);
    runtime.resume_source_work();
    runtime.saving = false;
    runtime.resolver_busy = false;
    runtime.saved.category = "Saved category".into();
    let fingerprint = selection::local_root_fingerprint(&root).unwrap();
    let policy = Stored::local_folder(
        Origin {
            provider: "local".into(),
            profile: "Movies".into(),
            subprofile: None,
            source_alias: root.name.clone(),
        },
        true,
        root.id.clone(),
        fingerprint.clone(),
    )
    .unwrap();
    runtime
        .saved
        .settings
        .insert(stored::KEY.into(), policy.encode().unwrap());
    (directory, app, root, fingerprint)
}
fn start(
    app: &mut App,
    root: &local::BoundSource,
    fingerprint: &str,
) -> transient::KioskPreparation {
    let mut runtime = app.ott.take().unwrap();
    assert_eq!(
        transient::kiosk_restore_local_folder(&mut runtime, app, &root.id, fingerprint),
        Ok(())
    );
    let receipt = runtime.kiosk_preparation_receipt_for_test().unwrap();
    assert!(transient::kiosk_preparation_is_current(
        &runtime, app, &receipt
    ));
    app.ott = Some(runtime);
    receipt
}
fn first_row(app: &mut App) {
    let mut runtime = app.ott.take().unwrap();
    runtime.resolver_busy = true;
    let mut incoming = Catalog { hierarchy: None,
        entries: vec![item("new-first")],
    };
    assert!(transient::scan_result(
        &mut runtime,
        app,
        &mut incoming,
        &mut None
    ));
    assert!(incoming.entries.is_empty());
    assert_eq!(runtime.play_target.as_ref().unwrap().id, "new-first");
    assert!(runtime.resolver_next.is_some());
    assert!(runtime.pending_play.is_none());
    app.ott = Some(runtime);
}

#[test]
fn authorized_unremembered_folder_uses_same_candidate_without_durable_provider_change() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let (_directory, mut app, root, fingerprint) = fixture();
    let before = serde_json::to_value(&app.ott.as_ref().unwrap().saved).unwrap();
    assert!(crate::app::ott_local_sources::RememberedFolders::read(
        &app.ott.as_ref().unwrap().saved.settings
    )
    .unwrap()
    .folders()
    .is_empty());
    start(&mut app, &root, &fingerprint);
    let runtime = app.ott.as_ref().unwrap();
    assert!(matches!(
        runtime.transient.candidate,
        Some(transient::Candidate::Local(transient::LocalCandidate {
            purpose: transient::LocalPurpose::KioskFolder(_),
            phase: transient::CandidatePhase::Scanning { .. },
            ..
        }))
    ));
    assert!(runtime.pending_play.is_none() && runtime.resolver_next.is_none());
    assert!(
        !runtime.catalog_done.load(Ordering::Acquire),
        "occupied scanner was not cleared"
    );
    assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), before);
}

#[test]
fn first_prefix_queues_exact_local_entry_and_staged_binder_rejects_changed_request() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let (_directory, mut app, root, fingerprint) = fixture();
    start(&mut app, &root, &fingerprint);
    first_row(&mut app);
    let runtime = app.ott.as_ref().unwrap();
    let request = runtime.play_target.as_ref().unwrap().clone();
    let (profile, entry) = transient::staged_native_binding(runtime, &request)
        .unwrap()
        .unwrap();
    assert_eq!(profile.kind, ProviderKind::Local);
    assert_eq!(entry.id, "new-first");
    assert_ne!(
        runtime.catalog.entries[0].id, entry.id,
        "staged entry is outside old accepted catalogue"
    );
    for changed in 0..5 {
        let mut altered = request.clone();
        match changed {
            0 => altered.local_selection.as_mut().unwrap().generation += 1,
            1 => altered.local_selection = None,
            2 => altered.id = "first".into(),
            3 => altered.position_ms = 1,
            _ => altered.advance_on_failure = true,
        }
        assert!(transient::staged_native_binding(runtime, &altered)
            .unwrap()
            .is_err());
    }
}

#[test]
fn started_receipt_allows_only_its_exact_first_pending_target_on_suspend() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let (_directory, mut app, root, fingerprint) = fixture();
    let receipt = start(&mut app, &root, &fingerprint);
    first_row(&mut app);
    let mut runtime = app.ott.take().unwrap();
    app.ott_standby = true;
    assert!(transient::kiosk_preparation_resume_policy(&runtime, &app, &receipt).is_some());
    runtime.play_target.as_mut().unwrap().id = "foreign".into();
    assert!(transient::kiosk_preparation_resume_policy(&runtime, &app, &receipt).is_none());
    app.ott = Some(runtime);
}

#[test]
fn resume_policy_uses_current_strictness_and_rejects_revoked_root() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let (_directory, mut app, root, fingerprint) = fixture();
    let receipt = start(&mut app, &root, &fingerprint);
    let mut runtime = app.ott.take().unwrap();
    let policy = stored::read(&runtime.saved.settings)
        .unwrap()
        .unwrap()
        .with_strict(false);
    runtime
        .saved
        .settings
        .insert(stored::KEY.into(), policy.encode().unwrap());
    assert!(
        !transient::kiosk_preparation_resume_policy(&runtime, &app, &receipt)
            .unwrap()
            .strict()
    );
    runtime.local_inventory.sources[0].withdraw();
    assert!(transient::kiosk_preparation_resume_policy(&runtime, &app, &receipt).is_none());
    app.ott = Some(runtime);
}

#[test]
fn own_failed_admission_rebases_receipt_but_manual_generation_change_invalidates_it() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let (_directory, mut app, root, fingerprint) = fixture();
    let original = start(&mut app, &root, &fingerprint);
    first_row(&mut app);
    let mut runtime = app.ott.take().unwrap();
    let old_generation = runtime.generation;
    let before = serde_json::to_value(&runtime.saved).unwrap();
    assert!(
        transient::finish_native(&mut runtime, &mut app, Err("fixture native failure"), None).is_some()
    );
    let failed = runtime.kiosk_preparation_receipt_for_test().unwrap();
    assert!(failed.same_attempt(&original));
    assert_ne!(runtime.generation, old_generation);
    // The real handler clears the consumed target after finish_native.
    runtime.play_target = None;
    runtime.resolver_next = None;
    assert!(transient::kiosk_preparation_is_current(
        &runtime, &app, &failed
    ));
    assert_eq!(
        transient::retry_kiosk_preparation(&mut runtime, &mut app, &failed),
        Err(RestoreStartError::Busy)
    );
    assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), before);
    runtime.generation = runtime.generation.wrapping_add(1);
    runtime.cancel = Arc::new(AtomicBool::new(false));
    assert!(!transient::kiosk_preparation_is_current(
        &runtime, &app, &failed
    ));
    app.ott = Some(runtime);
}

#[test]
fn existing_watchdog_at_sixty_seconds_retires_pending_with_retryable_timeout() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let (_directory, mut app, root, fingerprint) = fixture();
    let receipt = start(&mut app, &root, &fingerprint);
    let mut runtime = app.ott.take().unwrap();
    let now = runtime.now_ms();
    runtime.kiosk_preparation_tick(&mut app, now + 59_999);
    assert!(runtime.transient.candidate.is_some());
    runtime.kiosk_preparation_tick(&mut app, now + 60_000);
    assert!(runtime.transient.candidate.is_none());
    assert!(!runtime.catalog_done.load(Ordering::Acquire));
    let timeout = runtime.kiosk_preparation_receipt_for_test().unwrap();
    assert!(timeout.same_attempt(&receipt));
    assert!(transient::kiosk_preparation_is_current(
        &runtime, &app, &timeout
    ));
    assert_eq!(
        transient::retry_kiosk_preparation(&mut runtime, &mut app, &timeout),
        Err(RestoreStartError::Busy)
    );
    app.ott = Some(runtime);
}

#[test]
fn explicit_stop_cancels_receipt_instead_of_scheduling_retry() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let (_directory, mut app, root, fingerprint) = fixture();
    let receipt = start(&mut app, &root, &fingerprint);
    first_row(&mut app);
    let mut runtime = app.ott.take().unwrap();
    transient::cancel_play_candidate(&mut runtime, &mut app);
    assert!(runtime.transient.candidate.is_none());
    assert!(runtime.kiosk_preparation_receipt_for_test().is_none());
    assert!(!transient::kiosk_preparation_is_current(
        &runtime, &app, &receipt
    ));
    app.ott = Some(runtime);
}

#[test]
fn stale_fingerprint_or_failed_inventory_cannot_begin_or_replace_existing_owner() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let (_directory, mut app, root, fingerprint) = fixture();
    let mut runtime = app.ott.take().unwrap();
    let generation = runtime.generation;
    assert_eq!(
        transient::kiosk_restore_local_folder(&mut runtime, &mut app, &root.id, "wrong"),
        Err(RestoreStartError::ContextChanged)
    );
    runtime.local_inventory.phase = transient::InventoryPhase::Failed;
    assert_eq!(
        transient::kiosk_restore_local_folder(&mut runtime, &mut app, &root.id, &fingerprint),
        Err(RestoreStartError::Unavailable)
    );
    assert_eq!(runtime.generation, generation);
    assert!(runtime.transient.candidate.is_none());
    assert!(runtime.kiosk_preparation_receipt_for_test().is_none());
    app.ott = Some(runtime);
}

#[test]
fn addressed_native_failure_after_actual_ticket_advance_keeps_retry_receipt() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let (_directory, mut app, root, fingerprint) = fixture();
    let original = start(&mut app, &root, &fingerprint);
    first_row(&mut app);
    let mut runtime = app.ott.take().unwrap();
    let before = plx_media::route::play_request_ticket();
    // Exercise the real route owner, not a fabricated ticket value. This is
    // the same ticket transition a synchronous native teardown can perform.
    plx_media::route::cancel_play(&mut app.player.session);
    let after = plx_media::route::play_request_ticket();
    assert_ne!(before, after);
    let native = NativeStartReceipt {
        generation: runtime.play_generation,
        before,
        after,
    };
    assert!(transient::finish_native(
        &mut runtime,
        &mut app,
        Err("native refused after teardown"),
        Some(&native)
    )
    .is_some());
    let failed = runtime.kiosk_preparation_receipt_for_test().unwrap();
    assert!(original.same_attempt(&failed));
    runtime.play_target = None;
    runtime.resolver_next = None;
    assert!(transient::kiosk_preparation_is_current(
        &runtime, &app, &failed
    ));
    assert_eq!(
        transient::retry_kiosk_preparation(&mut runtime, &mut app, &failed),
        Err(RestoreStartError::Busy)
    );
    app.ott = Some(runtime);
}

#[test]
fn wrong_native_generation_cannot_refresh_a_real_advanced_ticket() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let (_directory, mut app, root, fingerprint) = fixture();
    let original = start(&mut app, &root, &fingerprint);
    first_row(&mut app);
    let mut runtime = app.ott.take().unwrap();
    let before = plx_media::route::play_request_ticket();
    plx_media::route::cancel_play(&mut app.player.session);
    let after = plx_media::route::play_request_ticket();
    assert_ne!(before, after);
    let wrong = NativeStartReceipt {
        generation: runtime.play_generation.wrapping_add(1),
        before,
        after,
    };
    transient::finish_native(
        &mut runtime,
        &mut app,
        Err("unrelated native failure"),
        Some(&wrong),
    );
    assert!(runtime.kiosk_preparation_receipt_for_test().is_none());
    assert!(!transient::kiosk_preparation_is_current(
        &runtime, &app, &original
    ));
    app.ott = Some(runtime);
}

#[test]
fn native_accepted_local_receipt_requires_exact_installed_playback_and_preserves_saved_state() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let (_directory, mut app, root, fingerprint) = fixture();
    let original = start(&mut app, &root, &fingerprint);
    first_row(&mut app);
    let mut runtime = app.ott.take().unwrap();
    let saved = serde_json::to_value(&runtime.saved).unwrap();
    let request = runtime.play_target.as_ref().unwrap().clone();
    let (profile, entry) = transient::staged_native_binding(&runtime, &request)
        .unwrap()
        .unwrap();
    let source = plx_media::ff::ExternalSource::Local(
        std::path::Path::new(root.canonical_path.as_deref().unwrap()).join("new-first.mp4"),
    );
    let mut info = unsupported_info();
    info.video_codec = "h264".into();
    info.audio_codec = "aac".into();
    info.audio_channels = 2;
    info.audio_sample_rate = 48000;
    info.audio_bit_depth = 16;
    let before = plx_media::route::play_request_ticket();
    plx_media::route::request_external_source(
        &mut app.player.session,
        &info,
        source.clone(),
        "new-first",
        "Movies",
        plx_platform::devcaps::assumed_shared(),
    )
    .unwrap();
    let after = plx_media::route::play_request_ticket();
    assert_ne!(before, after);
    let identity = plx_media::route::playback_identity(&app.player.session);
    assert!(identity.is_some());
    runtime.playing = Some(Playing { channel_capture: None,
        archive: None,
        kiosk_selection: None,
        transient_owner: request.transient_owner,
        local_selection: request.local_selection,
        identity,
        profile,
        entry,
        selection_id: request.id,
        source: Some(source),
        advance_on_failure: false,
        requires_server_lease: false,
    });
    let native = NativeStartReceipt {
        generation: runtime.play_generation,
        before,
        after,
    };
    let response = transient::finish_native(&mut runtime, &mut app, Ok(()), Some(&native)).unwrap();
    assert_eq!(response["data"]["state"], "ready");
    assert!(runtime.transient.candidate.is_none());
    let accepted = runtime.kiosk_preparation_receipt_for_test().unwrap();
    assert!(original.same_attempt(&accepted));
    runtime.play_target = None;
    runtime.resolver_next = None;
    assert!(transient::kiosk_preparation_is_current(
        &runtime, &app, &accepted
    ));
    assert_eq!(
        transient::retry_kiosk_preparation(&mut runtime, &mut app, &accepted),
        Err(RestoreStartError::Busy)
    );
    assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), saved);
    runtime.playing.as_mut().unwrap().transient_owner = None;
    assert!(!transient::kiosk_preparation_is_current(
        &runtime, &app, &accepted
    ));
    app.ott = Some(runtime);
}


#[test]
fn accepted_navigation_rejects_a_late_first_local_prefix() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let (_directory, mut app, root, fingerprint) = fixture();
    let receipt = start(&mut app, &root, &fingerprint);
    let mut runtime = app.ott.take().unwrap();
    let saved = serde_json::to_value(&runtime.saved).unwrap();
    runtime.accepted_navigation(false);
    runtime.resolver_busy = true;
    assert!(!transient::kiosk_preparation_is_current(&runtime, &app, &receipt));
    let mut incoming = Catalog { hierarchy: None, entries: vec![item("late-first")] };
    assert!(transient::scan_result(&mut runtime, &mut app, &mut incoming, &mut None));
    assert!(runtime.transient.candidate.is_none());
    assert!(runtime.play_target.is_none());
    assert!(runtime.resolver_next.is_none());
    assert!(runtime.kiosk_preparation_receipt_for_test().is_none());
    assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), saved);
    app.ott = Some(runtime);
}

#[test]
fn accepted_navigation_prevents_local_preparation_resurrection_on_wake() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let (_directory, mut app, root, fingerprint) = fixture();
    let receipt = start(&mut app, &root, &fingerprint);
    let mut runtime = app.ott.take().unwrap();
    runtime.accepted_navigation(false);
    assert!(transient::kiosk_preparation_resume_policy(&runtime, &app, &receipt).is_none());
    app.ott_standby = true;
    runtime.suspend(&mut app);
    assert!(runtime.kiosk_preparation_receipt_for_test().is_none());
    assert!(runtime.transient.candidate.is_none());
    assert!(runtime.resume_pending.is_none());
    app.ott_standby = false;
    runtime.resume_source_work();
    let now = runtime.now_ms().saturating_add(60_000);
    runtime.kiosk_preparation_tick(&mut app, now);
    assert!(runtime.transient.candidate.is_none());
    assert!(runtime.play_target.is_none());
    app.ott = Some(runtime);
}
