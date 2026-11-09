//! Real HEVC/AAC muxed TS through inspection, native admission and both queues.
use super::*;

#[test]
#[ignore = "requires bundled host FFmpeg and ffmpeg/libx265/AAC; synthetic localhost only"]
fn muxed_hevc_aac_keeps_native_caps_and_complete_av_timing() {
    let _serial = plx_base::testlock::serial();
    let _ff = super::super::image_subtitle_tests::bind_host_ffmpeg();
    assert!(plx_net::net::global_init());
    crate::curlio::boot();
    let dir = Scratch::new();
    let generated = std::process::Command::new("ffmpeg")
        .args(["-v", "error", "-f", "lavfi", "-i", "testsrc2=size=160x90:rate=12",
            "-f", "lavfi", "-i", "sine=frequency=440:sample_rate=48000", "-t", "3",
            "-c:v", "libx265", "-preset", "ultrafast", "-x265-params",
            "pools=1:frame-threads=1:log-level=error:keyint=12:min-keyint=12:open-gop=1:repeat-headers=1",
            "-c:a", "aac", "-ac", "2", "-b:a", "96k", "-hls_time", "1",
            "-hls_playlist_type", "vod", "-hls_segment_filename"])
        .arg(dir.0.join("part-%d.ts")).args(["-y"]).arg(dir.0.join("all.m3u8"))
        .output().unwrap();
    assert!(generated.status.success(), "synthetic fixture generation failed");
    let http = HttpFixture::start(dir.0.clone());
    let url = format!("{}/all.m3u8", http.url);
    let info = inspect::http(&url, Arc::new(AtomicBool::new(false))).unwrap();
    assert_eq!((&*info.video_codec, &*info.audio_codec), ("hevc", "aac"));
    let source = info.resolved_source.clone().unwrap();
    for (hevc, aac, allowed) in [(true,true,true), (false,true,false), (true,false,false)] {
        let mut caps = plx_platform::devcaps::Caps::assumed();
        caps.hevc = hevc;
        if !aac {
            caps.audio_channels.insert("aac".into(), 0);
            caps.native_ceiling.insert("aac".into(), 0);
        }
        let mut session = crate::route::PlaybackSession::IDLE;
        crate::route::reset_player_control_for_test(&session);
        let result = crate::route::request_external_source(&mut session, &info,
            source.clone(), "Fixture", "HLS", Arc::new(caps));
        assert_eq!(result.is_ok(), allowed, "native codec admission must remain enforced");
    }
    crate::route::reset_player_control_for_test(&crate::route::PlaybackSession::IDLE);
    let selected = hls_audio::Selection::new(&info.tracks, 0).unwrap();
    let previous_audio = FEED_AUDIO.swap(true, Ordering::Relaxed);
    SHARED.reset_session();
    let mut video = crate::aq::aq_new(8 * 1024 * 1024);
    let mut audio = crate::aq::aq_new(4 * 1024 * 1024);
    let mut hs = plx_net::stream::http_stream_boxed();
    let result = external_hls::demux_video(&url, "hevc", "aac", Some(&selected),
        &mut *video, &mut *audio, &mut *hs, None);
    let mut videos = 0;
    let mut audios = 0;
    let mut first_video = None;
    let mut first_audio = None;
    let mut last_audio = None;
    let mut last_video = None;
    for queue in [&mut *video, &mut *audio] {
        loop {
            let node = crate::aq::aq_pop(queue, std::ptr::null_mut());
            if node.is_null() { break; }
            unsafe {
                let (es, key, pts, len, data) = crate::aq::au_fields(node);
                let bytes = std::slice::from_raw_parts(data, len as usize);
                if es == 1 {
                    if videos == 0 {
                        assert_eq!(key, 1);
                        assert!(bytes.windows(5).any(|nal| nal == [0,0,1,64,1]));
                        first_video = Some(pts);
                    }
                    last_video = Some(last_video.map_or(pts, |old: i64| old.max(pts)));
                    videos += 1;
                } else if es == 2 {
                    assert!(packet_has_adts(bytes));
                    assert_eq!(adts_duration_ns(bytes), Some(1_024 * 1_000_000_000 / 48_000));
                    assert!(last_audio.is_none_or(|previous| pts >= previous));
                    first_audio.get_or_insert(pts); last_audio = Some(pts); audios += 1;
                }
                libc::free(node.cast());
            }
        }
    }
    plx_net::stream::http_close(&mut *hs);
    crate::aq::aq_destroy(&mut *video); crate::aq::aq_destroy(&mut *audio);
    FEED_AUDIO.store(previous_audio, Ordering::Relaxed); SHARED.reset_session();
    assert!(result.is_ok(), "HEVC/AAC HLS failed: {result:?}");
    assert!(videos > 20 && audios > 50);
    assert!(first_audio.unwrap() >= first_video.unwrap());
    assert!(last_video.unwrap() > 2_000_000_000 && last_audio.unwrap() > 2_000_000_000);
}
