use super::*;

#[test]
fn original_mpeg_clock_survives_without_per_lane_zeroing_or_millisecond_rounding() {
    let sample = SourceTiming::new(Some(900_001), Some(891_001), Some(3_750), 1, 90_000).unwrap();
    assert_eq!(sample.pts, 900_001);
    assert_eq!(sample.dts, Some(891_001));
    assert_eq!(sample.duration, Some(3_750));
    assert_eq!(sample.time_base, (1, 90_000));
}

#[test]
fn decode_order_keeps_b_frame_presentation_reordering() {
    let mut capture = Capture::new(Lane::Video, None);
    for (pts, dts) in [(90_000, 82_500), (101_250, 86_250), (93_750, 90_000)] {
        capture.push(SourceTiming::new(Some(pts), Some(dts), Some(3_750), 1, 90_000).unwrap()).unwrap();
    }
    assert_eq!(capture.timing.iter().map(|s| s.pts).collect::<Vec<_>>(), [90_000, 101_250, 93_750]);
}

#[test]
fn missing_presentation_clock_is_not_manufactured_from_decode_clock_or_zero() {
    assert!(SourceTiming::new(None, Some(90_000), Some(1_920), 1, 90_000).is_err());
    assert!(SourceTiming::new(None, None, Some(1_920), 1, 90_000).is_err());
    assert!(SourceTiming::new(Some(90_000), None, None, 0, 90_000).is_err());
    assert!(SourceTiming::new(Some(90_000), None, None, 1, 0).is_err());
}

#[test]
fn raw_timing_metadata_has_an_independent_bounded_budget() {
    let mut capture = Capture::new(Lane::Audio, None);
    let sample = SourceTiming::new(Some(90_000), Some(90_000), Some(1_920), 1, 90_000).unwrap();
    for _ in 0..SAMPLE_LIMIT { capture.push(sample).unwrap(); }
    assert!(capture.push(sample).is_err());
    assert_eq!(capture.timing.len(), SAMPLE_LIMIT);
}

#[test]
fn late_stream_shape_changes_are_rejected_before_publishing_samples() {
    let old = Binding::Video { width: 160, height: 90 };
    let new = Binding::Video { width: 320, height: 180 };
    let mut capture = Capture::new(Lane::Video, Some(&old));
    assert!(capture.accept_probe(new.clone()).is_err());
    capture.accept_probe(old.clone()).unwrap();
    capture.push(SourceTiming::new(Some(90_000), Some(90_000), Some(3_750), 1, 90_000).unwrap()).unwrap();
    assert!(capture.validate_end(new).is_err());
    assert!(capture.validate_end(old).is_ok());
}
