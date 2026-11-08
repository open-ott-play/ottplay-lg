//! Delayed mount regressions through the actual Restore worker and USB owner.
//! Synthetic mount/identity observations, real temporary directory descriptors.
//! No worker threads, player, private writer, network or device are started.
use super::*;

fn sampled_runtime(app: &App) -> Runtime {
    let mut runtime = runtime();
    runtime.clock_sample = Some(
        plx_base::task::time::ClockSample::capture_main(app.adapters.player.mt())
            .expect("real monotonic frame input"),
    );
    runtime
}

fn start() -> (App, Runtime) {
    // App first: its native initialization precedes the captured USB ticket.
    let app = app();
    let mut runtime = sampled_runtime(&app);
    assert!(playback_allowed(&app));
    assert!(!runtime.usb_catalog_allowed());
    (app, runtime)
}

#[test]
fn missing_frame_clock_does_not_invent_a_retry_deadline() {
    let _lock = plx_base::testlock::serial();
    let mut app = app();
    let mut r = runtime();
    assert!(r.clock_sample.is_none());
    assert!(!r.usb_catalog_allowed());
    missing(&mut app, &mut r);
    assert!(r.local_inventory.usb.delayed_mount.is_none());
    assert!(r.local_inventory.usb.pending.is_none());
}

fn missing(app: &mut App, runtime: &mut Runtime) {
    let Some(ott_catalog::Task::Usb(task)) = runtime.take_usb_task() else {
        panic!("real queued Restore task")
    };
    let answer = usb::execute_with_identity(
        &OffFrame::for_test(),
        task,
        &runtime.cancel,
        &mut |_, _| Ok(Vec::new()),
        &mut |_, rows, _| {
            assert!(rows.is_empty());
            Ok(Vec::new())
        },
    );
    assert!(
        matches!(answer, Err(usb::Error::Unavailable)),
        "an absent saved UUID is Unavailable, not Changed or success"
    );
    runtime.usb_result(app, runtime.generation, answer);
}

fn release_slots(runtime: &mut Runtime) {
    // The earlier synchronous fixture job has actually returned before release.
    runtime.catalog_done.store(true, Ordering::Release);
    runtime.metadata_done.store(true, Ordering::Release);
    runtime.resolver_busy = false;
}

#[test]
fn absent_then_same_uuid_retries_and_reaches_real_relocation_without_new_input() {
    let _lock = plx_base::testlock::serial();
    let disk = Directory::new();
    let (mut app, mut runtime) = start();
    let before = serde_json::to_value(&runtime.saved).unwrap();
    let profile = runtime.active().unwrap().clone();
    let original_generation = runtime.generation;
    missing(&mut app, &mut runtime);
    release_slots(&mut runtime);
    runtime.usb_delayed_mount_tick_at(&app, runtime.now_ms().saturating_add(1_999));
    assert!(runtime.local_inventory.usb.pending.is_none());
    runtime.usb_delayed_mount_tick_at(&app, runtime.now_ms().saturating_add(2_000));
    assert_ne!(runtime.generation, original_generation);
    assert_eq!(runtime.active(), Some(&profile));
    assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), before);
    assert!(!runtime.saving && runtime.local_inventory.sources.is_empty());
    let target = runtime.local_inventory.usb.target.clone().unwrap();
    let Some(ott_catalog::Task::Usb(task)) = runtime.take_usb_task() else {
        panic!("one retried Restore task")
    };
    let off = OffFrame::for_test();
    let mount = disk.mount(&off);
    let answer = usb::execute_with_identity(
        &off,
        task,
        &runtime.cancel,
        &mut |_, _| Ok(vec![mount.clone()]),
        &mut |_, rows, _| {
            Ok(rows
                .iter()
                .map(|m| {
                    identity::IdentityObservation::for_test(
                        m.clone(),
                        identity::StableIdentity::new("fixture-uuid", m.filesystem()).unwrap(),
                    )
                })
                .collect())
        },
    )
    .unwrap();
    let usb::Answer::Restored { source, relocation } = answer else {
        panic!("real receipt")
    };
    assert!(relocation.is_current(&source));
    assert_eq!(relocation.stable_binding().uuid, "fixture-uuid");
    assert_eq!(source.descriptor().name, "Videos");
    assert_eq!(
        source.descriptor().canonical_path.as_deref(),
        disk.0.join("Movies/夏").to_str()
    );
    runtime.local_inventory.usb.waiting = false;
    // Existing accepted-result staging, deliberately not a fabricated disk ACK.
    let candidate = runtime
        .stage_usb_install_with_relocation(target, source, Some(relocation))
        .unwrap();
    assert_eq!(
        candidate.profiles.active_profile,
        runtime.saved.profiles.active_profile
    );
    assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), before);
    assert!(runtime.local_inventory.sources.is_empty() && !runtime.saving);
    runtime.usb_delayed_mount_tick_at(&app, runtime.now_ms().saturating_add(20_000));
    assert!(
        runtime.local_inventory.usb.pending.is_none(),
        "staged install is not retried"
    );
}

#[test]
fn delayed_retry_never_releases_or_overlaps_an_occupied_shared_slot() {
    let _lock = plx_base::testlock::serial();
    for held in 0..3 {
        let (mut app, mut r) = start();
        missing(&mut app, &mut r);
        release_slots(&mut r);
        match held {
            0 => r.catalog_done.store(false, Ordering::Release),
            1 => r.metadata_done.store(false, Ordering::Release),
            _ => r.resolver_busy = true,
        }
        let generation = r.generation;
        for now in [2_000, 2_001, 15_000] {
            r.usb_delayed_mount_tick_at(&app, r.now_ms().saturating_add(now));
            assert!(r.local_inventory.usb.pending.is_none());
            assert_eq!(r.generation, generation);
        }
        release_slots(&mut r);
        r.usb_delayed_mount_tick_at(&app, r.now_ms().saturating_add(15_001));
        assert!(r.local_inventory.usb.pending.is_some());
        let issued = r.generation;
        for now in [15_002, 50_000, 119_999] {
            r.usb_delayed_mount_tick_at(&app, r.now_ms().saturating_add(now));
            assert_eq!(r.generation, issued, "one pending attempt; never catch up");
        }
    }
}

#[test]
fn old_failed_observation_cannot_retire_the_retried_source_owner() {
    let _lock = plx_base::testlock::serial();
    let (mut app, mut r) = start();
    let old_generation = r.generation;
    missing(&mut app, &mut r);
    release_slots(&mut r);
    r.usb_delayed_mount_tick_at(&app, r.now_ms().saturating_add(2_000));
    let generation = r.generation;
    let cancel = r.cancel.clone();
    assert_ne!(generation, old_generation);
    assert!(r.local_inventory.usb.pending.is_some());
    r.usb_result(&mut app, old_generation, Err(usb::Error::Changed));
    assert_eq!(r.generation, generation);
    assert!(Arc::ptr_eq(&r.cancel, &cancel));
    assert!(!cancel.load(Ordering::Acquire));
    assert!(r.local_inventory.usb.pending.is_some());
    assert!(r.local_inventory.usb.delayed_mount.is_some());
    assert!(r.local_inventory.sources.is_empty() && !r.saving);
}

#[test]
fn delayed_retry_is_retired_by_stop_native_intent_profile_binding_or_navigation() {
    let _lock = plx_base::testlock::serial();
    for change in 0..6 {
        let (mut app, mut r) = start();
        missing(&mut app, &mut r);
        release_slots(&mut r);
        match change {
            0 => r.stop_catalog(&mut app),
            1 => plx_media::route::cancel_play(&mut app.player.session),
            2 => r.saved.profiles.profiles[0].source.push_str("/different"),
            3 => {
                let folders = RememberedFolders::read(&r.saved.settings).unwrap();
                let mut folder = folders.folders()[0].clone();
                folder.usb.as_mut().unwrap().uuid = "other-volume".into();
                folders
                    .with_reselected(folder)
                    .unwrap()
                    .put_into(&mut r.saved.settings)
                    .unwrap();
            }
            4 => r.accepted_navigation(false),
            _ => r.playback_epoch = r.playback_epoch.wrapping_add(1),
        }
        let generation = r.generation;
        r.usb_delayed_mount_tick_at(&app, r.now_ms().saturating_add(2_000));
        r.usb_delayed_mount_tick_at(&app, r.now_ms().saturating_add(90_000));
        assert!(r.local_inventory.usb.pending.is_none());
        assert!(r.local_inventory.usb.delayed_mount.is_none());
        assert_eq!(r.generation, generation);
    }
}

#[test]
fn standby_retires_timer_and_foreground_keeps_its_existing_single_rearm() {
    let _lock = plx_base::testlock::serial();
    let (mut app, mut r) = start();
    missing(&mut app, &mut r);
    r.suspend_source_work(&mut app);
    r.usb_delayed_mount_tick_at(&app, r.now_ms().saturating_add(2_000));
    assert!(r.local_inventory.usb.delayed_mount.is_none());
    assert!(r.local_inventory.usb.pending.is_none());
    r.resume_source_work();
    r.rearm_source_work(&mut app);
    assert!(
        r.catalog_next.is_none(),
        "old worker still occupies its slot"
    );
    release_slots(&mut r);
    r.rearm_source_work(&mut app);
    assert_eq!(r.catalog_next.as_ref(), r.active());
    assert!(r.local_inventory.usb.delayed_mount.is_none());
}

#[test]
fn wrong_volume_is_never_adopted_and_ambiguous_or_terminal_errors_are_not_retried() {
    let _lock = plx_base::testlock::serial();
    let disk = Directory::new();
    let (mut app, mut r) = start();
    let Some(ott_catalog::Task::Usb(task)) = r.take_usb_task() else {
        panic!()
    };
    let off = OffFrame::for_test();
    let mount = disk.mount(&off);
    let answer = usb::execute_with_identity(
        &off,
        task,
        &r.cancel,
        &mut |_, _| Ok(vec![mount.clone()]),
        &mut |_, rows, _| {
            Ok(rows
                .iter()
                .map(|m| {
                    identity::IdentityObservation::for_test(
                        m.clone(),
                        identity::StableIdentity::new("wrong-volume", m.filesystem()).unwrap(),
                    )
                })
                .collect())
        },
    );
    assert!(matches!(answer, Err(usb::Error::Unavailable)));
    r.usb_result(&mut app, r.generation, answer);
    assert!(r.local_inventory.sources.is_empty() && !r.saving);
    for error in [
        usb::Error::Changed,
        usb::Error::Invalid,
        usb::Error::TooMany,
        usb::Error::MissingFolder,
        usb::Error::PermissionDenied,
        usb::Error::Cancelled,
    ] {
        let (mut app, mut r) = start();
        let _ = r.take_usb_task().unwrap();
        r.usb_result(&mut app, r.generation, Err(error));
        release_slots(&mut r);
        r.usb_delayed_mount_tick_at(&app, r.now_ms().saturating_add(20_000));
        assert!(r.local_inventory.usb.delayed_mount.is_none());
        assert!(r.local_inventory.sources.is_empty() && r.local_inventory.usb.pending.is_none());
        assert!(!r.saving);
    }
}

#[test]
fn missing_identity_never_acquires_ordinary_retry_policy() {
    let _lock = plx_base::testlock::serial();
    let app = app();
    let mut r = sampled_runtime(&app);
    let folders = RememberedFolders::read(&r.saved.settings).unwrap();
    let mut folder = folders.folders()[0].clone();
    folder.usb = None;
    assert_eq!(
        folders.with_reselected(folder.clone()).err(),
        Some("A stable USB binding cannot be erased by a legacy selection")
    );
    // Model an originally unbound legacy record, not removal of verified identity.
    RememberedFolders::default()
        .with_connected(folder)
        .unwrap()
        .put_into(&mut r.saved.settings)
        .unwrap();
    assert!(!r.usb_catalog_allowed());
    r.usb_delayed_mount_tick_at(&app, r.now_ms().saturating_add(2_000));
    assert!(r.local_inventory.usb.delayed_mount.is_none());
    assert!(r.local_inventory.usb.pending.is_none());
}

#[test]
fn kiosk_unavailable_stays_with_its_existing_owner_not_the_ordinary_timer() {
    use crate::app::ott_kiosk_stored::{self as stored, Stored};
    use plx_data::ott::favorites::Origin;
    let _lock = plx_base::testlock::serial();
    let mut app = app();
    let mut r = sampled_runtime(&app);
    let source = BoundSource {
        id: "root".into(),
        name: "Videos".into(),
        kind: SourceKind::Folder,
        available: false,
        authority_ref: AuthorityRef(1),
        generation: r.generation,
        canonical_path: Some("/tmp/usb/old/part/Movies/夏".into()),
    };
    let fingerprint = selection::local_root_fingerprint(&source).unwrap();
    let policy = Stored::local_folder(
        Origin {
            provider: "local".into(),
            profile: "Saved videos".into(),
            subprofile: None,
            source_alias: source.name.clone(),
        },
        true,
        source.id.clone(),
        fingerprint.clone(),
    )
    .unwrap();
    r.saved
        .settings
        .insert(stored::KEY.into(), policy.encode().unwrap());
    assert!(matches!(
        r.usb_prepare_kiosk_folder("root", &fingerprint),
        Ok(true)
    ));
    missing(&mut app, &mut r);
    release_slots(&mut r);
    r.usb_delayed_mount_tick_at(&app, r.now_ms().saturating_add(20_000));
    assert!(r.local_inventory.usb.delayed_mount.is_none());
    assert!(r.local_inventory.usb.pending.is_none());
    assert!(matches!(
        r.usb_prepare_kiosk_folder("root", &fingerprint),
        Err(RestoreStartError::Unavailable)
    ));
}

#[test]
fn ordinary_retry_expires_without_resetting_its_window_or_rewriting_saved() {
    let _lock = plx_base::testlock::serial();
    let (mut app, mut r) = start();
    missing(&mut app, &mut r);
    let before = serde_json::to_value(&r.saved).unwrap();
    release_slots(&mut r);
    // Model an already-debited original window without sleeping or fabricating
    // a ClockSample. Resetting State to a new 120s window must fail this premise.
    let original_deadline = r.now_ms().saturating_add(110_000);
    r.local_inventory
        .usb
        .delayed_mount
        .as_mut()
        .unwrap()
        .deadline_ms = original_deadline;
    r.usb_delayed_mount_tick_at(&app, original_deadline - 1);
    assert!(
        r.local_inventory.usb.pending.is_some(),
        "one late attempt, no catch-up series"
    );
    let deadline = r
        .local_inventory
        .usb
        .delayed_mount
        .as_ref()
        .unwrap()
        .deadline_ms;
    assert_eq!(deadline, original_deadline);
    let generation = r.generation;
    let _ = r.take_usb_task().unwrap();
    r.usb_result(&mut app, generation, Err(usb::Error::Unavailable));
    r.usb_delayed_mount_tick_at(&app, original_deadline);
    assert!(r.local_inventory.usb.delayed_mount.is_none());
    assert!(r.local_inventory.usb.pending.is_none());
    assert_eq!(r.generation, generation);
    assert_eq!(serde_json::to_value(&r.saved).unwrap(), before);
    assert!(!r.saving);
}
