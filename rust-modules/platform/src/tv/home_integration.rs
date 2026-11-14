//! Optional Home companion handoff, independent of the television's own Home.
//! The installed port owns the fixed destination and platform request.
use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Launch {
    /// The platform accepted a request; foreground display is not confirmed.
    Requested,
    Rejected,
    Unavailable,
}

/// Off-frame only. Cancellation is sticky and is also passed to the port so
/// setup can recheck it before admitting the platform effect. No automatic retry.
pub fn open(cancelled: &AtomicBool) -> Launch {
    open_with(cancelled, |flag| {
        (super::port().open_home_integration)(flag)
    })
}

fn open_with(cancelled: &AtomicBool, effect: impl FnOnce(&AtomicBool) -> Launch) -> Launch {
    if cancelled.load(Ordering::Acquire) {
        Launch::Unavailable
    } else {
        effect(cancelled)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absent_port_home_integration_is_unavailable() {
        let cancelled = AtomicBool::new(false);
        assert_eq!(
            open_with(&cancelled, super::super::ABSENT.open_home_integration),
            Launch::Unavailable
        );
    }

    #[test]
    fn cancellation_skips_port_effect_and_live_request_keeps_the_same_flag() {
        use std::cell::Cell;
        let cancelled = AtomicBool::new(true);
        let calls = Cell::new(0);
        let effect = |flag: &AtomicBool| {
            assert!(std::ptr::eq(flag, &cancelled));
            calls.set(calls.get() + 1);
            Launch::Requested
        };
        assert_eq!(open_with(&cancelled, effect), Launch::Unavailable);
        assert_eq!(calls.get(), 0);
        cancelled.store(false, Ordering::Release);
        assert_eq!(open_with(&cancelled, effect), Launch::Requested);
        assert_eq!(calls.get(), 1);
        cancelled.store(true, Ordering::Release);
        assert_eq!(open_with(&cancelled, effect), Launch::Unavailable);
        assert_eq!(
            calls.get(),
            1,
            "Cancellation cannot replay a platform effect"
        );
    }
}
