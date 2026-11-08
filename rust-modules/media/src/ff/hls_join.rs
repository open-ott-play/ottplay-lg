//! Join separately fetched, validated VOD lanes on an explicit common PDT clock.
//! Sequence numbers never take part in alignment. This module owns no transport
//! or player globals; the caller keeps its seek/generation and rendition receipt.
use super::*;
use super::hls_rendition::{Binding, Lane, TimedLaneSegment};
#[cfg(test)]
use super::hls_rendition::{SourceTiming, TimedSample};
use std::collections::VecDeque;

const JOIN_BYTES: usize = 32 * 1024 * 1024;
const JOIN_SAMPLES: usize = 32_768;
// TS ticks and decimal EXTINF/PDT rounding may differ slightly. Do not rewrite
// timestamps to hide a gap; only tolerate a sub-millisecond boundary discrepancy.
const BOUNDARY_EPSILON_NS: i64 = 1_000_000;

struct MappedSample {
    au: HlsAu,
    dts_ns: i64,
    end_ns: i64,
}
pub(super) struct MappedSegment {
    binding: Binding,
    samples: Vec<MappedSample>,
    pub(super) start_ns: i64,
    pub(super) end_ns: i64,
    bytes: usize,
    transfer: SegmentTransfer,
}
fn rescale(ticks: i128, tb: (i32, i32)) -> Result<i64, &'static str> {
    if tb.0 <= 0 || tb.1 <= 0 { return Err("Invalid HLS presentation time base"); }
    let ns = ticks.checked_mul(i128::from(tb.0)).and_then(|v| v.checked_mul(1_000_000_000))
        .ok_or("HLS presentation time overflow")? / i128::from(tb.1);
    i64::try_from(ns).map_err(|_| "HLS presentation time overflow")
}
#[derive(Clone, Copy)]
struct ClockAnchor {
    pts: i64,
    time_base: (i32, i32),
    presentation_ns: i64,
    precision_ns: i64,
    lane: Lane,
}
/// Bind each encoder clock once per playback worker, retaining it across seeks. Inherited PDT cannot
/// conceal a raw PTS gap, duplicate segment, or unannounced timestamp reset.
#[derive(Clone, Default)]
pub(super) struct LaneClock {
    anchor: Option<ClockAnchor>,
    previous_end: Option<i64>,
}
impl LaneClock {
    pub(super) fn is_anchored(&self) -> bool { self.anchor.is_some() }
    pub(super) fn webvtt_clock(&self) -> Option<super::hls_webvtt::Clock> {
        let anchor = self.anchor?;
        (anchor.lane == Lane::Video && anchor.time_base == (1, 90_000)).then_some(super::hls_webvtt::Clock {
            pts_90k: anchor.pts, presentation_ns: anchor.presentation_ns,
        })
    }
    pub(super) fn restart(&mut self) { self.previous_end = None; }
    pub(super) fn map(&mut self, raw: TimedLaneSegment, pdt_ns: i64, origin_ns: i64,
        explicit_precision_ns: Option<i64>)
        -> Result<MappedSegment, &'static str>
    {
        let expected_start = pdt_ns.checked_sub(origin_ns).ok_or("HLS presentation time overflow")?;
        let first = raw.samples.first().ok_or("Empty HLS rendition")?;
        let tb = first.timing.time_base;
        if raw.samples.iter().any(|s| s.timing.time_base != tb) {
            return Err("HLS rendition time base changed within segment");
        }
        // PDT names the first presentation sample, not the first packet's DTS.
        // Strict video-only/audio-only input removes the hidden-other-lane ambiguity.
        let first_pts = raw.samples.iter().map(|s| s.timing.pts).min().unwrap();
        let lane = raw.binding.lane();
        if self.anchor.is_none() && explicit_precision_ns.is_none() {
            return Err("HLS clock requires an explicit initial anchor");
        }
        let anchor = self.anchor.unwrap_or(ClockAnchor {
            pts: first_pts, time_base: tb, presentation_ns: expected_start,
            precision_ns: explicit_precision_ns.unwrap_or(0), lane,
        });
        if anchor.time_base != tb || anchor.lane != lane {
            return Err("HLS rendition clock changed; reopen the source");
        }
        let map_time = |ticks: i64| -> Result<i64, &'static str> {
            anchor.presentation_ns.checked_add(rescale(i128::from(ticks) - i128::from(anchor.pts), tb)?)
                .ok_or("HLS presentation time overflow")
        };
        let start_ns = map_time(first_pts)?;
        // EXTINF-derived dates are navigation estimates, not clock evidence.
        // Compare only explicit PDT. Both the initial anchor and the later tag
        // contribute their decimal resolution; a coarse initial timestamp must
        // not become falsely exact when a later tag uses more decimal places.
        if explicit_precision_ns.is_some_and(|precision| start_ns.abs_diff(expected_start)
            > precision.saturating_add(anchor.precision_ns).max(BOUNDARY_EPSILON_NS) as u64)
        {
            return Err("HLS rendition timestamps disagree with explicit program date time");
        }
        if self.previous_end.is_some_and(|end| start_ns.abs_diff(end) > BOUNDARY_EPSILON_NS as u64) {
            return Err("HLS rendition raw timestamp gap or overlap");
        }
        let mut samples = Vec::with_capacity(raw.samples.len());
        let mut bytes = 0usize;
        let mut end_ns = start_ns;
        for sample in raw.samples {
            let timing = sample.timing;
            let pts = map_time(timing.pts)?;
            if pts < 0 { return Err("HLS rendition starts before presentation origin"); }
            let dts = map_time(timing.dts.unwrap_or(timing.pts))?;
            let duration = timing.duration.filter(|d| *d > 0).ok_or("HLS rendition has no sample duration")?;
            let end = pts.checked_add(rescale(i128::from(duration), tb)?)
                .filter(|end| *end > pts).ok_or("Invalid HLS rendition sample duration")?;
            bytes = bytes.checked_add(sample.data.len()).filter(|n| *n <= JOIN_BYTES)
                .ok_or("HLS joined buffer exceeds limit")?;
            end_ns = end_ns.max(end);
            samples.push(MappedSample { au: HlsAu { data: sample.data, pts_ns: pts,
                key: i32::from(sample.key), es: if lane == Lane::Video { 1 } else { 2 } }, dts_ns: dts, end_ns: end });
        }
        if lane == Lane::Audio && samples.windows(2).any(|w| w[1].au.pts_ns < w[0].au.pts_ns
            || w[1].au.pts_ns.abs_diff(w[0].end_ns) > BOUNDARY_EPSILON_NS as u64)
        {
            return Err("HLS audio contains a timestamp gap or overlap");
        }
        self.anchor = Some(anchor);
        self.previous_end = Some(end_ns);
        Ok(MappedSegment { binding: raw.binding, samples, start_ns, end_ns, bytes, transfer: raw.transfer })
    }
}

pub(super) struct JoinCursor {
    audio: VecDeque<MappedSample>,
    audio_binding: Option<Binding>,
    loaded_end: Option<i64>,
    emitted_end: i64,
    next_video_start: i64,
    bytes: usize,
    pending_transfer: SegmentTransfer,
}
impl JoinCursor {
    pub(super) fn new(start_ns: i64) -> Result<Self, &'static str> {
        if start_ns < 0 { return Err("Invalid HLS join origin"); }
        Ok(Self { audio: VecDeque::new(), audio_binding: None, loaded_end: None,
            emitted_end: start_ns, next_video_start: start_ns, bytes: 0,
            pending_transfer: SegmentTransfer { bytes: 0, active_us: 0, total_us: 0, audio_expected: true } })
    }
    pub(super) fn push_audio(&mut self, segment: MappedSegment) -> Result<(), &'static str> {
        if segment.binding.lane() != Lane::Audio { return Err("Expected HLS audio rendition"); }
        if self.audio_binding.as_ref().is_some_and(|binding| *binding != segment.binding) {
            return Err("Selected HLS audio changed between segments");
        }
        if let Some(end) = self.loaded_end {
            if segment.start_ns.abs_diff(end) > BOUNDARY_EPSILON_NS as u64 {
                return Err("HLS audio segment boundary has a gap or overlap");
            }
        } else if segment.start_ns > self.next_video_start.saturating_add(BOUNDARY_EPSILON_NS)
            || segment.end_ns <= self.next_video_start {
            return Err("HLS audio does not cover initial video time");
        }
        let bytes = self.bytes.checked_add(segment.bytes).filter(|n| *n <= JOIN_BYTES)
            .ok_or("HLS joined buffer exceeds limit")?;
        if self.audio.len() + segment.samples.len() > JOIN_SAMPLES { return Err("HLS joined sample limit exceeded"); }
        self.pending_transfer.bytes = self.pending_transfer.bytes.saturating_add(segment.transfer.bytes);
        self.pending_transfer.active_us = self.pending_transfer.active_us.saturating_add(segment.transfer.active_us);
        self.pending_transfer.total_us = self.pending_transfer.total_us.saturating_add(segment.transfer.total_us);
        self.audio_binding = Some(segment.binding);
        self.loaded_end = Some(segment.end_ns);
        self.bytes = bytes;
        for sample in segment.samples {
            if sample.end_ns <= self.next_video_start {
                self.bytes -= sample.au.data.len();
            } else { self.audio.push_back(sample); }
        }
        Ok(())
    }
    pub(super) fn covers(&self, video: &MappedSegment) -> bool {
        self.loaded_end.is_some_and(|end| end.saturating_add(BOUNDARY_EPSILON_NS) >= video.end_ns)
    }
    pub(super) fn join(&mut self, video: MappedSegment) -> Result<HlsSegmentOutput, &'static str> {
        let Binding::Video { width, height } = video.binding else { return Err("Expected HLS video rendition"); };
        if video.start_ns.abs_diff(self.next_video_start) > BOUNDARY_EPSILON_NS as u64 {
            return Err("HLS video segment boundary has a gap or overlap");
        }
        if !self.covers(&video) { return Err("Selected HLS audio does not cover video interval"); }
        if self.audio.len() + video.samples.len() > JOIN_SAMPLES {
            return Err("HLS joined sample limit exceeded");
        }
        if self.bytes.checked_add(video.bytes).is_none_or(|n| n > JOIN_BYTES) {
            return Err("HLS joined buffer exceeds limit");
        }
        let mut audio = Vec::new();
        while self.audio.front().is_some_and(|s| s.au.pts_ns < video.end_ns) {
            let sample = self.audio.pop_front().unwrap();
            self.bytes -= sample.au.data.len();
            self.emitted_end = self.emitted_end.max(sample.end_ns);
            audio.push(sample);
        }
        let mut audio = audio.into_iter().peekable();
        let mut frames = video.samples.into_iter().peekable();
        let mut aus = Vec::new();
        // Merge two decode-order queues. Never sort a lane by presentation time.
        while audio.peek().is_some() || frames.peek().is_some() {
            let take_video = match (frames.peek(), audio.peek()) {
                (Some(v), Some(a)) => v.dts_ns <= a.dts_ns,
                (Some(_), None) => true,
                _ => false,
            };
            aus.push(if take_video { frames.next().unwrap().au } else { audio.next().unwrap().au });
        }
        self.next_video_start = video.end_ns;
        let transfer = SegmentTransfer {
            bytes: video.transfer.bytes.saturating_add(self.pending_transfer.bytes),
            active_us: video.transfer.active_us.saturating_add(self.pending_transfer.active_us),
            total_us: video.transfer.total_us.saturating_add(self.pending_transfer.total_us), audio_expected: true,
        };
        self.pending_transfer = SegmentTransfer { bytes: 0, active_us: 0, total_us: 0, audio_expected: true };
        Ok(HlsSegmentOutput { aus, transfer, video_width: width, video_height: height,
            video_tail_ns: video.end_ns, audio_tail_ns: Some(self.emitted_end) })
    }
}

#[cfg(test)]
#[path = "hls_join_tests.rs"]
mod tests;
