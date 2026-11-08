//! Bounded event-file delivery through the shared writer mechanism, separate from user saves.
//! Closing admission is atomic. One reserved control slot fences all earlier record callbacks;
//! completion waits only on its existing worker ticket, outside the frame, with one deadline.

mod loss;
use loss::{Kind, Losses};

use crate::storage_worker::{Ticket, Writer};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub(super) const MAX_QUEUED_RECORDS: usize = 64;
/// At most 768 KiB queued record payload, one executing record, and a 4 KiB close path.
pub(super) const MAX_RECORD_BYTES: usize = 8 * 1024;
pub(super) const MAX_PATH_BYTES: usize = 4 * 1024;
const TRUNCATED: &str = "… [truncated]";

pub(super) fn bounded_line(mut line: String) -> String {
    if line.len() > MAX_RECORD_BYTES {
        let mut end = MAX_RECORD_BYTES - TRUNCATED.len();
        while !line.is_char_boundary(end) { end -= 1; }
        line.truncate(end);
        line.push_str(TRUNCATED);
    }
    line
}

pub(super) fn lose(total: &AtomicUsize) {
    let _ = total.try_update(Ordering::Relaxed, Ordering::Relaxed, |n| Some(n.saturating_add(1)));
}

struct Record { path: Box<Path>, line: Box<str> }
enum Command { Record(Record), Close { path: Option<Box<Path>>, prior_lost: usize } }
enum Reply { Record, Closed(super::CloseAttempt) }
#[derive(Default)]
struct Closing {
    ticket: Option<Ticket<Reply>>,
    waiting: bool,
    done: Option<super::CloseAttempt>,
}

pub(super) struct Sink {
    writer: Option<Writer<Command, Reply>>,
    lost: Arc<Losses>,
    queued: Arc<AtomicUsize>,
    capacity: usize,
    closed: AtomicBool,
    admission: Mutex<()>,
    closing: Mutex<Closing>,
}

impl Sink {
    pub(super) fn start(
        capacity: usize,
        mut write: impl FnMut(&Path, &str) -> std::io::Result<()> + Send + 'static,
    ) -> Self {
        let lost = Arc::new(Losses::new());
        let queued = Arc::new(AtomicUsize::new(0));
        let (worker_lost, worker_queued) = (lost.clone(), queued.clone());
        let mut reported = 0;
        // Payload admissions remain bounded by capacity; the extra slot is only for Close.
        let slots = if capacity == 0 { 0 } else { capacity.checked_add(1).unwrap_or(0) };
        let writer = Writer::start("eventlog", slots, move |command| match command {
            Command::Record(record) => {
                worker_queued.fetch_sub(1, Ordering::Release);
                if worker_lost.total() != reported {
                    let (total, summary) = worker_lost.report(reported, capacity, 0);
                    if let Some(summary) = summary {
                        if write(&record.path, &summary).is_ok() { reported = total; }
                    }
                }
                if let Err(error) = write(&record.path, &record.line) {
                    worker_lost.record(Kind::OutputError, worker_queued.load(Ordering::Acquire), Some(&error));
                }
                Reply::Record
            }
            Command::Close { path, prior_lost } => {
                let (total, summary) = worker_lost.report(reported, capacity, prior_lost);
                if let (Some(path), Some(summary)) = (&path, summary) { let _ = write(path, &summary); }
                let marker = format!("eventlog: closed lost={total} complete={}", usize::from(total == 0));
                let written = path.as_ref().is_some_and(|path| match write(path, &marker) {
                    Ok(()) => true,
                    Err(error) => { worker_lost.record(Kind::OutputError, 0, Some(&error)); false },
                });
                if path.is_none() { worker_lost.record(Kind::InvalidRecord, 0, None); }
                let reason = if path.is_none() { "invalid_close_path" }
                    else if !written { "marker_output_error" }
                    else if total != 0 { "record_loss" } else { "complete" };
                Reply::Closed(super::CloseAttempt { report: super::CloseReport { quiescent: true,
                    lost: total.saturating_add(usize::from(!written)), complete: total == 0 && written }, reason })
            }
        }).ok();
        Self { writer, lost, queued, capacity, closed: AtomicBool::new(false),
            admission: Mutex::new(()), closing: Mutex::new(Closing::default()) }
    }

    pub(super) fn emit(&self, path: PathBuf, line: String) {
        use std::os::unix::ffi::OsStrExt;
        if self.closed.load(Ordering::Acquire) { return; }
        let invalid = path.as_os_str().as_bytes().len() > MAX_PATH_BYTES || line.len() > MAX_RECORD_BYTES;
        let record = Record { path: path.into_boxed_path(), line: line.into_boxed_str() };
        // The lock protects only admission, never an output callback, a wait or a join.
        let _admission = self.admission.lock().unwrap_or_else(|e| e.into_inner());
        if self.closed.load(Ordering::Acquire) { return; }
        let queued = self.queued.load(Ordering::Acquire);
        if invalid { self.lost.record(Kind::InvalidRecord, queued, None); return; }
        let Some(writer) = &self.writer else {
            self.lost.record(Kind::WriterUnavailable, queued, None); return;
        };
        if queued >= self.capacity { self.lost.record(Kind::QueueFull, queued, None); return; }
        self.queued.fetch_add(1, Ordering::Relaxed);
        if let Err(error) = writer.submit_ticket(Command::Record(record)) {
            self.queued.fetch_sub(1, Ordering::Relaxed);
            let kind = match error {
                crate::storage_worker::SubmitErrorGeneric::Full(_) => Kind::QueueFull,
                crate::storage_worker::SubmitErrorGeneric::Stopped(_) => Kind::WriterStopped,
            };
            self.lost.record(kind, queued, None);
        }
    }

    pub(super) fn close(&self) { self.closed.store(true, Ordering::Release); }

    #[cfg(test)]
    pub(super) fn finish(&self, path: PathBuf, prior_lost: usize, allowance: Duration) -> super::CloseReport {
        self.finish_attempt(path, prior_lost, allowance).report
    }

    pub(super) fn diagnostic_snapshot(&self) -> String {
        let details = self.lost.try_diagnostic(self.capacity);
        format!("sink=started closed={} writer_present={} queued={} capacity={} observed_lost={} {details}",
            usize::from(self.closed.load(Ordering::Acquire)), usize::from(self.writer.is_some()),
            self.queued.load(Ordering::Acquire), self.capacity, self.lost.total())
    }

    pub(super) fn finish_attempt(&self, path: PathBuf, prior_lost: usize, allowance: Duration) -> super::CloseAttempt {
        use std::os::unix::ffi::OsStrExt;
        let started = Instant::now();
        let path = (path.as_os_str().as_bytes().len() <= MAX_PATH_BYTES).then(|| path.into_boxed_path());
        self.close();
        let unavailable = |reason| super::CloseAttempt { report: super::CloseReport { quiescent: true,
            lost: prior_lost.saturating_add(self.lost.total()), complete: false }, reason };
        let ticket = {
            let Ok(mut closing) = self.closing.try_lock() else { return super::CloseAttempt::pending("closing_lock_unavailable"); };
            if let Some(done) = closing.done { return done; }
            if closing.waiting { return super::CloseAttempt::pending("close_wait_in_progress"); }
            if closing.ticket.is_none() {
                let Ok(_admission) = self.admission.try_lock() else { return super::CloseAttempt::pending("admission_lock_unavailable"); };
                let Some(writer) = &self.writer else {
                    let done = unavailable("writer_unavailable"); closing.done = Some(done); return done;
                };
                use crate::storage_worker::SubmitErrorGeneric;
                match writer.submit_ticket(Command::Close { path, prior_lost }) {
                    Ok(ticket) => closing.ticket = Some(ticket),
                    Err(SubmitErrorGeneric::Full(_)) => return super::CloseAttempt::pending("close_queue_full"),
                    Err(SubmitErrorGeneric::Stopped(_)) => {
                        let done = unavailable("writer_stopped"); closing.done = Some(done); return done;
                    }
                }
            }
            closing.waiting = true;
            closing.ticket.take().expect("admitted close ticket")
        };
        let mut answer = None;
        let result = ticket.drain_for(allowance.saturating_sub(started.elapsed()), |reply| {
            if let Reply::Closed(done) = reply { answer = Some(done); }
            false
        });
        let mut closing = self.closing.lock().unwrap_or_else(|e| e.into_inner());
        closing.waiting = false;
        match result {
            crate::task::time::DrainResult::Complete => closing.done = answer,
            crate::task::time::DrainResult::Disconnected => closing.done = Some(unavailable("close_disconnected")),
            crate::task::time::DrainResult::TimedOut => closing.ticket = Some(ticket),
        }
        closing.done.unwrap_or_else(|| super::CloseAttempt::pending(match result {
            crate::task::time::DrainResult::TimedOut => "drain_timeout",
            _ => "close_reply_missing",
        }))
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod close_tests;
