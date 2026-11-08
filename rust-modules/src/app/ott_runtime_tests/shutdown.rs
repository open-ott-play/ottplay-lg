//! Post-loop cleanup must keep live and late source leases until their bounded
//! stop attempt has completed. Synthetic channels stand in for the server worker.
use super::*;
use plx_base::task::{time::Budget, OffFrame};

fn closing_session() -> (
    crate::app::ott_vportal_lease::Lease,
    Arc<AtomicBool>,
    std::thread::JoinHandle<()>,
) {
    let (lease, release, finished) = crate::app::ott_vportal_lease::Lease::pending_for_test();
    let closed = Arc::new(AtomicBool::new(false));
    let completed = closed.clone();
    let worker = std::thread::spawn(move || {
        release
            .recv_timeout(Duration::from_secs(2))
            .expect("shutdown must release the session");
        let off = OffFrame::for_test();
        Budget::start(&off, Duration::from_secs(1)).wait(
            &off,
            Duration::from_millis(40),
            Duration::from_millis(5),
            &|| false,
        );
        completed.store(true, Ordering::Release);
        // The old implementation drops the receiver before this completion.
        let _ = finished.send(());
    });
    (lease, closed, worker)
}

#[test]
fn shutdown_waits_for_an_active_session_stop_without_installing_media() {
    let _serial = plx_base::testlock::serial();
    let mut app = app_fixture();
    let (lease, closed, server) = closing_session();
    app.ott.as_mut().unwrap().active_lease = Some(lease.into());
    super::super::shutdown(&mut app);
    let waited = closed.load(Ordering::Acquire);
    server.join().unwrap();
    assert!(waited, "shutdown returned before the active stop completed");
    assert!(app.ott.is_none());
    assert!(app.bridge.metadata_view().now_playing().is_none());
}

#[test]
fn shutdown_drains_a_late_ready_and_waits_for_its_session_stop() {
    let _serial = plx_base::testlock::serial();
    let mut app = app_fixture();
    let (lease, closed, server) = closing_session();
    let runtime = app.ott.as_mut().unwrap();
    runtime.resolver_busy = true;
    let tx = runtime.tx.clone();
    let producer = std::thread::spawn(move || {
        let off = OffFrame::for_test();
        Budget::start(&off, Duration::from_secs(1)).wait(
            &off,
            Duration::from_millis(20),
            Duration::from_millis(5),
            &|| false,
        );
        // A successfully allocated session arrived during shutdown. Send failure
        // is expected in the old implementation and must also release the lease.
        let _ = tx.send(Job::Play(
            0,
            synthetic_profile(),
            Ok((
                synthetic_entry(),
                ReadySource::External(
                    plx_media::ff::ExternalSource::Http("https://media.invalid/video".into()),
                    unsupported_info(),
                    Some(lease),
                ),
            )),
        ));
    });
    super::super::shutdown(&mut app);
    let waited = closed.load(Ordering::Acquire);
    producer.join().unwrap();
    server.join().unwrap();
    assert!(
        waited,
        "shutdown returned before a late allocated session stopped"
    );
    assert!(
        app.bridge.metadata_view().now_playing().is_none(),
        "shutdown must never pass the late source to start_resolved"
    );
    assert!(!plx_media::route::has_url(&app.player.session));
}

#[test]
fn shutdown_does_not_extend_the_total_allowance_for_an_unfinished_stop() {
    let _serial = plx_base::testlock::serial();
    let mut app = app_fixture();
    let (lease, release, _finished) = crate::app::ott_vportal_lease::Lease::pending_for_test();
    app.ott.as_mut().unwrap().active_lease = Some(lease.into());
    let off = OffFrame::for_test();
    let measured = Budget::start(&off, Duration::from_secs(1));
    super::super::shutdown_with_budget(&mut app, Duration::from_millis(20));
    assert!(
        measured.spent(&off) < Duration::from_millis(500),
        "shutdown renewed the per-session allowance instead of the shared budget"
    );
    assert_eq!(release.try_recv(), Ok(()));
    assert!(app.ott.is_none());
}
