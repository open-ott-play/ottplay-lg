//! The real external-route admission and Load decision/builder; no file, demuxer or sink runs.
use super::*;
use crate::ff::inspect::{InspectedTrack, MediaInfo, TrackInventory, TrackKind};
use std::sync::atomic::Ordering::Relaxed;

// Admission publishes track/clock projections. Preserve those just like external continuation
// tests, so these decision tests neither depend on nor leak another test's playback state.
struct Globals {
    audio: i32,
    subtitle: i32,
    offset: i64,
    rate: i32,
    applied_rate: i32,
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
        let bitmaps = std::mem::take(&mut *SHARED.sub_bitmaps.lock().unwrap());
        Self {
            audio: SHARED.desired_audio_idx.load(Relaxed),
            subtitle: SHARED.desired_sub_idx.load(Relaxed),
            offset: crate::player::subtitle_offset_ms(),
            rate: crate::player::play_rate_permille(),
            applied_rate: SHARED.applied_rate_permille.swap(crate::player::RATE_NORMAL, Relaxed),
            rate_reload: SHARED.rate_reload.load(Relaxed),
            position: SHARED.playpos_ns.load(Relaxed),
            state: crate::player::swap_state_for_test(crate::player::PlaybackState::Playing),
            paused: crate::player::transport::paused(),
            audio_switch: SHARED.audio_switch.load(Relaxed),
            audio_pick_ms: SHARED.audio_pick_ms.load(Relaxed),
            restore_bitmaps: Some(Box::new(move || {
                *SHARED.sub_bitmaps.lock().unwrap() = bitmaps;
            })),
        }
    }
}
impl Drop for Globals {
    fn drop(&mut self) {
        crate::route::reset_player_control_for_test(&crate::route::PlaybackSession::IDLE);
        SHARED.desired_audio_idx.store(self.audio, Relaxed);
        SHARED.desired_sub_idx.store(self.subtitle, Relaxed);
        SHARED.play_rate_permille.store(self.rate, Relaxed);
        SHARED.applied_rate_permille.store(self.applied_rate, Relaxed);
        SHARED.rate_reload.store(self.rate_reload, Relaxed);
        SHARED.playpos_ns.store(self.position, Relaxed);
        SHARED.audio_switch.store(self.audio_switch, Relaxed);
        SHARED.audio_pick_ms.store(self.audio_pick_ms, Relaxed);
        if let Some(restore) = self.restore_bitmaps.take() { restore(); }
        crate::player::restore_subtitle_offset(self.offset);
        TX.commit_paused(self.paused);
        crate::player::restore_state_for_test(self.state);
    }
}
fn media() -> MediaInfo {
    MediaInfo {
        resolved_source: None, hls_playback: None,
        tracks: TrackInventory::complete_for_test(Vec::new()).unwrap(),
        video_codec: "h264".into(), audio_codec: String::new(),
        width: 1280, height: 720, fps: 24.0, duration_ns: 180_000_000_000,
        audio_channels: 0, audio_sample_rate: 0, audio_bit_depth: 0,
        dovi: plx_data::metadata::Dovi::NONE,
    }
}
fn admit(info: &MediaInfo) -> crate::route::PlaybackSession {
    let mut ps = crate::route::PlaybackSession::IDLE;
    crate::route::reset_player_control_for_test(&ps);
    let source = crate::ff::ExternalSource::Local("/synthetic/video-only.mp4".into());
    assert_eq!(crate::route::request_external_source(
        &mut ps, info, source.clone(), "Synthetic", "Local",
        plx_platform::devcaps::assumed_shared(),
    ), Ok(()), "the production external admission must accept these inspected facts");
    assert_eq!(crate::route::external_source(&ps), Some(source));
    ps
}
fn audio_row(codec: &str) -> InspectedTrack {
    InspectedTrack {
        kind: TrackKind::Audio, stream_index: 1, ordinal: 0, codec: codec.into(),
        language: String::new(), title: String::new(), channels: 2, sample_rate: 48_000,
        bit_depth: 16, decoder_available: true, disposition_known: false,
        default: false, forced: false,
    }
}
fn declared_audio(ps: &crate::route::PlaybackSession) -> Option<LoadAudio> {
    load_audio(&crate::route::stream_acodec(ps), crate::player::RATE_NORMAL,
        None, None, AudioStack::Unknown)
}

#[test]
fn complete_video_only_local_keeps_dynamic_envelope_and_video_metadata() {
    let _serial = plx_base::testlock::serial();
    let _globals = Globals::capture();
    for (width, height, fps, expected) in [
        (1280, 720, 24.0, (1920, 1080, 24)),
        (3840, 2160, 60.0, (3840, 2160, 60)),
    ] {
        let info = MediaInfo { width, height, fps, ..media() };
        let ps = admit(&info);
        assert!(inspected_audio_absent(&ps), "complete Local inventory establishes absent audio");
        let env = sink_envelope(LoadVideo::H264, crate::route::sink_max_raster(&ps),
            crate::route::stream_fps(&ps), &plx_platform::devcaps::Caps::assumed(), false);
        assert_eq!((env.w, env.h, env.fps), expected);
        let actual: serde_json::Value = serde_json::from_str(&build_video_payload(&ps, "H264", env)).unwrap();
        let mut shared: serde_json::Value = serde_json::from_str(&build_av_payload(&ps, "H264", "AAC", env)).unwrap();
        let option = &actual["args"][0]["option"];
        assert_eq!(option["needAudio"], false);
        assert!(option["externalStreamingInfo"]["contents"]["codec"].get("audio").is_none());
        assert_eq!(option["adaptiveStreaming"]["maxFrameRate"], expected.2);
        assert!(option["externalStreamingInfo"]["contents"]["esInfo"].get("videoFpsValue").is_some());
        shared["args"][0]["option"]["needAudio"] = false.into();
        shared["args"][0]["option"]["externalStreamingInfo"]["contents"]["codec"]
            .as_object_mut().unwrap().remove("audio");
        assert_eq!(actual, shared, "only audio differs; all shared video/HDR/timing fields survive");
    }
}

#[test]
fn incomplete_empty_inventory_and_uninspected_routes_do_not_authorize_video_only() {
    let _serial = plx_base::testlock::serial();
    let _globals = Globals::capture();
    let info = MediaInfo { tracks: TrackInventory::default(), ..media() };
    let ps = admit(&info);
    assert!(!inspected_audio_absent(&ps), "fallback empty vectors are not proof of absence");
    assert!(declared_audio(&ps).is_none(), "the missing-codec refusal remains");
    let mut uninspected = crate::route::PlaybackSession::IDLE;
    crate::route::set_url(&mut uninspected, "file:///synthetic/uninspected.mp4");
    assert!(!inspected_audio_absent(&uninspected), "a file URL alone conveys no inspection proof");
    assert!(declared_audio(&uninspected).is_none());
}

#[test]
fn present_audio_remains_required_even_when_its_codec_is_missing_or_unsupported() {
    let _serial = plx_base::testlock::serial();
    let _globals = Globals::capture();
    let info = MediaInfo {
        tracks: TrackInventory::complete_for_test(vec![audio_row("aac")]).unwrap(),
        audio_codec: "aac".into(), audio_channels: 2, audio_sample_rate: 48_000, ..media()
    };
    let mut ps = admit(&info);
    assert!(!inspected_audio_absent(&ps));
    assert_eq!(declared_audio(&ps).unwrap().name, "AAC");
    let av: serde_json::Value = serde_json::from_str(&build_av_payload(&ps, "H264", "AAC", ENVELOPE_FHD60)).unwrap();
    assert_eq!(av["args"][0]["option"]["needAudio"], true);
    assert_eq!(av["args"][0]["option"]["externalStreamingInfo"]["contents"]["codec"]["audio"], "AAC");
    // A missing/changed scalar must never erase a known track from the admitted inventory.
    for codec in ["", "unknown"] {
        crate::route::set_stream_acodec(&mut ps, codec);
        assert!(!inspected_audio_absent(&ps));
        assert!(declared_audio(&ps).is_none(), "unsupported audio must still refuse the Load");
    }
    // Even a complete empty inventory cannot override a contradictory, nonempty codec.
    let mut ps = admit(&media());
    crate::route::set_stream_acodec(&mut ps, "unknown");
    assert!(!inspected_audio_absent(&ps));
    assert!(declared_audio(&ps).is_none());
}

#[test]
fn unsupported_audio_is_still_rejected_by_external_admission() {
    let _serial = plx_base::testlock::serial();
    let _globals = Globals::capture();
    let mut ps = crate::route::PlaybackSession::IDLE;
    crate::route::reset_player_control_for_test(&ps);
    let info = MediaInfo {
        tracks: TrackInventory::complete_for_test(vec![audio_row("unknown")]).unwrap(),
        audio_codec: "unknown".into(), audio_channels: 2, audio_sample_rate: 48_000, ..media()
    };
    assert_eq!(crate::route::request_external_source(
        &mut ps, &info, crate::ff::ExternalSource::Local("/synthetic/unsupported.mp4".into()),
        "Unsupported", "Local", plx_platform::devcaps::assumed_shared(),
    ), Err(crate::route::ExternalError::UnsupportedAudio));
    assert!(crate::route::external_source(&ps).is_none());
    assert!(!inspected_audio_absent(&ps));
}
