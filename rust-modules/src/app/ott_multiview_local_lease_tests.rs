//! Successful but unadmitted resolutions still own provider cleanup. No decoder,
//! network operation or cleanup thread is started by these owner-boundary tests.
use super::*;
use multiview_sources::{Snapshot, SourceHandle};
use plx_data::ott::multiview::{Scene, SceneId};
use std::num::NonZeroU64;

fn receipt() -> (App, Runtime, Snapshot, SourceHandle, LocalResolved) {
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
