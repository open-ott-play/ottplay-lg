//! Exercise real transport streams through inspection, audio PID selection and both feed queues.
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
    let fixture = std::env::var_os("OTTPLAY_HEVC_AC3_TS").expect("set authorized local TS path");
    check_native_ac3_transport(Path::new(&fixture), "hevc", false);
}

#[test]
#[ignore = "requires an explicitly authorized private two-track H264/AC3 TS and bundled host FFmpeg"]
fn private_h264_ac3_hls_preserves_each_audio_pid_and_feeds_both_lanes() {
    let fixture = std::env::var_os("OTTPLAY_H264_AC3_TS").expect("set authorized local TS path");
    check_native_ac3_transport(Path::new(&fixture), "h264", true);
}

#[test]
#[ignore = "requires the bundled host FFmpeg and ffmpeg CLI with libx264; localhost only"]
fn generated_h264_ac3_hls_preserves_each_audio_pid_and_feeds_both_lanes() {
    let dir = Scratch::new();
    let fixture = dir.0.join("generated.ts");
    entry_fixture_command("ffmpeg", &[
        "-v", "error", "-nostdin", "-y",
        "-f", "lavfi", "-i", "testsrc2=size=160x90:rate=24",
        "-f", "lavfi", "-i", "sine=frequency=440:sample_rate=48000",
        "-f", "lavfi", "-i", "sine=frequency=880:sample_rate=48000",
        "-t", "1.5", "-map", "0:v", "-map", "1:a", "-map", "2:a",
        "-c:v", "libx264", "-preset", "ultrafast", "-tune", "zerolatency",
        "-pix_fmt", "yuv420p", "-g", "24", "-bf", "0",
        "-c:a", "ac3", "-ac", "6", "-b:a", "384k",
        "-metadata:s:a:0", "language=eng", "-metadata:s:a:1", "language=eng",
        "-f", "mpegts", fixture.to_str().unwrap(),
    ], true);
    check_native_ac3_transport(&fixture, "h264", true);
}

fn check_native_ac3_transport(fixture: &Path, video_codec: &str, start_live: bool) {
    let _serial = plx_base::testlock::serial();
    let _ff = super::super::image_subtitle_tests::bind_host_ffmpeg();
    assert!(plx_net::net::global_init());
    crate::curlio::boot();
    let _restore = Restore(FEED_AUDIO.swap(true, Ordering::Relaxed));
    let size = std::fs::metadata(fixture).unwrap().len();
    assert!(size > 0 && size <= 24 * 1024 * 1024);
    let dir = Scratch::new();
    std::fs::hard_link(fixture, dir.0.join("part.ts")).unwrap();
    let segment_requested = Arc::new(AtomicBool::new(false));
    let served = Arc::clone(&segment_requested);
    let http = HttpFixture::start_with(dir.0.clone(), move |request| {
        match request.split_whitespace().nth(1)? {
            "/one.m3u8" => {
                let end = if !start_live || served.load(Ordering::Relaxed) { "#EXT-X-ENDLIST\n" } else { "" };
                let body = format!("#EXTM3U\n#EXT-X-TARGETDURATION:60\n#EXTINF:60,\npart.ts\n{end}");
                Some(format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()).into_bytes())
            }
            "/part.ts" => { served.store(true, Ordering::Relaxed); None }
            _ => None,
        }
    });
    let url = format!("{}/one.m3u8", http.url);
    let info = inspect::http(&url, Arc::new(AtomicBool::new(false))).unwrap();
    assert_eq!((&*info.video_codec, &*info.audio_codec), (video_codec, "ac3"));
    assert_eq!(info.duration_ns == 0, start_live);
    assert_eq!(info.tracks.audio.len(), 2);
    let ids: Vec<_> = info.tracks.audio.iter().map(|track|
        info.tracks.transport_identity(track.stream_index).expect("initial PMT/PID evidence")).collect();
    assert_ne!(ids[0].pid, ids[1].pid, "same-label audio must retain distinct PIDs");
    for track in &info.tracks.audio {
        segment_requested.store(false, Ordering::Relaxed);
        SHARED.reset_session();
        let selected = hls_audio::Selection::new(&info.tracks, track.ordinal).unwrap();
        assert!(selected.requires_transport_identity());
        let mut video = crate::aq::aq_new(32 * 1024 * 1024);
        let mut audio = crate::aq::aq_new(16 * 1024 * 1024);
        let mut hs = plx_net::stream::http_stream_boxed();
        let result = external_hls::demux_video(&url, &info.video_codec, &info.audio_codec, Some(&selected),
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
                            assert_eq!(key, 1, "first video AU must permit independent decoding");
                            if video_codec == "hevc" {
                                assert!(bytes.windows(5).any(|nal| nal == [0, 0, 1, 64, 1]), "entry carries VPS");
                            } else {
                                for kind in [7, 8, 5] {
                                    assert!(bytes.windows(4).any(|nal| nal[..3] == [0, 0, 1] && nal[3] & 31 == kind),
                                        "H.264 entry must carry SPS, PPS and IDR (missing NAL type {kind})");
                                }
                            }
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
        assert!(result.is_ok(), "{video_codec}/AC3 HLS failed: {result:?}");
        assert!(videos > 5 && audios > 5);
        assert!(first_audio.unwrap() >= first_video.unwrap(), "no preroll audio before entry");
        let pid = info.tracks.transport_identity(track.stream_index).unwrap().pid;
        let reference = std::process::Command::new("ffmpeg")
            .args(["-v", "error", "-i"]).arg(fixture)
            .args(["-map", &format!("0:i:{pid}"), "-c:a", "copy", "-f", "ac3", "pipe:1"])
            .output().unwrap();
        assert!(reference.status.success(), "independent selected-PID extraction failed");
        assert!(reference.stdout.ends_with(&emitted_audio),
            "native queue must contain the selected PID's exact AC3 frames after preroll trimming");
    }
}
