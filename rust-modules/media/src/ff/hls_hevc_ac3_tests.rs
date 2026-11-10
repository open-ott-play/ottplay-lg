//! Opt-in real TS -> inspection/PID choice -> HEVC and native AC3 queue proof.
use super::*;

struct Restore(bool);
impl Drop for Restore {
    fn drop(&mut self) {
        FEED_AUDIO.store(self.0, Ordering::Relaxed);
        SHARED.reset_session();
    }
}

#[test]
#[ignore = "requires an explicitly authorized private HEVC/AC3 TS and bundled host FFmpeg"]
fn private_hevc_ac3_hls_preserves_each_audio_pid_and_feeds_both_lanes() {
    let _serial = plx_base::testlock::serial();
    let _ff = super::super::image_subtitle_tests::bind_host_ffmpeg();
    assert!(plx_net::net::global_init());
    crate::curlio::boot();
    let _restore = Restore(FEED_AUDIO.swap(true, Ordering::Relaxed));
    let fixture = std::env::var_os("OTTPLAY_HEVC_AC3_TS").expect("set authorized local TS path");
    let size = std::fs::metadata(&fixture).unwrap().len();
    assert!(size > 0 && size <= 24 * 1024 * 1024);
    let dir = Scratch::new();
    std::fs::hard_link(&fixture, dir.0.join("part.ts")).unwrap();
    std::fs::write(dir.0.join("one.m3u8"),
        "#EXTM3U\n#EXT-X-TARGETDURATION:60\n#EXTINF:60,\npart.ts\n#EXT-X-ENDLIST\n").unwrap();
    let http = HttpFixture::start(dir.0.clone());
    let url = format!("{}/one.m3u8", http.url);
    let info = inspect::http(&url, Arc::new(AtomicBool::new(false))).unwrap();
    assert_eq!((&*info.video_codec, &*info.audio_codec), ("hevc", "ac3"));
    assert_eq!(info.tracks.audio.len(), 2);
    let ids: Vec<_> = info.tracks.audio.iter().map(|track|
        info.tracks.transport_identity(track.stream_index).expect("initial PMT/PID evidence")).collect();
    assert_ne!(ids[0].pid, ids[1].pid, "same-label audio must retain distinct PIDs");
    for track in &info.tracks.audio {
        SHARED.reset_session();
        let selected = hls_audio::Selection::new(&info.tracks, track.ordinal).unwrap();
        assert!(selected.requires_transport_identity());
        let mut video = crate::aq::aq_new(32 * 1024 * 1024);
        let mut audio = crate::aq::aq_new(16 * 1024 * 1024);
        let mut hs = plx_net::stream::http_stream_boxed();
        let result = external_hls::demux_video(&url, "hevc", "ac3", Some(&selected),
            &mut *video, &mut *audio, &mut *hs, None);
        let mut videos = 0;
        let mut audios = 0;
        let mut first_video = None;
        let mut first_audio = None;
        let mut last_audio = None;
        let mut emitted_audio = Vec::new();
        for queue in [&mut *video, &mut *audio] {
            loop {
                let node = crate::aq::aq_pop(queue, std::ptr::null_mut());
                if node.is_null() { break; }
                unsafe {
                    let (es, key, pts, len, data) = crate::aq::au_fields(node);
                    let bytes = std::slice::from_raw_parts(data, len as usize);
                    if es == 1 {
                        if videos == 0 {
                            assert_eq!(key, 1, "entry must be an actual HEVC IRAP");
                            assert!(bytes.windows(5).any(|nal| nal == [0, 0, 1, 64, 1]), "entry carries VPS");
                            first_video = Some(pts);
                        }
                        videos += 1;
                    } else if es == 2 {
                        hls_ac3::frame(bytes, track.channels, track.sample_rate).unwrap();
                        assert!(last_audio.is_none_or(|previous| pts >= previous));
                        first_audio.get_or_insert(pts);
                        last_audio = Some(pts);
                        audios += 1;
                        emitted_audio.extend_from_slice(bytes);
                    }
                    libc::free(node.cast());
                }
            }
        }
        plx_net::stream::http_close(&mut *hs);
        crate::aq::aq_destroy(&mut *video);
        crate::aq::aq_destroy(&mut *audio);
        assert!(result.is_ok(), "HEVC/AC3 HLS failed: {result:?}");
        assert!(videos > 5 && audios > 5);
        assert!(first_audio.unwrap() >= first_video.unwrap(), "no preroll audio before entry");
        let pid = info.tracks.transport_identity(track.stream_index).unwrap().pid;
        let reference = std::process::Command::new("ffmpeg")
            .args(["-v", "error", "-i"]).arg(&fixture)
            .args(["-map", &format!("0:i:{pid}"), "-c:a", "copy", "-f", "ac3", "pipe:1"])
            .output().unwrap();
        assert!(reference.status.success(), "independent selected-PID extraction failed");
        assert!(reference.stdout.ends_with(&emitted_audio),
            "native queue must contain the selected PID's exact AC3 frames after preroll trimming");
    }
}
