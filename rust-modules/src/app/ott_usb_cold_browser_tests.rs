//! Combined cold-owner/browser scheduling regressions. Reuse the existing real
//! Loaded and Restore fixtures; run only injected empty mount observations.
//! No launcher, worker thread, protected writer, decoder or device is invoked.
use super::*;

fn idle_cold_owner(retryable: bool) -> (Box<App>, Box<Runtime>) {
    // Keep App on the heap across the nested Loaded/Restore fixture calls.
    let mut app = Box::new(app());
    let mut r = load_into(&mut app, saved_intent());
    let task = take_restore(&mut r);
    let result = if retryable {
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
    } else {
        Err(usb::Error::Changed)
    };
    assert!(matches!(
        result,
        Err(usb::Error::Unavailable | usb::Error::Changed)
    ));
    r.usb_result(&mut app, r.generation, result);
    assert_eq!(r.local_inventory.usb.delayed_mount.is_some(), retryable);
    assert!(!r.local_inventory.usb.waiting);
    assert!(r.local_inventory.usb.pending.is_none());
    assert!(
        r.take_catalog_task().is_none(),
        "cold idle must not scan other roots"
    );
    (app, r)
}

fn queue_roots(r: &mut Runtime, id: u64) -> Invocation {
    let now = r.now_ms();
    let body = json!({"commands":[],"server_time":1000,"requests":[{
        "id":format!("{id:032x}"),"action":"local_browser",
        "params":{"version":1,"runtime":r.remote.runtime(),"op":"roots"},
        "expires_at":1020
    }]})
    .to_string();
    let mut requests = r.remote.raw_poll_for_test(&body, now);
    assert_eq!(requests.len(), 1);
    let invocation = requests.pop().unwrap();
    assert!(r.remote.invocation_is_current(&invocation));
    assert_eq!(invocation.deadline_ms, now.saturating_add(20_000));
    // Existing helper executes the production parser/admission, without its
    // launcher. The actual shared task selector is exercised below.
    r.usb_browser_prepare_for_test(&invocation).unwrap();
    invocation
}

fn finish_empty_roots(
    app: &mut App,
    r: &mut Runtime,
    task: ott_catalog::Task,
    invocation: &Invocation,
) {
    let ott_catalog::Task::UsbRemote {
        serial,
        task,
        cancel,
    } = task
    else {
        panic!("accepted browser request must own the next shared task");
    };
    assert!(r.usb_remote_waiting_result());
    let result = usb::execute_with_identity(
        &OffFrame::for_test(),
        task,
        &cancel,
        &mut |_, _| Ok(Vec::new()),
        &mut |_, rows, _| {
            assert!(rows.is_empty());
            Ok(Vec::new())
        },
    );
    r.usb_remote_result(app, serial, result);
    let response = r.remote.response_for_id_for_test(&invocation.id).unwrap();
    assert_eq!(response["status"], "ok");
    assert_eq!(response["data"]["op"], "roots");
    assert_eq!(response["data"]["entries"], json!([]));
    assert!(!r.usb_remote_waiting_result());
}

#[test]
fn accepted_browser_roots_precedes_idle_terminal_or_retrying_cold_owner() {
    let _lock = plx_base::testlock::serial();
    let _reset = NativeReset;
    for retryable in [false, true] {
        let (mut app, mut r) = idle_cold_owner(retryable);
        let before = serde_json::to_value(&r.saved).unwrap();
        let generation = r.generation;
        let cancel = r.cancel.clone();
        let native = plx_media::route::play_request_ticket();
        let invocation = queue_roots(&mut r, 0xc401);
        let task = r
            .take_catalog_task()
            .expect("browser cannot starve behind cold idle");
        assert!(matches!(&task, ott_catalog::Task::UsbRemote { .. }));
        finish_empty_roots(&mut app, &mut r, task, &invocation);
        assert_eq!(r.generation, generation);
        assert!(Arc::ptr_eq(&r.cancel, &cancel) && !cancel.load(Ordering::Acquire));
        assert!(plx_media::route::play_request_is_current(native));
        assert_eq!(serde_json::to_value(&r.saved).unwrap(), before);
        assert!(r.local_inventory.sources.is_empty());
        assert!(
            r.take_catalog_task().is_none(),
            "browsing does not release cold fallback guard"
        );
        assert_eq!(r.local_inventory.usb.delayed_mount.is_some(), retryable);
    }
}

#[test]
fn queued_and_running_browser_preserve_due_cold_retry_until_settlement() {
    let _lock = plx_base::testlock::serial();
    let _reset = NativeReset;
    let (mut app, mut r) = idle_cold_owner(true);
    let now = r.now_ms();
    let deadline = r
        .local_inventory
        .usb
        .delayed_mount
        .as_ref()
        .unwrap()
        .deadline_ms;
    let generation = r.generation;
    let cancel = r.cancel.clone();
    let native = plx_media::route::play_request_ticket();
    let invocation = queue_roots(&mut r, 0xc402);

    // Queued browser work is busy before waiting_result becomes true.
    assert!(!r.usb_remote_waiting_result());
    r.usb_delayed_mount_tick_at(&app, now.saturating_add(2_000));
    assert_eq!(r.generation, generation);
    assert!(r.local_inventory.usb.pending.is_none());
    assert_eq!(
        r.local_inventory
            .usb
            .delayed_mount
            .as_ref()
            .unwrap()
            .deadline_ms,
        deadline
    );
    assert!(Arc::ptr_eq(&r.cancel, &cancel) && !cancel.load(Ordering::Acquire));
    let task = r
        .take_catalog_task()
        .expect("queued browser still available");
    assert!(matches!(&task, ott_catalog::Task::UsbRemote { .. }));

    // Cover both an occupied worker and a returned-but-not-consumed completion.
    // Neither may consume Retry or replace the current Runtime cancel token.
    for slot_done in [false, true] {
        r.catalog_done.store(slot_done, Ordering::Release);
        assert!(r.usb_remote_waiting_result());
        r.usb_delayed_mount_tick_at(&app, now.saturating_add(5_000));
        assert_eq!(r.generation, generation);
        assert!(r.local_inventory.usb.pending.is_none());
        assert_eq!(
            r.local_inventory
                .usb
                .delayed_mount
                .as_ref()
                .unwrap()
                .deadline_ms,
            deadline
        );
        assert!(Arc::ptr_eq(&r.cancel, &cancel) && !cancel.load(Ordering::Acquire));
        assert!(plx_media::route::play_request_is_current(native));
        assert!(r.take_catalog_task().is_none());
    }
    finish_empty_roots(&mut app, &mut r, task, &invocation);
    r.usb_delayed_mount_tick_at(&app, now.saturating_add(5_001));
    assert_ne!(
        r.generation, generation,
        "settled browser releases the due retry"
    );
    assert_eq!(
        r.local_inventory
            .usb
            .delayed_mount
            .as_ref()
            .unwrap()
            .deadline_ms,
        deadline
    );
    assert!(matches!(
        r.take_catalog_task(),
        Some(ott_catalog::Task::Usb(usb::Task::Restore { .. }))
    ));
    assert!(r.local_inventory.sources.is_empty());
    assert!(plx_media::route::play_request_is_current(native));
}

#[path = "ott_usb_cold_select_tests.rs"]
mod passive_select_tests;
