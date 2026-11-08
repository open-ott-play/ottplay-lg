use super::*;

fn master() -> String {
    String::from("#EXTM3U\n#EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID=\"sound\",NAME=\"English\",URI=\"en.m3u8\"\n#EXT-X-MEDIA:TYPE=SUBTITLES,GROUP-ID=\"other\",NAME=\"Other\",URI=\"other.m3u8\"\n#EXT-X-MEDIA:TYPE=SUBTITLES,GROUP-ID=\"subs\",NAME=\"English\",LANGUAGE=\"en\",URI=\"en-vtt.m3u8\"\n#EXT-X-MEDIA:TYPE=SUBTITLES,GROUP-ID=\"subs\",NAME=\"Spanish\",LANGUAGE=\"es\",URI=\"es-vtt.m3u8\"\n#EXT-X-STREAM-INF:BANDWIDTH=1000,AUDIO=\"sound\",SUBTITLES=\"subs\"\nvideo.m3u8\n")
}
#[test]
fn hls_subtitle_candidates_are_selected_group_only_with_separate_ids() {
    let got = parse_master("https://example.invalid/master.m3u8", &master()).unwrap();
    assert_eq!(got.renditions.len(), 1);
    assert_eq!(got.default, RenditionId(1));
    assert_eq!(got.subtitles.len(), 2);
    assert_eq!(got.subtitles[0].id, HlsSubtitleId(2));
    assert_eq!(got.subtitles[1].language, "es");
    assert_eq!(got.subtitles[0].reference.as_deref().unwrap(), "en-vtt.m3u8");
    let playback = HlsPlayback::fixture_with_subtitles_for_test();
    let ids: Vec<_> = playback.subtitle_rows().map(|row| row.0).collect();
    assert_eq!(ids.len(), 2);
    assert!(ids.into_iter().all(|id| playback.has_subtitle(id)));
    assert!(!playback.has_subtitle(HlsSubtitleId(999)));
}
#[test]
fn hls_subtitle_bad_candidate_uri_does_not_reject_audio_video() {
    for bad in [master().replace("URI=\"en-vtt.m3u8\"", "URI=\"http://example.invalid/plain.m3u8\""),
        master().replace(",URI=\"en-vtt.m3u8\"", "")] {
        let got = parse_master("https://example.invalid/master.m3u8", &bad).unwrap();
        assert_eq!(got.renditions.len(), 1);
        assert!(got.subtitles[0].reference.is_err());
        assert!(got.subtitles[1].reference.is_ok());
        let mut playback = HlsPlayback::fixture_with_subtitles_for_test();
        Arc::get_mut(&mut playback.presentation).unwrap().subtitles = got.subtitles;
        assert_eq!(playback.subtitle_rows().count(), 1);
        assert_eq!(playback.subtitle_rows().next().unwrap().2, "es");
    }
}
#[test]
fn hls_subtitle_missing_clock_cannot_be_fabricated_from_fixture_metadata() {
    let mut playback = HlsPlayback::fixture_with_subtitles_for_test();
    let id = playback.subtitle_rows().next().unwrap().0;
    Arc::get_mut(&mut playback.presentation).unwrap().video_clock = None;
    assert_eq!(playback.subtitle_rows().count(), 0);
    assert!(!playback.has_subtitle(id));
    assert_eq!(playback.prepare_subtitle(id, Arc::new(AtomicBool::new(false))).unwrap_err(),
        "HLS subtitle is unavailable in this presentation");
}
#[test]
fn hls_subtitle_video_anchor_is_derived_and_reused_by_actual_lane_clock() {
    use super::super::hls_join::LaneClock;
    use super::super::hls_rendition::{SourceTiming, TimedLaneSegment, TimedSample};
    let raw = |pts| TimedLaneSegment { binding: Binding::Video { width: 160, height: 90 },
        samples: vec![TimedSample { data: Vec::new(), key: true,
            timing: SourceTiming { pts, dts: Some(pts), duration: Some(90_000), time_base: (1, 90_000) } }],
        transfer: SegmentTransfer { bytes: 0, active_us: 0, total_us: 0, audio_expected: false } };
    let mut inspected = LaneClock::default();
    inspected.map(raw(900_000), 1_000_000_000_000, 1_000_000_000_000, Some(1)).unwrap();
    let anchor = inspected.webvtt_clock().unwrap();
    assert_eq!(anchor.pts_90k, 900_000);
    assert_eq!(anchor.presentation_ns, 0);
    let mut playback = inspected.clone();
    playback.restart();
    assert_eq!(playback.map(raw(900_000), 1_000_000_000_000, 1_000_000_000_000, Some(1)).unwrap().start_ns, 0);
    playback.restart();
    assert_eq!(playback.map(raw(990_000), 1_001_000_000_000, 1_000_000_000_000, Some(1)).unwrap().start_ns, 1_000_000_000);
    assert_eq!(playback.webvtt_clock(), Some(anchor));
    playback.restart();
    assert!(playback.map(raw(900_001), 1_000_000_000_000, 1_000_000_000_000, Some(1)).is_ok());
    playback.restart();
    assert!(playback.map(raw(990_000), 1_000_000_000_000, 1_000_000_000_000, Some(1)).is_err());
}
