//! Actual route/menu APIs; no socket, resolver, demuxer or native engine runs.
use super::*;
use crate::ff::inspect::{InspectedTrack, TrackKind};
use std::sync::atomic::Ordering::Relaxed;

struct Globals {
    audio: i32,
    subtitle: i32,
    offset: i64,
    rate: i32,
    rate_reload: bool,
    position: i64,
    state: u8,
    paused: bool,
    audio_switch: u8,
    audio_pick_ms: i64,
    restore_bitmaps: Option<Box<dyn FnOnce()>>,
}
impl Globals {
    fn capture() -> Self {
        let shared = &crate::player::SHARED;
        let bitmaps = std::mem::take(&mut *shared.sub_bitmaps.lock().unwrap());
        Self {
            audio: shared.desired_audio_idx.load(Relaxed),
            subtitle: shared.desired_sub_idx.load(Relaxed),
            offset: crate::player::subtitle_offset_ms(),
            rate: crate::player::play_rate_permille(),
            rate_reload: shared.rate_reload.load(Relaxed),
            position: shared.playpos_ns.load(Relaxed),
            state: crate::player::swap_state_for_test(crate::player::PlaybackState::Playing),
            paused: crate::player::transport::paused(),
            audio_switch: shared.audio_switch.load(Relaxed),
            audio_pick_ms: shared.audio_pick_ms.load(Relaxed),
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
        shared.play_rate_permille.store(self.rate, Relaxed);
        shared.rate_reload.store(self.rate_reload, Relaxed);
        shared.playpos_ns.store(self.position, Relaxed);
        shared.audio_switch.store(self.audio_switch, Relaxed);
        shared.audio_pick_ms.store(self.audio_pick_ms, Relaxed);
        if let Some(restore) = self.restore_bitmaps.take() {
            restore();
        }
        crate::player::restore_subtitle_offset(self.offset);
        crate::player::TX.commit_paused(self.paused);
        crate::player::restore_state_for_test(self.state);
    }
}
fn row(kind: TrackKind, stream_index: i32, ordinal: i32, codec: &str) -> InspectedTrack {
    InspectedTrack {
        kind,
        stream_index,
        ordinal,
        codec: codec.into(),
        language: if ordinal == 0 { "eng" } else { "spa" }.into(),
        title: format!("Continuation track {ordinal}"),
        channels: if kind == TrackKind::Audio { 2 } else { 0 },
        sample_rate: if kind == TrackKind::Audio { 48_000 } else { 0 },
        bit_depth: 16,
        decoder_available: true,
        disposition_known: false,
        default: false,
        forced: false,
    }
}
fn media() -> MediaInfo {
    let mut info = MediaInfo {
        tracks: Default::default(),
        resolved_source: None,
        hls_playback: None,
        video_codec: "h264".into(),
        audio_codec: "flac".into(),
        width: 1280,
        height: 720,
        fps: 24.0,
        duration_ns: 120_000_000_000,
        audio_channels: 2,
        audio_sample_rate: 48_000,
        audio_bit_depth: 16,
        dovi: Default::default(),
    };
    info.tracks.audio = vec![
        row(TrackKind::Audio, 1, 0, "flac"),
        row(TrackKind::Audio, 3, 1, "flac"),
    ];
    info.tracks.subtitles = vec![row(TrackKind::Subtitle, 2, 0, "hdmv_pgs_subtitle")];
    info
}
fn source(lease: &str) -> crate::ff::ExternalSource {
    crate::ff::ExternalSource::Http(format!("https://example.invalid/managed.mkv?lease={lease}"))
}
fn admit() -> PlaybackSession {
    let mut ps = PlaybackSession::IDLE;
    reset_player_control_for_test(&ps);
    request_external_source(
        &mut ps,
        &media(),
        source("old"),
        "Film",
        "Managed",
        plx_platform::devcaps::assumed_shared(),
    )
    .unwrap();
    // Settle the route controller without starting a decoder, as in the
    // existing external-track admission and logical-identity fixtures.
    reset_player_control_for_test(&ps);
    ps
}
fn choose(ps: &mut PlaybackSession, kind: TrackKind, ordinal: Option<i32>) {
    let choice = external_tracks(ps).unwrap().choice(kind, ordinal).unwrap();
    assert_eq!(apply_external_track(ps, &choice), Ok(true));
    reset_player_control_for_test(ps);
}
fn stopped(ps: &mut PlaybackSession) {
    clear_url(ps);
    cancel_play(ps);
    reset_player_control_for_test(ps);
}
fn resume(ps: &mut PlaybackSession, identity: &PlaybackIdentity) {
    continue_external_source(
        ps,
        &media(),
        source("renewed"),
        "Film",
        "Managed",
        plx_platform::devcaps::assumed_shared(),
        identity,
    )
    .unwrap();
    assert!(playback_identity_matches(ps, identity));
}

#[test]
fn managed_external_continuation_preserves_selected_audio() {
    let _serial = plx_base::testlock::serial();
    let _globals = Globals::capture();
    let mut ps = admit();
    choose(&mut ps, TrackKind::Audio, Some(1));
    assert_eq!(external_tracks(&ps).unwrap().selected_audio(), Some(1));
    let identity = playback_identity(&ps).unwrap();
    stopped(&mut ps);
    resume(&mut ps, &identity);
    assert_eq!(external_tracks(&ps).unwrap().selected_audio(), Some(1));
    assert_eq!(ps.cur_audio.as_ref().unwrap().ordinal, 1);
    assert_eq!(crate::player::SHARED.desired_audio_idx.load(Relaxed), 1);
}

#[test]
fn managed_external_continuation_preserves_subtitle_and_offset() {
    let _serial = plx_base::testlock::serial();
    let _globals = Globals::capture();
    let mut ps = admit();
    choose(&mut ps, TrackKind::Subtitle, Some(0));
    crate::player::set_subtitle_offset(1_750);
    assert_eq!(crate::player::subtitle_offset_ms(), 1_750);
    let identity = playback_identity(&ps).unwrap();
    stopped(&mut ps);
    resume(&mut ps, &identity);
    assert_eq!(external_tracks(&ps).unwrap().selected_subtitle(), Some(0));
    assert_eq!(ps.cur_sub_ordinal, 0);
    assert_eq!(crate::player::SHARED.desired_sub_idx.load(Relaxed), 0);
    assert_eq!(crate::player::subtitle_offset_ms(), 1_750);
}

#[test]
fn managed_external_continuation_preserves_supported_playback_rate() {
    let _serial = plx_base::testlock::serial();
    let _globals = Globals::capture();
    // Route tests do not load FFmpeg, so use the same decoder-capability seam
    // as the player's speed tests. This does not run or simulate a decoder.
    struct SpeedDecoder;
    impl Drop for SpeedDecoder {
        fn drop(&mut self) {
            crate::ff::force_speed_decoder_for_test(false);
        }
    }
    crate::ff::force_speed_decoder_for_test(true);
    let _speed_decoder = SpeedDecoder;
    let mut ps = admit();
    assert!(crate::ff::speed_decoder_available("flac"));
    assert!(crate::player::speed_offer(&ps).contains(&1_500));
    assert!(
        crate::player::set_play_rate(&ps, 1_500),
        "host FLAC fixture must offer 1.5x"
    );
    let identity = playback_identity(&ps).unwrap();
    stopped(&mut ps);
    resume(&mut ps, &identity);
    assert_eq!(crate::player::play_rate_permille(), 1_500);
}

#[test]
fn managed_external_continuation_does_not_leak_across_fresh_admission_or_stale_identity() {
    let _serial = plx_base::testlock::serial();
    let _globals = Globals::capture();
    let mut ps = admit();
    choose(&mut ps, TrackKind::Audio, Some(1));
    choose(&mut ps, TrackKind::Subtitle, Some(0));
    crate::player::set_subtitle_offset(1_750);
    let old = playback_identity(&ps).unwrap();
    stopped(&mut ps);
    request_external_source(
        &mut ps,
        &media(),
        source("different-item"),
        "Other",
        "Managed",
        plx_platform::devcaps::assumed_shared(),
    )
    .unwrap();
    let fresh = playback_identity(&ps).unwrap();
    assert_ne!(fresh, old);
    // Keep the decoder's initial stream aligned with the admitted catalog.
    assert_eq!(crate::player::SHARED.desired_audio_idx.load(Relaxed), 0);
    assert_eq!(external_tracks(&ps).unwrap().selected_audio(), Some(0));
    assert_eq!(external_tracks(&ps).unwrap().selected_subtitle(), None);
    assert_eq!(crate::player::subtitle_offset_ms(), 0);
    assert_eq!(crate::player::play_rate_permille(), 1_000);
    stopped(&mut ps);
    assert_eq!(
        continue_external_source(
            &mut ps,
            &media(),
            source("late-old-result"),
            "Old",
            "Managed",
            plx_platform::devcaps::assumed_shared(),
            &old
        ),
        Err(ExternalError::StalePlayback)
    );
    assert!(playback_identity_matches(&ps, &fresh));
    assert_eq!(external_tracks(&ps).unwrap().selected_audio(), Some(0));
    assert_eq!(external_tracks(&ps).unwrap().selected_subtitle(), None);
}

#[test]
fn managed_external_continuation_matches_reordered_track_metadata_not_old_ordinals() {
    let _serial = plx_base::testlock::serial();
    let _globals = Globals::capture();
    let mut ps = admit();
    choose(&mut ps, TrackKind::Audio, Some(1));
    choose(&mut ps, TrackKind::Subtitle, Some(0));
    crate::player::set_subtitle_offset(1_750);
    let identity = playback_identity(&ps).unwrap();
    let mut changed = media();
    let original = changed.tracks.audio.clone();
    let mut french = row(TrackKind::Audio, 5, 0, "flac");
    french.language = "fra".into();
    french.title = "French".into();
    let mut english = original[0].clone();
    english.ordinal = 1;
    let mut spanish = original[1].clone();
    spanish.ordinal = 2;
    changed.tracks.audio = vec![french, english, spanish];
    let mut english_subtitle = changed.tracks.subtitles[0].clone();
    english_subtitle.ordinal = 1;
    let mut french_subtitle = row(TrackKind::Subtitle, 4, 0, "hdmv_pgs_subtitle");
    french_subtitle.language = "fra".into();
    french_subtitle.title = "French".into();
    changed.tracks.subtitles = vec![french_subtitle, english_subtitle];
    stopped(&mut ps);
    continue_external_source(
        &mut ps,
        &changed,
        source("reordered"),
        "Film",
        "Managed",
        plx_platform::devcaps::assumed_shared(),
        &identity,
    )
    .unwrap();
    let tracks = external_tracks(&ps).unwrap();
    assert_eq!(tracks.selected_audio(), Some(2));
    assert_eq!(tracks.inventory().audio[2].language, "spa");
    assert_eq!(tracks.selected_subtitle(), Some(1));
    assert_eq!(tracks.inventory().subtitles[1].language, "eng");
    assert_eq!(crate::player::SHARED.desired_audio_idx.load(Relaxed), 2);
    assert_eq!(crate::player::SHARED.desired_sub_idx.load(Relaxed), 1);
    assert_eq!(crate::player::subtitle_offset_ms(), 1_750);
}

#[test]
fn managed_external_continuation_does_not_guess_missing_or_ambiguous_tracks() {
    let _serial = plx_base::testlock::serial();
    let _globals = Globals::capture();
    for ambiguous in [false, true] {
        let mut ps = admit();
        choose(&mut ps, TrackKind::Audio, Some(1));
        choose(&mut ps, TrackKind::Subtitle, Some(0));
        crate::player::set_subtitle_offset(1_750);
        let old_choice = external_tracks(&ps)
            .unwrap()
            .choice(TrackKind::Audio, Some(1))
            .unwrap();
        let identity = playback_identity(&ps).unwrap();
        let mut changed = media();
        if ambiguous {
            let mut duplicate = changed.tracks.audio[1].clone();
            duplicate.ordinal = 2;
            duplicate.stream_index = 5;
            changed.tracks.audio.push(duplicate);
            let mut duplicate = changed.tracks.subtitles[0].clone();
            duplicate.ordinal = 1;
            duplicate.stream_index = 4;
            changed.tracks.subtitles.push(duplicate);
        } else {
            changed.tracks.audio[1].language = "ita".into();
            changed.tracks.audio[1].title = "Different audio at the old ordinal".into();
            changed.tracks.subtitles[0].language = "deu".into();
            changed.tracks.subtitles[0].title = "Different subtitle at the old ordinal".into();
        }
        stopped(&mut ps);
        continue_external_source(
            &mut ps,
            &changed,
            source("changed-tracks"),
            "Film",
            "Managed",
            plx_platform::devcaps::assumed_shared(),
            &identity,
        )
        .unwrap();
        assert_eq!(external_tracks(&ps).unwrap().selected_audio(), Some(0));
        assert_eq!(external_tracks(&ps).unwrap().selected_subtitle(), None);
        assert_eq!(crate::player::subtitle_offset_ms(), 0);
        assert_eq!(
            apply_external_track(&mut ps, &old_choice),
            Err(ExternalTrackApplyError::InvalidChoice(
                ExternalTrackError::StaleSession
            ))
        );
    }
}

#[test]
fn managed_external_continuation_keeps_explicit_subtitles_off() {
    let _serial = plx_base::testlock::serial();
    let _globals = Globals::capture();
    let mut ps = admit();
    choose(&mut ps, TrackKind::Subtitle, Some(0));
    crate::player::set_subtitle_offset(1_750);
    choose(&mut ps, TrackKind::Subtitle, None);
    let identity = playback_identity(&ps).unwrap();
    let mut changed = media();
    changed.tracks.subtitles[0].disposition_known = true;
    changed.tracks.subtitles[0].default = true;
    changed.tracks.subtitles[0].forced = true;
    stopped(&mut ps);
    continue_external_source(
        &mut ps,
        &changed,
        source("new-default-subtitle"),
        "Film",
        "Managed",
        plx_platform::devcaps::assumed_shared(),
        &identity,
    )
    .unwrap();
    assert_eq!(external_tracks(&ps).unwrap().selected_subtitle(), None);
    assert_eq!(crate::player::SHARED.desired_sub_idx.load(Relaxed), -1);
    assert_eq!(crate::player::subtitle_offset_ms(), 0);
}

#[test]
fn managed_external_continuation_invalid_candidate_leaves_retained_selection_untouched() {
    let _serial = plx_base::testlock::serial();
    let _globals = Globals::capture();
    let mut ps = admit();
    choose(&mut ps, TrackKind::Audio, Some(1));
    choose(&mut ps, TrackKind::Subtitle, Some(0));
    crate::player::set_subtitle_offset(1_750);
    let identity = playback_identity(&ps).unwrap();
    let catalog = external_tracks(&ps).unwrap().clone();
    let old_source = external_source(&ps);
    let mut invalid = media();
    invalid.width = 0;
    stopped(&mut ps);
    assert_eq!(
        continue_external_source(
            &mut ps,
            &invalid,
            source("invalid"),
            "Film",
            "Managed",
            plx_platform::devcaps::assumed_shared(),
            &identity
        ),
        Err(ExternalError::UnsupportedVideo)
    );
    assert!(external_tracks(&ps).unwrap().same_session(&catalog));
    assert_eq!(external_source(&ps), old_source);
    assert!(playback_identity_matches(&ps, &identity));
    assert_eq!(external_tracks(&ps).unwrap().selected_audio(), Some(1));
    assert_eq!(external_tracks(&ps).unwrap().selected_subtitle(), Some(0));
    assert_eq!(crate::player::SHARED.desired_audio_idx.load(Relaxed), 1);
    assert_eq!(crate::player::SHARED.desired_sub_idx.load(Relaxed), 0);
    assert_eq!(crate::player::subtitle_offset_ms(), 1_750);
}

#[test]
fn managed_external_continuation_restores_the_selected_audio_codec_declaration() {
    let _serial = plx_base::testlock::serial();
    let _globals = Globals::capture();
    let mut info = media();
    info.tracks.audio[1].codec = "aac".into();
    let mut ps = PlaybackSession::IDLE;
    reset_player_control_for_test(&ps);
    request_external_source(
        &mut ps,
        &info,
        source("old"),
        "Film",
        "Managed",
        plx_platform::devcaps::assumed_shared(),
    )
    .unwrap();
    reset_player_control_for_test(&ps);
    choose(&mut ps, TrackKind::Audio, Some(1));
    assert_eq!(stream_acodec(&ps), "aac");
    let identity = playback_identity(&ps).unwrap();
    stopped(&mut ps);
    continue_external_source(
        &mut ps,
        &info,
        source("renewed"),
        "Film",
        "Managed",
        plx_platform::devcaps::assumed_shared(),
        &identity,
    )
    .unwrap();
    assert_eq!(external_tracks(&ps).unwrap().selected_audio(), Some(1));
    assert_eq!(stream_acodec(&ps), "aac");
    assert_eq!(ps.cur_audio.as_ref().unwrap().codec, "aac");
    assert_eq!(crate::player::SHARED.desired_audio_idx.load(Relaxed), 1);
}

#[test]
fn managed_external_continuation_does_not_guess_which_duplicate_old_track_survived() {
    let _serial = plx_base::testlock::serial();
    let _globals = Globals::capture();
    let mut original = media();
    let mut duplicate_audio = original.tracks.audio[0].clone();
    duplicate_audio.ordinal = 1;
    duplicate_audio.stream_index = 3;
    original.tracks.audio[1] = duplicate_audio;
    let mut duplicate_subtitle = original.tracks.subtitles[0].clone();
    duplicate_subtitle.ordinal = 1;
    duplicate_subtitle.stream_index = 4;
    original.tracks.subtitles.push(duplicate_subtitle);
    let mut ps = PlaybackSession::IDLE;
    reset_player_control_for_test(&ps);
    request_external_source(
        &mut ps,
        &original,
        source("old-duplicates"),
        "Film",
        "Managed",
        plx_platform::devcaps::assumed_shared(),
    )
    .unwrap();
    reset_player_control_for_test(&ps);
    choose(&mut ps, TrackKind::Audio, Some(1));
    choose(&mut ps, TrackKind::Subtitle, Some(1));
    crate::player::set_subtitle_offset(1_750);
    let identity = playback_identity(&ps).unwrap();
    let mut changed = original.clone();
    let mut french = row(TrackKind::Audio, 5, 0, "flac");
    french.language = "fra".into();
    french.title = "French".into();
    let mut survivor = original.tracks.audio[0].clone();
    survivor.ordinal = 1;
    changed.tracks.audio = vec![french, survivor];
    changed.tracks.subtitles.truncate(1);
    stopped(&mut ps);
    continue_external_source(
        &mut ps,
        &changed,
        source("single-survivor"),
        "Film",
        "Managed",
        plx_platform::devcaps::assumed_shared(),
        &identity,
    )
    .unwrap();
    // The new single match could be either old row. Use ordinary default/Off.
    assert_eq!(external_tracks(&ps).unwrap().selected_audio(), Some(0));
    assert_eq!(external_tracks(&ps).unwrap().selected_subtitle(), None);
    assert_eq!(crate::player::SHARED.desired_audio_idx.load(Relaxed), 0);
    assert_eq!(crate::player::SHARED.desired_sub_idx.load(Relaxed), -1);
    assert_eq!(crate::player::subtitle_offset_ms(), 0);
}

#[test]
fn managed_external_continuation_keeps_duplicate_tracks_when_layout_is_exactly_unchanged() {
    let _serial = plx_base::testlock::serial();
    let _globals = Globals::capture();
    let mut info = media();
    let mut duplicate_audio = info.tracks.audio[0].clone();
    duplicate_audio.ordinal = 1;
    duplicate_audio.stream_index = 3;
    info.tracks.audio[1] = duplicate_audio;
    let mut duplicate_subtitle = info.tracks.subtitles[0].clone();
    duplicate_subtitle.ordinal = 1;
    duplicate_subtitle.stream_index = 4;
    info.tracks.subtitles.push(duplicate_subtitle);
    let mut ps = PlaybackSession::IDLE;
    reset_player_control_for_test(&ps);
    request_external_source(
        &mut ps,
        &info,
        source("duplicates"),
        "Film",
        "Managed",
        plx_platform::devcaps::assumed_shared(),
    )
    .unwrap();
    reset_player_control_for_test(&ps);
    choose(&mut ps, TrackKind::Audio, Some(1));
    choose(&mut ps, TrackKind::Subtitle, Some(1));
    crate::player::set_subtitle_offset(1_750);
    let identity = playback_identity(&ps).unwrap();
    stopped(&mut ps);
    continue_external_source(
        &mut ps,
        &info,
        source("same-layout"),
        "Film",
        "Managed",
        plx_platform::devcaps::assumed_shared(),
        &identity,
    )
    .unwrap();
    assert_eq!(external_tracks(&ps).unwrap().selected_audio(), Some(1));
    assert_eq!(external_tracks(&ps).unwrap().selected_subtitle(), Some(1));
    assert_eq!(crate::player::SHARED.desired_audio_idx.load(Relaxed), 1);
    assert_eq!(crate::player::SHARED.desired_sub_idx.load(Relaxed), 1);
    assert_eq!(crate::player::subtitle_offset_ms(), 1_750);
}

#[test]
fn managed_external_continuation_does_not_reuse_withdrawn_decoder_support() {
    let _serial = plx_base::testlock::serial();
    let _globals = Globals::capture();
    let mut ps = admit();
    choose(&mut ps, TrackKind::Audio, Some(1));
    choose(&mut ps, TrackKind::Subtitle, Some(0));
    crate::player::set_subtitle_offset(1_750);
    let identity = playback_identity(&ps).unwrap();
    let mut changed = media();
    changed.tracks.audio[1].decoder_available = false;
    changed.tracks.subtitles[0].decoder_available = false;
    stopped(&mut ps);
    continue_external_source(
        &mut ps,
        &changed,
        source("decoder-withdrawn"),
        "Film",
        "Managed",
        plx_platform::devcaps::assumed_shared(),
        &identity,
    )
    .unwrap();
    let tracks = external_tracks(&ps).unwrap();
    assert!(tracks.choice(TrackKind::Audio, Some(1)).is_err());
    assert!(tracks.choice(TrackKind::Subtitle, Some(0)).is_err());
    assert_eq!(tracks.selected_audio(), Some(0));
    assert_eq!(tracks.selected_subtitle(), None);
    assert_eq!(crate::player::subtitle_offset_ms(), 0);
}

#[test]
fn managed_external_continuation_fresh_empty_inventory_keeps_initial_audio_fallback() {
    let _serial = plx_base::testlock::serial();
    let _globals = Globals::capture();
    let mut ps = admit();
    choose(&mut ps, TrackKind::Audio, Some(1));
    let mut info = media();
    info.tracks = Default::default();
    stopped(&mut ps);
    request_external_source(
        &mut ps,
        &info,
        source("empty-inventory"),
        "Other",
        "Managed",
        plx_platform::devcaps::assumed_shared(),
    )
    .unwrap();
    assert_eq!(external_tracks(&ps).unwrap().selected_audio(), None);
    assert_eq!(crate::player::SHARED.desired_audio_idx.load(Relaxed), 0);
    assert_eq!(stream_acodec(&ps), "flac");
}
