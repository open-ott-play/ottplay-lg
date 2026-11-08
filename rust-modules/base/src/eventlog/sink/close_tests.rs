use super::*;
use std::sync::mpsc;

fn path() -> PathBuf { PathBuf::from("unused-fixture") }
fn receive<T>(rx: &mpsc::Receiver<T>) -> T {
    rx.recv_timeout(Duration::from_secs(2)).expect("bounded receipt")
}
struct Release(Option<mpsc::Sender<()>>);
impl Release { fn now(&mut self) { if let Some(tx) = self.0.take() { let _ = tx.send(()); } } }
impl Drop for Release { fn drop(&mut self) { self.now(); } }

#[test]
fn close_fences_prior_records_and_writes_one_complete_marker() {
    let (tx, rx) = mpsc::channel();
    let sink = Sink::start(4, move |_, line| { tx.send(line.to_owned()).unwrap(); Ok(()) });
    sink.emit(path(), "first".into()); sink.emit(path(), "second".into());
    let result = sink.finish(path(), 0, Duration::from_millis(250));
    assert!(result.quiescent && result.complete); assert_eq!(result.lost, 0);
    assert_eq!(receive(&rx), "first"); assert_eq!(receive(&rx), "second");
    assert_eq!(receive(&rx), "eventlog: closed lost=0 complete=1");
    assert_eq!(sink.finish(path(), 0, Duration::ZERO), result);
    sink.emit(path(), "late".into());
    assert!(matches!(rx.try_recv(), Err(mpsc::TryRecvError::Empty)));
}

#[test]
fn a_blocked_close_times_out_and_the_same_receipt_can_complete_later() {
    let (tx, rx) = mpsc::channel(); let (release, gate) = mpsc::channel();
    let mut release = Release(Some(release));
    let sink = Sink::start(1, move |_, line| {
        tx.send(line.to_owned()).unwrap(); if line == "first" { gate.recv().unwrap(); } Ok(())
    });
    sink.emit(path(), "first".into()); assert_eq!(receive(&rx), "first");
    sink.emit(path(), "second".into()); // Fill every payload slot; Close still has its reserved slot.
    let started = Instant::now();
    let pending = sink.finish(path(), 0, Duration::from_millis(20));
    assert!(!pending.quiescent && !pending.complete);
    assert!(started.elapsed() < Duration::from_secs(2));
    release.now();
    let done = sink.finish(path(), 0, Duration::from_millis(250));
    assert!(done.quiescent && done.complete);
    assert_eq!(receive(&rx), "second");
    assert_eq!(receive(&rx), "eventlog: closed lost=0 complete=1");
    assert!(matches!(rx.try_recv(), Err(mpsc::TryRecvError::Empty)));
}

#[test]
fn lost_records_prevent_a_complete_marker_but_not_quiescence() {
    let (tx, rx) = mpsc::channel(); let (release, gate) = mpsc::channel();
    let mut release = Release(Some(release));
    let sink = Sink::start(1, move |_, line| {
        tx.send(line.to_owned()).unwrap(); if line == "first" { gate.recv().unwrap(); } Ok(())
    });
    sink.emit(path(), "first".into()); assert_eq!(receive(&rx), "first");
    sink.emit(path(), "second".into()); sink.emit(path(), "dropped".into());
    sink.close(); release.now();
    let done = sink.finish(path(), 0, Duration::from_millis(250));
    assert!(done.quiescent && !done.complete); assert_eq!(done.lost, 1);
    assert!(receive(&rx).starts_with("eventlog: 1 diagnostic records lost (queue/output unavailable); total=1 "));
    assert_eq!(receive(&rx), "second");
    assert_eq!(receive(&rx), "eventlog: closed lost=1 complete=0");
}

#[test]
fn marker_output_failure_is_never_a_complete_receipt() {
    let sink = Sink::start(2, |_, _| Err(std::io::Error::other("output unavailable")));
    let done = sink.finish(path(), 0, Duration::from_millis(250));
    assert!(done.quiescent && !done.complete); assert_eq!(done.lost, 1);
}

#[test]
fn unavailable_worker_is_quiescent_but_not_complete() {
    let sink = Sink::start(0, |_, _| panic!("no inline output"));
    sink.emit(path(), "dropped".into());
    let done = sink.finish(path(), 0, Duration::from_millis(250));
    assert!(done.quiescent && !done.complete); assert_eq!(done.lost, 1);
}

#[test]
fn an_admission_in_progress_does_not_turn_close_into_a_waiting_mutex() {
    let (tx, rx) = mpsc::channel();
    let sink = Arc::new(Sink::start(2, move |_, line| { tx.send(line.to_owned()).unwrap(); Ok(()) }));
    let held = sink.admission.lock().unwrap();
    sink.close();
    let done = sink.finish(path(), 0, Duration::from_millis(20));
    assert!(!done.quiescent);
    drop(held);
    let done = sink.finish(path(), 0, Duration::from_millis(250));
    assert!(done.quiescent && done.complete);
    assert_eq!(receive(&rx), "eventlog: closed lost=0 complete=1");
}

#[test]
fn early_output_failures_are_included_in_the_final_count() {
    let (tx, rx) = mpsc::channel();
    let sink = Sink::start(2, move |_, line| { tx.send(line.to_owned()).unwrap(); Ok(()) });
    let done = sink.finish(path(), 2, Duration::from_millis(250));
    assert!(done.quiescent && !done.complete); assert_eq!(done.lost, 2);
    assert!(receive(&rx).contains("prior_output_error=2"));
    assert_eq!(receive(&rx), "eventlog: closed lost=2 complete=0");
}

#[test]
fn overlong_close_paths_are_not_truncated_or_opened() {
    let sink = Sink::start(2, |_, _| panic!("invalid path must not be opened"));
    let done = sink.finish(PathBuf::from("x".repeat(MAX_PATH_BYTES + 1)), 0, Duration::from_millis(250));
    assert!(done.quiescent && !done.complete); assert_eq!(done.lost, 1);
}

#[test]
fn erasure_waits_for_old_io_and_closed_admission_cannot_recreate_the_file() {
    crate::eventlog::with_private_log(|| {
        let file = crate::eventlog::events_log();
        let (entered, entry) = mpsc::channel(); let (release, gate) = mpsc::channel();
        let mut release = Release(Some(release));
        let sink = Sink::start(2, move |path, line| {
            if line == "held" { entered.send(()).unwrap(); gate.recv().unwrap(); }
            crate::eventlog::append_record(path, line)
        });
        sink.emit(file.clone(), "held".into()); receive(&entry);
        assert!(!sink.finish(file.clone(), 0, Duration::from_millis(20)).quiescent);
        // No sweep is authorized yet. Once the actual callback completes, deletion is safe.
        release.now();
        assert!(sink.finish(file.clone(), 0, Duration::from_millis(250)).quiescent);
        std::fs::remove_file(&file).unwrap();
        sink.emit(file.clone(), "late".into());
        assert!(sink.finish(file.clone(), 0, Duration::ZERO).quiescent);
        assert!(!file.exists());
    });
}

#[test]
fn close_diagnostics_survive_a_blocked_output_without_claiming_zero_loss() {
    let (entered, entry) = mpsc::channel(); let (release, gate) = mpsc::channel();
    let mut release = Release(Some(release));
    let sink = Sink::start(1, move |_, line| {
        if line == "held" { entered.send(()).unwrap(); gate.recv().unwrap(); } Ok(())
    });
    sink.emit(path(), "held".into()); receive(&entry);
    sink.emit(path(), "queued".into());
    let attempt = sink.finish_attempt(path(), 0, Duration::from_millis(20));
    assert_eq!(attempt.reason, "drain_timeout");
    assert_eq!(attempt.report.exit_code(0), super::super::INCOMPLETE_EXIT_CODE);
    let record = super::super::close_diagnostic_record(attempt, 0, 0, &sink.diagnostic_snapshot()).unwrap();
    assert!(record.contains("receipt_lost=unknown") && record.contains("queued=1 capacity=1"));
    assert!(!record.contains("unused-fixture") && !record.contains("held"));
    // The separate diagnostic is already available while the actual output callback is held.
    release.now();
    let done = sink.finish_attempt(path(), 0, Duration::from_millis(250));
    assert!(done.report.complete);
    assert!(super::super::close_diagnostic_record(done, 0, 0, &sink.diagnostic_snapshot()).is_none());
}

#[test]
fn close_diagnostics_survive_permanent_output_error_and_preserve_exit_70() {
    let sink = Sink::start(2, |_, _| Err(std::io::Error::from_raw_os_error(5)));
    sink.emit(path(), "private record that must not escape".into());
    let attempt = sink.finish_attempt(path(), 0, Duration::from_millis(250));
    assert_eq!(attempt.reason, "marker_output_error");
    assert!(attempt.report.quiescent && !attempt.report.complete);
    assert_eq!(attempt.report.lost, 2);
    assert_eq!(attempt.report.exit_code(0), super::super::INCOMPLETE_EXIT_CODE);
    let record = super::super::close_diagnostic_record(attempt, 0, 0, &sink.diagnostic_snapshot()).unwrap();
    assert!(record.contains("receipt_lost=2") && record.contains("output_error=2"));
    assert!(record.contains("raw_os_error=5") && record.contains("first=output_error"));
    assert!(!record.contains("private record") && !record.contains("unused-fixture"));
    assert_eq!(record.lines().count(), 1);
    assert!(record.len() < 1024);
}

#[test]
fn close_diagnostics_distinguish_admission_contention_from_missing_writer() {
    let sink = Sink::start(1, |_, _| Ok(()));
    let held = sink.admission.lock().unwrap();
    let pending = sink.finish_attempt(path(), 0, Duration::ZERO);
    assert_eq!(pending.reason, "admission_lock_unavailable");
    assert!(!pending.report.quiescent);
    drop(held);
    assert!(sink.finish(path(), 0, Duration::from_millis(250)).complete);
    let unavailable = Sink::start(0, |_, _| panic!("no inline output"));
    let failed = unavailable.finish_attempt(path(), 0, Duration::ZERO);
    assert_eq!(failed.reason, "writer_unavailable");
    assert!(failed.report.quiescent && !failed.report.complete);
}

#[test]
fn close_diagnostics_format_pending_and_large_counters_without_payloads() {
    let pending = super::super::CloseAttempt::pending("preinit_active");
    let record = super::super::close_diagnostic_record(pending, usize::MAX, usize::MAX,
        "sink=not_started").unwrap();
    assert!(record.starts_with("eventlog: close incomplete reason=preinit_active "));
    assert!(record.contains("receipt_lost=unknown"));
    assert_eq!(record.lines().count(), 1);
    assert!(record.len() < 1024);
}
