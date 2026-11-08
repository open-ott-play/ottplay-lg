//! Consumer of immutable HLS VOD presentations and timed single-lane transport.
//! Only complete, jointly validated intervals enter the existing decoder feeder.
use super::*;
use super::hls_join::{JoinCursor, LaneClock, MappedSegment};
use super::hls_presentation::Vod;
use super::hls_rendition::Binding;
use super::hls_presentation::HlsPlayback;

fn at_time(media: &Vod, absolute_ns: i64) -> Result<usize, &'static str> {
    media.segments.iter().enumerate().find_map(|(i, s)| {
        let duration = i64::try_from(s.segment.duration.as_nanos()).ok()?;
        (absolute_ns < s.pdt_ns.checked_add(duration)? || i + 1 == media.segments.len()).then_some(i)
    }).ok_or("Cannot seek empty HLS rendition")
}

fn read_at(media: &Vod, index: usize, binding: &Binding, clock: &mut LaneClock,
    origin: i64, aq: *mut AuQueue, net: &mut HlsNet) -> Result<MappedSegment, HlsExit>
{
    let segment = media.segments.get(index).ok_or(HlsExit::Failed("HLS segment is missing"))?;
    let raw = unsafe { hls_rendition::read_segment(&segment.segment, binding.lane(),
        Some(binding), aq, net, SegmentAcquisition::for_cursor(None)) }?;
    clock.map(raw, segment.pdt_ns, origin, segment.explicit_precision_ns).map_err(HlsExit::Failed)
}

/// An initial resume must first establish the encoder's true clock from the
/// explicit first PDT. Later seeks retain this binding instead of using rounded
/// EXTINF sums as new packet origins. Priming produces no decoder output.
fn prime(media: &Vod, binding: &Binding, clock: &mut LaneClock, origin: i64,
    aq: *mut AuQueue, net: &mut HlsNet) -> Result<(), HlsExit>
{
    if !clock.is_anchored() { read_at(media, 0, binding, clock, origin, aq, net)?; }
    clock.restart();
    Ok(())
}

/// EXTINF locates a candidate only. Correct rounding drift against real packet
/// boundaries before giving the native decoder its preroll. A gap/overlap which
/// causes a direction reversal is rejected rather than looping between segments.
fn locate(media: &Vod, target: i64, binding: &Binding, clock: &mut LaneClock,
    origin: i64, aq: *mut AuQueue, net: &mut HlsNet) -> Result<Option<(usize, MappedSegment)>, HlsExit>
{
    let absolute = origin.checked_add(target).ok_or(HlsExit::Failed("HLS seek overflow"))?;
    let mut index = at_time(media, absolute).map_err(HlsExit::Failed)?;
    let mut direction = 0;
    loop {
        if SHARED.seek_to_ns.load(Ordering::Acquire) >= 0 { return Ok(None); }
        clock.restart();
        let segment = read_at(media, index, binding, clock, origin, aq, net)?;
        if SHARED.seek_to_ns.load(Ordering::Acquire) >= 0 { return Ok(None); }
        let step = if segment.start_ns > target && index > 0 { -1 }
            else if segment.end_ns <= target && index + 1 < media.segments.len() { 1 }
            else { return Ok(Some((index, segment))); };
        if direction != 0 && direction != step {
            return Err(HlsExit::Failed("HLS seek encountered a timestamp gap or overlap"));
        }
        direction = step;
        if step < 0 { index -= 1; } else { index += 1; }
    }
}

pub(super) fn demux(playback: &HlsPlayback, aq: *mut AuQueue, aqa: *mut AuQueue, hs: *mut HttpStream)
    -> Result<(), HlsExit>
{
    let video = playback.video();
    let origin = video.start_ns;
    let mut net = HlsNet { hs, curl: None };
    let mut next_video = 0usize;
    let mut next_audio = 0usize;
    let mut join = JoinCursor::new(0).map_err(HlsExit::Failed)?;
    let mut video_clock = playback.video_clock().cloned().unwrap_or_default();
    video_clock.restart();
    let mut audio_clock = LaneClock::default();
    'playback: loop {
        if unsafe { crate::aq::aq_is_aborted(aq) } { return Err(HlsExit::Aborted); }
        let target = SHARED.seek_to_ns.swap(-1, Ordering::Acquire);
        let frames = if target >= 0 {
            prime(video, playback.video_binding(), &mut video_clock, origin, aq, &mut net)?;
            if SHARED.seek_to_ns.load(Ordering::Acquire) >= 0 { continue; }
            prime(&playback.audio, &playback.audio_binding, &mut audio_clock, origin, aq, &mut net)?;
            let Some((index, frames)) = locate(video, target, playback.video_binding(),
                &mut video_clock, origin, aq, &mut net)? else { continue; };
            let Some((audio_index, audio)) = locate(&playback.audio, frames.start_ns, &playback.audio_binding,
                &mut audio_clock, origin, aq, &mut net)? else { continue; };
            next_video = index;
            next_audio = audio_index + 1;
            // The decoder's resume target still owns preroll. Join begins at
            // the preceding actual video boundary, not the approximate playlist date.
            join = JoinCursor::new(frames.start_ns).map_err(HlsExit::Failed)?;
            join.push_audio(audio).map_err(HlsExit::Failed)?;
            SHARED.hls_video_tail_ns.store(-1, Ordering::Release);
            SHARED.hls_audio_tail_ns.store(-1, Ordering::Release);
            SHARED.first_audio_pts.store(i64::MIN, Ordering::Release);
            frames
        } else {
            if next_video >= video.segments.len() {
                if SHARED.seek_to_ns.load(Ordering::Acquire) >= 0 { continue; }
                return Ok(());
            }
            let frames = read_at(video, next_video, playback.video_binding(), &mut video_clock, origin, aq, &mut net)?;
            if SHARED.seek_to_ns.load(Ordering::Acquire) >= 0 { continue; }
            frames
        };
        while !join.covers(&frames) {
            if next_audio >= playback.audio.segments.len() {
                return Err(HlsExit::Failed("Selected HLS audio ended before video"));
            }
            let audio = read_at(&playback.audio, next_audio, &playback.audio_binding,
                &mut audio_clock, origin, aq, &mut net)?;
            if SHARED.seek_to_ns.load(Ordering::Acquire) >= 0 { continue 'playback; }
            join.push_audio(audio).map_err(HlsExit::Failed)?;
            next_audio += 1;
        }
        let output = join.join(frames).map_err(HlsExit::Failed)?;
        if !hls_feed_segment_interruptible(&output, aq, aqa, true)? { continue; }
        next_video += 1;
    }
}
