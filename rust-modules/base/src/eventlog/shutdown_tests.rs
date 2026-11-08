use super::*;
use std::sync::{mpsc, Arc};
use std::time::Duration;

#[test]
fn preinit_failure_is_not_reported_as_complete_and_does_not_start_a_worker() {
    with_private_log(|| {
        let service = Service::new();
        let path = events_log().join("missing-parent");
        service.emit(path.clone(), "failed".into());
        let result = service.finish(path, Duration::from_millis(250));
        assert!(result.quiescent && !result.complete); assert_eq!(result.lost, 1);
        assert!(service.sink.get().is_none());
    });
}

#[test]
fn a_preinit_callback_must_finish_before_erasure_is_quiescent() {
    let service = Arc::new(Service::new());
    let producer = service.clone();
    let (entered, entry) = mpsc::channel(); let (release, gate) = mpsc::channel();
    let thread = std::thread::spawn(move || producer.emit_with("unused".into(), "held".into(), |_, _| {
        entered.send(()).unwrap(); gate.recv().unwrap(); Ok(())
    }));
    let seen = entry.recv_timeout(Duration::from_secs(2));
    let pending = service.finish("unused".into(), Duration::from_millis(20));
    let _ = release.send(()); thread.join().unwrap();
    assert!(seen.is_ok()); assert!(!pending.quiescent);
    let done = service.finish("unused".into(), Duration::ZERO);
    assert!(done.quiescent && done.complete);
    service.emit_with("unused".into(), "late".into(), |_, _| panic!("closed pre-init sink"));
    service.start(); assert!(service.sink.get().is_none(), "erasure cannot restart the sink");
}

#[test]
fn completed_sync_preamble_preserves_its_normal_exit_semantics() {
    with_private_log(|| {
        let service = Service::new(); let path = events_log();
        service.emit(path.clone(), "preamble".into());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "preamble\n");
        let done = service.finish(path, Duration::ZERO);
        assert!(done.quiescent && done.complete); assert_eq!(done.lost, 0);
    });
}

#[test]
fn only_complete_diagnostics_preserve_the_functional_exit_code() {
    for code in [0, 1, 2] {
        assert_eq!(CloseReport { quiescent: true, complete: true, lost: 0 }.exit_code(code), code);
        assert_eq!(CloseReport { quiescent: true, complete: false, lost: 1 }.exit_code(code), 70);
        assert_eq!(CloseReport::PENDING.exit_code(code), 70);
    }
}
