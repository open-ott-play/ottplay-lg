//! Completion ownership shared by bounded catalogue and metadata workers.
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

pub(super) fn spawn(
    name: &'static str,
    done: Arc<AtomicBool>,
    work: impl FnOnce(&plx_base::task::OffFrame) + Send + 'static,
) -> bool {
    done.store(false, Ordering::Release);
    let not_started = done.clone();
    let started = plx_base::task::spawn_off_frame(name, move |off_frame| {
        let _completion = Completion(done);
        work(off_frame);
    });
    if !started {
        not_started.store(true, Ordering::Release);
        plx_machine::idle::invalidate();
    }
    started
}

struct Completion(Arc<AtomicBool>);
impl Drop for Completion {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
        // Wake after publishing completion: a cancelled worker has no result
        // event, but the frame must still launch its queued replacement.
        plx_machine::idle::invalidate();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn worker_completion_is_signalled_on_unwind() {
        let done = Arc::new(AtomicBool::new(false));
        let worker_done = done.clone();
        let result = std::panic::catch_unwind(move || {
            let _completion = Completion(worker_done);
            panic!("cancelled worker fixture");
        });
        assert!(result.is_err());
        assert!(done.load(Ordering::Acquire));
    }
}
