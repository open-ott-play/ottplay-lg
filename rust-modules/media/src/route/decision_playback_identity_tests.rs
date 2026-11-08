//! No decoder, socket or resolve worker is started by these fixtures.
use super::*;

struct ResetControl {
    audio: i32,
    subtitle: i32,
    offset_ms: i64,
}
impl ResetControl {
    fn new() -> Self {
        let value = Self {
            audio: crate::player::SHARED.desired_audio_idx.load(Ordering::Relaxed),
            subtitle: crate::player::SHARED.desired_sub_idx.load(Ordering::Relaxed),
            offset_ms: crate::player::subtitle_offset_ms(),
        };
        reset_player_control_for_test(&PlaybackSession::IDLE);
        value
    }
}
impl Drop for ResetControl {
    fn drop(&mut self) {
        reset_player_control_for_test(&PlaybackSession::IDLE);
        crate::player::SHARED.desired_audio_idx.store(self.audio, Ordering::Relaxed);
        crate::player::SHARED.desired_sub_idx.store(self.subtitle, Ordering::Relaxed);
        crate::player::restore_subtitle_offset(self.offset_ms);
    }
}
fn media() -> crate::ff::inspect::MediaInfo {
    crate::ff::inspect::MediaInfo {
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
        audio_sample_rate: 48_000,
        audio_bit_depth: 16,
        dovi: Default::default(),
    }
}
fn source(name: &str) -> crate::ff::ExternalSource {
    crate::ff::ExternalSource::Local(std::path::PathBuf::from(format!("/synthetic/{name}.mp4")))
}
fn admit(ps: &mut PlaybackSession, name: &str) -> PlaybackIdentity {
    request_external_source(ps, &media(), source(name), "Synthetic", "Local",
        plx_platform::devcaps::assumed_shared()).unwrap();
    playback_identity(ps).expect("successful admission installs a logical owner")
}
fn stopped(ps: &mut PlaybackSession) {
    // The engine's ordinary stop clears the URL, retaining source and logical
    // item for retries. This fixture never creates an Engine to tear down.
    clear_url(ps);
    cancel_play(ps);
    reset_player_control_for_test(ps);
}

#[test]
fn logical_identity_uses_owned_instances_not_equal_values() {
    let first = PlaybackIdentity::fresh();
    let copy = first.clone();
    let next = PlaybackIdentity::fresh();
    assert_eq!(first, copy);
    assert_ne!(first, next);
    assert_eq!(format!("{first:?}"), "PlaybackIdentity(<opaque>)");
}

#[test]
fn logical_admission_rotates_fresh_but_preserves_retries() {
    let old = PlaybackIdentity::fresh();
    let fresh = PlaybackIdentity::for_admission(Some(&old), false).unwrap();
    assert_ne!(old, fresh);
    assert_eq!(PlaybackIdentity::for_admission(Some(&fresh), true), Some(fresh));
    assert_eq!(PlaybackIdentity::for_admission(None, true), None,
        "an untagged legacy retry cannot fabricate prior logical authority");
}

#[test]
fn external_same_source_fresh_admission_invalidates_old_identity() {
    let _serial = plx_base::testlock::serial();
    let _reset = ResetControl::new();
    let mut ps = PlaybackSession::IDLE;
    let old = admit(&mut ps, "same");
    assert!(playback_identity_matches(&ps.publication(), &old));
    stopped(&mut ps);
    let fresh = admit(&mut ps, "same");
    assert_ne!(old, fresh);
    assert!(!playback_identity_matches(&ps, &old));
    assert!(playback_identity_matches(&ps, &fresh));
}

#[test]
fn external_managed_continuation_preserves_identity_and_rejects_stale_owner() {
    let _serial = plx_base::testlock::serial();
    let _reset = ResetControl::new();
    let mut ps = PlaybackSession::IDLE;
    let old = admit(&mut ps, "expired");
    stopped(&mut ps);
    continue_external_source(&mut ps, &media(), source("renewed"), "Synthetic", "Local",
        plx_platform::devcaps::assumed_shared(), &old).unwrap();
    assert!(playback_identity_matches(&ps, &old));
    assert_eq!(external_source(&ps), Some(source("renewed")));
    stopped(&mut ps);
    let newer = admit(&mut ps, "new-selection");
    stopped(&mut ps);
    let before_source = external_source(&ps);
    assert_eq!(continue_external_source(&mut ps, &media(), source("late-old-result"), "Old", "Local",
        plx_platform::devcaps::assumed_shared(), &old), Err(ExternalError::StalePlayback));
    assert_eq!(external_source(&ps), before_source);
    assert!(playback_identity_matches(&ps, &newer));
}

#[test]
fn rejected_external_admission_does_not_rotate_logical_identity() {
    let _serial = plx_base::testlock::serial();
    let _reset = ResetControl::new();
    let mut ps = PlaybackSession::IDLE;
    let old = admit(&mut ps, "old");
    let url = ps.url.clone();
    assert_eq!(request_external_source(&mut ps, &media(), source("new"), "New", "Local",
        plx_platform::devcaps::assumed_shared()), Err(ExternalError::Busy));
    assert!(playback_identity_matches(&ps, &old));
    assert_eq!(ps.url, url);
    stopped(&mut ps);
    let mut invalid = media();
    invalid.width = 0;
    assert_eq!(request_external_source(&mut ps, &invalid, source("invalid"), "Invalid", "Local",
        plx_platform::devcaps::assumed_shared()), Err(ExternalError::UnsupportedVideo));
    assert!(playback_identity_matches(&ps, &old));
    assert_eq!(external_source(&ps), Some(source("old")));
    forget_external_source(&mut ps);
    assert_eq!(playback_identity(&ps), None);
    assert_eq!(continue_external_source(&mut ps, &media(), source("renewed"), "Synthetic", "Local",
        plx_platform::devcaps::assumed_shared(), &old), Err(ExternalError::StalePlayback));
}

#[test]
fn rejected_plex_request_does_not_rotate_logical_identity() {
    let _serial = plx_base::testlock::serial();
    let _reset = ResetControl::new();
    let mut ps = PlaybackSession::IDLE;
    let old = admit(&mut ps, "old");
    let mut meta = plx_data::stores::metadata::MetadataStore::default();
    assert!(!request_play(&mut ps, &mut meta, plx_platform::devcaps::assumed_shared(),
        ServerId::UNSET, "", "", "", "", "Invalid", "", None));
    assert!(playback_identity_matches(&ps, &old));
    force_applying_for_test(42);
    assert!(!request_play(&mut ps, &mut meta, plx_platform::devcaps::assumed_shared(),
        ServerId::UNSET, "42", "", "", "", "Busy", "", None));
    assert!(playback_identity_matches(&ps, &old));
    assert_eq!(external_source(&ps), Some(source("old")));
}

#[test]
fn refused_plex_plan_and_route_rollback_preserve_logical_identity() {
    let _serial = plx_base::testlock::serial();
    let _reset = ResetControl::new();
    let mut ps = PlaybackSession::IDLE;
    ps.playback_identity = PlaybackIdentity::for_admission(None, false);
    let id = playback_identity(&ps).unwrap();
    let mut meta = plx_data::stores::metadata::MetadataStore::default();
    assert!(begin_playback_request());
    assert!(apply_plan(&mut ps, &mut meta, Plan::default(), "42").is_none());
    assert!(playback_identity_matches(&ps, &id));
    assert!(playback_identity_matches(&ps.publication(), &id));
    let projection = route_projection(&ps);
    ps.url = "https://synthetic.invalid/staged".into();
    install_route_projection(&mut ps, &projection);
    assert!(playback_identity_matches(&ps, &id));
    assert!(ps.url.is_empty());
}
