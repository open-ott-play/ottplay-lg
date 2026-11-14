use super::*;

fn output(raw: i64, local: i64, duration: i64) -> HlsSegmentOutput {
    HlsSegmentOutput { resident: None,
        transport_timing: Some((raw, local, local + duration, Some(local + duration))),
        webvtt_clock: Some(hls_webvtt::Clock { pts_90k: raw / 1_000_000 * 90, presentation_ns: local }),
        observed: None,
        aus: vec![
            HlsAu { data: vec![], pts_ns: local, key: 1, es: 1 },
            // B-frame presentation order must survive the common shift.
            HlsAu { data: vec![], pts_ns: local + 80_000_000, key: 0, es: 1 },
            HlsAu { data: vec![], pts_ns: local + 40_000_000, key: 0, es: 1 },
            HlsAu { data: vec![], pts_ns: local + 12_000_000, key: 1, es: 2 },
        ],
        transfer: SegmentTransfer { bytes: 1, active_us: 1, total_us: 1, audio_expected: true },
        video_width: 160, video_height: 90,
        video_tail_ns: local + 2_000_000_000,
        audio_tail_ns: Some(local + 2_000_000_000),
    }
}

#[test]
fn hls_transport_rounded_extinf_cannot_accumulate_mp2_av_drift() {
    // 83/85 MP2 frames at 48kHz span 1.992/2.040s; a playlist rounds to 2s.
    // The old MP2-only persistent map disagreed with video by 2/10s after 250 segments.
    for duration in [1_992_000_000, 2_040_000_000] {
        let mut committed = crate::hls::TransportClock::default();
        for n in 0..=250 {
            let mut candidate = committed;
            let mut segment = output(10_000_000_000 + n * duration, n * 2_000_000_000, duration);
            segment.remap_transport(&mut candidate).unwrap();
            assert_eq!(segment.aus[0].pts_ns, n * duration);
            assert_eq!(segment.aus[3].pts_ns - segment.aus[0].pts_ns, 12_000_000);
            assert_eq!(segment.aus[1].pts_ns - segment.aus[2].pts_ns, 40_000_000);
            assert_eq!(segment.webvtt_clock.unwrap().presentation_ns, segment.aus[0].pts_ns);
            assert_eq!(segment.video_tail_ns, (n + 1) * duration);
            assert_eq!(segment.audio_tail_ns, Some((n + 1) * duration));
            committed = candidate; // production commits only after complete feeding
        }
    }
}

#[test]
fn hls_transport_candidate_abort_gap_overlap_wrap_and_epoch_restart() {
    let wrap = 95_443_717_688_889i64;
    let mut committed = crate::hls::TransportClock::default();
    output(wrap - 1_000_000_000, 0, 1_000_000_000).remap_transport(&mut committed).unwrap();
    let before = committed;
    let mut rejected = committed;
    output(1_000_000_000, 1_000_000_000, 1_000_000_000).remap_transport(&mut rejected).unwrap();
    assert_eq!(committed, before, "a rejected candidate cannot publish its clock");
    let mut wrapped = output(100_000_000, 1_000_000_000, 1_000_000_000);
    wrapped.remap_transport(&mut committed).unwrap();
    assert_eq!(wrapped.aus[0].pts_ns, 1_100_000_000, "preserve the actual 100ms gap");
    let mut overlap = output(1_050_000_000, 2_000_000_000, 1_000_000_000);
    overlap.remap_transport(&mut committed).unwrap();
    assert_eq!(overlap.aus[0].pts_ns, 2_050_000_000, "do not hide a real 50ms overlap");
    let end = committed.end_ns();
    let mut epoch = crate::hls::TransportClock::restart_at(end);
    let mut restarted = output(600_000_000_000, 99_000_000_000, 1_000_000_000);
    restarted.remap_transport(&mut epoch).unwrap();
    assert_eq!(Some(restarted.aus[0].pts_ns), end, "epoch joins the actual sample end, not EXTINF");
    let mut fresh = output(600_000_000_000, 0, 1_000_000_000);
    fresh.remap_transport(&mut crate::hls::TransportClock::default()).unwrap();
    assert_eq!(fresh.aus[0].pts_ns, 0, "new worker/seek has no old epoch authority");
}

#[test]
fn hls_transport_renews_wrap_reference_for_long_lived_channels() {
    let wrap = 95_443_717_688_889i64;
    let step = 20_000_000_000;
    let mut clock = crate::hls::TransportClock::default();
    for n in 0..15_000 {
        let raw = (n * step) % wrap;
        let mut segment = output(raw, n * 2_000_000_000, 1_000_000_000);
        segment.remap_transport(&mut clock).unwrap();
        assert_eq!(segment.aus[0].pts_ns, n * step);
    }
}

#[test]
fn hls_transport_repeated_backwards_and_unannounced_large_gaps_fail_closed() {
    let mut committed = crate::hls::TransportClock::default();
    output(100_000_000_000, 0, 2_000_000_000).remap_transport(&mut committed).unwrap();
    for raw in [100_000_000_000, 99_000_000_000, 1_000_000_000, 132_000_000_001] {
        let before = committed;
        assert!(output(raw, 2_000_000_000, 2_000_000_000).remap_transport(&mut committed).is_err());
        assert_eq!(committed, before, "invalid timing cannot mutate the committed reference");
    }
    output(132_000_000_000, 2_000_000_000, 2_000_000_000).remap_transport(&mut committed).unwrap();
}

#[test]
fn hls_transport_segment_overlap_is_bounded_without_clamping_samples() {
    let mut committed = crate::hls::TransportClock::default();
    output(100_000_000_000, 0, 2_000_000_000).remap_transport(&mut committed).unwrap();
    let before = committed;
    assert!(output(100_999_999_999, 2_000_000_000, 2_000_000_000)
        .remap_transport(&mut committed).is_err());
    assert_eq!(committed, before);
    let mut boundary = output(101_000_000_000, 2_000_000_000, 2_000_000_000);
    boundary.remap_transport(&mut committed).unwrap();
    assert_eq!(boundary.aus[0].pts_ns, 1_000_000_000);
    assert_eq!(boundary.aus[1].pts_ns - boundary.aus[2].pts_ns, 40_000_000);
}
