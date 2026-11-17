//! Pure evidence of sustained progress from one admitted live playback owner.
//!
//! Loaded/Playing events are not samples. The host supplies the decoder's real
//! position only while the exact owner is playing, and supplies None when held,
//! replaced, stopped, loading or failed. Continuity adds its own intent epoch.
const HEALTHY_MS: u64 = 30_000;
const MAX_SAMPLE_GAP_MS: u64 = 2_500;
const MAX_STAGNATION_MS: u64 = 2_500;
const POSITION_TOLERANCE_MS: u64 = 500;

pub(super) struct Sample<'a, I> {
    pub generation: u64,
    pub play_generation: u64,
    pub selection_id: &'a str,
    pub identity: &'a I,
    pub position_ms: u64,
    pub now_ms: u64,
}

struct Observation<I> {
    epoch: u64,
    generation: u64,
    play_generation: u64,
    selection_id: String,
    identity: I,
    started_ms: u64,
    started_position_ms: u64,
    sampled_ms: u64,
    position_ms: u64,
    progressed_ms: u64,
}

pub(super) struct Tracker<I> {
    observed: Option<Observation<I>>,
}

impl<I> Default for Tracker<I> {
    fn default() -> Self { Self { observed: None } }
}

impl<I: Clone + Eq> Tracker<I> {
    /// A receipt is consumed immediately by Continuity; no asynchronous health
    /// callback may renew a later decoder's retry budget.
    pub(super) fn observe(&mut self, epoch: u64, sample: Option<Sample<'_, I>>) -> bool {
        let Some(sample) = sample.filter(|sample| !sample.selection_id.is_empty()) else {
            self.observed = None;
            return false;
        };
        let Some(old) = self.observed.as_mut().filter(|old|
            old.epoch == epoch && old.generation == sample.generation
                && old.play_generation == sample.play_generation
                && old.selection_id == sample.selection_id && old.identity == *sample.identity
                && sample.now_ms >= old.sampled_ms && sample.position_ms >= old.position_ms
                && sample.now_ms - old.sampled_ms <= MAX_SAMPLE_GAP_MS)
        else {
            self.begin(epoch, sample);
            return false;
        };
        if sample.position_ms == old.position_ms {
            if sample.now_ms - old.progressed_ms > MAX_STAGNATION_MS {
                self.begin(epoch, sample);
            } else {
                old.sampled_ms = sample.now_ms;
            }
            return false;
        }
        // A seek, injected position or clock discontinuity is not played time.
        // Quantized decoder callbacks may advance by more than one UI frame.
        if sample.now_ms == old.sampled_ms
            || sample.position_ms - old.position_ms >
                (sample.now_ms - old.progressed_ms).saturating_add(POSITION_TOLERANCE_MS)
        {
            self.begin(epoch, sample);
            return false;
        }
        old.sampled_ms = sample.now_ms;
        old.progressed_ms = sample.now_ms;
        old.position_ms = sample.position_ms;
        if sample.now_ms - old.started_ms >= HEALTHY_MS
            && sample.position_ms - old.started_position_ms >= HEALTHY_MS
        {
            old.started_ms = sample.now_ms;
            old.started_position_ms = sample.position_ms;
            return true;
        }
        false
    }

    fn begin(&mut self, epoch: u64, sample: Sample<'_, I>) {
        self.observed = Some(Observation {
            epoch, generation: sample.generation, play_generation: sample.play_generation,
            selection_id: sample.selection_id.into(), identity: sample.identity.clone(),
            started_ms: sample.now_ms, started_position_ms: sample.position_ms,
            sampled_ms: sample.now_ms, position_ms: sample.position_ms,
            progressed_ms: sample.now_ms,
        });
    }
}

/// Startup and deliberate holds are excluded by the caller. Once this
/// particular Load has made progress, watch both the position and callbacks.
/// A busy main loop, clock jump or replacement cannot count as observed silence.
#[derive(Clone, Copy)]
pub(super) struct PresentationSample {
    pub generation: u64,
    pub play_generation: u64,
    pub epoch: u32,
    pub now_ms: u64,
    pub position_ms: u64,
    pub callbacks: i32,
}

pub(super) struct PresentationWatch {
    previous: PresentationSample,
    first_callbacks: i32,
    first_position_ms: u64,
    activity_ms: u64,
}

impl PresentationWatch {
    pub(super) fn stopped(state: &mut Option<Self>, sample: Option<PresentationSample>) -> bool {
        let Some(s) = sample.filter(|s| s.callbacks > 0) else {
            *state = None;
            return false;
        };
        let Some(old) = state.as_mut().filter(|old| {
            let p = old.previous;
            p.generation == s.generation && p.play_generation == s.play_generation && p.epoch == s.epoch
                && s.now_ms.checked_sub(p.now_ms).is_some_and(|gap| gap <= MAX_SAMPLE_GAP_MS)
                && s.position_ms >= p.position_ms && s.callbacks >= p.callbacks
        }) else {
            *state = Some(Self { previous: s, first_callbacks: s.callbacks, first_position_ms: s.position_ms,
                activity_ms: s.now_ms });
            return false;
        };
        let before = old.previous;
        old.previous = s;
        if s.position_ms != before.position_ms || s.callbacks != before.callbacks {
            old.activity_ms = s.now_ms;
        }
        // At the measured 5 Hz position-callback cadence this requires around
        // 50 missed callbacks, as well as a playhead that never advanced.
        let stopped = s.position_ms > old.first_position_ms && s.callbacks > old.first_callbacks
            && s.now_ms - old.activity_ms >= 10_000;
        if stopped { *state = None; }
        stopped
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(now_ms: u64, position_ms: u64) -> Sample<'static, u64> {
        Sample { generation: 7, play_generation: 12, selection_id: "channel", identity: &19,
            position_ms, now_ms }
    }

    #[test]
    fn stopped_presentation_requires_observed_progress_and_ten_seconds_of_both_clocks_stopping() {
        for boundary in ["none", "callbacks", "position", "new load", "new request",
            "new catalog", "held", "late poll", "never advanced"] {
            let mut watch = None;
            let mut s = PresentationSample { generation: 1, play_generation: 3, epoch: 4,
                now_ms: 0, position_ms: 100, callbacks: 1 };
            assert!(!PresentationWatch::stopped(&mut watch, Some(s)));
            s.now_ms = 200;
            if boundary != "never advanced" { s.callbacks += 1; s.position_ms += 200; }
            for i in 0..=10 {
                if i == 5 {
                    match boundary {
                        "callbacks" => s.callbacks += 1,
                        "position" => s.position_ms += 200,
                        "new load" => s.epoch += 1,
                        "new request" => s.play_generation += 1,
                        "new catalog" => s.generation += 1,
                        "held" => { assert!(!PresentationWatch::stopped(&mut watch, None)); }
                        "late poll" => s.now_ms += MAX_SAMPLE_GAP_MS,
                        _ => {}
                    }
                }
                assert_eq!(PresentationWatch::stopped(&mut watch, Some(s)),
                    boundary == "none" && i == 10, "{boundary}, step {i}");
                s.now_ms += 1_000;
            }
        }
    }

    #[test]
    fn only_sustained_wall_and_real_position_progress_proves_health() {
        let mut tracker = Tracker::default();
        for second in 0..30 { assert!(!tracker.observe(1, Some(sample(second * 1_000, second * 1_000)))); }
        assert!(tracker.observe(1, Some(sample(30_000, 30_000))));
        assert!(!tracker.observe(1, Some(sample(30_000, 30_000))), "same sample is not new proof");
    }

    #[test]
    fn quantized_positions_between_ui_frames_can_still_prove_health() {
        let mut tracker = Tracker::default();
        let mut proved = false;
        for frame in 0..=1_800 {
            let now = frame * 20;
            proved |= tracker.observe(1, Some(sample(now, now / 500 * 500)));
        }
        assert!(proved);
    }

    #[test]
    fn brief_success_and_stagnant_positions_never_prove_health() {
        let mut tracker = Tracker::default();
        for second in 0..=10 { assert!(!tracker.observe(1, Some(sample(second * 1_000, second * 1_000)))); }
        for second in 11..=60 { assert!(!tracker.observe(1, Some(sample(second * 1_000, 10_000)))); }
    }

    #[test]
    fn interrupted_or_replaced_owners_cannot_lend_their_healthy_prefix() {
        for boundary in 0..8 {
            let mut tracker = Tracker::default();
            for second in 0..=29 { assert!(!tracker.observe(1, Some(sample(second * 1_000, second * 1_000)))); }
            let mut changed = sample(30_000, 30_000);
            let epoch = if boundary == 0 { 2 } else { 1 };
            match boundary {
                1 => changed.generation += 1,
                2 => changed.play_generation += 1,
                3 => changed.selection_id = "another channel",
                4 => changed.identity = &20,
                5 => { assert!(!tracker.observe(1, None)); }
                6 => changed.now_ms = 28_000,
                7 => changed.position_ms = 28_000,
                _ => {}
            }
            assert!(!tracker.observe(epoch, Some(changed)), "boundary {boundary}");
        }
    }

    #[test]
    fn missing_samples_and_forward_seeks_are_not_continuous_playback() {
        for seek in [false, true] {
            let mut tracker = Tracker::default();
            for second in 0..=29 { assert!(!tracker.observe(1, Some(sample(second * 1_000, second * 1_000)))); }
            let changed = if seek { sample(30_000, 60_000) } else { sample(40_000, 40_000) };
            assert!(!tracker.observe(1, Some(changed)));
        }
    }
}
