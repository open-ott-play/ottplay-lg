//! Accepted wall-clock samples for Classic paint. This is application-side
//! presentation work: it neither rebuilds the catalogue nor gives a screen IO.
use super::{App, Runtime, current_ui_owner};
use plx_machine::machine::{Delivery, Fx, InstanceId, MachineId, UiMode};
use plx_screens::registry::{AppMsg, OttClockSample};

#[derive(Default)]
pub(super) struct State {
    key: Option<(Option<i64>, usize)>,
    sample: OttClockSample,
    recipient: Option<InstanceId>,
}

impl State {
    fn observe(&mut self, recipient: Option<InstanceId>, seconds: Option<i64>,
        locale: &plx_platform::i18n::LocaleContext) -> Option<(InstanceId, OttClockSample)>
    {
        let Some(recipient) = recipient else { self.recipient = None; return None; };
        // Reuse the existing calendar/time_t range check, retaining second precision.
        let seconds = seconds.filter(|seconds| plx_platform::i18n::clock_minute_key(*seconds).is_some());
        let key = (seconds, locale as *const _ as usize);
        if self.key == Some(key) && self.recipient == Some(recipient) { return None; }
        let sample = if self.key != Some(key) {
            let label = seconds.and_then(plx_platform::i18n::local_clock_parts)
                .and_then(|parts| locale.time(parts));
            self.key = Some(key);
            OttClockSample { utc_s: seconds, label }
        } else {
            self.sample.clone()
        };
        self.prepare_delivery(recipient, sample)
    }

    fn prepare_delivery(&mut self, recipient: InstanceId, sample: OttClockSample)
        -> Option<(InstanceId, OttClockSample)>
    {
        // A repeated local clock face still represents a new UTC observation.
        let changed = sample != self.sample || self.recipient != Some(recipient);
        self.recipient = Some(recipient);
        if !changed { return None; }
        self.sample = sample.clone();
        Some((recipient, sample))
    }
}

pub(super) fn publish(runtime: &mut Runtime, app: &mut App) {
    // Live wall time is not on the controlled tape. Present bits are graded, so
    // suppress recording as well as replay rather than inventing a replay clock.
    if !matches!(&app.rec, super::super::recorder::Recplay::Off)
        || app.bridge.is_controlled_replay()
        || runtime.interface_mode() != UiMode::Classic
        || app.bridge.ui_mode() != UiMode::Classic
    {
        runtime.classic_clock.recipient = None;
        return;
    }
    let recipient = runtime.reply.filter(|reply| Some(*reply) == current_ui_owner(app));
    if recipient.is_none() {
        runtime.classic_clock.recipient = None;
        return;
    }
    let seconds = runtime.wall_now_s();
    if let Some((recipient, sample)) = runtime.classic_clock.observe(
        recipient, seconds, plx_platform::i18n::current())
    {
        app.pages.emit(MachineId::Nav, Fx::Deliver(MachineId::Instance(recipient),
            Delivery::Screen(plx_ui::screen::ScreenEvent::App(AppMsg::OttClock(sample)))));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use plx_screens::registry::OttClockSample;

    #[test]
    fn seconds_locale_and_recipient_are_independent_cache_inputs() {
        let a = plx_platform::i18n::LocaleContext::resolve(
            plx_platform::i18n::Preference::En, None, Some("en-US"), Some("24"), None);
        let b = plx_platform::i18n::LocaleContext::resolve(
            plx_platform::i18n::Preference::En, None, Some("en-US"), Some("12"), None);
        let mut state = State::default();
        let second = 1_730_000_001;
        let first = state.observe(Some(InstanceId(1)), Some(second), &a).unwrap();
        assert!(first.1.label.is_some());
        assert_eq!(first.1.utc_s, Some(second));
        assert_eq!(state.observe(Some(InstanceId(1)), Some(second), &a), None);
        let next = state.observe(Some(InstanceId(1)), Some(second + 1), &a).unwrap();
        assert_ne!(next.1.label, first.1.label, "seconds are visible, not rounded to minutes");
        assert_eq!(next.1.utc_s, Some(second + 1));
        let local = plx_platform::i18n::local_clock_parts(second + 1).unwrap();
        assert_eq!(state.observe(Some(InstanceId(1)), Some(second + 1), &b).unwrap().1.label, b.time(local));
        assert!(state.observe(Some(InstanceId(2)), Some(second + 1), &b).is_some());
        assert_eq!(state.observe(None, Some(second + 1), &b), None);
        assert!(state.observe(Some(InstanceId(2)), Some(second + 1), &b).is_some(),
            "uncover in the same second republishes to the current owner");
    }

    #[test]
    fn repeated_or_absent_labels_still_publish_accepted_utc() {
        let mut state = State::default();
        let owner = InstanceId(1);
        let first = OttClockSample { utc_s: Some(1_730_000_001), label: Some("16:12:20".into()) };
        assert_eq!(state.prepare_delivery(owner, first.clone()), Some((owner, first.clone())));
        assert_eq!(state.prepare_delivery(owner, first.clone()), None);
        // The delivery decision sees an already formatted sample. A repeated
        // clock face (another day or a wall correction) is not a repeated UTC.
        let next = OttClockSample { utc_s: Some(1_730_086_401), label: first.label.clone() };
        assert_eq!(state.prepare_delivery(owner, next.clone()), Some((owner, next)));
        // A local formatting failure does not erase the accepted UTC sample.
        let unlabelled = OttClockSample { utc_s: Some(1_730_086_402), label: None };
        assert_eq!(state.prepare_delivery(owner, unlabelled.clone()), Some((owner, unlabelled.clone())));
        assert_eq!(state.prepare_delivery(owner, unlabelled), None);
        let backwards = OttClockSample { utc_s: first.utc_s, label: None };
        assert_eq!(state.prepare_delivery(owner, backwards.clone()), Some((owner, backwards)));
        assert_eq!(state.prepare_delivery(owner, OttClockSample::default()),
            Some((owner, OttClockSample::default())));
        assert_eq!(state.prepare_delivery(owner, OttClockSample::default()), None);
    }

    #[test]
    fn missing_invalid_and_backwards_wall_samples_never_invent_time() {
        let locale = plx_platform::i18n::LocaleContext::resolve(
            plx_platform::i18n::Preference::En, None, Some("en-US"), Some("24"), None);
        let mut state = State::default();
        let owner = Some(InstanceId(1));
        assert_eq!(state.observe(owner, None, &locale), Some((InstanceId(1), OttClockSample::default())));
        assert_eq!(state.observe(owner, Some(0), &locale), None);
        assert_eq!(state.observe(owner, Some(i64::MAX), &locale), None);
        let first = state.observe(owner, Some(1_730_000_001), &locale).unwrap();
        let backwards = state.observe(owner, Some(1_730_000_000), &locale).unwrap();
        assert_ne!(first.1.label, backwards.1.label, "a wall correction is not an elapsed-time clock");
        assert_eq!(first.1.utc_s, Some(1_730_000_001));
        assert_eq!(backwards.1.utc_s, Some(1_730_000_000));
        assert_eq!(state.observe(owner, None, &locale), Some((InstanceId(1), OttClockSample::default())));
    }
}
