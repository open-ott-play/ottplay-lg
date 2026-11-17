//! Successful but unadmitted resolutions still own provider cleanup. No decoder,
//! network operation or cleanup thread is started by these owner-boundary tests.
use super::*;
use multiview_sources::{Snapshot, SourceHandle};
use plx_data::ott::multiview::{Scene, SceneId};
use std::num::NonZeroU64;

pub(super) fn receipt() -> (App, Runtime, Snapshot, SourceHandle, LocalResolved) {
    let mut app = crate::app::run::lifecycle_regression_tests::app();
    app.running = true;
    let mut runtime = Runtime::unloaded(crate::app::ott_companion::Supervisor::inert_for_test());
    runtime.remote = Remote::disabled_for_test("dev_local_lease", "fixture");
    runtime.loaded = true;
    runtime.state_load = StateLoad::Idle;
    runtime.entropy = Some([4; 16]);
    runtime.resume_source_work();
    runtime.saved.profiles.profiles.push(ProviderProfile {
        id: "lease-source".into(),
        name: "Source".into(),
        kind: ProviderKind::M3u,
        source: "https://fixture.invalid/list.m3u".into(),
        source_alias: "Source".into(),
        enabled: true,
    });
    runtime.saved.profiles.active_profile = Some("lease-source".into());
    runtime.provider = ProviderKind::M3u;
    runtime.catalog.entries.push(CatalogEntry {
        id: "channel".into(),
        title: "Channel".into(),
        category: String::new(),
        description: String::new(),
        poster: String::new(),
        synopsis: None,
        protection: Some(false),
        media: MediaLocator::Http {
            url: "https://fixture.invalid/live.m3u8".into(),
        },
        duration_ms: None,
        provider_id: None,
        provider_id_field: None,
        live: None,
    });
    let snapshot = Snapshot::capture_local(&runtime).unwrap();
    let handle = snapshot.rows().next().unwrap().handle;
    let mut scene = Scene::new(SceneId::from_opaque(NonZeroU64::new(701).unwrap()));
    scene.set_layout(Layout::Grid4).unwrap();
    scene
        .bind_source(SlotId::SLOT_0, handle.source_ref())
        .unwrap();
    let ticket = scene
        .begin_resolution(SlotId::SLOT_0, handle.source_ref(), handle.media_ref())
        .unwrap();
    let worker = plx_base::task::OffFrame::for_test();
    let resolved = snapshot
        .capture_local_work(&mut runtime, &app, &handle, ticket)
        .unwrap()
        .prepare_local_metadata(&worker)
        .unwrap()
        .authorize_completion(&mut runtime, &app)
        .unwrap()
        .resolve_local(&worker)
        .unwrap();
    (app, runtime, snapshot, handle, resolved)
}

#[test]
fn rejected_successful_resolves_retain_each_lease_without_opening_a_decoder() {
    let _serial = plx_base::testlock::serial();
    for case in ["unmatched", "cancelled", "reauthorization", "open_failure"] {
        let (app, mut runtime, _snapshot, _, mut resolved) = receipt();
        let ticket = resolved.input.ticket;
        let (lease, release, _completion) =
            crate::app::ott_vportal_lease::Lease::pending_for_test();
        resolved.lease = Some(lease);
        let mut driver = Driver::default();
        let mut opened = false;
        match case {
            "unmatched" => driver.receive_job(&mut runtime, &app, Job::Media(ticket, Ok(resolved))),
            "cancelled" => {
                let cancel = resolved.input.cancellation();
                cancel.store(true, Ordering::Release);
                driver.workers[0] = Some(Work {
                    ticket,
                    cancel,
                    done: Arc::new(AtomicBool::new(true)),
                    terminal_consumed: false,
                });
                driver.receive_job(&mut runtime, &app, Job::Media(ticket, Ok(resolved)));
                driver.retire_worker(0);
            }
            "reauthorization" => {
                runtime.multiview_local_profile_epoch += 1;
                driver.media_ready_with(&mut runtime, &app, ticket, Ok(resolved), |_| {
                    panic!("stale authority cannot open a decoder")
                });
            }
            _ => driver.media_ready_with(&mut runtime, &app, ticket, Ok(resolved), |_| {
                opened = true;
                Err(plx_media::local_multiview::Error::Unavailable)
            }),
        }
        assert_eq!(opened, case == "open_failure");
        assert!(
            release.try_recv().is_ok(),
            "{case}: cleanup must be requested promptly"
        );
        assert_eq!(
            driver.leases[0].len(),
            1,
            "{case}: retain the cleanup receipt"
        );
        assert!(!driver.slot_drained(0) && driver.has_work());
        assert!(driver.pictures.iter().all(Option::is_none));
    }
}

#[test]
fn completion_after_rx_drain_cannot_release_slot_before_its_lease_receipt_is_consumed() {
    let _serial = plx_base::testlock::serial();
    let (app, mut runtime, _snapshot, handle, mut resolved) = receipt();
    let ticket = resolved.input.ticket;
    let (lease, release, _completion) = crate::app::ott_vportal_lease::Lease::pending_for_test();
    resolved.lease = Some(lease);
    let cancel = resolved.input.cancellation();
    cancel.store(true, Ordering::Release);
    let done = Arc::new(AtomicBool::new(false));
    let mut driver = Driver::default();
    driver.workers[0] = Some(Work {
        ticket,
        cancel,
        done: done.clone(),
        terminal_consumed: false,
    });
    driver.stopping[0] = Some(ticket);
    driver.replacements[0] = Some(handle);
    assert!(matches!(
        driver.rx.try_recv(),
        Err(mpsc::TryRecvError::Empty)
    ));
    // The producer wins just after this frame's RX drain.
    driver
        .tx
        .send(Job::Media(ticket, Ok(resolved)))
        .unwrap_or_else(|_| panic!("fixture receiver"));
    done.store(true, Ordering::Release);
    driver.retire_worker(0);
    assert!(driver.workers[0].is_some() && !driver.slot_drained(0));
    assert_eq!(driver.stopping[0], Some(ticket));
    let job = driver.rx.try_recv().unwrap();
    driver.receive_job(&mut runtime, &app, job);
    driver.retire_worker(0);
    assert!(driver.workers[0].is_none());
    assert!(release.try_recv().is_ok());
    assert!(
        !driver.slot_drained(0),
        "consumed successful receipt still owns its lease barrier"
    );
    assert_eq!(driver.replacements[0], Some(handle));
}

#[test]
fn multiple_late_receipts_preserve_fifo_and_failed_close_fences_replacement_and_restore() {
    let _serial = plx_base::testlock::serial();
    let mut driver = Driver::default();
    let (app, mut runtime, _snapshot, handle, mut resolved) = receipt();
    let ticket = resolved.input.ticket;
    let (lease, _release, _completion) = crate::app::ott_vportal_lease::Lease::pending_for_test();
    resolved.lease = Some(lease);
    driver.reject_job(Job::Media(ticket, Ok(resolved)));
    let (_app2, _runtime2, _snapshot2, _, mut second) = receipt();
    let (lease2, _release2, _completion2) =
        crate::app::ott_vportal_lease::Lease::pending_for_test();
    let ticket2 = second.input.ticket;
    second.lease = Some(lease2);
    driver.reject_job(Job::Media(ticket2, Ok(second)));
    assert_eq!(driver.leases[0].len(), 2);
    driver.restore_previous = true;
    driver.replacements[0] = Some(handle);
    driver.leases[0]
        .front()
        .unwrap()
        .done
        .store(true, Ordering::Release);
    driver.finish_lease_if_ready(&mut runtime, 0);
    assert_eq!(
        driver.leases[0].len(),
        1,
        "a failed close cannot erase a later owned lease"
    );
    assert!(driver.cleanup_failed && !driver.restore_previous && driver.replacements[0].is_none());
    assert!(driver
        .play_source(&mut runtime, &app, SlotId::SLOT_0, handle)
        .is_err());
    assert!(!driver.slot_drained(0));
}

#[test]
fn rapid_channel_steps_before_first_tick_replace_queued_attempt_and_off_cancels_latest() {
    let _serial = plx_base::testlock::serial();
    let (app, mut runtime, _snapshot, _, _resolved) = receipt();
    let first = runtime.catalog.entries[0].clone();
    for number in [2, 3] {
        let mut entry = first.clone();
        entry.id = format!("channel-{number}");
        entry.title = format!("Channel {number}");
        entry.media = MediaLocator::Http {
            url: format!("https://fixture.invalid/{number}.m3u8"),
        };
        runtime.catalog.entries.push(entry);
    }
    let mut coordinator = std::mem::take(&mut runtime.multiview);
    let transition = coordinator.activate_local(&runtime, &app, Layout::Grid4).unwrap();
    runtime.multiview = coordinator;
    runtime.multiview_effects.extend(transition.effects);
    let handles = runtime.multiview.source_rows().iter().map(|row| row.handle).collect::<Vec<_>>();
    assert_eq!(handles.len(), 3);
    let native_ticket = plx_media::route::play_request_ticket();
    let mut driver = Driver::default();
    driver.layout = Some(Layout::Grid4);
    driver.play_source(&mut runtime, &app, SlotId::SLOT_0, handles[0]).unwrap();
    let (ticket, cancel) = runtime.multiview_effects.iter().find_map(|effect| {
        match effect {
            Effect::PrepareMetadata { ticket, work } => Some((*ticket, work.cancellation())),
            _ => None,
        }
    }).expect("first source queues its actual metadata preparation");
    assert!(!cancel.load(Ordering::Acquire));
    assert!(matches!(runtime.multiview.slot_status(SlotId::SLOT_0),
        Ok(plx_data::ott::multiview::SlotStatus::Resolving { .. })));

    // No Driver tick: the coordinator has admitted work, but no worker exists.
    driver.step_active(&mut runtime, &app, 1).unwrap();
    assert_eq!(driver.replacements[0], Some(handles[1]));
    driver.step_active(&mut runtime, &app, 1).unwrap();
    assert_eq!(driver.replacements[0], Some(handles[2]));
    driver.step_active(&mut runtime, &app, -1).unwrap();
    assert_eq!(driver.replacements[0], Some(handles[1]));
    assert!(cancel.load(Ordering::Acquire));
    assert!(matches!(runtime.multiview.slot_status(SlotId::SLOT_0),
        Ok(plx_data::ott::multiview::SlotStatus::Empty)));
    assert!(runtime.multiview_effects.iter().any(|effect| matches!(effect,
        Effect::CancelResolve { ticket: current, .. } if *current == ticket)));
    assert!(runtime.multiview_effects.iter().filter(|effect| matches!(effect,
        Effect::PrepareMetadata { .. })).all(|effect| !runtime.multiview.effect_current(&runtime, effect)));
    assert!(driver.workers.iter().all(Option::is_none));
    assert!(driver.pictures.iter().all(Option::is_none));
    assert!(driver.leases.iter().all(|leases| leases.is_empty()));
    assert!(driver.slot_drained(0));

    driver.close_restore_previous(&mut runtime, &app).unwrap();
    assert!(driver.closing);
    assert!(driver.replacements.iter().all(Option::is_none));
    assert!(cancel.load(Ordering::Acquire));
    assert!(runtime.multiview_effects.iter().filter(|effect| matches!(effect,
        Effect::PrepareMetadata { .. } | Effect::Backend(BackendIntent::Load { .. })))
        .all(|effect| !runtime.multiview.effect_current(&runtime, effect)));
    assert_eq!(plx_media::route::play_request_ticket(), native_ticket);
}

fn queued_local_attempt() -> (App, Runtime, Driver, Vec<SourceHandle>, ResolveTicket, multiview_sources::SourceWork) {
    let (app, mut runtime, _snapshot, _, _resolved) = receipt();
    let mut next = runtime.catalog.entries[0].clone();
    next.id = "second".into();
    next.title = "Second".into();
    next.media = MediaLocator::Http { url: "https://fixture.invalid/second.m3u8".into() };
    runtime.catalog.entries.push(next);
    let mut coordinator = std::mem::take(&mut runtime.multiview);
    let transition = coordinator.activate_local(&runtime, &app, Layout::Grid4).unwrap();
    runtime.multiview = coordinator;
    runtime.multiview_effects.extend(transition.effects);
    let handles = runtime.multiview.source_rows().iter().map(|row| row.handle).collect::<Vec<_>>();
    let mut driver = Driver::default();
    driver.layout = Some(Layout::Grid4);
    driver.play_source(&mut runtime, &app, SlotId::SLOT_0, handles[0]).unwrap();
    let index = runtime.multiview_effects.iter().position(|effect| matches!(effect,
        Effect::PrepareMetadata { .. })).unwrap();
    let Effect::PrepareMetadata { ticket, work } = runtime.multiview_effects.remove(index).unwrap()
        else { unreachable!() };
    (app, runtime, driver, handles, ticket, work)
}

fn resolve_attempt(runtime: &mut Runtime, app: &App, work: multiview_sources::SourceWork) {
    let metadata = work.prepare_local_metadata(&plx_base::task::OffFrame::for_test()).unwrap();
    let mut coordinator = std::mem::take(&mut runtime.multiview);
    let transition = coordinator.metadata_succeeded(metadata, runtime, app).unwrap();
    runtime.multiview = coordinator;
    runtime.multiview_effects.extend(transition.effects);
}

#[test]
fn failed_and_unsupported_slots_accept_replacement_without_losing_cleanup_custody() {
    let _serial = plx_base::testlock::serial();
    for failure in ["metadata", "resolve", "rejected", "unsupported"] {
        let (app, mut runtime, mut driver, handles, ticket, work) = queued_local_attempt();
        if failure == "metadata" || failure == "resolve" {
            let transition = if failure == "metadata" {
                runtime.multiview.metadata_failed(ticket, multiview_sources::Error::Resolve)
            } else {
                runtime.multiview.resolve_failed(ticket, multiview_sources::Error::Resolve)
            }.unwrap();
            runtime.multiview_effects.extend(transition.effects);
        } else {
            resolve_attempt(&mut runtime, &app, work);
            let transition = runtime.multiview.backend_start_completed(ticket,
                if failure == "unsupported" { AdmissionOutcome::Unsupported } else { AdmissionOutcome::Rejected }).unwrap();
            runtime.multiview_effects.extend(transition.effects);
        }
        let cleanup = if failure == "rejected" || failure == "unsupported" {
            let (lease, release, completion) = crate::app::ott_vportal_lease::Lease::pending_for_test();
            driver.retain_lease(0, lease);
            assert!(release.try_recv().is_ok());
            Some((release, completion))
        } else { None };
        // This exercises Driver replacement against each real coordinator
        // failure shape, including Failed with its resolution ticket consumed.
        driver.play_source(&mut runtime, &app, SlotId::SLOT_0, handles[1]).unwrap();
        if failure == "metadata" {
            assert!(matches!(runtime.multiview.slot_status(SlotId::SLOT_0),
                Ok(plx_data::ott::multiview::SlotStatus::Resolving { .. })));
            assert!(driver.replacements[0].is_none());
        } else {
            assert_eq!(driver.replacements[0], Some(handles[1]));
            assert!(matches!(runtime.multiview.slot_status(SlotId::SLOT_0),
                Ok(plx_data::ott::multiview::SlotStatus::Empty)));
        }
        assert!(driver.pictures.iter().all(Option::is_none));
        assert!(driver.workers.iter().all(Option::is_none));
        if cleanup.is_some() {
            assert_eq!(driver.leases[0].len(), 1);
            assert!(!driver.slot_drained(0), "{failure}: replacement must wait for provider close");
        }
        assert!(runtime.multiview.set_paused(ticket, true).is_err(), "{failure}: old failed ticket cannot mutate replacement");
    }
}

#[test]
fn pause_publication_is_ticket_bound_idempotent_and_visible_in_actual_scene_status() {
    let _serial = plx_base::testlock::serial();
    let (app, mut runtime, mut driver, handles, ticket, work) = queued_local_attempt();
    resolve_attempt(&mut runtime, &app, work);
    runtime.multiview.backend_start_completed(ticket, AdmissionOutcome::Admitted).unwrap();
    for paused in [true, false] {
        let transition = runtime.multiview.set_paused(ticket, paused).unwrap();
        let revision = transition.revision;
        assert_eq!(runtime.multiview.set_paused(ticket, paused).unwrap().revision, revision);
        let status = runtime.multiview.scene_status(&runtime).unwrap();
        assert_eq!(status.slots.iter().find(|slot| slot.slot == SlotId::SLOT_0).unwrap().status,
            if paused { "paused" } else { "playing" });
    }
    // A model admission alone cannot actuate transport or advertise seeking.
    assert!(driver.set_active_paused(&mut runtime, &app, true).is_err());
    assert!(!driver.active_seekable(&mut runtime, &app));
    assert!(matches!(runtime.multiview.slot_status(SlotId::SLOT_0),
        Ok(plx_data::ott::multiview::SlotStatus::Playing { .. })));
    driver.play_source(&mut runtime, &app, SlotId::SLOT_0, handles[1]).unwrap();
    assert!(runtime.multiview.set_paused(ticket, true).is_err(), "pending Stop fences transport publication");
    let stop = runtime.multiview_effects.iter().find_map(|effect| match effect {
        Effect::Backend(BackendIntent::Stop { ticket }) => Some(*ticket), _ => None,
    }).unwrap();
    runtime.multiview.stop_completed(stop, StopResult::Stopped).unwrap();
    assert!(runtime.multiview.set_paused(ticket, true).is_err(), "retired ticket cannot pause another attempt");
}
