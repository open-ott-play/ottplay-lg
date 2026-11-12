//! Paired rolling H.264/AAC playlists. PDT is an explicit epoch anchor; only raw
//! packet clocks and proven complete audio coverage authorize decoder output.
use super::*;
use crate::ff::hls_join::{JoinCursor, LaneClock, MappedSegment};
use std::time::{Duration, Instant};

const PRECISE_NS: i64 = 1_000_000;
const LOCATE_LIMIT: usize = 8;
const AUDIO_READ_LIMIT: usize = 64;

#[derive(Clone)]
pub(super) struct Window {
    pub(super) media: Vod,
    epochs: Vec<u64>,
    target: Duration,
    ended: bool,
}
impl Window {
    fn index(&self, sequence: u64) -> Option<usize> {
        self.media.segments.iter().position(|s| s.segment.sequence == sequence)
    }
    fn anchor(&self, epoch: u64) -> Result<usize, &'static str> {
        self.epochs.iter().enumerate().find_map(|(i, e)| (*e == epoch
            && self.media.segments[i].explicit_precision_ns.is_some_and(|p| p <= PRECISE_NS)).then_some(i))
            .ok_or("Live HLS epoch requires a precise explicit program date time")
    }
    fn last_sequence(&self) -> u64 { self.media.segments.last().unwrap().segment.sequence }
    fn cadence(&self) -> Duration { (self.target / 2).clamp(Duration::from_millis(250), Duration::from_secs(3)) }
}
fn parse(base: &str, text: &str) -> Result<Window, &'static str> {
    let (segments, target, ended) = crate::ff::external_hls::rendition_window(base, text)?;
    let mut result = Vec::with_capacity(segments.len());
    let mut epochs = Vec::with_capacity(segments.len());
    let mut explicit = None;
    let mut next_pdt = None;
    for line in text.lines().map(str::trim) {
        if line == "#EXT-X-DISCONTINUITY" { next_pdt = None; }
        if let Some(value) = line.strip_prefix("#EXT-X-PROGRAM-DATE-TIME:") {
            if explicit.is_some() { return Err("Duplicate HLS program date time"); }
            let time = crate::ff::hls_pdt::parse_program_date_time(value)?;
            let digits = value.split_once('.').map(|(_, tail)| tail.chars().take_while(|c| c.is_ascii_digit()).count()).unwrap_or(0);
            explicit = Some((time, 10_i64.pow(9 - digits as u32)));
        } else if !line.is_empty() && !line.starts_with('#') {
            let (segment, epoch) = segments.get(result.len()).ok_or("Invalid live HLS segment table")?.clone();
            let stamp = explicit.take();
            let pdt = stamp.map(|(time,_)| time).or(next_pdt)
                .ok_or("Live HLS epoch is missing program date time")?;
            let duration = i64::try_from(segment.duration.as_nanos()).map_err(|_| "HLS duration overflow")?;
            next_pdt = Some(pdt.checked_add(duration).ok_or("HLS date time overflow")?);
            result.push(AnchoredSegment { segment, pdt_ns: pdt, explicit_precision_ns: stamp.map(|(_,p)| p) });
            epochs.push(epoch);
        }
    }
    if explicit.is_some() || result.len() != segments.len() { return Err("Incomplete live HLS date time table"); }
    let start_ns = result[0].pdt_ns;
    let end_ns = next_pdt.ok_or("Empty live HLS playlist")?;
    Ok(Window { media: Vod { url: base.into(), segments: result, start_ns, end_ns }, epochs, target, ended })
}

pub(super) fn fetch_window(url: &str, cancelled: &AtomicBool) -> Result<Window, &'static str> {
    let (landed, text) = crate::ff::external_hls::fetch(url, &mut crate::ff::external_hls::Owner::Inspect(cancelled))?;
    parse(&landed, &text)
}
pub(super) fn prepare_audio(url: &str, cancelled: Arc<AtomicBool>) -> Result<(Window, Binding), &'static str> {
    let (landed, text) = crate::ff::external_hls::fetch(url, &mut crate::ff::external_hls::Owner::Inspect(&cancelled))?;
    let media = parse(&landed, &text)?;
    let info = crate::ff::inspect::raw_lane(&source_of(&media.media.segments[0]), cancelled, Lane::Audio)?;
    let track = info.tracks.audio.first().cloned().ok_or("Missing live HLS AAC rendition")?;
    Ok((media, Binding::Audio(track)))
}
pub(super) fn validate_pair(video: &Window, audio: &Window) -> Result<(), &'static str> {
    let index = video.media.segments.len().saturating_sub(3);
    let epoch = video.epochs[index];
    video.anchor(epoch)?;
    audio.anchor(epoch)?;
    let time = video.media.segments[index].pdt_ns;
    if !audio.media.segments.iter().enumerate().any(|(i,s)| audio.epochs[i] == epoch
        && s.pdt_ns <= time && s.pdt_ns.saturating_add(s.segment.duration.as_nanos().min(i64::MAX as u128) as i64) > time)
    { return Err("Live HLS audio window does not overlap video epoch"); }
    Ok(())
}
pub(super) fn inspect(source_url: &str, master_url: &str, master: Master, landed: &str, text: &str,
    cancelled: Arc<AtomicBool>) -> Result<crate::ff::inspect::MediaInfo, &'static str>
{
    let video = parse(landed, text)?;
    let row = master.renditions.iter().find(|r| r.id == master.default).unwrap();
    let url = crate::ff::external_hls::uri(master_url, &row.url)?;
    let (audio, audio_binding) = prepare_audio(&url, Arc::clone(&cancelled))?;
    validate_pair(&video, &audio)?;
    let mut info = crate::ff::inspect::raw_lane(&source_of(&video.media.segments[0]), cancelled, Lane::Video)?;
    let Binding::Audio(track) = &audio_binding else { return Err("Invalid live HLS AAC binding"); };
    info.audio_codec = track.codec.clone(); info.audio_channels = track.channels;
    info.audio_sample_rate = track.sample_rate; info.audio_bit_depth = track.bit_depth;
    info.duration_ns = 0; info.tracks.mark_incomplete();
    info.resolved_source = Some(ExternalSource::Hls(source_url.into()));
    info.hls_playback = Some(HlsPlayback { presentation: Arc::new(HlsPresentation {
        source_url: source_url.into(), master_url: master_url.into(), variant_url: landed.into(),
        video: video.media.clone(), video_binding: Binding::Video { width: info.width.into(), height: info.height.into() },
        renditions: master.renditions, subtitles: master.subtitles, video_clock: None,
        live: Some(crate::ff::hls_webvtt::live::Timeline::default()), live_video: Some(video.clone()),
    }), selected: master.default, audio: audio.media.clone(), audio_binding, live_audio: Some((video.clone(), audio)) });
    Ok(info)
}

fn refresh_valid(previous: &Window, current: &Window, next: u64) -> Result<(), &'static str> {
    if previous.media.url != current.media.url { return Err("Live HLS rendition source changed"); }
    if current.media.segments[0].segment.sequence > next { return Err("Live HLS rendition window advanced past playback"); }
    if current.last_sequence() < previous.last_sequence() { return Err("Live HLS rendition playlist regressed"); }
    if previous.ended && current.last_sequence() > previous.last_sequence() { return Err("Live HLS rendition grew after end"); }
    for (i, segment) in current.media.segments.iter().enumerate() {
        if let Some(old) = previous.index(segment.segment.sequence) {
            let before = &previous.media.segments[old];
            if before.segment != segment.segment || previous.epochs[old] != current.epochs[i]
                || before.explicit_precision_ns.is_some() && segment.explicit_precision_ns.is_some() && before.pdt_ns != segment.pdt_ns
            { return Err("Live HLS rendition overlap changed"); }
        }
    }
    Ok(())
}
struct Refresh {
    window: Window,
    due: Instant,
    progress: Instant,
}
impl Refresh {
    fn new(window: Window) -> Self { Self { window, due: Instant::now(), progress: Instant::now() } }
    fn ensure_epoch(&mut self, epoch: u64, next: u64, aq: *mut AuQueue) -> Result<(), HlsExit> {
        self.ensure_epoch_until(epoch, next, aq, Instant::now() + Duration::from_secs(30))
    }
    fn ensure_epoch_until(&mut self, epoch: u64, next: u64, aq: *mut AuQueue, deadline: Instant) -> Result<(), HlsExit> {
        loop {
            if Instant::now() >= deadline { return Err(HlsExit::Failed("Live HLS matching audio epoch timed out")); }
            stopped(aq)?;
            if self.window.anchor(epoch).is_ok() { return Ok(()); }
            if self.window.ended || self.window.epochs.iter().all(|e| *e > epoch) {
                return Err(HlsExit::Failed("Live HLS lane epochs disagree"));
            }
            if Instant::now() < self.due {
                let delay = self.due.saturating_duration_since(Instant::now()).min(Duration::from_millis(250))
                    .min(deadline.saturating_duration_since(Instant::now()));
                crate::ff::hls_wait(aq, delay, None, &mut crate::ff::external_hls::Owner::Playback(aq))?;
                continue;
            }
            let (landed, text) = crate::ff::external_hls::fetch(&self.window.media.url,
                &mut crate::ff::external_hls::Owner::Playback(aq)).map_err(HlsExit::Failed)?;
            let candidate = parse(&landed, &text).map_err(HlsExit::Failed)?;
            // At an epoch boundary old pending audio is discarded; permit the
            // publisher to retire it only when the requested epoch is present.
            let cursor = if candidate.anchor(epoch).is_ok() {
                candidate.media.segments[0].segment.sequence.max(next)
            } else { next };
            refresh_valid(&self.window, &candidate, cursor).map_err(HlsExit::Failed)?;
            if candidate.last_sequence() > self.window.last_sequence() { self.progress = Instant::now(); }
            self.window = candidate;
            self.due = Instant::now() + self.window.cadence();
            let limit = self.window.target.saturating_mul(6).clamp(Duration::from_secs(30), Duration::from_secs(120));
            if self.progress.elapsed() > limit { return Err(HlsExit::Failed("Live HLS rendition stopped advancing")); }
        }
    }
    fn get(&mut self, sequence: u64, aq: *mut AuQueue) -> Result<Option<usize>, HlsExit> {
        loop {
            stopped(aq)?;
            if let Some(index) = self.window.index(sequence) { return Ok(Some(index)); }
            if sequence < self.window.media.segments[0].segment.sequence {
                return Err(HlsExit::Failed("Live HLS rendition window advanced past playback"));
            }
            if self.window.ended { return Ok(None); }
            if Instant::now() < self.due {
                let delay = self.due.saturating_duration_since(Instant::now()).min(Duration::from_millis(250));
                crate::ff::hls_wait(aq, delay, None, &mut crate::ff::external_hls::Owner::Playback(aq))?;
                continue;
            }
            let (landed, text) = crate::ff::external_hls::fetch(&self.window.media.url,
                &mut crate::ff::external_hls::Owner::Playback(aq)).map_err(HlsExit::Failed)?;
            let mut candidate = parse(&landed, &text).map_err(HlsExit::Failed)?;
            refresh_valid(&self.window, &candidate, sequence).map_err(HlsExit::Failed)?;
            if candidate.last_sequence() > self.window.last_sequence() { self.progress = Instant::now(); }
            candidate.ended |= self.window.ended;
            self.window = candidate;
            self.due = Instant::now() + self.window.cadence();
            let limit = self.window.target.saturating_mul(6).clamp(Duration::from_secs(30), Duration::from_secs(120));
            if self.progress.elapsed() > limit { return Err(HlsExit::Failed("Live HLS rendition stopped advancing")); }
        }
    }
}
fn stopped(aq: *mut AuQueue) -> Result<(), HlsExit> {
    if unsafe { crate::aq::aq_is_aborted(aq) } { return Err(HlsExit::Aborted); }
    if SHARED.seek_to_ns.swap(-1, Ordering::Acquire) >= 0 {
        return Err(HlsExit::Failed("Seeking requires a finite HLS playlist"));
    }
    Ok(())
}
fn read(window: &Window, index: usize, binding: &Binding, clock: &mut LaneClock,
    origin: i64, aq: *mut AuQueue, net: &mut HlsNet) -> Result<MappedSegment, HlsExit>
{
    stopped(aq)?;
    let segment = &window.media.segments[index];
    let raw = unsafe { crate::ff::hls_rendition::read_segment(&segment.segment, binding.lane(), Some(binding),
        aq, net, SegmentAcquisition::for_cursor(None)) }?;
    stopped(aq)?;
    clock.map(raw, segment.pdt_ns, origin, segment.explicit_precision_ns).map_err(HlsExit::Failed)
}
fn prime(window: &Window, epoch: u64, binding: &Binding, origin: i64,
    aq: *mut AuQueue, net: &mut HlsNet) -> Result<LaneClock, HlsExit>
{
    let index = window.anchor(epoch).map_err(HlsExit::Failed)?;
    let mut clock = LaneClock::default();
    read(window, index, binding, &mut clock, origin, aq, net)?;
    clock.restart();
    Ok(clock)
}
fn locate_audio(window: &Window, epoch: u64, target: i64, origin: i64, binding: &Binding,
    clock: &mut LaneClock, aq: *mut AuQueue, net: &mut HlsNet) -> Result<(usize, MappedSegment), HlsExit>
{
    let absolute = origin.checked_add(target).ok_or(HlsExit::Failed("Live HLS clock overflow"))?;
    let mut index = window.media.segments.iter().enumerate().find_map(|(i,s)| (window.epochs[i] == epoch
        && absolute < s.pdt_ns.saturating_add(s.segment.duration.as_nanos().min(i64::MAX as u128) as i64)).then_some(i))
        .ok_or(HlsExit::Failed("Live HLS audio window does not cover video"))?;
    let mut direction = 0;
    for _ in 0..LOCATE_LIMIT {
        if window.epochs[index] != epoch { return Err(HlsExit::Failed("Live HLS lane epochs disagree")); }
        clock.restart();
        let mapped = read(window, index, binding, clock, origin, aq, net)?;
        let step = if mapped.start_ns > target + PRECISE_NS { -1 }
            else if mapped.end_ns <= target { 1 } else { return Ok((index, mapped)); };
        if direction != 0 && direction != step { return Err(HlsExit::Failed("Live HLS audio clock has a gap or overlap")); }
        direction = step;
        index = if step < 0 { index.checked_sub(1) } else { index.checked_add(1).filter(|i| *i < window.epochs.len()) }
            .ok_or(HlsExit::Failed("Live HLS audio does not cover video start"))?;
    }
    Err(HlsExit::Failed("Live HLS audio location exceeds work limit"))
}

fn rebase_output(output: &mut HlsSegmentOutput, offset: i64) -> Result<(), &'static str> {
    for au in &mut output.aus {
        au.pts_ns = au.pts_ns.checked_sub(offset).filter(|pts| *pts >= 0)
            .ok_or("Live HLS output precedes its common origin")?;
    }
    output.video_tail_ns = output.video_tail_ns.checked_sub(offset)
        .ok_or("Live HLS output clock overflow")?;
    output.audio_tail_ns = output.audio_tail_ns.map(|time| time.checked_sub(offset)
        .ok_or("Live HLS output clock overflow")).transpose()?;
    if let Some(clock) = &mut output.webvtt_clock {
        clock.presentation_ns = clock.presentation_ns.checked_sub(offset)
            .ok_or("Live HLS subtitle clock overflow")?;
    }
    Ok(())
}

pub(in crate::ff) fn demux(playback: &HlsPlayback, aq: *mut AuQueue, aqa: *mut AuQueue,
    hs: *mut HttpStream) -> Result<(), HlsExit>
{
    playback.invalidate_live_clock();
    let pair = playback.live_audio.clone().ok_or(HlsExit::Failed("Missing live HLS paired presentation"))?;
    let mut video = Refresh::new(pair.0);
    let mut audio = Refresh::new(pair.1);
    let recovery_generation = SHARED.hls_candidate_generation.load(Ordering::Acquire);
    let mut next_video = video.window.media.segments[video.window.media.segments.len().saturating_sub(3)].segment.sequence;
    let mut next_audio = 0;
    let mut epoch = None;
    let mut origin = 0;
    let mut tail = 0;
    let mut output_offset = None;
    let mut video_clock = LaneClock::default();
    let mut audio_clock = LaneClock::default();
    let mut join = None;
    let mut net = HlsNet { hs, curl: None };
    loop {
        stopped(aq)?;
        let Some(index) = video.get(next_video, aq)? else { return Ok(()); };
        let current_epoch = video.window.epochs[index];
        let new_epoch = epoch != Some(current_epoch);
        if new_epoch {
            // Both lanes must explicitly describe the same epoch. Never align
            // unrelated media sequence numbers or extrapolate across a reset.
            audio.ensure_epoch(current_epoch, next_audio, aq)?;
            let va = video.window.anchor(current_epoch).map_err(HlsExit::Failed)?;
            let aa = audio.window.anchor(current_epoch).map_err(HlsExit::Failed)?;
            let video_pdt = video.window.media.segments[va].pdt_ns;
            let audio_pdt = audio.window.media.segments[aa].pdt_ns;
            origin = if epoch.is_none() { video_pdt.min(audio_pdt) }
                else { video.window.media.segments[index].pdt_ns.checked_sub(tail)
                    .ok_or(HlsExit::Failed("Live HLS epoch clock overflow"))? };
            video_clock = prime(&video.window, current_epoch, playback.video_binding(), origin, aq, &mut net)?;
            audio_clock = prime(&audio.window, current_epoch, &playback.audio_binding, origin, aq, &mut net)?;
        }
        let frames = read(&video.window, index, playback.video_binding(), &mut video_clock, origin, aq, &mut net)?;
        if new_epoch {
            let (audio_index, mapped) = locate_audio(&audio.window, current_epoch, frames.start_ns, origin,
                &playback.audio_binding, &mut audio_clock, aq, &mut net)?;
            let mut candidate = JoinCursor::new(frames.start_ns).map_err(HlsExit::Failed)?;
            candidate.push_audio(mapped).map_err(HlsExit::Failed)?;
            next_audio = audio.window.media.segments[audio_index].segment.sequence.checked_add(1)
                .ok_or(HlsExit::Failed("Live HLS sequence overflow"))?;
            join = Some(candidate);
        }
        let joined = join.as_mut().ok_or(HlsExit::Failed("Missing live HLS join cursor"))?;
        let mut reads = 0;
        while !joined.covers(&frames) {
            reads += 1;
            if reads > AUDIO_READ_LIMIT { return Err(HlsExit::Failed("Live HLS audio coverage exceeds work limit")); }
            let Some(ai) = audio.get(next_audio, aq)? else { return Err(HlsExit::Failed("Live HLS audio ended before video")); };
            if audio.window.epochs[ai] != current_epoch { return Err(HlsExit::Failed("Live HLS lane epochs disagree")); }
            let mapped = read(&audio.window, ai, &playback.audio_binding, &mut audio_clock, origin, aq, &mut net)?;
            joined.push_audio(mapped).map_err(HlsExit::Failed)?;
            next_audio = next_audio.checked_add(1).ok_or(HlsExit::Failed("Live HLS sequence overflow"))?;
        }
        let end = frames.end_ns;
        let media_ns = frames.end_ns.checked_sub(frames.start_ns)
            .and_then(|ns| u64::try_from(ns).ok()).filter(|ns| *ns > 0)
            .ok_or(HlsExit::Failed("Live HLS output clock overflow"))?;
        let mut output = joined.join(frames).map_err(HlsExit::Failed)?;
        // Prime with retained explicit epoch anchors, then start the native
        // timeline at the earliest actual retained AU, preserving AAC/video
        // lead exactly. Rounded EXTINF never chooses this output offset.
        let offset = *output_offset.get_or_insert(output.aus.iter().map(|au| au.pts_ns).min()
            .ok_or(HlsExit::Failed("Empty live HLS joined interval"))?);
        output.webvtt_clock = video_clock.webvtt_clock();
        rebase_output(&mut output, offset).map_err(HlsExit::Failed)?;
        if !crate::ff::hls_feed_external_segment(&output, std::time::Duration::from_nanos(media_ns), recovery_generation, aq, aqa)? { return Err(HlsExit::Aborted); }
        stopped(aq)?;
        playback.commit_live_clock(&video.window.media.url, current_epoch, output.webvtt_clock, output.video_tail_ns);
        tail = end; epoch = Some(current_epoch);
        next_video = next_video.checked_add(1).ok_or(HlsExit::Failed("Live HLS sequence overflow"))?;
    }
}

#[cfg(test)]
#[path = "hls_live_audio_tests.rs"]
mod tests;
