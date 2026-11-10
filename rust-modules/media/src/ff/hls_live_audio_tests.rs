use super::*;
fn window(sequence: u64, epoch: u64, count: usize) -> Window {
    let mut text = format!("#EXTM3U\n#EXT-X-TARGETDURATION:1\n#EXT-X-MEDIA-SEQUENCE:{sequence}\n#EXT-X-DISCONTINUITY-SEQUENCE:{epoch}\n");
    for n in 0..count {
        text.push_str(&format!("#EXT-X-PROGRAM-DATE-TIME:2026-01-01T00:00:{:02}.000000000Z\n#EXTINF:1,\n{}.ts\n", sequence+n as u64, sequence+n as u64));
    }
    parse("https://example.invalid/live.m3u8", &text).unwrap()
}
#[test]
fn paired_live_refresh_accepts_overlap_but_rejects_mutation_rollback_and_skipped_cursor() {
    let first = window(0, 4, 3);
    let next = window(1, 4, 3);
    assert!(refresh_valid(&first, &next, 3).is_ok());
    assert!(refresh_valid(&next, &first, 3).is_err());
    assert!(refresh_valid(&first, &window(4, 4, 2), 3).is_err());
    let mut changed = next.clone();
    changed.epochs[0] += 1;
    assert!(refresh_valid(&first, &changed, 3).is_err());
}
#[test]
fn paired_live_requires_matching_epochs_precise_explicit_pdt_and_real_overlap() {
    let video = window(0, 4, 3);
    assert!(validate_pair(&video, &window(0, 4, 3)).is_ok());
    assert!(validate_pair(&video, &window(0, 5, 3)).is_err());
    assert!(validate_pair(&video, &window(4, 4, 3)).is_err());
    let mut coarse = video.clone();
    for segment in &mut coarse.media.segments { segment.explicit_precision_ns = Some(1_000_000_000); }
    assert!(validate_pair(&coarse, &window(0, 4, 3)).is_err());
}
#[test]
fn paired_live_parser_bounds_work_and_refuses_epoch_without_pdt() {
    let text = "#EXTM3U\n#EXT-X-TARGETDURATION:1\n#EXT-X-PROGRAM-DATE-TIME:2026-01-01T00:00:00.000Z\n#EXTINF:1,\na.ts\n#EXT-X-DISCONTINUITY\n#EXTINF:1,\nb.ts\n";
    assert_eq!(parse("https://example.invalid/live.m3u8", text).err().unwrap(), "Live HLS epoch is missing program date time");
    let text = format!("#EXTM3U\n#EXT-X-TARGETDURATION:1\n{}", "#EXTINF:1,\na.ts\n".repeat(257));
    assert!(parse("https://example.invalid/live.m3u8", &text).is_err());
}

#[test]
fn paired_live_near_edge_rebase_preserves_actual_audio_video_and_subtitle_offset() {
    let mut output = HlsSegmentOutput { transport_timing: None,
        aus: vec![HlsAu { data: vec![1], pts_ns: 20_005_000_000, key: 1, es: 1 },
            HlsAu { data: vec![2], pts_ns: 19_990_000_000, key: 0, es: 2 }],
        webvtt_clock: Some(crate::ff::hls_webvtt::Clock { pts_90k: 900_000, presentation_ns: 0 }),
        observed: None,
        video_tail_ns: 21_005_000_000, audio_tail_ns: Some(21_010_000_000),
        video_width: 160, video_height: 90,
        transfer: SegmentTransfer { bytes: 2, active_us: 0, total_us: 0, audio_expected: true },
    };
    rebase_output(&mut output, 19_990_000_000).unwrap();
    assert_eq!(output.aus.iter().map(|au| au.pts_ns).collect::<Vec<_>>(), vec![15_000_000, 0]);
    assert_eq!(output.video_tail_ns, 1_015_000_000);
    assert_eq!(output.audio_tail_ns, Some(1_020_000_000));
    let subtitle_clock = output.webvtt_clock.unwrap();
    assert_eq!(subtitle_clock.presentation_ns, -19_990_000_000);
    // MPEGTS 2745450 is half a second after this actual video sample.
    let caption_ns = subtitle_clock.presentation_ns
        + (2_745_450-subtitle_clock.pts_90k)*1_000_000_000/90_000;
    assert_eq!(caption_ns,output.aus[0].pts_ns+500_000_000);
}

#[path = "hls_live_audio_http_tests.rs"]
mod http;

#[test]
fn paired_live_advancing_wrong_audio_epoch_cannot_extend_acquisition_deadline() {
    use std::io::{Read, Write};
    let _serial = plx_base::testlock::serial();
    assert!(plx_net::net::global_init());
    assert!(crate::curlio::available());
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let url = format!("http://{}/audio.m3u8", listener.local_addr().unwrap());
    let first = "#EXTM3U\n#EXT-X-TARGETDURATION:1\n#EXT-X-DISCONTINUITY-SEQUENCE:4\n#EXT-X-PROGRAM-DATE-TIME:2026-01-01T00:00:00.000000000Z\n#EXTINF:1,\na.ts\n";
    let advanced = format!("{first}#EXT-X-PROGRAM-DATE-TIME:2026-01-01T00:00:01.000000000Z\n#EXTINF:1,\nb.ts\n");
    let server = std::thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(1);
        let mut socket = loop {
            if let Ok((socket,_)) = listener.accept() { break socket; }
            assert!(Instant::now() < deadline,"client did not request the advancing publisher");
            std::thread::sleep(Duration::from_millis(1));
        };
        socket.set_nonblocking(false).unwrap();
        socket.set_read_timeout(Some(Duration::from_secs(1))).unwrap();
        socket.set_write_timeout(Some(Duration::from_secs(1))).unwrap();
        let mut request = [0;8192]; let _ = socket.read(&mut request);
        let header = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",advanced.len());
        socket.write_all(header.as_bytes()).unwrap(); socket.write_all(advanced.as_bytes()).unwrap();
    });
    let mut refresh = Refresh::new(parse(&url,first).unwrap());
    let before = refresh.progress;
    let previous_seek = SHARED.seek_to_ns.swap(-1,Ordering::AcqRel);
    let mut queue = crate::aq::aq_new(4096);
    let started = Instant::now();
    let result = refresh.ensure_epoch_until(5,0,&mut *queue,started+Duration::from_millis(100));
    crate::aq::aq_destroy(&mut *queue);
    SHARED.seek_to_ns.store(previous_seek,Ordering::Release);
    server.join().unwrap();
    assert!(matches!(result,Err(HlsExit::Failed("Live HLS matching audio epoch timed out"))));
    assert!(refresh.progress > before,"a real sequence advance occurred inside the wrong epoch");
    assert_eq!(refresh.window.last_sequence(),1);
    assert!(started.elapsed() < Duration::from_secs(1),"publication progress cannot reset the fixed epoch budget");
}

#[test]
fn paired_live_declared_epoch_reset_keeps_joined_native_timeline_contiguous() {
    use crate::ff::hls_rendition::{TimedLaneSegment, TimedSample, SourceTiming};
    fn raw(lane: Lane, pts: i64, count: i64, step: i64) -> TimedLaneSegment {
        let fixture = HlsPlayback::fixture_for_test();
        TimedLaneSegment {
            binding: if lane == Lane::Audio { fixture.audio_binding.clone() }
                else { Binding::Video { width: 160, height: 90 } },
            samples: (0..count).map(|n| TimedSample { data: vec![1], key: n == 0,
                timing: SourceTiming { pts: pts+n*step, dts: Some(pts+n*step), duration: Some(step), time_base: (1,90_000) } }).collect(),
            transfer: SegmentTransfer { bytes: count as u64, active_us: 0, total_us: 0, audio_expected: true },
        }
    }
    let mut previous_end = 20_000_000_000;
    let offset = previous_end;
    for (video_pts, audio_pts, pdt) in [(900_000,70_000,50_000_000_000),(45_000,1_000,800_000_000_000)] {
        // Explicit matching epoch PDT anchors a fresh encoder clock after the
        // declared reset. Raw PTS and wall-clock epochs may both change.
        let origin = pdt-previous_end;
        let mut video_clock = LaneClock::default();
        let frames = video_clock.map(raw(Lane::Video,video_pts,2,45_000),pdt,origin,Some(1)).unwrap();
        let audio = LaneClock::default().map(raw(Lane::Audio,audio_pts,4,22_500),pdt,origin,Some(1)).unwrap();
        let mut join = JoinCursor::new(previous_end).unwrap(); join.push_audio(audio).unwrap();
        let mut output = join.join(frames).unwrap();
        previous_end = output.video_tail_ns;
        output.webvtt_clock = video_clock.webvtt_clock();
        rebase_output(&mut output,offset).unwrap();
        let start = output.video_tail_ns-1_000_000_000;
        assert_eq!(output.aus.iter().map(|au| au.pts_ns).min(),Some(start));
        assert_eq!(output.webvtt_clock.unwrap().presentation_ns,start);
        assert_eq!(output.audio_tail_ns,Some(output.video_tail_ns));
    }
    assert_eq!(previous_end-offset,2_000_000_000);
}
