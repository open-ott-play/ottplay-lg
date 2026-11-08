//! Real embedded TS MP2 -> inspected route -> PCM samples, using the bundled ABI.
use super::*;

struct PcmRestore(Option<crate::ff_pcm::PcmOut>, bool, bool);
impl PcmRestore {
    fn install() -> Self {
        let previous = Self(pcm_out(), pcm_lossy(), FEED_AUDIO.load(Ordering::Relaxed));
        set_pcm_out(Some(crate::ff_pcm::STEREO_48K), true);
        FEED_AUDIO.store(true, Ordering::Relaxed);
        previous
    }
}
impl Drop for PcmRestore {
    fn drop(&mut self) {
        set_pcm_out(self.0, self.1);
        FEED_AUDIO.store(self.2, Ordering::Relaxed);
        SHARED.reset_session();
    }
}

fn read_pcm(
    http: &HttpFixture,
    name: &str,
    selected: &hls_audio::Selection,
    seek: i64,
) -> (Vec<(i64, usize)>, Vec<u8>, usize) {
    SHARED.reset_session();
    SHARED.seek_to_ns.store(seek, Ordering::Release);
    let _pcm = PcmRestore::install();
    let mut video = crate::aq::aq_new(4 * 1024 * 1024);
    let mut audio = crate::aq::aq_new(4 * 1024 * 1024);
    let mut hs = plx_net::stream::http_stream_boxed();
    let result = external_hls::demux(
        &format!("{}/{name}", http.url),
        "mp2",
        Some(selected),
        &mut *video,
        &mut *audio,
        &mut *hs,
    );
    let mut stamps = Vec::new();
    let mut pcm = Vec::new();
    let mut videos = 0;
    for queue in [&mut *video, &mut *audio] {
        loop {
            let node = crate::aq::aq_pop(queue, std::ptr::null_mut());
            if node.is_null() {
                break;
            }
            unsafe {
                let (es, _, pts, len, data) = crate::aq::au_fields(node);
                if es == 2 {
                    assert!(len > 0 && len % 4 == 0, "stereo S16 frame alignment");
                    stamps.push((pts, len as usize / 4));
                    pcm.extend_from_slice(std::slice::from_raw_parts(data, len as usize));
                } else if es == 1 {
                    videos += 1;
                }
                libc::free(node.cast());
            }
        }
    }
    plx_net::stream::http_close(&mut *hs);
    crate::aq::aq_destroy(&mut *video);
    crate::aq::aq_destroy(&mut *audio);
    assert!(result.is_ok(), "MP2 embedded HLS failed: {result:?}");
    assert!(
        videos > 5 && !stamps.is_empty(),
        "both lanes must reach real queues"
    );
    (stamps, pcm, videos)
}

#[test]
#[ignore = "requires bundled host FFmpeg and ffmpeg/libx264/MP2; synthetic localhost only"]
fn embedded_hls_mp2_decodes_pcm_with_continuity_discontinuity_and_seek() {
    let _serial = plx_base::testlock::serial();
    let _ff = super::super::image_subtitle_tests::bind_host_ffmpeg();
    assert!(plx_net::net::global_init());
    crate::curlio::boot();
    for rate in [48_000, 44_100] {
        let rate_arg = rate.to_string();
        let sine = format!("sine=frequency=440:sample_rate={rate}");
        let dir = Scratch::new();
        let generated = std::process::Command::new("ffmpeg")
            .args([
                "-v",
                "error",
                "-f",
                "lavfi",
                "-i",
                "testsrc2=size=160x90:rate=10",
                "-f",
                "lavfi",
                "-i",
                &sine,
                "-t",
                "3",
                "-c:v",
                "libx264",
                "-preset",
                "ultrafast",
                "-tune",
                "zerolatency",
                "-g",
                "10",
                "-c:a",
                "mp2",
                "-ac",
                "2",
                "-ar",
                &rate_arg,
                "-b:a",
                "192k",
                "-hls_time",
                "1",
                "-hls_playlist_type",
                "vod",
                "-hls_segment_filename",
            ])
            .arg(dir.0.join("part-%d.ts"))
            .args(["-y"])
            .arg(dir.0.join("all.m3u8"))
            .output()
            .unwrap();
        assert!(
            generated.status.success(),
            "fixture: {}",
            String::from_utf8_lossy(&generated.stderr)
        );
        let one = "#EXTM3U\n#EXT-X-TARGETDURATION:1\n#EXTINF:1,\npart-0.ts\n#EXT-X-ENDLIST\n";
        let twice = "#EXTM3U\n#EXT-X-TARGETDURATION:1\n#EXTINF:1,\npart-0.ts\n#EXT-X-DISCONTINUITY\n#EXTINF:1,\npart-0.ts\n#EXT-X-ENDLIST\n";
        std::fs::write(dir.0.join("one.m3u8"), one).unwrap();
        std::fs::write(dir.0.join("disco.m3u8"), twice).unwrap();
        let http = HttpFixture::start(dir.0.clone());
        let url = format!("{}/all.m3u8", http.url);
        let info = inspect::http(&url, Arc::new(AtomicBool::new(false))).unwrap();
        assert_eq!((&*info.video_codec, &*info.audio_codec), ("h264", "mp2"));
        assert!(info.tracks.audio[0].decoder_available);
        let selected = hls_audio::Selection::new(&info.tracks, 0).unwrap();
        let mut caps = plx_platform::devcaps::Caps::assumed();
        assert!(!hls_mp2::supported(&info.tracks.audio[0], &caps));
        caps.audio.push_str(",pcm");
        caps.audio_channels.insert("pcm".into(), 2);
        assert!(hls_mp2::supported(&info.tracks.audio[0], &caps));
        let source = info.resolved_source.clone().unwrap();
        let mut separate = hls_presentation::HlsPlayback::fixture_for_test().media_info_for_test();
        separate.audio_codec = "mp2".into();
        separate.audio_sample_rate = rate;
        separate.tracks = info.tracks.clone();
        let mut separate_session = crate::route::PlaybackSession::IDLE;
        crate::route::reset_player_control_for_test(&separate_session);
        assert!(
            crate::route::request_external_source(
                &mut separate_session,
                &separate,
                ExternalSource::Hls("https://example.invalid/master.m3u8".into()),
                "Fixture",
                "HLS",
                Arc::new(caps.clone())
            )
            .is_err(),
            "MP2 must not enter the separate AUDIO-rendition leg"
        );
        let mut refused = crate::route::PlaybackSession::IDLE;
        crate::route::reset_player_control_for_test(&refused);
        assert!(crate::route::request_external_source(
            &mut refused,
            &info,
            source.clone(),
            "Fixture",
            "M3U",
            plx_platform::devcaps::assumed_shared()
        )
        .is_err());
        crate::route::reset_player_control_for_test(&refused);
        let mut ordinary = info.clone();
        ordinary.resolved_source = None;
        assert!(
            crate::route::request_external_source(
                &mut refused,
                &ordinary,
                ExternalSource::Http(format!("{}/ordinary.ts", http.url)),
                "Fixture",
                "HTTP",
                Arc::new(caps.clone())
            )
            .is_err(),
            "embedded MP2 admission must not enable ordinary HTTP/Plex demux policy"
        );
        let mut session = crate::route::PlaybackSession::IDLE;
        crate::route::reset_player_control_for_test(&session);
        crate::route::request_external_source(
            &mut session,
            &info,
            source,
            "Fixture",
            "M3U",
            Arc::new(caps),
        )
        .unwrap();
        assert_eq!(
            crate::route::stream_acodec(&session),
            "mp2",
            "retain real source codec"
        );
        assert_eq!(
            crate::route::stream_pcm(&session),
            Some(Ok(crate::route::PcmFormat::STEREO_48K))
        );
        assert!(crate::route::stream_pcm_is_lossy(&session));
        crate::route::reset_player_control_for_test(&crate::route::PlaybackSession::IDLE);

        let (stamps, pcm, _) = read_pcm(&http, "all.m3u8", &selected, -1);
        assert!(
            stamps.windows(2).all(|w| w[1].0 >= w[0].0),
            "continuous audio time regressed"
        );
        assert!(pcm.len() / 4 > 2 * 48_000, "decoded sample count");
        let signal: Vec<i16> = pcm
            .chunks_exact(4)
            .map(|f| i16::from_le_bytes([f[0], f[1]]))
            .collect();
        let signal = &signal[4800..signal.len() - 4800];
        assert!(
            signal.iter().any(|s| s.unsigned_abs() > 500),
            "audible signal, not silence"
        );
        let crossings = signal.windows(2).filter(|w| w[0] <= 0 && w[1] > 0).count();
        let frequency = crossings as f64 * 48_000.0 / signal.len() as f64;
        assert!(
            (frequency - 440.0).abs() < 5.0,
            "wrong PCM rate/pitch: {frequency}"
        );
        for pair in stamps.windows(2) {
            let end = pair[0].0 + pair[0].1 as i64 * 1_000_000_000 / 48_000;
            assert!(
                (pair[1].0 - end).abs() < 3_000_000,
                "PCM clock discontinuity: {pair:?}"
            );
        }
        let (_, single, _) = read_pcm(&http, "one.m3u8", &selected, -1);
        let (disco_stamps, repeated, _) = read_pcm(&http, "disco.m3u8", &selected, -1);
        assert_eq!(repeated.len(), single.len() * 2);
        assert_eq!(&repeated[..single.len()], &single);
        assert_eq!(
            &repeated[single.len()..],
            &single,
            "discontinuity must reset MP2 synthesis history"
        );
        assert!(disco_stamps.last().unwrap().0 >= 1_000_000_000);
        let (seek_stamps, after_seek, _) = read_pcm(&http, "disco.m3u8", &selected, 1_000_000_000);
        assert!(seek_stamps[0].0 >= 1_000_000_000);
        assert_eq!(
            after_seek, single,
            "seek must enter with fresh audio decoder state"
        );

        // Request a seek only when the worker has fed part 0 and is reading part 1.
        // Compare the resulting part 2 with a fresh worker starting there: unlike
        // initial seek alone, this exercises discarding populated synthesis history.
        let (_, seek_reference, _) = read_pcm(&http, "all.m3u8", &selected, 2_000_000_000);
        let triggered = Arc::new(AtomicBool::new(false));
        let on_part_one = Arc::clone(&triggered);
        let seeking_http = HttpFixture::start_with(dir.0.clone(), move |head| {
            if head.starts_with("GET /part-1.ts ") && !on_part_one.swap(true, Ordering::AcqRel) {
                SHARED.seek_to_ns.store(2_000_000_000, Ordering::Release);
            }
            None
        });
        let (_, sought, _) = read_pcm(&seeking_http, "all.m3u8", &selected, -1);
        assert!(
            triggered.load(Ordering::Acquire),
            "seek must happen inside the running worker"
        );
        assert_eq!(sought.len(), single.len() + seek_reference.len());
        assert_eq!(&sought[..single.len()], &single);
        assert_eq!(
            &sought[single.len()..],
            &seek_reference,
            "in-flight seek must discard the first segment's decoder/filter history"
        );
    }
}
