//! Stereo 48 kHz output for independent multiview timelines. Slot PTS values
//! refer to different media; the common clock is the consuming SDL device.
use crate::local_multiview::PcmChunk;
use std::collections::VecDeque;

const STEREO: usize = 2;
const SAMPLE_RATE: usize = 48_000;
const BLOCK_FRAMES: usize = SAMPLE_RATE / 100;
const MAX_LANE_FRAMES: usize = SAMPLE_RATE * 120 / 1000;
const MAX_AHEAD_FRAMES: usize = SAMPLE_RATE * 60 / 1000;

#[derive(Default)]
struct Lane {
    samples: VecDeque<i16>,
    end_us: Option<i64>,
    gain: f32,
}

#[derive(Default)]
pub struct Mixer {
    lanes: [Lane; 9],
}

impl Mixer {
    pub fn clear(&mut self) {
        for index in 0..self.lanes.len() {
            self.clear_slot(index);
        }
    }

    pub fn clear_slot(&mut self, index: usize) {
        if let Some(lane) = self.lanes.get_mut(index) {
            lane.samples.clear();
            lane.end_us = None;
            lane.gain = 0.0;
        }
    }

    pub fn push(&mut self, index: usize, chunk: PcmChunk) {
        let Some(lane) = self.lanes.get_mut(index) else {
            return;
        };
        let samples = chunk.samples;
        let frames = samples.len() / STEREO;
        if frames == 0 || !samples.len().is_multiple_of(STEREO) {
            return;
        }
        if lane
            .end_us
            .is_some_and(|pts| pts.abs_diff(chunk.pts_us) > 100_000)
        {
            lane.samples.clear();
        }
        let keep = frames.min(MAX_LANE_FRAMES) * STEREO;
        let drop = (lane.samples.len() + keep).saturating_sub(MAX_LANE_FRAMES * STEREO);
        lane.samples.drain(..drop);
        lane.samples.extend(&samples[samples.len() - keep..]);
        lane.end_us = Some(
            chunk
                .pts_us
                .saturating_add((frames as i64) * 1_000_000 / SAMPLE_RATE as i64),
        );
    }

    /// Never wait for a silent, slow or paused decoder. Backpressure comes
    /// from the device queue, so several independent readers cannot each
    /// advance the output clock by the duration of their own packets.
    pub fn pull(
        &mut self,
        focus: usize,
        background_percent: u8,
        queued_frames: usize,
    ) -> Option<PcmChunk> {
        let available = self.lanes.iter().map(|l| l.samples.len() / STEREO).max()?;
        let frames = available
            .min(BLOCK_FRAMES)
            .min(MAX_AHEAD_FRAMES.saturating_sub(queued_frames));
        if frames == 0 || focus >= self.lanes.len() {
            return None;
        }
        let pts_us = self
            .lanes
            .iter()
            .find(|l| !l.samples.is_empty())
            .and_then(|l| {
                l.end_us.map(|pts| {
                    pts.saturating_sub(
                        (l.samples.len() / STEREO) as i64 * 1_000_000 / SAMPLE_RATE as i64,
                    )
                })
            })
            .unwrap_or(0);
        let mut foreground = [[0.0f32; STEREO]; BLOCK_FRAMES];
        let mut background = [[0.0f32; STEREO]; BLOCK_FRAMES];
        for (index, lane) in self.lanes.iter_mut().enumerate() {
            let (target, out) = if index == focus {
                (1.0, &mut foreground)
            } else {
                (
                    f32::from(background_percent.min(100)) / 100.0,
                    &mut background,
                )
            };
            let count = frames.min(lane.samples.len() / STEREO);
            for stereo in &mut out[..count] {
                // Each lane reaches its new gain within 10 ms after a focus change.
                lane.gain += (target - lane.gain)
                    .clamp(-1.0 / BLOCK_FRAMES as f32, 1.0 / BLOCK_FRAMES as f32);
                for value in stereo {
                    *value += f32::from(lane.samples.pop_front().unwrap()) * lane.gain;
                }
            }
        }
        let mut samples = Vec::with_capacity(frames * STEREO);
        for (front, rear) in foreground[..frames].iter().zip(&background) {
            // Preserve the main track at unity. Scale the summed background
            // only as necessary to fit the remaining headroom in both channels.
            let mut gain = 1.0f32;
            for (&f, &b) in front.iter().zip(rear) {
                if b > 0.0 {
                    gain = gain.min((32767.0 - f) / b);
                } else if b < 0.0 {
                    gain = gain.min((-32768.0 - f) / b);
                }
            }
            for (&f, &b) in front.iter().zip(rear) {
                samples.push((f + b * gain).round() as i16);
            }
        }
        Some(PcmChunk { samples, pts_us })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chunk(value: i16, frames: usize, pts_us: i64) -> PcmChunk {
        PcmChunk {
            samples: vec![value; frames * STEREO],
            pts_us,
        }
    }

    #[test]
    fn independent_timestamps_share_one_clock_and_focus_inverts_the_mix() {
        let mut mix = Mixer::default();
        mix.push(0, chunk(12000, 2 * BLOCK_FRAMES, 100_000));
        mix.push(1, chunk(-5000, 2 * BLOCK_FRAMES, 8_000_000));
        assert_eq!(mix.pull(0, 20, 0).unwrap().samples.last(), Some(&11000));
        assert_eq!(
            mix.pull(1, 20, BLOCK_FRAMES).unwrap().samples.last(),
            Some(&-2600)
        );
        assert!(mix.pull(1, 20, 0).is_none());
    }

    #[test]
    fn output_preserves_foreground_without_wrap_or_a_missing_input_stall() {
        let mut mix = Mixer::default();
        for slot in 0..9 {
            mix.push(slot, chunk(30_000, BLOCK_FRAMES, 0));
        }
        assert_eq!(mix.pull(0, 50, 0).unwrap().samples.last(), Some(&32767));
        mix.clear_slot(0);
        mix.push(2, chunk(12_000, BLOCK_FRAMES, 10_000));
        assert_eq!(mix.pull(0, 25, 0).unwrap().samples.last(), Some(&3000));
    }

    #[test]
    fn overflow_backpressure_discontinuities_and_revocation_bound_latency() {
        let mut mix = Mixer::default();
        mix.push(3, chunk(50, 3 * MAX_LANE_FRAMES, 1_000_000));
        assert_eq!(mix.lanes[3].samples.len(), MAX_LANE_FRAMES * STEREO);
        assert!(mix.pull(3, 20, MAX_AHEAD_FRAMES).is_none());
        mix.push(3, chunk(100, 100, 9_000_000));
        assert_eq!(mix.lanes[3].samples.len(), 200);
        assert_eq!(
            mix.pull(3, 20, MAX_AHEAD_FRAMES - 1).unwrap().samples.len(),
            STEREO
        );
        mix.clear();
        assert!(mix.pull(3, 20, 0).is_none());
    }
}
