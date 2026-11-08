use super::*;

fn transfer() -> SegmentTransfer {
    SegmentTransfer { bytes: 1, active_us: 1, total_us: 1, audio_expected: true }
}
fn raw_lane(kind: Lane, start: i64, count: usize, step: i64) -> TimedLaneSegment {
    let binding = if kind == Lane::Video {
        Binding::Video { width: 160, height: 90 }
    } else {
        Binding::Audio(super::super::inspect::InspectedTrack {
            kind: super::super::inspect::TrackKind::Audio, stream_index: 0, ordinal: 0,
            codec: "aac".into(), language: "eng".into(), title: String::new(), channels: 2,
            sample_rate: 48000, bit_depth: 0, decoder_available: true,
            disposition_known: false, default: false, forced: false,
        })
    };
    let samples = (0..count).map(|n| TimedSample {
        data: vec![n as u8], key: n == 0,
        timing: SourceTiming { pts: start + n as i64 * step, dts: Some(start + n as i64 * step),
            duration: Some(step), time_base: (1, 1000) },
    }).collect();
    TimedLaneSegment { binding, samples, transfer: transfer() }
}
fn lane(kind: Lane, start: i64, count: usize, step: i64, anchor: i64, origin: i64) -> MappedSegment {
    LaneClock::default().map(raw_lane(kind, start, count, step), anchor, origin, Some(1)).unwrap()
}

#[test]
fn one_video_interval_joins_multiple_audio_segments_and_keeps_tail() {
    let mut join = JoinCursor::new(0).unwrap();
    let mut clock = LaneClock::default();
    join.push_audio(clock.map(raw_lane(Lane::Audio, 70_000, 3, 20), 1_000_000_000, 1_000_000_000, Some(1)).unwrap()).unwrap();
    join.push_audio(clock.map(raw_lane(Lane::Audio, 70_060, 3, 20), 1_060_000_000, 1_000_000_000, Some(1)).unwrap()).unwrap();
    let video = lane(Lane::Video, 900, 2, 50, 1_000_000_000, 1_000_000_000);
    assert!(join.covers(&video));
    let first = join.join(video).unwrap();
    assert_eq!(first.audio_tail_ns, Some(100_000_000));
    assert_eq!(first.aus.iter().filter(|a| a.es == 2).count(), 5);
    assert_eq!(join.audio.len(), 1);
    join.push_audio(clock.map(raw_lane(Lane::Audio, 70_120, 4, 20), 1_120_000_000, 1_000_000_000, Some(1)).unwrap()).unwrap();
    let second = join.join(lane(Lane::Video, 10, 2, 50, 1_100_000_000, 1_000_000_000)).unwrap();
    assert_eq!(second.aus.iter().filter(|a| a.es == 2).count(), 5);
    assert_eq!(second.audio_tail_ns, Some(200_000_000));
}

#[test]
fn missing_or_overlapping_audio_is_never_replaced_by_another_lane() {
    let mut join = JoinCursor::new(0).unwrap();
    join.push_audio(lane(Lane::Audio, 0, 2, 20, 0, 0)).unwrap();
    assert!(!join.covers(&lane(Lane::Video, 0, 2, 50, 0, 0)));
    assert!(join.push_audio(lane(Lane::Audio, 0, 2, 20, 60_000_000, 0)).is_err());
    assert!(join.push_audio(lane(Lane::Audio, 0, 2, 20, 20_000_000, 0)).is_err());
}

#[test]
fn mapping_keeps_b_frame_decode_order_and_distinct_original_epochs() {
    let mut video = lane(Lane::Video, 1000, 3, 40, 5_000_000_000, 5_000_000_000);
    // Swap presentation order while keeping monotonically increasing DTS.
    video.samples[1].au.pts_ns = 80_000_000;
    video.samples[1].end_ns = 120_000_000;
    video.samples[2].au.pts_ns = 40_000_000;
    video.samples[2].end_ns = 80_000_000;
    // The join never sorts video by presentation timestamp.
    let mut join = JoinCursor::new(0).unwrap();
    join.push_audio(lane(Lane::Audio, 900_000, 6, 20, 5_000_000_000, 5_000_000_000)).unwrap();
    let output = join.join(video).unwrap();
    assert_eq!(output.aus.iter().filter(|a| a.es == 1).map(|a| a.pts_ns).collect::<Vec<_>>(),
        [0, 80_000_000, 40_000_000]);
}

#[test]
fn negative_origin_is_rejected_before_arithmetic() {
    assert!(JoinCursor::new(i64::MIN).is_err());
}
#[test]
fn crossing_audio_frame_is_not_repeated_and_seek_keeps_preroll() {
    let mut join = JoinCursor::new(10_000_000).unwrap();
    join.push_audio(lane(Lane::Audio, 0, 6, 20, 0, 0)).unwrap();
    let first = join.join(lane(Lane::Video, 0, 1, 45, 10_000_000, 0)).unwrap();
    let points: Vec<_> = first.aus.iter().filter(|a| a.es == 2).map(|a| a.pts_ns).collect();
    assert_eq!(points, [0, 20_000_000, 40_000_000]);
    assert_eq!(first.audio_tail_ns, Some(60_000_000));
    let second = join.join(lane(Lane::Video, 0, 1, 45, 55_000_000, 0)).unwrap();
    assert_eq!(second.aus.iter().filter(|a| a.es == 2).map(|a| a.pts_ns).collect::<Vec<_>>(),
        [60_000_000, 80_000_000]);
}

#[test]
fn inherited_pdt_cannot_hide_raw_timestamp_gap_duplicate_or_reset() {
    let mut clock = LaneClock::default();
    let first = clock.map(raw_lane(Lane::Audio, 70_000, 50, 20), 5_000_000_000, 5_000_000_000, Some(1)).unwrap();
    assert_eq!(first.end_ns, 1_000_000_000);
    // The playlist claims contiguous one-second segments. All of these raw
    // timelines disagree: skipped second, duplicate segment, and encoder reset.
    for raw_start in [72_000, 70_000, 0] {
        assert!(clock.map(raw_lane(Lane::Audio, raw_start, 50, 20), 6_000_000_000, 5_000_000_000, None).is_err());
    }
    // A failed candidate does not move the retained clock.
    let valid = clock.map(raw_lane(Lane::Audio, 71_000, 50, 20), 6_000_000_000, 5_000_000_000, Some(1)).unwrap();
    assert_eq!(valid.start_ns, 1_000_000_000);
    assert_eq!(valid.end_ns, 2_000_000_000);
}

#[test]
fn retained_clock_rejects_timebase_changes_and_does_not_snap_pdt_rounding() {
    let mut clock = LaneClock::default();
    clock.map(raw_lane(Lane::Video, 900_000, 25, 40), 0, 0, Some(1)).unwrap();
    let mut changed = raw_lane(Lane::Video, 900_000, 25, 40);
    for sample in &mut changed.samples { sample.timing.time_base = (1, 90_000); }
    assert!(clock.map(changed, 1_000_000_000, 0, None).is_err());
    let rounded = clock.map(raw_lane(Lane::Video, 901_000, 25, 40), 1_000_500_000, 0, Some(1)).unwrap();
    assert_eq!(rounded.start_ns, 1_000_000_000);
    assert_eq!(rounded.samples[0].au.pts_ns, 1_000_000_000);
}

#[test]
fn rounded_extinf_estimates_do_not_accumulate_into_the_raw_clock() {
    let mut clock = LaneClock::default();
    clock.map(raw_lane(Lane::Audio, 70_000, 3, 20), 0, 0, Some(1)).unwrap();
    for index in 1..200 {
        // Declared 59ms versus real 60ms: the navigation estimate drifts 199ms,
        // but the raw clock remains continuous and must not be rebased or fail.
        let segment = clock.map(raw_lane(Lane::Audio, 70_000 + index * 60, 3, 20),
            index * 59_000_000, 0, None).unwrap();
        assert_eq!(segment.start_ns, index * 60_000_000);
    }
}

#[test]
fn seek_restarts_continuity_but_keeps_the_original_clock_anchor() {
    let mut clock = LaneClock::default();
    clock.map(raw_lane(Lane::Video, 90_000, 25, 40), 5_000_000_000, 5_000_000_000, Some(1)).unwrap();
    clock.restart();
    let seek = clock.map(raw_lane(Lane::Video, 95_000, 25, 40), 9_900_000_000, 5_000_000_000, None).unwrap();
    assert_eq!(seek.start_ns, 5_000_000_000);
    assert_eq!(seek.end_ns, 6_000_000_000);
    assert!(LaneClock::default().map(raw_lane(Lane::Video, 95_000, 25, 40), 0, 0, None).is_err());
}

#[test]
fn explicit_date_is_checked_with_its_resolution_without_rebasing() {
    let mut clock = LaneClock::default();
    clock.map(raw_lane(Lane::Video, 1000, 30, 40), 0, 0, Some(1)).unwrap();
    assert!(clock.map(raw_lane(Lane::Video, 2200, 30, 40), 4_000_000_000, 0, Some(1_000_000)).is_err());
    let rounded = clock.map(raw_lane(Lane::Video, 2200, 30, 40), 1_000_000_000, 0, Some(1_000_000_000)).unwrap();
    assert_eq!(rounded.start_ns, 1_200_000_000);
}

#[test]
fn coarse_initial_pdt_remains_uncertain_when_later_pdt_is_precise() {
    let mut clock = LaneClock::default();
    clock.map(raw_lane(Lane::Audio, 1000, 50, 20), 0, 0, Some(1_000_000_000)).unwrap();
    let next = clock.map(raw_lane(Lane::Audio, 2000, 50, 20), 1_400_000_000, 0, Some(1_000_000)).unwrap();
    // Preserve original raw mapping; a more precise tag never shifts queued AUs.
    assert_eq!(next.start_ns, 1_000_000_000);
    assert!(clock.map(raw_lane(Lane::Audio, 3000, 50, 20), 4_000_000_000, 0, Some(1_000_000)).is_err());
}
