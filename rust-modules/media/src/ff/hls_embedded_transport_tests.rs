//! Actual selected MPEG-TS DVB subtitle packets through the bundled decoder and segment feeder.
use super::*;

#[test]
#[ignore = "requires bundled host FFmpeg and ffmpeg/libx264; synthetic localhost MPEG-TS only"]
fn observed_ts_subtitle_packets_publish_only_after_full_feed_and_caption_failure_preserves_av() {
    let _serial = plx_base::testlock::serial();
    let _globals = crate::route::HlsTrackRequest::test_globals_guard();
    let _ff = super::super::image_subtitle_tests::bind_host_ffmpeg();
    assert!(plx_net::net::global_init());
    crate::curlio::boot();
    let dir = Scratch::new();
    let subtitles = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/mkv_zlib_subs.mkv");
    let ts = dir.0.join("captions.ts");
    entry_fixture_command("ffmpeg", &["-v", "error", "-nostdin", "-y",
        "-i", subtitles.to_str().unwrap(), "-f", "lavfi", "-i", "testsrc2=size=160x90:rate=25",
        "-map", "1:v:0", "-map", "0:s:1", "-t", "2", "-c:v", "libx264",
        "-preset", "ultrafast", "-g", "25", "-bf", "0", "-c:s", "dvbsub",
        "-streamid", "0:256", "-streamid", "1:288", "-mpegts_service_id", "1",
        "-mpegts_transport_stream_id", "1", "-f", "mpegts", ts.to_str().unwrap()], true);
    std::fs::write(dir.0.join("master.m3u8"),
        "#EXTM3U\n#EXT-X-TARGETDURATION:2\n#EXT-X-MEDIA-SEQUENCE:0\n#EXTINF:2,\ncaptions.ts\n#EXT-X-ENDLIST\n").unwrap();
    let http = HttpFixture::start(dir.0.clone());
    let source = format!("{}/master.m3u8", http.url);
    let variant = source.clone();
    SHARED.reset_session();
    let previous_feed = FEED_AUDIO.swap(false, Ordering::Relaxed);
    let info = inspect::http(&source, Arc::new(AtomicBool::new(false))).unwrap();
    let mut session = crate::route::PlaybackSession::IDLE;
    crate::route::reset_player_control_for_test(&session);
    crate::route::request_external_source(&mut session, &info,
        ExternalSource::Hls(source.clone()), "Fixture", "M3U", plx_platform::devcaps::assumed_shared()).unwrap();
    let logical = crate::route::playback_identity(&session).unwrap();
    let native = SHARED.begin_native_session().unwrap();
    let recovery = SHARED.hls_candidate_generation.load(Ordering::Acquire);
    let mut video = crate::aq::aq_new(4 * 1024 * 1024);
    let mut audio = crate::aq::aq_new(128 * 1024);
    let mut hs = plx_net::stream::http_stream_boxed();
    let mut net = HlsNet { hs: &mut *hs, curl: None };
    let mut timeline = crate::hls::SegmentTimeline::default();
    with_observed_hls_worker(native, logical, || {
        for sequence in 0..4 {
            if sequence == 1 {
                // Content time survives a cold-prefix/seek rebase. The engine
                // separately adds pts_shift before sink feed and removes it
                // from position callbacks; captions must not apply it again.
                SHARED.disp_base.store(9_000_000_000, Ordering::Release);
                SHARED.pts_shift.store(3_000_000_000, Ordering::Release);
            }
            let segment = crate::hls::Segment { sequence, duration: Duration::from_secs(2),
                resource: crate::hls::Resource::new(plx_plex::plex::StreamUrl::parse(&http.url).origin,
                    "/captions.ts").unwrap() };
            // Replaying the fixture is an explicit source discontinuity, not a
            // fabricated advancing transport clock or inherited decoder state.
            let transaction = hls_observed_worker::begin_subtitle_segment(&source, &variant, sequence, sequence);
            let mut clock = timeline.begin(segment.duration).unwrap();
            let mut state = H264SegmentState::default();
            let mut output = unsafe { hls_demux_segment_path(&segment,
                HlsSegmentRequest { path: &segment.resource.path, transport: HlsSegmentTransport::External,
                    body_limit: Some(8 * 1024 * 1024), audio: None, rendition: None,
                    video: HlsVideoEntry::External(&mut state), mp2: None },
                &mut clock, &mut *video, &mut net, "", SegmentAcquisition::for_cursor(None)) }.unwrap();
            let inventory = output.observed.as_ref().expect("actual complete PAT/PMT observation").inventory();
            assert_eq!(inventory.subtitles.len(), 1);
            assert_eq!(inventory.subtitles[0].codec, "dvb_subtitle");
            let transport = inventory.transport_identity(inventory.subtitles[0].stream_index).unwrap();
            assert_eq!((transport.transport_stream_id, transport.program_id, transport.pid, transport.stream_type),
                (1, 1, 0x120, 0x06), "the real PMT DVB descriptor binds this subtitle PID");
            let before_count = SHARED.sub_bitmaps.lock().unwrap().len();
            if sequence == 1 { assert_eq!(before_count, 0, "demux success alone cannot publish captions"); }
            let local_shift = output.transport_timing.and_then(|(raw, local, _, _)| local.checked_sub(raw));
            let before = output.aus[0].pts_ns;
            output.remap_transport(&mut crate::hls::TransportClock::default()).unwrap();
            let shift = local_shift.and_then(|local| output.aus[0].pts_ns.checked_sub(before)
                .and_then(|offset| local.checked_add(offset)));
            let first_video = output.aus.iter().find(|au| au.es == 1).unwrap().pts_ns;
            if sequence == 2 {
                // Caption decode failure is deliberately contained in the
                // private transaction; the valid media output stays feedable.
                hls_observed_worker::abort_subtitle_segment();
            }
            if sequence == 3 {
                hls_observed::clear_subtitle_selection();
                crate::player::request_subtitle(-1);
                crate::player::ass_source::retire_selection();
            }
            assert!(hls_feed_external_segment(&output, segment.duration, recovery,
                &mut *video, &mut *audio).unwrap(), "caption state cannot suppress valid A/V feeding");
            assert!(crate::aq::aq_bytes(&mut *video) > 0);
            let scope = hls_observed_worker::scope(&source, &variant, sequence).unwrap();
            let snapshot = hls_observed::commit_observed(scope, sequence, output.observed.take().unwrap()).unwrap();
            transaction.publish(&source, &variant, sequence, sequence, shift, recovery);
            if sequence == 0 {
                let choice = hls_observed::Selection::new(snapshot.inventory(), inspect::TrackKind::Subtitle, 0).unwrap();
                hls_observed::set_subtitle_selection(&snapshot, choice).unwrap();
                crate::player::request_subtitle(0);
            } else if sequence == 1 {
                let cues = SHARED.sub_bitmaps.lock().unwrap();
                assert!(!cues.is_empty(), "the real selected DVB decoder must publish pixels after feeding");
                assert!(cues.iter().all(|cue| cue.start_ns >= 0 && !cue.rects.is_empty()));
                let sink_pts = first_video + SHARED.pts_shift.load(Ordering::Acquire);
                let callback_position = sink_pts - SHARED.pts_shift.load(Ordering::Acquire)
                    + SHARED.disp_base.load(Ordering::Acquire);
                assert!((cues[0].start_ns - callback_position).abs() <= 40_000_000,
                    "the first authored DVB cue and first picture share content time: caption={} callback={callback_position}",
                    cues[0].start_ns);
                drop(cues);
                assert!(crate::player::active_bitmap_key(callback_position + 40_000_000).is_some(),
                    "the renderer must find the caption at the nonzero content playhead");
            } else if sequence == 2 {
                assert!(SHARED.sub_bitmaps.lock().unwrap().is_empty(),
                    "a current failed caption decode retires its old open display after valid A/V feeding");
            } else {
                assert_eq!(SHARED.sub_bitmaps.lock().unwrap().len(), before_count,
                    "failed/retired caption batches never publish into the renderer");
            }
            timeline.commit(clock);
            for queue in [&mut *video, &mut *audio] {
                loop {
                    let node = crate::aq::aq_pop(queue, std::ptr::null_mut());
                    if node.is_null() { break; }
                    unsafe { libc::free(node.cast()); }
                }
            }
        }
    });
    hls_observed::clear_subtitle_selection();
    hls_observed::retire_observed(native);
    SHARED.retire_native_session(native);
    drop(net);
    plx_net::stream::http_close(&mut *hs);
    crate::aq::aq_destroy(&mut *video); crate::aq::aq_destroy(&mut *audio);
    crate::player::request_subtitle(-1);
    SHARED.reset_session();
    FEED_AUDIO.store(previous_feed, Ordering::Relaxed);
}
