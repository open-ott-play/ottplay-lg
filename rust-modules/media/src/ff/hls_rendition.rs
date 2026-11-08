//! Strict single-lane transport used by the session-bound HLS VOD joiner.
//! Raw packet clocks stay intact; only the joiner may produce decoder output.
use super::*;

const BODY_LIMIT: u64 = 8 * 1024 * 1024;
const SAMPLE_LIMIT: usize = 16_384;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Lane { Video, Audio }

/// Exact single-container facts; this is not a master-rendition identity. In
/// particular, a container's missing language tag cannot prove a requested
/// language. The presentation scheduler additionally binds this to its rendition.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Binding {
    Video { width: i32, height: i32 },
    Audio(inspect::InspectedTrack),
}
impl Binding {
    pub(super) fn lane(&self) -> Lane {
        match self { Self::Video { .. } => Lane::Video, Self::Audio(_) => Lane::Audio }
    }

    pub(super) unsafe fn inspect(lane: Lane, context: *mut AVFormatContext) -> Result<Self, &'static str> {
        if context.is_null() || (*context).nb_streams != 1 || (*context).streams.is_null() {
            return Err("HLS rendition requires exactly one elementary stream");
        }
        let stream = *(*context).streams;
        if stream.is_null() || stream_index(stream) != 0 {
            return Err("Invalid HLS rendition stream identity");
        }
        let codec = stream_codecpar(stream);
        if codec.is_null() { return Err("Invalid HLS rendition codec"); }
        match lane {
            Lane::Video if (*codec).codec_type == AVMEDIA_TYPE_VIDEO
                && (*codec).codec_id == AV_CODEC_ID_H264
                && (*codec).width > 0 && (*codec).height > 0 =>
            {
                Ok(Self::Video { width: (*codec).width, height: (*codec).height })
            }
            Lane::Audio if (*codec).codec_type == AVMEDIA_TYPE_AUDIO
                && (*codec).codec_id == AV_CODEC_ID_AAC =>
            {
                let inventory = inspect::collect_tracks(context)?;
                let track = inventory.audio.into_iter().next()
                    .ok_or("HLS rendition audio is missing")?;
                if track.channels <= 0 || track.sample_rate <= 0 {
                    return Err("HLS rendition requires a known AAC format");
                }
                Ok(Self::Audio(track))
            }
            _ => Err("HLS rendition must be video-only H.264 or audio-only AAC"),
        }
    }
}

/// The demuxer's original ticks and rational time base, before SegmentClock's
/// independent zeroing, clamping and millisecond rounding. Preserve packet/decode
/// order: B-frame PTS values are allowed to move backwards. These values alone
/// do not establish a common epoch between independent encoders/playlists.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct SourceTiming {
    pub(super) pts: i64,
    pub(super) dts: Option<i64>,
    pub(super) duration: Option<i64>,
    pub(super) time_base: (i32, i32),
}
impl SourceTiming {
    fn new(pts: Option<i64>, dts: Option<i64>, duration: Option<i64>, num: i32, den: i32)
        -> Result<Self, &'static str>
    {
        if num <= 0 || den <= 0 { return Err("Invalid HLS rendition time base"); }
        Ok(Self {
            pts: pts.ok_or("HLS rendition packet has no presentation timestamp")?,
            dts, duration: duration.filter(|value| *value > 0), time_base: (num, den),
        })
    }
    unsafe fn packet(packet: *const AVPacket, stream: *mut AVStream) -> Result<Self, &'static str> {
        let tb = stream_time_base(stream);
        Self::new(
            ((*packet).pts != AV_NOPTS_VALUE).then_some((*packet).pts),
            ((*packet).dts != AV_NOPTS_VALUE).then_some((*packet).dts),
            ((*packet).duration > 0).then_some((*packet).duration), tb.num, tb.den,
        )
    }
}

/// Keep absent AAC clocks private until a real presentation anchor is available.
/// The packet's original fields are independent of the exact ADTS core duration:
/// FFmpeg can supply floor-rounded durations while advancing PTS rationally.
#[derive(Clone, Copy)]
struct AudioPacketTiming {
    pts: Option<i64>,
    dts: Option<i64>,
    duration: Option<i64>,
    time_base: (i32, i32),
    adts: Option<adts_timing::Duration>,
}
impl AudioPacketTiming {
    fn new(pts: Option<i64>, dts: Option<i64>, duration: Option<i64>,
        time_base: (i32, i32), data: &[u8]) -> Self
    {
        Self { pts, dts, duration: duration.filter(|value| *value > 0), time_base,
            adts: adts_timing::duration(data).and_then(adts_timing::Duration::for_rendition) }
    }
    unsafe fn packet(packet: *const AVPacket, stream: *mut AVStream) -> Self {
        let tb = stream_time_base(stream);
        let data = if (*packet).data.is_null() || (*packet).size <= 0 { &[] } else {
            std::slice::from_raw_parts((*packet).data, (*packet).size as usize)
        };
        Self::new(
            ((*packet).pts != AV_NOPTS_VALUE).then_some((*packet).pts),
            ((*packet).dts != AV_NOPTS_VALUE).then_some((*packet).dts),
            ((*packet).duration > 0).then_some((*packet).duration),
            (tb.num, tb.den), data,
        )
    }
}

/// A common exact denominator for one segment, with no floating point or ns
/// conversion. Only qualified ADTS supplies a fractional step; other positive
/// packet durations remain integer evidence. Format changes fail closed.
struct AudioPhase {
    denominator: i128,
    time_base: (i32, i32),
}
impl AudioPhase {
    fn new(packets: &[AudioPacketTiming], time_base: (i32, i32)) -> Result<Self, &'static str> {
        let adts = packets.iter().find_map(|packet| packet.adts);
        if packets.iter().filter_map(|packet| packet.adts)
            .any(|value| adts.is_some_and(|first| first.config != value.config))
        { return Err("HLS rendition ADTS format changed within the segment"); }
        let denominator = match adts {
            Some(value) => i128::from(value.rate).checked_mul(i128::from(time_base.0))
                .ok_or("HLS rendition audio timestamp overflow")?,
            None => 1,
        };
        Ok(Self { denominator, time_base })
    }
    fn step(&self, packet: &AudioPacketTiming) -> Result<Option<i128>, &'static str> {
        if let Some(value) = packet.adts {
            let numerator = i128::from(value.samples).checked_mul(i128::from(self.time_base.1))
                .ok_or("HLS rendition audio timestamp overflow")?;
            if let Some(duration) = packet.duration.filter(|value| *value > 0) {
                let floor = numerator / self.denominator;
                let ceil = floor + if numerator % self.denominator != 0 { 1 } else { 0 };
                if i128::from(duration) < floor || i128::from(duration) > ceil {
                    return Err("HLS rendition packet duration conflicts with ADTS");
                }
            }
            return Ok(Some(numerator));
        }
        packet.duration.filter(|value| *value > 0).map(|duration| {
            i128::from(duration).checked_mul(self.denominator)
                .ok_or("HLS rendition audio timestamp overflow")
        }).transpose()
    }
    // Distances are nonnegative; reverse inference subtracts this rounded
    // distance. Thus ties are away from the real anchor in either direction.
    fn round(&self, distance: i128) -> Result<i128, &'static str> {
        distance.checked_add(self.denominator / 2).map(|value| value / self.denominator)
            .ok_or("HLS rendition audio timestamp overflow")
    }
    fn point(&self, anchor: i64, distance: i128, reverse: bool) -> Result<i64, &'static str> {
        let rounded = self.round(distance)?;
        let value = if reverse { i128::from(anchor).checked_sub(rounded) }
            else { i128::from(anchor).checked_add(rounded) };
        value.and_then(|value| i64::try_from(value).ok())
            .ok_or("HLS rendition audio timestamp overflow")
    }
    fn span(&self, begin: i128, end: i128) -> Result<i64, &'static str> {
        let duration = self.round(end)?.checked_sub(self.round(begin)?)
            .and_then(|value| i64::try_from(value).ok()).filter(|value| *value > 0);
        duration.ok_or("Invalid HLS rendition audio duration")
    }
}

fn resolve_audio_timing(packets: &[AudioPacketTiming]) -> Result<Vec<SourceTiming>, &'static str> {
    if packets.is_empty() || packets.len() > SAMPLE_LIMIT {
        return Err("Invalid HLS rendition audio timing count");
    }
    let time_base = packets[0].time_base;
    if time_base.0 <= 0 || time_base.1 <= 0
        || packets.iter().any(|packet| packet.time_base != time_base)
    {
        return Err("Invalid or changed HLS rendition audio time base");
    }
    let anchor = packets.iter().position(|packet| packet.pts.is_some())
        .ok_or("HLS rendition audio has no presentation anchor")?;
    let clock = AudioPhase::new(packets, time_base)?;
    let mut pts: Vec<_> = packets.iter().map(|packet| packet.pts).collect();
    let mut durations: Vec<_> = packets.iter().map(|packet| packet.duration).collect();
    let first = pts[anchor].ok_or("Missing HLS audio anchor")?;
    let mut distance = 0i128;
    for index in (0..anchor).rev() {
        let step = clock.step(&packets[index])?
            .ok_or("HLS rendition audio timestamp hole has no duration")?;
        let next = distance.checked_add(step).ok_or("HLS rendition audio timestamp overflow")?;
        pts[index] = Some(clock.point(first, next, true)?);
        if durations[index].is_none() { durations[index] = Some(clock.span(distance, next)?); }
        distance = next;
    }
    let mut origin = first;
    let mut phase = Some(0i128);
    for index in anchor..packets.len() {
        if let Some(explicit) = packets[index].pts {
            origin = explicit;
            phase = Some(0);
        }
        let begin = phase.ok_or("HLS rendition audio timestamp hole has no duration")?;
        pts[index] = Some(clock.point(origin, begin, false)?);
        phase = match clock.step(&packets[index])? {
            Some(step) => {
                let end = begin.checked_add(step).ok_or("HLS rendition audio timestamp overflow")?;
                if durations[index].is_none() {
                    // Validate the endpoint too, including the final packet.
                    clock.point(origin, end, false)?;
                    durations[index] = Some(clock.span(begin, end)?);
                }
                Some(end)
            }
            None => None,
        };
        // A later explicit PTS resets phase, but never rewrites this run's end.
        // Existing rendition continuity checks retain every real gap or overlap.
    }
    packets.iter().zip(pts).zip(durations).map(|((packet, pts), duration)| {
        // Qualified LC-core ADTS is non-delayed audio (FFmpeg demux.c). Only
        // absent DTS is filled, and only after recovering PTS from a real anchor.
        let dts = packet.dts.or_else(|| packet.adts.and(pts));
        SourceTiming::new(pts, dts, duration, time_base.0, time_base.1)
    }).collect()
}

pub(super) struct TimedSample {
    pub(super) data: Vec<u8>,
    pub(super) key: bool,
    pub(super) timing: SourceTiming,
}
pub(super) struct TimedLaneSegment {
    pub(super) binding: Binding,
    pub(super) samples: Vec<TimedSample>,
    pub(super) transfer: SegmentTransfer,
}

/// Borrowed by the shared segment reader. All data remains private until a full
/// body and the final stream table have both passed validation.
pub(super) struct Capture {
    pub(super) lane: Lane,
    expected: Option<Binding>,
    binding: Option<Binding>,
    timing: Vec<SourceTiming>,
    audio_timing: Vec<AudioPacketTiming>,
}
impl Capture {
    fn new(lane: Lane, expected: Option<&Binding>) -> Self {
        Self { lane, expected: expected.cloned(), binding: None, timing: Vec::new(), audio_timing: Vec::new() }
    }
    pub(super) unsafe fn begin(&mut self, context: *mut AVFormatContext) -> Result<(), &'static str> {
        self.accept_probe(Binding::inspect(self.lane, context)?)
    }
    fn accept_probe(&mut self, binding: Binding) -> Result<(), &'static str> {
        if self.expected.as_ref().is_some_and(|expected| *expected != binding) {
            return Err("HLS rendition descriptor changed; reopen the source");
        }
        self.binding = Some(binding);
        Ok(())
    }
    pub(super) unsafe fn finish(&mut self, context: *mut AVFormatContext) -> Result<(), &'static str> {
        let binding = Binding::inspect(self.lane, context)?;
        if self.binding.as_ref() != Some(&binding) {
            return Err("HLS rendition descriptor changed within the segment");
        }
        if self.lane == Lane::Audio {
            self.timing = resolve_audio_timing(&self.audio_timing)?;
            self.audio_timing.clear();
        }
        self.validate_end(binding)
    }
    fn validate_end(&self, binding: Binding) -> Result<(), &'static str> {
        if self.binding.as_ref() != Some(&binding) {
            return Err("HLS rendition descriptor changed within the segment");
        }
        if self.timing.is_empty() { return Err("HLS rendition produced no access units"); }
        Ok(())
    }
    fn push(&mut self, timing: SourceTiming) -> Result<(), &'static str> {
        if self.timing.len() >= SAMPLE_LIMIT { return Err("HLS rendition exceeds sample limit"); }
        self.timing.push(timing);
        Ok(())
    }
    fn push_audio(&mut self, timing: AudioPacketTiming) -> Result<(), &'static str> {
        if self.audio_timing.len() >= SAMPLE_LIMIT { return Err("HLS rendition exceeds sample limit"); }
        self.audio_timing.push(timing);
        Ok(())
    }
    pub(super) unsafe fn packet(&mut self, packet: *const AVPacket, stream: *mut AVStream)
        -> Result<(), &'static str>
    {
        if self.lane == Lane::Audio {
            self.push_audio(AudioPacketTiming::packet(packet, stream))
        } else {
            self.push(SourceTiming::packet(packet, stream)?)
        }
    }
}

/// Uses the same owned HTTP/Curl, acquisition deadline, cancellation and full-body
/// validation as normal HLS. The first result is also its audio-only inspection;
/// subsequent calls can require its exact container binding. No FFmpeg network
/// protocol or unconstrained URL reaches the demuxer.
///
/// The caller must own this playback worker's queue/net for the whole operation.
/// A result needs the owning scheduler's generation and master-rendition checks
/// before use; this helper itself never publishes a track, profile or queued AU.
pub(super) unsafe fn read_segment(
    segment: &crate::hls::Segment,
    lane: Lane,
    expected: Option<&Binding>,
    aq: *mut AuQueue,
    net: &mut HlsNet,
    acquisition: SegmentAcquisition,
) -> Result<TimedLaneSegment, HlsExit> {
    if !ABI_OK.load(Ordering::Relaxed) {
        return Err(HlsExit::Failed("Bundled media demuxer is unavailable"));
    }
    ensure_registered();
    let mut capture = Capture::new(lane, expected);
    let timeline = crate::hls::SegmentTimeline::default();
    let mut clock = timeline.begin(segment.duration)
        .map_err(|_| HlsExit::Failed("Invalid HLS rendition duration"))?;
    let output = hls_demux_segment_path(
        segment,
        HlsSegmentRequest { path: &segment.resource.path, transport: HlsSegmentTransport::External, body_limit: Some(BODY_LIMIT),
            audio: None, rendition: Some(&mut capture), video: HlsVideoEntry::Independent },
        &mut clock, aq, net, "aac", acquisition,
    )?;
    if output.aus.len() != capture.timing.len() {
        return Err(HlsExit::Failed("HLS rendition packet/timing mismatch"));
    }
    let binding = capture.binding.take()
        .ok_or(HlsExit::Failed("HLS rendition was not inspected"))?;
    let samples = output.aus.into_iter().zip(capture.timing).map(|(au, timing)| {
        TimedSample { data: au.data, key: au.key != 0, timing }
    }).collect();
    Ok(TimedLaneSegment { binding, samples, transfer: output.transfer })
}

#[cfg(test)]
#[path = "hls_rendition_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "hls_rendition_audio_timing_tests.rs"]
mod audio_timing_tests;

#[cfg(test)]
#[path = "hls_rendition_adts_timing_tests.rs"]
mod adts_timing_tests;
