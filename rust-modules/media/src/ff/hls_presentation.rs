//! Immutable external-HLS presentation, separate from embedded AVStream ordinals.
//! Private URLs and authentication remain in worker-owned memory only. Debug
//! output and public selections contain no URL, path, query or credential.
use super::*;
use std::sync::Arc;
use super::hls_rendition::{Binding, Lane};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RenditionId(u32);
impl RenditionId {
    /// Presentation-scoped feedback/row key, never a persistent or container ordinal.
    pub fn opaque_key(self) -> u32 { self.0 }
}

/// Presentation-local subtitle identity, never an audio ID or container ordinal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HlsSubtitleId(pub(crate) u32);
impl HlsSubtitleId {
    pub fn opaque_key(self) -> u32 { self.0 }
}
#[derive(Clone)]
struct SubtitleRendition {
    id: HlsSubtitleId,
    name: String,
    language: String,
    // Metadata is only a candidate. Its selected playlist and every body must validate.
    reference: Result<String, &'static str>,
}
#[derive(Clone)]
struct Rendition {
    id: RenditionId,
    name: String,
    language: String,
    // Validated URI reference, resolved against the one retained master URL on selection.
    url: String,
    default: bool,
}
// Borrow the bounded input until the chosen variant identifies its audio group.
// Unselected groups still pass every validation, but do not retain owned URLs.
struct RenditionRef<'a> {
    id: RenditionId,
    group: &'a str,
    name: &'a str,
    language: &'a str,
    url: &'a str,
    default: bool,
}
struct Master {
    variant_url: String,
    renditions: Vec<Rendition>,
    default: RenditionId,
    subtitles: Vec<SubtitleRendition>,
}
#[derive(Clone)]
pub(super) struct AnchoredSegment {
    pub(super) segment: crate::hls::Segment,
    pub(super) pdt_ns: i64,
    // None means EXTINF-extrapolated navigation estimate, never a clock anchor.
    pub(super) explicit_precision_ns: Option<i64>,
}
#[derive(Clone)]
pub(super) struct Vod {
    pub(super) url: String,
    pub(super) segments: Vec<AnchoredSegment>,
    pub(super) start_ns: i64,
    pub(super) end_ns: i64,
}
struct HlsPresentation {
    source_url: String,
    master_url: String,
    variant_url: String,
    video: Vod,
    video_binding: Binding,
    renditions: Vec<Rendition>,
    subtitles: Vec<SubtitleRendition>,
    video_clock: Option<super::hls_join::LaneClock>,
}

#[derive(Clone)]
pub struct HlsPlayback {
    presentation: Arc<HlsPresentation>,
    selected: RenditionId,
    pub(super) audio: Vod,
    pub(super) audio_binding: Binding,
}
impl std::fmt::Debug for HlsPlayback {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HlsPlayback").field("selected", &self.selected)
            .field("renditions", &self.presentation.renditions.len()).finish_non_exhaustive()
    }
}
impl HlsPlayback {
    pub fn selected(&self) -> RenditionId { self.selected }
    pub fn rows(&self) -> impl Iterator<Item = (RenditionId, &str, &str)> {
        self.presentation.renditions.iter().map(|r| (r.id, r.name.as_str(), r.language.as_str()))
    }
    pub fn subtitle_rows(&self) -> impl Iterator<Item = (HlsSubtitleId, &str, &str)> {
        let clock_available = self.presentation.video_clock.as_ref().and_then(|clock| clock.webvtt_clock()).is_some();
        self.presentation.subtitles.iter().filter(move |r| clock_available && r.reference.is_ok())
            .map(|r| (r.id, r.name.as_str(), r.language.as_str()))
    }
    pub fn has_subtitle(&self, id: HlsSubtitleId) -> bool {
        self.subtitle_rows().any(|(candidate, _, _)| candidate == id)
    }
    /// Off-frame candidate validation. The caller owns admission/cancellation and
    /// keeps its current subtitle choice until this complete canonical sidecar lands.
    pub fn prepare_subtitle(&self, id: HlsSubtitleId, cancelled: Arc<AtomicBool>) -> Result<Vec<u8>, &'static str> {
        if !self.has_subtitle(id) { return Err("HLS subtitle is unavailable in this presentation"); }
        let row = self.presentation.subtitles.iter().find(|r| r.id == id)
            .ok_or("HLS subtitle is not in this presentation")?;
        let reference = row.reference.as_ref().map_err(|error| *error)?;
        let clock = self.presentation.video_clock.as_ref().and_then(|clock| clock.webvtt_clock())
            .ok_or("HLS subtitle video clock is unavailable")?;
        super::hls_webvtt::prepare(&self.presentation.master_url, reference, clock, cancelled)
    }
    pub(super) fn video_clock(&self) -> Option<&super::hls_join::LaneClock> { self.presentation.video_clock.as_ref() }
    pub fn same_presentation(&self, other: &Self) -> bool { Arc::ptr_eq(&self.presentation, &other.presentation) }
    pub fn has_rendition(&self, id: RenditionId) -> bool { self.presentation.renditions.iter().any(|r| r.id == id) }
    pub fn audio_track(&self) -> &super::inspect::InspectedTrack {
        match &self.audio_binding { Binding::Audio(track) => track, _ => unreachable!("prepared audio lane") }
    }
    pub(super) fn video(&self) -> &Vod { &self.presentation.video }
    pub(super) fn video_binding(&self) -> &Binding { &self.presentation.video_binding }
    pub fn matches_source(&self, source: &str) -> bool {
        self.presentation.source_url == source
            && plx_data::ott::http_url(&self.presentation.master_url).is_some()
            && plx_data::ott::http_url(&self.presentation.variant_url).is_some()
            && plx_data::ott::http_url(&self.presentation.video.url).is_some()
            && plx_data::ott::http_url(&self.audio.url).is_some()
    }

    /// Off-frame private preparation only. The application must check its exact
    /// logical playback, choice and intention/worker tickets before admission.
    /// No playing stream, decoder queue, settings or selected track is changed.
    pub fn prepare_rendition(&self, id: RenditionId, cancelled: Arc<AtomicBool>) -> Result<Self, &'static str> {
        let row = self.presentation.renditions.iter().find(|r| r.id == id)
            .ok_or("HLS rendition choice is not in this presentation")?;
        let url = super::external_hls::uri(&self.presentation.master_url, &row.url)?;
        let (audio, binding) = prepare_audio(&url, cancelled)?;
        validate_window(&self.presentation.video, &audio)?;
        Ok(Self { presentation: Arc::clone(&self.presentation), selected: id, audio, audio_binding: binding })
    }
}

fn yes(value: Option<&&str>) -> bool { value.is_some_and(|v| *v == "YES") }
fn parse_master(base: &str, text: &str) -> Result<Master, &'static str> {
    if text.len() > 1024 * 1024 { return Err("HLS playlist exceeds size limit"); }
    let mut lines = text.trim_start_matches('\u{feff}').lines().map(str::trim);
    if lines.next() != Some("#EXTM3U") { return Err("Invalid HLS playlist header"); }
    let mut renditions = Vec::new();
    let mut identities = std::collections::BTreeSet::new();
    let mut variants = Vec::new();
    let mut pending = None;
    for line in lines {
        if line.len() > 64 * 1024 { return Err("HLS playlist line exceeds limit"); }
        if let Some(raw) = line.strip_prefix("#EXT-X-MEDIA:") {
            let a = super::external_hls::attrs(raw)?;
            if a.get("TYPE") != Some(&"AUDIO") { continue; }
            let group = *a.get("GROUP-ID").filter(|s| !s.is_empty()).ok_or("Missing HLS audio group")?;
            let name = *a.get("NAME").filter(|s| !s.is_empty()).ok_or("Missing HLS audio name")?;
            if !identities.insert((group, name)) {
                return Err("Duplicate HLS audio rendition identity");
            }
            let url = *a.get("URI").ok_or("Embedded audio group is outside this route")?;
            // Validate every group now; discard the expanded URL so a long base
            // is not copied once per row. The raw references borrow the input.
            super::external_hls::uri(base, url)?;
            // The 1 MiB input bound also keeps this presentation-scoped ID within u32.
            renditions.push(RenditionRef { id: RenditionId(renditions.len() as u32 + 1), group, name,
                language: a.get("LANGUAGE").copied().unwrap_or(""), url, default: yes(a.get("DEFAULT")) });
        } else if let Some(raw) = line.strip_prefix("#EXT-X-STREAM-INF:") {
            if pending.is_some() { return Err("HLS variant has no URI"); }
            let a = super::external_hls::attrs(raw)?;
            // Subtitle candidates remain separate from audio IDs and fetch only on selection.
            if a.contains_key("VIDEO") {
                return Err("Additional HLS rendition groups are outside this route");
            }
            let bandwidth = a.get("BANDWIDTH").and_then(|s| s.parse::<u64>().ok())
                .filter(|v| *v > 0).ok_or("Invalid HLS variant bandwidth")?;
            let group = a.get("AUDIO").ok_or("HLS variant has no audio group")?.to_string();
            pending = Some((bandwidth, group, a.get("SUBTITLES").map(|value| value.to_string())));
        } else if !line.is_empty() && !line.starts_with('#') {
            let (bandwidth, group, subtitles) = pending.take().ok_or("Unexpected HLS master URI")?;
            if variants.len() >= 64 { return Err("Too many HLS variants"); }
            variants.push((bandwidth, group, super::external_hls::uri(base, line)?, subtitles));
        }
    }
    if pending.is_some() { return Err("HLS variant has no URI"); }
    variants.sort_by_key(|v| v.0);
    let (_, group, variant_url, subtitle_group) = variants.into_iter().next().ok_or("HLS master has no variant")?;
    let renditions: Vec<_> = renditions.into_iter().filter(|r| r.group == group).map(|r| Rendition {
        id: r.id, name: r.name.into(), language: r.language.into(), url: r.url.into(), default: r.default,
    }).collect();
    if renditions.is_empty() { return Err("HLS audio group has no external rendition"); }
    if renditions.iter().filter(|r| r.default).count() > 1 { return Err("Ambiguous default HLS audio"); }
    let default = renditions.iter().find(|r| r.default).unwrap_or(&renditions[0]).id;
    let subtitles = subtitle_candidates(base, text, subtitle_group.as_deref())?;
    Ok(Master { variant_url, renditions, default, subtitles })
}

fn subtitle_candidates(base: &str, text: &str, group: Option<&str>) -> Result<Vec<SubtitleRendition>, &'static str> {
    let Some(group) = group else { return Ok(Vec::new()) };
    let mut rows = Vec::new();
    let mut names = std::collections::BTreeSet::new();
    let mut duplicate = false;
    let mut next_id = 0u32;
    for raw in text.lines().filter_map(|line| line.trim().strip_prefix("#EXT-X-MEDIA:")) {
        let a = super::external_hls::attrs(raw)?;
        if a.get("TYPE") != Some(&"SUBTITLES") { continue; }
        next_id = next_id.checked_add(1).ok_or("Too many HLS subtitle candidates")?;
        if a.get("GROUP-ID").copied() != Some(group) { continue; }
        let name = a.get("NAME").copied().filter(|s| !s.is_empty()).unwrap_or("Subtitles");
        duplicate |= !names.insert(name);
        let reference = a.get("URI").ok_or("HLS subtitle URI is missing")
            .and_then(|reference| super::external_hls::uri(base, reference).map(|_| (*reference).to_string()));
        rows.push(SubtitleRendition { id: HlsSubtitleId(next_id), name: name.into(),
            language: a.get("LANGUAGE").copied().unwrap_or("").into(), reference });
    }
    if duplicate {
        for row in &mut rows { row.reference = Err("Ambiguous HLS subtitle candidate identity"); }
    }
    Ok(rows)
}

fn parse_vod(base: &str, text: &str) -> Result<Vod, &'static str> {
    let segments = super::external_hls::vod_segments(base, text)?;
    let mut out = Vec::new();
    let mut next_pdt = None;
    let mut explicit_pdt = None;
    let mut saw_inf = false;
    for line in text.lines().map(str::trim) {
        if line.starts_with("#EXT-X-DISCONTINUITY") || line.starts_with("#EXT-X-GAP")
            || line.starts_with("#EXT-X-MAP:") || line.starts_with("#EXT-X-BYTERANGE:")
            || line.starts_with("#EXT-X-START:") || line.starts_with("#EXT-X-I-FRAMES-ONLY")
            || line.starts_with("#EXT-X-PART:")
        { return Err("HLS presentation requires continuous full TS segments"); }
        if let Some(value) = line.strip_prefix("#EXT-X-PROGRAM-DATE-TIME:") {
            if explicit_pdt.is_some() { return Err("Duplicate HLS program date time"); }
            let pdt = super::hls_pdt::parse_program_date_time(value)?;
            let fraction_digits = value.split_once('.').map(|(_, tail)|
                tail.chars().take_while(|c| c.is_ascii_digit()).count()).unwrap_or(0);
            let precision = 10_i64.pow(9 - fraction_digits as u32);
            explicit_pdt = Some((pdt, precision));
        } else if line.starts_with("#EXTINF:") { saw_inf = true; }
        else if !line.is_empty() && !line.starts_with('#') {
            if !saw_inf { return Err("HLS segment duration is missing"); }
            saw_inf = false;
            let segment = segments.get(out.len()).ok_or("Invalid HLS segment table")?.clone();
            let explicit = explicit_pdt.take();
            let pdt = explicit.map(|(time, _)| time).or(next_pdt)
                .ok_or("HLS rendition requires PROGRAM-DATE-TIME")?;
            // EXTINF may be rounded. Epoch/continuity checks belong to the raw
            // packet clock, not a comparison against accumulated durations.
            let duration = i64::try_from(segment.duration.as_nanos()).map_err(|_| "HLS duration overflow")?;
            next_pdt = Some(pdt.checked_add(duration).ok_or("HLS date time overflow")?);
            out.push(AnchoredSegment { segment, pdt_ns: pdt,
                explicit_precision_ns: explicit.map(|(_, precision)| precision) });
        }
    }
    if out.len() != segments.len() || out.is_empty() || explicit_pdt.is_some() || saw_inf {
        return Err("Incomplete HLS anchored segment table");
    }
    Ok(Vod { url: base.into(), start_ns: out[0].pdt_ns, end_ns: next_pdt.unwrap(), segments: out })
}
fn validate_window(video: &Vod, audio: &Vod) -> Result<(), &'static str> {
    // The first VOD slice starts at a common explicit anchor. A partial/live
    // overlap requires a different priming policy and is intentionally refused.
    // Duration sums may be rounded. The joiner proves actual audio coverage
    // from packet timestamps before publishing each video interval.
    if video.start_ns != audio.start_ns {
        return Err("HLS renditions do not share the initial program date time");
    }
    Ok(())
}
fn fetch_vod(url: &str, cancelled: &AtomicBool) -> Result<Vod, &'static str> {
    let (landed, text) = super::external_hls::fetch(url, &mut super::external_hls::Owner::Inspect(cancelled))?;
    parse_vod(&landed, &text)
}
fn source_of(segment: &AnchoredSegment) -> ExternalSource {
    ExternalSource::Http(format!("{}{}", segment.segment.resource.origin.base(), segment.segment.resource.path))
}
fn prepare_audio(url: &str, cancelled: Arc<AtomicBool>) -> Result<(Vod, Binding), &'static str> {
    let media = fetch_vod(url, &cancelled)?;
    let info = super::inspect::raw_lane(&source_of(&media.segments[0]), Arc::clone(&cancelled), Lane::Audio)?;
    let track = info.tracks.audio.first().cloned().ok_or("Missing HLS AAC rendition")?;
    Ok((media, Binding::Audio(track)))
}
pub(super) fn inspect_master(source_url: &str, master_url: &str, text: &str,
    cancelled: Arc<AtomicBool>) -> Result<super::inspect::MediaInfo, &'static str>
{
    let master = parse_master(master_url, text)?;
    let video = fetch_vod(&master.variant_url, &cancelled)?;
    let (mut info, video_clock) = if !master.subtitles.iter().any(|row| row.reference.is_ok()) {
        (super::inspect::raw_lane(&source_of(&video.segments[0]), Arc::clone(&cancelled), Lane::Video)?, None)
    } else {
        let (info, timing) = super::inspect::raw_video_timed(&source_of(&video.segments[0]), Arc::clone(&cancelled))?;
        let mut clock = super::hls_join::LaneClock::default();
        let first = &video.segments[0];
        let accepted = timing.and_then(|raw| clock.map(raw, first.pdt_ns, video.start_ns, first.explicit_precision_ns))
            .is_ok() && clock.webvtt_clock().is_some();
        (info, accepted.then_some(clock))
    };
    let row = master.renditions.iter().find(|r| r.id == master.default).unwrap();
    let url = super::external_hls::uri(master_url, &row.url)?;
    let (audio, audio_binding) = prepare_audio(&url, cancelled)?;
    validate_window(&video, &audio)?;
    let Binding::Audio(track) = &audio_binding else { return Err("Invalid HLS audio binding"); };
    info.audio_codec = track.codec.clone();
    info.audio_channels = track.channels;
    info.audio_sample_rate = track.sample_rate;
    info.audio_bit_depth = track.bit_depth;
    info.duration_ns = video.end_ns.checked_sub(video.start_ns).ok_or("HLS duration overflow")?;
    info.tracks.mark_incomplete();
    info.resolved_source = Some(ExternalSource::Hls(source_url.into()));
    let presentation = HlsPresentation { source_url: source_url.into(), master_url: master_url.into(),
        variant_url: master.variant_url, video_binding: Binding::Video { width: info.width.into(), height: info.height.into() },
        video, renditions: master.renditions, subtitles: master.subtitles, video_clock };
    info.hls_playback = Some(HlsPlayback { presentation: Arc::new(presentation), selected: master.default, audio, audio_binding });
    Ok(info)
}

#[cfg(test)]
#[path = "hls_presentation_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "hls_subtitle_presentation_tests.rs"]
mod subtitle_tests;

#[cfg(any(test, feature = "test-support"))]
impl HlsPlayback {
    /// Synthetic route/menu contract only; performs no fetch and proves no decoder capability.
    pub fn fixture_for_test() -> Self {
        let source = plx_data::ott::http_url("https://example.invalid/master.m3u8")
            .expect("valid HLS fixture URL");
        let playlist = "#EXTM3U\n#EXT-X-TARGETDURATION:1\n#EXT-X-PROGRAM-DATE-TIME:2026-01-01T00:00:00Z\n#EXTINF:1,\n1.ts\n#EXT-X-ENDLIST\n";
        let video = parse_vod("https://example.invalid/video.m3u8", playlist).unwrap();
        let audio = parse_vod("https://example.invalid/audio.m3u8", playlist).unwrap();
        let track = super::inspect::InspectedTrack {
            kind: super::inspect::TrackKind::Audio, stream_index: 0, ordinal: 0,
            codec: "aac".into(), language: "eng".into(), title: String::new(), channels: 2,
            sample_rate: 48000, bit_depth: 0, decoder_available: true, disposition_known: false,
            default: false, forced: false,
        };
        let renditions = [("English", "eng"), ("Spanish", "spa")].into_iter().enumerate()
            .map(|(i, (name, lang))| Rendition { id: RenditionId(i as u32 + 1),
                name: name.into(), language: lang.into(), url: format!("https://example.invalid/{lang}.m3u8"), default: i == 0 }).collect();
        Self { presentation: Arc::new(HlsPresentation { source_url: source.clone(), master_url: source,
            variant_url: video.url.clone(), video, video_binding: Binding::Video { width: 160, height: 90 }, renditions,
            subtitles: Vec::new(), video_clock: None }),
            selected: RenditionId(1), audio, audio_binding: Binding::Audio(track) }
    }
    /// Synthetic menu/route contract only: no network and no real decoder proof.
    pub fn fixture_with_subtitles_for_test() -> Self {
        let mut playback = Self::fixture_for_test();
        let presentation = Arc::get_mut(&mut playback.presentation).expect("unshared fixture");
        use super::hls_rendition::{SourceTiming, TimedLaneSegment, TimedSample};
        let mut clock = super::hls_join::LaneClock::default();
        clock.map(TimedLaneSegment { binding: presentation.video_binding.clone(), samples: vec![TimedSample {
            data: Vec::new(), key: true,
            timing: SourceTiming { pts: 900_000, dts: Some(900_000), duration: Some(90_000), time_base: (1, 90_000) },
        }], transfer: SegmentTransfer { bytes: 0, active_us: 0, total_us: 0, audio_expected: false } },
            presentation.video.start_ns, presentation.video.start_ns, Some(1)).expect("synthetic video clock");
        presentation.video_clock = Some(clock);
        presentation.subtitles = [("English", "en"), ("Spanish", "es")].into_iter().enumerate().map(|(i, (name, language))|
            SubtitleRendition { id: HlsSubtitleId(i as u32 + 1), name: name.into(), language: language.into(),
                reference: Ok(format!("captions-{i}.m3u8")) }).collect();
        playback
    }
    /// Negative capability fixtures for actual menu/route consumers.
    pub fn unavailable_subtitle_fixtures_for_test() -> [Self; 2] {
        let mut no_clock = Self::fixture_with_subtitles_for_test();
        Arc::get_mut(&mut no_clock.presentation).expect("unshared fixture").video_clock = None;
        let mut invalid_uri = Self::fixture_with_subtitles_for_test();
        for row in &mut Arc::get_mut(&mut invalid_uri.presentation).expect("unshared fixture").subtitles {
            row.reference = Err("Invalid synthetic subtitle URI");
        }
        [no_clock, invalid_uri]
    }
    pub fn media_info_for_test(&self) -> super::inspect::MediaInfo {
        super::inspect::MediaInfo {
            resolved_source: None, hls_playback: Some(self.clone()), tracks: Default::default(),
            video_codec: "h264".into(), audio_codec: "aac".into(), width: 160, height: 90,
            fps: 24.0, duration_ns: 1_000_000_000, audio_channels: 2, audio_sample_rate: 48000,
            audio_bit_depth: 0, dovi: plx_data::metadata::Dovi::NONE,
        }
    }
    pub fn selected_for_test(&self, id: RenditionId) -> Self {
        let mut next = self.clone();
        let row = next.presentation.renditions.iter().find(|row| row.id == id).unwrap();
        next.selected = id;
        if let Binding::Audio(track) = &mut next.audio_binding { track.language = row.language.clone(); }
        next
    }
}
