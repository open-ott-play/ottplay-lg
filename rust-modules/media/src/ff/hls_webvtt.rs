//! Bounded external VOD WebVTT preparation. Parsing/mapping own no player state;
//! the caller admits the finished sidecar under its existing request identity.
use super::external_hls::{self, Owner};
use crate::player::text_ass;
use std::collections::BTreeSet;
use std::sync::{atomic::{AtomicBool, Ordering}, Arc};
use std::time::{Duration, Instant};

const TEXT_BYTES: usize = plx_plex::plex::SIDECAR_MAX_BYTES;
const CUE_LIMIT: usize = 32_768;
const SEGMENT_LIMIT: usize = 2_048;
const NS: i128 = 1_000_000_000;
const RATE: i128 = 90_000;
const WRAP: i128 = 1_i128 << 33;

/// Actual video PES clock, mapped by the same presentation clock as the decoder.
/// A source without this evidence must not guess from a playlist date or EXTINF.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Clock {
    pub(super) pts_90k: i64,
    pub(super) presentation_ns: i64,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct TimestampMap { local_ns: i64, mpegts: u64 }
#[derive(Clone, Copy)]
struct Window { start_ns: i64, end_ns: i64 }

fn timestamp(value: &str) -> Result<i64, &'static str> {
    let (whole, millis) = value.split_once('.').ok_or("Invalid WebVTT timestamp")?;
    if millis.len() != 3 || !millis.bytes().all(|b| b.is_ascii_digit()) {
        return Err("Invalid WebVTT timestamp");
    }
    let fields: Vec<_> = whole.split(':').collect();
    if !(2..=3).contains(&fields.len()) || fields.iter().any(|p| p.len() < 2 || !p.bytes().all(|b| b.is_ascii_digit())) {
        return Err("Invalid WebVTT timestamp");
    }
    for part in &fields[fields.len() - 2..] {
        if part.len() != 2 || part.parse::<u8>().map_err(|_| "Invalid WebVTT timestamp")? >= 60 {
            return Err("Invalid WebVTT timestamp");
        }
    }
    text_ass::timestamp(value).ok_or("WebVTT timestamp overflow")
}
fn timestamp_map(value: &str) -> Result<TimestampMap, &'static str> {
    let mut local = None;
    let mut mpegts = None;
    for attr in value.split(',') {
        if let Some(v) = attr.strip_prefix("LOCAL:") {
            if local.replace(timestamp(v)?).is_some() { return Err("Duplicate WebVTT LOCAL"); }
        } else if let Some(v) = attr.strip_prefix("MPEGTS:") {
            if v.is_empty() || !v.bytes().all(|b| b.is_ascii_digit()) { return Err("Invalid WebVTT MPEGTS"); }
            let ticks = v.parse::<u64>().map_err(|_| "Invalid WebVTT MPEGTS")?;
            if i128::from(ticks) >= WRAP || mpegts.replace(ticks).is_some() { return Err("Invalid WebVTT MPEGTS"); }
        } else { return Err("Unknown WebVTT timestamp map attribute"); }
    }
    Ok(TimestampMap { local_ns: local.ok_or("Missing WebVTT LOCAL")?, mpegts: mpegts.ok_or("Missing WebVTT MPEGTS")? })
}

fn percent(value: &str) -> bool {
    value.strip_suffix('%').is_some_and(|number| !number.is_empty()
        && number.bytes().all(|b| b.is_ascii_digit() || b == b'.')
        && number.parse::<f64>().is_ok_and(|n| (0.0..=100.0).contains(&n)))
}
fn settings(value: &str) -> Result<String, &'static str> {
    let mut fields = std::collections::BTreeMap::new();
    for field in value.split_whitespace() {
        let (name, value) = field.split_once(':').ok_or("Invalid WebVTT cue setting")?;
        let (position, alignment) = value.split_once(',').map(|(a, b)| (a, Some(b))).unwrap_or((value, None));
        let valid = match name {
            "line" => (position == "auto" || percent(position)
                || position.strip_prefix('-').unwrap_or(position).bytes().all(|b| b.is_ascii_digit()) && position.parse::<i64>().is_ok())
                && alignment.is_none_or(|a| ["start", "center", "end"].contains(&a)),
            "position" => percent(position) && alignment.is_none_or(|a| ["line-left", "center", "line-right", "auto"].contains(&a)),
            "size" => percent(position) && alignment.is_none(),
            "align" => ["start", "center", "end", "left", "right"].contains(&value),
            _ => false,
        };
        if !valid || fields.insert(name, value).is_some() { return Err("Unsupported or invalid WebVTT cue setting"); }
    }
    Ok(fields.into_iter().map(|(k, v)| format!("{k}:{v}")).collect::<Vec<_>>().join(" "))
}

#[cfg(test)]
fn parse(text: &str) -> Result<(TimestampMap, Vec<text_ass::FileCue>), &'static str> {
    parse_cancellable(text, &mut || false)
}
fn parse_cancellable(text: &str, stop: &mut dyn FnMut() -> bool)
    -> Result<(TimestampMap, Vec<text_ass::FileCue>), &'static str>
{
    if stop() { return Err("HLS subtitle preparation cancelled or timed out"); }
    if text.len() > TEXT_BYTES || text.chars().any(|c| c.is_control() && !matches!(c, '\t' | '\r' | '\n')) {
        return Err("Invalid WebVTT segment");
    }
    let text = text.trim_start_matches('\u{feff}').replace("\r\n", "\n").replace('\r', "\n");
    let (header, body) = match text.split_once("\n\n") {
        Some(parts) => parts,
        None if !text.contains("-->") => (text.trim_end_matches('\n'), ""),
        None => return Err("WebVTT header is missing its separator"),
    };
    let mut lines = header.lines();
    let first = lines.next().ok_or("Missing WebVTT header")?;
    if !(first == "WEBVTT" || first.starts_with("WEBVTT ") || first.starts_with("WEBVTT\t")) || first.contains("-->") {
        return Err("Missing WebVTT header");
    }
    let mut map = None;
    for line in lines {
        if let Some(raw) = line.strip_prefix("X-TIMESTAMP-MAP=") {
            if map.replace(timestamp_map(raw)?).is_some() { return Err("Duplicate WebVTT timestamp map"); }
        } else {
            return Err("Unsupported or invalid WebVTT metadata header");
        }
    }
    let mut count = 0usize;
    for block in body.split("\n\n").filter(|b| !b.trim().is_empty()) {
        if stop() { return Err("HLS subtitle preparation cancelled or timed out"); }
        let mut lines = block.lines();
        let first = lines.next().ok_or("Invalid WebVTT cue")?;
        if first == "NOTE" || first.starts_with("NOTE ") || first.starts_with("NOTE\t") { continue; }
        if first == "STYLE" || first == "REGION" { return Err("WebVTT style and region blocks are unsupported"); }
        let timing = if first.contains("-->") { first } else {
            lines.next().ok_or("WebVTT cue timing is missing")?
        };
        let (start, tail) = timing.split_once("-->").ok_or("Invalid WebVTT cue timing")?;
        let end = tail.split_whitespace().next().ok_or("Invalid WebVTT cue timing")?;
        if timestamp(end)? <= timestamp(start.trim())? { return Err("Invalid WebVTT cue interval"); }
        let (_, _, raw_settings) = text_ass::timing_line_settings(timing).ok_or("Invalid WebVTT cue timing")?;
        settings(raw_settings)?;
        if lines.clone().any(|line| line.contains("-->") || line.starts_with("X-TIMESTAMP-MAP")) {
            return Err("Invalid WebVTT cue text");
        }
        if lines.next().is_none() { return Err("WebVTT cue text is missing"); }
        count += 1;
        if count > CUE_LIMIT { return Err("WebVTT cue limit exceeded"); }
    }
    if stop() { return Err("HLS subtitle preparation cancelled or timed out"); }
    let mut cues = text_ass::read_cues(body);
    for cue in &mut cues {
        if stop() { return Err("HLS subtitle preparation cancelled or timed out"); }
        cue.settings = settings(&cue.settings)?;
    }
    if cues.len() != count { return Err("WebVTT cue parse mismatch"); }
    // RFC 8216 section 3.5: absent mapping means LOCAL 0 maps to MPEGTS 0.
    Ok((map.unwrap_or(TimestampMap { local_ns: 0, mpegts: 0 }), cues))
}

fn mapped(clock: Clock, map: TimestampMap, cue: &text_ass::FileCue, window: Window)
    -> Result<(i64, i64), &'static str>
{
    // Keep the fractional 90k tick in an integer numerator until final ns conversion.
    let start = i128::from(map.mpegts) * NS + (i128::from(cue.start_ns) - i128::from(map.local_ns)) * RATE;
    let end = i128::from(map.mpegts) * NS + (i128::from(cue.end_ns) - i128::from(map.local_ns)) * RATE;
    let anchor = i128::from(clock.pts_90k) * NS;
    let wanted = (i128::from(window.start_ns) - i128::from(clock.presentation_ns)) * RATE;
    let turn = (wanted - (start - anchor)).div_euclid(WRAP * NS);
    let mut answer = None;
    for candidate in [turn - 1, turn, turn + 1] {
        let shift = candidate * WRAP * NS - anchor;
        let a = i128::from(clock.presentation_ns) + (start + shift).div_euclid(RATE);
        let b = i128::from(clock.presentation_ns) + (end + shift).div_euclid(RATE);
        if a < i128::from(window.end_ns) && b > i128::from(window.start_ns) {
            if answer.is_some() { return Err("Ambiguous WebVTT timestamp wrap"); }
            answer = Some((i64::try_from(a).map_err(|_| "WebVTT timeline overflow")?,
                i64::try_from(b).map_err(|_| "WebVTT timeline overflow")?));
        }
    }
    answer.ok_or("WebVTT cue does not cover its declared segment")
}
fn stamp(ns: i64) -> String {
    let ms = ns / 1_000_000;
    format!("{:02}:{:02}:{:02}.{:03}", ms / 3_600_000, ms / 60_000 % 60, ms / 1000 % 60, ms % 1000)
}
struct Assembly {
    clock: Clock,
    cues: BTreeSet<(i64, i64, Option<String>, String, String)>,
    input_bytes: usize,
    retained_bytes: usize,
}
impl Assembly {
    fn new(clock: Clock) -> Self { Self { clock, cues: BTreeSet::new(), input_bytes: 0, retained_bytes: 0 } }
    #[cfg(test)]
    fn push(&mut self, text: &str, window: Window) -> Result<(), &'static str> {
        self.push_cancellable(text, window, &mut || false)
    }
    fn push_cancellable(&mut self, text: &str, window: Window, stop: &mut dyn FnMut() -> bool)
        -> Result<(), &'static str>
    {
        self.input_bytes = self.input_bytes.checked_add(text.len()).filter(|n| *n <= TEXT_BYTES)
            .ok_or("HLS subtitle text exceeds sidecar limit")?;
        let (map, cues) = parse_cancellable(text, stop)?;
        for cue in cues {
            if stop() { return Err("HLS subtitle preparation cancelled or timed out"); }
            let (start, end) = mapped(self.clock, map, &cue, window)?;
            if end <= 0 { continue; }
            let key = (start.max(0), end, cue.id, cue.settings, cue.text);
            if !self.cues.contains(&key) {
                self.retained_bytes = self.retained_bytes.checked_add(key.2.as_ref().map_or(0, String::len) + key.3.len() + key.4.len() + 96)
                    .filter(|n| *n <= TEXT_BYTES).ok_or("HLS subtitle output exceeds sidecar limit")?;
                if self.cues.len() >= CUE_LIMIT { return Err("WebVTT cue limit exceeded"); }
                self.cues.insert(key);
            }
        }
        Ok(())
    }
    #[cfg(test)]
    fn finish(self) -> Result<Vec<u8>, &'static str> {
        self.finish_cancellable(&mut || false)
    }
    fn finish_cancellable(self, stop: &mut dyn FnMut() -> bool) -> Result<Vec<u8>, &'static str> {
        if stop() { return Err("HLS subtitle preparation cancelled or timed out"); }
        let mut out = String::from("WEBVTT\n\n");
        for (start, end, id, settings, text) in self.cues {
            if stop() { return Err("HLS subtitle preparation cancelled or timed out"); }
            if let Some(id) = id { out.push_str(&id); out.push('\n'); }
            out.push_str(&stamp(start)); out.push_str(" --> "); out.push_str(&stamp(end));
            if !settings.is_empty() { out.push(' '); out.push_str(&settings); }
            out.push('\n'); out.push_str(&text); out.push_str("\n\n");
        }
        if out.len() > TEXT_BYTES { return Err("HLS subtitle output exceeds sidecar limit"); }
        Ok(out.into_bytes())
    }
}

fn segments(base: &str, text: &str) -> Result<Vec<crate::hls::Segment>, &'static str> {
    if text.lines().any(|line| line.trim().starts_with("#EXT-X-DISCONTINUITY")
        || line.trim().starts_with("#EXT-X-GAP") || line.trim().starts_with("#EXT-X-START:")) {
        return Err("HLS subtitles require continuous VOD segments");
    }
    external_hls::vod_segments_bounded(base, text, SEGMENT_LIMIT, TEXT_BYTES)
}
/// Worker-only: no active player state, no selected-track mutation, no secret logging.
pub(super) fn prepare(master: &str, reference: &str, clock: Clock, cancelled: Arc<AtomicBool>)
    -> Result<Vec<u8>, &'static str>
{
    let deadline = Instant::now() + Duration::from_secs(60);
    let url = external_hls::uri(master, reference)?;
    let mut owner = Owner::Inspect(&cancelled);
    let (landed, text) = external_hls::fetch_until(&url, &mut owner, deadline)?;
    let playlist = segments(&landed, &text)?;
    let mut assembly = Assembly::new(clock);
    let mut start_ns = 0i64;
    for segment in playlist {
        if cancelled.load(Ordering::Relaxed) || Instant::now() >= deadline { return Err("HLS subtitle preparation cancelled or timed out"); }
        let end_ns = start_ns.checked_add(i64::try_from(segment.duration.as_nanos()).map_err(|_| "HLS subtitle duration overflow")?)
            .ok_or("HLS subtitle duration overflow")?;
        let url = format!("{}{}", segment.resource.origin.base(), segment.resource.path);
        let (_, text) = external_hls::fetch_until(&url, &mut owner, deadline)?;
        assembly.push_cancellable(&text, Window { start_ns, end_ns }, &mut ||
            cancelled.load(Ordering::Relaxed) || Instant::now() >= deadline)?;
        start_ns = end_ns;
    }
    if cancelled.load(Ordering::Relaxed) || Instant::now() >= deadline { return Err("HLS subtitle preparation cancelled or timed out"); }
    assembly.finish_cancellable(&mut || cancelled.load(Ordering::Relaxed) || Instant::now() >= deadline)
}

#[cfg(test)]
#[path = "hls_webvtt_tests.rs"]
mod tests;

#[path = "hls_webvtt_live.rs"]
pub(super) mod live;
