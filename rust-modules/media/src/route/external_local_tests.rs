//! Real route/catalog/sidecar admission; no file, network or native decoder is started.
use super::*;
use crate::ff::inspect::{InspectedTrack, TrackKind};
use crate::player::{sidecar, text_style::TextStyle};
use crate::route::{LocalSubtitleTarget, SidecarFormat};
use plx_base::task::OffFrame;
use std::sync::atomic::Ordering::Relaxed;

struct Reset {
    audio: i32,
    subtitle: i32,
    offset: i64,
}
impl Reset {
    fn new() -> Self {
        sidecar::reset();
        reset_player_control_for_test(&PlaybackSession::IDLE);
        Self {
            audio: crate::player::SHARED.desired_audio_idx.load(Relaxed),
            subtitle: crate::player::SHARED.desired_sub_idx.load(Relaxed),
            offset: crate::player::subtitle_offset_ms(),
        }
    }
}
impl Drop for Reset {
    fn drop(&mut self) {
        sidecar::reset();
        reset_player_control_for_test(&PlaybackSession::IDLE);
        crate::player::SHARED
            .desired_audio_idx
            .store(self.audio, Relaxed);
        crate::player::SHARED
            .desired_sub_idx
            .store(self.subtitle, Relaxed);
        crate::player::restore_subtitle_offset(self.offset);
    }
}
fn media() -> MediaInfo {
    let mut info = MediaInfo {
        tracks: Default::default(),
        resolved_source: None,
        hls_playback: None,
        video_codec: "h264".into(),
        audio_codec: "aac".into(),
        width: 1280,
        height: 720,
        fps: 24.0,
        duration_ns: 60_000_000_000,
        audio_channels: 2,
        audio_sample_rate: 48000,
        audio_bit_depth: 16,
        dovi: Default::default(),
    };
    info.tracks.subtitles.push(InspectedTrack {
        kind: TrackKind::Subtitle,
        stream_index: 2,
        ordinal: 0,
        codec: "hdmv_pgs_subtitle".into(),
        language: "en".into(),
        title: "Embedded".into(),
        channels: 0,
        sample_rate: 0,
        bit_depth: 0,
        decoder_available: true,
        disposition_known: false,
        default: false,
        forced: false,
    });
    info
}
fn admitted() -> PlaybackSession {
    let mut ps = PlaybackSession::IDLE;
    request_external_source(
        &mut ps,
        &media(),
        crate::ff::ExternalSource::Local("/synthetic/movie.mkv".into()),
        "Synthetic",
        "Local",
        plx_platform::devcaps::assumed_shared(),
    )
    .unwrap();
    reset_player_control_for_test(&ps);
    ps
}
fn row(format: SidecarFormat) -> LocalSubtitleTrack {
    LocalSubtitleTrack {
        target: LocalSubtitleTarget::new(),
        format,
        title: "Adjacent subtitle".into(),
        language: "en".into(),
    }
}
fn publish(ps: &mut PlaybackSession, rows: Vec<LocalSubtitleTrack>) -> bool {
    let identity = playback_identity(ps).unwrap();
    let catalog = external_tracks(ps).unwrap().clone();
    install_external_local_subtitles(ps, &identity, &catalog, rows, true, 128, 64).unwrap()
}
fn pick(ps: &mut PlaybackSession, row: &LocalSubtitleTrack) -> sidecar::LocalReadRequest {
    let choice = external_tracks(ps)
        .unwrap()
        .local_choice(&row.target)
        .unwrap();
    assert_eq!(apply_external_track(ps, &choice), Ok(true));
    let peek = sidecar::peek_local_request().expect("selection queues an app-owned read");
    assert_eq!(peek.target(), &row.target);
    assert_eq!(peek.playback(), &playback_identity(ps).unwrap());
    let request = sidecar::take_local_request().unwrap();
    assert!(sidecar::peek_local_request().is_none());
    assert!(sidecar::local_request_is_current(&request));
    request
}
fn text(request: &sidecar::LocalReadRequest) -> sidecar::PreparedLocal {
    request
        .prepare_owned(
            &OffFrame::for_test(),
            b"1\n00:00:01,000 --> 00:00:02,000\nHello\n".to_vec(),
        )
        .unwrap()
}
fn style() -> TextStyle {
    TextStyle::current(Default::default())
}

#[test]
fn local_pick_disables_embedded_without_reload_and_off_rejects_late_results() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset::new();
    let mut ps = admitted();
    let identity = playback_identity(&ps);
    let source = external_source(&ps);
    let local = row(SidecarFormat::Srt);
    publish(&mut ps, vec![local.clone()]);
    let request = pick(&mut ps, &local);
    let ready = text(&request);
    let failure = request.read_failed(&OffFrame::for_test());
    assert_eq!(ps.cur_sub_ordinal, -1);
    assert!(ps.cur_sub_sidecar && sidecar::selected());
    assert_eq!(subtitle_presenter(&ps), SubtitlePresenter::Client);
    let off = external_tracks(&ps)
        .unwrap()
        .choice(TrackKind::Subtitle, None)
        .unwrap();
    assert_eq!(apply_external_track(&mut ps, &off), Ok(true));
    assert!(!sidecar::selected());
    assert_eq!(subtitle_presenter(&ps), SubtitlePresenter::None);
    assert!(!sidecar::install_local(&request, ready));
    assert!(!sidecar::fail_local(&request, failure));
    assert_eq!(playback_identity(&ps), identity);
    assert_eq!(external_source(&ps), source);
    assert!(PLAYER_CONTROL.lock().unwrap().pending_user.is_none());
}

#[test]
fn embedded_pick_silences_loaded_local_and_catalog_removal_retires_pending() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset::new();
    let mut ps = admitted();
    let local = row(SidecarFormat::Srt);
    publish(&mut ps, vec![local.clone()]);
    let request = pick(&mut ps, &local);
    assert!(sidecar::install_local(&request, text(&request)));
    assert!(sidecar::ass_source(false, style()).is_some());
    let embedded = external_tracks(&ps)
        .unwrap()
        .choice(TrackKind::Subtitle, Some(0))
        .unwrap();
    assert_eq!(apply_external_track(&mut ps, &embedded), Ok(true));
    assert_eq!(ps.cur_sub_ordinal, 0);
    assert!(!sidecar::selected());
    assert!(sidecar::ass_source(false, style()).is_none());
    // Source suspension drops retained bytes, without changing the selected catalog.
    sidecar::reset_local();
    let request = pick(&mut ps, &local);
    let ready = text(&request);
    assert!(publish(&mut ps, Vec::new()));
    assert!(!sidecar::selected());
    assert!(!sidecar::install_local(&request, ready));
}

#[test]
fn repick_and_same_source_new_playback_cannot_accept_an_old_read() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset::new();
    let mut ps = admitted();
    let local = row(SidecarFormat::Srt);
    publish(&mut ps, vec![local.clone()]);
    let old = pick(&mut ps, &local);
    let ready = text(&old);
    let current = pick(&mut ps, &local);
    assert!(!sidecar::install_local(&old, ready));
    assert!(!sidecar::fail_local(
        &old,
        old.read_failed(&OffFrame::for_test())
    ));
    assert!(sidecar::install_local(&current, text(&current)));
    assert!(
        !sidecar::local_request_is_current(&current),
        "terminal consumes the ticket"
    );
    assert!(
        !sidecar::install_local(&current, text(&current)),
        "duplicate completion is refused"
    );
    let old_identity = playback_identity(&ps).unwrap();
    let old_catalog = external_tracks(&ps).unwrap().clone();
    forget_external_source(&mut ps);
    assert!(sidecar::ass_source(false, style()).is_none());
    ps = admitted();
    assert_ne!(playback_identity(&ps).unwrap(), old_identity);
    assert_eq!(
        install_external_local_subtitles(
            &mut ps,
            &old_identity,
            &old_catalog,
            vec![local],
            true,
            128,
            64
        ),
        Err(ExternalTrackApplyError::InvalidChoice(
            ExternalTrackError::StaleSession
        ))
    );
}

#[test]
fn local_suspend_rearm_retains_selection_but_never_retains_old_read_authority() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset::new();
    let mut ps = admitted();
    let local = row(SidecarFormat::Srt);
    publish(&mut ps, vec![local.clone()]);
    let request = pick(&mut ps, &local);
    let ready = text(&request);
    sidecar::reset_local();
    assert_eq!(
        external_tracks(&ps).unwrap().selected_local_subtitle(),
        Some(&local.target)
    );
    assert!(!sidecar::install_local(&request, ready));
    assert_eq!(rearm_selected_local_subtitle(&ps), Ok(true));
    let current = sidecar::take_local_request().unwrap();
    assert!(sidecar::install_local(&current, text(&current)));
    let held = sidecar::ass_source(false, style()).unwrap();
    // Rendering across different clocks/pause does not reload the whole file.
    assert!(std::sync::Arc::ptr_eq(
        &held,
        &sidecar::ass_source(false, style()).unwrap()
    ));
    clear_url(&mut ps);
    sidecar::reset_local();
    assert_eq!(
        rearm_selected_local_subtitle(&ps),
        Err(ExternalTrackApplyError::Inactive)
    );
    assert!(sidecar::peek_local_request().is_none());
}

#[test]
fn owned_formats_use_the_shared_decoder_and_reject_empty_or_oversized_bodies() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset::new();
    let mut ps = admitted();
    let authored = b"[Script Info]\nScriptType: v4.00+\n[Events]\nDialogue: 0,0:00:01.00,0:00:03.00,Default,,0,0,0,,Hello\n";
    for (format, bytes, is_authored) in [
        (
            SidecarFormat::Srt,
            b"1\n00:00:01,000 --> 00:00:02,000\nHello\n".as_slice(),
            false,
        ),
        (
            SidecarFormat::Vtt,
            b"WEBVTT\n\n00:01.000 --> 00:02.000\nHello\n".as_slice(),
            false,
        ),
        (SidecarFormat::Ass, authored.as_slice(), true),
        (SidecarFormat::Ssa, authored.as_slice(), true),
    ] {
        let local = row(format);
        publish(&mut ps, vec![local.clone()]);
        let request = pick(&mut ps, &local);
        let ready = request
            .prepare_owned(&OffFrame::for_test(), bytes.to_vec())
            .unwrap();
        assert!(sidecar::install_local(&request, ready));
        let source = sidecar::ass_source(false, style()).unwrap();
        assert_eq!(
            matches!(source.origin, crate::player::ass::Origin::Authored),
            is_authored
        );
        assert!(request
            .prepare_owned(&OffFrame::for_test(), Vec::new())
            .is_err());
        assert!(request
            .prepare_owned(
                &OffFrame::for_test(),
                vec![b'x'; sidecar::owned_byte_limit() + 1]
            )
            .is_err());
    }
}

#[test]
fn local_read_failure_is_once_only_and_retry_uses_a_new_ticket() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset::new();
    let mut ps = admitted();
    let local = row(SidecarFormat::Srt);
    publish(&mut ps, vec![local.clone()]);
    let request = pick(&mut ps, &local);
    assert!(sidecar::fail_local(
        &request,
        request.read_failed(&OffFrame::for_test())
    ));
    assert!(!sidecar::fail_local(
        &request,
        request.read_failed(&OffFrame::for_test())
    ));
    assert!(sidecar::notice().is_some());
    let current = pick(&mut ps, &local);
    assert!(sidecar::notice().is_none());
    assert!(sidecar::install_local(&current, text(&current)));
}

#[test]
fn stale_discovery_or_inactive_route_cannot_publish_or_queue_a_local_read() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset::new();
    let mut ps = admitted();
    let identity = playback_identity(&ps).unwrap();
    let old_catalog = external_tracks(&ps).unwrap().clone();
    let local = row(SidecarFormat::Srt);
    publish(&mut ps, vec![local.clone()]);
    assert_eq!(
        install_external_local_subtitles(
            &mut ps,
            &identity,
            &old_catalog,
            Vec::new(),
            true,
            128,
            64
        ),
        Err(ExternalTrackApplyError::InvalidChoice(
            ExternalTrackError::StaleSession
        ))
    );
    assert_eq!(external_tracks(&ps).unwrap().local_subtitles().len(), 1);
    let choice = external_tracks(&ps)
        .unwrap()
        .local_choice(&local.target)
        .unwrap();
    clear_url(&mut ps);
    assert_eq!(
        apply_external_track(&mut ps, &choice),
        Err(ExternalTrackApplyError::Inactive)
    );
    assert!(sidecar::peek_local_request().is_none());
    ps = admitted();
    ps.external_source = Some(crate::ff::ExternalSource::Http(
        "https://example.invalid/movie.mkv".into(),
    ));
    let catalog = external_tracks(&ps).unwrap().clone();
    let identity = playback_identity(&ps).unwrap();
    assert_eq!(
        install_external_local_subtitles(&mut ps, &identity, &catalog, vec![local], true, 128, 64),
        Err(ExternalTrackApplyError::Inactive)
    );
    assert!(external_tracks(&ps).unwrap().local_subtitles().is_empty());
}

#[test]
fn refused_metadata_worker_is_terminal_until_explicit_local_reselect() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset::new();
    let mut ps = admitted();
    let local = row(SidecarFormat::Srt);
    publish(&mut ps, vec![local.clone()]);
    let request = pick(&mut ps, &local);
    assert!(sidecar::fail_local_start(&request));
    assert!(sidecar::selected());
    assert!(sidecar::local_failed());
    assert!(sidecar::peek_local_request().is_none());
    assert!(!sidecar::local_request_is_current(&request));
    assert!(!sidecar::fail_local_start(&request));
    assert!(!sidecar::install_local(&request, text(&request)));
    let current = pick(&mut ps, &local);
    assert!(!sidecar::local_failed());
    assert!(sidecar::install_local(&current, text(&current)));
}

#[test]
fn discovery_and_rearm_wait_for_real_route_completion_without_advancing_user_intent() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset::new();
    let mut ps = admitted();
    let local = row(SidecarFormat::Srt);
    let revision = PLAYER_CONTROL.lock().unwrap().desired_revision;
    publish(&mut ps, vec![local.clone()]);
    assert_eq!(
        PLAYER_CONTROL.lock().unwrap().desired_revision,
        revision,
        "passive discovery is not a user intention"
    );
    let request = pick(&mut ps, &local);
    assert!(sidecar::install_local(&request, text(&request)));
    let held = sidecar::ass_source(false, style()).unwrap();
    let identity = playback_identity(&ps).unwrap();
    let catalog = external_tracks(&ps).unwrap().clone();
    request_user_route_intent(&ps, UserRouteIntent::AdaptiveReload);
    let revision = PLAYER_CONTROL.lock().unwrap().desired_revision;
    // A queued intent is already a reservation; do not wait until Applying.
    assert_eq!(
        install_external_local_subtitles(&mut ps, &identity, &catalog, Vec::new(), true, 128, 64),
        Err(ExternalTrackApplyError::Busy)
    );
    let action = claim_route_action().expect("actual route intent claimed");
    assert!(matches!(
        PLAYER_CONTROL.lock().unwrap().phase,
        ControlPhase::Applying(_)
    ));
    assert_eq!(
        install_external_local_subtitles(&mut ps, &identity, &catalog, Vec::new(), true, 128, 64),
        Err(ExternalTrackApplyError::Busy)
    );
    assert_eq!(
        rearm_selected_local_subtitle(&ps),
        Err(ExternalTrackApplyError::Busy)
    );
    assert_eq!(PLAYER_CONTROL.lock().unwrap().desired_revision, revision);
    assert!(external_tracks(&ps).unwrap().same_session(&catalog));
    assert_eq!(
        external_tracks(&ps).unwrap().selected_local_subtitle(),
        Some(&local.target)
    );
    assert!(std::sync::Arc::ptr_eq(
        &held,
        &sidecar::ass_source(false, style()).unwrap()
    ));
    assert!(finish_route_action(
        &mut ps,
        &action,
        RouteApplyResult::Cancelled
    ));
    let revision = PLAYER_CONTROL.lock().unwrap().desired_revision;
    // The existing producer can now retry the held discovery result, with no IO.
    assert!(install_external_local_subtitles(
        &mut ps,
        &identity,
        &catalog,
        Vec::new(),
        true,
        128,
        64
    )
    .unwrap());
    assert_eq!(PLAYER_CONTROL.lock().unwrap().desired_revision, revision);
    assert!(external_tracks(&ps).unwrap().local_subtitles().is_empty());
    assert!(!sidecar::selected());
    request_user_route_intent(&ps, UserRouteIntent::AdaptiveReload);
    let action = claim_route_action().unwrap();
    assert!(finish_route_action(
        &mut ps,
        &action,
        RouteApplyResult::Cancelled
    ));
    assert!(
        external_tracks(&ps).unwrap().local_subtitles().is_empty(),
        "a later rollback uses the revised authoritative projection"
    );
    assert_eq!(
        external_tracks(&ps).unwrap().selected_local_subtitle(),
        None
    );
    assert!(!sidecar::selected());
}
