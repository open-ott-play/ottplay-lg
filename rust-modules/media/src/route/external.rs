//! Install independently inspected HTTP media without manufacturing a Plex identity.
//! The ordinary buffer-feed engine and lifecycle retain ownership of native Load/Unload.
use super::*;
use crate::ff::inspect::MediaInfo;
use crate::route::{carried_audio, classify_audio, classify_hls_audio, classify_subtitle, ExternalTrackChoice,
    ExternalTrackError, ExternalTrackPlan, ExternalTrackTransport, TrackAvailability, LocalSubtitleTrack};

#[path = "external_continuation.rs"]
mod continuation_tracks;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExternalError {
    InvalidAddress,
    InvalidTracks,
    UnsupportedPlaylist,
    UnsupportedVideo,
    UnsupportedAudio,
    Busy,
    StalePlayback,
}
impl std::fmt::Display for ExternalError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::InvalidAddress => "Invalid HTTP media address",
            Self::InvalidTracks => "Invalid embedded track inventory",
            Self::UnsupportedPlaylist => {
                "This segmented playlist is not supported by native direct playback"
            }
            Self::UnsupportedVideo => "This video requires a compatible server stream",
            Self::UnsupportedAudio => "This audio requires a compatible server stream",
            Self::Busy => "The previous playback is still stopping",
            Self::StalePlayback => "The playback to continue is no longer current",
        })
    }
}

/// Main-thread commit, after the caller stopped the previous Engine and discarded stale
/// inspection results. A successful return prepares a route; it does not claim a decoder started.
pub fn request_external(
    ps: &mut PlaybackSession,
    info: &MediaInfo,
    url: &str,
    title: &str,
    context: &str,
    caps: std::sync::Arc<plx_platform::devcaps::Caps>,
) -> Result<(), ExternalError> {
    request_external_source(
        ps,
        info,
        crate::ff::ExternalSource::Http(url.into()),
        title,
        context,
        caps,
    )
}

#[path = "external_request.rs"]
mod request;
pub use request::{apply_external_track_request, cancel_external_track_request, external_track_request_status, ExternalTrackRequest, ExternalTrackRequestStatus};

pub fn external_source(ps: &PlaybackSession) -> Option<crate::ff::ExternalSource> {
    ps.external_source.clone()
}
pub fn external_hls(ps: &PlaybackSession) -> Option<&crate::ff::hls_presentation::HlsPlayback> {
    ps.external_source.as_ref()?;
    ps.external_hls.as_ref()
}

pub fn external_duration_ns(ps: &PlaybackSession) -> i64 {
    ps.external_duration_ns
}

/// Only an inspected, selected embedded MP2 track can declare the PCM HLS leg.
/// Separate AUDIO presentations and ordinary/Plex media keep their own policy.
pub(super) fn embedded_mp2_pcm(ps: &PlaybackSession) -> bool {
    if !matches!(ps.external_source, Some(crate::ff::ExternalSource::Hls(_)))
        || ps.external_hls.as_ref().is_some_and(|hls| !hls.is_live_subtitles()) || ps.stream_acodec != "mp2"
    { return false; }
    let Some(tracks) = &ps.external_tracks else { return false; };
    let Some(track) = tracks.inventory().audio.iter()
        .find(|track| Some(track.ordinal) == tracks.selected_audio()) else { return false; };
    ps.cur_audio.as_ref().is_some_and(|audio| audio.codec == track.codec
        && audio.channels == track.channels && audio.sample_rate == track.sample_rate)
        && crate::ff::hls_mp2::supported(track, &ps.attempt_caps())
}

pub fn external_tracks(ps: &PlaybackSession) -> Option<&ExternalTracks> {
    ps.external_source.as_ref()?;
    ps.external_tracks.as_ref()
}

fn track_transport(source: &crate::ff::ExternalSource) -> ExternalTrackTransport {
    if matches!(source, crate::ff::ExternalSource::Hls(_)) {
        ExternalTrackTransport::Hls
    } else {
        ExternalTrackTransport::Demux
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExternalTrackApplyError {
    InvalidChoice(ExternalTrackError),
    Inactive,
    Busy,
}

/// Atomically reserve an otherwise idle route before changing any selection or
/// player mailbox. A historical claimed_user record is not an active claim:
/// its authority exists only while the phase is Applying, which this gate refuses.
fn reserve_external_track_edit() -> Result<UserEditGuard, ExternalTrackApplyError> {
    reserve_external_projection(true)
}

/// The same idle-route reservation for passive catalog/rearm publication. Its
/// guard protects the projection without manufacturing a new user intention.
fn reserve_external_projection(user_intent: bool) -> Result<UserEditGuard, ExternalTrackApplyError> {
    let mut control = PLAYER_CONTROL.lock().unwrap_or_else(|e| e.into_inner());
    if control.phase != ControlPhase::Stable
        || control.pending_user.is_some()
        || control.pending_auto.is_some()
        || control.pending_seek_ns.is_some()
        || control.flight.is_some()
        || control.pending_original.is_some()
        || control.start_deferred.is_some()
    {
        return Err(ExternalTrackApplyError::Busy);
    }
    if user_intent { advance_user_contract_locked(&mut control); }
    control.next_action = next_generation(control.next_action);
    let serial = control.next_action;
    control.phase = ControlPhase::StagingUser(serial);
    Ok(UserEditGuard { serial: Some(serial) })
}

/// Admit a menu choice for the exact external playback which minted it. Audio
/// enters the existing native reload path; subtitles change their renderer ordinal
/// in place. Neither action manufactures a Plex identity or issues a server request.
/// Success means admitted, not that a decoder has already presented the new track.
pub fn apply_external_track(
    ps: &mut PlaybackSession,
    choice: &ExternalTrackChoice,
) -> Result<bool, ExternalTrackApplyError> {
    let source = ps.external_source.as_ref().ok_or(ExternalTrackApplyError::Inactive)?;
    if !has_url(ps) || ps.jail_load_blocked || ps.native_start_refused.is_some()
        || ps.engine_failure.is_some()
    {
        return Err(ExternalTrackApplyError::Inactive);
    }
    if ps.hls_observed.as_ref().is_some_and(|snapshot| !crate::ff::hls_observed::snapshot_is_current(snapshot)) {
        return Err(ExternalTrackApplyError::InvalidChoice(ExternalTrackError::StaleSession));
    }
    let transport = track_transport(source);
    let mut selected = ps.external_tracks.as_ref()
        .ok_or(ExternalTrackApplyError::Inactive)?.clone();
    // Prepare the entire after-image locally. A failed identity/policy/busy check
    // must leave the session, catalog and all player atomics untouched.
    let mut plan = selected.commit(choice).map_err(ExternalTrackApplyError::InvalidChoice)?;
    let hls_subtitle_edit = (ps.external_hls.is_some() || ps.hls_observed.is_some()) && choice.kind() == crate::ff::inspect::TrackKind::Subtitle;
    // The embedded catalog may already say Off while an HLS rendition is
    // selected in the shared sidecar. An accepted embedded Off must retire it.
    if hls_subtitle_edit && matches!(plan, ExternalTrackPlan::Unchanged)
        && (choice.ordinal().is_some() && ps.hls_observed.is_some()
            || playback_identity(ps).as_ref().and_then(|owner| crate::player::sidecar::selected_hls(owner)).is_some())
    {
        plan = ExternalTrackPlan::Subtitle(choice.ordinal().and_then(|ordinal|
            selected.inventory().subtitles.iter().find(|track| track.ordinal == ordinal).cloned()));
    }
    let supported = match &plan {
        ExternalTrackPlan::Unchanged => TrackAvailability::Supported,
        ExternalTrackPlan::Audio(track) => if transport == ExternalTrackTransport::Hls {
            classify_hls_audio(selected.inventory(), track, &ps.attempt_caps(), ps.direct_play_mode)
        } else {
            classify_audio(track, &ps.attempt_caps(), ps.direct_play_mode, transport)
        },
        // This build ships the text renderer. Per-track parse/render failures remain runtime errors.
        ExternalTrackPlan::Subtitle(Some(track)) => if transport == ExternalTrackTransport::Hls && ps.hls_observed.is_some() {
            hls_tracks::classify_observed_hls_subtitle(selected.inventory(), track)
        } else { classify_subtitle(track, transport, true) },
        ExternalTrackPlan::Subtitle(None) => TrackAvailability::Supported,
        ExternalTrackPlan::LocalSubtitle(_) => {
            local_subtitle_playback(ps)?;
            TrackAvailability::Supported
        }
    };
    if let TrackAvailability::Unsupported(reason) = supported {
        return Err(ExternalTrackApplyError::InvalidChoice(ExternalTrackError::Unsupported(reason)));
    }
    let _edit = reserve_external_track_edit()?;
    if hls_subtitle_edit {
        if let Some(ordinal) = choice.ordinal() {
            if let Some(snapshot) = &ps.hls_observed {
                let target = crate::ff::hls_observed::Selection::new(selected.inventory(), crate::ff::inspect::TrackKind::Subtitle, ordinal)
                    .map_err(|_| ExternalTrackApplyError::InvalidChoice(ExternalTrackError::StaleSession))?;
                crate::ff::hls_observed::set_subtitle_selection(snapshot, target)
                    .map_err(|_| ExternalTrackApplyError::InvalidChoice(ExternalTrackError::StaleSession))?;
            }
        } else { crate::ff::hls_observed::clear_subtitle_selection(); }
        ps.hls_subtitle_intent = if choice.ordinal().is_some() || choice.local_target().is_some() {
            HlsSubtitleIntent::ManualTrack
        } else { HlsSubtitleIntent::ManualOff };
    }
    match plan {
        ExternalTrackPlan::Audio(track) => {
            ps.cur_audio = Some(carried_audio(&track));
            ps.selection_known = true;
            ps.external_tracks = Some(selected);
            crate::player::request_audio_track(ps, track.ordinal, &track.codec);
            crate::player::note_audio_pick();
        }
        ExternalTrackPlan::Subtitle(track) => {
            if ps.hls_observed.is_some() {
                crate::player::request_subtitle(-1);
                crate::player::ass_source::retire_selection();
            }
            crate::player::sidecar::deselect();
            let ordinal = track.as_ref().map_or(-1, |track| track.ordinal);
            ps.cur_sub_sid = 0;
            ps.cur_sub_sidecar = false;
            ps.cur_sub_client_drawable = track.is_some();
            ps.cur_sub_ordinal = ordinal;
            ps.sub_mode = SubMode::Track;
            ps.selection_known = true;
            ps.external_tracks = Some(selected);
            crate::player::set_subtitle_offset(0);
            crate::player::request_subtitle(ordinal);
            commit_in_place_route_projection(ps, false);
            if ps.hls_observed.is_some() {
                crate::route::request_user_route_intent(ps, UserRouteIntent::NativeAudioReload);
            }
        }
        ExternalTrackPlan::LocalSubtitle(track) => {
            let playback = local_subtitle_playback(ps)?;
            ps.cur_sub_sid = 0;
            ps.cur_sub_sidecar = true;
            ps.cur_sub_client_drawable = true;
            ps.cur_sub_ordinal = -1;
            ps.sub_mode = SubMode::Track;
            ps.selection_known = true;
            ps.external_tracks = Some(selected);
            crate::player::set_subtitle_offset(0);
            crate::player::request_subtitle(-1);
            crate::player::sidecar::select_local(playback, track.target, track.format,
                crate::player::text_style::TextStyle::current(subtitle_text_appearance(ps)));
            commit_in_place_route_projection(ps, false);
        }
        // Even an accepted no-op retires older user intent through the same
        // admission boundary. It does not request a decoder or renderer change.
        ExternalTrackPlan::Unchanged => return Ok(false),
    }
    Ok(true)
}

/// This proves only a live native Local route. App-owned root/volume authorization
/// is deliberately not reconstructed here from a path or logical playback token.
fn local_subtitle_playback(ps: &PlaybackSession) -> Result<PlaybackIdentity, ExternalTrackApplyError> {
    if !ps.external_source.as_ref().is_some_and(crate::ff::ExternalSource::is_local)
        || !has_url(ps) || ps.jail_load_blocked || ps.native_start_refused.is_some()
        || ps.engine_failure.is_some()
    { return Err(ExternalTrackApplyError::Inactive); }
    ps.playback_identity.clone().ok_or(ExternalTrackApplyError::Inactive)
}

/// Publish only a discovery addressed to the exact playing item and catalog.
/// The app checks the current root/volume receipt before calling this. No file is
/// opened or selected. Every accepted replacement invalidates prior menu choices.
/// Returns true when the currently selected Local asset was removed.
pub fn install_external_local_subtitles(
    ps: &mut PlaybackSession, expected: &PlaybackIdentity, expected_catalog: &ExternalTracks,
    rows: Vec<LocalSubtitleTrack>, complete: bool, max_total: usize, max_kind: usize,
) -> Result<bool, ExternalTrackApplyError> {
    if local_subtitle_playback(ps)? != *expected {
        return Err(ExternalTrackApplyError::InvalidChoice(ExternalTrackError::StaleSession));
    }
    let current = ps.external_tracks.as_ref().ok_or(ExternalTrackApplyError::Inactive)?;
    if !current.same_session(expected_catalog) {
        return Err(ExternalTrackApplyError::InvalidChoice(ExternalTrackError::StaleSession));
    }
    let mut updated = current.clone();
    let removed = updated.replace_local_subtitles(rows, complete, max_total, max_kind)
        .map_err(ExternalTrackApplyError::InvalidChoice)?;
    let _edit = reserve_external_projection(false)?;
    ps.external_tracks = Some(updated);
    if removed {
        crate::player::sidecar::reset_local();
        ps.cur_sub_sid = 0;
        ps.cur_sub_sidecar = false;
        ps.cur_sub_client_drawable = false;
        ps.cur_sub_ordinal = -1;
        crate::player::set_subtitle_offset(0);
        crate::player::request_subtitle(-1);
    }
    commit_in_place_route_projection(ps, false);
    Ok(removed)
}

/// Requeue a selected Local subtitle after the app has revalidated the current
/// playback/root/volume binding. Ordinary pause and seek need no requeue/reset.
/// No-op when a non-Local subtitle is selected. Does not change the user's choice.
pub fn rearm_selected_local_subtitle(ps: &PlaybackSession) -> Result<bool, ExternalTrackApplyError> {
    let playback = local_subtitle_playback(ps)?;
    let tracks = ps.external_tracks.as_ref().ok_or(ExternalTrackApplyError::Inactive)?;
    let Some(target) = tracks.selected_local_subtitle() else { return Ok(false); };
    let track = tracks.local_subtitles().iter().find(|row| &row.target == target)
        .ok_or(ExternalTrackApplyError::InvalidChoice(ExternalTrackError::MissingTrack))?;
    let _edit = reserve_external_projection(false)?;
    crate::player::sidecar::select_local(playback, track.target.clone(), track.format,
        crate::player::text_style::TextStyle::current(subtitle_text_appearance(ps)));
    Ok(true)
}

/// Forget a retired external source after its playback owner has stopped it.
/// Resetting local data must not retain an address or lease in an idle route.
pub fn forget_external_source(ps: &mut PlaybackSession) {
    if ps.hls_observed.is_some() {
        crate::ff::hls_observed::clear_subtitle_selection();
        crate::player::ass_source::retire_selection();
        crate::player::request_subtitle(-1);
    }
    crate::player::sidecar::reset_hls();
    crate::player::sidecar::reset_local();
    ps.playback_identity = None;
    ps.external_source = None;
    ps.external_hls = None;
    ps.hls_observed = None;
    ps.external_duration_ns = 0;
    ps.external_tracks = None;
    clear_url(ps);
}

pub fn request_external_source(
    ps: &mut PlaybackSession,
    info: &MediaInfo,
    source: crate::ff::ExternalSource,
    title: &str,
    context: &str,
    caps: std::sync::Arc<plx_platform::devcaps::Caps>,
) -> Result<(), ExternalError> {
    install_external_source(ps, info, source, title, context, caps, None)
}

/// Re-resolve the same logical external item after a managed URL expired or a
/// suspend released its server lease. The caller must retain the item binding
/// and supply freshly validated media; this token grants no filesystem/network
/// authority. A newer logical target always rejects an old continuation.
#[allow(clippy::too_many_arguments)]
pub fn continue_external_source(
    ps: &mut PlaybackSession,
    info: &MediaInfo,
    source: crate::ff::ExternalSource,
    title: &str,
    context: &str,
    caps: std::sync::Arc<plx_platform::devcaps::Caps>,
    expected: &PlaybackIdentity,
) -> Result<(), ExternalError> {
    if !playback_identity_matches(ps, expected) || ps.external_source.is_none() {
        return Err(ExternalError::StalePlayback);
    }
    install_external_source(ps, info, source, title, context, caps, Some(expected))
}

#[allow(clippy::too_many_arguments)]
fn install_external_source(
    ps: &mut PlaybackSession,
    info: &MediaInfo,
    source: crate::ff::ExternalSource,
    title: &str,
    context: &str,
    caps: std::sync::Arc<plx_platform::devcaps::Caps>,
    continuation: Option<&PlaybackIdentity>,
) -> Result<(), ExternalError> {
    use crate::ff::ExternalSource;
    // Only the existing logical-item continuation may retain per-item choices.
    // Recheck before reading them; equal media URLs never grant this authority.
    if continuation.is_some_and(|expected| {
        !playback_identity_matches(ps, expected) || ps.external_source.is_none()
    })
    {
        return Err(ExternalError::StalePlayback);
    }
    let mut retry = continuation.map(|_| current_retry_context(ps, 0));
    if matches!(source, ExternalSource::OwnedHttp(_))
        && info.resolved_source.as_ref().is_some_and(|resolved| resolved != &source)
    {
        return Err(ExternalError::InvalidAddress);
    }
    let source = info.resolved_source.clone().unwrap_or(source);
    let (source, url) = match source {
        ExternalSource::Http(url) => {
            let url = plx_data::ott::http_url(&url).ok_or(ExternalError::InvalidAddress)?;
            if url
                .split('?')
                .next()
                .unwrap_or(&url)
                .to_ascii_lowercase()
                .ends_with(".m3u8")
            {
                return Err(ExternalError::UnsupportedPlaylist);
            }
            (ExternalSource::Http(url.clone()), url)
        }
        ExternalSource::OwnedHttp(url) => {
            if !crate::curlio::owned_http_url_allowed(&url) { return Err(ExternalError::InvalidAddress); }
            (ExternalSource::OwnedHttp(url.clone()), url)
        }
        ExternalSource::Hls(url) => {
            let url = plx_data::ott::http_url(&url).ok_or(ExternalError::InvalidAddress)?;
            (ExternalSource::Hls(url.clone()), url)
        }
        ExternalSource::HeldLocal(file) => {
            if !file.is_live() { return Err(ExternalError::InvalidAddress); }
            let url = format!("file://{}", file.path().to_str().ok_or(ExternalError::InvalidAddress)?);
            (ExternalSource::HeldLocal(file), url)
        }
        ExternalSource::Local(path) => {
            if !path.is_absolute()
                || path.components().any(|c| {
                    matches!(
                        c,
                        std::path::Component::ParentDir | std::path::Component::CurDir
                    )
                })
            {
                return Err(ExternalError::InvalidAddress);
            }
            let url = format!(
                "file://{}",
                path.to_str()
                    .filter(|s| !s.chars().any(char::is_control))
                    .ok_or(ExternalError::InvalidAddress)?
            );
            (ExternalSource::Local(path), url)
        }
    };
    if let Some(playback) = &info.hls_playback {
        if !matches!(&source, ExternalSource::Hls(url) if playback.matches_source(url)) {
            return Err(ExternalError::InvalidAddress);
        }
    }
    // Build a complete candidate first: validation cannot partially replace the old session.
    let mut next = PlaybackSession::IDLE;
    next.subtitle_text_appearance = ps.subtitle_text_appearance;
    next.native_subtitle_preferences = ps.native_subtitle_preferences;
    next.hls_subtitle_policy = ps.hls_subtitle_policy.clone();
    if continuation.is_some() && matches!(ps.hls_subtitle_intent, HlsSubtitleIntent::ManualTrack | HlsSubtitleIntent::ManualOff) {
        next.hls_subtitle_intent = ps.hls_subtitle_intent;
    }
    next.begin_attempt(caps.clone());
    if info.width == 0
        || info.height == 0
        || !info.fps.is_finite()
        || info.fps < 0.0
        || !set_stream_declaration(
            &mut next,
            &info.video_codec,
            &info.audio_codec,
            info.fps,
            info.dovi,
            false,
        )
        || !super::super::plan::video_direct_plays(
            &info.video_codec,
            i64::from(info.width),
            i64::from(info.height),
            stream_dv_presentation(&next),
            &caps,
        )
    {
        return Err(ExternalError::UnsupportedVideo);
    }
    set_stream_source_raster(&mut next, info.width, info.height);
    next.src_vcodec = info.video_codec.clone();
    next.src_acodec = info.audio_codec.clone();
    next.src_fps = info.fps;
    next.src_dovi = info.dovi;
    next.src_dv_decision = next.stream_dv_decision;
    let embedded_mp2 = matches!(&source, ExternalSource::Hls(_)) && info.hls_playback.as_ref().is_none_or(|hls| hls.is_live_subtitles())
        && info.audio_codec == "mp2" && info.tracks.audio.first()
            .is_some_and(|track| crate::ff::hls_mp2::supported(track, &caps));
    if !info.audio_codec.is_empty() {
        // AAC is native. Embedded MP2 is independently decoded to the admitted
        // PCM sink; no native MPEG capability is inferred from decoder presence.
        if matches!(&source, ExternalSource::Hls(_))
            && !embedded_mp2
            && !caps.native_audio_track(&info.audio_codec, info.audio_channels,
                info.audio_sample_rate, next.direct_play_mode == DirectPlayMode::Forced)
        {
            return Err(ExternalError::UnsupportedAudio);
        }
        next.cur_audio = Some(CarriedAudio {
            sid: 0,
            ordinal: 0,
            codec: info.audio_codec.clone(),
            channels: info.audio_channels,
            can_normalize_loudness: false,
            immersive: false,
            sample_rate: info.audio_sample_rate,
            bit_depth: info.audio_bit_depth,
        });
        if !audio_track_direct_plays(
            &next,
            &info.audio_codec,
            info.audio_channels,
            info.audio_sample_rate,
        ) && !matches!(stream_pcm(&next), Some(Ok(_))) && !embedded_mp2
        {
            return Err(ExternalError::UnsupportedAudio);
        }
    }
    let transport = track_transport(&source);
    let audio = info.tracks.audio.iter().map(|track| {
        if transport == ExternalTrackTransport::Hls {
            classify_hls_audio(&info.tracks, track, &caps, next.direct_play_mode)
        } else {
            classify_audio(track, &caps, next.direct_play_mode, transport)
        }
    }).collect();
    let subtitles = info.tracks.subtitles.iter()
        .map(|track| classify_subtitle(track, transport, true)).collect();
    let initial_audio = info.tracks.audio.first().map(|track| track.ordinal);
    let mut tracks = ExternalTracks::new(info.tracks.clone(), audio, subtitles, initial_audio)
        .map_err(|_| ExternalError::InvalidTracks)?;
    if let Some(track) = info.tracks.audio.first() {
        // Scalar compatibility fields and the inventory describe the same inspected
        // first stream. Do not stage another codec under that initial declaration.
        if plx_platform::devcaps::audio_codec_id(&track.codec)
            != plx_platform::devcaps::audio_codec_id(&info.audio_codec)
        {
            return Err(ExternalError::InvalidTracks);
        }
        if classify_audio(track, &caps, next.direct_play_mode, ExternalTrackTransport::Demux)
            != TrackAvailability::Supported
            && !embedded_mp2
        {
            return Err(ExternalError::UnsupportedAudio);
        }
        next.cur_audio = Some(carried_audio(track));
    }
    if let Some(retry) = retry.as_mut() {
        let subtitle = ps.external_tracks.as_ref().map_or(
            continuation_tracks::SubtitleContinuation::Missing,
            |previous| continuation_tracks::restore(previous, &mut tracks),
        );
        // A different/missing subtitle must not inherit timing tuned to the old
        // one. Explicit Off remains Off even if a new track is marked default.
        if !matches!(subtitle, continuation_tracks::SubtitleContinuation::Retained) {
            retry.sub_offset_ms = 0;
        }
        if let Some(track) = tracks.selected_audio()
            .and_then(|ordinal| usize::try_from(ordinal).ok())
            .and_then(|index| tracks.inventory().audio.get(index))
        {
            next.cur_audio = Some(carried_audio(track));
            set_stream_acodec(&mut next, &track.codec);
        }
        if let Some(ordinal) = tracks.selected_subtitle() {
            next.cur_sub_sid = 0;
            next.cur_sub_sidecar = false;
            next.cur_sub_client_drawable = true;
            next.cur_sub_ordinal = ordinal;
            next.sub_mode = retry.sub_mode;
        }
    }
    let selected_audio = tracks.selected_audio();
    let selected_subtitle = tracks.selected_subtitle();
    next.external_tracks = Some(tracks);
    next.url = url;
    next.external_source = Some(source);
    next.external_hls = info.hls_playback.clone();
    next.external_duration_ns = info.duration_ns.max(0);
    next.now_ms = ps.now_ms;
    next.selection_known = true;
    unsafe {
        set_c(next.title.as_mut_ptr(), next.title.len(), title);
        set_c(next.ctxline.as_mut_ptr(), next.ctxline.len(), context);
    }
    // A caller must stop its old native engine, not merely hide its screen.
    if has_url(ps) {
        return Err(ExternalError::Busy);
    }
    cancel_play(ps);
    if !begin_playback_request() {
        return Err(ExternalError::Busy);
    }
    next.playback_identity = Some(continuation.cloned().unwrap_or_else(PlaybackIdentity::fresh));
    if continuation.is_none() { crate::ff::hls_observed::clear_subtitle_selection(); }
    *ps = next;
    install_active_encoder("");
    reset_track_selection(ServerId::UNSET, "", retry);
    crate::player::set_audio_track(selected_audio.unwrap_or(0));
    if continuation.is_some() {
        if let Some(ordinal) = selected_subtitle { crate::player::request_subtitle(ordinal); }
        // Retry restoration allows a sidecar's advance; this catalog contains
        // only embedded tracks, whose timing must use the embedded range.
        crate::player::reclamp_subtitle_offset();
    }
    prepare_playback_landing(ps, true).ok_or(ExternalError::Busy)?;
    plx_machine::idle::invalidate();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn media() -> MediaInfo {
        MediaInfo {
            tracks: Default::default(),
            resolved_source: None,
            hls_playback: None,
            video_codec: "h264".into(),
            audio_codec: "aac".into(),
            width: 1280,
            height: 720,
            fps: 24.0,
            duration_ns: 1_000_000_000,
            audio_channels: 2,
            audio_sample_rate: 48000,
            audio_bit_depth: 16,
            dovi: plx_data::metadata::Dovi::NONE,
        }
    }
    #[test]
    fn invalid_input_does_not_replace_existing_source() {
        let mut ps = PlaybackSession::IDLE;
        ps.url = "https://old.invalid:443/file.mp4".into();
        assert_eq!(
            request_external(
                &mut ps,
                &media(),
                "file:///private/video.mp4",
                "Film",
                "",
                plx_platform::devcaps::assumed_shared()
            ),
            Err(ExternalError::InvalidAddress)
        );
        assert_eq!(ps.url, "https://old.invalid:443/file.mp4");
    }
    #[test]
    fn segmented_media_is_not_misrepresented_as_progressive() {
        let mut ps = PlaybackSession::IDLE;
        assert_eq!(
            request_external(
                &mut ps,
                &media(),
                "https://example.invalid/master.m3u8",
                "Film",
                "",
                plx_platform::devcaps::assumed_shared()
            ),
            Err(ExternalError::UnsupportedPlaylist)
        );
    }
    #[test]
    fn prepared_external_route_has_no_plex_identity_and_preserves_local_source() {
        let _guard = plx_base::testlock::serial();
        let mut ps = PlaybackSession::IDLE;
        reset_player_control_for_test(&ps);
        let source = crate::ff::ExternalSource::Local(std::path::PathBuf::from("/media/film.mp4"));
        assert_eq!(
            request_external_source(
                &mut ps,
                &media(),
                source.clone(),
                "Film",
                "Local",
                plx_platform::devcaps::assumed_shared()
            ),
            Ok(())
        );
        assert_eq!(external_source(&ps), Some(source.clone()));
        assert_eq!(external_source(&ps.publication()), Some(source));
        assert_eq!(external_duration_ns(&ps), 1_000_000_000);
        assert!(!ps.cur_sid.is_set());
        assert!(ps.cur_rk.is_empty());
        assert!(ps.sess.is_empty());
        assert!(ps.request.is_none());
        assert!(active_encoder().is_empty());
        assert!(matches!(
            PLAYER_CONTROL.lock().unwrap().phase,
            ControlPhase::Prepared(_)
        ));
        clear_url(&mut ps);
        assert!(
            external_source(&ps).is_some(),
            "engine reload retains its typed source"
        );
        reset_player_control_for_test(&PlaybackSession::IDLE);
    }

    // Admission regressions: no engine, demuxer, filesystem, network, or TV is started.
    mod track_admission {
        use super::*;
        use crate::ff::inspect::{InspectedTrack, TrackKind};
        use crate::route::{apply_external_track, external_tracks, ExternalTrackApplyError,
            ExternalTrackError, TrackUnavailable};
        use std::sync::atomic::Ordering::Relaxed;

        struct Globals {
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
        impl Globals {
            fn capture() -> Self {
                let shared = &crate::player::SHARED;
                let bitmaps = std::mem::take(&mut *shared.sub_bitmaps.lock().unwrap());
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
                        *crate::player::SHARED.sub_bitmaps.lock().unwrap() = bitmaps;
                    })),
                }
            }
        }
        impl Drop for Globals {
            fn drop(&mut self) {
                reset_player_control_for_test(&PlaybackSession::IDLE);
                let shared = &crate::player::SHARED;
                shared.desired_audio_idx.store(self.audio, Relaxed);
                shared.desired_sub_idx.store(self.subtitle, Relaxed);
                shared.audio_switch.store(self.audio_switch, Relaxed);
                shared.audio_pick_ms.store(self.audio_pick_ms, Relaxed);
                shared.playpos_ns.store(self.position, Relaxed);
                if let Some(restore) = self.restore_bitmaps.take() {
                    restore();
                }
                crate::player::restore_subtitle_offset(self.subtitle_offset);
                crate::player::TX.commit_paused(self.paused);
                crate::player::restore_state_for_test(self.state);
            }
        }

        fn row(kind: TrackKind, index: i32, ordinal: i32, codec: &str) -> InspectedTrack {
            InspectedTrack {
                kind, stream_index: index, ordinal, codec: codec.into(),
                language: if ordinal == 0 { "eng" } else { "spa" }.into(),
                title: format!("Synthetic track {ordinal}"),
                channels: if kind == TrackKind::Audio { 2 } else { 0 },
                sample_rate: if kind == TrackKind::Audio { 48_000 } else { 0 },
                bit_depth: 16, decoder_available: true,
                disposition_known: false, default: false, forced: false,
            }
        }
        fn multitrack() -> MediaInfo {
            let mut info = media();
            info.duration_ns = 120_000_000_000;
            info.tracks.audio = vec![row(TrackKind::Audio, 1, 0, "flac"),
                row(TrackKind::Audio, 3, 1, "flac")];
            info.tracks.audio[0].channels = 1;
            info.tracks.audio[0].sample_rate = 44_100;
            info.tracks.audio[1].sample_rate = 96_000;
            info.tracks.audio[1].bit_depth = 24;
            // Bitmap selection is independent of host libass initialization.
            info.tracks.subtitles = vec![row(TrackKind::Subtitle, 2, 0, "hdmv_pgs_subtitle")];
            info.audio_codec = "flac".into();
            info.audio_channels = 1;
            info.audio_sample_rate = 44_100;
            info.audio_bit_depth = 16;
            info
        }
        fn admit(info: &MediaInfo, caps: std::sync::Arc<plx_platform::devcaps::Caps>) -> PlaybackSession {
            let mut ps = PlaybackSession::IDLE;
            reset_player_control_for_test(&ps);
            assert_eq!(request_external_source(&mut ps, info,
                crate::ff::ExternalSource::Http("https://example.invalid/movie.mkv".into()),
                "Synthetic film", "External", caps), Ok(()));
            // Settle only the controller; an actual native Load is deliberately not exercised.
            reset_player_control_for_test(&ps);
            crate::player::SHARED.playpos_ns.store(23_700_000_000, Relaxed);
            assert!(external_tracks(&ps).is_some(), "real source admission installs its inventory");
            ps
        }
        fn assert_route_unchanged(before: &AppliedRouteProjection, ps: &PlaybackSession) {
            let after = route_projection(ps);
            match (&before.external_tracks, &after.external_tracks) {
                (Some(before), Some(after)) => {
                    assert!(after.same_session(before));
                    assert_eq!(after.inventory(), before.inventory());
                    assert_eq!(after.selected_audio(), before.selected_audio());
                    assert_eq!(after.selected_subtitle(), before.selected_subtitle());
                }
                (None, None) => {}
                _ => panic!("rejected action changed external catalog presence"),
            }
            assert_eq!(after.external_source, before.external_source);
            assert_eq!(after.external_duration_ns, before.external_duration_ns);
            assert_eq!(after.url, before.url);
            assert_eq!(after.tsession, before.tsession);
            assert_eq!(after.contract, before.contract);
            assert_eq!(after.audio, before.audio);
            assert_eq!(after.subtitle_sid, before.subtitle_sid);
            assert_eq!(after.subtitle_sidecar, before.subtitle_sidecar);
            assert_eq!(after.subtitle_client_drawable, before.subtitle_client_drawable);
            assert_eq!(after.subtitle_ordinal, before.subtitle_ordinal);
            assert_eq!(after.side_subs_refused, before.side_subs_refused);
            assert_eq!(after.stream_vcodec, before.stream_vcodec);
            assert_eq!(after.stream_acodec, before.stream_acodec);
            assert_eq!(after.stream_fps, before.stream_fps);
            assert_eq!(after.stream_dovi, before.stream_dovi);
            assert_eq!(after.stream_dv_decision, before.stream_dv_decision);
            assert_eq!(after.stream_immersive, before.stream_immersive);
        }
        fn controller() -> (u64, u64, u64, u64, ControlPhase, Option<UserRouteIntent>, Option<i64>, Option<u64>) {
            let c = PLAYER_CONTROL.lock().unwrap();
            (c.engine_epoch, c.media_epoch, c.desired_revision, c.applied_revision,
                c.phase, c.pending_user, c.pending_seek_ns, c.flight)
        }

        #[test]
        fn native_preferences_survive_source_replacement_and_route_rollback() {
            let _serial = plx_base::testlock::serial();
            let _globals = Globals::capture();
            let mut ps = admit(&multitrack(), plx_platform::devcaps::assumed_shared());
            let older = NativeSubtitlePreferences {
                size: SubtitleSize::Small, position: SubtitlePosition::Middle, tone: SubtitleTone::Grey,
            };
            restore_native_subtitle_preferences(&mut ps, older);
            let projection = route_projection(&ps);
            let current = NativeSubtitlePreferences {
                size: SubtitleSize::Huge, position: SubtitlePosition::High, tone: SubtitleTone::Silver,
            };
            restore_native_subtitle_preferences(&mut ps, current);
            install_route_projection(&mut ps, &projection);
            assert_eq!(native_subtitle_preferences(&ps.publication()), Some(current));
            clear_url(&mut ps);
            forget_external_source(&mut ps);
            assert_eq!(native_subtitle_preferences(&ps.publication()), None,
                "the native snapshot must not override the Plex profile path");
            reset_player_control_for_test(&ps);
            assert_eq!(request_external_source(&mut ps, &multitrack(),
                crate::ff::ExternalSource::Local(std::path::PathBuf::from("/media/second.mkv")),
                "Second film", "Local", plx_platform::devcaps::assumed_shared()), Ok(()));
            assert_eq!(native_subtitle_preferences(&ps), Some(current));
            assert_eq!(native_subtitle_preferences(&ps.publication()), Some(current));
        }

        #[test]
        fn hls_audio_pick_uses_native_reload_and_preserves_position_without_enabling_subtitles() {
            let _serial = plx_base::testlock::serial();
            let _globals = Globals::capture();
            let mut info = multitrack();
            info.video_codec = "h264".into();
            info.audio_codec = "aac".into();
            info.audio_channels = 2;
            info.audio_sample_rate = 48_000;
            info.audio_bit_depth = 0;
            info.tracks.audio = vec![row(TrackKind::Audio, 1, 0, "aac"),
                row(TrackKind::Audio, 3, 1, "aac")];
            let mut refused = PlaybackSession::IDLE;
            reset_player_control_for_test(&refused);
            let source_url = plx_data::ott::http_url("https://example.invalid/live.m3u8")
                .expect("valid canonical HLS fixture URL");
            let mut no_native = plx_platform::devcaps::Caps {
                app_lossy: plx_platform::devcaps::AppLossy::Measured,
                ..plx_platform::devcaps::Caps::assumed()
            };
            // AAC has no app-decoded fallback. Refuse it in both Auto and Forced.
            no_native.audio_channels.insert("aac".into(), 0);
            no_native.native_ceiling.insert("aac".into(), 0);
            for forced in [false, true] {
                assert!(!no_native.native_audio_track("aac", 2, 48_000, forced));
            }
            assert_eq!(request_external_source(&mut refused, &info,
                crate::ff::ExternalSource::Hls(source_url.clone()),
                "Live", "M3U", std::sync::Arc::new(no_native)), Err(ExternalError::UnsupportedAudio));
            for paused in [false, true] {
                let mut ps = PlaybackSession::IDLE;
                reset_player_control_for_test(&ps);
                let source = crate::ff::ExternalSource::Hls(source_url.clone());
                request_external_source(&mut ps, &info, source.clone(), "Live", "M3U",
                    plx_platform::devcaps::assumed_shared()).unwrap();
                reset_player_control_for_test(&ps);
                crate::player::SHARED.playpos_ns.store(23_700_000_000, Relaxed);
                crate::player::TX.commit_paused(paused);
                let catalog = external_tracks(&ps).unwrap().clone();
                let pick = catalog.choice(TrackKind::Audio, Some(1)).unwrap();
                assert_eq!(apply_external_track(&mut ps, &pick), Ok(true));
                assert_eq!(external_tracks(&ps).unwrap().selected_audio(), Some(1));
                assert_eq!(external_source(&ps), Some(source.clone()));
                assert_eq!(crate::player::SHARED.desired_audio_idx.load(Relaxed), 1);
                assert_eq!(crate::player::SHARED.playpos_ns.load(Relaxed), 23_700_000_000);
                assert_eq!(crate::player::transport::paused(), paused);
                assert!(pending_user_route_intent(UserRouteIntent::NativeAudioReload));
                assert!(external_tracks(&ps).unwrap().choice(TrackKind::Subtitle, Some(0)).is_err());
                // This fixture never starts an Engine. Cancel its own queued reload,
                // then retire the old source before admitting a replacement playback.
                let reload = claim_route_action().expect("the audio reload remains queued");
                assert_eq!(reload.intent, RouteIntent::User(UserRouteIntent::NativeAudioReload));
                assert!(finish_route_action(&mut ps, &reload, RouteApplyResult::Cancelled));
                forget_external_source(&mut ps);
                assert!(!has_url(&ps));
                reset_player_control_for_test(&ps);
                request_external_source(&mut ps, &info, source, "New playback", "M3U",
                    plx_platform::devcaps::assumed_shared()).unwrap();
                reset_player_control_for_test(&ps);
                assert!(!external_tracks(&ps).unwrap().same_session(&catalog));
                assert_eq!(apply_external_track(&mut ps, &pick),
                    Err(ExternalTrackApplyError::InvalidChoice(ExternalTrackError::StaleSession)));
            }
        }

        #[test]
        fn external_audio_switch_uses_candidate_pcm_shape_not_only_its_codec() {
            let _serial = plx_base::testlock::serial();
            let _globals = Globals::capture();
            for paused in [false, true] {
                let mut ps = admit(&multitrack(), plx_platform::devcaps::assumed_shared());
                assert_eq!(stream_pcm(&ps), Some(Ok(PcmFormat {
                    sample_rate: 44_100, bits: 16, channels: 1, source_rate: 44_100,
                })));
                crate::player::TX.commit_paused(paused);
                crate::player::swap_state_for_test(if paused { crate::player::PlaybackState::Paused }
                    else { crate::player::PlaybackState::Playing });
                let source = external_source(&ps);
                let choice = external_tracks(&ps).unwrap().choice(TrackKind::Audio, Some(1)).unwrap();
                assert_eq!(apply_external_track(&mut ps, &choice), Ok(true));
                assert_eq!(stream_pcm(&ps), Some(Ok(PcmFormat {
                    sample_rate: 48_000, bits: 32, channels: 2, source_rate: 96_000,
                })));
                assert_eq!(ps.cur_audio.as_ref().unwrap().ordinal, 1);
                assert_eq!(crate::player::SHARED.desired_audio_idx.load(Relaxed), 1);
                assert_eq!(external_tracks(&ps).unwrap().selected_audio(), Some(1));
                assert_eq!(external_source(&ps), source);
                assert_eq!(crate::player::SHARED.playpos_ns.load(Relaxed), 23_700_000_000);
                assert!(pending_user_route_intent(UserRouteIntent::NativeAudioReload));
                assert_eq!(crate::player::transport::paused(), paused, "audio selection preserves viewer pause intent");
            }
        }

        #[test]
        fn external_busy_audio_choice_cannot_mutate_a_route_or_steal_an_existing_action() {
            let _serial = plx_base::testlock::serial();
            let _globals = Globals::capture();
            for seek in [false, true] {
                let mut ps = admit(&multitrack(), plx_platform::devcaps::assumed_shared());
                let catalog = external_tracks(&ps).unwrap().clone();
                let choice = catalog.choice(TrackKind::Audio, Some(1)).unwrap();
                if seek { note_user_seek_intent(42_000_000_000); }
                else { request_user_route_intent(&ps, UserRouteIntent::AdaptiveReload); }
                let before = route_projection(&ps);
                let control = controller();
                let desired_audio = crate::player::SHARED.desired_audio_idx.load(Relaxed);
                assert_eq!(apply_external_track(&mut ps, &choice), Err(ExternalTrackApplyError::Busy));
                assert_route_unchanged(&before, &ps);
                assert_eq!(controller(), control);
                let after = external_tracks(&ps).unwrap();
                assert!(after.same_session(&catalog));
                assert_eq!(after.selected_audio(), catalog.selected_audio());
                assert_eq!(after.selected_subtitle(), catalog.selected_subtitle());
                assert_eq!(crate::player::SHARED.desired_audio_idx.load(Relaxed), desired_audio);
                assert_eq!(crate::player::SHARED.playpos_ns.load(Relaxed), 23_700_000_000);
            }
        }

        #[test]
        fn external_active_claim_blocks_but_settled_claim_history_does_not() {
            let _serial = plx_base::testlock::serial();
            let _globals = Globals::capture();
            let mut ps = admit(&multitrack(), plx_platform::devcaps::assumed_shared());
            let choice = external_tracks(&ps).unwrap().choice(TrackKind::Audio, Some(1)).unwrap();
            request_user_route_intent(&ps, UserRouteIntent::AdaptiveReload);
            let action = claim_route_action().expect("real queued intent is claimed");
            assert!(matches!(PLAYER_CONTROL.lock().unwrap().phase, ControlPhase::Applying(_)));
            let before = route_projection(&ps);
            let control = controller();
            assert_eq!(apply_external_track(&mut ps, &choice), Err(ExternalTrackApplyError::Busy));
            assert_route_unchanged(&before, &ps);
            assert_eq!(controller(), control);
            assert_eq!(crate::player::SHARED.desired_audio_idx.load(Relaxed), 0);
            // The existing reducer deliberately retains claimed_user as historical data.
            assert!(finish_route_action(&mut ps, &action, RouteApplyResult::Cancelled));
            {
                let control = PLAYER_CONTROL.lock().unwrap();
                assert_eq!(control.phase, ControlPhase::Stable);
                assert!(control.claimed_user.is_some(), "premise: settlement retains claim history");
            }
            assert_eq!(apply_external_track(&mut ps, &choice), Ok(true));
            assert_eq!(crate::player::SHARED.desired_audio_idx.load(Relaxed), 1);
            assert!(pending_user_route_intent(UserRouteIntent::NativeAudioReload));
        }

        #[test]
        fn external_reopen_of_identical_url_rejects_old_opaque_choice() {
            let _serial = plx_base::testlock::serial();
            let _globals = Globals::capture();
            let info = multitrack();
            let old = admit(&info, plx_platform::devcaps::assumed_shared());
            let old_catalog = external_tracks(&old).unwrap().clone();
            let stale = old_catalog.choice(TrackKind::Audio, Some(1)).unwrap();
            let mut ps = admit(&info, plx_platform::devcaps::assumed_shared());
            assert_eq!(external_source(&ps), external_source(&old));
            assert!(!external_tracks(&ps).unwrap().same_session(&old_catalog));
            let before = route_projection(&ps);
            let control = controller();
            assert_eq!(apply_external_track(&mut ps, &stale), Err(ExternalTrackApplyError::InvalidChoice(
                ExternalTrackError::StaleSession)));
            assert_route_unchanged(&before, &ps);
            assert_eq!(controller(), control);
            assert_eq!(crate::player::SHARED.desired_audio_idx.load(Relaxed), 0);
            assert_eq!(external_tracks(&ps).unwrap().selected_audio(), Some(0));
        }

        #[test]
        fn external_subtitle_on_off_keeps_audio_source_position_and_has_no_reload() {
            let _serial = plx_base::testlock::serial();
            let _globals = Globals::capture();
            let mut ps = admit(&multitrack(), plx_platform::devcaps::assumed_shared());
            let source = external_source(&ps);
            let audio = ps.cur_audio.clone();
            let control = controller();
            for selected in [Some(0), None] {
                let choice = external_tracks(&ps).unwrap().choice(TrackKind::Subtitle, selected).unwrap();
                assert_eq!(apply_external_track(&mut ps, &choice), Ok(true));
                assert_eq!(external_tracks(&ps).unwrap().selected_subtitle(), selected);
                assert_eq!(crate::player::desired_sub_idx(), selected.unwrap_or(-1));
                assert_eq!(ps.cur_audio, audio);
                assert_eq!(external_source(&ps), source);
                assert_eq!(crate::player::SHARED.desired_audio_idx.load(Relaxed), 0);
                assert_eq!(crate::player::SHARED.playpos_ns.load(Relaxed), 23_700_000_000);
                let after = controller();
                assert_eq!((after.0, after.1, after.4, after.5, after.6, after.7),
                    (control.0, control.1, control.4, control.5, control.6, control.7),
                    "local subtitle selection must not reload or change the physical timeline");
                assert_eq!(apply_external_track(&mut ps, &choice), Ok(false));
            }
        }


        #[test]
        fn external_unchanged_choice_still_advances_shared_user_contract() {
            let _serial = plx_base::testlock::serial();
            let _globals = Globals::capture();
            let mut ps = admit(&multitrack(), plx_platform::devcaps::assumed_shared());
            for (kind, ordinal) in [(TrackKind::Audio, Some(0)), (TrackKind::Subtitle, None)] {
                let choice = external_tracks(&ps).unwrap().choice(kind, ordinal).unwrap();
                let before = route_projection(&ps);
                let control = controller();
                assert_eq!(apply_external_track(&mut ps, &choice), Ok(false));
                let after = controller();
                assert_ne!(after.2, control.2, "an accepted no-op is still a new user intent");
                assert_eq!((after.0, after.1, after.3, after.4, after.5, after.6, after.7),
                    (control.0, control.1, control.3, control.4, control.5, control.6, control.7),
                    "unchanged choice must not reload or alter physical playback");
                assert_route_unchanged(&before, &ps);
                assert_eq!(crate::player::SHARED.playpos_ns.load(Relaxed), 23_700_000_000);
            }
        }

        #[test]
        fn external_unchanged_choice_cannot_bypass_busy_admission() {
            let _serial = plx_base::testlock::serial();
            let _globals = Globals::capture();
            let mut ps = admit(&multitrack(), plx_platform::devcaps::assumed_shared());
            let choice = external_tracks(&ps).unwrap().choice(TrackKind::Audio, Some(0)).unwrap();
            note_user_seek_intent(42_000_000_000);
            let control = controller();
            let before = route_projection(&ps);
            assert_eq!(apply_external_track(&mut ps, &choice), Err(ExternalTrackApplyError::Busy));
            assert_eq!(controller(), control, "rejected no-op must not supersede the existing intent");
            assert_route_unchanged(&before, &ps);
        }

        #[test]
        fn external_unchanged_choice_keeps_recovery_and_next_audio_edit_live() {
            let _serial = plx_base::testlock::serial();
            let _globals = Globals::capture();
            for (kind, ordinal) in [(TrackKind::Audio, Some(0)), (TrackKind::Subtitle, None)] {
                let mut ps = admit(&multitrack(), plx_platform::devcaps::assumed_shared());
                let before = route_projection(&ps);
                let worker = worker_ticket();
                let choice = external_tracks(&ps).unwrap().choice(kind, ordinal).unwrap();
                assert_eq!(apply_external_track(&mut ps, &choice), Ok(false));
                assert_eq!(worker_ticket(), worker,
                    "unchanged playback must keep its physical worker authorized");
                assert_route_unchanged(&before, &ps);
                assert_eq!(crate::player::SHARED.playpos_ns.load(Relaxed), 23_700_000_000);

                // Exercise the real reducer ingress and claim; do not execute the
                // resulting network/decoder work in this control-boundary fixture.
                assert_eq!(publish_automatic_route_intent(AutomaticRouteIntent::OriginalToHls {
                    ticket: worker.clone(),
                    conservative_kbps: 4_000,
                    position_ns: 23_700_000_000,
                }), AutomaticIntentResult::Accepted,
                    "the running worker can still request automatic recovery after the no-op");
                let recovery = claim_route_action().expect("automatic recovery remains claimable");
                assert_eq!(recovery.ticket, worker);
                assert!(matches!(&recovery.intent,
                    RouteIntent::Automatic(AutomaticRouteIntent::OriginalToHls { .. })));
                assert!(finish_route_action(&mut ps, &recovery, RouteApplyResult::Cancelled));
                assert_eq!(worker_ticket(), worker);
                assert_route_unchanged(&before, &ps);

                let different_audio = external_tracks(&ps).unwrap()
                    .choice(TrackKind::Audio, Some(1)).unwrap();
                assert_eq!(apply_external_track(&mut ps, &different_audio), Ok(true));
                assert_eq!(external_tracks(&ps).unwrap().selected_audio(), Some(1));
                assert!(pending_user_route_intent(UserRouteIntent::NativeAudioReload));
                let manual = claim_route_action().expect("the next audio edit remains claimable");
                assert_eq!(manual.intent, RouteIntent::User(UserRouteIntent::NativeAudioReload));
                assert!(finish_route_action(&mut ps, &manual, RouteApplyResult::Cancelled));
            }
        }

        #[test]
        fn external_terminal_or_refused_session_is_not_a_live_track_target() {
            let _serial = plx_base::testlock::serial();
            let _globals = Globals::capture();
            for failure in 0..4 {
                let mut ps = admit(&multitrack(), plx_platform::devcaps::assumed_shared());
                match failure {
                    0 => ps.jail_load_blocked = true,
                    1 => ps.native_start_refused = Some(StartRefusal::LoadTimeout),
                    2 => ps.native_start_refused = Some(StartRefusal::ReleaseTimeout),
                    _ => ps.engine_failure = Some(crate::player::RuntimeFailure::MediaSource),
                }
                let flags = (ps.jail_load_blocked, ps.native_start_refused, ps.engine_failure);
                let catalog = external_tracks(&ps).unwrap().clone();
                let choice = catalog.choice(TrackKind::Audio, Some(1)).unwrap();
                let before = route_projection(&ps);
                let control = controller();
                assert_eq!(apply_external_track(&mut ps, &choice), Err(ExternalTrackApplyError::Inactive));
                assert_route_unchanged(&before, &ps);
                assert_eq!(controller(), control);
                assert_eq!((ps.jail_load_blocked, ps.native_start_refused, ps.engine_failure), flags);
                let after = external_tracks(&ps).unwrap();
                assert!(after.same_session(&catalog));
                assert_eq!(after.selected_audio(), catalog.selected_audio());
                assert_eq!(after.selected_subtitle(), catalog.selected_subtitle());
                assert_eq!(crate::player::SHARED.desired_audio_idx.load(Relaxed), 0);
                assert_eq!(crate::player::SHARED.playpos_ns.load(Relaxed), 23_700_000_000);
            }
        }

        #[test]
        fn external_choice_is_reclassified_after_current_attempt_mode_narrows() {
            let _serial = plx_base::testlock::serial();
            let _globals = Globals::capture();
            let mut info = multitrack();
            info.tracks.audio[1].codec = "dca".into();
            info.tracks.audio[1].channels = 6;
            info.tracks.audio[1].sample_rate = 48_000;
            let caps = plx_platform::devcaps::Caps {
                app_lossy: plx_platform::devcaps::AppLossy::Measured,
                ..plx_platform::devcaps::Caps::assumed()
            }.with_app_lossy_for("dts");
            let mut ps = admit(&info, std::sync::Arc::new(caps));
            let catalog = external_tracks(&ps).unwrap().clone();
            let choice = catalog.choice(TrackKind::Audio, Some(1)).unwrap();
            set_direct_play_mode_for_test(&mut ps, DirectPlayMode::Forced);
            let before = route_projection(&ps);
            let control = controller();
            assert_eq!(apply_external_track(&mut ps, &choice), Err(ExternalTrackApplyError::InvalidChoice(
                ExternalTrackError::Unsupported(TrackUnavailable::Codec))));
            assert_route_unchanged(&before, &ps);
            assert_eq!(controller(), control);
            assert!(external_tracks(&ps).unwrap().same_session(&catalog));
            assert_eq!(external_tracks(&ps).unwrap().selected_audio(), Some(0));
            assert_eq!(crate::player::SHARED.desired_audio_idx.load(Relaxed), 0);
        }
    }
}

#[cfg(test)]
#[path = "external_local_tests.rs"]
mod local_sidecar_tests;

#[cfg(test)]
#[path = "external_continuation_tests.rs"]
mod managed_continuation_tests;

#[path = "external_hls_tracks.rs"]
mod hls_tracks;
pub use hls_tracks::{reconcile_observed_hls_tracks, install_observed_hls_tracks, observed_hls_tracks, observed_hls_subtitle_selection, HlsSubtitleMode, HlsSubtitlePolicy, HlsSubtitleIntent, hls_subtitle_policy, hls_subtitle_intent, restore_hls_subtitle_policy, begin_automatic_hls_subtitle_request, hls_track_catalog, begin_hls_track_request, hls_track_request_is_current, complete_hls_track_request, HlsTrackRow, HlsSubtitleRow, HlsTrackTarget, HlsTrackCatalog, HlsTrackChoice, HlsTrackRequest, PreparedHlsTrack};
