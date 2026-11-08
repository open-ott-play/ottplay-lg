//! Time at the worker boundary, never a clock for a stepped machine.
//!
//! A worker receives [`OffFrame`] from the task launcher. It may use a finite
//! [`Budget`] for blocking IO, or observe a [`Deadline`] carried through a queue.
//! Neither exposes a raw clock: construction and observation need the worker
//! capability. A [`MainDeadline`] permits main-thread observations of one fixed
//! deadline after authorization by the existing [`MainThread`] capability.
//! [`ClockSample`] hands its scalar epoch to the frame runner as an
//! input; converting an expiry back to a deadline is pure and does not renew it.
//! UI cooldowns still count the frame ticks they are handed.

use super::{assert_may_block, BlockingLabel, MainThread, OffFrame};
use crate::checkpoint::{Checkpoint, Flow};
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::marker::PhantomData;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// A fixed point in one monotonic clock domain. Moving it through a queue or
/// checking it repeatedly never grants the operation more time.
#[derive(Clone, Copy, Debug)]
pub struct Deadline {
    origin: Instant,
    allowance: Duration,
}

impl Deadline {
    pub fn after(_worker: &OffFrame, allowance: Duration) -> Self {
        Self {
            origin: Instant::now(),
            allowance,
        }
    }

    fn remaining_at(self, now: Instant) -> Duration {
        self.allowance
            .saturating_sub(now.saturating_duration_since(self.origin))
    }

    /// Pure observation at an already captured boundary. The deadline keeps
    /// its original monotonic origin, including all time spent in a queue.
    pub fn remaining_at_sample(self, sample: ClockSample) -> Duration {
        self.remaining_at(sample.origin)
    }

    /// Authorize observations of this fixed deadline on the main thread. This
    /// does not start a new budget or expose a general-purpose clock.
    pub fn on_main(self, _main: &MainThread) -> MainDeadline {
        MainDeadline { deadline: self, _main: PhantomData }
    }

    pub fn remaining(self, _worker: &OffFrame) -> Duration {
        self.remaining_at(Instant::now())
    }

    pub fn expired(self, worker: &OffFrame) -> bool {
        self.remaining(worker).is_zero()
    }

    /// The existing transport checkpoint protocol, bounded both by the fixed
    /// deadline and by how long the caller permits cancellation to go unseen.
    pub fn checkpoint<'a>(
        self,
        worker: &'a OffFrame,
        cancelled: &'a dyn Fn() -> bool,
        poll: Duration,
    ) -> DeadlineCheckpoint<'a> {
        DeadlineCheckpoint {
            deadline: self,
            _worker: worker,
            cancelled,
            poll: poll.max(Duration::from_millis(1)),
        }
    }
}

/// Main-thread observation authority for one already fixed deadline. The raw
/// pointer marker deliberately makes this value !Send and !Sync; workers carry
/// the underlying Deadline instead and observe it with their OffFrame token.
#[derive(Clone, Copy, Debug)]
pub struct MainDeadline {
    deadline: Deadline,
    _main: PhantomData<*const ()>,
}
impl MainDeadline {
    pub fn remaining(self) -> Duration { self.deadline.remaining_at(Instant::now()) }
    pub fn remaining_at_sample(self, sample: ClockSample) -> Duration {
        self.deadline.remaining_at_sample(sample)
    }
}

/// A finite IO budget that can also measure the work already spent. Unlike a
/// sequence of per-read timeouts, progress does not renew its total allowance.
#[derive(Clone, Copy, Debug)]
pub struct Budget {
    deadline: Deadline,
}

impl Budget {
    pub fn start(worker: &OffFrame, allowance: Duration) -> Self {
        Self {
            deadline: Deadline::after(worker, allowance),
        }
    }

    pub fn deadline(self) -> Deadline {
        self.deadline
    }

    pub fn remaining(self, worker: &OffFrame) -> Duration {
        self.deadline.remaining(worker)
    }

    pub fn expired(self, worker: &OffFrame) -> bool {
        self.deadline.expired(worker)
    }

    pub fn spent(self, _worker: &OffFrame) -> Duration {
        Instant::now().saturating_duration_since(self.deadline.origin)
    }

    /// Wait without losing cancellation or the operation's total deadline.
    /// `false` means cancelled/timed out, not successful completion of the wait.
    pub fn wait(
        self,
        worker: &OffFrame,
        delay: Duration,
        poll: Duration,
        cancelled: &dyn Fn() -> bool,
    ) -> bool {
        let pause = Deadline::after(worker, delay);
        let poll = poll.max(Duration::from_millis(1));
        loop {
            if cancelled() || self.expired(worker) {
                return false;
            }
            let remaining = pause.remaining(worker);
            if remaining.is_zero() {
                return true;
            }
            std::thread::sleep(remaining.min(self.remaining(worker)).min(poll));
        }
    }
}

pub struct DeadlineCheckpoint<'a> {
    deadline: Deadline,
    _worker: &'a OffFrame,
    cancelled: &'a dyn Fn() -> bool,
    poll: Duration,
}

impl Checkpoint for DeadlineCheckpoint<'_> {
    fn check(&mut self) -> Flow {
        if (self.cancelled)() {
            return Flow::Stop;
        }
        let now = Instant::now();
        let remaining = self.deadline.remaining_at(now);
        if remaining.is_zero() {
            return Flow::Stop;
        }
        Flow::Continue {
            next_check: now.checked_add(remaining.min(self.poll)),
        }
    }
}

/// One epoch scalar paired with a monotonic sample. A captured value contains
/// actual wall time; [`Self::steady_from`] projects a stable local timeline from
/// an earlier capture. Keep the raw value for calendar/EPG data. The application
/// supplies the projected value to deadline-driven machines as an input, never
/// substituting a made-up epoch when a capture fails.
#[derive(Clone, Copy, Debug)]
pub struct ClockSample {
    origin: Instant,
    unix_ms: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ClockUnavailable;

impl ClockSample {
    pub fn capture(_worker: &OffFrame) -> Result<Self, ClockUnavailable> {
        Self::sample()
    }

    /// The frame driver samples external time at its input boundary and passes
    /// the resulting value to the runtime. Machines must never call this or
    /// retain the main-thread capability merely to obtain their own clock.
    pub fn capture_main(_main: &MainThread) -> Result<Self, ClockUnavailable> {
        Self::sample()
    }

    fn sample() -> Result<Self, ClockUnavailable> {
        let origin = Instant::now();
        let unix_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .ok()
            .and_then(|value| u64::try_from(value.as_millis()).ok())
            .ok_or(ClockUnavailable)?;
        Ok(Self { origin, unix_ms })
    }

    pub fn unix_ms(self) -> u64 {
        self.unix_ms
    }

    /// Project this sample onto one fixed anchor's local timeline. Wall-clock
    /// corrections do not change admitted command lifetimes. Keep the first
    /// anchor unchanged: rounding the total interval once preserves fractional
    /// milliseconds that would disappear if each frame used its predecessor.
    /// The current monotonic origin is retained for deadline conversion.
    pub fn steady_from(self, anchor: ClockSample) -> ClockSample {
        let elapsed_ms = self
            .origin
            .saturating_duration_since(anchor.origin)
            .as_millis();
        let elapsed_ms = u64::try_from(elapsed_ms).unwrap_or(u64::MAX);
        ClockSample {
            origin: self.origin,
            unix_ms: anchor.unix_ms.saturating_add(elapsed_ms),
        }
    }

    /// Deterministic boundary samples for dependent-crate tests. Advancing a
    /// sample never changes a previously constructed Deadline.
    #[cfg(any(test, feature = "test-support"))]
    pub fn advance_for_test(self, elapsed: Duration) -> Self {
        Self {
            origin: self.origin.checked_add(elapsed).expect("bounded test clock"),
            unix_ms: self.unix_ms.saturating_add(u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)),
        }
    }

    /// Pure conversion: an expiry before the sample is already expired. Queue
    /// latency consumes the allowance, and later wall-clock changes cannot
    /// revive or extend an admitted operation.
    pub fn deadline_at_unix_ms(self, expiry: u64) -> Deadline {
        Deadline {
            origin: self.origin,
            allowance: Duration::from_millis(expiry.saturating_sub(self.unix_ms)),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DrainResult {
    Complete,
    TimedOut,
    Disconnected,
}

/// Drain already admitted worker results during teardown, outside the frame
/// loop. Like `task::join`, the blocking/clock boundary lives here. A callback
/// returning `false` has received everything it needs. Continuous unrelated
/// messages cannot extend the deadline. The callback must itself be bounded.
pub fn drain_for<T>(
    receiver: &Receiver<T>,
    allowance: Duration,
    mut keep_going: impl FnMut(T) -> bool,
) -> DrainResult {
    let _guard = assert_may_block(const { &BlockingLabel::new("task result drain") });
    let origin = Instant::now();
    let deadline = Deadline { origin, allowance };
    loop {
        let remaining = deadline.remaining_at(Instant::now());
        if remaining.is_zero() {
            return DrainResult::TimedOut;
        }
        match receiver.recv_timeout(remaining) {
            Ok(value) => {
                if !keep_going(value) {
                    return DrainResult::Complete;
                }
            }
            Err(RecvTimeoutError::Timeout) => return DrainResult::TimedOut,
            Err(RecvTimeoutError::Disconnected) => return DrainResult::Disconnected,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    #[test]
    fn steady_samples_ignore_wall_clock_jumps_without_renewing_deadlines() {
        let at = Instant::now();
        let anchor = ClockSample {
            origin: at,
            unix_ms: 10_000,
        };
        let forward = ClockSample {
            origin: at + Duration::from_millis(250),
            unix_ms: 3_600_250,
        };
        let backward = ClockSample {
            origin: at + Duration::from_millis(800),
            unix_ms: 100,
        };
        let first = forward.steady_from(anchor);
        let second = backward.steady_from(anchor);
        assert_eq!(first.unix_ms(), 10_250);
        assert_eq!(second.unix_ms(), 10_800);
        assert_eq!(first.origin, forward.origin);
        assert_eq!(second.origin, backward.origin);
        for sample in [anchor, first, second] {
            let deadline = sample.deadline_at_unix_ms(11_000);
            assert_eq!(
                deadline.remaining_at(at + Duration::from_millis(950)),
                Duration::from_millis(50)
            );
            assert_eq!(
                deadline.remaining_at(at + Duration::from_secs(1)),
                Duration::ZERO
            );
        }
        assert_eq!(
            forward.unix_ms(),
            3_600_250,
            "calendar consumers keep the original sample"
        );
        assert_eq!(backward.unix_ms(), 100);
    }

    #[test]
    fn steady_samples_accumulate_fractional_intervals_from_the_fixed_anchor() {
        let at = Instant::now();
        let anchor = ClockSample {
            origin: at,
            unix_ms: 10_000,
        };
        let deadline = anchor.deadline_at_unix_ms(10_005);
        for frame in 1..=10 {
            let micros = frame * 600;
            let raw = ClockSample {
                origin: at + Duration::from_micros(micros),
                unix_ms: 1,
            };
            let steady = raw.steady_from(anchor);
            assert_eq!(steady.unix_ms(), 10_000 + micros / 1000);
            if frame == 10 {
                assert_eq!(steady.unix_ms(), 10_006);
                assert_eq!(deadline.remaining_at(steady.origin), Duration::ZERO);
                assert_eq!(
                    steady
                        .deadline_at_unix_ms(10_005)
                        .remaining_at(steady.origin),
                    Duration::ZERO
                );
            }
        }
    }

    #[test]
    fn steady_sample_epoch_addition_saturates() {
        let at = Instant::now();
        let anchor = ClockSample {
            origin: at,
            unix_ms: u64::MAX - 1,
        };
        let raw = ClockSample {
            origin: at + Duration::from_millis(10),
            unix_ms: 1,
        };
        assert_eq!(raw.steady_from(anchor).unix_ms(), u64::MAX);
    }

    #[test]
    fn queued_deadline_keeps_its_original_allowance_and_expires_at_the_boundary() {
        let at = Instant::now();
        let sample = ClockSample {
            origin: at,
            unix_ms: 10_000,
        };
        let deadline = sample.deadline_at_unix_ms(10_100);
        assert_eq!(
            deadline.remaining_at(at + Duration::from_millis(75)),
            Duration::from_millis(25)
        );
        assert_eq!(
            deadline.remaining_at(at + Duration::from_millis(100)),
            Duration::ZERO
        );
        assert_eq!(
            deadline.remaining_at(at + Duration::from_millis(200)),
            Duration::ZERO
        );
        assert_eq!(
            sample.deadline_at_unix_ms(1).remaining_at(at),
            Duration::ZERO
        );
    }

    #[test]
    fn main_deadline_observation_does_not_restart_queued_allowance() {
        let sample = ClockSample { origin: Instant::now(), unix_ms: 10_000 };
        let deadline = sample.deadline_at_unix_ms(11_000);
        let delayed = sample.advance_for_test(Duration::from_millis(800));
        let main = unsafe { MainThread::assume() };
        let observer = deadline.on_main(&main);
        assert_eq!(observer.remaining_at_sample(delayed), Duration::from_millis(200));
        assert_eq!(observer.remaining_at_sample(sample.advance_for_test(Duration::from_secs(1))), Duration::ZERO);
        assert_eq!(deadline.remaining_at_sample(sample.advance_for_test(Duration::from_secs(2))), Duration::ZERO);
    }

    #[test]
    fn cancellation_and_expiry_stop_the_existing_transport_checkpoint() {
        let worker = OffFrame::for_test();
        let deadline = Deadline::after(&worker, Duration::from_secs(60));
        assert_eq!(
            deadline
                .checkpoint(&worker, &|| true, Duration::from_secs(1))
                .check(),
            Flow::Stop
        );
        let expired = Deadline::after(&worker, Duration::ZERO);
        assert_eq!(
            expired
                .checkpoint(&worker, &|| false, Duration::from_secs(1))
                .check(),
            Flow::Stop
        );
        assert!(!Budget::start(&worker, Duration::from_secs(1)).wait(
            &worker,
            Duration::from_secs(1),
            Duration::from_millis(10),
            &|| true
        ));
    }

    #[test]
    fn epoch_sample_is_real_and_zero_deadline_is_already_expired() {
        let worker = OffFrame::for_test();
        let sample = ClockSample::capture(&worker).unwrap();
        assert!(sample.unix_ms() > 0);
        assert!(sample.deadline_at_unix_ms(0).expired(&worker));
    }

    #[test]
    fn result_drain_stops_at_completion_timeout_or_disconnection() {
        let (tx, rx) = mpsc::channel();
        tx.send(1).unwrap();
        tx.send(2).unwrap();
        tx.send(3).unwrap();
        let mut seen = Vec::new();
        assert_eq!(
            drain_for(&rx, Duration::from_secs(1), |value| {
                seen.push(value);
                value != 2
            }),
            DrainResult::Complete
        );
        assert_eq!(seen, vec![1, 2]);
        assert_eq!(rx.try_recv(), Ok(3));
        assert_eq!(
            drain_for(&rx, Duration::ZERO, |_| true),
            DrainResult::TimedOut
        );
        drop(tx);
        assert_eq!(
            drain_for(&rx, Duration::from_secs(1), |_| true),
            DrainResult::Disconnected
        );
    }
}
