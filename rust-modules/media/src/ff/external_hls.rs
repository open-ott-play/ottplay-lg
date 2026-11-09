//! Independent HLS input. It never inherits or invents Plex authentication.
//! MPEG-TS renditions share the existing segment decoder. External AUDIO groups
//! use the separate bounded VOD/PDT joiner. Encryption, byte ranges, fMP4 maps
//! and unsupported group shapes are refused before a native Load is prepared.
use super::*;
use plx_base::checkpoint::{Checkpoint, Flow};
use std::sync::Arc;
use std::time::{Duration, Instant};

const PLAYLIST_BYTES: usize = 1024 * 1024;
const PLAYLIST_SEGMENTS: usize = 20_000;

#[derive(Clone)]
struct Variant {
    url: String,
    bandwidth: u64,
    subtitles: Option<String>,
}
#[derive(Clone)]
struct MediaSegment {
    inner: crate::hls::Segment,
    discontinuity_sequence: u64,
}
#[derive(Clone)]
struct Media {
    segments: Vec<MediaSegment>,
    target: Duration,
    ended: bool,
    independent: bool,
}

/// Only a completely fed segment can authorize dependent frames in its successor.
struct VideoContinuation {
    source: String,
    sequence: u64,
    discontinuity_sequence: u64,
    state: H264SegmentState,
}
impl VideoContinuation {
    fn candidate(previous: Option<&Self>, source: &str, segment: &MediaSegment,
        independent: bool) -> H264SegmentState
    {
        previous.filter(|previous| !independent
            && previous.source == source
            && previous.sequence.checked_add(1) == Some(segment.inner.sequence)
            && previous.discontinuity_sequence == segment.discontinuity_sequence)
            .map(|previous| previous.state.clone()).unwrap_or_default()
    }
}
enum Playlist {
    Master(Vec<Variant>),
    Media(Media),
}

pub(super) fn uri(base: &str, value: &str) -> Result<String, &'static str> {
    let url = plx_data::ott::resolve_http_url(base, value).ok_or("Invalid HLS resource address")?;
    if base.starts_with("https:") && !url.starts_with("https:") {
        return Err("HLS resource would leave HTTPS");
    }
    Ok(url)
}
fn resource(url: &str) -> Result<crate::hls::Resource, &'static str> {
    let split = plx_plex::plex::StreamUrl::parse(url);
    crate::hls::Resource::new(split.origin, split.path).map_err(|_| "Invalid HLS resource")
}
pub(super) fn attrs(value: &str) -> Result<std::collections::BTreeMap<&str, &str>, &'static str> {
    let mut out = std::collections::BTreeMap::new();
    let mut rest = value;
    while !rest.is_empty() {
        let (key, tail) = rest.split_once('=').ok_or("Malformed HLS attribute")?;
        let (val, next) = if let Some(tail) = tail.strip_prefix('"') {
            let end = tail.find('"').ok_or("Malformed HLS quoted attribute")?;
            let next = &tail[end + 1..];
            if !next.is_empty() && !next.starts_with(',') {
                return Err("Malformed HLS attribute separator");
            }
            (&tail[..end], next.strip_prefix(',').unwrap_or(next))
        } else {
            tail.split_once(',').unwrap_or((tail, ""))
        };
        if key.is_empty() || out.insert(key, val).is_some() {
            return Err("Duplicate HLS attribute");
        }
        rest = next;
    }
    Ok(out)
}

fn parse(base: &str, text: &str) -> Result<Playlist, &'static str> {
    parse_with_limits(base, text, PLAYLIST_SEGMENTS, None)
}

// The subtitle caller bounds work before expansion, not after a full playlist
// has already allocated thousands of copies of a long base URI.
fn parse_with_limits(base: &str, text: &str, segment_limit: usize, uri_bytes_limit: Option<usize>)
    -> Result<Playlist, &'static str>
{
    if uri_bytes_limit.is_some() && base.len() > 64 * 1024 {
        return Err("HLS resource base exceeds size limit");
    }
    let mut uri_bytes = 0usize;
    if text.len() > PLAYLIST_BYTES {
        return Err("HLS playlist exceeds size limit");
    }
    let mut lines = text.trim_start_matches('\u{feff}').lines().map(str::trim);
    if lines.next() != Some("#EXTM3U") {
        return Err("Invalid HLS playlist header");
    }
    let mut variants = Vec::new();
    let mut segments = Vec::new();
    let mut pending_variant = None;
    let mut pending_subtitles = None;
    let mut duration = None;
    let mut sequence = 0u64;
    let mut sequence_set = false;
    let mut discontinuity_sequence = 0u64;
    let mut discontinuity_sequence_set = false;
    let mut saw_discontinuity = false;
    let mut pending_discontinuity = false;
    let mut independent = false;
    let mut target = None;
    let mut ended = false;
    for line in lines {
        if line.len() > 64 * 1024 {
            return Err("HLS playlist line exceeds limit");
        }
        if line.is_empty() {
            continue;
        }
        if let Some(raw) = line.strip_prefix("#EXT-X-STREAM-INF:") {
            if pending_variant.is_some() || duration.is_some() || !segments.is_empty() {
                return Err("Mixed HLS playlist types");
            }
            let a = attrs(raw)?;
            if a.contains_key("AUDIO") || a.contains_key("VIDEO") {
                return Err("Separate HLS rendition groups are not supported");
            }
            let bandwidth = a
                .get("BANDWIDTH")
                .and_then(|v| v.parse::<u64>().ok())
                .filter(|v| *v > 0)
                .ok_or("Missing HLS variant bandwidth")?;
            pending_variant = Some(bandwidth);
            pending_subtitles = a.get("SUBTITLES").map(|v| (*v).to_owned());
        } else if let Some(raw) = line.strip_prefix("#EXTINF:") {
            if duration.is_some() || pending_variant.is_some() || !variants.is_empty() {
                return Err("Mixed HLS playlist types");
            }
            let seconds = raw
                .split(',')
                .next()
                .unwrap_or("")
                .parse::<f64>()
                .map_err(|_| "Invalid HLS segment duration")?;
            if !seconds.is_finite() || seconds <= 0.0 || seconds > 3600.0 {
                return Err("Invalid HLS segment duration");
            }
            duration = Some(Duration::from_secs_f64(seconds));
        } else if let Some(raw) = line.strip_prefix("#EXT-X-TARGETDURATION:") {
            if target.is_some() {
                return Err("Duplicate HLS target duration");
            }
            target = Some(Duration::from_secs(
                raw.parse::<u64>()
                    .ok()
                    .filter(|v| *v > 0 && *v <= 3600)
                    .ok_or("Invalid HLS target duration")?,
            ));
        } else if let Some(raw) = line.strip_prefix("#EXT-X-MEDIA-SEQUENCE:") {
            if sequence_set || !segments.is_empty() {
                return Err("Invalid HLS media sequence");
            }
            sequence = raw.parse().map_err(|_| "Invalid HLS media sequence")?;
            sequence_set = true;
        } else if let Some(raw) = line.strip_prefix("#EXT-X-DISCONTINUITY-SEQUENCE:") {
            if discontinuity_sequence_set || saw_discontinuity || !segments.is_empty()
                || raw.is_empty() || !raw.bytes().all(|b| b.is_ascii_digit())
            {
                return Err("Invalid HLS discontinuity sequence");
            }
            discontinuity_sequence = raw.parse().map_err(|_| "Invalid HLS discontinuity sequence")?;
            discontinuity_sequence_set = true;
        } else if line == "#EXT-X-DISCONTINUITY" {
            if pending_discontinuity { return Err("Duplicate HLS discontinuity"); }
            discontinuity_sequence = discontinuity_sequence.checked_add(1)
                .ok_or("HLS discontinuity sequence overflow")?;
            saw_discontinuity = true;
            pending_discontinuity = true;
        } else if line == "#EXT-X-INDEPENDENT-SEGMENTS" {
            if std::mem::replace(&mut independent, true) {
                return Err("Duplicate HLS independent-segments tag");
            }
        } else if line == "#EXT-X-ENDLIST" {
            ended = true;
        } else if line.starts_with("#EXT-X-KEY:") {
            let a = attrs(line.trim_start_matches("#EXT-X-KEY:"))?;
            if a.get("METHOD") != Some(&"NONE") {
                return Err("Encrypted HLS is not supported");
            }
        } else if line.starts_with("#EXT-X-MAP:")
            || line.starts_with("#EXT-X-BYTERANGE:")
            || line.starts_with("#EXT-X-SESSION-KEY:")
            || line.starts_with("#EXT-X-PART:")
            || line.starts_with("#EXT-X-PRELOAD-HINT:")
            || line == "#EXT-X-I-FRAMES-ONLY"
        {
            return Err("This HLS container or encryption feature is not supported");
        } else if line.starts_with("#EXT-X-MEDIA:") {
            let a = attrs(line.trim_start_matches("#EXT-X-MEDIA:"))?;
            if a.get("TYPE") == Some(&"AUDIO") && a.contains_key("URI") {
                return Err("Separate HLS audio is not supported");
            }
        } else if line.starts_with('#') {
            // Other metadata does not grant decoder-reference continuity. The exact
            // discontinuity and independence tags above are never accepted by prefix.
            if line == "#EXTM3U" {
                return Err("Duplicate HLS header");
            }
            if line.starts_with("#EXT-X-")
                && ![
                    "#EXT-X-VERSION:",
                    "#EXT-X-PROGRAM-DATE-TIME:",
                    "#EXT-X-PLAYLIST-TYPE:",
                    "#EXT-X-ALLOW-CACHE:",
                    "#EXT-X-I-FRAME-STREAM-INF:",
                    "#EXT-X-SESSION-DATA:",
                ]
                .iter()
                .any(|p| line.starts_with(p))
            {
                return Err("Unsupported HLS feature");
            }
        } else {
            if pending_variant.is_none() && duration.is_some() && segments.len() >= segment_limit {
                return Err("Too many HLS segments");
            }
            let url = uri(base, line)?;
            if let Some(limit) = uri_bytes_limit {
                if url.len() > 64 * 1024 { return Err("HLS resource exceeds size limit"); }
                uri_bytes = uri_bytes.checked_add(url.len()).filter(|bytes| *bytes <= limit)
                    .ok_or("HLS resource table exceeds size limit")?;
            }
            if let Some(bandwidth) = pending_variant.take() {
                if variants.len() >= 64 {
                    return Err("Too many HLS variants");
                }
                variants.push(Variant { url, bandwidth, subtitles: pending_subtitles.take() });
            } else if let Some(duration) = duration.take() {
                segments.push(MediaSegment {
                    inner: crate::hls::Segment { sequence, duration, resource: resource(&url)? },
                    discontinuity_sequence,
                });
                pending_discontinuity = false;
                sequence = sequence.checked_add(1).ok_or("HLS sequence overflow")?;
            } else {
                return Err("HLS URI has no media declaration");
            }
        }
    }
    if pending_variant.is_some() || duration.is_some() || pending_discontinuity {
        return Err("Truncated HLS playlist");
    }
    if !variants.is_empty() {
        Ok(Playlist::Master(variants))
    } else if !segments.is_empty() {
        Ok(Playlist::Media(Media {
            segments,
            target: target.ok_or("Missing HLS target duration")?,
            ended,
            independent,
        }))
    } else {
        Err("HLS playlist has no media")
    }
}

pub(super) enum Owner<'a> {
    Inspect(&'a AtomicBool),
    Playback(*mut AuQueue),
}
impl Owner<'_> {
    fn stopped(&self) -> bool {
        match self {
            Self::Inspect(a) => a.load(Ordering::Relaxed),
            Self::Playback(a) => unsafe { crate::aq::aq_is_aborted(*a) },
        }
    }
}
impl Checkpoint for Owner<'_> {
    fn check(&mut self) -> Flow {
        if self.stopped() {
            Flow::Stop
        } else {
            Flow::Continue {
                next_check: Some(Instant::now() + Duration::from_millis(100)),
            }
        }
    }
}

pub(super) fn fetch(url: &str, owner: &mut Owner<'_>) -> Result<(String, String), &'static str> {
    fetch_until(url, owner, Instant::now() + Duration::from_secs(15))
}

/// Shared bounded text transport. A multi-segment preparation keeps one outer deadline.
pub(super) fn fetch_until(url: &str, owner: &mut Owner<'_>, deadline: Instant)
    -> Result<(String, String), &'static str>
{
    fetch_traced(url, owner, deadline, PlaylistStage::Shared, 0)
}

#[derive(Clone, Copy)]
enum PlaylistStage { Initial, Selected, Playback, Shared }
impl PlaylistStage {
    fn label(self) -> &'static str {
        match self { Self::Initial => "initial", Self::Selected => "selected",
            Self::Playback => "playback", Self::Shared => "playlist" }
    }
    fn timeout(self) -> &'static str {
        match self {
            Self::Initial => "Initial HLS playlist request timed out",
            Self::Selected => "Selected HLS playlist request timed out",
            Self::Playback | Self::Shared => "HLS playlist request timed out",
        }
    }
}

/// Closed facts only. This object cannot hold a URL, header or server body.
struct FetchTrace { stage: PlaylistStage, depth: u8, started: Instant }
impl FetchTrace {
    fn record(&self, phase: &'static str, kind: &'static str, code: i32) {
        if kind == "ok" && !matches!(self.stage, PlaylistStage::Initial | PlaylistStage::Selected) {
            return;
        }
        crate::player::log(&format!(
            "hls: playlist stage={} depth={} phase={phase} elapsed_ms={} kind={kind} code={code}",
            self.stage.label(), self.depth.min(4), self.started.elapsed().as_millis()));
    }
    fn fail(&self, phase: &'static str, kind: &'static str, code: i32, message: &'static str) -> &'static str {
        self.record(phase, kind, code);
        message
    }
    fn open_error(&self, error: crate::curlio::OpenErr) -> &'static str {
        use crate::curlio::OpenErr;
        let (kind, code, message) = match error {
            OpenErr::Deadline => ("deadline", 0, self.stage.timeout()),
            OpenErr::Aborted | OpenErr::Stopped => ("cancelled", 0, "HLS request cancelled"),
            OpenErr::Status(code) => ("http", code, match code {
                401 => "HLS playlist server returned HTTP 401",
                403 => "HLS playlist server returned HTTP 403",
                404 => "HLS playlist server returned HTTP 404",
                429 => "HLS playlist server returned HTTP 429",
                500..=599 => "HLS playlist server returned a server error",
                _ => "HLS playlist server returned an HTTP error",
            }),
            OpenErr::Transport(code) => ("transport", code, "HLS playlist network transport failed"),
            OpenErr::Multi(code) => ("multi", code, "HLS playlist local transport failed"),
            OpenErr::Local => ("local", 0, "HLS playlist local transport setup failed"),
            OpenErr::Unavailable => ("unavailable", 0, "HLS transport unavailable"),
            OpenErr::RangeIgnored => ("range", 0, "HLS playlist server refused the requested byte range"),
        };
        self.fail("open", kind, code, message)
    }
}

fn fetch_traced(url: &str, owner: &mut Owner<'_>, deadline: Instant, stage: PlaylistStage, depth: u8)
    -> Result<(String, String), &'static str>
{
    let trace = FetchTrace { stage, depth, started: Instant::now() };
    if owner.stopped() { return Err(trace.open_error(crate::curlio::OpenErr::Stopped)); }
    if Instant::now() >= deadline { return Err(trace.open_error(crate::curlio::OpenErr::Deadline)); }
    let deadline = deadline.min(Instant::now() + Duration::from_secs(15));
    let mut source = match owner {
        Owner::Inspect(_) => {
            crate::curlio::CurlSource::open_private_checked(url, 0, Some(deadline), owner)
                .map(|(s, _)| s)
        }
        Owner::Playback(_) => {
            let reserved = crate::curlio::CurlSource::reserve_open()
                .map_err(|error| trace.open_error(error))?;
            if owner.stopped() {
                return Err(trace.open_error(crate::curlio::OpenErr::Stopped));
            }
            crate::curlio::CurlSource::open_reserved_checked(
                url,
                0,
                reserved,
                Some(deadline),
                owner,
            )
        }
    }
    .map_err(|error| trace.open_error(error))?;
    trace.record("open", "ok", 0);
    if source.size() > PLAYLIST_BYTES as i64 {
        return Err(trace.fail("body", "size", 0, "HLS playlist exceeds size limit"));
    }
    let landed = source.effective_url().to_owned();
    let mut bytes = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        let n = source.read_until(&mut chunk, Some(deadline), owner);
        if n < 0 {
            let (kind, message) = if owner.stopped() || n == crate::curlio::READ_STOPPED {
                ("cancelled", "HLS request cancelled")
            } else if n == crate::curlio::READ_DEADLINE {
                ("deadline", "HLS playlist body transfer timed out")
            } else { ("transport", "HLS playlist transfer failed") };
            return Err(trace.fail("body", kind, source.body_failure().map_or(0, |f| f.rc), message));
        }
        if n == 0 {
            break;
        }
        if bytes.len() + n as usize > PLAYLIST_BYTES {
            return Err(trace.fail("body", "size", 0, "HLS playlist exceeds size limit"));
        }
        bytes.extend_from_slice(&chunk[..n as usize]);
    }
    if source.size() >= 0 && bytes.len() as i64 != source.size() {
        return Err(trace.fail("body", "truncated", 0, "Truncated HLS response"));
    }
    let text = String::from_utf8(bytes)
        .map_err(|_| trace.fail("validate", "utf8", 0, "HLS playlist is not UTF-8"))?;
    trace.record("complete", "ok", 0);
    Ok((landed, text))
}

/// Keep the existing admission rule and expose only fixed codec-pair labels.
/// Unknown probe strings must never carry source text into a player error.
fn codec_refusal(video: &str, audio: &str) -> Option<&'static str> {
    Some(match (video, audio) {
        ("h264", "" | "aac") => return None,
        ("hevc", "") => "Native HLS does not support HEVC video without audio",
        ("hevc", "aac") => "Native HLS does not support HEVC video with AAC audio",
        ("hevc", "ac3") => "Native HLS does not support HEVC video with AC-3 audio",
        ("hevc", "eac3") => "Native HLS does not support HEVC video with E-AC-3 audio",
        ("hevc", "mp2") => "Native HLS does not support HEVC video with MP2 audio",
        ("h264", "ac3") => "Native HLS does not support H.264 video with AC-3 audio",
        ("h264", "eac3") => "Native HLS does not support H.264 video with E-AC-3 audio",
        ("h264", "mp2") => "Native HLS does not support H.264 video with MP2 audio",
        ("h264", "mp3") => "Native HLS does not support H.264 video with MP3 audio",
        ("mpeg2video", "aac") => "Native HLS does not support MPEG-2 video with AAC audio",
        ("mpeg2video", "mp2") => "Native HLS does not support MPEG-2 video with MP2 audio",
        ("mpeg2video", "ac3") => "Native HLS does not support MPEG-2 video with AC-3 audio",
        _ => "Native HLS requires H.264 video and AAC audio",
    })
}

pub(super) fn inspect(
    url: &str,
    cancelled: Arc<AtomicBool>,
) -> Result<super::inspect::MediaInfo, &'static str> {
    let source_url = plx_data::ott::http_url(url).ok_or("Invalid HLS address")?;
    let mut url = source_url.clone();
    let mut subtitle_master = None;
    for depth in 0..4 {
        let stage = if depth == 0 { PlaylistStage::Initial } else { PlaylistStage::Selected };
        let (landed, text) = fetch_traced(&url, &mut Owner::Inspect(&cancelled),
            Instant::now() + Duration::from_secs(15), stage, depth)?;
        if text.lines().any(|line| line.trim().strip_prefix("#EXT-X-STREAM-INF:")
            .and_then(|raw| attrs(raw).ok()).is_some_and(|a| a.contains_key("AUDIO")))
        {
            return super::hls_presentation::inspect_master(&source_url, &landed, &text, cancelled);
        }
        match parse(&landed, &text)? {
            Playlist::Master(mut variants) => {
                variants.sort_by_key(|v| v.bandwidth);
                let selected = &variants[0];
                if selected.subtitles.is_some() {
                    subtitle_master = Some((landed, text, selected.subtitles.clone()));
                }
                url = selected.url.clone();
            }
            Playlist::Media(media) => {
                let segment = media
                    .segments
                    .get(if media.ended {
                        0
                    } else {
                        media.segments.len().saturating_sub(3)
                    })
                    .ok_or("Empty HLS playlist")?;
                let url = format!(
                    "{}{}",
                    segment.inner.resource.origin.base(),
                    segment.inner.resource.path
                );
                let mut info = super::inspect::raw(&ExternalSource::Http(url), Arc::clone(&cancelled))?;
                let mp2 = info.video_codec == "h264" && info.audio_codec == "mp2"
                    && info.tracks.audio.first().is_some_and(|track|
                        hls_mp2::format(&track.codec, track.channels, track.sample_rate)
                            && track.decoder_available);
                if let Some(reason) = codec_refusal(&info.video_codec, &info.audio_codec).filter(|_| !mp2) {
                    return Err(reason);
                }
                // This table describes one segment, not all variants/tracks in
                // the presentation. Keep its rows for the existing manual menu.
                info.tracks.mark_incomplete();
                info.resolved_source = Some(ExternalSource::Hls(landed.clone()));
                info.duration_ns = if media.ended {
                    media
                        .segments
                        .iter()
                        .try_fold(0i64, |sum, s| {
                            sum.checked_add(i64::try_from(s.inner.duration.as_nanos()).ok()?)
                        })
                        .ok_or("HLS duration overflow")?
                } else {
                    0
                };
                if !media.ended {
                    if let Some((master_url, master_text, group)) = subtitle_master.as_ref() {
                        info.hls_playback = super::hls_presentation::live_presentation(
                            &source_url, master_url, master_text, group.as_deref(), &landed, &info)?;
                    }
                }
                return Ok(info);
            }
        }
    }
    Err("HLS playlist nesting exceeds limit")
}

pub(super) fn demux(
    url: &str,
    acodec: &str,
    audio: Option<&hls_audio::Selection>,
    aq: *mut AuQueue,
    aqa: *mut AuQueue,
    hs: *mut HttpStream,
    presentation: Option<&super::hls_presentation::HlsPlayback>,
) -> Result<(), HlsExit> {
    if let Some(presentation) = presentation { presentation.invalidate_live_clock(); }
    let mut net = HlsNet { hs, curl: None };
    let mut next = None;
    let mut video_continuation: Option<VideoContinuation> = None;
    let mut mp2 = (acodec == "mp2").then(hls_mp2::Decoder::default);
    let mut timeline = crate::hls::SegmentTimeline::default();
    let mut last_progress = Instant::now();
    'refresh: loop {
        if unsafe { crate::aq::aq_is_aborted(aq) } {
            return Err(HlsExit::Aborted);
        }
        let (landed, text) = fetch_traced(url, &mut Owner::Playback(aq),
            Instant::now() + Duration::from_secs(15), PlaylistStage::Playback, 0).map_err(HlsExit::Failed)?;
        let Playlist::Media(media) = parse(&landed, &text).map_err(HlsExit::Failed)? else {
            return Err(HlsExit::Failed(
                "HLS media address became a master playlist",
            ));
        };
        let target = SHARED.seek_to_ns.swap(-1, Ordering::Acquire);
        if target >= 0 {
            video_continuation = None;
            mp2 = (acodec == "mp2").then(hls_mp2::Decoder::default);
            if media.ended {
                let (index, start) = seek_start(&media, target).map_err(HlsExit::Failed)?;
                next = Some(media.segments[index].inner.sequence);
                timeline = start;
                SHARED.hls_video_tail_ns.store(-1, Ordering::Release);
                SHARED.hls_audio_tail_ns.store(-1, Ordering::Release);
                SHARED.first_audio_pts.store(i64::MIN, Ordering::Release);
            } else {
                // Live has no durable absolute timeline. Its controls must refuse seeking;
                // a stale command is not permission to reinterpret wall time as media time.
                return Err(HlsExit::Failed("Seeking requires a finite HLS playlist"));
            }
        }
        let expected = next.unwrap_or_else(|| {
            if media.ended {
                media.segments[0].inner.sequence
            } else {
                media.segments[media.segments.len().saturating_sub(3)].inner.sequence
            }
        });
        if media.segments[0].inner.sequence > expected {
            return Err(HlsExit::Failed("HLS live window advanced past playback"));
        }
        for segment in media.segments.iter().filter(|s| s.inner.sequence >= expected) {
            if SHARED.seek_to_ns.load(Ordering::Acquire) >= 0 {
                continue 'refresh;
            }
            if next.is_some_and(|n| n != segment.inner.sequence) {
                return Err(HlsExit::Failed("HLS segment sequence changed"));
            }
            let mut clock = timeline
                .begin(segment.inner.duration)
                .map_err(|_| HlsExit::Failed("HLS timeline overflow"))?;
            let mut candidate = VideoContinuation::candidate(
                video_continuation.as_ref(), &landed, segment, media.independent);
            if video_continuation.as_ref().is_some_and(|previous|
                previous.source != landed || previous.sequence.checked_add(1) != Some(segment.inner.sequence)
                    || previous.discontinuity_sequence != segment.discontinuity_sequence)
            {
                mp2 = (acodec == "mp2").then(hls_mp2::Decoder::default);
            }
            let output = unsafe {
                hls_demux_segment_path(
                    &segment.inner,
                    HlsSegmentRequest { path: &segment.inner.resource.path, transport: HlsSegmentTransport::External, body_limit: Some(64 * 1024 * 1024), audio, rendition: None,
                        video: HlsVideoEntry::External(&mut candidate), mp2: mp2.as_mut() },
                    &mut clock,
                    aq,
                    &mut net,
                    acodec,
                    SegmentAcquisition::for_cursor(None),
                )
            }?;
            if !hls_feed_segment_interruptible(&output, aq, aqa, true)? {
                video_continuation = None;
                mp2 = (acodec == "mp2").then(hls_mp2::Decoder::default);
                continue 'refresh;
            }
            if unsafe { crate::aq::aq_is_aborted(aq) } { return Err(HlsExit::Aborted); }
            if SHARED.seek_to_ns.load(Ordering::Acquire) >= 0 {
                video_continuation = None;
                mp2 = (acodec == "mp2").then(hls_mp2::Decoder::default);
                continue 'refresh;
            }
            // Commit references with the sequence/timeline, never on read success
            // alone: a seek may have interrupted feeding partway through the AUs.
            video_continuation = Some(VideoContinuation {
                source: landed.clone(), sequence: segment.inner.sequence,
                discontinuity_sequence: segment.discontinuity_sequence, state: candidate,
            });
            timeline.commit(clock);
            if let Some(presentation) = presentation {
                presentation.commit_live_clock(&landed, segment.discontinuity_sequence,
                    output.webvtt_clock, timeline.end_ns());
            }
            next = Some(
                segment.inner
                    .sequence
                    .checked_add(1)
                    .ok_or(HlsExit::Failed("HLS sequence overflow"))?,
            );
            last_progress = Instant::now();
        }
        if media.ended {
            if SHARED.seek_to_ns.load(Ordering::Acquire) >= 0 {
                continue;
            }
            return Ok(());
        }
        if last_progress.elapsed() > media.target.saturating_mul(6).max(Duration::from_secs(30)) {
            return Err(HlsExit::Failed("HLS playlist stopped advancing"));
        }
        hls_wait(
            aq,
            (media.target / 2).clamp(Duration::from_millis(250), Duration::from_secs(3)),
            None,
            &mut Owner::Playback(aq),
        )?;
    }
}

fn seek_start(
    media: &Media,
    target: i64,
) -> Result<(usize, crate::hls::SegmentTimeline), &'static str> {
    let mut timeline = crate::hls::SegmentTimeline::default();
    for (index, segment) in media.segments.iter().enumerate() {
        let next = timeline
            .begin(segment.inner.duration)
            .map_err(|_| "HLS timeline overflow")?;
        if target < next.end().0.saturating_mul(1_000_000) || index + 1 == media.segments.len() {
            return Ok((index, timeline));
        }
        timeline.commit(next);
    }
    Err("Cannot seek an empty HLS playlist")
}

#[cfg(test)]
#[path = "external_hls_fetch_tests.rs"]
mod fetch_tests;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codec_diagnostics_preserve_the_existing_admission_rule() {
        for video in ["h264", "hevc", "mpeg2video", "H264", "", "https://private.invalid/source"] {
            for audio in ["", "aac", "ac3", "eac3", "mp2", "mp3", "opus", "AAC"] {
                let previously_admitted = video == "h264" && (audio.is_empty() || audio == "aac");
                assert_eq!(codec_refusal(video, audio).is_none(), previously_admitted);
            }
        }
    }

    #[test]
    fn codec_diagnostics_name_known_rejected_pairs_without_echoing_unknown_values() {
        for (video, audio, label) in [
            ("hevc", "", "HEVC video without audio"),
            ("hevc", "aac", "HEVC video with AAC audio"),
            ("hevc", "ac3", "HEVC video with AC-3 audio"),
            ("hevc", "eac3", "HEVC video with E-AC-3 audio"),
            ("hevc", "mp2", "HEVC video with MP2 audio"),
            ("h264", "ac3", "H.264 video with AC-3 audio"),
            ("h264", "eac3", "H.264 video with E-AC-3 audio"),
            ("h264", "mp2", "H.264 video with MP2 audio"),
            ("h264", "mp3", "H.264 video with MP3 audio"),
            ("mpeg2video", "aac", "MPEG-2 video with AAC audio"),
            ("mpeg2video", "mp2", "MPEG-2 video with MP2 audio"),
            ("mpeg2video", "ac3", "MPEG-2 video with AC-3 audio"),
        ] {
            assert_eq!(codec_refusal(video, audio).unwrap(), format!("Native HLS does not support {label}"));
        }
        for (video, audio) in [
            ("h264", "https://private.invalid/source?token=private"),
            ("https://private.invalid/source?token=private", "aac"),
            ("hevc", "untrusted\ntext"), ("vp9", "aac"), ("hevc", "opus"),
        ] {
            assert_eq!(codec_refusal(video, audio), Some("Native HLS requires H.264 video and AAC audio"));
        }
    }
    #[test]
    fn separate_parser_supports_variants_and_cross_origin_without_credentials() {
        let p=parse("https://a.invalid:443/master.m3u8","#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=100,CODECS=\"avc1.64001f,mp4a.40.2\"\nhttps://b.invalid/low.m3u8\n#EXT-X-STREAM-INF:BANDWIDTH=200\nhigh.m3u8").unwrap();
        let Playlist::Master(v) = p else {
            panic!("master")
        };
        assert_eq!(v.len(), 2);
        assert_eq!(v[0].url, "https://b.invalid:443/low.m3u8");
    }
    #[test]
    fn encrypted_or_mapped_sources_are_explicitly_refused() {
        for tag in [
            "#EXT-X-KEY:METHOD=AES-128,URI=\"key\"",
            "#EXT-X-MAP:URI=\"init.mp4\"",
            "#EXT-X-BYTERANGE:20@0",
        ] {
            assert!(parse(
                "https://a.invalid:443/m.m3u8",
                &format!("#EXTM3U\n#EXT-X-TARGETDURATION:4\n{tag}\n#EXTINF:4,\n1.ts")
            )
            .is_err());
        }
    }
    #[test]
    fn media_sequences_and_durations_survive_discontinuity() {
        let p=parse("https://a.invalid:443/x/m.m3u8","#EXTM3U\n#EXT-X-TARGETDURATION:4\n#EXT-X-MEDIA-SEQUENCE:50\n#EXTINF:3.5,\n1.ts\n#EXT-X-DISCONTINUITY\n#EXTINF:4,\n2.ts\n#EXT-X-ENDLIST").unwrap();
        let Playlist::Media(m) = p else {
            panic!("media")
        };
        assert_eq!(m.segments[1].inner.sequence, 51);
        assert_eq!(m.segments[0].inner.duration.as_millis(), 3500);
        assert!(m.ended);
    }
    #[test]
    fn tls_downgrade_and_truncated_playlist_are_rejected() {
        assert!(uri("https://a.invalid:443/x", "http://a.invalid/1.ts").is_err());
        assert!(parse("https://a.invalid:443/x", "#EXTM3U\n#EXTINF:3,").is_err());
    }
    #[test]
    fn seeking_uses_cumulative_duration_and_clamps_to_final_segment() {
        let Playlist::Media(m)=parse("https://a.invalid:443/m.m3u8","#EXTM3U\n#EXT-X-TARGETDURATION:5\n#EXTINF:3,\n1.ts\n#EXTINF:5,\n2.ts\n#EXTINF:4,\n3.ts\n#EXT-X-ENDLIST").unwrap()else{panic!("media")};
        let (index, timeline) = seek_start(&m, 6_000_000_000).unwrap();
        assert_eq!(index, 1);
        assert_eq!(timeline.end_ns(), 3_000_000_000);
        let (index, timeline) = seek_start(&m, 50_000_000_000).unwrap();
        assert_eq!(index, 2);
        assert_eq!(timeline.end_ns(), 8_000_000_000);
        assert_eq!(seek_start(&m, 0).unwrap().0, 0);
    }

    #[test]
    fn discontinuity_tags_are_exact_ordered_and_checked() {
        let parse_media = |body: &str| parse("https://a.invalid:443/m.m3u8",
            &format!("#EXTM3U\n#EXT-X-TARGETDURATION:4\n{body}"));
        for body in [
            "#EXT-X-DISCONTINUITY-SEQUENCE:-1\n#EXTINF:4,\n1.ts",
            "#EXT-X-DISCONTINUITY-SEQUENCE:+1\n#EXTINF:4,\n1.ts",
            "#EXT-X-DISCONTINUITY-SEQUENCE:1x\n#EXTINF:4,\n1.ts",
            "#EXT-X-DISCONTINUITY-SEQUENCE:1\n#EXT-X-DISCONTINUITY-SEQUENCE:2\n#EXTINF:4,\n1.ts",
            "#EXT-X-DISCONTINUITY\n#EXT-X-DISCONTINUITY-SEQUENCE:1\n#EXTINF:4,\n1.ts",
            "#EXTINF:4,\n1.ts\n#EXT-X-DISCONTINUITY-SEQUENCE:1\n#EXTINF:4,\n2.ts",
            "#EXT-X-DISCONTINUITY-SEQUENCE:18446744073709551615\n#EXT-X-DISCONTINUITY\n#EXTINF:4,\n1.ts",
            "#EXT-X-DISCONTINUITY:1\n#EXTINF:4,\n1.ts",
            "#EXT-X-INDEPENDENT-SEGMENTS:1\n#EXTINF:4,\n1.ts",
        ] {
            assert!(parse_media(body).is_err(), "accepted malformed boundary: {body}");
        }
        let Playlist::Media(media) = parse_media("#EXTINF:4,\n1.ts\n#EXT-X-DISCONTINUITY\n#EXTINF:4,\n2.ts").unwrap()
            else { panic!("media") };
        assert_eq!(media.segments[0].discontinuity_sequence, 0);
        assert_eq!(media.segments[1].discontinuity_sequence, 1);
    }

    #[test]
    fn only_committed_contiguous_same_source_same_epoch_reuses_video_state() {
        let source = "https://a.invalid:443/m.m3u8";
        let media = |body: &str| {
            let Playlist::Media(media) = parse(source,
                &format!("#EXTM3U\n#EXT-X-TARGETDURATION:4\n{body}")).unwrap()
                else { panic!("media") };
            media
        };
        let old = media("#EXT-X-MEDIA-SEQUENCE:100\n#EXT-X-DISCONTINUITY-SEQUENCE:5\n#EXTINF:4,\n100.ts\n#EXT-X-DISCONTINUITY\n#EXTINF:4,\n101.ts");
        let new = media("#EXT-X-MEDIA-SEQUENCE:101\n#EXT-X-DISCONTINUITY-SEQUENCE:6\n#EXTINF:4,\n101.ts\n#EXTINF:4,\n102.ts");
        assert_eq!(old.segments[1].discontinuity_sequence, new.segments[0].discontinuity_sequence);
        let accepted = VideoContinuation {
            source: source.into(), sequence: old.segments[1].inner.sequence,
            discontinuity_sequence: old.segments[1].discontinuity_sequence,
            state: H264SegmentState { params: Some(H264ParamSets {
                sps: vec![0, 0, 0, 1, 0x67], pps: vec![0, 0, 0, 1, 0x68],
            }) },
        };
        let next = &new.segments[1];
        let candidate = VideoContinuation::candidate(Some(&accepted), source, next, false);
        assert_eq!(candidate.params.unwrap().sps, vec![0, 0, 0, 1, 0x67]);
        assert!(VideoContinuation::candidate(None, source, next, false).params.is_none());
        assert!(VideoContinuation::candidate(Some(&accepted), source, next, true).params.is_none());
        assert!(VideoContinuation::candidate(Some(&accepted), "https://b.invalid:443/m.m3u8", next, false).params.is_none());
        assert!(VideoContinuation::candidate(Some(&accepted), source, &new.segments[0], false).params.is_none());
        let mut changed = next.clone();
        changed.discontinuity_sequence += 1;
        assert!(VideoContinuation::candidate(Some(&accepted), source, &changed, false).params.is_none());
        changed = next.clone();
        changed.inner.sequence += 1;
        assert!(VideoContinuation::candidate(Some(&accepted), source, &changed, false).params.is_none());
    }
}

pub(super) fn vod_segments(base: &str, text: &str) -> Result<Vec<crate::hls::Segment>, &'static str> {
    match parse(base, text)? {
        Playlist::Media(media) if media.ended => Ok(media.segments.into_iter().map(|segment| segment.inner).collect()),
        _ => Err("Separate HLS audio currently requires a complete VOD playlist"),
    }
}

/// Same grammar as A/V; stronger per-job allocation bounds for a complete sidecar.
pub(super) fn vod_segments_bounded(base: &str, text: &str, segment_limit: usize, uri_bytes_limit: usize)
    -> Result<Vec<crate::hls::Segment>, &'static str>
{
    match parse_with_limits(base, text, segment_limit, Some(uri_bytes_limit))? {
        Playlist::Media(media) if media.ended => Ok(media.segments.into_iter().map(|segment| segment.inner).collect()),
        _ => Err("HLS subtitles require a complete VOD playlist"),
    }
}

/// Subtitle window parser shares the exact transport feature refusals with A/V.
pub(super) fn subtitle_window(base: &str, text: &str)
    -> Result<Vec<(crate::hls::Segment, u64)>, &'static str>
{
    match parse_with_limits(base, text, 64, Some(1024 * 1024))? {
        Playlist::Media(media) => Ok(media.segments.into_iter()
            .map(|s| (s.inner, s.discontinuity_sequence)).collect()),
        _ => Err("HLS subtitle address became a master playlist"),
    }
}

/// Bounded full-segment window for paired live single-lane scheduling.
pub(super) fn rendition_window(base: &str, text: &str)
    -> Result<(Vec<(crate::hls::Segment, u64)>, Duration, bool), &'static str>
{
    match parse_with_limits(base, text, 256, Some(1024 * 1024))? {
        Playlist::Media(media) => Ok((media.segments.into_iter()
            .map(|s| (s.inner, s.discontinuity_sequence)).collect(), media.target, media.ended)),
        _ => Err("HLS rendition address became a master playlist"),
    }
}
