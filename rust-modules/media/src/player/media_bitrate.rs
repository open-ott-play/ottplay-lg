//! Measured main-media bitrate, independent of download speed or a declared rung.
//! One worker owns the last eight distinct completed segment observations.
use std::time::Duration;

const WINDOW: usize = 8;

#[derive(Clone, Copy)]
struct Sample {
    sequence: u64,
    bytes: u64,
    duration_ns: u128,
}

/// Source/worker-local meter. Construct a fresh value or reset it on a new
/// source/timeline; callers admit only completed main-media segments.
pub(crate) struct MediaBitrate {
    samples: [Option<Sample>; WINDOW],
}

impl Default for MediaBitrate {
    fn default() -> Self {
        Self {
            samples: [None; WINDOW],
        }
    }
}

impl MediaBitrate {
    pub(crate) fn reset(&mut self) {
        self.samples = [None; WINDOW];
    }

    /// Return decimal bits/second, weighted by media duration. A retry of a
    /// retained sequence replaces that observation; it never adds another row.
    /// Invalid observations and replay older than the retained window preserve
    /// the last known rate. No elapsed download time enters this calculation.
    pub(crate) fn observe(&mut self, sequence: u64, bytes: u64, duration: Duration) -> Option<u64> {
        let duration_ns = duration.as_nanos();
        if bytes == 0 || duration_ns == 0 {
            return self.rate();
        }
        let sample = Some(Sample {
            sequence,
            bytes,
            duration_ns,
        });
        if let Some(index) = self
            .samples
            .iter()
            .position(|row| row.is_some_and(|row| row.sequence == sequence))
        {
            self.samples[index] = sample;
            return self.rate();
        }
        let oldest = self
            .samples
            .iter()
            .enumerate()
            .filter_map(|(index, row)| row.map(|row| (index, row.sequence)))
            .min_by_key(|(_, sequence)| *sequence);
        if oldest.is_some_and(|(_, oldest)| sequence < oldest) {
            return self.rate();
        }
        if let Some(index) = self.samples.iter().position(Option::is_none) {
            self.samples[index] = sample;
        } else if let Some((index, _)) = oldest {
            self.samples[index] = sample;
        }
        self.rate()
    }

    fn rate(&self) -> Option<u64> {
        let (bytes, duration_ns) = self.samples.iter().flatten().try_fold(
            (0u128, 0u128),
            |(bytes, duration), sample| {
                Some((
                    bytes.checked_add(u128::from(sample.bytes))?,
                    duration.checked_add(sample.duration_ns)?,
                ))
            },
        )?;
        let bps = bytes
            .checked_mul(8)?
            .checked_mul(1_000_000_000)?
            .checked_div(duration_ns)?;
        let bps = bps.min(u128::from(u64::MAX)) as u64;
        (bps > 0).then_some(bps)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn media_rate_weights_bytes_by_duration_and_uses_decimal_bits() {
        let mut meter = MediaBitrate::default();
        assert_eq!(
            meter.observe(1, 1_000_000, Duration::from_secs(1)),
            Some(8_000_000)
        );
        assert_eq!(
            meter.observe(2, 1_000_000, Duration::from_secs(3)),
            Some(4_000_000)
        );
        // Arithmetic mean of the two segment rates would be wrong.
        assert_eq!(meter.samples.iter().flatten().count(), 2);
    }

    #[test]
    fn same_sequence_retry_replaces_without_double_counting() {
        let mut meter = MediaBitrate::default();
        meter.observe(1, 1_000_000, Duration::from_secs(1));
        meter.observe(2, 1_000_000, Duration::from_secs(3));
        assert_eq!(
            meter.observe(2, 3_000_000, Duration::from_secs(3)),
            Some(8_000_000)
        );
        assert_eq!(
            meter.observe(2, 3_000_000, Duration::from_secs(3)),
            Some(8_000_000)
        );
        assert_eq!(meter.samples.iter().flatten().count(), 2);
    }

    #[test]
    fn eighth_window_evicts_oldest_and_replayed_old_segments_do_not_evict() {
        let mut meter = MediaBitrate::default();
        for sequence in 1..=9 {
            meter.observe(sequence, sequence * 1_000, Duration::from_secs(1));
        }
        assert_eq!(meter.rate(), Some(44_000)); // sum(2..=9)*1000*8/8s
        assert_eq!(meter.samples.iter().flatten().count(), 8);
        assert_eq!(
            meter.observe(1, u64::MAX, Duration::from_nanos(1)),
            Some(44_000)
        );
        assert_eq!(
            meter.observe(0, u64::MAX, Duration::from_nanos(1)),
            Some(44_000)
        );
        assert_eq!(
            meter.samples.iter().flatten().map(|s| s.sequence).min(),
            Some(2)
        );
        assert_eq!(
            meter.samples.iter().flatten().map(|s| s.sequence).max(),
            Some(9)
        );
    }

    #[test]
    fn invalid_samples_do_not_poison_last_rate_and_reset_allows_new_timeline() {
        let mut meter = MediaBitrate::default();
        assert_eq!(meter.observe(1, 0, Duration::from_secs(1)), None);
        assert_eq!(meter.observe(1, 1, Duration::ZERO), None);
        assert_eq!(
            meter.observe(100, 1_000_000, Duration::from_secs(1)),
            Some(8_000_000)
        );
        assert_eq!(
            meter.observe(u64::MAX, 0, Duration::from_secs(1)),
            Some(8_000_000)
        );
        assert_eq!(meter.observe(u64::MAX, 1, Duration::ZERO), Some(8_000_000));
        assert_eq!(
            meter.observe(101, 1_000_000, Duration::from_secs(1)),
            Some(8_000_000)
        );
        assert_eq!(meter.samples.iter().flatten().count(), 2);
        meter.reset();
        assert_eq!(meter.rate(), None);
        assert_eq!(
            meter.observe(1, 1_000_000, Duration::from_secs(2)),
            Some(4_000_000)
        );
    }

    #[test]
    fn extreme_metadata_is_bounded_without_an_arbitrary_size_ceiling() {
        let mut meter = MediaBitrate::default();
        for sequence in 0..8 {
            assert_eq!(
                meter.observe(sequence, u64::MAX, Duration::from_nanos(1)),
                Some(u64::MAX)
            );
        }
        assert_eq!(meter.samples.iter().flatten().count(), 8);
        meter.reset();
        assert_eq!(meter.observe(1, u64::MAX, Duration::MAX), Some(7));
        meter.reset();
        assert_eq!(meter.observe(1, 1, Duration::MAX), None); // sub-bit/s rounds to unknown zero
    }
}
