//! Loss reasons are recorded where they occur, not when the writer later reports them.
//! One fixed snapshot contains no record contents or paths and does not alter admission.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::Instant;

#[derive(Clone, Copy)]
pub(super) enum Kind { QueueFull, WriterUnavailable, WriterStopped, InvalidRecord, OutputError }

impl Kind {
    fn name(self) -> &'static str {
        match self {
            Self::QueueFull => "queue_full",
            Self::WriterUnavailable => "writer_unavailable",
            Self::WriterStopped => "writer_stopped",
            Self::InvalidRecord => "invalid_record",
            Self::OutputError => "output_error",
        }
    }
}

struct First {
    kind: Kind,
    elapsed_ms: u128,
    queued: usize,
    error: Option<(std::io::ErrorKind, Option<i32>)>,
}

#[derive(Default)]
struct State { counts: [usize; 5], first: Option<First> }

pub(super) struct Losses {
    total: AtomicUsize,
    state: Mutex<State>,
    started: Instant,
}

impl Losses {
    pub(super) fn new() -> Self {
        Self { total: AtomicUsize::new(0), state: Mutex::new(State::default()), started: Instant::now() }
    }

    pub(super) fn total(&self) -> usize { self.total.load(Ordering::Acquire) }

    pub(super) fn try_diagnostic(&self, capacity: usize) -> String {
        let state = match self.state.try_lock() {
            Ok(state) => state,
            Err(std::sync::TryLockError::Poisoned(error)) => error.into_inner(),
            Err(std::sync::TryLockError::WouldBlock) => return "counts=unavailable first=unavailable".into(),
        };
        let [queue_full, writer_unavailable, writer_stopped, invalid_record, output_error] = state.counts;
        let first = state.first.as_ref().map_or_else(|| "first=none".into(), |first| {
            let (kind, code) = first.error.map_or_else(|| ("none".into(), "none".into()), |(kind, code)| {
                (format!("{kind:?}"), code.map_or_else(|| "none".into(), |n| n.to_string()))
            });
            format!("first={} after_ms={} first_queued={} first_capacity={capacity} error_kind={kind} raw_os_error={code}",
                first.kind.name(), first.elapsed_ms, first.queued)
        });
        format!("counts=available queue_full={queue_full} writer_unavailable={writer_unavailable} writer_stopped={writer_stopped} invalid_record={invalid_record} output_error={output_error} {first}")
    }

    pub(super) fn record(&self, kind: Kind, queued: usize, error: Option<&std::io::Error>) {
        // Only loss accounting holds this lock; it never encloses output or worker admission.
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let counter = &mut state.counts[kind as usize];
        *counter = counter.saturating_add(1);
        if state.first.is_none() {
            state.first = Some(First { kind, elapsed_ms: self.started.elapsed().as_millis(), queued,
                error: error.map(|e| (e.kind(), e.raw_os_error())) });
        }
        self.total.store(self.total.load(Ordering::Relaxed).saturating_add(1), Ordering::Release);
    }

    pub(super) fn report(&self, reported: usize, capacity: usize, prior_output_error: usize)
        -> (usize, Option<String>) {
        let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let total = self.total.load(Ordering::Relaxed).saturating_add(prior_output_error);
        if total == reported { return (total, None); }
        let first = state.first.as_ref().map_or_else(|| "first=unavailable".to_owned(), |first| {
            let error = first.error.map_or_else(|| "error_kind=none raw_os_error=none".to_owned(), |(kind, code)| {
                format!("error_kind={kind:?} raw_os_error={}", code.map_or_else(|| "none".to_owned(), |n| n.to_string()))
            });
            format!("first={} after_ms={} queued={} capacity={} {error}",
                first.kind.name(), first.elapsed_ms, first.queued, capacity)
        });
        let [queue_full, writer_unavailable, writer_stopped, invalid_record, output_error] = state.counts;
        (total, Some(format!(
            "eventlog: {} diagnostic records lost (queue/output unavailable); total={total} queue_full={queue_full} writer_unavailable={writer_unavailable} writer_stopped={writer_stopped} invalid_record={invalid_record} output_error={output_error} prior_output_error={prior_output_error}; {first}",
            total.saturating_sub(reported))))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn later_losses_preserve_the_first_loss_context_and_all_reason_counts() {
        let losses = Losses::new();
        losses.record(Kind::QueueFull, 2, None);
        let (first_total, first) = losses.report(0, 2, 0);
        let first = first.unwrap();
        losses.record(Kind::OutputError, 0, Some(&std::io::Error::from_raw_os_error(5)));
        let (total, later) = losses.report(first_total, 2, 0);
        let later = later.unwrap();
        assert_eq!((first_total, total), (1, 2));
        assert!(later.contains("queue_full=1") && later.contains("output_error=1"));
        assert_eq!(first.split("; ").last(), later.split("; ").last());
        assert!(later.contains("first=queue_full") && later.contains("queued=2 capacity=2"));
        assert!(losses.report(total, 2, 0).1.is_none());
    }

    #[test]
    fn close_diagnostics_do_not_wait_for_loss_accounting() {
        let losses = Losses::new();
        let held = losses.state.lock().unwrap();
        assert_eq!(losses.try_diagnostic(64), "counts=unavailable first=unavailable");
        drop(held);
        assert!(losses.try_diagnostic(64).contains("counts=available"));
    }
}
