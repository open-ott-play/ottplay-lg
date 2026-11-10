use super::*;
fn admit(playback: HlsPlayback) -> PlaybackSession {
    let mut ps = PlaybackSession::IDLE;
    crate::route::reset_player_control_for_test(&ps);
    let info = crate::ff::inspect::MediaInfo {
        resolved_source: None, hls_playback: Some(playback), tracks: Default::default(),
        video_codec: "h264".into(), audio_codec: "aac".into(), width: 160, height: 90,
        fps: 24.0, duration_ns: 1_000_000_000, audio_channels: 2, audio_sample_rate: 48000,
        audio_bit_depth: 0, dovi: plx_data::metadata::Dovi::NONE,
    };
    request_external_source(&mut ps, &info, crate::ff::ExternalSource::Hls("https://example.invalid/master.m3u8".into()),
        "Fixture", "M3U", plx_platform::devcaps::assumed_shared()).unwrap();
    crate::route::reset_player_control_for_test(&ps);
    ps
}
#[test]
fn current_prepared_rendition_changes_only_its_native_audio_route() {
    let _serial = plx_base::testlock::serial();
    let _globals = HlsTrackRequest::test_globals_guard();
    let playback = HlsPlayback::fixture_for_test();
    let mut ps = admit(playback.clone());
    let catalog = hls_track_catalog(&ps).unwrap();
    let target = catalog.rows()[1].id;
    let source = external_source(&ps);
    crate::player::TX.commit_paused(true);
    crate::player::SHARED.playpos_ns.store(700_000_000, std::sync::atomic::Ordering::Relaxed);
    let request = begin_hls_track_request(&mut ps, &catalog.choice(target).unwrap()).unwrap();
    assert!(hls_track_request_is_current(&ps, &request));
    assert_eq!(complete_hls_track_request(&mut ps, &request, PreparedHlsTrack(Prepared::Audio(playback.selected_for_test(target)))), Ok(true));
    assert_eq!(external_source(&ps), source);
    assert!(crate::player::transport::paused());
    assert_eq!(crate::player::SHARED.playpos_ns.load(std::sync::atomic::Ordering::Relaxed), 700_000_000);
    assert!(hls_track_catalog(&ps).unwrap().rows()[1].selected);
    assert!(external_tracks(&ps).unwrap().inventory().audio.is_empty(), "rendition is not an AVStream row");
    assert!(matches!(PLAYER_CONTROL.lock().unwrap().pending_user, Some(UserRouteIntent::NativeAudioReload)));
}
#[test]
fn later_user_selection_retires_preparation_even_when_its_target_is_unchanged() {
    let _serial = plx_base::testlock::serial();
    let _globals = HlsTrackRequest::test_globals_guard();
    let playback = HlsPlayback::fixture_for_test();
    let mut ps = admit(playback.clone());
    let catalog = hls_track_catalog(&ps).unwrap();
    let first = catalog.rows()[0].id;
    let other = catalog.rows()[1].id;
    let older = begin_hls_track_request(&mut ps, &catalog.choice(other).unwrap()).unwrap();
    let newer = begin_hls_track_request(&mut ps, &catalog.choice(first).unwrap()).unwrap();
    assert!(!hls_track_request_is_current(&ps, &older));
    assert!(hls_track_request_is_current(&ps, &newer));
    assert!(complete_hls_track_request(&mut ps, &older, PreparedHlsTrack(Prepared::Audio(playback.selected_for_test(other)))).is_err());
    assert!(hls_track_catalog(&ps).unwrap().rows()[0].selected);
}
#[test]
fn reopen_same_url_rejects_old_menu_and_private_urls_do_not_enter_debug() {
    let _serial = plx_base::testlock::serial();
    let _globals = HlsTrackRequest::test_globals_guard();
    let playback = HlsPlayback::fixture_for_test();
    let old = admit(playback.clone());
    let catalog = hls_track_catalog(&old).unwrap();
    let choice = catalog.choice(catalog.rows()[1].id).unwrap();
    let mut replacement = admit(playback);
    assert!(begin_hls_track_request(&mut replacement, &choice).is_err());
    for debug in [format!("{catalog:?}"), format!("{choice:?}")] {
        assert!(!debug.contains("example.invalid"));
        assert!(!debug.contains("m3u8"));
    }
}

struct ResetHlsSubtitles;
impl ResetHlsSubtitles {
    fn new() -> Self { sidecar::reset_hls(); Self }
}
impl Drop for ResetHlsSubtitles {
    fn drop(&mut self) { sidecar::reset_hls(); }
}
fn subtitle_choice(ps: &PlaybackSession, index: usize) -> HlsTrackChoice {
    let catalog = hls_track_catalog(ps).unwrap();
    catalog.subtitle_choice(Some(catalog.subtitle_rows()[index].id)).unwrap()
}
fn install_subtitle(ps: &mut PlaybackSession, index: usize) {
    let choice = subtitle_choice(ps, index);
    let request = begin_hls_track_request(ps, &choice).unwrap();
    let prepared = request.prepared_for_test();
    assert_eq!(complete_hls_track_request(ps, &request, prepared), Ok(true));
}

#[test]
fn hls_webvtt_prepares_then_installs_without_audio_reload_or_transport_change() {
    let _serial = plx_base::testlock::serial();
    let _globals = HlsTrackRequest::test_globals_guard();
    let _subtitles = ResetHlsSubtitles::new();
    let mut ps = admit(HlsPlayback::fixture_with_subtitles_for_test());
    let source = external_source(&ps);
    crate::player::TX.commit_paused(true);
    crate::player::SHARED.playpos_ns.store(700_000_000, std::sync::atomic::Ordering::Relaxed);
    let choice = subtitle_choice(&ps, 0);
    let request = begin_hls_track_request(&mut ps, &choice).unwrap();
    let prepared = request.prepared_for_test();
    assert_eq!(hls_track_catalog(&ps).unwrap().selected_subtitle(), None);
    assert!(!sidecar::selected(), "private preparation must not publish a new selection");
    assert_eq!(complete_hls_track_request(&mut ps, &request, prepared), Ok(true));
    assert!(hls_track_catalog(&ps).unwrap().subtitle_rows()[0].selected);
    assert_eq!(subtitle_presenter(&ps), SubtitlePresenter::Client);
    assert!(sidecar::ass_source(false, request.text_style).is_some(), "real shared renderer script");
    assert_eq!(external_source(&ps), source);
    assert!(crate::player::transport::paused());
    assert_eq!(crate::player::SHARED.playpos_ns.load(std::sync::atomic::Ordering::Relaxed), 700_000_000);
    assert!(PLAYER_CONTROL.lock().unwrap().pending_user.is_none(), "no A/V reload for WebVTT");
    assert!(hls_track_catalog(&ps).unwrap().rows()[0].selected);
    assert!(external_tracks(&ps).unwrap().inventory().subtitles.is_empty(), "no fabricated AVStream");
}

#[test]
fn hls_subtitle_off_retires_pending_result_and_clears_shared_renderer() {
    let _serial = plx_base::testlock::serial();
    let _globals = HlsTrackRequest::test_globals_guard();
    let _subtitles = ResetHlsSubtitles::new();
    let mut ps = admit(HlsPlayback::fixture_with_subtitles_for_test());
    install_subtitle(&mut ps, 0);
    let choice = subtitle_choice(&ps, 1);
    let pending = begin_hls_track_request(&mut ps, &choice).unwrap();
    let late = pending.prepared_for_test();
    let off = hls_track_catalog(&ps).unwrap().subtitle_choice(None).unwrap();
    let off = begin_hls_track_request(&mut ps, &off).unwrap();
    assert!(!hls_track_request_is_current(&ps, &pending));
    assert_eq!(complete_hls_track_request(&mut ps, &off, off.prepared_for_test()), Ok(true));
    assert_eq!(subtitle_presenter(&ps), SubtitlePresenter::None);
    assert!(!sidecar::selected());
    assert!(complete_hls_track_request(&mut ps, &pending, late).is_err());
    assert_eq!(hls_track_catalog(&ps).unwrap().selected_subtitle(), None);
    assert!(PLAYER_CONTROL.lock().unwrap().pending_user.is_none());
}

#[test]
fn hls_subtitle_applied_seek_keeps_installed_script_but_retires_unfinished_selection() {
    let _serial = plx_base::testlock::serial();
    let _globals = HlsTrackRequest::test_globals_guard();
    let _subtitles = ResetHlsSubtitles::new();
    let mut ps = admit(HlsPlayback::fixture_with_subtitles_for_test());
    install_subtitle(&mut ps, 0);
    let style = TextStyle::current(subtitle_text_appearance(&ps));
    let script = sidecar::ass_source(false, style).unwrap();
    let choice = subtitle_choice(&ps, 1);
    let pending = begin_hls_track_request(&mut ps, &choice).unwrap();
    let late = pending.prepared_for_test();
    note_user_seek_intent(100_000_000);
    assert!(commit_user_seek());
    assert!(!hls_track_request_is_current(&ps, &pending));
    assert!(complete_hls_track_request(&mut ps, &pending, late).is_err());
    assert!(Arc::ptr_eq(&script, &sidecar::ass_source(false, style).unwrap()));
    assert!(hls_track_catalog(&ps).unwrap().subtitle_rows()[0].selected);
    assert!(PLAYER_CONTROL.lock().unwrap().pending_user.is_none());
}

#[test]
fn hls_subtitle_same_url_new_playback_and_foreign_prepared_target_are_rejected() {
    let _serial = plx_base::testlock::serial();
    let _globals = HlsTrackRequest::test_globals_guard();
    let _subtitles = ResetHlsSubtitles::new();
    let playback = HlsPlayback::fixture_with_subtitles_for_test();
    let mut ps = admit(playback.clone());
    install_subtitle(&mut ps, 0);
    let choice = subtitle_choice(&ps, 1);
    let pending = begin_hls_track_request(&mut ps, &choice).unwrap();
    let late = pending.prepared_for_test();
    forget_external_source(&mut ps);
    assert!(!sidecar::selected());
    assert!(hls_track_catalog(&ps).is_none());
    ps = admit(playback);
    assert!(begin_hls_track_request(&mut ps, &choice).is_err());
    assert!(complete_hls_track_request(&mut ps, &pending, late).is_err());
    let first = subtitle_choice(&ps, 0);
    let first = begin_hls_track_request(&mut ps, &first).unwrap();
    let wrong = first.prepared_for_test();
    let second = subtitle_choice(&ps, 1);
    let second = begin_hls_track_request(&mut ps, &second).unwrap();
    assert!(complete_hls_track_request(&mut ps, &second, wrong).is_err());
    assert_eq!(hls_track_catalog(&ps).unwrap().selected_subtitle(), None);
}

#[test]
fn hls_same_subtitle_reselection_keeps_offset_and_supersedes_pending_other_track() {
    let _serial = plx_base::testlock::serial();
    let _globals = HlsTrackRequest::test_globals_guard();
    let _subtitles = ResetHlsSubtitles::new();
    let mut ps = admit(HlsPlayback::fixture_with_subtitles_for_test());
    install_subtitle(&mut ps, 0);
    crate::player::set_subtitle_offset(1200);
    let choice = subtitle_choice(&ps, 1);
    let pending = begin_hls_track_request(&mut ps, &choice).unwrap();
    let late = pending.prepared_for_test();
    install_subtitle(&mut ps, 0);
    assert_eq!(crate::player::subtitle_offset_ms(), 1200);
    assert!(complete_hls_track_request(&mut ps, &pending, late).is_err());
    assert_eq!(crate::player::subtitle_offset_ms(), 1200);
    assert!(hls_track_catalog(&ps).unwrap().subtitle_rows()[0].selected);
    install_subtitle(&mut ps, 1);
    assert_eq!(crate::player::subtitle_offset_ms(), 0, "different subtitle clears prior timing adjustment");
}

#[test]
fn subtitle_preparation_keeps_previous_refresh_until_successful_commit() {
    let _serial = plx_base::testlock::serial();
    let _globals = HlsTrackRequest::test_globals_guard();
    let _subtitles = ResetHlsSubtitles::new();
    let mut ps = admit(HlsPlayback::fixture_with_subtitles_for_test());
    install_subtitle(&mut ps, 0);
    let catalog = hls_track_catalog(&ps).unwrap();
    let owner = playback_identity(&ps).unwrap();
    let id = catalog.selected_subtitle().unwrap();
    let token = sidecar::hls_refresh_token(&owner, id).unwrap();
    let choice = subtitle_choice(&ps, 1);
    let request = begin_hls_track_request(&mut ps, &choice).unwrap();
    assert!(!token.cancelled.load(std::sync::atomic::Ordering::Acquire));
    assert!(sidecar::hls_refresh_is_current(&token), "failed preparation must leave old refresh alive");
    assert_eq!(sidecar::selected_hls(&owner), Some(id));
    assert!(hls_track_request_is_current(&ps, &request));
    assert_eq!(complete_hls_track_request(&mut ps, &request, request.prepared_for_test()), Ok(true));
    assert!(token.cancelled.load(std::sync::atomic::Ordering::Acquire));
    assert!(!sidecar::clear_hls_refresh(&token), "late worker cannot erase the new subtitle");
    assert_eq!(sidecar::selected_hls(&owner), Some(catalog.subtitle_rows()[1].id));
}

#[test]
fn live_menu_subtitle_uses_existing_request_path_and_rejects_old_decoder_epoch() {
    let _serial = plx_base::testlock::serial();
    let _globals = HlsTrackRequest::test_globals_guard();
    let _subtitles = ResetHlsSubtitles::new();
    let playback = HlsPlayback::fixture_live_with_subtitles_for_test();
    let mut ps = admit(playback.clone());
    let catalog = hls_track_catalog(&ps).unwrap();
    assert!(catalog.rows().is_empty(), "live embedded audio is not a separate VOD rendition");
    assert_eq!(catalog.subtitle_rows().len(), 2);
    let choice = subtitle_choice(&ps, 0);
    let request = begin_hls_track_request(&mut ps, &choice).unwrap();
    let prepared = request.prepared_for_test();
    playback.commit_live_clock_for_test(1, 20_000_000_000);
    assert!(complete_hls_track_request(&mut ps, &request, prepared).is_err());
    assert!(!sidecar::selected(), "old-epoch preparation cannot enter renderer");
}

fn auto_policy(mode: HlsSubtitleMode, language: &str) -> HlsSubtitlePolicy {
    HlsSubtitlePolicy { mode, language: language.into() }
}
#[test]
fn automatic_default_and_forced_requests_use_canonical_policy_and_manual_off_wins() {
    let _serial = plx_base::testlock::serial();
    let _globals = HlsTrackRequest::test_globals_guard();
    let _subtitles = ResetHlsSubtitles::new();
    let mut ps = admit(HlsPlayback::fixture_with_subtitles_for_test());
    assert_eq!(hls_subtitle_policy(&ps), HlsSubtitlePolicy::default());
    assert!(begin_automatic_hls_subtitle_request(&mut ps).unwrap().is_none());
    restore_hls_subtitle_policy(&mut ps, auto_policy(HlsSubtitleMode::Auto, "en"));
    let automatic = begin_automatic_hls_subtitle_request(&mut ps).unwrap().unwrap();
    assert_eq!(automatic.choice.target, HlsTrackTarget::Subtitle(Some(HlsSubtitleId(1))));
    assert_eq!(hls_subtitle_intent(&ps), HlsSubtitleIntent::Auto);
    assert!(begin_automatic_hls_subtitle_request(&mut ps).unwrap().is_none(), "one auto worker per admitted intent");
    let prepared = automatic.prepared_for_test();
    let off = hls_track_catalog(&ps).unwrap().subtitle_choice(None).unwrap();
    let off = begin_hls_track_request(&mut ps, &off).unwrap();
    assert_eq!(hls_subtitle_intent(&ps), HlsSubtitleIntent::ManualOff);
    assert!(complete_hls_track_request(&mut ps, &automatic, prepared).is_err());
    assert_eq!(complete_hls_track_request(&mut ps, &off, off.prepared_for_test()), Ok(true));
    restore_hls_subtitle_policy(&mut ps, auto_policy(HlsSubtitleMode::Forced, "es"));
    assert!(begin_automatic_hls_subtitle_request(&mut ps).unwrap().is_none());
    assert_eq!(hls_subtitle_intent(&ps.publication()), HlsSubtitleIntent::ManualOff);
    ps = admit(HlsPlayback::fixture_with_subtitles_for_test());
    restore_hls_subtitle_policy(&mut ps, auto_policy(HlsSubtitleMode::Forced, "en"));
    let forced = begin_automatic_hls_subtitle_request(&mut ps).unwrap().unwrap();
    assert_eq!(forced.choice.target, HlsTrackTarget::Subtitle(Some(HlsSubtitleId(2))), "Forced cannot fall back to full DEFAULT captions");
}

#[test]
fn automatic_policy_off_retires_auto_cues_but_preserves_manual_selection() {
    let _serial = plx_base::testlock::serial();
    let _globals = HlsTrackRequest::test_globals_guard();
    let _subtitles = ResetHlsSubtitles::new();
    let mut ps = admit(HlsPlayback::fixture_with_subtitles_for_test());
    restore_hls_subtitle_policy(&mut ps, auto_policy(HlsSubtitleMode::Auto, "en"));
    let request = begin_automatic_hls_subtitle_request(&mut ps).unwrap().unwrap();
    assert_eq!(complete_hls_track_request(&mut ps, &request, request.prepared_for_test()), Ok(true));
    assert!(sidecar::selected());
    restore_hls_subtitle_policy(&mut ps, HlsSubtitlePolicy::default());
    let off = begin_automatic_hls_subtitle_request(&mut ps).unwrap().unwrap();
    assert_eq!(off.choice.target, HlsTrackTarget::Subtitle(None));
    assert_eq!(complete_hls_track_request(&mut ps, &off, off.prepared_for_test()), Ok(true));
    assert!(!sidecar::selected());
    install_subtitle(&mut ps, 0);
    restore_hls_subtitle_policy(&mut ps, auto_policy(HlsSubtitleMode::Forced, "es"));
    assert!(begin_automatic_hls_subtitle_request(&mut ps).unwrap().is_none());
    restore_hls_subtitle_policy(&mut ps, HlsSubtitlePolicy::default());
    assert!(begin_automatic_hls_subtitle_request(&mut ps).unwrap().is_none());
    assert_eq!(hls_subtitle_intent(&ps), HlsSubtitleIntent::ManualTrack);
    assert_eq!(hls_track_catalog(&ps).unwrap().selected_subtitle(), Some(HlsSubtitleId(1)));
}

#[test]
fn manual_track_and_policy_change_retire_private_automatic_candidates() {
    let _serial = plx_base::testlock::serial();
    let _globals = HlsTrackRequest::test_globals_guard();
    let _subtitles = ResetHlsSubtitles::new();
    let mut ps = admit(HlsPlayback::fixture_with_subtitles_for_test());
    restore_hls_subtitle_policy(&mut ps, auto_policy(HlsSubtitleMode::Auto, "en"));
    let old = begin_automatic_hls_subtitle_request(&mut ps).unwrap().unwrap();
    let prepared = old.prepared_for_test();
    restore_hls_subtitle_policy(&mut ps, auto_policy(HlsSubtitleMode::Forced, "es"));
    assert!(!hls_track_request_is_current(&ps, &old));
    assert!(complete_hls_track_request(&mut ps, &old, prepared).is_err());
    let newer = begin_automatic_hls_subtitle_request(&mut ps).unwrap().unwrap();
    let prepared = newer.prepared_for_test();
    install_subtitle(&mut ps, 0);
    assert!(complete_hls_track_request(&mut ps, &newer, prepared).is_err());
    assert_eq!(hls_subtitle_intent(&ps), HlsSubtitleIntent::ManualTrack);
}

#[test]
fn manual_off_survives_audio_reload_and_logical_continuation_but_new_item_resets_it() {
    let _serial = plx_base::testlock::serial();
    let _globals = HlsTrackRequest::test_globals_guard();
    let _subtitles = ResetHlsSubtitles::new();
    let playback = HlsPlayback::fixture_with_subtitles_for_test();
    let mut ps = admit(playback.clone());
    restore_hls_subtitle_policy(&mut ps, auto_policy(HlsSubtitleMode::Auto, "en"));
    let choice = hls_track_catalog(&ps).unwrap().subtitle_choice(None).unwrap();
    let off = begin_hls_track_request(&mut ps, &choice).unwrap();
    assert_eq!(complete_hls_track_request(&mut ps, &off, off.prepared_for_test()), Ok(true));
    let catalog = hls_track_catalog(&ps).unwrap();
    let audio = begin_hls_track_request(&mut ps, &catalog.choice(catalog.rows()[1].id).unwrap()).unwrap();
    assert_eq!(complete_hls_track_request(&mut ps, &audio, audio.prepared_for_test()), Ok(true));
    assert_eq!(hls_subtitle_intent(&ps), HlsSubtitleIntent::ManualOff);
    let owner = playback_identity(&ps).unwrap();
    let info = playback.media_info_for_test();
    let source = external_source(&ps).unwrap();
    clear_url(&mut ps);
    cancel_play(&mut ps);
    reset_player_control_for_test(&ps);
    continue_external_source(&mut ps, &info, source.clone(), "Fixture", "M3U",
        plx_platform::devcaps::assumed_shared(), &owner).unwrap();
    assert_eq!(hls_subtitle_intent(&ps), HlsSubtitleIntent::ManualOff);
    assert!(begin_automatic_hls_subtitle_request(&mut ps).unwrap().is_none());
    clear_url(&mut ps);
    cancel_play(&mut ps);
    reset_player_control_for_test(&ps);
    request_external_source(&mut ps, &info, source, "New item", "M3U",
        plx_platform::devcaps::assumed_shared()).unwrap();
    reset_player_control_for_test(&ps);
    assert_eq!(hls_subtitle_intent(&ps), HlsSubtitleIntent::Unset);
    assert_eq!(hls_subtitle_policy(&ps), auto_policy(HlsSubtitleMode::Auto, "en"));
    assert!(begin_automatic_hls_subtitle_request(&mut ps).unwrap().is_some());
}

#[test]
fn embedded_catalog_off_retires_an_auto_hls_rendition_and_fences_late_automatic_work() {
    let _serial = plx_base::testlock::serial();
    let _globals = HlsTrackRequest::test_globals_guard();
    let _subtitles = ResetHlsSubtitles::new();
    let mut ps = admit(HlsPlayback::fixture_with_subtitles_for_test());
    restore_hls_subtitle_policy(&mut ps, auto_policy(HlsSubtitleMode::Auto, "en"));
    let automatic = begin_automatic_hls_subtitle_request(&mut ps).unwrap().unwrap();
    let prepared = automatic.prepared_for_test();
    let off = external_tracks(&ps).unwrap().choice(crate::ff::inspect::TrackKind::Subtitle, None).unwrap();
    assert_eq!(apply_external_track(&mut ps, &off), Ok(false), "accepted no-op still records explicit Off");
    assert_eq!(hls_subtitle_intent(&ps), HlsSubtitleIntent::ManualOff);
    assert!(complete_hls_track_request(&mut ps, &automatic, prepared).is_err());
    assert!(begin_automatic_hls_subtitle_request(&mut ps).unwrap().is_none());
    install_subtitle(&mut ps, 0);
    assert!(sidecar::selected());
    let off = external_tracks(&ps).unwrap().choice(crate::ff::inspect::TrackKind::Subtitle, None).unwrap();
    assert_eq!(apply_external_track(&mut ps, &off), Ok(true), "HLS sidecar makes embedded Off an actual change");
    assert!(!sidecar::selected());
    assert_eq!(subtitle_presenter(&ps), SubtitlePresenter::None);
    assert_eq!(hls_subtitle_intent(&ps), HlsSubtitleIntent::ManualOff);
}
