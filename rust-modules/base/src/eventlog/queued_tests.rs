//! Real `log` ingress with an explicitly scoped output; no process-global test sink.
use super::{log, sink::Sink};
use std::cell::RefCell;
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

thread_local! {
    static OUTPUT: RefCell<Option<Arc<Sink>>> = const { RefCell::new(None) };
}

pub(super) fn with_sink<R>(sink: Arc<Sink>, f: impl FnOnce() -> R) -> R {
    struct Restore(Option<Arc<Sink>>);
    impl Drop for Restore {
        fn drop(&mut self) { OUTPUT.with(|slot| *slot.borrow_mut() = self.0.take()); }
    }
    let _restore = Restore(OUTPUT.with(|slot| slot.borrow_mut().replace(sink)));
    f()
}

pub(super) fn emit_if_set(path: &std::path::Path, line: &str) -> bool {
    let sink = OUTPUT.with(|slot| slot.borrow().clone());
    if let Some(sink) = sink { sink.emit(path.to_owned(), line.to_owned()); true } else { false }
}

#[test]
fn log_returns_before_blocked_output_is_released() {
    let (entered, entry) = mpsc::channel();
    let (release, gate) = mpsc::channel();
    let gate = Mutex::new(gate);
    let sink = Arc::new(Sink::start(2, move |_path, line| {
        entered.send(line.to_owned()).unwrap();
        gate.lock().unwrap().recv().unwrap();
        Ok(())
    }));
    let (finished, done) = mpsc::channel();
    let producer = std::thread::spawn(move || {
        with_sink(sink, || log("blocked-output: first"));
        // The scoped sink was dropped too: dropping the last producer must not join its writer.
        finished.send(()).unwrap();
    });
    // Always release and join the producer before asserting, including the failing baseline.
    let reached_output = entry.recv_timeout(Duration::from_secs(2));
    let returned_before_release = done.recv_timeout(Duration::from_secs(2));
    let _ = release.send(());
    producer.join().unwrap();
    assert_eq!(reached_output.unwrap(), "blocked-output: first");
    assert!(returned_before_release.is_ok(), "log waited for blocked output on its caller");
}
