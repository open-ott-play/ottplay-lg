//! Separate HLS rendition choices. These are presentation/session identities,
//! never AVStream indexes, Plex stream IDs or persisted endpoint settings.
use super::*;
use crate::ff::hls_presentation::{HlsPlayback, HlsSubtitleId, RenditionId};
use std::sync::{Arc, atomic::AtomicBool};
use plx_base::task::OffFrame;
use crate::player::{sidecar, text_style::TextStyle};

/// Native HLS selection policy; independent of Plex's server preferences.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HlsSubtitleMode { #[default] Off, Auto, Forced }
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HlsSubtitlePolicy { pub mode: HlsSubtitleMode, pub language: String }
impl HlsSubtitlePolicy {
    pub const OFF: Self = Self { mode: HlsSubtitleMode::Off, language: String::new() };
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum HlsSubtitleIntent { #[default] Unset, Auto, ManualTrack, ManualOff }
pub fn hls_subtitle_policy(ps: &PlaybackSession) -> HlsSubtitlePolicy { ps.hls_subtitle_policy.clone() }
pub fn hls_subtitle_intent(ps: &PlaybackSession) -> HlsSubtitleIntent { ps.hls_subtitle_intent }
/// Restoring/changing a global preference never replaces this item's manual pick.
pub fn restore_hls_subtitle_policy(ps: &mut PlaybackSession, policy: HlsSubtitlePolicy) {
    if ps.hls_subtitle_policy == policy { return; }
    ps.hls_subtitle_policy = policy;
    if ps.hls_subtitle_intent == HlsSubtitleIntent::Auto { ps.hls_subtitle_intent = HlsSubtitleIntent::Unset; }
}
pub fn begin_automatic_hls_subtitle_request(ps: &mut PlaybackSession)
    -> Result<Option<HlsTrackRequest>, ExternalTrackApplyError>
{
    if ps.hls_subtitle_intent != HlsSubtitleIntent::Unset { return Ok(None); }
    let policy = hls_subtitle_policy(ps);
    let Some(catalog) = hls_track_catalog(ps) else { return Ok(None); };
    let language = (!policy.language.is_empty()).then_some(policy.language.as_str());
    let id = match policy.mode {
        HlsSubtitleMode::Off => None,
        HlsSubtitleMode::Auto | HlsSubtitleMode::Forced =>
            catalog.presentation.automatic_subtitle(language, policy.mode == HlsSubtitleMode::Forced),
    };
    if id.is_none() && catalog.selected_subtitle().is_none() {
        if policy.mode != HlsSubtitleMode::Off { ps.hls_subtitle_intent = HlsSubtitleIntent::Auto; }
        return Ok(None);
    }
    let choice = catalog.subtitle_choice(id)?;
    let mut request = begin_hls_request(ps, &choice)?;
    ps.hls_subtitle_intent = HlsSubtitleIntent::Auto;
    request.automatic_policy = Some(policy);
    Ok(Some(request))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HlsTrackRow {
    pub id: RenditionId,
    pub name: String,
    pub language: String,
    pub selected: bool,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HlsSubtitleRow {
    pub id: HlsSubtitleId,
    pub name: String,
    pub language: String,
    pub selected: bool,
    pub forced: bool,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HlsTrackTarget {
    Audio(RenditionId),
    Subtitle(Option<HlsSubtitleId>),
}
#[derive(Clone)]
pub struct HlsTrackCatalog {
    playback: PlaybackIdentity,
    presentation: HlsPlayback,
    subtitle: Option<HlsSubtitleId>,
    embedded: Option<ExternalTracks>,
}
impl std::fmt::Debug for HlsTrackCatalog {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HlsTrackCatalog").field("rows", &self.rows()).finish()
    }
}
impl HlsTrackCatalog {
    pub fn rows(&self) -> Vec<HlsTrackRow> {
        self.presentation.rows().map(|(id, name, language)| HlsTrackRow {
            id, name: name.into(), language: language.into(), selected: id == self.presentation.selected(),
        }).collect()
    }
    pub fn subtitle_rows(&self) -> Vec<HlsSubtitleRow> {
        self.presentation.subtitle_rows().map(|(id, name, language)| HlsSubtitleRow {
            id, name: name.into(), language: language.into(), selected: self.subtitle == Some(id), forced: self.presentation.subtitle_forced(id),
        }).collect()
    }
    pub fn selected_subtitle(&self) -> Option<HlsSubtitleId> { self.subtitle }
    /// Observed container tracks remain distinct from manifest rendition IDs.
    pub fn embedded_tracks(&self) -> Option<&ExternalTracks> { self.embedded.as_ref() }
    pub fn subtitle_choice(&self, id: Option<HlsSubtitleId>) -> Result<HlsTrackChoice, ExternalTrackApplyError> {
        if id.is_some_and(|id| !self.presentation.has_subtitle(id)) {
            return Err(ExternalTrackApplyError::Inactive);
        }
        Ok(HlsTrackChoice { catalog: self.clone(), target: HlsTrackTarget::Subtitle(id) })
    }
    pub fn same_session(&self, other: &Self) -> bool {
        self.playback == other.playback && self.presentation.same_presentation(&other.presentation)
            && match (&self.embedded, &other.embedded) {
                (Some(a), Some(b)) => a.same_session(b), (None, None) => true, _ => false,
            }
    }
    pub fn choice(&self, id: RenditionId) -> Result<HlsTrackChoice, ExternalTrackApplyError> {
        if !self.presentation.has_rendition(id) { return Err(ExternalTrackApplyError::Inactive); }
        Ok(HlsTrackChoice { catalog: self.clone(), target: HlsTrackTarget::Audio(id) })
    }
}
#[derive(Clone)]
pub struct HlsTrackChoice {
    catalog: HlsTrackCatalog,
    target: HlsTrackTarget,
}
impl HlsTrackChoice {
    pub fn target(&self) -> HlsTrackTarget { self.target }
}
impl PartialEq for HlsTrackChoice {
    fn eq(&self, other: &Self) -> bool {
        self.target == other.target && self.catalog.same_session(&other.catalog)
    }
}
impl Eq for HlsTrackChoice {}
impl std::fmt::Debug for HlsTrackChoice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HlsTrackChoice").field("target", &self.target).finish_non_exhaustive()
    }
}
#[derive(Clone)]
pub struct HlsTrackRequest {
    choice: HlsTrackChoice,
    revision: u64,
    worker: WorkerTicket,
    text_style: TextStyle,
    automatic_policy: Option<HlsSubtitlePolicy>,
}
impl std::fmt::Debug for HlsTrackRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HlsTrackRequest").field("target", &self.choice.target).finish_non_exhaustive()
    }
}
pub struct PreparedHlsTrack(Prepared);
enum Prepared {
    Audio(HlsPlayback),
    Subtitle { playback: HlsPlayback, id: HlsSubtitleId, content: sidecar::PreparedHls, epoch: Option<u64> },
    Off(HlsPlayback),
}
impl PreparedHlsTrack {
    fn matches(&self, choice: &HlsTrackChoice) -> bool {
        let (presentation, target) = match &self.0 {
            Prepared::Audio(playback) => (playback, HlsTrackTarget::Audio(playback.selected())),
            Prepared::Subtitle { playback, id, .. } => (playback, HlsTrackTarget::Subtitle(Some(*id))),
            Prepared::Off(playback) => (playback, HlsTrackTarget::Subtitle(None)),
        };
        presentation.same_presentation(&choice.catalog.presentation) && target == choice.target
    }
}
impl HlsTrackRequest {
    /// Serial unit-test fixture only: restore shared player fields on success
    /// and unwind, and settle the test controller back to its idle baseline.
    #[cfg(any(test, feature = "test-support"))]
    pub fn test_globals_guard() -> impl Drop {
        test_globals::Guard::capture()
    }
    #[cfg(any(test, feature = "test-support"))]
    pub fn prepared_for_test(&self) -> PreparedHlsTrack {
        let presentation = &self.choice.catalog.presentation;
        PreparedHlsTrack(match self.choice.target {
            HlsTrackTarget::Audio(id) => Prepared::Audio(presentation.selected_for_test(id)),
            HlsTrackTarget::Subtitle(None) => Prepared::Off(presentation.clone()),
            HlsTrackTarget::Subtitle(Some(id)) => Prepared::Subtitle {
                playback: presentation.clone(), id, epoch: presentation.live_subtitle_revision(),
                content: sidecar::prepare_hls(&OffFrame::for_test(), self.choice.catalog.playback.clone(), id,
                    b"WEBVTT\n\n00:00.100 --> 00:00.800\nFixture\n".to_vec(), self.text_style, &mut || false).unwrap(),
            },
        })
    }
    /// Only a background application worker may perform this private fetch/probe.
    /// The result cannot publish selection; complete_hls_track_request owns that.
    pub fn prepare(&self, off: &OffFrame, cancelled: Arc<AtomicBool>) -> Result<PreparedHlsTrack, &'static str> {
        let presentation = &self.choice.catalog.presentation;
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
        let mut should_stop = || cancelled.load(std::sync::atomic::Ordering::Acquire)
            || std::time::Instant::now() >= deadline;
        if should_stop() { return Err("HLS preparation cancelled or timed out"); }
        let ready = match self.choice.target {
            HlsTrackTarget::Audio(id) => Prepared::Audio(presentation.prepare_rendition(id, cancelled.clone())?),
            HlsTrackTarget::Subtitle(None) => Prepared::Off(presentation.clone()),
            HlsTrackTarget::Subtitle(Some(id)) => {
                let (bytes, epoch) = presentation.prepare_subtitle_snapshot(id, cancelled.clone())?;
                if should_stop() { return Err("HLS preparation cancelled or timed out"); }
                let content = sidecar::prepare_hls(off, self.choice.catalog.playback.clone(), id, bytes,
                    self.text_style, &mut should_stop).map_err(|_| "HLS WebVTT conversion failed or cancelled")?;
                if should_stop() { return Err("HLS preparation cancelled or timed out"); }
                Prepared::Subtitle { playback: presentation.clone(), id, content, epoch }
            }
        };
        Ok(PreparedHlsTrack(ready))
    }
}

/// Main-thread projection of committed TS facts. Snapshot construction and the
/// registry's current-worker fence remain with the decoder publication owner.
pub fn reconcile_observed_hls_tracks(ps: &mut PlaybackSession) -> Result<bool, ExternalTrackApplyError> {
    let Some(snapshot) = crate::ff::hls_observed::observed_hls_tracks() else { return Ok(false); };
    install_observed_hls_tracks(ps, &snapshot)
}
pub fn observed_hls_tracks(ps: &PlaybackSession) -> Option<&crate::ff::hls_observed::Snapshot> { ps.hls_observed.as_ref() }
pub fn observed_hls_subtitle_selection(ps: &PlaybackSession) -> Option<crate::ff::hls_observed::Selection> {
    ps.hls_observed.as_ref()?;
    let tracks = ps.external_tracks.as_ref()?;
    crate::ff::hls_observed::Selection::new(tracks.inventory(), crate::ff::inspect::TrackKind::Subtitle,
        tracks.selected_subtitle()?).ok()
}
pub fn install_observed_hls_tracks(ps: &mut PlaybackSession, snapshot: &crate::ff::hls_observed::Snapshot)
    -> Result<bool, ExternalTrackApplyError>
{
    use crate::ff::hls_observed;
    if !hls_observed::snapshot_is_current(snapshot)
        || snapshot.scope().playback() != ps.playback_identity.as_ref()
        || !matches!(&ps.external_source, Some(crate::ff::ExternalSource::Hls(source)) if snapshot.scope().matches_source(source))
    { return Err(ExternalTrackApplyError::InvalidChoice(ExternalTrackError::StaleSession)); }
    if ps.hls_observed.as_ref().is_some_and(|previous| previous.scope() == snapshot.scope()
        && previous.sequence() > snapshot.sequence()) {
        return Err(ExternalTrackApplyError::InvalidChoice(ExternalTrackError::StaleSession));
    }
    let inventory = snapshot.inventory().clone();
    let audio = inventory.audio.iter().map(|track|
        classify_hls_audio(&inventory, track, &ps.attempt_caps(), ps.direct_play_mode)).collect();
    let subtitles = inventory.subtitles.iter().map(|track| classify_observed_hls_subtitle(&inventory, track)).collect();
    let force_identity = ps.hls_observed.as_ref().is_none_or(|previous| previous.scope() != snapshot.scope());
    let observed_audio = snapshot.selected_audio().and_then(|identity| {
        let mut rows = inventory.audio.iter().filter(|track| inventory.transport_identity(track.stream_index) == Some(identity));
        let ordinal = rows.next()?.ordinal;
        rows.next().is_none().then_some(ordinal)
    });
    let mut catalog = ps.external_tracks.as_ref().ok_or(ExternalTrackApplyError::Inactive)?.clone();
    let (changed, removed_subtitle) = catalog.replace_observed_inventory(inventory, audio, subtitles, force_identity, observed_audio)
        .map_err(ExternalTrackApplyError::InvalidChoice)?;
    if !changed { ps.hls_observed = Some(snapshot.clone()); return Ok(false); }
    let _edit = reserve_external_projection(false)?;
    if let Some(track) = catalog.selected_audio().and_then(|ordinal| catalog.inventory().audio.iter().find(|track| track.ordinal == ordinal)) {
        ps.cur_audio = Some(carried_audio(track));
        crate::player::set_audio_track(track.ordinal);
    }
    ps.external_tracks = Some(catalog);
    ps.hls_observed = Some(snapshot.clone());
    if removed_subtitle {
        hls_observed::clear_subtitle_selection();
        crate::player::request_subtitle(-1);
        crate::player::ass_source::retire_selection();
        ps.cur_sub_sid = 0; ps.cur_sub_sidecar = false; ps.cur_sub_client_drawable = false; ps.cur_sub_ordinal = -1;
    } else if let Some(ordinal) = ps.external_tracks.as_ref().and_then(|tracks| tracks.selected_subtitle()) {
        ps.cur_sub_ordinal = ordinal;
        crate::player::request_subtitle(ordinal);
    }
    commit_in_place_route_projection(ps, false);
    Ok(true)
}
// Packet decode and post-feed publication must both be wired before this gate
// can be enabled. A compiled decoder helper alone is not an active transport.
const OBSERVED_SUBTITLE_DECODER_READY: bool = false;
pub(super) fn classify_observed_hls_subtitle(inventory: &crate::ff::inspect::TrackInventory,
    track: &crate::ff::inspect::InspectedTrack) -> TrackAvailability {
    if crate::ff::hls_observed::Selection::new(inventory, crate::ff::inspect::TrackKind::Subtitle, track.ordinal).is_err() {
        return TrackAvailability::Unsupported(crate::route::TrackUnavailable::Transport);
    }
    if !OBSERVED_SUBTITLE_DECODER_READY {
        return TrackAvailability::Unsupported(crate::route::TrackUnavailable::Transport);
    }
    if !crate::ff::hls_embedded_subtitles::supported(track) {
        return TrackAvailability::Unsupported(crate::route::TrackUnavailable::Codec);
    }
    TrackAvailability::Supported
}
fn retire_observed_subtitle(ps: &mut PlaybackSession) -> bool {
    crate::ff::hls_observed::clear_subtitle_selection();
    let Some(tracks) = ps.external_tracks.as_mut() else { return false; };
    if tracks.selected_subtitle().is_none() { return false; }
    let Ok(off) = tracks.choice(crate::ff::inspect::TrackKind::Subtitle, None) else { return false; };
    if tracks.commit(&off).is_err() { return false; }
    crate::player::request_subtitle(-1);
    crate::player::ass_source::retire_selection();
    true
}

pub fn hls_track_catalog(ps: &PlaybackSession) -> Option<HlsTrackCatalog> {
    let playback = playback_identity(ps)?;
    Some(HlsTrackCatalog { subtitle: sidecar::selected_hls(&playback),
        playback, presentation: external_hls(ps)?.clone(), embedded: ps.external_tracks.clone() })
}
fn choice_is_current(ps: &PlaybackSession, choice: &HlsTrackChoice) -> bool {
    if ps.hls_observed.as_ref().is_some_and(|snapshot| !crate::ff::hls_observed::snapshot_is_current(snapshot)) { return false; }
    hls_track_catalog(ps).is_some_and(|catalog| catalog.same_session(&choice.catalog))
}
pub fn begin_hls_track_request(ps: &mut PlaybackSession, choice: &HlsTrackChoice)
    -> Result<HlsTrackRequest, ExternalTrackApplyError>
{
    let request = begin_hls_request(ps, choice)?;
    match choice.target {
        HlsTrackTarget::Subtitle(Some(_)) => ps.hls_subtitle_intent = HlsSubtitleIntent::ManualTrack,
        HlsTrackTarget::Subtitle(None) => ps.hls_subtitle_intent = HlsSubtitleIntent::ManualOff,
        HlsTrackTarget::Audio(_) => {},
    }
    Ok(request)
}
fn begin_hls_request(ps: &mut PlaybackSession, choice: &HlsTrackChoice)
    -> Result<HlsTrackRequest, ExternalTrackApplyError>
{
    if !choice_is_current(ps, choice) {
        return Err(ExternalTrackApplyError::InvalidChoice(ExternalTrackError::StaleSession));
    }
    if !has_url(ps) || ps.jail_load_blocked || ps.native_start_refused.is_some() || ps.engine_failure.is_some() {
        return Err(ExternalTrackApplyError::Inactive);
    }
    // Admit the user's intent now, before network work. Even same-target requests
    // supersede prior preparation; invalid choices above never advance the epoch.
    let _edit = reserve_external_track_edit()?;
    let text_style = TextStyle::current(subtitle_text_appearance(ps));
    let control = PLAYER_CONTROL.lock().unwrap_or_else(|e| e.into_inner());
    Ok(HlsTrackRequest { choice: choice.clone(), revision: control.desired_revision,
        worker: worker_ticket_of(&control),
        text_style, automatic_policy: None })
}
pub fn hls_track_request_is_current(ps: &PlaybackSession, request: &HlsTrackRequest) -> bool {
    if !choice_is_current(ps, &request.choice) { return false; }
    if request.automatic_policy.as_ref().is_some_and(|policy|
        ps.hls_subtitle_intent != HlsSubtitleIntent::Auto || &ps.hls_subtitle_policy != policy) { return false; }
    let control = PLAYER_CONTROL.lock().unwrap_or_else(|e| e.into_inner());
    control.desired_revision == request.revision && worker_ticket_of(&control) == request.worker
}
pub fn complete_hls_track_request(ps: &mut PlaybackSession, request: &HlsTrackRequest, prepared: PreparedHlsTrack)
    -> Result<bool, ExternalTrackApplyError>
{
    if !hls_track_request_is_current(ps, request) || !prepared.matches(&request.choice) {
        return Err(ExternalTrackApplyError::InvalidChoice(ExternalTrackError::StaleSession));
    }
    if let Prepared::Subtitle { playback, epoch, .. } = &prepared.0 {
        if playback.is_live_subtitles() && (*epoch == None || playback.live_subtitle_revision() != *epoch) {
            return Err(ExternalTrackApplyError::InvalidChoice(ExternalTrackError::StaleSession));
        }
    }
    if let Prepared::Audio(playback) = &prepared.0 {
        let track = playback.audio_track();
        if !ps.attempt_caps().native_audio_track(&track.codec, track.channels, track.sample_rate,
            ps.direct_play_mode == DirectPlayMode::Forced)
        {
            return Err(ExternalTrackApplyError::InvalidChoice(ExternalTrackError::Unsupported(
                crate::route::TrackUnavailable::Codec)));
        }
    }
    // No new intent: the private job owns the earlier revision. Recheck under
    // the existing projection reservation before touching renderer or session.
    let _edit = reserve_external_projection(false)?;
    if !hls_track_request_is_current(ps, request) {
        return Err(ExternalTrackApplyError::InvalidChoice(ExternalTrackError::StaleSession));
    }
    match prepared.0 {
        Prepared::Audio(playback) => {
            ps.cur_audio = Some(carried_audio(playback.audio_track()));
            ps.selection_known = true;
            ps.external_hls = Some(playback);
            // Zero selects the sole ES in the prepared audio-only lane. It is
            // not the presentation-scoped rendition identifier.
            crate::player::request_audio_track(ps, 0, "aac");
            crate::player::note_audio_pick();
        }
        Prepared::Subtitle { playback, id, content, epoch } => {
            let retired_embedded = retire_observed_subtitle(ps);
            let changed = sidecar::selected_hls(&request.choice.catalog.playback) != Some(id);
            if !sidecar::install_hls(&request.choice.catalog.playback, id, content) {
                return Err(ExternalTrackApplyError::InvalidChoice(ExternalTrackError::StaleSession));
            }
            commit_hls_subtitle(ps, true, changed);
            if retired_embedded { crate::route::request_user_route_intent(ps, UserRouteIntent::NativeAudioReload); }
            if playback.is_live_subtitles() {
                if let Some(token) = sidecar::hls_refresh_token(&request.choice.catalog.playback, id) {
                    if let Some(epoch) = epoch {
                        if !sidecar::bind_hls_epoch(&token, &playback, epoch) { sidecar::clear_hls_refresh(&token); }
                    }
                    start_live_refresh(LiveRefresh { playback, owner: request.choice.catalog.playback.clone(),
                        id, style: request.text_style, token, epoch });
                }
            }
        }
        Prepared::Off(_) => {
            let retired_embedded = retire_observed_subtitle(ps);
            sidecar::reset_hls();
            commit_hls_subtitle(ps, false, true);
            if retired_embedded { crate::route::request_user_route_intent(ps, UserRouteIntent::NativeAudioReload); }
        }
    }
    Ok(true)
}

// One live download worker and one latest-selection slot. Repeated menu picks
// cannot accumulate workers while a previous request is being cancelled.
struct LiveRefresh {
    playback: HlsPlayback,
    owner: PlaybackIdentity,
    id: HlsSubtitleId,
    style: TextStyle,
    token: sidecar::HlsRefreshToken,
    epoch: Option<u64>,
}
struct LiveWorker { running: bool, pending: Option<LiveRefresh> }
static LIVE_WORKER: std::sync::Mutex<LiveWorker> = std::sync::Mutex::new(LiveWorker { running: false, pending: None });
fn start_live_refresh(job: LiveRefresh) {
    let mut worker = LIVE_WORKER.lock().unwrap_or_else(|e| e.into_inner());
    worker.pending = Some(job);
    if worker.running { return; }
    worker.running = true;
    drop(worker);
    if !plx_base::task::spawn_off_frame("hls-subtitles", |off| loop {
        let job = {
            let mut worker = LIVE_WORKER.lock().unwrap_or_else(|e| e.into_inner());
            match worker.pending.take() {
                Some(job) => job,
                None => { worker.running = false; return; }
            }
        };
        run_live_refresh(off, job);
    }) {
        let mut worker = LIVE_WORKER.lock().unwrap_or_else(|e| e.into_inner());
        worker.running = false;
        worker.pending = None;
    }
}
fn run_live_refresh(off: &OffFrame, mut job: LiveRefresh) {
    use std::time::{Duration, Instant};
    let mut next = Instant::now() + Duration::from_secs(1);
    while sidecar::hls_refresh_is_current(&job.token) {
        let epoch = job.playback.live_subtitle_revision();
        if epoch != job.epoch {
            if !sidecar::clear_hls_refresh(&job.token) { return; }
            plx_machine::present::wake_from_worker();
            job.epoch = epoch;
            next = Instant::now();
        }
        if epoch.is_none() || Instant::now() < next {
            std::thread::sleep(Duration::from_millis(100));
            continue;
        }
        let snapshot = job.playback.prepare_subtitle_snapshot(job.id, job.token.cancelled.clone());
        if let Ok((bytes, prepared_epoch)) = snapshot {
            let mut stop = || !sidecar::hls_refresh_is_current(&job.token)
                || job.playback.live_subtitle_revision() != prepared_epoch;
            if let Ok(content) = sidecar::prepare_hls(off, job.owner.clone(), job.id, bytes, job.style, &mut stop) {
                if !stop() && prepared_epoch.is_some_and(|epoch| sidecar::refresh_hls(&job.token, content, &job.playback, epoch)) {
                    plx_machine::present::wake_from_worker();
                }
            }
        }
        next = Instant::now() + Duration::from_secs(1);
    }
}

fn commit_hls_subtitle(ps: &mut PlaybackSession, selected: bool, reset_offset: bool) {
    ps.cur_sub_sid = 0;
    ps.cur_sub_sidecar = selected;
    ps.cur_sub_client_drawable = selected;
    ps.cur_sub_ordinal = -1;
    ps.sub_mode = SubMode::Track;
    ps.selection_known = true;
    if reset_offset { crate::player::set_subtitle_offset(0); }
    crate::player::request_subtitle(-1);
    commit_in_place_route_projection(ps, false);
}

#[cfg(test)]
#[path = "external_hls_tracks_tests.rs"]
mod tests;

#[cfg(any(test, feature = "test-support"))]
mod test_globals {
    use super::*;
    use std::sync::atomic::Ordering::Relaxed;

    pub(super) struct Guard {
        audio: i32,
        subtitle: i32,
        position: i64,
        state: u8,
        paused: bool,
        subtitle_offset: i64,
        audio_switch: u8,
        audio_pick_ms: i64,
        restore_bitmaps: Option<Box<dyn FnOnce()>>,
    }
    impl Guard {
        pub(super) fn capture() -> Self {
            let shared = &crate::player::SHARED;
            let bitmaps = std::mem::take(&mut *shared.sub_bitmaps.lock()
                .unwrap_or_else(|error| error.into_inner()));
            Self {
                paused: crate::player::transport::paused(),
                subtitle_offset: crate::player::subtitle_offset_ms(),
                audio_switch: shared.audio_switch.load(Relaxed),
                audio_pick_ms: shared.audio_pick_ms.load(Relaxed),
                audio: shared.desired_audio_idx.load(Relaxed),
                subtitle: shared.desired_sub_idx.load(Relaxed),
                position: shared.playpos_ns.load(Relaxed),
                state: crate::player::swap_state_for_test(crate::player::PlaybackState::Playing),
                restore_bitmaps: Some(Box::new(move || {
                    *crate::player::SHARED.sub_bitmaps.lock()
                        .unwrap_or_else(|error| error.into_inner()) = bitmaps;
                })),
            }
        }
    }
    impl Drop for Guard {
        fn drop(&mut self) {
            sidecar::reset_hls();
            // A prior fixture's controller tickets must not be resurrected.
            // Match the established external-track fixture's idle teardown.
            reset_player_control_for_test(&PlaybackSession::IDLE);
            let shared = &crate::player::SHARED;
            shared.desired_audio_idx.store(self.audio, Relaxed);
            shared.desired_sub_idx.store(self.subtitle, Relaxed);
            shared.audio_switch.store(self.audio_switch, Relaxed);
            shared.audio_pick_ms.store(self.audio_pick_ms, Relaxed);
            shared.playpos_ns.store(self.position, Relaxed);
            if let Some(restore) = self.restore_bitmaps.take() { restore(); }
            crate::player::restore_subtitle_offset(self.subtitle_offset);
            crate::player::TX.commit_paused(self.paused);
            crate::player::restore_state_for_test(self.state);
        }
    }
}
