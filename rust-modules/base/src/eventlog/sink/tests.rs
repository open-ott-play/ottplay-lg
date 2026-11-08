use super::*;
use crate::eventlog::{events_log, log, queued_tests::with_sink, with_private_log};
use std::sync::mpsc;
use std::time::Duration;

struct Release(Option<mpsc::Sender<()>>);
impl Release {
    fn now(&mut self) { if let Some(sender) = self.0.take() { let _ = sender.send(()); } }
}
impl Drop for Release {
    fn drop(&mut self) { self.now(); }
}
fn receive<T>(rx: &mpsc::Receiver<T>) -> T {
    rx.recv_timeout(Duration::from_secs(2)).expect("bounded output receipt")
}

#[test]
fn accepted_records_keep_fifo_order() {
    let (tx, rx) = mpsc::channel();
    let sink = Arc::new(Sink::start(4, move |_path, line| { tx.send(line.to_owned()).unwrap(); Ok(()) }));
    with_sink(sink, || { log("first"); log("second"); log("third"); });
    assert_eq!([receive(&rx), receive(&rx), receive(&rx)], ["first", "second", "third"]);
}

#[test]
fn a_full_queue_drops_without_inline_io_and_reports_loss_on_recovery() {
    let (tx, rx) = mpsc::channel();
    let (release, gate) = mpsc::channel();
    let mut release = Release(Some(release));
    let sink = Arc::new(Sink::start(1, move |_path, line| {
        tx.send(line.to_owned()).unwrap();
        if line == "first" { gate.recv().unwrap(); }
        Ok(())
    }));
    with_sink(sink.clone(), || log("first"));
    assert_eq!(receive(&rx), "first"); // The one executing record is held outside the queue.
    with_sink(sink.clone(), || { log("second"); log("third"); });
    assert_eq!(sink.lost.total(), 1);
    assert!(matches!(rx.try_recv(), Err(mpsc::TryRecvError::Empty)));
    release.now();
    assert!(receive(&rx).starts_with("eventlog: 1 diagnostic records lost (queue/output unavailable); total=1 "));
    assert_eq!(receive(&rx), "second");
    drop(sink);
    assert!(matches!(rx.recv_timeout(Duration::from_secs(2)), Err(mpsc::RecvTimeoutError::Disconnected)));
}

#[test]
fn an_unavailable_writer_never_uses_inline_output() {
    let writes = Arc::new(AtomicUsize::new(0));
    let observed = writes.clone();
    // The real shared constructor rejects zero capacity; the sink retains the unavailable state.
    let sink = Arc::new(Sink::start(0, move |_path, _line| { observed.fetch_add(1, Ordering::Relaxed); Ok(()) }));
    with_sink(sink.clone(), || log("unavailable-output"));
    assert_eq!(sink.lost.total(), 1);
    assert_eq!(writes.load(Ordering::Relaxed), 0);
    assert!(sink.lost.report(0, 0, 0).1.unwrap().contains("writer_unavailable=1"));
}

#[test]
fn a_stopped_writer_never_uses_inline_output() {
    let writes = Arc::new(AtomicUsize::new(0));
    let observed = writes.clone();
    let sink = Arc::new(Sink::start(2, move |_path, _line| {
        observed.fetch_add(1, Ordering::Relaxed);
        panic!("injected terminal output failure")
    }));
    let ticket = sink.writer.as_ref().unwrap().submit_ticket(Command::Close {
        path: Some(PathBuf::from("unused-fixture").into_boxed_path()), prior_lost: 0,
    }).ok().unwrap();
    assert!(ticket.wait_blocking().is_err()); // Writer closes admission before closing this receipt.
    with_sink(sink.clone(), || log("after-output-stopped"));
    assert_eq!(sink.lost.total(), 1);
    assert_eq!(writes.load(Ordering::Relaxed), 1);
    assert!(sink.lost.report(0, 2, 0).1.unwrap().contains("writer_stopped=1"));
}

#[test]
fn output_errors_are_counted_without_retrying_on_the_producer() {
    let (tx, rx) = mpsc::channel();
    let sink = Arc::new(Sink::start(3, move |_path, line| {
        tx.send(line.to_owned()).unwrap();
        if line == "discard" { Err(std::io::Error::other("injected output error")) } else { Ok(()) }
    }));
    with_sink(sink.clone(), || { log("discard"); log("kept"); });
    assert_eq!(receive(&rx), "discard");
    assert!(receive(&rx).starts_with("eventlog: 1 diagnostic records lost (queue/output unavailable); total=1 "));
    assert_eq!(receive(&rx), "kept");
    assert_eq!(sink.lost.total(), 1);
}

#[test]
fn log_scrubs_and_bounds_records_before_the_output_receives_them() {
    let (tx, rx) = mpsc::channel();
    let sink = Arc::new(Sink::start(3, move |_path, line| { tx.send(line.to_owned()).unwrap(); Ok(()) }));
    with_sink(sink, || {
        log("fixture X-Plex-Token=fixture-secret");
        log(&"é".repeat(MAX_RECORD_BYTES));
    });
    assert_eq!(receive(&rx), "fixture X-Plex-Token=<redacted>");
    let bounded = receive(&rx);
    assert!(bounded.len() <= MAX_RECORD_BYTES);
    assert!(bounded.ends_with(TRUNCATED));
    assert!(bounded.starts_with("éé"));
}

#[test]
fn each_log_record_retains_its_own_resolved_output_path() {
    let (tx, rx) = mpsc::channel();
    let sink = Arc::new(Sink::start(3, move |path, line| { tx.send((path.to_owned(), line.to_owned())).unwrap(); Ok(()) }));
    let (first, second) = with_sink(sink, || {
        let first = with_private_log(|| { log("one"); events_log() });
        let second = with_private_log(|| { log("two"); events_log() });
        (first, second)
    });
    assert_ne!(first, second);
    assert_eq!(receive(&rx), (first, "one".into()));
    assert_eq!(receive(&rx), (second, "two".into()));
}

#[test]
fn oversized_output_paths_are_counted_and_never_written() {
    let writes = Arc::new(AtomicUsize::new(0));
    let observed = writes.clone();
    let sink = Sink::start(2, move |_path, _line| { observed.fetch_add(1, Ordering::Relaxed); Ok(()) });
    sink.emit(PathBuf::from("x".repeat(MAX_PATH_BYTES + 1)), "record".into());
    assert_eq!(sink.lost.total(), 1);
    assert_eq!(writes.load(Ordering::Relaxed), 0);
    assert!(sink.lost.report(0, 2, 0).1.unwrap().contains("invalid_record=1"));
}

#[test]
fn the_worker_uses_the_secure_file_sink_at_the_record_path() {
    with_private_log(|| {
        let path = events_log();
        let (tx, rx) = mpsc::channel();
        let sink = Arc::new(Sink::start(2, move |path, line| {
            let result = crate::eventlog::append_record(path, line);
            tx.send(result.is_ok()).unwrap();
            result
        }));
        with_sink(sink, || log("worker-file-record"));
        assert!(receive(&rx));
        assert_eq!(std::fs::read_to_string(path).unwrap(), "worker-file-record\n");
    });
}

#[test]
fn loss_diagnostics_distinguish_a_full_queue_at_the_producer() {
    let (tx, rx) = mpsc::channel();
    let (release, gate) = mpsc::channel();
    let mut release = Release(Some(release));
    let sink = Sink::start(1, move |_path, line| {
        tx.send(line.to_owned()).unwrap();
        if line == "held" { gate.recv().unwrap(); }
        Ok(())
    });
    let path = PathBuf::from("unused-fixture");
    sink.emit(path.clone(), "held".into());
    assert_eq!(receive(&rx), "held");
    sink.emit(path.clone(), "queued".into());
    sink.emit(path.clone(), "refused".into());
    assert!(matches!(rx.try_recv(), Err(mpsc::TryRecvError::Empty)));
    release.now();
    let summary = receive(&rx);
    assert!(summary.contains("queue_full=1") && summary.contains("output_error=0"), "{summary}");
    assert!(summary.contains("first=queue_full") && summary.contains("queued=1 capacity=1"), "{summary}");
    assert_eq!(receive(&rx), "queued");
    let report = sink.finish(path, 0, Duration::from_millis(250));
    assert!(report.quiescent && !report.complete);
    assert_eq!(report.lost, 1);
    assert_eq!(receive(&rx), "eventlog: closed lost=1 complete=0");
}

#[test]
fn loss_diagnostics_report_a_last_output_failure_before_the_close_marker() {
    let (tx, rx) = mpsc::channel();
    let sink = Sink::start(1, move |_path, line| {
        tx.send(line.to_owned()).unwrap();
        if line == "discard" { Err(std::io::Error::from_raw_os_error(5)) } else { Ok(()) }
    });
    let path = PathBuf::from("unused-fixture");
    sink.emit(path.clone(), "discard".into());
    assert_eq!(receive(&rx), "discard");
    let report = sink.finish(path, 0, Duration::from_millis(250));
    assert!(report.quiescent && !report.complete);
    assert_eq!(report.lost, 1);
    let summary = receive(&rx);
    assert!(summary.contains("output_error=1") && summary.contains("queue_full=0"), "{summary}");
    assert!(summary.contains("first=output_error") && summary.contains("raw_os_error=5"), "{summary}");
    assert_eq!(receive(&rx), "eventlog: closed lost=1 complete=0");
}
