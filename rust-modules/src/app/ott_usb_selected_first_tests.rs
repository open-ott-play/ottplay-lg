//! Cold-start scheduling through real Loaded, Restore and Saved-result paths.
//! Synthetic mount observations open real temporary directory handles. Existing
//! work slots stay held around tick; this fixture never launches a worker.
use super::*;

struct NativeReset;
impl Drop for NativeReset {
    fn drop(&mut self) {
        plx_media::player::SHARED.reset_session();
        plx_media::player::TX.reset();
        plx_media::route::reset_player_control_for_test(&plx_media::route::PlaybackSession::IDLE);
    }
}

fn saved_intent() -> Saved {
    let mut saved = runtime().saved;
    saved.favorites_selected = false;
    let selected = RememberedFolders::read(&saved.settings).unwrap().folders()[0].clone();
    let other = FolderDescriptor {
        id: "unrelated".into(),
        name: "Other offline folder".into(),
        path: "/offline-selected-first/unrelated-must-not-be-observed".into(),
        kind: crate::app::ott_local_sources::LocalSourceKind::Folder,
        usb: None,
    };
    RememberedFolders::default()
        .with_connected(other)
        .unwrap()
        .with_connected(selected)
        .unwrap()
        .put_into(&mut saved.settings)
        .unwrap();
    saved.validate().unwrap();
    saved
}

fn hold_slots(r: &mut Runtime) {
    r.catalog_done.store(false, Ordering::Release);
    r.metadata_done.store(false, Ordering::Release);
    r.resolver_busy = true;
}

fn loaded(saved: Saved) -> (Box<App>, Runtime) {
    // App embeds the runtime owner. Keep it off the libtest thread stack while
    // Loaded/Restore/Saved helpers retain their own runtime and receipt values.
    let mut app = Box::new(app());
    let runtime = load_into(&mut app, saved);
    (app, runtime)
}

// Keep nested fixture owners in place rather than copying App through every
// Loaded/Restore/Saved return tuple on the libtest thread stack.
fn load_into(app: &mut App, saved: Saved) -> Runtime {
    app.running = true;
    let mut r = Runtime::unloaded(crate::app::ott_companion::Supervisor::inert_for_test());
    r.remote = Remote::disabled_for_test("dev_usb_cold_fixture", "usb");
    hold_slots(&mut r);
    assert!(!r.loaded && r.local_inventory.sources.is_empty());
    assert!(matches!(
        r.local_inventory.phase,
        transient::InventoryPhase::Unloaded
    ));
    r.tx.send(Job::Loaded(Ok((saved, [9; 16])))).unwrap();
    app.ott = Some(r);
    let sample = plx_base::task::time::ClockSample::capture_main(app.adapters.player.mt()).unwrap();
    crate::app::ott_runtime::tick(app, sample);
    let r = app.ott.take().unwrap();
    assert!(r.loaded && r.source_work_allowed());
    assert!(!r.catalog_done.load(Ordering::Acquire));
    assert!(r.local_inventory.sources.is_empty());
    r
}

fn release_slots(r: &mut Runtime) {
    r.catalog_done.store(true, Ordering::Release);
    r.metadata_done.store(true, Ordering::Release);
    r.resolver_busy = false;
}

fn take_restore(r: &mut Runtime) -> usb::Task {
    release_slots(r);
    let Some(ott_catalog::Task::Usb(task @ usb::Task::Restore { .. })) = r.take_catalog_task()
    else {
        panic!("cold selected USB must precede all-root LocalBindings");
    };
    assert!(
        r.local_inventory.sources.is_empty(),
        "task selection grants no root"
    );
    task
}

fn restored(r: &Runtime, directory: &Directory, task: usb::Task) -> usb::Answer {
    let off = OffFrame::for_test();
    let mount = directory.mount(&off);
    usb::execute_with_identity(
        &off,
        task,
        &r.cancel,
        &mut |_, _| Ok(vec![mount.clone()]),
        &mut |_, rows, _| {
            Ok(rows
                .iter()
                .map(|row| {
                    identity::IdentityObservation::for_test(
                        row.clone(),
                        identity::StableIdentity::new("fixture-uuid", row.filesystem()).unwrap(),
                    )
                })
                .collect())
        },
    )
    .unwrap()
}

fn installed(directory: &Directory) -> (Box<App>, Runtime, LocalSource, u64) {
    let mut app = Box::new(app());
    let mut r = load_into(&mut app, saved_intent());
    let old_generation = r.generation;
    let before = r.saved.clone();
    let task = take_restore(&mut r);
    let target = r.local_inventory.usb.target.clone().unwrap();
    let usb::Answer::Restored { source, relocation } = restored(&r, directory, task) else {
        panic!("real held-root Restore receipt");
    };
    assert!(source.is_live() && relocation.is_current(&source));
    let expected = source.clone();
    r.local_inventory.usb.waiting = false;
    let mut saved = r
        .stage_usb_install_with_relocation(target, source, Some(relocation))
        .unwrap();
    saved.resume.extend(r.checkpoints.clone());
    r.usb_preserve_checkpoint(&mut saved);
    assert_eq!(
        serde_json::to_value(&r.saved).unwrap(),
        serde_json::to_value(&before).unwrap()
    );
    assert!(r.local_inventory.sources.is_empty());
    hold_slots(&mut r);
    arm_writer(&mut r);
    deliver_into(&mut app, r, Ok(saved));
    let r = app.ott.take().unwrap();
    assert!(matches!(
        r.local_inventory.phase,
        transient::InventoryPhase::Ready(_)
    ));
    assert_eq!(r.local_inventory.sources.len(), 1);
    assert!(r.local_inventory.sources[0].same_grant(&expected));
    assert!(expected.is_live());
    assert_eq!(
        r.saved.profiles.active_profile,
        before.profiles.active_profile
    );
    assert_eq!(r.saved.favorites, before.favorites);
    assert_eq!(r.saved.category, before.category);
    assert_eq!(
        r.saved.resume["local-usb-1"].entry_id,
        before.resume["local-usb-1"].entry_id
    );
    assert_eq!(
        r.saved.resume["local-usb-1"].position_ms,
        before.resume["local-usb-1"].position_ms
    );
    assert!(RememberedFolders::read(&r.saved.settings)
        .unwrap()
        .folders()
        .iter()
        .any(|row| row.id == "unrelated"));
    (app, r, expected, old_generation)
}

#[test]
fn first_loaded_selects_only_saved_usb_before_unrelated_root_observation() {
    let _lock = plx_base::testlock::serial();
    let _reset = NativeReset;
    let (_app, mut r) = loaded(saved_intent());
    let saved = serde_json::to_value(&r.saved).unwrap();
    let navigation = r.navigation_epoch;
    let playback = r.playback_epoch;
    let ticket = plx_media::route::play_request_ticket();
    let usb::Task::Restore { binding, source } = take_restore(&mut r) else {
        unreachable!()
    };
    assert_eq!(source.id, "root");
    assert_eq!(binding.uuid, "fixture-uuid");
    assert_eq!(binding.filesystem_relative_folder, "Movies/夏");
    assert_eq!(source.generation, r.generation);
    assert_eq!(r.navigation_epoch, navigation);
    assert_eq!(r.playback_epoch, playback);
    assert!(plx_media::route::play_request_is_current(ticket));
    assert_eq!(serde_json::to_value(&r.saved).unwrap(), saved);
    assert!(
        r.take_catalog_task().is_none(),
        "no all-root job beside the selected Restore"
    );
}

#[test]
fn initial_selection_waits_for_actual_catalog_slot_retirement() {
    let _lock = plx_base::testlock::serial();
    let _reset = NativeReset;
    let (_app, mut r) = loaded(saved_intent());
    let generation = r.generation;
    let cancel = r.cancel.clone();
    assert!(r.take_catalog_task().is_none());
    assert_eq!(r.generation, generation);
    assert!(Arc::ptr_eq(&r.cancel, &cancel) && !cancel.load(Ordering::Acquire));
    assert!(!r.catalog_done.load(Ordering::Acquire));
    let _ = take_restore(&mut r);
}

#[test]
fn verified_selected_install_schedules_held_scan_without_granting_other_roots() {
    let _lock = plx_base::testlock::serial();
    let _reset = NativeReset;
    let directory = Directory::new();
    let (_app, mut r, selected, _) = installed(&directory);
    release_slots(&mut r);
    let Some(ott_catalog::Task::LocalFolder { profile, source }) = r.take_catalog_task() else {
        panic!("selected held scan, not all-root validation, follows the Saved ACK");
    };
    assert!(source.same_grant(&selected));
    assert_eq!(profile.id, "local-usb-1");
    assert_eq!(
        profile.source,
        directory.0.join("Movies/夏").to_str().unwrap()
    );
    assert_eq!(r.local_inventory.sources.len(), 1);
    assert_eq!(r.local_inventory.sources[0].descriptor().id, "root");
}

#[test]
fn selected_failure_and_retry_expiry_never_fall_through_to_all_root_bindings() {
    let _lock = plx_base::testlock::serial();
    let _reset = NativeReset;
    for terminal in [false, true] {
        let (mut app, mut r) = loaded(saved_intent());
        let saved = serde_json::to_value(&r.saved).unwrap();
        let task = take_restore(&mut r);
        let answer = if terminal {
            Err(usb::Error::Changed)
        } else {
            usb::execute_with_identity(
                &OffFrame::for_test(),
                task,
                &r.cancel,
                &mut |_, _| Ok(Vec::new()),
                &mut |_, rows, _| {
                    assert!(rows.is_empty());
                    Ok(Vec::new())
                },
            )
        };
        assert!(matches!(
            answer,
            Err(usb::Error::Unavailable | usb::Error::Changed)
        ));
        let generation = r.generation;
        r.usb_result(&mut app, generation, answer);
        assert!(r.take_catalog_task().is_none());
        if !terminal {
            let now = r.now_ms();
            r.usb_delayed_mount_tick_at(&app, now.saturating_add(2_000));
            assert!(matches!(
                r.take_catalog_task(),
                Some(ott_catalog::Task::Usb(usb::Task::Restore { .. }))
            ));
            let generation = r.generation;
            r.usb_result(&mut app, generation, Err(usb::Error::Unavailable));
            r.usb_delayed_mount_tick_at(&app, now.saturating_add(120_000));
        }
        for _ in 0..3 {
            assert!(r.take_catalog_task().is_none());
        }
        assert!(r.local_inventory.sources.is_empty());
        assert_eq!(serde_json::to_value(&r.saved).unwrap(), saved);
    }
}

#[test]
fn late_all_root_result_cannot_replace_or_revoke_selected_playing_grant() {
    let _lock = plx_base::testlock::serial();
    let _reset = NativeReset;
    let directory = Directory::new();
    let (mut app, r, selected, old_generation) = installed(&directory);
    let descriptor = selected.descriptor().clone();
    app.ott = Some(r);
    crate::app::ott_runtime::tests::local_integration::install(
        &mut app,
        Some(&descriptor),
        "first",
    );
    let mut r = app.ott.take().unwrap();
    let identity = plx_media::route::playback_identity(&app.player.session);
    assert!(identity.is_some());
    let generation = r.generation;
    let cancel = r.cancel.clone();
    assert_ne!(generation, old_generation);
    transient::binding_result(
        &mut r,
        &mut app,
        old_generation,
        Ok(ott_catalog::LocalBindings {
            sources: vec![LocalSource::unavailable(descriptor)],
            lookup: None,
            matched: Vec::new(),
            operation_error: None,
        }),
    );
    assert_eq!(r.generation, generation);
    assert!(Arc::ptr_eq(&r.cancel, &cancel) && !cancel.load(Ordering::Acquire));
    assert_eq!(r.local_inventory.sources.len(), 1);
    assert!(r.local_inventory.sources[0].same_grant(&selected) && selected.is_live());
    assert_eq!(
        plx_media::route::playback_identity(&app.player.session),
        identity
    );
    assert!(r
        .playing
        .as_ref()
        .is_some_and(|playing| playing_matches(playing, &app)));
}

#[test]
fn changed_profile_cancel_or_native_intent_rejects_late_selected_receipt() {
    let _lock = plx_base::testlock::serial();
    let _reset = NativeReset;
    for change in 0..3 {
        let directory = Directory::new();
        let (mut app, mut r) = loaded(saved_intent());
        let task = take_restore(&mut r);
        let answer = restored(&r, &directory, task);
        let generation = r.generation;
        match change {
            0 => r.saved.profiles.profiles[0].source.push_str("/new-intent"),
            1 => r.cancel.store(true, Ordering::Release),
            _ => plx_media::route::cancel_play(&mut app.player.session),
        }
        let saved = serde_json::to_value(&r.saved).unwrap();
        let cancel = r.cancel.clone();
        r.usb_result(&mut app, generation, Ok(answer));
        assert!(r.local_inventory.sources.is_empty() && r.local_inventory.usb.install.is_none());
        assert!(!r.saving && Arc::ptr_eq(&r.cancel, &cancel));
        assert_eq!(serde_json::to_value(&r.saved).unwrap(), saved);
    }
}

#[test]
fn foreground_empty_inventory_is_not_an_initial_load_shortcut() {
    let _lock = plx_base::testlock::serial();
    let _reset = NativeReset;
    let (mut app, mut r) = loaded(saved_intent());
    r.suspend_source_work(&mut app);
    r.resume_source_work();
    release_slots(&mut r);
    r.rearm_source_work(&mut app);
    assert!(r.local_inventory.sources.is_empty());
    assert!(
        matches!(
            r.take_catalog_task(),
            Some(ott_catalog::Task::LocalBindings(_))
        ),
        "foreground keeps ordinary inventory validation"
    );
    assert!(r.local_inventory.usb.pending.is_none());
}

#[test]
fn subsequent_loaded_event_cannot_create_a_new_cold_opportunity() {
    let _lock = plx_base::testlock::serial();
    let _reset = NativeReset;
    let (mut app, mut r) = loaded(Saved::default());
    assert!(r.active().is_none());
    hold_slots(&mut r);
    r.tx.send(Job::Loaded(Ok((saved_intent(), [10; 16]))))
        .unwrap();
    app.ott = Some(r);
    let sample = plx_base::task::time::ClockSample::capture_main(app.adapters.player.mt()).unwrap();
    crate::app::ott_runtime::tick(&mut app, sample);
    let mut r = app.ott.take().unwrap();
    release_slots(&mut r);
    assert!(matches!(
        r.take_catalog_task(),
        Some(ott_catalog::Task::LocalBindings(_))
    ));
    assert!(r.local_inventory.usb.pending.is_none());
}

#[test]
fn saved_favorites_view_keeps_ordinary_all_root_admission() {
    let _lock = plx_base::testlock::serial();
    let _reset = NativeReset;
    let mut saved = saved_intent();
    saved.favorites_selected = true;
    let (_app, mut r) = loaded(saved);
    assert_eq!(r.page, "favorites");
    release_slots(&mut r);
    assert!(matches!(
        r.take_catalog_task(),
        Some(ott_catalog::Task::LocalBindings(_))
    ));
    assert!(r.local_inventory.usb.pending.is_none());
}

#[test]
fn newly_installed_live_grant_or_native_playback_prevents_cold_takeover() {
    let _lock = plx_base::testlock::serial();
    let _reset = NativeReset;
    for live_grant in [true, false] {
        let directory = Directory::new();
        let (mut app, mut r) = loaded(saved_intent());
        let source = if live_grant {
            let source = LocalSource::acquire_authorized(
                &OffFrame::for_test(),
                BoundSource {
                    id: "already-live".into(),
                    name: "Current source".into(),
                    kind: SourceKind::Folder,
                    available: true,
                    authority_ref: AuthorityRef(23),
                    generation: r.generation,
                    canonical_path: Some(directory.0.to_str().unwrap().into()),
                },
                &mut || true,
            )
            .unwrap();
            r.local_inventory
                .install(vec![source.clone()], r.generation);
            // Empty/Unloaded must not be approximated as only Unloaded.
            r.local_inventory.phase = transient::InventoryPhase::Unloaded;
            Some(source)
        } else {
            app.ott = Some(r);
            crate::app::ott_runtime::tests::local_integration::install(
                &mut app,
                None,
                "live-native",
            );
            r = app.ott.take().unwrap();
            assert!(plx_media::route::playback_identity(&app.player.session).is_some());
            None
        };
        release_slots(&mut r);
        assert!(matches!(
            r.take_catalog_task(),
            Some(ott_catalog::Task::LocalBindings(_))
        ));
        assert!(r.local_inventory.usb.pending.is_none());
        if let Some(source) = source {
            assert!(source.is_live() && r.local_inventory.sources[0].same_grant(&source));
        } else {
            assert!(r
                .playing
                .as_ref()
                .is_some_and(|playing| playing_matches(playing, &app)));
        }
    }
}

#[test]
fn failed_cold_saved_ack_keeps_the_selected_attempt_terminal_without_all_root_fallback() {
    let _lock = plx_base::testlock::serial();
    let _reset = NativeReset;
    let directory = Directory::new();
    let (mut app, mut r) = loaded(saved_intent());
    let before = serde_json::to_value(&r.saved).unwrap();
    let checkpoints = r.checkpoints.clone();
    let task = take_restore(&mut r);
    let target = r.local_inventory.usb.target.clone().unwrap();
    let usb::Answer::Restored { source, relocation } = restored(&r, &directory, task) else {
        panic!("actual Restore receipt");
    };
    assert!(source.is_live() && relocation.is_current(&source));
    r.local_inventory.usb.waiting = false;
    let _candidate = r
        .stage_usb_install_with_relocation(target, source, Some(relocation))
        .unwrap();
    assert!(r.local_inventory.usb.install.is_some());
    hold_slots(&mut r);
    arm_writer(&mut r);
    deliver_into(&mut app, r, Err("Synthetic current protected writer refusal"));
    let mut r = app.ott.take().unwrap();
    assert!(r.local_inventory.usb.install.is_none());
    assert!(r.local_inventory.sources.is_empty());
    assert!(matches!(
        r.local_inventory.phase,
        transient::InventoryPhase::Unloaded
    ));
    assert_eq!(serde_json::to_value(&r.saved).unwrap(), before);
    assert!(r.checkpoints == checkpoints);
    release_slots(&mut r);
    for delay in [0, 2_000, 120_000] {
        r.usb_delayed_mount_tick_at(&app, r.now_ms().saturating_add(delay));
        assert!(
            r.take_catalog_task().is_none(),
            "writer failure cannot launch unrelated all-root validation"
        );
        assert!(r.local_inventory.usb.pending.is_none());
        assert!(r.local_inventory.usb.delayed_mount.is_none());
    }
}

fn change_cold_context(r: &mut Runtime, directory: &Directory, change: u8) -> Option<LocalSource> {
    match change {
        0 => r.saved.favorites_selected = true,
        1 => {
            let remembered = RememberedFolders::read(&r.saved.settings).unwrap();
            let mut row = remembered
                .folders()
                .iter()
                .find(|row| row.id == "root")
                .unwrap()
                .clone();
            row.name = "New remembered intent".into();
            remembered
                .with_reselected(row)
                .unwrap()
                .put_into(&mut r.saved.settings)
                .unwrap();
        }
        2 => {
            let source = LocalSource::acquire_authorized(
                &OffFrame::for_test(),
                BoundSource {
                    id: "new-live-owner".into(),
                    name: "Current live source".into(),
                    kind: SourceKind::Folder,
                    available: true,
                    authority_ref: AuthorityRef(29),
                    generation: r.generation,
                    canonical_path: Some(directory.0.to_str().unwrap().into()),
                },
                &mut || true,
            )
            .unwrap();
            r.local_inventory
                .install(vec![source.clone()], r.generation);
            return Some(source);
        }
        3 => r.saved.profiles.profiles[0]
            .source
            .push_str("/new-profile-intent"),
        _ => unreachable!(),
    }
    None
}

#[test]
fn queued_cold_retry_retires_on_same_generation_context_change_without_cancelling_current_work() {
    let _lock = plx_base::testlock::serial();
    let _reset = NativeReset;
    for before_due_tick in [false, true] {
        for change in 0..4 {
            let directory = Directory::new();
            let (mut app, mut r) = loaded(saved_intent());
            let task = take_restore(&mut r);
            let missing = usb::execute_with_identity(
                &OffFrame::for_test(),
                task,
                &r.cancel,
                &mut |_, _| Ok(Vec::new()),
                &mut |_, rows, _| {
                    assert!(rows.is_empty());
                    Ok(Vec::new())
                },
            );
            assert!(matches!(missing, Err(usb::Error::Unavailable)));
            let initial_generation = r.generation;
            r.usb_result(&mut app, initial_generation, missing);
            if !before_due_tick {
                r.usb_delayed_mount_tick_at(&app, r.now_ms().saturating_add(2_000));
                assert_ne!(r.generation, initial_generation);
                assert!(matches!(
                    &r.local_inventory.usb.pending,
                    Some(usb::Task::Restore { .. })
                ));
                assert!(
                    r.local_inventory.usb.waiting && r.local_inventory.usb.delayed_mount.is_some()
                );
            }
            let live = change_cold_context(&mut r, &directory, change);
            let saved = serde_json::to_value(&r.saved).unwrap();
            let generation = r.generation;
            let cancel = r.cancel.clone();
            let ticket = plx_media::route::play_request_ticket();
            if before_due_tick {
                // tick runs before the selector in production; it must reject the
                // changed authority before minting a retry generation/token.
                r.usb_delayed_mount_tick_at(&app, r.now_ms().saturating_add(2_000));
                assert!(r.local_inventory.usb.pending.is_none());
                assert!(r.local_inventory.usb.delayed_mount.is_none());
                assert_eq!(r.generation, generation);
                assert!(Arc::ptr_eq(&r.cancel, &cancel) && !cancel.load(Ordering::Acquire));
            }
            // Ordinary validation may take over, but the old queued Restore may not.
            assert!(!matches!(
                r.take_catalog_task(),
                Some(ott_catalog::Task::Usb(_))
            ));
            assert!(r.local_inventory.usb.pending.is_none());
            assert!(!r.local_inventory.usb.waiting);
            assert!(r.local_inventory.usb.delayed_mount.is_none());
            assert_eq!(r.generation, generation);
            assert!(Arc::ptr_eq(&r.cancel, &cancel) && !cancel.load(Ordering::Acquire));
            assert!(plx_media::route::play_request_is_current(ticket));
            assert_eq!(serde_json::to_value(&r.saved).unwrap(), saved);
            if let Some(live) = live {
                assert!(live.is_live());
                assert_eq!(r.local_inventory.sources.len(), 1);
                assert!(r.local_inventory.sources[0].same_grant(&live));
            }
        }
    }
}

#[test]
fn cold_result_and_saved_admission_recheck_context_before_another_selector_tick() {
    let _lock = plx_base::testlock::serial();
    let _reset = NativeReset;
    for at_saved_gate in [false, true] {
        for change in [0, 2] {
            let directory = Directory::new();
            let replacement = Directory::new();
            let (mut app, mut r) = loaded(saved_intent());
            let task = take_restore(&mut r);
            let target = r.local_inventory.usb.target.clone().unwrap();
            let mut answer = Some(restored(&r, &directory, task));
            let candidate = if at_saved_gate {
                let usb::Answer::Restored { source, relocation } = answer.take().unwrap() else {
                    panic!("actual Restore receipt");
                };
                assert!(relocation.is_current(&source));
                r.local_inventory.usb.waiting = false;
                Some(
                    r.stage_usb_install_with_relocation(target, source, Some(relocation))
                        .unwrap(),
                )
            } else {
                None
            };
            if at_saved_gate {
                arm_writer(&mut r);
            }
            if let Some(candidate) = &candidate {
                assert!(
                    r.usb_save_ready(candidate),
                    "the staged current receipt starts eligible"
                );
            }
            let original_saved = r.saved.clone();
            let live = change_cold_context(&mut r, &replacement, change);
            let mut saved = serde_json::to_value(&r.saved).unwrap();
            let generation = r.generation;
            let cancel = r.cancel.clone();
            let ticket = plx_media::route::play_request_ticket();
            if let Some(candidate) = &candidate {
                // A write may already be durable. This asserts only source
                // admission refusal, not rollback of that Saved transaction.
                assert!(!r.usb_save_ready(candidate));
                // Use the production retirement hook here: running the whole
                // selector would also enqueue ordinary bindings and change
                // Unloaded to Loading, obscuring the ABA eligibility premise.
                // The queued-retry test above covers actual selector dispatch.
                r.retire_usb_work();
                assert!(
                    r.local_inventory.usb.install.is_some(),
                    "keep the addressed install until ACK cleanup"
                );
                r.saved = original_saved;
                saved = serde_json::to_value(&r.saved).unwrap();
                if change == 0 {
                    assert!(r.local_inventory.sources.is_empty());
                    assert!(matches!(
                        r.local_inventory.phase,
                        transient::InventoryPhase::Unloaded
                    ));
                }
                assert!(
                    !r.usb_save_ready(candidate),
                    "restoring old preferences cannot resurrect a retired install"
                );
            } else {
                r.usb_result(&mut app, generation, Ok(answer.take().unwrap()));
                assert!(r.local_inventory.usb.install.is_none());
                assert!(!r.saving);
            }
            assert_eq!(r.generation, generation);
            assert!(Arc::ptr_eq(&r.cancel, &cancel) && !cancel.load(Ordering::Acquire));
            assert!(plx_media::route::play_request_is_current(ticket));
            assert_eq!(serde_json::to_value(&r.saved).unwrap(), saved);
            if let Some(live) = &live {
                assert!(live.is_live());
                assert_eq!(r.local_inventory.sources.len(), 1);
                assert!(r.local_inventory.sources[0].same_grant(live));
            } else {
                assert!(r.local_inventory.sources.is_empty());
            }
            if let Some(candidate) = candidate {
                let durable = serde_json::to_value(&candidate).unwrap();
                hold_slots(&mut r);
                deliver_into(&mut app, r, Ok(candidate));
                let r = app.ott.as_ref().unwrap();
                assert_eq!(
                    serde_json::to_value(&r.saved).unwrap(),
                    durable,
                    "do not roll back a durable Saved ACK"
                );
                assert!(r.local_inventory.usb.install.is_none());
                assert!(!r
                    .local_inventory
                    .sources
                    .iter()
                    .any(|source| source.descriptor().id == "root"));
                assert!(
                    r.catalog_next.is_none() && !r.catalog_loading,
                    "a retired USB install cannot reload its source"
                );
            }
        }
    }
}

#[path = "ott_usb_cold_browser_tests.rs"]
mod browser_seam_tests;


fn portable_saved_intent(path: &str, binding: Option<UsbBinding>) -> Saved {
    let mut saved = saved_intent();
    let profile = &mut saved.profiles.profiles[0];
    profile.source = path.into();
    let fingerprint = profile_fingerprint(profile);
    saved.resume.get_mut("local-usb-1").unwrap().source_fingerprint = fingerprint;
    let old = RememberedFolders::read(&saved.settings).unwrap();
    let mut folders = RememberedFolders::default();
    for mut folder in old.folders().iter().cloned() {
        if folder.id == "root" {
            folder.path = path.into();
            folder.usb = binding.clone();
        }
        folders = folders.with_connected(folder).unwrap();
    }
    folders.put_into(&mut saved.settings).unwrap();
    saved.validate().unwrap();
    saved
}

#[test]
fn portable_binding_cold_start_restores_exact_relative_then_scans_only_selected_root() {
    let _lock = plx_base::testlock::serial();
    let _reset = NativeReset;
    let directory = Directory::new();
    // A receiver-private path has no Linux USB prefix and need not retain the
    // volume's old pathname. Only the saved filesystem-relative binding wins.
    let old_path = directory.0.join("old-placement").to_str().unwrap().to_owned();
    assert!(!usb::is_usb_path(&old_path));
    let binding = UsbBinding::from_observed("ext4", "fixture-uuid", "/", "Movies/夏").unwrap();
    // Keep the owner in place across both Loaded and Saved, as installed() does.
    let mut app = Box::new(app());
    let mut r = load_into(&mut app, portable_saved_intent(&old_path, Some(binding.clone())));
    let before = serde_json::to_value(&r.saved).unwrap();
    let task = take_restore(&mut r);
    let usb::Task::Restore { binding: actual, source } = &task else { unreachable!() };
    assert!(*actual == binding);
    assert_eq!(source.canonical_path.as_deref(), Some(old_path.as_str()));
    let target = r.local_inventory.usb.target.clone().unwrap();
    let usb::Answer::Restored { source, relocation } = restored(&r, &directory, task) else {
        panic!("verified private-root Restore");
    };
    assert!(relocation.is_current(&source) && source.usb_witness().is_some());
    assert_eq!(source.descriptor().canonical_path.as_deref(), directory.0.join("Movies/夏").to_str());
    let expected = source.clone();
    r.local_inventory.usb.waiting = false;
    let mut saved = r.stage_usb_install_with_relocation(target, source, Some(relocation)).unwrap();
    saved.resume.extend(r.checkpoints.clone());
    r.usb_preserve_checkpoint(&mut saved);
    assert_eq!(serde_json::to_value(&r.saved).unwrap(), before);
    assert!(r.local_inventory.sources.is_empty());
    hold_slots(&mut r);
    arm_writer(&mut r);
    deliver_into(&mut app, r, Ok(saved));
    let mut r = app.ott.take().unwrap();
    release_slots(&mut r);
    let Some(ott_catalog::Task::LocalFolder { source, .. }) = r.take_catalog_task() else {
        panic!("selected verified root must precede all-root LocalBindings");
    };
    assert!(source.same_grant(&expected));
    assert_eq!(r.local_inventory.sources.len(), 1);
    assert_eq!(r.local_inventory.sources[0].descriptor().id, "root");
}

#[test]
fn portable_binding_wrong_or_missing_volume_cannot_become_plain_path_work() {
    let _lock = plx_base::testlock::serial();
    let _reset = NativeReset;
    let directory = Directory::new();
    let path = directory.0.join("Movies/夏").to_str().unwrap().to_owned();
    for wrong_identity in [false, true] {
        let binding = UsbBinding::from_observed("ext4", "fixture-uuid", "/", "Movies/夏").unwrap();
        let mut app = Box::new(app());
        let mut r = load_into(&mut app, portable_saved_intent(&path, Some(binding)));
        let before = serde_json::to_value(&r.saved).unwrap();
        let task = take_restore(&mut r);
        let off = OffFrame::for_test();
        let mount = directory.mount(&off);
        let answer = usb::execute_with_identity(&off, task, &r.cancel,
            &mut |_, _| Ok(if wrong_identity { vec![mount.clone()] } else { vec![] }),
            &mut |_, rows, _| Ok(rows.iter().map(|row|
                identity::IdentityObservation::for_test(row.clone(),
                    identity::StableIdentity::new("different-uuid", row.filesystem()).unwrap()))
                .collect()));
        assert!(matches!(answer, Err(usb::Error::Unavailable | usb::Error::Changed)));
        let generation = r.generation;
        r.usb_result(&mut app, generation, answer);
        assert!(r.take_catalog_task().is_none(), "never fall through to pathname acquisition");
        assert!(r.local_inventory.sources.is_empty());
        assert_eq!(serde_json::to_value(&r.saved).unwrap(), before);
    }
}

#[test]
fn portable_binding_absent_is_ordinary_local_but_legacy_usb_without_binding_is_denied() {
    let _lock = plx_base::testlock::serial();
    let mut r = runtime();
    r.saved = portable_saved_intent("/private/local/videos", None);
    let profile = r.active().unwrap().clone();
    r.catalog_next = Some(profile.clone());
    assert!(r.usb_catalog_allowed());
    assert!(r.usb_restore_target(&profile).is_err(), "plain Local is not a USB grant");
    r.saved = portable_saved_intent("/tmp/usb/disk/part/videos", None);
    r.catalog_next = Some(r.active().unwrap().clone());
    assert!(!r.usb_catalog_allowed());
    assert!(r.catalog_next.is_none() && r.local_inventory.usb.pending.is_none());
}

#[test]
fn portable_binding_invalid_or_ambiguous_metadata_never_falls_through() {
    let _lock = plx_base::testlock::serial();
    let binding = UsbBinding::from_observed("ext4", "fixture-uuid", "/", "Movies/夏").unwrap();
    for invalid in 0..4 {
        let mut r = runtime();
        r.saved = portable_saved_intent("/private/local/videos", Some(binding.clone()));
        let value = r.saved.settings.get_mut("nativeLocalSources").unwrap();
        let rows = value["folders"].as_array_mut().unwrap();
        let selected = rows.iter().position(|row| row["id"] == "root").unwrap();
        match invalid {
            0 => rows[selected]["name"] = json!("bad\nname"),
            1 => rows[selected]["usb"] = serde_json::Value::Null,
            2 => { let duplicate = rows[selected].clone(); rows.push(duplicate); }
            _ => rows[selected]["usb"]["filesystem_relative_folder"] = json!("../outside"),
        }
        assert!(RememberedFolders::read(&r.saved.settings).is_err());
        r.catalog_next = Some(r.active().unwrap().clone());
        assert!(!r.usb_catalog_allowed());
        assert!(r.catalog_next.is_none() && r.local_inventory.usb.pending.is_none());
        assert!(r.local_inventory.sources.is_empty());
    }
}

#[test]
fn portable_binding_lost_witness_is_revoked_before_path_reconnect() {
    let _lock = plx_base::testlock::serial();
    let _reset = NativeReset;
    let directory = Directory::new();
    let path = directory.0.join("Movies/夏").to_str().unwrap().to_owned();
    let binding = UsbBinding::from_observed("ext4", "fixture-uuid", "/", "Movies/夏").unwrap();
    let mut app = Box::new(app());
    let mut r = load_into(&mut app, portable_saved_intent(&path, Some(binding)));
    drop(app);
    let task = take_restore(&mut r);
    let usb::Answer::Restored { source, .. } = restored(&r, &directory, task) else { unreachable!() };
    let mut observations = 0;
    let report = crate::app::ott_catalog::validate_local_bindings_observed_for_test(
        &OffFrame::for_test(), crate::app::ott_catalog::LocalBindingRequest {
            known: vec![source.clone()], remembered: vec![], lookup: Some(path),
            allow_new: true, allow_reconnect: true, restore_remembered: true,
            new_id: "must-not-replace".into(), generation: source.descriptor().generation + 1,
        }, &AtomicBool::new(false), &mut |_, _| { observations += 1; Ok(vec![]) }).unwrap();
    assert_eq!(observations, 1);
    assert_eq!(report.operation_error, Some(crate::app::ott_local_session::LocalError::SourceUnavailable));
    assert!(report.matched.is_empty());
    assert_eq!(report.sources.len(), 1);
    assert!(!report.sources[0].descriptor().available && report.sources[0].root_handle().is_none());
    // Installation must retain the refusal after dropping the original grant.
    // A later, separate validation has no remembered Connect descriptor.
    r.local_inventory.sources = vec![source.clone()];
    r.local_inventory.install(report.sources, source.descriptor().generation + 1);
    assert!(!source.is_live(), "the installed loss withdraws the previous live grant");
    let again = crate::app::ott_catalog::validate_local_bindings_observed_for_test(
        &OffFrame::for_test(), crate::app::ott_catalog::LocalBindingRequest {
            known: r.local_inventory.sources.clone(), remembered: vec![],
            lookup: source.descriptor().canonical_path.clone(), allow_new: true,
            allow_reconnect: true, restore_remembered: true,
            new_id: "still-must-not-replace".into(), generation: source.descriptor().generation + 2,
        }, &AtomicBool::new(false), &mut |_, _| panic!("no live witness remains to observe")).unwrap();
    assert_eq!(again.operation_error, Some(crate::app::ott_local_session::LocalError::SourceUnavailable));
    assert!(again.matched.is_empty());
    assert_eq!(again.sources.len(), 1);
    assert!(!again.sources[0].is_live() && again.sources[0].root_handle().is_none());
}
