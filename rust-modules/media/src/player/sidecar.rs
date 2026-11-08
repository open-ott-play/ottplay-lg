//! **External (sidecar) subtitles on the direct-play path** — the `.srt` beside the film.
//!
//! The demuxer only ever sees the streams INSIDE the container it opened, so a sidecar was
//! unreachable on direct play and the track menu hid it (`docs/parity-gaps.md`, "Sidecar
//! (external) subtitle streams are unreachable on the direct-play path"). This is the missing
//! Plex producer: fetch the whole file from PMS once. Local adjacent files instead arrive from
//! the application's authorized-root metadata worker through [`install_local`]. Complete HLS
//! WebVTT timelines arrive from the route-owned preparation worker through [`install_hls`];
//! all three use the same store and renderer. ASS/SSA keeps its complete script for
//! [`ass_source`] and the native styled renderer (styles, overlapping events, drawings and embedded
//! fonts must never pass through a text parser). SubRip and WebVTT are converted on the fetch worker
//! to an ASS script of the viewer's synthesized style (`text_ass::script_from_srt`/`_vtt`, charset
//! by `sub_charset`), and render through the same libass worker as an embedded text track; the
//! script is restyled lazily when the viewer's size or position pick changes.
//!
//! # Why its own store, and not `ass_source`
//!
//! That store is a WINDOW the demuxer refills as it reads, and a backward seek empties it. A
//! sidecar arrives once, whole — 1,000-2,000 cues for a feature — and nothing re-reads it after a
//! seek. The converted script is kept intact (a long one is fed to libass a window at a time,
//! `ass_script`), and the clock a frame is rendered for is the screen's.
//!
//! # The clock
//!
//! A sidecar's timestamps are file time, and on DIRECT PLAY `playpos_ns` is file time too (the
//! rebase keeps it so across seeks). Where the server BURNS the selection into the picture (a
//! re-encode, or a remux the app may not draw over) [`ass_source`] answers nothing, and the caller
//! draws no [`notice`], while it says the selection is burned (`route::subtitles_burned`) — otherwise a direct play that becomes
//! a burn mid-film (a DTS audio pick) would show the line twice. Over a plain remux nothing is
//! burned and a sidecar is drawn from here. The fact is PASSED IN rather than read: the playback session is
//! the frame's publication, and the draw that calls this already holds it.
//!
//! The renderer is asked on the SUBTITLE clock (`player::subtitle_clock_ns`, the playhead less the
//! viewer's timing offset). Because the whole file is here, a sidecar is the one kind of track
//! that can be ADVANCED as well as delayed — [`selected`] is what `player::subtitle_offset_range_ms`
//! reads to offer it the sidecar offset range (see `subtitle_offset_range_ms`), where an embedded track
//! gets a delay only.
//!
//! # Threading
//!
//! [`select`]/[`deselect`]/[`active`]/[`ass_source`] are main-thread calls; the Plex fetch runs on a `task` worker and
//! lands under the one mutex, fenced by a generation so an answer for a pick the viewer has
//! already moved off is dropped rather than installed. One worker drains a single latest-pick
//! Plex slot, so repeated picks cannot accumulate threads or downloads. Local requests remain
//! in `Pending::Local` until the application takes them for its existing metadata worker; the
//! Plex worker cannot consume them. The application rechecks source authority before publishing,
//! and [`install_local`] rechecks the selected request before accepting the prepared content.
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use super::text_ass;
use super::text_style::TextStyle;
use crate::route::{LocalSubtitleTarget, PlaybackIdentity};
use crate::ff::hls_presentation::HlsSubtitleId;
use plx_base::task::OffFrame;

/// Why nothing is being drawn for a selected sidecar — said ON SCREEN for a few seconds, in the
/// caption's own place, because a picked subtitle that silently shows nothing is
/// indistinguishable from a film with a quiet first minute.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Failure {
    Fetch,
    Empty,
}

impl Failure {
    fn message(self) -> &'static str {
        match self {
            Failure::Fetch => plx_platform::i18n::msg::widgets_sidecar_fetch_failed(),
            Failure::Empty => plx_platform::i18n::msg::widgets_sidecar_empty(),
        }
    }
}

/// Owned local files use only their opaque app-validated target and playback identity.
/// A server stream keeps its server/key identity; neither kind can alias the other.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Selection {
    Plex { server: plx_plex::plex::ServerId, stream_id: i64, key: String, codec: String },
    Local { playback: PlaybackIdentity, target: LocalSubtitleTarget, format: SidecarFormat },
    Hls { playback: PlaybackIdentity, id: HlsSubtitleId },
}
impl Selection {
    fn is_local(&self) -> bool { matches!(self, Self::Local { .. }) }
    fn is_hls(&self) -> bool { matches!(self, Self::Hls { .. }) }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SidecarFormat { Srt, Vtt, Ass, Ssa }
impl SidecarFormat {
    pub fn codec(self) -> &'static str {
        match self { Self::Srt => "srt", Self::Vtt => "vtt", Self::Ass => "ass", Self::Ssa => "ssa" }
    }
}

/// The existing decoder's raw-byte limit. No second Local budget is introduced.
pub fn owned_byte_limit() -> usize { plx_plex::plex::SIDECAR_MAX_BYTES }

/// A selected Local read, not filesystem authority. The app must validate its
/// current source/volume binding before taking, reading and publishing this request.
#[derive(Clone)]
pub struct LocalReadRequest {
    generation: u64,
    playback: PlaybackIdentity,
    target: LocalSubtitleTarget,
    format: SidecarFormat,
    style: TextStyle,
}
impl std::fmt::Debug for LocalReadRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { f.write_str("LocalReadRequest(..)") }
}
impl LocalReadRequest {
    pub fn playback(&self) -> &PlaybackIdentity { &self.playback }
    pub fn target(&self) -> &LocalSubtitleTarget { &self.target }
    pub fn format(&self) -> SidecarFormat { self.format }
    fn selection(&self) -> Selection {
        Selection::Local { playback: self.playback.clone(), target: self.target.clone(), format: self.format }
    }
    fn same_request(&self, other: &Self) -> bool {
        self.generation == other.generation && self.selection() == other.selection()
    }
    /// Conversion runs on the existing app metadata worker, after protected reading.
    pub fn prepare_owned(&self, _off: &OffFrame, bytes: Vec<u8>) -> Result<PreparedLocal, LocalFailure> {
        decode(bytes, self.format.codec(), self.style)
            .map(|content| PreparedLocal { request: self.clone(), content })
            .map_err(|why| LocalFailure { request: self.clone(), why, at: Instant::now() })
    }
    /// Construct the existing short-lived failure notice on the worker, not the frame.
    pub fn read_failed(&self, _off: &OffFrame) -> LocalFailure {
        LocalFailure { request: self.clone(), why: Failure::Fetch, at: Instant::now() }
    }
}

/// Sealed conversion result: cannot be retargeted to a different pick.
pub struct PreparedLocal { request: LocalReadRequest, content: Content }
/// Sealed failure, bound to the same request and sampled by its worker.
pub struct LocalFailure { request: LocalReadRequest, why: Failure, at: Instant }
impl std::fmt::Debug for LocalFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { f.write_str("LocalFailure(..)") }
}

enum Pending {
    Plex(u64, Selection, TextStyle),
    Local(LocalReadRequest),
}

/// A converted, complete HLS WebVTT timeline, bound to its logical playback and
/// rendition. Preparation grants no right to publish: the route must still check
/// its presentation identity, user revision and worker ticket at commit time.
pub struct PreparedHls {
    playback: PlaybackIdentity,
    id: HlsSubtitleId,
    content: Content,
}
impl std::fmt::Debug for PreparedHls {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("PreparedHls(..)")
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HlsPrepareError { InvalidContent, Cancelled }

/// Convert the backend's strict, already timeline-mapped WebVTT off-frame. No
/// selection changes here, so a failed download/conversion preserves its predecessor.
/// Completed timelines retain overlapping cue intervals and may be empty;
/// ordinary sidecar decoding keeps its existing newest-wins and nonempty rules.
pub fn prepare_hls(_off: &OffFrame, playback: PlaybackIdentity, id: HlsSubtitleId,
    bytes: Vec<u8>, style: TextStyle, should_stop: &mut dyn FnMut() -> bool,
) -> Result<PreparedHls, HlsPrepareError> {
    if should_stop() { return Err(HlsPrepareError::Cancelled); }
    if bytes.len() > owned_byte_limit() { return Err(HlsPrepareError::InvalidContent); }
    let text = std::str::from_utf8(&bytes).map_err(|_| HlsPrepareError::InvalidContent)?;
    let text = text.trim_start_matches('\u{feff}');
    let first_line = text.lines().next().unwrap_or("");
    if first_line != "WEBVTT" && !first_line.starts_with("WEBVTT ") && !first_line.starts_with("WEBVTT\t") {
        return Err(HlsPrepareError::InvalidContent);
    }
    // Bound the actual ASS result, including the synthesized header, before
    // joining. Text escaping can make output larger than the canonical input.
    let header = text_ass::header(&style);
    let budget = owned_byte_limit().checked_sub(header.len())
        .ok_or(HlsPrepareError::InvalidContent)?;
    let mut cancelled = false;
    let converted = {
        let mut stop = || { cancelled |= should_stop(); cancelled };
        text_ass::script_from_vtt_timeline_bounded(text, budget, &mut stop)
    };
    if cancelled || should_stop() { return Err(HlsPrepareError::Cancelled); }
    let converted = converted.map_err(|_| HlsPrepareError::InvalidContent)?;
    if header.len().checked_add(converted.events.len())
        .is_none_or(|len| len > owned_byte_limit()) {
        return Err(HlsPrepareError::InvalidContent);
    }
    drop(bytes);
    // Only the WebVTT converter is used: authored-ASS detection cannot change
    // the interpretation of a canonical HLS cue payload.
    let content = content_from_text(converted, header, style);
    Ok(PreparedHls { playback, id, content })
}

/// MAIN THREAD, inside the route's guarded commit. The sealed result cannot be
/// retargeted. It does not carry the route's revision ticket: that remains the
/// caller's admission responsibility, including Off or a newer same-target pick.
pub fn install_hls(playback: &PlaybackIdentity, id: HlsSubtitleId, prepared: PreparedHls) -> bool {
    if prepared.playback != *playback || prepared.id != id { return false; }
    let selection = Selection::Hls { playback: playback.clone(), id };
    let mut st = state();
    st.generation = st.generation.wrapping_add(1);
    st.pending = None;
    st.want = Some(selection.clone());
    st.failed = None;
    st.loaded = Some((selection, prepared.content));
    // A previous Plex worker retains its occupancy until it actually finishes;
    // the advanced generation prevents its result from replacing this selection.
    true
}

pub fn selected_hls(playback: &PlaybackIdentity) -> Option<HlsSubtitleId> {
    match &state().want {
        Some(Selection::Hls { playback: selected, id }) if selected == playback => Some(*id),
        _ => None,
    }
}

/// Retire only HLS content on Off/source loss. Ordinary seeks and reloads retain
/// the whole mapped timeline. A new logical item still uses the common reset().
pub fn reset_hls() {
    let mut st = state();
    if st.want.as_ref().is_some_and(Selection::is_hls) {
        st.generation = st.generation.wrapping_add(1);
        st.want = None;
    }
    if st.loaded.as_ref().is_some_and(|(selection, _)| selection.is_hls()) { st.loaded = None; }
    if st.failed.as_ref().is_some_and(|(selection, _, _)| selection.is_hls()) { st.failed = None; }
}

/// A loaded file as the renderer takes it.
struct Content {
    source: Arc<super::ass::Source>,
    font_generation: Option<u64>,
    /// Set for a converted SubRip/WebVTT file, whose header follows the viewer's picks.
    text: Option<TextHold>,
}

/// What a converted file needs to be restyled: the script is the synthesized header followed by
/// the converted `Dialogue:` lines, so the events are the bytes past `header_len` and are not kept
/// twice.
struct TextHold {
    header_len: usize,
    /// The picks (`TextStyle::generation`) the held header was synthesized for.
    style_generation: u32,
    /// Cues that survived conversion.
    cues: usize,
    /// A worker is already building the script for newer picks (one at a time; a later pick that
    /// lands meanwhile is taken up by the next call once this one has published).
    restyling: bool,
}

struct State {
    want: Option<Selection>,
    generation: u64,
    pending: Option<Pending>,
    running: bool,
    /// The complete selected file, kept across Off→On so re-picking is instant.
    loaded: Option<(Selection, Content)>,
    // Worker failures have a sampled timestamp. A refused app-worker launch is
    // terminal too, but carries no fabricated frame-thread clock sample.
    failed: Option<(Selection, Failure, Option<Instant>)>,
}

static STATE: Mutex<State> = Mutex::new(State {
    want: None,
    generation: 0,
    pending: None,
    running: false,
    loaded: None,
    failed: None,
});

/// Signalled, under [`STATE`], whenever the download worker goes idle or a restyle worker has
/// finished: what the tests wait on instead of the clock.
static SETTLED: std::sync::Condvar = std::sync::Condvar::new();

/// How long a [`Failure`] stays on screen.
const FAILURE_SHOWN: Duration = Duration::from_secs(6);
fn state() -> std::sync::MutexGuard<'static, State> {
    STATE.lock().unwrap_or_else(|e| e.into_inner())
}

/// Select the sidecar `stream_id`, fetching `key` from `server` unless that file is already
/// loaded. MAIN THREAD.
pub fn select(server: plx_plex::plex::ServerId, stream_id: i64, key: String, codec: String, style: TextStyle) {
    select_with_fetch(server, stream_id, key, codec, style, |server, key, codec| {
        let Some(client) = plx_plex::plex::client_for(server) else {
            super::log("sidecar: no client for the playing server");
            return None;
        };
        client.sidecar_subtitle_detailed(key, codec)
            .map_err(|why| super::log(&format!("sidecar: fetch failed: {why}")))
            .ok()
    });
}

fn select_with_fetch(
    server: plx_plex::plex::ServerId, stream_id: i64, key: String, codec: String,
    style: TextStyle,
    fetch: impl Fn(plx_plex::plex::ServerId, &str, &str) -> Option<Vec<u8>> + Send + 'static,
) {
    if stream_id <= 0 || key.is_empty() {
        deselect();
        return;
    }
    let selection = Selection::Plex { server, stream_id, key, codec: codec.to_ascii_lowercase() };
    {
        let mut st = state();
        st.generation = st.generation.wrapping_add(1);
        st.want = Some(selection.clone());
        st.failed = None;
        st.pending = None;
        if matches!(&st.loaded, Some((source, _)) if *source == selection) {
            return;
        }
        st.loaded = None;
        st.pending = Some(Pending::Plex(st.generation, selection.clone(), style));
        if st.running { return; }
        st.running = true;
    }
    let spawned = plx_base::task::spawn_small("sidecar", move || loop {
        let (gen, selection, style) = {
            let mut st = state();
            // A Local request belongs to the app's existing metadata worker. Do
            // not take it or pretend the previous Plex worker has finished early.
            if !matches!(st.pending, Some(Pending::Plex(..))) {
                st.running = false;
                SETTLED.notify_all();
                return;
            }
            let Some(Pending::Plex(gen, selection, style)) = st.pending.take() else { unreachable!() };
            (gen, selection, style)
        };
        let Selection::Plex { server, stream_id, key, codec } = &selection else { unreachable!() };
        super::log(&format!("sidecar: fetching stream {stream_id}"));
        let body = fetch(*server, key, codec);
        let outcome = match body {
            None => Err(Failure::Fetch),
            Some(bytes) => decode(bytes, codec, style),
        };
        let mut st = state();
        if st.generation != gen {
            continue; // check and publication share the selection/reset lock
        }
        match outcome {
            Ok(content) => {
                match &content.text {
                    Some(hold) => super::log(&format!("sidecar: stream {stream_id} ready, {} cues", hold.cues)),
                    None => super::log(&format!("sidecar: stream {stream_id} ready, styled ASS")),
                }
                st.loaded = Some((selection, content));
            }
            Err(why) => {
                super::log(&format!("sidecar: stream {stream_id} FAILED ({why:?})"));
                st.failed = Some((selection, why, Some(Instant::now())));
            }
        }
        plx_machine::present::wake_from_worker();
    });
    if !spawned {
        let mut st = state();
        st.running = false;
        st.pending = None;
        st.failed = Some((selection, Failure::Fetch, Some(Instant::now())));
        SETTLED.notify_all();
    }
}

/// Route-only commit after ordinary track-choice admission. Never starts a worker.
pub(crate) fn select_local(playback: PlaybackIdentity, target: LocalSubtitleTarget, format: SidecarFormat, style: TextStyle) {
    let mut st = state();
    st.generation = st.generation.wrapping_add(1);
    let request = LocalReadRequest { generation: st.generation, playback, target, format, style };
    let selection = request.selection();
    st.want = Some(selection.clone());
    st.failed = None;
    st.pending = None;
    if matches!(&st.loaded, Some((held, _)) if *held == selection) { return; }
    st.loaded = None;
    st.pending = Some(Pending::Local(request));
}

/// Inspect before take so the app can prove the opaque target's current authority.
pub fn peek_local_request() -> Option<LocalReadRequest> {
    match &state().pending { Some(Pending::Local(request)) => Some(request.clone()), _ => None }
}
/// Only takes a Local request; a Plex pending download remains with its existing worker.
pub fn take_local_request() -> Option<LocalReadRequest> {
    let mut st = state();
    if !matches!(st.pending, Some(Pending::Local(_))) { return None; }
    match st.pending.take() { Some(Pending::Local(request)) => Some(request), _ => unreachable!() }
}
fn local_request_matches(st: &State, request: &LocalReadRequest) -> bool {
    st.generation == request.generation && st.want.as_ref() == Some(&request.selection())
}
/// App admission still verifies current playback and source authority before this check.
pub fn local_request_is_current(request: &LocalReadRequest) -> bool { local_request_matches(&state(), request) }

pub fn install_local(request: &LocalReadRequest, prepared: PreparedLocal) -> bool {
    let mut st = state();
    if !request.same_request(&prepared.request) || !local_request_matches(&st, request) { return false; }
    // Consume the existing ticket. Duplicate terminal delivery cannot publish twice.
    st.generation = st.generation.wrapping_add(1);
    st.pending = None;
    st.failed = None;
    st.loaded = Some((request.selection(), prepared.content));
    true
}
pub fn fail_local(request: &LocalReadRequest, failure: LocalFailure) -> bool {
    let mut st = state();
    if !request.same_request(&failure.request) || !local_request_matches(&st, request) { return false; }
    st.generation = st.generation.wrapping_add(1);
    st.pending = None;
    st.loaded = None;
    st.failed = Some((request.selection(), failure.why, Some(failure.at)));
    true
}

/// The existing app metadata worker could not start. This is a terminal failure,
/// not permission to retry every frame. It has no timed notice because no worker
/// sampled a clock; the app may display its ordinary launch-error notification.
pub fn fail_local_start(request: &LocalReadRequest) -> bool {
    let mut st = state();
    if !local_request_matches(&st, request) { return false; }
    st.generation = st.generation.wrapping_add(1);
    st.pending = None;
    st.loaded = None;
    st.failed = Some((request.selection(), Failure::Fetch, None));
    true
}
/// True for a terminal failure of the selected Local asset, including launch refusal.
pub fn local_failed() -> bool {
    let st = state();
    st.failed.as_ref().is_some_and(|(selection, _, _)|
        selection.is_local() && st.want.as_ref() == Some(selection))
}

/// Retire Local bytes/requests after source suspension or loss, preserving Plex.
/// Does not release the download worker occupancy: only its terminal path may do that.
/// The app may rearm a still-selected target only after validating its live source again.
pub fn reset_local() {
    let mut st = state();
    if st.want.as_ref().is_some_and(Selection::is_local) {
        st.generation = st.generation.wrapping_add(1);
        st.want = None;
    }
    if matches!(st.pending, Some(Pending::Local(_))) { st.pending = None; }
    if st.loaded.as_ref().is_some_and(|(selection, _)| selection.is_local()) { st.loaded = None; }
    if st.failed.as_ref().is_some_and(|(selection, _, _)| selection.is_local()) { st.failed = None; }
}

/// Stop drawing a sidecar (Off, or an embedded track was picked). The loaded file is KEPT, so
/// turning the same one back on is instant. MAIN THREAD.
pub fn deselect() {
    let mut st = state();
    st.generation = st.generation.wrapping_add(1);
    st.pending = None;
    st.want = None;
    st.failed = None;
}

/// Is a sidecar the selected subtitle? True from the pick, before its file has arrived — the
/// question is which KIND of track the viewer chose (it decides the timing offset's range,
/// `player::subtitle_offset_range_ms`), not whether a cue is ready.
pub fn selected() -> bool {
    state().want.is_some()
}

/// Mark `stream_id` as the selected sidecar without fetching anything — for tests of what the
/// selection KIND decides (the timing offset's range), which never draw a cue.
#[cfg(any(test, feature = "test-support"))]
pub fn select_without_fetch_for_test(stream_id: i64) {
    let mut st = state();
    st.generation = st.generation.wrapping_add(1);
    st.pending = None;
    st.want = Some(Selection::Plex {
        server: plx_plex::plex::ServerId::UNSET, stream_id, key: String::new(), codec: String::new(),
    });
    st.failed = None;
}

/// Select `stream_id` as a sidecar whose conversion FAILED `age` ago, without fetching anything: the
/// failure notice is up for [`FAILURE_SHOWN`] from then. For the player screen's frame-fingerprint
/// tests, which cannot wait six seconds and cannot reach the private state.
#[cfg(any(test, feature = "test-support"))]
pub fn fail_for_test(stream_id: i64, age: Duration) {
    select_without_fetch_for_test(stream_id);
    let mut st = state();
    let want = st.want.clone().expect("just selected");
    st.failed = Some((want, Failure::Empty, Some(Instant::now().checked_sub(age).unwrap_or_else(Instant::now))));
}

/// A new item is starting: nothing of the previous one's may survive. MAIN THREAD.
pub fn reset() {
    let mut st = state();
    st.generation = st.generation.wrapping_add(1);
    st.pending = None;
    st.want = None;
    st.loaded = None;
    st.failed = None;
}

/// **Honour the sidecar this part shows from the start**: the one the SERVER already has selected
/// for this part (picked here in an earlier session, or on another Plex client), else
/// `remembered`, the stream id the profile's track memory chose for this episode
/// (`Plan::memory_sidecar`). Either way the key and codec are looked up in the PLAYING item, so a
/// remembered external subtitle uses the file THIS episode has, never the one it was picked on. The
/// embedded twin is `route::pick_dp_subtitle`, which leaves an external selection off because nothing
/// could render it; now something can. `remembered` must still be a sidecar the app can draw
/// (`Stream::sidecar_renderable`) or it is ignored. Direct play, or a plain remux start (the
/// caller's gate): a start that burns keeps subtitles off, as before.
pub fn restore_selection(
    server: plx_plex::plex::ServerId, meta: plx_data::metadata::MetadataView<'_>, remembered: Option<i64>,
    style: TextStyle,
) -> Option<i64> {
    let item = meta.playing()?;
    let (s, whose) = match plx_data::metadata::server_selected_sidecar(item) {
        Some(s) => (s, "server-selected"),
        None => {
            let id = remembered?;
            let s = item.subs.iter().find(|s| s.id == id && s.sidecar_renderable())?;
            (s, "remembered")
        }
    };
    super::log(&format!("{whose} sidecar subtitle: sid={}", s.id));
    select(server, s.id, s.key.clone(), s.codec.clone(), style);
    Some(s.id)
}

/// The failure notice to draw, if a sidecar is selected: why nothing is being drawn for it, while
/// that is worth saying on screen — for [`FAILURE_SHOWN`] after its fetch or conversion failed. The
/// cues themselves are libass's ([`ass_source`]); this is the one thing the renderer cannot say,
/// that a picked file produced nothing. The caller applies the burned rule (the server BURNS the
/// selection; drawing the notice too would double the line).
pub fn notice() -> Option<&'static str> {
    let st = state();
    let want = st.want.as_ref()?;
    match &st.failed {
        Some((source, why, Some(at))) if source == want && at.elapsed() < FAILURE_SHOWN => Some(why.message()),
        _ => None,
    }
}

/// Is [`notice`] up right now? The player's frame fingerprint reads it, so the instant the notice
/// expires still owes a frame (nothing else on the screen changes then).
pub fn notice_shown() -> bool {
    notice().is_some()
}

/// The selected script, unless the server burns it. It is the same immutable source through seeks
/// and Off→On; render requests carry the current clock, so no cue is lost on a backward seek. A
/// converted SubRip/WebVTT file whose header was synthesized for other picks than the viewer's now
/// has its restyle started here, lazily, on a worker; the old look stays up until the worker
/// publishes the restyled script (an unselected file is never restyled).
pub fn ass_source(burned: bool, style: TextStyle) -> Option<Arc<super::ass::Source>> {
    ass_source_styled(burned, style)
}

/// [`ass_source`] for explicit picks (the tests restyle without touching the preferences).
fn ass_source_styled(burned: bool, style: TextStyle) -> Option<Arc<super::ass::Source>> {
    if burned { return None; }
    let mut st = state();
    let State { want, loaded, .. } = &mut *st;
    let want = want.as_ref()?;
    let (selection, content) = loaded.as_mut()?;
    if selection != want {
        return None;
    }
    // The file can arrive before its movie's demuxer discovers attachments. Preserve both buffers
    // by Arc and revise only when that immutable font publication, or the picks, changed. The font
    // store never acquires the sidecar lock, so this short snapshot has one lock order; no native
    // work runs under either lock.
    let (generation, fonts) = super::ass_source::font_snapshot();
    let restyled = content.text.as_ref().is_some_and(|hold| hold.style_generation != style.generation());
    if restyled {
        // The new script is built off this thread (it is the whole file again, megabytes for a long
        // one) and published under this lock when ready; until then the old look stays up.
        plx_base::diag::spans::span("restyle", || start_restyle(content, style));
    }
    if content.font_generation != Some(generation) {
        let super::ass::Content::Script { bytes, .. } = &content.source.content else { return None; };
        content.source = Arc::new(super::ass::Source {
            id: content.source.id, revision: content.source.revision.wrapping_add(1),
            origin: content.source.origin, content: super::ass::Content::Script { bytes: bytes.clone(), fonts },
        });
        content.font_generation = Some(generation);
    }
    Some(content.source.clone())
}

/// Begin building `content`'s script for `style` on a worker, unless one is already running.
/// FRAME THREAD, under the sidecar lock: it copies two `Arc`s and starts a thread, whatever the size
/// of the file. The worker publishes with the font snapshot current by then, so a font arriving
/// meanwhile is not lost.
fn start_restyle(content: &mut Content, style: TextStyle) {
    let Some(hold) = content.text.as_mut().filter(|hold| !hold.restyling) else { return };
    let super::ass::Content::Script { bytes, .. } = &content.source.content else { return };
    let (id, base_generation, header_len, bytes) = (content.source.id, hold.style_generation, hold.header_len, bytes.clone());
    hold.restyling = true;
    let started = plx_base::task::spawn_small("sidecar restyle", move || {
        let header = text_ass::header(&style);
        let script = joined(header.as_bytes(), &bytes[header_len..]);
        drop(bytes);
        let mut st = state();
        if let Some((_, content)) = st.loaded.as_mut() {
            publish_restyle(content, id, base_generation, style, header.len(), script);
        }
        SETTLED.notify_all();
        drop(st);
        plx_machine::present::wake_from_worker();
    });
    if !started {
        super::log("sidecar: restyle worker refused");
        hold.restyling = false;
        SETTLED.notify_all();
    }
}

/// Publish a script built for `style` if `content` is still the file it was built from. The
/// fonts are the ones the content holds now, so a restyle never undoes a font publication.
fn publish_restyle(content: &mut Content, id: u64, base_generation: u32, style: TextStyle, header_len: usize, script: Arc<[u8]>) {
    let (revision, origin, fonts) = match (&content.source.content, content.text.as_mut()) {
        (super::ass::Content::Script { fonts, .. }, Some(hold)) if content.source.id == id => {
            hold.restyling = false;
            if hold.style_generation != base_generation {
                return;
            }
            (hold.header_len, hold.style_generation) = (header_len, style.generation());
            (content.source.revision.wrapping_add(1), super::ass::Origin::Text(style), fonts.clone())
        }
        _ => return,
    };
    content.source = Arc::new(super::ass::Source {
        id, revision, origin, content: super::ass::Content::Script { bytes: script, fonts },
    });
}

/// `head` then `tail` as one shared buffer, allocated once at its exact size (the iterator is
/// exact-sized, so no intermediate `Vec` is built and copied).
fn joined(head: &[u8], tail: &[u8]) -> Arc<[u8]> {
    let mut script = Vec::with_capacity(head.len() + tail.len());
    script.extend_from_slice(head);
    script.extend_from_slice(tail);
    script.into()
}

fn is_ass_script(text: &str) -> bool {
    text.lines().any(|line| line.trim().eq_ignore_ascii_case("[Events]"))
        && text.lines().any(|line| line.trim_start().get(..9)
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case("Dialogue:")))
}

/// Whether a text sidecar is WebVTT: it says so in its first line, or the server named the codec.
fn is_webvtt(text: &str, codec: &str) -> bool {
    codec.eq_ignore_ascii_case("vtt")
        || codec.eq_ignore_ascii_case("webvtt")
        || text.trim_start_matches('\u{feff}').starts_with("WEBVTT")
}

fn decode(bytes: Vec<u8>, codec: &str, style: TextStyle) -> Result<Content, Failure> {
    if bytes.len() > plx_plex::plex::SIDECAR_MAX_BYTES { return Err(Failure::Empty); }
    // The check's lossy copy (a whole file when it is not UTF-8) is gone before anything else is built.
    let authored = { let text = String::from_utf8_lossy(&bytes); is_ass_script(&text) };
    if authored {
        // Validate only the presence of events. libass owns the script grammar, including
        // style/format declarations, overlapping dialogue and vector drawing events.
        return Ok(Content {
            source: Arc::new(super::ass::Source {
                id: super::ass::next_source_id(), revision: 1, origin: super::ass::Origin::Authored,
                content: super::ass::Content::Script { bytes: bytes.into(), fonts: Arc::from([]) },
            }),
            font_generation: None,
            text: None,
        });
    }
    // A server conversion or error must not silently turn an ASS selection into plain text.
    if codec.eq_ignore_ascii_case("ass") || codec.eq_ignore_ascii_case("ssa") {
        return Err(Failure::Empty);
    }
    // A sidecar file is whole-file text of unknown language here: the charset step is the BOM,
    // valid UTF-8, else the encoding `chardetng` guesses from the whole file (`sub_charset`).
    let text = super::sub_charset::subtitle_text_whole(&bytes, &super::sub_charset::TrackCharset::unknown());
    // A NUL is never subtitle text: padding is dropped, as the line readers always did.
    let text = if text.contains('\0') { std::borrow::Cow::Owned(text.replace('\0', "")) } else { text };
    let text_ref = text.trim_start_matches('\u{feff}');
    if is_ass_script(text_ref) {
        return Err(Failure::Empty);
    }
    let converted = if is_webvtt(text_ref, codec) {
        text_ass::script_from_vtt(text_ref)
    } else { text_ass::script_from_srt(text_ref) };
    // Only the converted events are needed from here: the decoded text and the file go first.
    drop(text);
    drop(bytes);
    if converted.cues == 0 {
        return Err(Failure::Empty);
    }
    Ok(content_from_text(converted, text_ass::header(&style), style))
}

fn content_from_text(converted: text_ass::TextScript, header: String, style: TextStyle) -> Content {
    let script = joined(header.as_bytes(), converted.events.as_bytes());
    drop(converted.events);
    Content {
        source: Arc::new(super::ass::Source {
            id: super::ass::next_source_id(), revision: 1, origin: super::ass::Origin::Text(style),
            content: super::ass::Content::Script { bytes: script, fonts: Arc::from([]) },
        }),
        font_generation: None,
        text: Some(TextHold { header_len: header.len(), style_generation: style.generation(), cues: converted.cues, restyling: false }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::ass::{self, Origin};
    use super::super::ass_script::{ScriptIndex, WINDOW_BYTES, WINDOW_EVENTS};
    use plx_plex::plex::session::{SubtitlePosition, SubtitleSize};
    use std::sync::atomic::Ordering::Relaxed;

    /// A backstop against a hung worker only: the waits below end on the worker's own signal, so a
    /// loaded runner just takes longer. Hitting it means the worker never finished.
    const WORKER_HUNG: Duration = Duration::from_secs(120);

    /// Block until the download worker has gone idle ([`SETTLED`]), however long the host takes.
    fn finish_download() {
        let (st, timeout) = SETTLED
            .wait_timeout_while(state(), WORKER_HUNG, |st| st.running)
            .unwrap_or_else(|e| e.into_inner());
        assert!(!timeout.timed_out() && !st.running, "sidecar worker finished");
    }

    /// Block until no restyle worker is building a script for the loaded file ([`SETTLED`]).
    fn finish_restyle() {
        let (_st, timeout) = SETTLED
            .wait_timeout_while(state(), WORKER_HUNG, |st| {
                st.loaded.as_ref().and_then(|(_, c)| c.text.as_ref()).is_some_and(|hold| hold.restyling)
            })
            .unwrap_or_else(|e| e.into_inner());
        assert!(!timeout.timed_out(), "sidecar restyle worker finished");
    }

    const STYLED_SCRIPT: &[u8] = b"[Script Info]\nScriptType: v4.00+\n[Events]\n\
        Dialogue: 0,0:00:01.00,0:00:03.00,Default,,0,0,0,,{\\pos(300,100)\\c&H0000FF&}SIGN\n";

    /// The script bytes of a loaded file.
    fn script_bytes(content: &Content) -> Arc<[u8]> {
        let ass::Content::Script { bytes, .. } = &content.source.content else { panic!("a sidecar is a script") };
        bytes.clone()
    }

    /// The `Dialogue:` lines of a loaded file, as libass is given them.
    fn dialogue(content: &Content) -> Vec<String> {
        String::from_utf8(script_bytes(content).to_vec()).unwrap()
            .lines().filter(|l| l.starts_with("Dialogue:")).map(String::from).collect()
    }

    /// The text field of each line.
    fn texts(content: &Content) -> Vec<String> {
        dialogue(content).iter().map(|l| l.splitn(10, ',').nth(9).unwrap().to_string()).collect()
    }

    fn converted(body: &str) -> Content {
        decode(body.as_bytes().to_vec(), "srt", crate::player::text_style::TextStyle::current(Default::default())).expect("a usable file")
    }

    #[test]
    fn nul_bytes_in_a_text_sidecar_do_not_fail_the_whole_file() {
        let body = "1\n00:00:01,000 --> 00:00:02,000\nHel\0lo\n\n2\n00:00:03,000 --> 00:00:04,000\nWorld\n\0\0\0";
        let content = converted(body);
        assert_eq!(texts(&content), ["Hello", "World"]);
    }

    #[test]
    fn styled_sidecar_source_survives_seek_and_off_on_but_not_new_items() {
        let _guard = plx_base::testlock::serial();
        reset();
        finish_download();
        let server = plx_plex::plex::ServerId::from_raw(0);
        select_with_fetch(server, 42, "/library/streams/42".into(), "ass".into(), crate::player::text_style::TextStyle::current(Default::default()),
            |_, _, _| Some(STYLED_SCRIPT.to_vec()));
        finish_download();
        let first = ass_source(false, crate::player::text_style::TextStyle::current(Default::default())).expect("native script available");
        assert_eq!(first.origin, Origin::Authored, "an authored script keeps its own look");
        assert!(ass_source(true, crate::player::text_style::TextStyle::current(Default::default())).is_none(), "a server burn silences the native script");
        for _seek in 0..3 {
            assert_eq!(notice(), None, "a script has no notice");
            assert_eq!(ass_source(false, crate::player::text_style::TextStyle::current(Default::default())).unwrap().id, first.id, "seek keeps the whole script");
        }
        deselect();
        assert!(!selected());
        assert!(ass_source(false, crate::player::text_style::TextStyle::current(Default::default())).is_none());
        select_with_fetch(server, 42, "/library/streams/42".into(), "ASS".into(), crate::player::text_style::TextStyle::current(Default::default()),
            |_, _, _| panic!("the same cached script must not be downloaded again"));
        assert!(Arc::ptr_eq(&ass_source(false, crate::player::text_style::TextStyle::current(Default::default())).unwrap(), &first));
        reset();
        assert!(ass_source(false, crate::player::text_style::TextStyle::current(Default::default())).is_none(), "new item drops the old source");
    }

    #[test]
    fn styled_sidecar_adopts_font_attachments_that_arrive_after_its_download() {
        let _guard = plx_base::testlock::serial();
        reset();
        finish_download();
        super::super::ass_source::reset();
        select_with_fetch(plx_plex::plex::ServerId::from_raw(0), 42,
            "/library/streams/42".into(), "ass".into(), crate::player::text_style::TextStyle::current(Default::default()), |_, _, _| Some(STYLED_SCRIPT.to_vec()));
        finish_download();
        let before = ass_source(false, crate::player::text_style::TextStyle::current(Default::default())).unwrap();
        let font = super::super::ass::Font { name: "sign.ttf".into(), data: Arc::from(&b"font"[..]) };
        // Fonts belong to the media even when it has no embedded ASS track.
        super::super::ass_source::begin("fixture", Vec::new(), vec![font], crate::player::text_style::TextStyle::current(Default::default()));
        let after = ass_source(false, crate::player::text_style::TextStyle::current(Default::default())).unwrap();
        assert_eq!(after.id, before.id, "the script's identity is stable");
        assert!(after.revision > before.revision, "new fonts invalidate native rasterization");
        let super::super::ass::Content::Script { bytes: old_bytes, fonts: old_fonts } = &before.content else { panic!() };
        let super::super::ass::Content::Script { bytes, fonts } = &after.content else { panic!() };
        assert!(old_fonts.is_empty());
        assert_eq!(fonts.len(), 1);
        assert_eq!(fonts[0].name, "sign.ttf");
        assert!(Arc::ptr_eq(bytes, old_bytes), "script bytes must not be recopied per font update");
        assert!(Arc::ptr_eq(fonts, &super::super::ass_source::font_snapshot().1));
        assert!(Arc::ptr_eq(&ass_source(false, crate::player::text_style::TextStyle::current(Default::default())).unwrap(), &after), "unchanged frames reuse the source");
        reset();
        super::super::ass_source::reset();
    }

    #[test]
    fn identical_stream_ids_on_different_servers_do_not_reuse_the_sidecar() {
        let _guard = plx_base::testlock::serial();
        reset();
        finish_download();
        let key = "/library/streams/42";
        for server in [0, 1] {
            select_with_fetch(plx_plex::plex::ServerId::from_raw(server), 42, key.into(), "srt".into(), crate::player::text_style::TextStyle::current(Default::default()),
                move |_, _, _| Some(format!("00:00:01 --> 00:00:03\nserver {server}\n").into_bytes()));
            finish_download();
            let source = ass_source(false, crate::player::text_style::TextStyle::current(Default::default())).expect("the converted file");
            let ass::Content::Script { bytes, .. } = &source.content else { panic!() };
            let script = String::from_utf8_lossy(bytes);
            assert!(script.contains(&format!(",server {server}\n")), "{script}");
            assert!(!script.contains(&format!("server {}", 1 - server)));
        }
        reset();
    }

    #[test]
    fn sidecar_picks_share_one_worker_and_drop_abandoned_answers() {
        let _guard = plx_base::testlock::serial();
        reset();
        // A prior route test may have left its now-abandoned worker finishing a no-client
        // result. Let that worker retire before installing this test's controlled transport.
        finish_download();
        let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let release_rx = std::sync::Arc::new(Mutex::new(release_rx));
        let fetch = {
            let calls = calls.clone();
            move |_: plx_plex::plex::ServerId, key: &str, _: &str| {
                let n = calls.fetch_add(1, Relaxed);
                started_tx.send(()).unwrap();
                if n == 0 { release_rx.lock().unwrap().recv().unwrap(); }
                Some(format!("00:00:01 --> 00:00:03\n{key}\n").into_bytes())
            }
        };
        let sid = plx_plex::plex::ServerId::from_raw(0);
        select_with_fetch(sid, 1, "old".into(), "srt".into(), crate::player::text_style::TextStyle::current(Default::default()), fetch.clone());
        started_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        reset();
        select_with_fetch(sid, 1, "new".into(), "srt".into(), crate::player::text_style::TextStyle::current(Default::default()), fetch);
        let parallel = started_rx.recv_timeout(Duration::from_millis(100)).is_ok();
        release_tx.send(()).unwrap();
        let text_of = || ass_source(false, crate::player::text_style::TextStyle::current(Default::default())).map(|s| {
            let ass::Content::Script { bytes, .. } = &s.content else { panic!() };
            String::from_utf8_lossy(bytes).into_owned()
        });
        finish_download();
        let script = text_of().expect("the latest pick landed");
        assert!(script.ends_with(",new\n"), "{script}");
        assert!(!script.contains(",old\n"));
        reset();
        assert!(!parallel, "an abandoned fetch must finish before another worker is started");
    }

    #[test]
    fn sidecar_download_refuses_an_oversized_body() {
        let mut body = b"00:00:01 --> 00:00:03\n".to_vec();
        body.resize(4 * 1024 * 1024 + 1, b'x');
        assert!(decode(body.clone(), "srt", crate::player::text_style::TextStyle::current(Default::default())).is_err(), "oversized sidecars must not allocate a converted copy");
        assert!(decode(body, "ass", crate::player::text_style::TextStyle::current(Default::default())).is_err(), "native scripts obey the same download bound");
    }

    /// A cue is never cut: a 10,000-character line reaches the script whole.
    #[test]
    fn sidecar_cue_text_is_whole() {
        let line = "é".repeat(10_000);
        let content = converted(&format!("00:00:01 --> 00:00:03\n{line}"));
        assert_eq!(texts(&content), [line]);
    }

    /// The ordinary file: counters, CRLF, a BOM, italics, a two-line cue — the counter line must
    /// never leak into the cue before it, and times are written to the centisecond.
    #[test]
    fn a_subrip_file_becomes_clean_sorted_events() {
        let srt = "\u{feff}1\r\n00:00:01,000 --> 00:00:03,500\r\n<i>Hello</i>\r\nthere\r\n\r\n\
                   2\r\n00:01:00,250 --> 00:01:02,000\r\n{\\an8}Second line\r\n";
        let content = converted(srt);
        assert_eq!(content.text.as_ref().unwrap().cues, 2);
        assert_eq!(dialogue(&content), [
            "Dialogue: 0,0:00:01.00,0:00:03.50,Default,,0,0,0,,{\\i1}Hello{\\i0}\\Nthere",
            "Dialogue: 0,0:01:00.25,0:01:02.00,An8,,0,0,0,,Second line",
        ]);
        assert_eq!(content.source.origin, Origin::Text(TextStyle::current(Default::default())));
        let script = String::from_utf8(script_bytes(&content).to_vec()).unwrap();
        assert!(script.starts_with(&text_ass::header(&TextStyle::current(Default::default()))), "the synthesized header comes first");
    }

    /// PMS is asked for SubRip but may answer in whatever it has. WebVTT differs in a header,
    /// `.` before the milliseconds with the hours optional, cue settings after the end time and
    /// blocks that are not cues; none may cost a cue or reach the text, and `line:0` puts the cue
    /// at the top.
    #[test]
    fn a_webvtt_file_converts_and_line_zero_is_the_top() {
        let vtt = "WEBVTT\n\nNOTE made by hand\n\n01:02.500 --> 01:05.000 line:0 position:50%\nHi\n\n\
                   1:00:00.000 --> 1:00:01.000\nAn hour in\n\n\
                   00:00:10.000 --> 00:00:11.000 line:-1\nBottom\n";
        let content = converted(vtt);
        assert_eq!(dialogue(&content), [
            "Dialogue: 0,0:00:10.00,0:00:11.00,Default,,0,0,0,,Bottom",
            "Dialogue: 0,0:01:02.50,0:01:05.00,An8,,0,0,0,,Hi",
            "Dialogue: 0,1:00:00.00,1:00:01.00,Default,,0,0,0,,An hour in",
        ]);
        // the codec the server names is enough when the first line does not say so
        let bare = decode(b"00:01.000 --> 00:02.000 line:0\nTop\n".to_vec(), "vtt", crate::player::text_style::TextStyle::current(Default::default())).unwrap();
        assert_eq!(dialogue(&bare), ["Dialogue: 0,0:00:01.00,0:00:02.00,An8,,0,0,0,,Top"]);
    }

    /// A merged feature-length file is well past any one window: every cue is converted, and the
    /// script is fed to libass in windows that hold the first cue, cue 25,000 and the last.
    #[test]
    fn a_long_file_converts_whole_and_is_fed_in_windows() {
        const N: i64 = 30_000;
        let stamp = |secs: i64| format!("{:02}:{:02}:{:02},000", secs / 3600, secs / 60 % 60, secs % 60);
        let mut body = String::new();
        for k in 0..N {
            // Cue k runs over [2k s, 2k s + 1 s) and says "cue k".
            body.push_str(&format!("{}\n{} --> {}\ncue {k}\n\n", k + 1, stamp(2 * k), stamp(2 * k + 1)));
        }
        let content = converted(&body);
        assert_eq!(content.text.as_ref().unwrap().cues, N as usize);
        let index = ScriptIndex::parse(&script_bytes(&content)).unwrap();
        assert_eq!(index.len(), N as usize);
        assert!(index.windowed());
        for k in [0, 25_000, N - 1] {
            let window = index.window(2 * k * 1000 + 500);
            assert!(window.covers(2 * k * 1000 + 500));
            let held = String::from_utf8_lossy(&window.text).into_owned();
            assert!(held.contains(&format!(",cue {k}\n")), "cue {k} is in its window");
            assert!(held.starts_with(&text_ass::header(&TextStyle::current(Default::default()))));
            assert!(held.matches("Dialogue:").count() <= WINDOW_EVENTS);
        }
    }

    /// The worst case the size rule exists for: a 4 MiB (`SIDECAR_MAX_BYTES`) Thai SubRip file in
    /// TIS-620 is one byte per letter and three once decoded to UTF-8, so a converted script is
    /// well past the 8 MiB a script loaded whole may be. It must convert, be fed in windows, and
    /// every window must stay within the bound. Two shapes: many ordinary cues, and a handful of
    /// enormous ones (more bytes than a window but fewer lines than one).
    #[test]
    fn a_4_mib_tis620_file_converts_and_windows_within_the_bound() {
        let thai = "สวัสดีครับ ผมชื่อสมชาย ยินดีที่ได้รู้จักทุกคน ";
        let (tis, _, bad) = encoding_rs::WINDOWS_874.encode(thai);
        assert!(!bad);
        let cue = |k: usize, repeats: usize| {
            let at = |s: usize| format!("{:02}:{:02}:{:02},000", s / 3600, s / 60 % 60, s % 60);
            let mut out = format!("{}\r\n{} --> {}\r\n", k + 1, at(k * 2), at(k * 2 + 1)).into_bytes();
            for _ in 0..repeats { out.extend_from_slice(&tis); }
            out.extend_from_slice(b"\r\n\r\n");
            out
        };
        let limit = plx_plex::plex::SIDECAR_MAX_BYTES;
        for (name, repeats) in [("many small cues", 2), ("few enormous cues", 10_000)] {
            let mut file = Vec::new();
            let mut k = 0;
            loop {
                let next = cue(k, repeats);
                if file.len() + next.len() > limit { break; }
                file.extend_from_slice(&next);
                k += 1;
            }
            assert!(file.len() <= limit && file.len() > limit - 512 * 1024, "{name}: {} bytes", file.len());
            let content = decode(file, "srt", crate::player::text_style::TextStyle::current(Default::default())).unwrap_or_else(|why| panic!("{name}: {why:?}"));
            assert!(texts(&content)[0].contains("สวัสดีครับ"), "{name}: decoded as Thai, not mojibake");
            let bytes = script_bytes(&content);
            assert!(bytes.len() > WINDOW_BYTES, "{name}: {} bytes after conversion is the case under test", bytes.len());
            assert_eq!(ass::validate(&content.source), Ok(()), "{name}: a windowed script is bounded by its source");
            let index = ScriptIndex::parse(&bytes).unwrap();
            assert!(index.windowed(), "{name}");
            // walk it: every window is within the bound and covers its playhead; together they hold every cue
            let mut now = 0;
            let mut seen = std::collections::BTreeSet::new();
            loop {
                let window = index.window(now);
                assert!(window.text.len() <= WINDOW_BYTES, "{name}: a {} byte window", window.text.len());
                assert!(window.covers(now), "{name}");
                let held = String::from_utf8_lossy(&window.text).into_owned();
                for line in held.lines().filter(|l| l.starts_with("Dialogue:")) {
                    seen.insert(line.splitn(4, ',').nth(1).unwrap().to_string());
                }
                if window.hi_ms == i64::MAX { break; }
                now = window.hi_ms;
            }
            assert_eq!(seen.len(), index.len(), "{name}: every cue is in some window");
        }
    }

    /// Out-of-order files exist (merged fan subs); the script is in start order. A cue that ends
    /// before it starts, or says nothing, is dropped rather than drawn.
    #[test]
    fn events_are_sorted_and_the_unusable_cues_dropped() {
        let srt = "2\n00:00:10,000 --> 00:00:11,000\nlater\n\n1\n00:00:01,000 --> 00:00:02,000\nsooner\n\n\
                   3\n00:00:05,000 --> 00:00:04,000\nbackwards\n\n4\n00:00:20,000 --> 00:00:21,000\n<i></i>\n";
        assert_eq!(texts(&converted(srt)), ["sooner", "later"]);
    }

    /// Where two cues overlap the newer one wins: the older ends where the newer starts.
    #[test]
    fn an_overlap_is_clipped_to_the_newest_cue() {
        let content = converted("1\n00:00:01,000 --> 00:00:04,000\na\n\n2\n00:00:02,000 --> 00:00:04,000\nb\n");
        assert_eq!(dialogue(&content), [
            "Dialogue: 0,0:00:01.00,0:00:02.00,Default,,0,0,0,,a",
            "Dialogue: 0,0:00:02.00,0:00:04.00,Default,,0,0,0,,b",
        ]);
    }

    /// Garbage in, nothing out — an HTML error page or an image subtitle must not panic and must
    /// not produce a script, because that is what turns into the on-screen explanation.
    #[test]
    fn a_body_that_is_not_a_subtitle_yields_no_script() {
        for body in [
            &b"<html><body>501 Not Implemented</body></html>"[..],
            &[0xff, 0xfe, 0x00, 0x01, 0x02],
            b"",
            b"1\n99:99:99,999 --> nonsense\ntext\n",
        ] {
            assert!(matches!(decode(body.to_vec(), "srt", crate::player::text_style::TextStyle::current(Default::default())), Err(Failure::Empty)), "{body:?}");
        }
    }

    #[test]
    fn styled_script_keeps_headers_overrides_and_overlapping_events() {
        let script = b"[Script Info]\nScriptType: v4.00+\n[V4+ Styles]\nStyle: Sign,Example\n[Events]\n\
            Dialogue: 0,0:00:01.00,0:00:03.00,Sign,,0,0,0,,{\\pos(300,100)}SIGN\n\
            Dialogue: 1,0:00:01.00,0:00:03.00,Default,,0,0,0,,{\\k20}Dialogue\n";
        for codec in ["ass", "SSA"] {
            let content = decode(script.to_vec(), codec, crate::player::text_style::TextStyle::current(Default::default())).unwrap();
            assert_eq!(script_bytes(&content).as_ref(), script);
            assert_eq!(content.source.origin, Origin::Authored);
            assert!(content.text.is_none(), "an authored script is never restyled");
        }
        assert!(decode(b"1\n00:00:01,000 --> 00:00:03,000\nSIGN\n".to_vec(), "ass", crate::player::text_style::TextStyle::current(Default::default())).is_err(),
            "a converted response must not silently discard styling");
    }

    /// #708's legacy charsets reach the script as the same UTF-8 text the embedded path gets.
    #[test]
    fn legacy_charsets_convert_to_the_expected_event_body() {
        for (encoding, line) in [
            (encoding_rs::GBK, "你好，世界。今天天气很好，我们一起去公园散步吧。"),
            (encoding_rs::SHIFT_JIS, "こんにちは、世界。今日はいい天気ですね。一緒に散歩に行きましょう。"),
            (encoding_rs::EUC_KR, "안녕하세요, 세계. 오늘 날씨가 정말 좋네요. 같이 산책하러 가요."),
        ] {
            let (bytes, _, bad) = encoding.encode(line);
            assert!(!bad);
            let mut file = b"1\n00:00:01,000 --> 00:00:03,000\n".to_vec();
            file.extend_from_slice(&bytes);
            file.push(b'\n');
            let content = decode(file, "srt", crate::player::text_style::TextStyle::current(Default::default())).expect("a usable file");
            assert_eq!(texts(&content), [line], "{}", encoding.name());
        }
    }

    /// Another pair of picks than `current`, in both size and position.
    fn other_picks(current: TextStyle) -> TextStyle {
        TextStyle {
            size: if current.size == SubtitleSize::Large { SubtitleSize::Small } else { SubtitleSize::Large },
            position: if current.position == SubtitlePosition::High { SubtitlePosition::Low } else { SubtitlePosition::High },
            ..current
        }
    }

    /// The source for `picks`, once the worker that builds a restyled script has published it.
    fn restyled_source(picks: TextStyle) -> Arc<ass::Source> {
        finish_restyle();
        ass_source_styled(false, picks).expect("the converted file")
    }

    /// A pick change must cost the frame thread nothing in proportion to the file: the call that
    /// notices it returns the source already up (the old look stays until the new script is built
    /// elsewhere), and the new one arrives with the events untouched and the revision bumped once.
    #[test]
    fn a_pick_change_returns_the_held_source_at_once_and_publishes_the_restyle_later() {
        let _guard = plx_base::testlock::serial();
        reset();
        finish_download();
        let body: String = (0..20_000).map(|i| format!(
            "{n}\n00:{m:02}:{s:02},000 --> 00:{m:02}:{e:02},000\nA cue of some length number {n}\n\n",
            n = i + 1, m = (i / 30) % 60, s = (i % 30) * 2 % 60, e = ((i % 30) * 2 + 1) % 60)).collect();
        select_with_fetch(plx_plex::plex::ServerId::from_raw(0), 8, "/library/streams/8".into(), "srt".into(), crate::player::text_style::TextStyle::current(Default::default()),
            move |_, _, _| Some(body.clone().into_bytes()));
        finish_download();
        let current = TextStyle::current(Default::default());
        let picks = other_picks(current);
        let before = ass_source_styled(false, current).unwrap();
        let during = ass_source_styled(false, picks).unwrap();
        assert!(Arc::ptr_eq(&before, &during), "the old look is still up when the pick lands");
        let after = restyled_source(picks);
        assert_eq!((after.id, after.revision, after.origin), (before.id, before.revision.wrapping_add(1), Origin::Text(picks)));
        let (ass::Content::Script { bytes: old, .. }, ass::Content::Script { bytes: new, .. }) =
            (&before.content, &after.content) else { panic!() };
        assert_eq!(&old[text_ass::header(&current).len()..], &new[text_ass::header(&picks).len()..]);
        assert!(Arc::ptr_eq(&after, &ass_source_styled(false, picks).unwrap()), "and once only");
        reset();
    }

    /// Host timing of the restyle on the TV's repro file (4 MiB TIS-620, 35,735 cues). Run by hand:
    /// `cargo test -p plxnative-media --lib restyle_timing -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn restyle_timing() {
        let bytes = std::fs::read("/tmp/libass7b-fx/sidecar-big-tis620.srt").expect("the fixture");
        let content = decode(bytes, "srt", crate::player::text_style::TextStyle::current(Default::default())).expect("a usable file");
        let ass::Content::Script { bytes: script, .. } = &content.source.content else { panic!() };
        let hold = content.text.as_ref().unwrap();
        let header = text_ass::header(&other_picks(TextStyle::current(Default::default())));
        let t0 = Instant::now();
        let rebuilt = joined(header.as_bytes(), &script[hold.header_len..]);
        println!("restyle of {} bytes, {} cues: {:?}", rebuilt.len(), hold.cues, t0.elapsed());
        let t0 = Instant::now();
        let bytewise: Arc<[u8]> = header.as_bytes().iter().chain(&script[hold.header_len..]).copied().collect();
        println!("the byte-wise join it replaced: {:?} ({} bytes)", t0.elapsed(), bytewise.len());
    }

    /// The failure notice is all that is left of the old caption path: shown for six seconds in
    /// the caption's own place (the caller silences it where the server burns the selection).
    #[test]
    fn the_failure_notice_is_shown_for_six_seconds() {
        let _guard = plx_base::testlock::serial();
        reset();
        finish_download();
        assert_eq!((notice(), notice_shown()), (None, false), "nothing selected");
        select_with_fetch(plx_plex::plex::ServerId::from_raw(0), 9, "/library/streams/9".into(), "srt".into(), crate::player::text_style::TextStyle::current(Default::default()),
            |_, _, _| Some(b"<html>not a subtitle</html>".to_vec()));
        finish_download();
        let message = plx_platform::i18n::msg::widgets_sidecar_empty();
        assert_eq!(notice(), Some(message));
        assert!(notice_shown());
        assert!(ass_source(false, crate::player::text_style::TextStyle::current(Default::default())).is_none(), "and no script");
        // six seconds on
        {
            let mut st = state();
            let (_, _, at) = st.failed.as_mut().unwrap();
            *at = Some(Instant::now().checked_sub(FAILURE_SHOWN + Duration::from_millis(1)).unwrap());
        }
        assert_eq!((notice(), notice_shown()), (None, false));
        // a fetch failure says its own thing
        select_with_fetch(plx_plex::plex::ServerId::from_raw(0), 10, "/library/streams/10".into(), "srt".into(), crate::player::text_style::TextStyle::current(Default::default()),
            |_, _, _| None);
        finish_download();
        assert_eq!(notice(), Some(plx_platform::i18n::msg::widgets_sidecar_fetch_failed()));
        deselect();
        assert_eq!(notice(), None, "off clears it");
        reset();
    }

    /// Issue #266 I6: the route withholds the audio enhancement exactly when this restore would
    /// put a sidecar on screen, so the two must read ONE predicate
    /// (`metadata::server_selected_sidecar`). Graded against every shape the predicate splits on:
    /// a drawable selected sidecar is restored and is the shared answer; an unselected one, a
    /// keyless one, a bitmap one and a selected EMBEDDED track are all nothing to both.
    #[test]
    fn restore_uses_shared_server_selected_sidecar() {
        let _guard = plx_base::testlock::serial();
        let sidecar = |id: i64, codec: &str, key: &str, selected: bool, external: bool| plx_data::metadata::Stream {
            id,
            codec: codec.into(),
            key: key.into(),
            selected,
            external,
            ..Default::default()
        };
        let cases: [(Vec<plx_data::metadata::Stream>, Option<i64>); 5] = [
            (vec![sidecar(1, "srt", "/library/streams/1", false, true), sidecar(2, "srt", "/library/streams/2", true, true)], Some(2)),
            (vec![sidecar(3, "srt", "/library/streams/3", false, true)], None),
            (vec![sidecar(4, "srt", "", true, true)], None),
            (vec![sidecar(5, "pgs", "/library/streams/5", true, true)], None),
            (vec![sidecar(6, "srt", "", true, false)], None),
        ];
        for (subs, want) in cases {
            let ids: Vec<i64> = subs.iter().map(|s| s.id).collect();
            let item = plx_data::metadata::PlayingItem::with_subs(subs);
            assert_eq!(plx_data::metadata::server_selected_sidecar(&item).map(|s| s.id), want, "{ids:?}");
            let mut store = plx_data::stores::metadata::MetadataStore::default();
            assert!(store.run(plx_data::stores::metadata::MetadataCmd::InstallPlaying(Some(item))));
            assert_eq!(restore_selection(plx_plex::plex::ServerId::UNSET, store.view(), None, crate::player::text_style::TextStyle::current(Default::default())), want, "{ids:?}");
            finish_download();
            deselect();
        }
    }

    #[test]
    fn queued_sidecar_conversion_uses_the_captured_appearance() {
        let _serial = plx_base::testlock::serial();
        reset();
        finish_download();
        use crate::player::text_style::{TextAppearance, TextBackground, TextColor, TextFont, TextOutline};
        let style = TextStyle {
            appearance: TextAppearance {
                font: TextFont::InterRegular, color: TextColor::Cyan,
                outline: TextOutline::ExtraThick, background: TextBackground::Quarter,
            },
            ..TextStyle::default()
        };
        select_with_fetch(plx_plex::plex::ServerId::from_raw(0), 17,
            "/library/streams/17".into(), "srt".into(), style,
            |_, _, _| Some(b"1\n00:00:01,000 --> 00:00:02,000\nSynthetic caption\n".to_vec()));
        finish_download();
        // Inspect before the frame getter: lazy restyling must not hide lost worker arguments.
        {
            let st = state();
            let (_, content) = st.loaded.as_ref().expect("conversion completed");
            assert_eq!(content.source.origin, Origin::Text(style));
            assert!(script_bytes(content).starts_with(text_ass::header(&style).as_bytes()));
            assert_eq!(content.text.as_ref().unwrap().style_generation, style.generation());
        }
        reset();
    }
    fn local_identity() -> PlaybackIdentity {
        let info = crate::ff::inspect::MediaInfo {
            tracks: Default::default(), resolved_source: None, hls_playback: None, video_codec: "h264".into(),
            audio_codec: "aac".into(), width: 1280, height: 720, fps: 24.0,
            duration_ns: 60_000_000_000, audio_channels: 2, audio_sample_rate: 48000,
            audio_bit_depth: 16, dovi: Default::default(),
        };
        let mut ps = crate::route::PlaybackSession::IDLE;
        crate::route::reset_player_control_for_test(&ps);
        crate::route::request_external_source(&mut ps, &info,
            crate::ff::ExternalSource::Local("/synthetic/movie.mkv".into()),
            "Synthetic", "Local", plx_platform::devcaps::assumed_shared()).unwrap();
        crate::route::reset_player_control_for_test(&ps);
        crate::route::playback_identity(&ps).unwrap()
    }

    #[test]
    fn held_plex_worker_never_takes_or_publishes_over_pending_local_read() {
        let _guard = plx_base::testlock::serial();
        reset();
        finish_download();
        let identity = local_identity();
        let (entered_tx, entered_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let release_rx = Mutex::new(release_rx);
        select_with_fetch(plx_plex::plex::ServerId::from_raw(0), 42,
            "/library/streams/42".into(), "ass".into(), TextStyle::current(Default::default()),
            move |_, _, _| {
                entered_tx.send(()).unwrap();
                release_rx.lock().unwrap().recv().unwrap();
                Some(STYLED_SCRIPT.to_vec())
            });
        entered_rx.recv_timeout(WORKER_HUNG).unwrap();
        let target = LocalSubtitleTarget::new();
        select_local(identity.clone(), target.clone(), SidecarFormat::Srt, TextStyle::current(Default::default()));
        assert!(state().running, "Local selection cannot pretend the old worker stopped");
        assert_eq!(peek_local_request().unwrap().target(), &target);
        reset_local();
        assert!(state().running, "revocation retains actual worker occupancy too");
        select_local(identity, target.clone(), SidecarFormat::Srt, TextStyle::current(Default::default()));
        release_tx.send(()).unwrap();
        finish_download();
        assert_eq!(peek_local_request().unwrap().target(), &target,
            "Plex drain left the Local request for its app worker");
        assert!(state().loaded.is_none(), "late Plex success did not replace Local");
        let request = take_local_request().unwrap();
        let ready = request.prepare_owned(&OffFrame::for_test(),
            b"1\n00:00:01,000 --> 00:00:02,000\nLocal\n".to_vec()).unwrap();
        assert!(install_local(&request, ready));
        reset();
    }

    #[test]
    fn local_reset_does_not_retire_plex_selection_or_held_download() {
        let _guard = plx_base::testlock::serial();
        reset();
        finish_download();
        let (entered_tx, entered_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let release_rx = Mutex::new(release_rx);
        select_with_fetch(plx_plex::plex::ServerId::from_raw(0), 42,
            "/library/streams/42".into(), "ass".into(), TextStyle::current(Default::default()),
            move |_, _, _| {
                entered_tx.send(()).unwrap();
                release_rx.lock().unwrap().recv().unwrap();
                Some(STYLED_SCRIPT.to_vec())
            });
        entered_rx.recv_timeout(WORKER_HUNG).unwrap();
        let generation = state().generation;
        reset_local();
        assert_eq!(state().generation, generation);
        assert!(selected());
        assert!(state().running);
        assert!(take_local_request().is_none());
        release_tx.send(()).unwrap();
        finish_download();
        assert!(ass_source(false, TextStyle::current(Default::default())).is_some());
        reset_local();
        assert!(ass_source(false, TextStyle::current(Default::default())).is_some());
        reset();
    }

}

#[cfg(test)]
#[path = "sidecar_hls_tests.rs"]
mod hls_tests;
