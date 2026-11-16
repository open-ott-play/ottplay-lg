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

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(now_ms: u64, position_ms: u64) -> Sample<'static, u64> {
        Sample { generation: 7, play_generation: 12, selection_id: "channel", identity: &19,
            position_ms, now_ms }
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
