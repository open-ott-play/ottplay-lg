//! Separate HLS rendition choices. These are presentation/session identities,
//! never AVStream indexes, Plex stream IDs or persisted endpoint settings.
use super::*;
use crate::ff::hls_presentation::{HlsPlayback, HlsSubtitleId, RenditionId};
use std::sync::{Arc, atomic::AtomicBool};
use plx_base::task::OffFrame;
use crate::player::{sidecar, text_style::TextStyle};

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
            id, name: name.into(), language: language.into(), selected: self.subtitle == Some(id),
        }).collect()
    }
    pub fn selected_subtitle(&self) -> Option<HlsSubtitleId> { self.subtitle }
    pub fn subtitle_choice(&self, id: Option<HlsSubtitleId>) -> Result<HlsTrackChoice, ExternalTrackApplyError> {
        if id.is_some_and(|id| !self.presentation.has_subtitle(id)) {
            return Err(ExternalTrackApplyError::Inactive);
        }
        Ok(HlsTrackChoice { catalog: self.clone(), target: HlsTrackTarget::Subtitle(id) })
    }
    pub fn same_session(&self, other: &Self) -> bool {
        self.playback == other.playback && self.presentation.same_presentation(&other.presentation)
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
}
impl std::fmt::Debug for HlsTrackRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HlsTrackRequest").field("target", &self.choice.target).finish_non_exhaustive()
    }
}
pub struct PreparedHlsTrack(Prepared);
enum Prepared {
    Audio(HlsPlayback),
    Subtitle { playback: HlsPlayback, id: HlsSubtitleId, content: sidecar::PreparedHls },
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
                playback: presentation.clone(), id,
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
                let bytes = presentation.prepare_subtitle(id, cancelled.clone())?;
                if should_stop() { return Err("HLS preparation cancelled or timed out"); }
                let content = sidecar::prepare_hls(off, self.choice.catalog.playback.clone(), id, bytes,
                    self.text_style, &mut should_stop).map_err(|_| "HLS WebVTT conversion failed or cancelled")?;
                if should_stop() { return Err("HLS preparation cancelled or timed out"); }
                Prepared::Subtitle { playback: presentation.clone(), id, content }
            }
        };
        Ok(PreparedHlsTrack(ready))
    }
}

pub fn hls_track_catalog(ps: &PlaybackSession) -> Option<HlsTrackCatalog> {
    let playback = playback_identity(ps)?;
    Some(HlsTrackCatalog { subtitle: sidecar::selected_hls(&playback),
        playback, presentation: external_hls(ps)?.clone() })
}
fn choice_is_current(ps: &PlaybackSession, choice: &HlsTrackChoice) -> bool {
    hls_track_catalog(ps).is_some_and(|catalog| catalog.same_session(&choice.catalog))
}
pub fn begin_hls_track_request(ps: &mut PlaybackSession, choice: &HlsTrackChoice)
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
        text_style })
}
pub fn hls_track_request_is_current(ps: &PlaybackSession, request: &HlsTrackRequest) -> bool {
    if !choice_is_current(ps, &request.choice) { return false; }
    let control = PLAYER_CONTROL.lock().unwrap_or_else(|e| e.into_inner());
    control.desired_revision == request.revision && worker_ticket_of(&control) == request.worker
}
pub fn complete_hls_track_request(ps: &mut PlaybackSession, request: &HlsTrackRequest, prepared: PreparedHlsTrack)
    -> Result<bool, ExternalTrackApplyError>
{
    if !hls_track_request_is_current(ps, request) || !prepared.matches(&request.choice) {
        return Err(ExternalTrackApplyError::InvalidChoice(ExternalTrackError::StaleSession));
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
        Prepared::Subtitle { id, content, .. } => {
            let changed = sidecar::selected_hls(&request.choice.catalog.playback) != Some(id);
            if !sidecar::install_hls(&request.choice.catalog.playback, id, content) {
                return Err(ExternalTrackApplyError::InvalidChoice(ExternalTrackError::StaleSession));
            }
            commit_hls_subtitle(ps, true, changed);
        }
        Prepared::Off(_) => {
            sidecar::reset_hls();
            commit_hls_subtitle(ps, false, true);
        }
    }
    Ok(true)
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
