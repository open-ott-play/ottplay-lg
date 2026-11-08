use super::*;

fn packet(pts: Option<i64>, dts: Option<i64>, duration: Option<i64>) -> AudioPacketTiming {
    AudioPacketTiming { pts, dts, duration, time_base: (1, 90_000), adts: None }
}

#[test]
fn leading_audio_holes_backfill_from_real_pts_in_original_ticks() {
    let raw = [packet(None, None, Some(1_921)), packet(None, Some(898_001), Some(1_920)),
        packet(Some(901_841), Some(901_700), Some(1_920))];
    let timing = resolve_audio_timing(&raw).unwrap();
    assert_eq!(timing.iter().map(|s| s.pts).collect::<Vec<_>>(), [898_000, 899_921, 901_841]);
    assert_eq!(timing.iter().map(|s| s.dts).collect::<Vec<_>>(), [None, Some(898_001), Some(901_700)]);
    assert!(timing.iter().all(|s| s.time_base == (1, 90_000)));
    assert_eq!(timing.iter().map(|s| s.duration).collect::<Vec<_>>(), [Some(1_921), Some(1_920), Some(1_920)]);
}

#[test]
fn audio_holes_do_not_rewrite_explicit_gaps_or_overlaps() {
    for next_anchor in [910_123, 901_000] {
        let raw = [packet(Some(900_001), Some(900_000), Some(1_920)),
            packet(None, None, Some(1_920)), packet(Some(next_anchor), Some(next_anchor - 1), Some(2_089)),
            packet(None, None, None)];
        let timing = resolve_audio_timing(&raw).unwrap();
        assert_eq!(timing.iter().map(|s| s.pts).collect::<Vec<_>>(),
            [900_001, 901_921, next_anchor, next_anchor + 2_089]);
        assert_eq!(timing[2].dts, Some(next_anchor - 1));
        assert_eq!(timing[3].duration, None);
    }
}

#[test]
fn wholly_unanchored_audio_never_uses_dts_or_zero_as_pts() {
    for dts in [None, Some(900_001)] {
        assert!(resolve_audio_timing(&[packet(None, dts, Some(1_920)),
            packet(None, Some(901_921), Some(1_920))]).is_err());
    }
    assert!(resolve_audio_timing(&[]).is_err());
}

#[test]
fn missing_audio_pts_requires_each_duration_used_by_its_anchor() {
    for duration in [None, Some(0), Some(-1)] {
        assert!(resolve_audio_timing(&[packet(None, None, duration),
            packet(Some(900_001), None, Some(1_920))]).is_err());
        assert!(resolve_audio_timing(&[packet(Some(900_001), None, duration),
            packet(None, None, Some(1_920))]).is_err());
    }
    let raw = [packet(Some(900_001), None, None), packet(Some(901_921), None, None)];
    assert_eq!(resolve_audio_timing(&raw).unwrap().iter().map(|s| s.pts).collect::<Vec<_>>(),
        [900_001, 901_921]);
}

#[test]
fn audio_tick_recovery_rejects_timebase_changes_and_overflow() {
    for time_base in [(0, 90_000), (1, 0), (-1, 90_000), (1, 48_000)] {
        let mut next = packet(None, None, Some(1_920)); next.time_base = time_base;
        assert!(resolve_audio_timing(&[packet(Some(900_001), None, Some(1_920)), next]).is_err());
    }
    assert!(resolve_audio_timing(&[packet(None, None, Some(2)),
        packet(Some(i64::MIN + 1), None, Some(1))]).is_err());
    assert!(resolve_audio_timing(&[packet(Some(i64::MAX), None, Some(1)),
        packet(None, None, Some(1))]).is_err());
}

#[test]
fn raw_audio_capture_remains_bounded_before_pts_recovery() {
    let mut capture = Capture::new(Lane::Audio, None);
    for _ in 0..SAMPLE_LIMIT { capture.push_audio(packet(None, None, Some(1_920))).unwrap(); }
    assert!(capture.push_audio(packet(Some(900_001), None, Some(1_920))).is_err());
    assert_eq!(capture.audio_timing.len(), SAMPLE_LIMIT);
    assert!(capture.timing.is_empty(), "unanchored audio must not become publishable timing");
}

#[test]
fn video_missing_pts_stays_rejected_and_b_frame_order_stays_intact() {
    assert!(SourceTiming::new(None, Some(900_001), Some(3_750), 1, 90_000).is_err());
    let mut capture = Capture::new(Lane::Video, None);
    for (pts, dts) in [(90_000, 82_500), (101_250, 86_250), (93_750, 90_000)] {
        capture.push(SourceTiming::new(Some(pts), Some(dts), Some(3_750), 1, 90_000).unwrap()).unwrap();
    }
    assert_eq!(capture.timing.iter().map(|s| (s.pts, s.dts)).collect::<Vec<_>>(),
        [(90_000, Some(82_500)), (101_250, Some(86_250)), (93_750, Some(90_000))]);
    assert!(capture.audio_timing.is_empty());
}

#[test]
fn fractional_nanosecond_audio_ticks_never_round_through_another_clock() {
    let mut raw = [packet(None, None, Some(1_024)),
        packet(Some(1_234_567), Some(1_234_560), Some(1_024)),
        packet(None, None, Some(1_024))];
    for sample in &mut raw { sample.time_base = (1, 44_100); }
    let timing = resolve_audio_timing(&raw).unwrap();
    assert_eq!(timing.iter().map(|s| s.pts).collect::<Vec<_>>(), [1_233_543, 1_234_567, 1_235_591]);
    assert!(timing.iter().all(|s| s.time_base == (1, 44_100) && s.duration == Some(1_024)));
    assert_eq!(timing[1].dts, Some(1_234_560));
}
