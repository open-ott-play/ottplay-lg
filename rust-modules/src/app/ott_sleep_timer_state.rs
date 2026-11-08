//! Sans-IO inactivity deadline. Polling never counts as input. The runtime owns
//! this state; only the selected duration is persisted across application runs.
use super::Choice;

#[derive(Clone, Copy)]
pub(super) struct Sample<'a> {
    pub now_ms: u64,
    pub choice: Choice,
    pub revision: Option<&'a str>,
    pub activity: u32,
    pub permitted: bool,
    pub defer_expiry: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Fx { EnterStandby }

struct Armed {
    choice: Choice,
    revision: Option<String>,
    activity: u32,
    last_ms: u64,
    deadline_ms: Option<u64>,
    fired: bool,
}

#[derive(Default)]
pub(in crate::app::ott_runtime) struct State { armed: Option<Armed> }

impl State {
    pub(super) fn step(&mut self, sample: Sample<'_>) -> Option<Fx> {
        let Some(duration_ms) = sample.choice.duration_ms().filter(|_| sample.permitted) else {
            self.armed = None;
            return None;
        };
        let replace = self.armed.as_ref().is_none_or(|armed| {
            armed.choice != sample.choice || armed.revision.as_deref() != sample.revision
                || armed.activity != sample.activity || sample.now_ms < armed.last_ms
        });
        if replace {
            self.armed = Some(Armed {
                choice: sample.choice,
                revision: sample.revision.map(str::to_owned),
                activity: sample.activity,
                last_ms: sample.now_ms,
                // A clock saturation is not an immediate expiry. A later valid
                // clock/input/setting edge can rearm a representable deadline.
                deadline_ms: sample.now_ms.checked_add(duration_ms),
                fired: false,
            });
        }
        let armed = self.armed.as_mut()?;
        armed.last_ms = sample.now_ms;
        if !sample.defer_expiry && !armed.fired
            && armed.deadline_ms.is_some_and(|deadline| sample.now_ms >= deadline) {
            armed.fired = true;
            return Some(Fx::EnterStandby);
        }
        None
    }
}

#[cfg(test)]
#[path = "ott_sleep_timer_state_tests.rs"]
mod tests;
