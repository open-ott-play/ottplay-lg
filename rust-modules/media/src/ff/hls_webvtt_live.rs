//! Rolling WebVTT snapshots use only committed decoder clocks from this session.
//! No network activity or subtitle publication occurs while holding the timeline.
use super::*;
use std::sync::Mutex;

#[derive(Clone)]
struct Anchor {
    source: String,
    epoch: u64,
    revision: u64,
    clock: Clock,
    end_ns: i64,
}
#[derive(Clone)]
struct CachedSegment { sequence: u64, epoch: u64, url: String, text: String }
#[derive(Clone)]
struct Cache { revision: u64, url: String, refresh_at: Instant, segments: Vec<CachedSegment> }
#[derive(Default)]
pub(in crate::ff) struct Timeline {
    state: Mutex<(u64, Option<Anchor>)>,
    cache: Mutex<Option<Cache>>,
}
impl Timeline {
    pub(in crate::ff) fn invalidate(&self) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.0 = state.0.saturating_add(1);
        state.1 = None;
        drop(state);
        *self.cache.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }
    pub(in crate::ff) fn revision(&self) -> Option<u64> {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).1.as_ref().map(|a| a.revision)
    }
    pub(in crate::ff) fn commit(&self, source: &str, epoch: u64, clock: Option<Clock>, end_ns: i64) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        // A new epoch without an actual video timestamp invalidates the old clock.
        let changed = state.1.as_ref().is_none_or(|a| a.source != source || a.epoch != epoch
            || end_ns <= a.end_ns || clock.is_some_and(|clock| clock.presentation_ns < a.clock.presentation_ns));
        if changed { state.0 = state.0.saturating_add(1); }
        if let Some(clock) = clock {
            state.1 = Some(Anchor { source: source.into(), epoch, revision: state.0, clock, end_ns });
        } else if changed { state.1 = None; }
        else if let Some(anchor) = &mut state.1 { anchor.end_ns = end_ns; }
    }
    fn snapshot(&self) -> Option<Anchor> {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).1.clone()
    }
}

fn check(cancelled: &AtomicBool, deadline: Instant) -> Result<(), &'static str> {
    if cancelled.load(Ordering::Acquire) || Instant::now() >= deadline {
        Err("HLS subtitle preparation cancelled or timed out")
    } else { Ok(()) }
}

pub(in crate::ff) fn prepare(master: &str, reference: &str, timeline: &Timeline,
    cancelled: Arc<AtomicBool>) -> Result<(Vec<u8>, Option<u64>), &'static str>
{
    let deadline = Instant::now() + Duration::from_secs(15);
    let anchor = loop {
        check(&cancelled, deadline)?;
        if let Some(anchor) = timeline.snapshot() { break anchor; }
        std::thread::sleep(Duration::from_millis(25));
    };
    let url = external_hls::uri(master, reference)?;
    let previous = timeline.cache.lock().unwrap_or_else(|e| e.into_inner()).as_ref()
        .filter(|cache| cache.revision == anchor.revision && cache.url == url).cloned();
    let cache = if let Some(cache) = previous.as_ref().filter(|cache| Instant::now() < cache.refresh_at) {
        cache.clone()
    } else {
        let mut owner = Owner::Inspect(&cancelled);
        let (landed, text) = external_hls::fetch_until(&url, &mut owner, deadline)?;
        let playlist = external_hls::subtitle_window(&landed, &text)?;
        // Already grammar-validated above; use the publisher target for reload cadence.
        let target = text.lines().find_map(|line| line.trim().strip_prefix("#EXT-X-TARGETDURATION:")
            .and_then(|n| n.parse::<u64>().ok())).ok_or("Missing HLS target duration")?;
        let refresh_at = Instant::now() + (Duration::from_secs(target) / 2)
            .clamp(Duration::from_millis(250), Duration::from_secs(3));
        let mut segments = Vec::new();
        for (segment, epoch) in playlist.into_iter().rev().take(12).rev() {
            check(&cancelled, deadline)?;
            if epoch != anchor.epoch { continue; }
            let resource = format!("{}{}", segment.resource.origin.base(), segment.resource.path);
            let old = previous.as_ref().and_then(|cache| cache.segments.iter()
                .find(|old| old.sequence == segment.sequence && old.epoch == epoch));
            if old.is_some_and(|old| old.url != resource) { return Err("HLS subtitle overlap changed"); }
            let text = if let Some(old) = old { old.text.clone() }
                else { external_hls::fetch_until(&resource, &mut owner, deadline)?.1 };
            let bytes: usize = segments.iter().map(|s: &CachedSegment| s.text.len()).sum();
            if bytes.checked_add(text.len()).is_none_or(|n| n > TEXT_BYTES) {
                return Err("HLS subtitle text exceeds sidecar limit");
            }
            segments.push(CachedSegment { sequence: segment.sequence, epoch, url: resource, text });
        }
        Cache { revision: anchor.revision, url, refresh_at, segments }
    };
    let mut assembly = Assembly::new(anchor.clock);
    // Playlist sequence numbers are not shared between video and subtitles.
    // Timestamp maps and matching discontinuity epochs establish the relation.
    // Retain a bounded rolling interval around the decoder cursor, including
    // nearby future cues; filter old playlist cues before assembling output.
    let window = Window { start_ns: anchor.end_ns.saturating_sub(60 * 1_000_000_000),
        end_ns: anchor.end_ns.saturating_add(60 * 1_000_000_000) };
    for segment in &cache.segments {
        check(&cancelled, deadline)?;
        push_window(&mut assembly, &segment.text, window, &mut ||
            cancelled.load(Ordering::Acquire) || Instant::now() >= deadline)?;
    }
    check(&cancelled, deadline)?;
    let current = timeline.snapshot().ok_or("HLS subtitle video clock was invalidated")?;
    if current.revision != anchor.revision || current.epoch != anchor.epoch || current.source != anchor.source {
        return Err("HLS subtitle video epoch changed during preparation");
    }
    let bytes = assembly.finish_cancellable(&mut || cancelled.load(Ordering::Acquire) || Instant::now() >= deadline)?;
    *timeline.cache.lock().unwrap_or_else(|e| e.into_inner()) = Some(cache);
    Ok((bytes, Some(anchor.revision)))
}

fn push_window(assembly: &mut Assembly, text: &str, window: Window, stop: &mut dyn FnMut() -> bool)
    -> Result<(), &'static str>
{
    assembly.input_bytes = assembly.input_bytes.checked_add(text.len()).filter(|n| *n <= TEXT_BYTES)
        .ok_or("HLS subtitle text exceeds sidecar limit")?;
    let (map, cues) = parse_cancellable(text, stop)?;
    for cue in cues {
        if stop() { return Err("HLS subtitle preparation cancelled or timed out"); }
        let (start, end) = match mapped(assembly.clock, map, &cue, window) {
            Ok(interval) => interval,
            Err("WebVTT cue does not cover its declared segment") => continue,
            Err(error) => return Err(error),
        };
        if end <= 0 { continue; }
        let key = (start.max(0), end, cue.id, cue.settings, cue.text);
        if !assembly.cues.contains(&key) {
            assembly.retained_bytes = assembly.retained_bytes.checked_add(
                key.2.as_ref().map_or(0, String::len) + key.3.len() + key.4.len() + 96)
                .filter(|n| *n <= TEXT_BYTES).ok_or("HLS subtitle output exceeds sidecar limit")?;
            if assembly.cues.len() >= CUE_LIMIT { return Err("WebVTT cue limit exceeded"); }
            assembly.cues.insert(key);
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "hls_webvtt_live_tests.rs"]
mod tests;
