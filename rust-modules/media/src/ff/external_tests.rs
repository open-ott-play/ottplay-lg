//! Actual bundled-demux tests; these intentionally require the host FFmpeg build.
use super::*;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

#[path = "hls_mp2_tests.rs"]
mod mp2_tests;
#[path = "hls_hevc_ac3_tests.rs"]
mod hevc_ac3_tests;
#[path = "hls_hevc_aac_tests.rs"]
mod hevc_aac_tests;

struct Scratch(PathBuf);
impl Scratch {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "ott-native-media-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn fixture(dir: &Path, extension: &str) -> PathBuf {
    let path = dir.join(format!("fixture.{extension}"));
    let source =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../site/media/feel-720p60.h264.mp4");
    let result = std::process::Command::new("ffmpeg")
        .args(["-v", "error", "-i"])
        .arg(source)
        .args(["-t", "8", "-an", "-c:v", "copy", "-y"])
        .arg(&path)
        .output()
        .expect("ffmpeg CLI installed");
    assert!(
        result.status.success(),
        "fixture generation: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    path
}

fn decode(source: ExternalSource, seek: i64) -> Vec<i64> {
    SHARED.reset_session();
    SHARED.seek_to_ns.store(seek, Ordering::Release);
    FEED_AUDIO.store(false, Ordering::Relaxed);
    let mut video = crate::aq::aq_new(1024 * 1024);
    let mut audio = crate::aq::aq_new(128 * 1024);
    let mut hs = plx_net::stream::http_stream_boxed();
    let (v, a, h) = (
        SendPtr(&mut *video),
        SendPtr(&mut *audio),
        SendPtr(&mut *hs),
    );
    let th = std::thread::spawn(move || {
        plx_base::eventlog::with_private_log(|| {
            let off = plx_base::task::OffFrame::for_test();
            demux_external(&off, 0, source, "h264".into(), String::new(), None, v, a, h, crate::player::text_style::TextStyle::current(Default::default()));
            std::fs::read_to_string(plx_base::eventlog::events_log()).unwrap_or_default()
        })
    });
    let deadline = Instant::now() + Duration::from_secs(15);
    let mut stamps = Vec::new();
    while Instant::now() < deadline && stamps.len() < 30 {
        let node = crate::aq::aq_pop(&mut *video, std::ptr::null_mut());
        if !node.is_null() {
            unsafe {
                stamps.push(crate::aq::au_fields(node).2);
                libc::free(node.cast());
            }
        } else if th.is_finished() {
            break;
        } else {
            std::thread::sleep(Duration::from_millis(5));
        }
    }
    crate::aq::aq_abort(&mut *video);
    crate::aq::aq_abort(&mut *audio);
    crate::curlio::abort_active();
    plx_net::stream::http_shutdown(&mut *hs);
    let log = th.join().unwrap();
    plx_net::stream::http_close(&mut *hs);
    crate::aq::aq_destroy(&mut *video);
    crate::aq::aq_destroy(&mut *audio);
    assert!(
        !SHARED.demux_failed.load(Ordering::Acquire),
        "actual bundled demux refused the source: {log}"
    );
    assert!(
        stamps.len() >= 20,
        "expected video AUs, received {}",
        stamps.len()
    );
    assert!(
        SHARED.seek_to_ns.load(Ordering::Acquire) < 0,
        "demux must consume the resume/seek request"
    );
    FEED_AUDIO.store(true, Ordering::Relaxed);
    stamps
}

#[test]
#[ignore = "requires bundled host FFmpeg and ffmpeg CLI"]
fn native_local_mkv_avi_and_mp4_produce_video_aus_and_seek() {
    let _serial = plx_base::testlock::serial();
    let _ff = super::image_subtitle_tests::bind_host_ffmpeg();
    let dir = Scratch::new();
    for ext in ["mp4", "mkv", "avi"] {
        let path = fixture(&dir.0, ext);
        let info = inspect::source(
            &ExternalSource::Local(path.clone()),
            Arc::new(AtomicBool::new(false)),
        )
        .unwrap();
        assert_eq!(info.video_codec, "h264");
        assert_eq!((info.width, info.height), (1280, 720));
        let stamps = decode(ExternalSource::Local(path), 5_000_000_000);
        assert!(
            stamps[0] >= 2_000_000_000,
            "{ext} resumed at {} instead of a nearby keyframe",
            stamps[0]
        );
    }
}

struct HttpFixture {
    url: String,
    stop: Arc<AtomicBool>,
    requests: Arc<std::sync::Mutex<Vec<String>>>,
    thread: Option<std::thread::JoinHandle<()>>,
}
impl HttpFixture {
    fn start(root: PathBuf) -> Self {
        Self::start_with(root, |_| None)
    }
    fn start_with(
        root: PathBuf,
        on_head: impl Fn(&str) -> Option<Vec<u8>> + Send + 'static,
    ) -> Self {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        listener.set_nonblocking(true).unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let quitting = stop.clone();
        let requests = Arc::new(std::sync::Mutex::new(Vec::new()));
        let seen = Arc::clone(&requests);
        let thread = std::thread::spawn(move || {
            while !quitting.load(Ordering::Acquire) {
                let Ok((mut socket, _)) = listener.accept() else {
                    std::thread::sleep(Duration::from_millis(5));
                    continue;
                };
                // Darwin can inherit O_NONBLOCK from the listening socket. A partial
                // fixture write is not a complete HTTP response; send the whole body.
                socket.set_nonblocking(false).unwrap();
                socket
                    .set_read_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                socket
                    .set_write_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                let mut header = Vec::new();
                let mut byte = [0u8; 1];
                while header.len() < 8192 && !header.ends_with(b"\r\n\r\n") {
                    if socket.read(&mut byte).unwrap_or(0) == 0 {
                        break;
                    }
                    header.push(byte[0]);
                }
                let header = String::from_utf8_lossy(&header);
                let path = header
                    .lines()
                    .next()
                    .and_then(|l| l.split_whitespace().nth(1))
                    .unwrap_or("/");
                seen.lock().unwrap().push(path.to_owned());
                if let Some(response) = on_head(&header) {
                    let _ = socket.write_all(&response);
                    continue;
                }
                if path == "/redirect" {
                    let _ = socket.write_all(b"HTTP/1.1 302 Found\r\nLocation: /moved\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
                    continue;
                }
                let path = path
                    .split('?')
                    .next()
                    .unwrap_or(path)
                    .trim_start_matches('/');
                let data = if path.contains("..") {
                    Vec::new()
                } else {
                    std::fs::read(root.join(path)).unwrap_or_default()
                };
                let range = header.lines().find_map(|l| {
                    l.to_ascii_lowercase()
                        .strip_prefix("range: bytes=")
                        .and_then(|s| s.split('-').next())
                        .and_then(|s| s.parse::<usize>().ok())
                });
                let start = range.unwrap_or(0).min(data.len());
                let extra = if range.is_some() && start < data.len() {
                    format!(
                        "Content-Range: bytes {start}-{}/{}\r\n",
                        data.len() - 1,
                        data.len()
                    )
                } else {
                    String::new()
                };
                let status = if data.is_empty() {
                    404
                } else if range.is_some() {
                    206
                } else {
                    200
                };
                let response=format!("HTTP/1.1 {status} OK\r\nConnection: close\r\nContent-Length: {}\r\n{extra}\r\n",data.len()-start);
                if socket.write_all(response.as_bytes()).is_ok() {
                    let _ = socket.write_all(&data[start..]);
                }
            }
        });
        Self {
            url: format!("http://{addr}"),
            stop,
            requests,
            thread: Some(thread),
        }
    }
}
impl Drop for HttpFixture {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(th) = self.thread.take() {
            th.join().unwrap();
        }
    }
}

#[test]
#[ignore = "requires bundled host FFmpeg and ffmpeg CLI"]
fn external_hls_extensionless_vod_is_inspected_demuxed_and_seeked() {
    let _serial = plx_base::testlock::serial();
    let _ff = super::image_subtitle_tests::bind_host_ffmpeg();
    assert!(plx_net::net::global_init());
    crate::curlio::boot();
    let dir = Scratch::new();
    let mp4 = fixture(&dir.0, "mp4");
    let result = std::process::Command::new("ffmpeg")
        .args(["-v", "error", "-i"])
        .arg(mp4)
        .args([
            "-an",
            "-c:v",
            "copy",
            "-hls_time",
            "2",
            "-hls_playlist_type",
            "vod",
            "-hls_segment_filename",
        ])
        .arg(dir.0.join("part-%d.ts"))
        .args(["-y"])
        .arg(dir.0.join("index.m3u8"))
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "HLS fixture generation: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    std::fs::copy(dir.0.join("index.m3u8"), dir.0.join("index")).unwrap();
    let http = HttpFixture::start(dir.0.clone());
    let url = format!("{}/index", http.url);
    let info = inspect::http(&url, Arc::new(AtomicBool::new(false))).unwrap();
    assert_eq!(info.video_codec, "h264");
    assert!(info.duration_ns >= 7_000_000_000);
    let Some(source @ ExternalSource::Hls(_)) = info.resolved_source else {
        panic!("extensionless playlist not detected");
    };
    let stamps = decode(source, 5_000_000_000);
    assert!(
        stamps[0] >= 2_000_000_000,
        "HLS seek resumed from the beginning"
    );
}

struct ProxyEnvironment(Vec<(&'static str, Option<std::ffi::OsString>)>);
impl ProxyEnvironment {
    fn poisoned() -> Self {
        let names = ["http_proxy", "HTTP_PROXY", "all_proxy", "ALL_PROXY", "no_proxy", "NO_PROXY"];
        let old = names.iter().map(|&key| (key, std::env::var_os(key))).collect();
        for key in names {
            std::env::set_var(key, if key.eq_ignore_ascii_case("no_proxy") { "" } else { "http://127.0.0.1:9" });
        }
        Self(old)
    }
}
impl Drop for ProxyEnvironment {
    fn drop(&mut self) {
        for (key, value) in &self.0 {
            match value { Some(value) => std::env::set_var(key, value), None => std::env::remove_var(key) }
        }
    }
}

#[test]
#[ignore = "requires bundled host FFmpeg and ffmpeg CLI"]
fn owned_companion_keeps_exact_capability_without_proxy_or_child_requests() {
    let _serial = plx_base::testlock::serial();
    let _ff = super::image_subtitle_tests::bind_host_ffmpeg();
    assert!(plx_net::net::global_init());
    crate::curlio::boot();
    let dir = Scratch::new();
    fixture(&dir.0, "mp4");
    std::fs::write(dir.0.join("playlist"), "#EXTM3U\n#EXT-X-TARGETDURATION:4\n#EXTINF:4,\nhttp://127.0.0.1:9/forbidden\n").unwrap();
    let http = HttpFixture::start(dir.0.clone());
    let _proxy = ProxyEnvironment::poisoned();
    let source = ExternalSource::OwnedHttp(format!("{}/fixture.mp4?lease=fixture-only", http.url));
    let info = inspect::source(&source, Arc::new(AtomicBool::new(false))).unwrap();
    assert_eq!(info.video_codec, "h264");
    assert!(info.resolved_source.is_none());
    let stamps = decode(source, 5_000_000_000);
    assert!(stamps[0] >= 2_000_000_000);
    assert!(http.requests.lock().unwrap().iter().all(|path| path == "/fixture.mp4?lease=fixture-only"));
    http.requests.lock().unwrap().clear();
    assert!(inspect::source(&ExternalSource::OwnedHttp(format!("{}/redirect", http.url)), Arc::new(AtomicBool::new(false))).is_err());
    assert_eq!(*http.requests.lock().unwrap(), vec!["/redirect"]);
    http.requests.lock().unwrap().clear();
    assert!(inspect::source(&ExternalSource::OwnedHttp(format!("{}/playlist", http.url)), Arc::new(AtomicBool::new(false))).is_err());
    assert_eq!(*http.requests.lock().unwrap(), vec!["/playlist"]);
}


/// Exercises the actual bundled segment reader and its emitted AAC packets, not
/// just labels/selection fields. Run explicitly with the host FFmpeg test gate.
#[test]
#[ignore = "requires bundled host FFmpeg and ffmpeg CLI with libx264"]
fn independent_hls_audio_pick_emits_selected_tone_across_reordered_segments() {
    let _serial = plx_base::testlock::serial();
    let _ff = super::image_subtitle_tests::bind_host_ffmpeg();
    assert!(plx_net::net::global_init());
    crate::curlio::boot();
    SHARED.reset_session();
    struct AudioFeed(bool);
    impl Drop for AudioFeed {
        fn drop(&mut self) { FEED_AUDIO.store(self.0, Ordering::Relaxed); }
    }
    let _audio_feed = AudioFeed(FEED_AUDIO.swap(true, Ordering::Relaxed));
    let dir = Scratch::new();
    let run = |args: &[&str], output: &Path| {
        let result = std::process::Command::new("ffmpeg")
            .args(["-v", "error"]).args(args).args(["-y"]).arg(output).output().unwrap();
        assert!(result.status.success(), "fixture: {}", String::from_utf8_lossy(&result.stderr));
    };
    let first = dir.0.join("first.ts");
    run(&["-f", "lavfi", "-i", "testsrc2=size=160x90:rate=10",
        "-f", "lavfi", "-i", "sine=frequency=220:sample_rate=48000",
        "-f", "lavfi", "-i", "sine=frequency=880:sample_rate=48000",
        "-t", "1", "-map", "0:v:0", "-map", "1:a:0", "-map", "2:a:0",
        "-c:v", "libx264", "-preset", "ultrafast", "-tune", "zerolatency", "-g", "10",
        "-c:a", "aac", "-ac", "1", "-ar", "48000",
        "-metadata:s:a:0", "language=eng", "-metadata:s:a:1", "language=spa",
        "-f", "mpegts"], &first);
    run(&["-i", first.to_str().unwrap(), "-map", "0:v:0", "-map", "0:a:1",
        "-map", "0:a:0", "-c", "copy", "-f", "mpegts"], &dir.0.join("reordered.ts"));
    run(&["-i", first.to_str().unwrap(), "-map", "0:v:0", "-map", "0:a:0",
        "-c", "copy", "-f", "mpegts"], &dir.0.join("missing.ts"));
    let http = HttpFixture::start(dir.0.clone());
    let info = inspect::http(&format!("{}/first.ts", http.url),
        Arc::new(AtomicBool::new(false))).unwrap();
    assert_eq!(info.tracks.audio.len(), 2);
    assert_eq!(info.tracks.audio[0].language, "eng");
    assert_eq!(info.tracks.audio[1].language, "spa");
    let english = hls_audio::Selection::new(&info.tracks, 0).unwrap();
    let spanish = hls_audio::Selection::new(&info.tracks, 1).unwrap();
    let mut aq = crate::aq::aq_new(1024 * 1024);
    let mut hs = plx_net::stream::http_stream_boxed();
    let mut net = HlsNet { hs: &mut *hs, curl: None };
    let mut segment = |name: &str, pick: &hls_audio::Selection| {
        let parsed = plx_plex::plex::StreamUrl::parse(&format!("{}/{name}", http.url));
        let source = crate::hls::Segment {
            sequence: 0, duration: Duration::from_secs(1),
            resource: crate::hls::Resource::new(parsed.origin, parsed.path).unwrap(),
        };
        let timeline = crate::hls::SegmentTimeline::default();
        let mut clock = timeline.begin(source.duration).unwrap();
        unsafe { hls_demux_segment_path(&source,
            HlsSegmentRequest { path: &source.resource.path, transport: HlsSegmentTransport::External, body_limit: Some(4 * 1024 * 1024), audio: Some(pick), rendition: None, video: HlsVideoEntry::Independent, mp2: None },
            &mut clock, &mut *aq, &mut net, "aac", SegmentAcquisition::for_cursor(None)) }
    };
    let tone = |name: &str, output: HlsSegmentOutput| {
        let data: Vec<u8> = output.aus.iter().filter(|au| au.es == 2)
            .flat_map(|au| au.data.iter().copied()).collect();
        assert!(!data.is_empty(), "selected stream must emit AAC packets");
        let file = dir.0.join(name);
        std::fs::write(&file, data).unwrap();
        let decoded = std::process::Command::new("ffmpeg").args(["-v", "error", "-i"])
            .arg(file).args(["-ac", "1", "-ar", "48000", "-f", "s16le", "pipe:1"])
            .output().unwrap();
        assert!(decoded.status.success(), "AAC decode: {}", String::from_utf8_lossy(&decoded.stderr));
        let samples: Vec<i16> = decoded.stdout.chunks_exact(2)
            .map(|b| i16::from_le_bytes([b[0], b[1]])).collect();
        assert!(samples.len() > 24_000);
        let samples = &samples[2400..samples.len() - 2400];
        let crossings = samples.windows(2).filter(|w| w[0] <= 0 && w[1] > 0).count();
        crossings as f64 * 48_000.0 / samples.len() as f64
    };
    let low = tone("eng.aac", segment("first.ts", &english).unwrap_or_else(|_| panic!("English segment refused")));
    let high = tone("spa.aac", segment("first.ts", &spanish).unwrap_or_else(|_| panic!("Spanish segment refused")));
    let reordered = tone("reordered.aac", segment("reordered.ts", &spanish).unwrap_or_else(|_| panic!("Reordered segment refused")));
    assert!((180.0..260.0).contains(&low), "English tone {low}");
    assert!((800.0..960.0).contains(&high), "Spanish tone {high}");
    assert!((800.0..960.0).contains(&reordered), "Reordered Spanish tone {reordered}");
    assert!(segment("missing.ts", &spanish).is_err(), "missing selected audio must not fall back to English");
    drop(segment);
    plx_net::stream::http_close(&mut *hs);
    crate::aq::aq_destroy(&mut *aq);
}


#[test]
#[ignore = "requires bundled host FFmpeg and ffmpeg CLI with libx264"]
fn hls_audio_stream_survives_late_pmt_stream_table_growth() {
    let _serial = plx_base::testlock::serial();
    let _ff = super::image_subtitle_tests::bind_host_ffmpeg();
    assert!(plx_net::net::global_init());
    crate::curlio::boot();
    SHARED.reset_session();
    struct AudioFeed(bool);
    impl Drop for AudioFeed {
        fn drop(&mut self) { FEED_AUDIO.store(self.0, Ordering::Relaxed); }
    }
    let _audio_feed = AudioFeed(FEED_AUDIO.swap(true, Ordering::Relaxed));
    let dir = Scratch::new();
    let run = |args: &[&str], path: &Path| {
        let result = std::process::Command::new("ffmpeg").args(["-v", "error"])
            .args(args).args(["-y"]).arg(path).output().unwrap();
        assert!(result.status.success(), "fixture: {}", String::from_utf8_lossy(&result.stderr));
    };
    // An initial ordinary video+audio PMT is followed by an updated PMT that
    // retains their PIDs and adds audio streams. The selected audio AVStream
    // remains live, while libavformat may reallocate AVFormatContext.streams.
    let early = dir.0.join("early.ts");
    let late = dir.0.join("late.ts");
    run(&["-f", "lavfi", "-i", "testsrc2=size=160x90:rate=10",
        "-f", "lavfi", "-i", "sine=frequency=220:sample_rate=48000",
        "-t", "12", "-map", "0:v:0", "-map", "1:a:0", "-c:v", "libx264",
        "-preset", "ultrafast", "-tune", "zerolatency", "-g", "10",
        "-b:v", "4M", "-minrate:v", "4M", "-maxrate:v", "4M", "-bufsize:v", "1M",
        "-nal-hrd", "cbr", "-c:a", "aac",
        "-metadata:s:a:0", "language=eng", "-tables_version", "0", "-f", "mpegts"], &early);
    // The bundled build has no raw AAC demuxer, so MPEG-TS codec probing can
    // buffer up to 5 MB of packet payload before yielding the early packets.
    // H.264 CBR filler counts as payload; TS null-packet padding would not.
    let early_video = dir.0.join("early.h264");
    run(&["-i", early.to_str().unwrap(), "-map", "0:v:0", "-c:v", "copy",
        "-f", "h264"], &early_video);
    assert!(std::fs::metadata(&early_video).unwrap().len() > 5_500_000,
        "early video payload must exceed the 5 MB probe budget with margin");
    run(&["-f", "lavfi", "-i", "testsrc2=size=160x90:rate=10",
        "-f", "lavfi", "-i", "sine=frequency=220:sample_rate=48000",
        "-f", "lavfi", "-i", "sine=frequency=880:sample_rate=48000",
        "-t", "6", "-map", "0:v:0", "-map", "1:a:0", "-map", "2:a:0", "-map", "2:a:0",
        "-map", "2:a:0", "-map", "2:a:0", "-c:v", "libx264", "-preset", "ultrafast",
        "-tune", "zerolatency", "-g", "10", "-c:a", "aac", "-metadata:s:a:0", "language=eng",
        "-tables_version", "1", "-output_ts_offset", "13", "-f", "mpegts"], &late);
    let mut body = std::fs::read(early).unwrap();
    body.extend(std::fs::read(late).unwrap());
    assert!(body.len() <= 8 * 1024 * 1024, "fixture must fit the unchanged segment body cap");
    std::fs::write(dir.0.join("growing.ts"), body).unwrap();
    let http = HttpFixture::start(dir.0.clone());
    let parsed = plx_plex::plex::StreamUrl::parse(&format!("{}/growing.ts", http.url));
    let segment = crate::hls::Segment {
        sequence: 0, duration: Duration::from_secs(19),
        resource: crate::hls::Resource::new(parsed.origin, parsed.path).unwrap(),
    };
    let mut aq = crate::aq::aq_new(1024 * 1024);
    let mut hs = plx_net::stream::http_stream_boxed();
    let mut net = HlsNet { hs: &mut *hs, curl: None };
    let mut clock = crate::hls::SegmentTimeline::default().begin(segment.duration).unwrap();
    let output = unsafe {
        hls_demux_segment_path(&segment,
            HlsSegmentRequest { path: &segment.resource.path, transport: HlsSegmentTransport::External, body_limit: Some(8 * 1024 * 1024), audio: None, rendition: None, video: HlsVideoEntry::Independent, mp2: None },
            &mut clock, &mut *aq, &mut net, "aac", SegmentAcquisition::for_cursor(None))
    }.unwrap_or_else(|_| panic!("late PMT must not invalidate the retained audio AVStream"));
    assert_eq!(HLS_TEST_STREAM_COUNTS.with(|counts| counts.get()), (2, 6),
        "fixture must grow the stream table after probing, not merely contain several tracks");
    assert!(output.aus.iter().filter(|au| au.es == 1).count() >= 175);
    let audio: Vec<_> = output.aus.iter().filter(|au| au.es == 2).collect();
    assert!(audio.len() > 800, "both sides of the PMT change must retain audio");
    assert!(audio.last().unwrap().pts_ns > 18_000_000_000);
    let file = dir.0.join("selected.aac");
    std::fs::write(&file, audio.iter().flat_map(|au| au.data.iter().copied()).collect::<Vec<_>>()).unwrap();
    let decoded = std::process::Command::new("ffmpeg").args(["-v", "error", "-i"])
        .arg(file).args(["-ac", "1", "-ar", "48000", "-f", "s16le", "pipe:1"]).output().unwrap();
    assert!(decoded.status.success());
    let samples: Vec<i16> = decoded.stdout.chunks_exact(2).map(|b| i16::from_le_bytes([b[0], b[1]])).collect();
    assert!(samples.len() > 17 * 48_000);
    // Confirm the retained stream, rather than a newly discovered 880 Hz stream,
    // continues to produce the final three seconds of selected audio.
    let tail = &samples[samples.len() - 3 * 48_000..];
    let hz = tail.windows(2).filter(|w| w[0] <= 0 && w[1] > 0).count() as f64 / 3.0;
    assert!((180.0..260.0).contains(&hz), "selected audio changed: {hz} Hz");
    plx_net::stream::http_close(&mut *hs);
    crate::aq::aq_destroy(&mut *aq);
}

/// Separate renditions must retain their common MPEG clock. This entry never
/// feeds a decoder; the later presentation scheduler owns synchronization.
#[test]
#[ignore = "requires bundled host FFmpeg, ffmpeg/libx264 and ffprobe CLI"]
fn hls_rendition_lane_extraction_keeps_source_timing_and_rejects_shape_changes() {
    use super::hls_rendition::{Binding, Lane, TimedLaneSegment};
    let _serial = plx_base::testlock::serial();
    let _ff = super::image_subtitle_tests::bind_host_ffmpeg();
    assert!(plx_net::net::global_init());
    crate::curlio::boot();
    SHARED.reset_session();
    struct AudioFeed(bool);
    impl Drop for AudioFeed {
        fn drop(&mut self) { FEED_AUDIO.store(self.0, Ordering::Relaxed); }
    }
    // Extraction must not lose the audio lane just because the current player
    // is muted/has not selected its next native decoder yet.
    let _audio_feed = AudioFeed(FEED_AUDIO.swap(false, Ordering::Relaxed));
    let dir = Scratch::new();
    let run = |args: &[&str], output: &Path| {
        let result = std::process::Command::new("ffmpeg").args(["-v", "error"])
            .args(args).args(["-y"]).arg(output).output().unwrap();
        assert!(result.status.success(), "fixture: {}", String::from_utf8_lossy(&result.stderr));
    };
    let audio = dir.0.join("audio.ts");
    run(&["-f", "lavfi", "-i", "sine=frequency=880:sample_rate=48000", "-t", "1",
        "-c:a", "aac", "-metadata:s:a:0", "language=spa", "-output_ts_offset", "123.45679",
        "-f", "mpegts"], &audio);
    let audio44100 = dir.0.join("audio44100.ts");
    run(&["-f", "lavfi", "-i", "sine=frequency=880:sample_rate=44100", "-t", "1",
        "-c:a", "aac", "-metadata:s:a:0", "language=spa", "-output_ts_offset", "123.45679",
        "-f", "mpegts"], &audio44100);
    run(&["-i", audio.to_str().unwrap(), "-c", "copy", "-metadata:s:a:0", "language=eng",
        "-f", "mpegts"], &dir.0.join("other-language.ts"));
    run(&["-i", audio.to_str().unwrap(), "-c", "copy", "-f", "adts"], &dir.0.join("raw.aac"));
    let video = dir.0.join("video.ts");
    run(&["-f", "lavfi", "-i", "testsrc2=size=160x90:rate=24", "-t", "1", "-an",
        "-c:v", "libx264", "-preset", "fast", "-bf", "2", "-x264-params", "b-adapt=0",
        "-g", "24", "-output_ts_offset", "123.45679", "-f", "mpegts"], &video);
    run(&["-i", video.to_str().unwrap(), "-i", audio.to_str().unwrap(),
        "-map", "0:v:0", "-map", "1:a:0", "-c", "copy", "-f", "mpegts"], &dir.0.join("muxed.ts"));
    let http = HttpFixture::start(dir.0.clone());
    let mut aq = crate::aq::aq_new(1024 * 1024);
    let mut hs = plx_net::stream::http_stream_boxed();
    let mut net = HlsNet { hs: &mut *hs, curl: None };
    let fetch = |name: &str, lane: Lane, expected: Option<&Binding>, aq: *mut AuQueue, net: &mut HlsNet| {
        let parsed = plx_plex::plex::StreamUrl::parse(&format!("{}/{name}", http.url));
        let source = crate::hls::Segment {
            sequence: 7, duration: Duration::from_secs(1),
            resource: crate::hls::Resource::new(parsed.origin, parsed.path).unwrap(),
        };
        unsafe { hls_rendition::read_segment(&source, lane, expected, aq, net, SegmentAcquisition::for_cursor(None)) }
    };
    let actual_audio = fetch("audio.ts", Lane::Audio, None, &mut *aq, &mut net)
        .unwrap_or_else(|error| panic!("audio-only TS refused: {error:?}"));
    let actual_video = fetch("video.ts", Lane::Video, None, &mut *aq, &mut net)
        .unwrap_or_else(|error| panic!("video-only TS refused: {error:?}"));
    assert!(!actual_audio.samples.is_empty());
    assert_eq!(actual_audio.binding.lane(), Lane::Audio);
    assert_eq!(actual_video.binding.lane(), Lane::Video);
    let compare_clock = |path: &Path, actual: &TimedLaneSegment| {
        let probe = std::process::Command::new("ffprobe")
            .args(["-v", "error", "-show_packets", "-show_entries", "packet=pts,dts,duration",
                "-of", "compact=p=0:nk=0"]).arg(path).output().unwrap();
        assert!(probe.status.success());
        let text = String::from_utf8(probe.stdout).unwrap();
        let packets: Vec<_> = text.lines().filter(|line| line.starts_with("pts=")).collect();
        assert_eq!(actual.samples.len(), packets.len());
        for (sample, line) in actual.samples.iter().zip(packets) {
            let number = |key: &str| line.split('|').find_map(|part| part.strip_prefix(key)).and_then(|n| n.parse::<i64>().ok());
            assert_eq!(Some(sample.timing.pts), number("pts="));
            assert_eq!(sample.timing.dts, number("dts="));
            assert_eq!(sample.timing.duration, number("duration="));
            assert_eq!(sample.timing.time_base, (1, 90_000));
            assert!(!sample.data.is_empty());
        }
        assert!(actual.samples[0].timing.pts > 100 * 90_000, "original epoch was zeroed");
    };
    compare_clock(&audio, &actual_audio);
    compare_clock(&video, &actual_video);
    let fractional = fetch("audio44100.ts", Lane::Audio, None, &mut *aq, &mut net)
        .unwrap_or_else(|error| panic!("44.1 kHz audio-only TS refused: {error:?}"));
    let probe = std::process::Command::new("ffprobe")
        .args(["-v", "error", "-show_packets", "-show_entries", "packet=pts,dts,duration",
            "-of", "compact=p=0:nk=0"]).arg(&audio44100).output().unwrap();
    assert!(probe.status.success());
    let probe_text = String::from_utf8(probe.stdout).unwrap();
    let probe_packets: Vec<_> = probe_text.lines().filter(|line| line.starts_with("pts=")).collect();
    assert_eq!(fractional.samples.len(), probe_packets.len());
    assert!(fractional.samples.len() > 40, "fixture must span many fractional AAC steps");
    assert!(fractional.samples[0].timing.pts > 100 * 90_000, "original epoch was zeroed");
    assert!(fractional.samples.windows(2).all(|w| w[0].timing.pts < w[1].timing.pts));
    for (sample, line) in fractional.samples.iter().zip(&probe_packets) {
        let number = |key: &str| line.split('|').find_map(|part| part.strip_prefix(key))
            .and_then(|n| n.parse::<i64>().ok()).unwrap();
        // 1024*90000/44100 is fractional. Anchored cumulative rounding and
        // FFmpeg's absolute-lattice av_add_stable can differ by one raw tick;
        // the bound is per packet and cannot grow with the number of frames.
        assert!(sample.timing.pts.abs_diff(number("pts=")) <= 1);
        assert!(sample.timing.dts.unwrap().abs_diff(number("dts=")) <= 1);
        let duration = sample.timing.duration.unwrap();
        assert!(duration > 0);
        // FFmpeg reports floor(duration); an inferred endpoint difference may
        // use its ceiling. Preserve provided durations rather than rewriting them.
        assert!(duration.abs_diff(number("duration=")) <= 1);
        assert_eq!(sample.timing.time_base, (1, 90_000));
        assert!(!sample.data.is_empty());
    }
    let final_sample = &fractional.samples.last().unwrap().timing;
    let final_probe = probe_packets.last().unwrap();
    let number = |key: &str| final_probe.split('|').find_map(|part| part.strip_prefix(key))
        .and_then(|n| n.parse::<i64>().ok()).unwrap();
    let actual_end = final_sample.pts.checked_add(final_sample.duration.unwrap()).unwrap();
    let probe_end = number("pts=").checked_add(number("duration=")).unwrap();
    // Endpoint error is at most one timestamp tick plus one duration tick.
    assert!(actual_end.abs_diff(probe_end) <= 2);
    assert!(actual_video.samples.iter().any(|s| s.timing.dts != Some(s.timing.pts)), "fixture must contain B frames");
    assert!(actual_video.samples.windows(2).any(|w| w[1].timing.pts < w[0].timing.pts), "decode order must not be sorted by PTS");
    assert!(fetch("other-language.ts", Lane::Audio, Some(&actual_audio.binding), &mut *aq, &mut net).is_err());
    assert!(fetch("muxed.ts", Lane::Video, None, &mut *aq, &mut net).is_err());
    assert!(fetch("muxed.ts", Lane::Audio, None, &mut *aq, &mut net).is_err());
    assert!(fetch("raw.aac", Lane::Audio, None, &mut *aq, &mut net).is_err(), "raw ADTS is outside TS-only support");
    assert!(fetch("video.ts", Lane::Audio, None, &mut *aq, &mut net).is_err());
    // This internal entry does not make normal audio-only files playable videos.
    assert!(inspect::source(&ExternalSource::Local(audio), Arc::new(AtomicBool::new(false))).is_err());
    http.requests.lock().unwrap().clear();
    crate::aq::aq_abort(&mut *aq);
    assert!(matches!(fetch("audio.ts", Lane::Audio, None, &mut *aq, &mut net), Err(HlsExit::Aborted)));
    assert!(http.requests.lock().unwrap().is_empty(), "cancelled generation must perform no fetch");
    plx_net::stream::http_close(&mut *hs);
    crate::aq::aq_destroy(&mut *aq);
}

#[test]
#[ignore = "requires bundled host FFmpeg, ffmpeg/libx264 and ffprobe CLI; synthetic localhost fixtures only"]
fn external_hls_renditions_join_real_audio_video_and_switch_language() {
    check_external_hls_audio_presentation(false, false);
}

#[test]
#[ignore = "requires bundled host FFmpeg, ffmpeg/libx264 and ffprobe CLI; synthetic localhost fixtures only"]
fn external_hls_subtitle_presence_does_not_block_audio_video_or_fetch_subtitles() {
    check_external_hls_audio_presentation(true, false);
}

#[test]
#[ignore = "requires bundled host FFmpeg, ffmpeg/libx264 and ffprobe CLI; synthetic localhost fixtures only"]
fn external_hls_webvtt_selection_uses_probed_clock_and_preserves_av_on_failure() {
    check_external_hls_audio_presentation(true, true);
}

fn check_external_hls_audio_presentation(with_subtitles: bool, exercise_subtitles: bool) {
    let _serial = plx_base::testlock::serial();
    let _ff = super::image_subtitle_tests::bind_host_ffmpeg();
    assert!(plx_net::net::global_init());
    crate::curlio::boot();
    struct AudioFeed(bool);
    impl Drop for AudioFeed { fn drop(&mut self) { FEED_AUDIO.store(self.0, Ordering::Relaxed); } }
    let _feed = AudioFeed(FEED_AUDIO.swap(true, Ordering::Relaxed));
    let dir = Scratch::new();
    let run = |args: &[&str], output: &Path| {
        let result = std::process::Command::new("ffmpeg").args(["-v", "error"])
            .args(args).args(["-y"]).arg(output).output().unwrap();
        assert!(result.status.success(), "fixture: {}", String::from_utf8_lossy(&result.stderr));
    };
    run(&["-f", "lavfi", "-i", "testsrc2=size=160x90:rate=25", "-t", "2", "-an",
        "-c:v", "libx264", "-preset", "fast", "-bf", "2", "-g", "25", "-sc_threshold", "0",
        "-f", "segment", "-segment_time", "1", "-segment_format", "mpegts"], &dir.0.join("v%02d.ts"));
    for (prefix, tone, language) in [("en", "sine=frequency=220:sample_rate=48000", "eng"),
        ("es", "sine=frequency=880:sample_rate=48000", "spa")]
    {
        run(&["-f", "lavfi", "-i", tone, "-t", "2", "-c:a", "aac", "-metadata:s:a:0", &format!("language={language}"),
            "-f", "segment", "-segment_time", "0.5", "-segment_format", "mpegts"], &dir.0.join(format!("{prefix}%02d.ts")));
    }
    let playlist = |prefix: &str, sequence: usize| {
        let mut paths: Vec<_> = std::fs::read_dir(&dir.0).unwrap().map(|p| p.unwrap().path())
            .filter(|p| p.file_name().unwrap().to_string_lossy().starts_with(prefix) && p.extension().is_some_and(|s| s == "ts")).collect();
        paths.sort();
        let mut out = format!("#EXTM3U\n#EXT-X-TARGETDURATION:2\n#EXT-X-MEDIA-SEQUENCE:{sequence}\n");
        let mut elapsed = 0i64;
        let mut anchor_90k = None;
        for path in paths {
            let probe = std::process::Command::new("ffprobe")
                .args(["-v", "error", "-show_packets", "-show_streams", "-show_entries", "stream=time_base:packet=pts,duration", "-of", "json"])
                .arg(&path).output().unwrap();
            assert!(probe.status.success());
            let json: serde_json::Value = serde_json::from_slice(&probe.stdout).unwrap();
            let tb = json["streams"][0]["time_base"].as_str().unwrap();
            let (num, den) = tb.split_once('/').unwrap();
            let (num, den) = (num.parse::<i128>().unwrap(), den.parse::<i128>().unwrap());
            let packets = json["packets"].as_array().unwrap();
            let first = packets.iter().map(|p| p["pts"].as_i64().unwrap()).min().unwrap();
            anchor_90k.get_or_insert_with(|| i64::try_from(i128::from(first) * num * 90_000 / den).unwrap());
            let last = packets.iter().map(|p| p["pts"].as_i64().unwrap() + p["duration"].as_i64().unwrap()).max().unwrap();
            let duration = i64::try_from(i128::from(last - first) * num * 1_000_000_000 / den).unwrap();
            out.push_str(&format!("#EXT-X-PROGRAM-DATE-TIME:2026-01-01T00:00:{:02}.{:09}Z\n#EXTINF:{}.{:09},\n{}\n",
                elapsed / 1_000_000_000, elapsed % 1_000_000_000, duration / 1_000_000_000,
                duration % 1_000_000_000, path.file_name().unwrap().to_str().unwrap()));
            elapsed += duration;
        }
        out.push_str("#EXT-X-ENDLIST\n");
        std::fs::write(dir.0.join(format!("{prefix}.m3u8")), out).unwrap();
        anchor_90k.expect("generated lane has packets")
    };
    let video_anchor_90k = playlist("v", 41);
    playlist("en", 900); playlist("es", 7);
    let mut master = String::from("#EXTM3U\n#EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID=\"a\",NAME=\"English\",LANGUAGE=\"eng\",DEFAULT=YES,URI=\"en.m3u8\"\n#EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID=\"a\",NAME=\"Spanish\",LANGUAGE=\"spa\",DEFAULT=NO,URI=\"es.m3u8\"\n#EXT-X-STREAM-INF:BANDWIDTH=100000,AUDIO=\"a\"\nv.m3u8\n");
    if with_subtitles {
        master = master.replacen("#EXTM3U\n", "#EXTM3U\n#EXT-X-MEDIA:TYPE=SUBTITLES,GROUP-ID=\"subs\",NAME=\"English captions\",LANGUAGE=\"eng\",DEFAULT=YES,AUTOSELECT=YES,URI=\"unselected-captions.m3u8\"\n", 1)
            .replace("AUDIO=\"a\"\n", "AUDIO=\"a\",SUBTITLES=\"subs\"\n");
    }
    if exercise_subtitles {
        assert!(with_subtitles);
        master = master.replacen("#EXTM3U\n", "#EXTM3U\n#EXT-X-MEDIA:TYPE=SUBTITLES,GROUP-ID=\"subs\",NAME=\"Selected captions\",LANGUAGE=\"fra\",URI=\"selected-captions.m3u8\"\n#EXT-X-MEDIA:TYPE=SUBTITLES,GROUP-ID=\"subs\",NAME=\"Missing captions\",LANGUAGE=\"deu\",URI=\"missing-captions.m3u8\"\n", 1);
        std::fs::write(dir.0.join("selected-captions.m3u8"),
            "#EXTM3U\n#EXT-X-TARGETDURATION:2\n#EXT-X-MEDIA-SEQUENCE:0\n#EXTINF:2,\nselected-caption.vtt\n#EXT-X-ENDLIST\n").unwrap();
        // This independent CLI probe supplies fixture timestamps. Production
        // inspect must recover its own video anchor from the bundled demuxer.
        std::fs::write(dir.0.join("selected-caption.vtt"), format!(
            "WEBVTT\nX-TIMESTAMP-MAP=LOCAL:00:00:00.000,MPEGTS:{video_anchor_90k}\n\n00:00.500 --> 00:01.500\nSelected <b>WebVTT</b> caption\n"
        )).unwrap();
    }
    std::fs::write(dir.0.join("master.m3u8"), master).unwrap();
    let http = HttpFixture::start(dir.0.clone());
    let url = format!("{}/master.m3u8", http.url);
    let info = inspect::http(&url, Arc::new(AtomicBool::new(false))).unwrap();
    assert!(info.tracks.subtitles.is_empty(), "HLS subtitle groups must not become AVStream tracks");
    if exercise_subtitles {
        use crate::{player::{sidecar, text_style::TextStyle}, route};
        let _globals = route::HlsTrackRequest::test_globals_guard();
        let mut ps = route::PlaybackSession::IDLE;
        route::reset_player_control_for_test(&ps);
        route::request_external_source(&mut ps, &info, ExternalSource::Hls(url.clone()),
            "Fixture", "M3U", plx_platform::devcaps::assumed_shared()).unwrap();
        route::reset_player_control_for_test(&ps);
        let catalog = route::hls_track_catalog(&ps).unwrap();
        let rows = catalog.subtitle_rows();
        assert_eq!(rows.len(), 3, "real video inspection must make declared captions available");
        assert_eq!(catalog.selected_subtitle(), None);
        assert!(http.requests.lock().unwrap().iter().all(|path| !path.contains("caption")),
            "inspection must not fetch any unselected subtitle URL");
        let selected = rows.iter().find(|row| row.language == "fra").unwrap().id;
        let missing = rows.iter().find(|row| row.language == "deu").unwrap().id;
        let audio = catalog.rows();
        let identity = route::playback_identity(&ps);
        let worker = route::worker_ticket();
        let state = crate::player::state(&ps);
        let position = SHARED.playpos_ns.load(Ordering::Acquire);
        let audio_switch = crate::player::audio_switching();
        let request = route::begin_hls_track_request(&mut ps,
            &catalog.subtitle_choice(Some(selected)).unwrap()).unwrap();
        let off = plx_base::task::OffFrame::for_test();
        let prepared = request.prepare(&off, Arc::new(AtomicBool::new(false))).unwrap();
        assert_eq!(route::hls_track_catalog(&ps).unwrap().selected_subtitle(), None,
            "network and sidecar preparation cannot publish selection");
        assert!(route::complete_hls_track_request(&mut ps, &request, prepared).unwrap());
        let style = TextStyle::current(route::subtitle_text_appearance(&ps));
        let source = sidecar::ass_source(false, style).expect("shared subtitle renderer source");
        let crate::player::ass::Content::Script { bytes, .. } = &source.content else {
            panic!("WebVTT must reach the shared text script");
        };
        let text = std::str::from_utf8(bytes).unwrap();
        assert!(text.contains("0:00:00.50,0:00:01.50"), "PES clock must map into presentation time");
        assert!(text.contains("Selected {\\b1}WebVTT{\\b0} caption"));
        assert_eq!(route::hls_track_catalog(&ps).unwrap().selected_subtitle(), Some(selected));
        {
            let paths = http.requests.lock().unwrap();
            assert!(paths.iter().any(|path| path == "/selected-captions.m3u8"));
            assert!(paths.iter().any(|path| path == "/selected-caption.vtt"));
            assert!(!paths.iter().any(|path| path == "/missing-captions.m3u8"));
        }
        let failed = route::begin_hls_track_request(&mut ps,
            &catalog.subtitle_choice(Some(missing)).unwrap()).unwrap();
        assert!(failed.prepare(&off, Arc::new(AtomicBool::new(false))).is_err(),
            "missing selected subtitle playlist must fail preparation");
        assert!(http.requests.lock().unwrap().iter().any(|path| path == "/missing-captions.m3u8"));
        assert_eq!(route::hls_track_catalog(&ps).unwrap().selected_subtitle(), Some(selected));
        assert!(Arc::ptr_eq(&source, &sidecar::ass_source(false, style).unwrap()),
            "failed fetch must leave the exact prior renderer source installed");
        assert_eq!(route::hls_track_catalog(&ps).unwrap().rows(), audio);
        assert_eq!(route::playback_identity(&ps), identity);
        assert_eq!(route::worker_ticket(), worker);
        assert_eq!(crate::player::state(&ps), state);
        assert_eq!(SHARED.playpos_ns.load(Ordering::Acquire), position);
        assert_eq!(crate::player::audio_switching(), audio_switch);
    }
    let english = info.hls_playback.unwrap();
    assert_eq!(english.rows().count(), 2);
    let spanish_id = english.rows().find(|(_, _, lang)| *lang == "spa").unwrap().0;
    let spanish = english.prepare_rendition(spanish_id, Arc::new(AtomicBool::new(false))).unwrap();
    let play = |selected: hls_presentation::HlsPlayback, seek: i64| {
        SHARED.reset_session();
        SHARED.seek_to_ns.store(seek, Ordering::Release);
        let mut video = crate::aq::aq_new(1024 * 1024);
        let mut audio = crate::aq::aq_new(1024 * 1024);
        let mut hs = plx_net::stream::http_stream_boxed();
        let (v, a, h) = (SendPtr(&mut *video), SendPtr(&mut *audio), SendPtr(&mut *hs));
        let source = ExternalSource::Hls(plx_data::ott::http_url(&url).unwrap());
        let worker = std::thread::spawn(move || {
            let off = plx_base::task::OffFrame::for_test();
            demux_external(&off, 0, source, "h264".into(), "aac".into(), Some(HlsTrackSelection::Rendition(selected)),
                v, a, h, crate::player::text_style::TextStyle::current(Default::default()));
        });
        let mut video_pts = Vec::new();
        let mut audio_pts = Vec::new();
        let mut encoded = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            let mut drained = false;
            for (queue, pts, is_audio) in [(&mut *video, &mut video_pts, false), (&mut *audio, &mut audio_pts, true)] {
                let node = crate::aq::aq_pop(queue, std::ptr::null_mut());
                if !node.is_null() {
                    drained = true;
                    unsafe {
                        let (_, _, stamp, len, data) = crate::aq::au_fields(node);
                        pts.push(stamp);
                        if is_audio { encoded.extend_from_slice(std::slice::from_raw_parts(data, len as usize)); }
                        libc::free(node.cast());
                    }
                }
            }
            if worker.is_finished() && !drained { break; }
            if Instant::now() >= deadline { break; }
            if !drained { std::thread::sleep(Duration::from_millis(2)); }
        }
        crate::aq::aq_abort(&mut *video); crate::aq::aq_abort(&mut *audio);
        crate::curlio::abort_active(); plx_net::stream::http_shutdown(&mut *hs);
        worker.join().unwrap();
        plx_net::stream::http_close(&mut *hs);
        crate::aq::aq_destroy(&mut *video); crate::aq::aq_destroy(&mut *audio);
        assert!(!SHARED.demux_failed.load(Ordering::Acquire));
        assert!(!SHARED.demux_io_failed.load(Ordering::Acquire));
        assert!(video_pts.len() >= 20 && audio_pts.len() >= 40);
        (video_pts, audio_pts, encoded)
    };
    let tone = |name: &str, data: Vec<u8>| {
        let path = dir.0.join(name); std::fs::write(&path, data).unwrap();
        let pcm = std::process::Command::new("ffmpeg").args(["-v", "error", "-i"]).arg(path)
            .args(["-ac", "1", "-ar", "48000", "-f", "s16le", "pipe:1"]).output().unwrap();
        assert!(pcm.status.success());
        let samples: Vec<i16> = pcm.stdout.chunks_exact(2).map(|b| i16::from_le_bytes([b[0],b[1]])).collect();
        let trim = &samples[2400..samples.len() - 2400];
        trim.windows(2).filter(|w| w[0] <= 0 && w[1] > 0).count() as f64 * 48_000.0 / trim.len() as f64
    };
    let (v, a, data) = play(english, 0);
    assert_eq!(v[0], 0); assert_eq!(a[0], 0);
    assert!((180.0..260.0).contains(&tone("joined-en.aac", data)));
    let (_, _, data) = play(spanish.clone(), 0);
    assert!((800.0..960.0).contains(&tone("joined-es.aac", data)));
    let (v, a, _) = play(spanish.clone(), 1_200_000_000);
    assert!(v[0] >= 1_000_000_000 && v[0] < 1_200_000_000);
    assert!(a[0] >= 970_000_000 && a[0] <= 1_020_000_000);
    std::fs::remove_file(dir.0.join("es.m3u8")).unwrap();
    assert!(spanish.prepare_rendition(spanish_id, Arc::new(AtomicBool::new(false))).is_err(), "missing selected language must not fall back to English");
    assert!(http.requests.lock().unwrap().iter().all(|path| !path.contains("unselected-captions")),
        "unselected subtitle playlists must never be fetched");
}


/// The inspected external HLS source must use the same guarded redirect contract
/// when its first segment is opened by the actual playback demuxer. A direct HLS
/// control proves the TS fixture meets that demuxer's framing requirements first.
#[test]
#[ignore = "requires bundled host FFmpeg and ffmpeg CLI"]
fn external_hls_first_segment_cross_origin_redirect_reaches_actual_video_aus() {
    let _serial = plx_base::testlock::serial();
    let _ff = super::image_subtitle_tests::bind_host_ffmpeg();
    assert!(plx_net::net::global_init());
    crate::curlio::boot();
    struct RestoreFeed(bool);
    impl Drop for RestoreFeed {
        fn drop(&mut self) { FEED_AUDIO.store(self.0, Ordering::Relaxed); }
    }
    let _feed = RestoreFeed(FEED_AUDIO.load(Ordering::Relaxed));
    let dir = Scratch::new();
    fixture(&dir.0, "ts");
    let playlist = |segment: &str| format!(
        "#EXTM3U\n#EXT-X-TARGETDURATION:8\n#EXT-X-MEDIA-SEQUENCE:0\n#EXTINF:8,\n{segment}\n#EXT-X-ENDLIST\n"
    );
    std::fs::write(dir.0.join("direct.m3u8"), playlist("fixture.ts")).unwrap();
    std::fs::write(
        dir.0.join("redirect.m3u8"),
        playlist("segment-redirect?token=origin-only-test-token"),
    ).unwrap();
    let target_heads = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let target_seen = Arc::clone(&target_heads);
    let target = HttpFixture::start_with(dir.0.clone(), move |head| {
        target_seen.lock().unwrap().push(head.to_owned());
        None
    });
    let location = format!("{}/fixture.ts?sig=target-only-test-signature", target.url);
    let origin_heads = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let origin_seen = Arc::clone(&origin_heads);
    let origin = HttpFixture::start_with(dir.0.clone(), move |head| {
        origin_seen.lock().unwrap().push(head.to_owned());
        head.lines().next().unwrap_or_default().starts_with("GET /segment-redirect?")
            .then(|| format!(
                "HTTP/1.1 307 Temporary Redirect\r\nLocation: {location}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            ).into_bytes())
    });

    let direct = decode(ExternalSource::Hls(format!("{}/direct.m3u8", target.url)), 0);
    assert!(direct.len() >= 20, "direct HLS control must actually emit video AUs");
    target_heads.lock().unwrap().clear();
    let url = format!("{}/redirect.m3u8", origin.url);
    let info = inspect::http(&url, Arc::new(AtomicBool::new(false)))
        .expect("inspection must follow the same first-segment redirect");
    assert_eq!(info.video_codec, "h264");
    let Some(source @ ExternalSource::Hls(_)) = info.resolved_source else {
        panic!("inspection did not retain an HLS source");
    };
    assert!(!target_heads.lock().unwrap().is_empty(), "inspection reached the target TS");

    // The internal PMS opener keeps its stricter same-origin contract. This is
    // the existing production entry, not a new policy type available only later.
    let before_pms = target_heads.lock().unwrap().len();
    let parsed = plx_plex::plex::StreamUrl::parse(&format!(
        "{}/segment-redirect?token=origin-only-test-token", origin.url
    ));
    let resource = crate::hls::Resource::new(parsed.origin, parsed.path).unwrap();
    let mut aq = crate::aq::aq_new(1024 * 1024);
    let mut hs = plx_net::stream::http_stream_boxed();
    let mut net = HlsNet { hs: &mut *hs, curl: None };
    let opened = hls_open_source(
        &resource, &resource.path, &mut *aq, &mut net,
        Some(Instant::now() + Duration::from_secs(2)),
        &mut plx_base::checkpoint::NoCheckpoint,
    );
    let refused = matches!(&opened, Err(HlsExit::Failed("HTTP redirect refused")));
    drop(opened);
    drop(net);
    plx_net::stream::http_close(&mut *hs);
    crate::aq::aq_destroy(&mut *aq);
    assert!(refused, "PMS same-origin restriction must remain enforced");
    assert_eq!(target_heads.lock().unwrap().len(), before_pms, "PMS must not dial the redirected origin");

    target_heads.lock().unwrap().clear();
    origin_heads.lock().unwrap().clear();
    let redirected = decode(source, 0);
    assert!(redirected.len() >= 20, "redirected external HLS must actually emit video AUs");
    let target_requests = target_heads.lock().unwrap();
    assert!(!target_requests.is_empty(), "playback, not only inspection, reached the target TS");
    for head in target_requests.iter() {
        assert_eq!(head.lines().next(), Some("GET /fixture.ts?sig=target-only-test-signature HTTP/1.1"));
        assert!(!head.contains("origin-only-test-token"), "the original URL credential must not be copied to another origin");
        assert!(!head.to_ascii_lowercase().lines().any(|line|
            line.starts_with("range:") || line.starts_with("authorization:") || line.starts_with("x-plex-token:")
        ), "the segment GET carries no Range or forwarded credential header");
    }
    let origin_requests = origin_heads.lock().unwrap();
    assert!(origin_requests.iter().any(|head| head.lines().next() ==
        Some("GET /segment-redirect?token=origin-only-test-token HTTP/1.1")));
    assert!(origin_requests.iter().all(|head|
        !head.to_ascii_lowercase().lines().any(|line| line.starts_with("range:"))
    ), "playback playlist and first segment open at byte zero without Range");
}

fn entry_fixture_command(program: &str, args: &[&str], clean_stderr: bool) -> Vec<u8> {
    let out = std::process::Command::new(program).args(args).output()
        .unwrap_or_else(|error| panic!("required fixture program {program}: {error}"));
    assert!(out.status.success(), "{program}: {}", String::from_utf8_lossy(&out.stderr));
    if clean_stderr {
        assert!(out.stderr.is_empty(), "fixture must decode cleanly: {}", String::from_utf8_lossy(&out.stderr));
    }
    out.stdout
}

/// One continuous encoded stream, cut on time rather than forced random-access points.
/// Repeated parameter sets let every segment be probed independently; they do not
/// turn the P-slices at the second segment's head into independent pictures.
fn entry_fixture(dir: &Path, gop: &str) -> String {
    entry_fixture_with_audio(dir, gop, false)
}

fn entry_fixture_with_audio(dir: &Path, gop: &str, audio: bool) -> String {
    entry_fixture_with_length(dir, gop, audio, "3", 3)
}
fn entry_fixture_with_length(dir: &Path, gop: &str, audio: bool, seconds: &str, count: usize) -> String {
    let source = dir.join("source.mp4");
    let mut encode = vec!["-v", "error", "-nostdin", "-y",
        "-f", "lavfi", "-i", "testsrc2=size=160x90:rate=10"];
    if audio {
        encode.extend(["-f", "lavfi", "-i", "sine=frequency=440:sample_rate=48000",
            "-map", "0:v:0", "-map", "1:a:0", "-c:a", "aac", "-ar", "48000", "-ac", "1"]);
    } else {
        encode.push("-an");
    }
    encode.extend(["-t", seconds, "-c:v", "libx264", "-preset", "ultrafast", "-tune", "zerolatency",
        "-g", gop, "-keyint_min", gop, "-sc_threshold", "0", "-bf", "0", source.to_str().unwrap()]);
    entry_fixture_command("ffmpeg", &encode, true);
    let index = dir.join("index.m3u8");
    let segments = dir.join("part-%d.ts");
    let mut split = vec!["-v", "error", "-nostdin", "-y",
        "-i", source.to_str().unwrap(), "-map", "0:v:0", "-c:v", "copy"];
    if audio {
        split.extend(["-map", "0:a:0", "-c:a", "copy"]);
    } else {
        split.push("-an");
    }
    split.extend(["-bsf:v", "h264_mp4toannexb,dump_extra=freq=all", "-hls_time", "1",
        "-hls_flags", "split_by_time", "-hls_playlist_type", "vod",
        "-hls_segment_filename", segments.to_str().unwrap(), index.to_str().unwrap()]);
    entry_fixture_command("ffmpeg", &split, true);
    let text = std::fs::read_to_string(&index).unwrap();
    assert!(!text.contains("#EXT-X-INDEPENDENT-SEGMENTS"));
    assert_eq!(text.lines().filter(|line| line.ends_with(".ts")).count(), count);
    let bytes: u64 = (0..count).map(|i| std::fs::metadata(dir.join(format!("part-{i}.ts"))).unwrap().len()).sum();
    assert!(bytes < 256 * 1024, "tiny synchronous fixture must fit comfortably in its AU queue");
    // A control independent of our segment decoder: the complete stream is decodable.
    entry_fixture_command("ffmpeg", &["-v", "error", "-nostdin", "-i", index.to_str().unwrap(),
        "-f", "null", "-"], true);
    text
}

fn entry_packet_nals(path: &Path) -> Vec<(i64, Vec<u8>)> {
    let raw = entry_fixture_command("ffprobe", &["-v", "error", "-select_streams", "v:0",
        "-show_packets", "-show_data", "-show_streams", "-show_entries",
        "packet=pts_time,data:stream=width,height", "-of", "json", path.to_str().unwrap()], false);
    let value: serde_json::Value = serde_json::from_slice(&raw).unwrap();
    assert_eq!(value["streams"][0]["width"], 160);
    assert_eq!(value["streams"][0]["height"], 90);
    value["packets"].as_array().unwrap().iter().map(|packet| {
        let pts = (packet["pts_time"].as_str().unwrap().parse::<f64>().unwrap() * 1e9).round() as i64;
        let mut bytes = Vec::new();
        for line in packet["data"].as_str().unwrap().lines() {
            let Some((_, tail)) = line.split_once(':') else { continue; };
            let hex: String = tail.trim_start().split("  ").next().unwrap().split_whitespace().collect();
            for pair in hex.as_bytes().chunks_exact(2) {
                bytes.push(u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap());
            }
        }
        let mut cursor = 0;
        let mut nals = Vec::new();
        while let Some((start, prefix)) = annexb_start(&bytes, cursor) {
            cursor = start + prefix;
            assert!(cursor < bytes.len(), "empty NAL in generated fixture");
            nals.push(bytes[cursor] & 0x1f);
            cursor += 1;
        }
        (pts, nals)
    }).collect()
}

/// Return the real producer Result and drain the complete bounded finite stream.
/// The generic decode helper intentionally stops after 30 AUs and cannot prove this contract.
fn entry_demux(http: &HttpFixture, name: &str, seek: i64) -> (Result<(), HlsExit>, Vec<(i64, i32)>, bool) {
    let (result, aus, failed) = entry_demux_tracks(http, name, seek, false);
    (result, aus.into_iter().map(|(_, pts, key)| (pts, key)).collect(), failed)
}

fn entry_demux_tracks(http: &HttpFixture, name: &str, seek: i64, audio_enabled: bool)
    -> (Result<(), HlsExit>, Vec<(i32, i64, i32)>, bool)
{
    entry_demux_tracks_with_abort(http, name, seek, audio_enabled, false, None)
}
fn entry_demux_tracks_with_abort(http: &HttpFixture, name: &str, seek: i64, audio_enabled: bool, aborted: bool, source_offset: Option<(i64, bool)>)
    -> (Result<(), HlsExit>, Vec<(i32, i64, i32)>, bool)
{
    SHARED.reset_session();
    assert!(SHARED.arm_initial_clock_hold(false, false));
    if source_offset.is_some_and(|(_, paused)| paused) {
        assert!(SHARED.prepare_hls_user_pause().is_some());
        assert!(SHARED.hand_hold_to_seek_prime(), "real paused-before-first-frame preroll path");
    }
    SHARED.seek_to_ns.store(seek, Ordering::Release);
    PUSHED_ANY.store(false, Ordering::Relaxed);
    let feed = FEED_AUDIO.swap(audio_enabled, Ordering::Relaxed);
    let mut video = crate::aq::aq_new(16 * 1024 * 1024);
    let mut audio = crate::aq::aq_new(128 * 1024);
    let mut hs = plx_net::stream::http_stream_boxed();
    if aborted { crate::aq::aq_abort(&mut *video); }
    let result = external_hls::demux(&format!("{}/{name}", http.url), if audio_enabled { "aac" } else { "" }, None,
        &mut *video, &mut *audio, &mut *hs, None);
    let mut aus = Vec::new();
    for queue in [&mut *video, &mut *audio] {
        loop {
            let node = crate::aq::aq_pop(queue, std::ptr::null_mut());
            if node.is_null() { break; }
            unsafe {
                let (es, key, pts, _, _) = crate::aq::au_fields(node);
                libc::free(node.cast());
                aus.push((es, pts, key));
            }
        }
    }
    let failed = SHARED.demux_io_failed.load(Ordering::Acquire) || SHARED.demux_failed.load(Ordering::Acquire);
    if let Some((source_offset, _)) = source_offset {
        assert_eq!(SHARED.disp_base.load(Ordering::Acquire), source_offset);
        let epoch = SHARED.begin_native_session().expect("position callback owner");
        assert!(SHARED.arm_native_presentations(epoch));
        for pts in [aus.first().expect("safe first AU").1, aus.last().expect("safe last AU").1] {
            crate::player::sf_on_event(epoch, 0, pts, std::ptr::null());
            assert_eq!(crate::player::playpos_ns(), pts + source_offset,
                "public playback position must remain on the original finite source timeline");
        }
        assert!(SHARED.retire_native_session(epoch));
    }
    plx_net::stream::http_close(&mut *hs);
    crate::aq::aq_destroy(&mut *video);
    crate::aq::aq_destroy(&mut *audio);
    FEED_AUDIO.store(feed, Ordering::Relaxed);
    SHARED.reset_session();
    PUSHED_ANY.store(false, Ordering::Relaxed);
    (result, aus, failed)
}

#[test]
#[ignore = "requires bundled host FFmpeg, ffmpeg/libx264 and ffprobe CLI; synthetic localhost only"]
fn external_hls_continuation_keeps_non_idr_frames_and_resets_entry_proof() {
    let _serial = plx_base::testlock::serial();
    let _ff = super::image_subtitle_tests::bind_host_ffmpeg();
    assert!(plx_net::net::global_init());
    crate::curlio::boot();
    let dir = Scratch::new();
    let text = entry_fixture(&dir.0, "30");
    let later = entry_packet_nals(&dir.0.join("part-1.ts"));
    assert_eq!(later.len(), 10);
    assert!(later[0].1.contains(&7) && later[0].1.contains(&8));
    assert_eq!(later[0].1.iter().copied().find(|n| (1..=5).contains(n)), Some(1));
    assert!(later.iter().all(|(_, nals)| !nals.contains(&5)), "negative entries must have no later IDR to recover to");
    let isolated = "#EXTM3U\n#EXT-X-TARGETDURATION:1\n#EXT-X-MEDIA-SEQUENCE:1\n#EXTINF:1,\npart-1.ts\n#EXT-X-ENDLIST\n";
    std::fs::write(dir.0.join("isolated.m3u8"), isolated).unwrap();
    let mut discontinuity = text.clone();
    let part = discontinuity.find("part-1.ts").unwrap();
    let boundary = discontinuity[..part].rfind("#EXTINF:").unwrap();
    discontinuity.insert_str(boundary, "#EXT-X-DISCONTINUITY\n");
    std::fs::write(dir.0.join("discontinuity.m3u8"), discontinuity).unwrap();
    std::fs::write(dir.0.join("independent.m3u8"),
        text.replacen("#EXTM3U", "#EXTM3U\n#EXT-X-INDEPENDENT-SEGMENTS", 1)).unwrap();
    let http = HttpFixture::start(dir.0.clone());
    let (result, aus, failed) = entry_demux(&http, "index.m3u8", -1);
    assert!(result.is_ok(), "continuous segment entry must not require a new IDR: {result:?}");
    assert!(!failed);
    assert_eq!(aus.len(), 30, "continuation must retain every P-frame across both boundaries");
    assert_eq!(aus.iter().map(|(pts, _)| *pts).collect::<Vec<_>>(),
        (0..30).map(|i| i * 100_000_000).collect::<Vec<i64>>());
    assert_eq!(aus[10].1, 0, "the second segment's first P-frame must survive");
    assert!(http.requests.lock().unwrap().iter().any(|path| path == "/part-2.ts"));
    for (name, seek) in [("isolated.m3u8", -1), ("index.m3u8", 1_000_000_000),
        ("discontinuity.m3u8", -1), ("independent.m3u8", -1)]
    {
        let (result, aus, _) = entry_demux(&http, name, seek);
        assert!(matches!(result, Err(HlsExit::Failed("HLS segment does not begin with an IDR"))),
            "{name} seek={seek} must not borrow continuity across a fresh entry: {result:?}");
        assert!(aus.iter().all(|(pts, _)| *pts < 1_000_000_000), "{name} fed the dependent segment");
    }
}

#[test]
#[ignore = "requires bundled host FFmpeg, ffmpeg/libx264 and ffprobe CLI; synthetic localhost only"]
fn external_hls_cold_entry_skips_prefix_to_idr_without_rebasing_time() {
    let _serial = plx_base::testlock::serial();
    let _ff = super::image_subtitle_tests::bind_host_ffmpeg();
    assert!(plx_net::net::global_init());
    crate::curlio::boot();
    let dir = Scratch::new();
    entry_fixture(&dir.0, "15");
    let packets = entry_packet_nals(&dir.0.join("part-1.ts"));
    assert_eq!(packets.len(), 10);
    assert!(!packets[0].1.contains(&5));
    let first_idr = packets.iter().position(|(_, nals)| nals.contains(&5)).expect("fixture IDR inside second segment");
    assert_eq!(first_idr, 5);
    let offset = packets[first_idr].0 - packets[0].0;
    assert_eq!(offset, 500_000_000);
    std::fs::write(dir.0.join("cold.m3u8"),
        "#EXTM3U\n#EXT-X-TARGETDURATION:1\n#EXT-X-MEDIA-SEQUENCE:1\n#EXTINF:1,\npart-1.ts\n#EXT-X-ENDLIST\n").unwrap();
    let http = HttpFixture::start(dir.0.clone());
    let (result, aus, failed) = entry_demux(&http, "cold.m3u8", -1);
    assert!(result.is_ok(), "cold entry must scan its bounded segment for a true IDR: {result:?}");
    assert!(!failed);
    assert_eq!(aus.len(), packets.len() - first_idr);
    assert_eq!(aus[0], (offset, 1), "skipping dependent pictures must retain the original segment time origin");
    assert!(aus.iter().all(|(pts, _)| *pts >= offset));
}

#[test]
#[ignore = "requires bundled host FFmpeg, ffmpeg/libx264/AAC and ffprobe CLI; synthetic localhost only"]
fn external_hls_cold_entry_filters_audio_before_the_retained_idr() {
    let _serial = plx_base::testlock::serial();
    let _ff = super::image_subtitle_tests::bind_host_ffmpeg();
    assert!(plx_net::net::global_init());
    crate::curlio::boot();
    let dir = Scratch::new();
    entry_fixture_with_audio(&dir.0, "15", true);
    let packets = entry_packet_nals(&dir.0.join("part-1.ts"));
    let first_idr = packets.iter().position(|(_, nals)| nals.contains(&5)).expect("mid-segment IDR");
    assert_eq!(first_idr, 5);
    let cutoff = packets[first_idr].0 - packets[0].0;
    assert_eq!(cutoff, 500_000_000);
    std::fs::write(dir.0.join("cold-av.m3u8"),
        "#EXTM3U\n#EXT-X-TARGETDURATION:1\n#EXT-X-MEDIA-SEQUENCE:1\n#EXTINF:1,\npart-1.ts\n#EXT-X-ENDLIST\n").unwrap();
    let http = HttpFixture::start(dir.0.clone());
    let (result, aus, failed) = entry_demux_tracks(&http, "cold-av.m3u8", -1, true);
    assert!(result.is_ok(), "cold A/V entry must find its IDR: {result:?}");
    assert!(!failed);
    let video: Vec<_> = aus.iter().filter(|(es, _, _)| *es == 1).collect();
    let audio: Vec<_> = aus.iter().filter(|(es, _, _)| *es == 2).collect();
    assert!(!video.is_empty() && !audio.is_empty(), "both retained lanes must contain real AUs");
    assert_eq!(video[0].1, cutoff);
    assert_eq!(video[0].2, 1);
    assert!(aus.iter().all(|(_, pts, _)| *pts >= cutoff), "no retained lane may start before the IDR cutoff");
    assert!(audio.iter().all(|(_, pts, _)| *pts >= cutoff), "prefix audio must not precede the retained IDR");
    assert!(audio[0].1 < cutoff + 30_000_000, "audio immediately after the cutoff must not be discarded");
}

/// Replay the same six-segment chain through the real external producer, including
/// its sequence/timeline state. Private media and playlist stay outside the tree.
#[test]
#[ignore = "requires bundled host FFmpeg and NATIVE_HLS_REPLAY_DIR with six TS bodies and replay.m3u8; localhost only"]
fn external_hls_private_six_segment_chain_reaches_dependent_tail() {
    let _serial = plx_base::testlock::serial();
    let root = PathBuf::from(std::env::var_os("NATIVE_HLS_REPLAY_DIR")
        .expect("set NATIVE_HLS_REPLAY_DIR to the private six-segment fixture directory"));
    let mut bytes = 0u64;
    for index in 0..6 {
        let metadata = std::fs::metadata(root.join(format!("replay-{index}.ts")))
            .unwrap_or_else(|_| panic!("private replay segment {index} is unavailable"));
        assert!(metadata.is_file() && metadata.len() > 0,
            "private replay segment {index} must be a nonempty file");
        bytes = bytes.checked_add(metadata.len()).expect("private replay byte count overflow");
    }
    assert!(bytes < 16 * 1024 * 1024, "private replay must fit the synchronous AU queue");
    let body = std::fs::read_to_string(root.join("replay.m3u8"))
        .expect("private replay playlist is unavailable");
    let lines: Vec<_> = body.lines().collect();
    assert_eq!(lines.len(), 16, "private replay requires the six-segment canonical playlist");
    assert!(lines[0] == "#EXTM3U" && lines[2] == "#EXT-X-MEDIA-SEQUENCE:0"
        && lines[15] == "#EXT-X-ENDLIST", "unexpected private replay playlist structure");
    let target = lines[1].strip_prefix("#EXT-X-TARGETDURATION:")
        .and_then(|raw| raw.parse::<u64>().ok()).expect("invalid private replay target duration");
    assert!(target > 0, "private replay target duration must be positive");
    for index in 0..6 {
        let duration = lines[3 + index * 2].strip_prefix("#EXTINF:")
            .and_then(|raw| raw.strip_suffix(','))
            .and_then(|raw| raw.parse::<f64>().ok()).expect("invalid private replay segment duration");
        assert!(duration.is_finite() && duration > 0.0, "invalid private replay segment duration");
        assert!(lines[4 + index * 2] == format!("replay-{index}.ts"),
            "private replay must contain only its ordered relative segment aliases");
    }
    let _ff = super::image_subtitle_tests::bind_host_ffmpeg();
    assert!(plx_net::net::global_init());
    crate::curlio::boot();
    let http = HttpFixture::start(root);
    let (result, aus, failed) = entry_demux(&http, "replay.m3u8", -1);
    let last_pts = aus.last().map(|(pts, _)| *pts);
    eprintln!("hls six-segment replay result={result:?} video_aus={} last_pts_ns={last_pts:?}", aus.len());
    assert!(result.is_ok(), "the complete private chain must reach its dependent tail: {result:?}");
    assert!(!failed, "the private chain published a demux failure");
    assert_eq!(aus.first().map(|(_, key)| *key), Some(1), "private chain must start at an IDR");
    assert!(last_pts.is_some_and(|pts| pts > 42_000_000_000),
        "private chain must emit video beyond the sixth-segment boundary");
    let requests = http.requests.lock().unwrap();
    for index in 0..6 {
        assert!(requests.iter().any(|path| path == &format!("/replay-{index}.ts")),
            "private replay did not request segment {index}");
    }
}

#[test]
#[ignore = "requires bundled host FFmpeg, ffmpeg/libx264 and ffprobe CLI; synthetic localhost only"]
fn external_live_hls_subtitles_use_committed_mux_video_clock_and_route_selection() {
    use crate::{player::{sidecar, text_style::TextStyle}, route};
    let _serial = plx_base::testlock::serial();
    let _ff = super::image_subtitle_tests::bind_host_ffmpeg();
    let _globals = route::HlsTrackRequest::test_globals_guard();
    assert!(plx_net::net::global_init());
    crate::curlio::boot();
    let dir = Scratch::new();
    let _text = entry_fixture_with_audio(&dir.0, "10", true);
    std::fs::write(dir.0.join("live.m3u8"),
        "#EXTM3U\n#EXT-X-TARGETDURATION:1\n#EXT-X-MEDIA-SEQUENCE:0\n#EXTINF:1,\npart-0.ts\n").unwrap();
    std::fs::write(dir.0.join("master-live.m3u8"),
        "#EXTM3U\n#EXT-X-MEDIA:TYPE=SUBTITLES,GROUP-ID=\"cc\",NAME=\"English\",LANGUAGE=\"en\",FORCED=YES,URI=\"live-cc.m3u8\"\n#EXT-X-STREAM-INF:BANDWIDTH=100,SUBTITLES=\"cc\"\nlive.m3u8\n").unwrap();
    let video_ns = entry_packet_nals(&dir.0.join("part-0.ts"))[0].0;
    let video_ticks = (i128::from(video_ns) * 90_000 + 500_000_000) / 1_000_000_000;
    std::fs::write(dir.0.join("live-cc.m3u8"),
        "#EXTM3U\n#EXT-X-TARGETDURATION:1\n#EXT-X-MEDIA-SEQUENCE:999\n#EXTINF:3,\nlive-cc.vtt\n").unwrap();
    std::fs::write(dir.0.join("live-cc.vtt"), format!(
        "WEBVTT\nX-TIMESTAMP-MAP=LOCAL:00:00.000,MPEGTS:{video_ticks}\n\n00:00.500 --> 00:01.500\nLive committed video caption\n\n")).unwrap();
    let http = HttpFixture::start(dir.0.clone());
    let info = inspect::http(&format!("{}/master-live.m3u8", http.url), Arc::new(AtomicBool::new(false))).unwrap();
    let playback = info.hls_playback.as_ref().expect("selected master subtitle group retained").clone();
    assert!(playback.is_live_subtitles());
    assert_eq!(playback.live_subtitle_revision(), None, "inspection cannot publish a decoder clock");
    assert!(http.requests.lock().unwrap().iter().all(|path| !path.contains("live-cc")));
    let source = info.resolved_source.clone().unwrap();
    let mut ps = route::PlaybackSession::IDLE;
    route::reset_player_control_for_test(&ps);
    route::request_external_source(&mut ps, &info, source.clone(), "Live fixture", "M3U",
        plx_platform::devcaps::assumed_shared()).unwrap();
    route::reset_player_control_for_test(&ps);
    SHARED.reset_session();
    SHARED.seek_to_ns.store(-1, Ordering::Release);
    let prior_feed = FEED_AUDIO.swap(true, Ordering::Relaxed);
    let mut video = crate::aq::aq_new(1024 * 1024);
    let mut audio = crate::aq::aq_new(1024 * 1024);
    let mut hs = plx_net::stream::http_stream_boxed();
    let (v, a, h) = (SendPtr(&mut *video), SendPtr(&mut *audio), SendPtr(&mut *hs));
    let selected = playback.clone();
    let worker = std::thread::spawn(move || {
        let off = plx_base::task::OffFrame::for_test();
        demux_external(&off, 0, source, "h264".into(), "aac".into(), Some(HlsTrackSelection::Rendition(selected)),
            v, a, h, TextStyle::current(Default::default()));
    });
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut first_video = None;
    while Instant::now() < deadline && (!worker.is_finished()) {
        for (queue, is_video) in [(&mut *video, true), (&mut *audio, false)] {
            let node = crate::aq::aq_pop(queue, std::ptr::null_mut());
            if !node.is_null() {
                unsafe {
                    if is_video && first_video.is_none() { first_video = Some(crate::aq::au_fields(node).2); }
                    libc::free(node.cast());
                }
            }
        }
        if playback.live_subtitle_revision().is_some() && first_video.is_some() { break; }
        std::thread::sleep(Duration::from_millis(2));
    }
    // Always stop and join the media producer before assertion/route failures.
    crate::aq::aq_abort(&mut *video); crate::aq::aq_abort(&mut *audio);
    crate::curlio::abort_active(); plx_net::stream::http_shutdown(&mut *hs);
    worker.join().unwrap();
    plx_net::stream::http_close(&mut *hs);
    crate::aq::aq_destroy(&mut *video); crate::aq::aq_destroy(&mut *audio);
    FEED_AUDIO.store(prior_feed, Ordering::Relaxed);
    assert!(!SHARED.demux_failed.load(Ordering::Acquire));
    assert!(playback.live_subtitle_revision().is_some(), "successful segment feed must publish its decoder clock");
    let first_video = first_video.expect("real muxed TS must emit video AUs");
    let catalog = route::hls_track_catalog(&ps).unwrap();
    let rows = catalog.subtitle_rows();
    assert_eq!(rows.len(), 1);
    assert!(rows[0].forced);
    assert!(catalog.rows().is_empty(), "muxed metadata does not invent an audio rendition");
    let id = rows[0].id;
    let (canonical, epoch) = playback.prepare_subtitle_snapshot(id, Arc::new(AtomicBool::new(false))).unwrap();
    assert_eq!(epoch, playback.live_subtitle_revision());
    let expected_ms = (first_video + 500_000_000) / 1_000_000;
    let expected = format!("00:00:{:02}.{:03}", expected_ms / 1000, expected_ms % 1000);
    assert!(String::from_utf8(canonical).unwrap().contains(&expected), "cue must retain the actual muxed audio/video lead");
    let request = route::begin_hls_track_request(&mut ps, &catalog.subtitle_choice(Some(id)).unwrap()).unwrap();
    let off = plx_base::task::OffFrame::for_test();
    let prepared = request.prepare(&off, Arc::new(AtomicBool::new(false))).unwrap();
    assert!(route::complete_hls_track_request(&mut ps, &request, prepared).unwrap());
    let style = TextStyle::current(route::subtitle_text_appearance(&ps));
    let source = sidecar::ass_source(false, style).expect("live selection must reach shared renderer");
    let crate::player::ass::Content::Script { bytes, .. } = &source.content else { panic!("live text script expected"); };
    assert!(std::str::from_utf8(bytes).unwrap().contains("Live committed video caption"));
    assert_eq!(route::hls_track_catalog(&ps).unwrap().selected_subtitle(), Some(id));
    let off_request = route::begin_hls_track_request(&mut ps, &catalog.subtitle_choice(None).unwrap()).unwrap();
    let prepared = off_request.prepare(&off, Arc::new(AtomicBool::new(false))).unwrap();
    assert!(route::complete_hls_track_request(&mut ps, &off_request, prepared).unwrap());
    assert_eq!(route::hls_track_catalog(&ps).unwrap().selected_subtitle(), None);
}

/// A non-independent entry may precede the next decoder random-access point.
/// A cold start must bound its search, not declare the presentation invalid.
#[test]
#[ignore = "requires bundled host FFmpeg, ffmpeg/libx264 and ffprobe CLI; synthetic localhost only"]
fn external_hls_cold_entry_continues_past_a_wholly_dependent_segment() {
    let _serial = plx_base::testlock::serial();
    let _ff = super::image_subtitle_tests::bind_host_ffmpeg();
    assert!(plx_net::net::global_init());
    crate::curlio::boot();
    let dir = Scratch::new();
    entry_fixture_with_length(&dir.0, "20", false, "4", 4);
    let dependent = entry_packet_nals(&dir.0.join("part-1.ts"));
    let entry = entry_packet_nals(&dir.0.join("part-2.ts"));
    assert_eq!(dependent.len(), 10);
    assert!(dependent.iter().all(|(_, nals)| !nals.contains(&5)),
        "precondition: the first available segment has no random-access picture");
    assert!(entry[0].1.contains(&5), "precondition: the next segment starts with a genuine IDR");
    std::fs::write(dir.0.join("cold-next.m3u8"),
        "#EXTM3U\n#EXT-X-TARGETDURATION:1\n#EXT-X-MEDIA-SEQUENCE:1\n#EXTINF:1,\npart-1.ts\n#EXTINF:1,\npart-2.ts\n#EXTINF:1,\npart-3.ts\n#EXT-X-ENDLIST\n").unwrap();
    // Independent decoder control: unavailable preroll references must not
    // prevent the later random-access point from producing displayable frames.
    let frames = entry_fixture_command("ffmpeg", &["-v", "error", "-nostdin", "-i",
        dir.0.join("cold-next.m3u8").to_str().unwrap(), "-an", "-f", "framemd5", "-"], false);
    let frames = std::str::from_utf8(&frames).unwrap().lines()
        .filter(|line| !line.starts_with('#') && !line.trim().is_empty()).count();
    assert!(frames >= 10, "independent decoder must reach the next segment's IDR");
    let http = HttpFixture::start(dir.0.clone());
    let (result, tracks, failed) = entry_demux_tracks_with_abort(&http, "cold-next.m3u8", -1,
        false, false, Some((1_000_000_000, false)));
    let aus: Vec<_> = tracks.into_iter().map(|(_, pts, key)| (pts, key)).collect();
    assert!(result.is_ok(), "valid non-independent HLS must search past dependent entry: {result:?}");
    assert!(!failed);
    assert_eq!(aus.len(), 20, "safe entry and its dependent successor must both be emitted");
    assert_eq!(aus[0], (0, 1), "discarded source gap must not appear as accepted sink runway");
    assert_eq!(aus.iter().map(|(pts, _)| *pts).collect::<Vec<_>>(),
        (0..20).map(|i| i * 100_000_000).collect::<Vec<i64>>(),
        "the continuation transport clock remains monotonic after cold rebasing");
    assert!(http.requests.lock().unwrap().iter().any(|path| path == "/part-2.ts"),
        "producer must actually fetch the later entry, not synthesize success");
    let (paused, tracks, _) = entry_demux_tracks_with_abort(&http, "cold-next.m3u8", -1,
        false, false, Some((1_000_000_000, true)));
    assert!(paused.is_ok(), "viewer pause/preroll must not discard the cold producer: {paused:?}");
    assert_eq!(tracks.first().map(|(_, pts, _)| *pts), Some(0));
}

#[test]
#[ignore = "requires bundled host FFmpeg, ffmpeg/libx264 and ffprobe CLI; synthetic localhost only"]
fn external_hls_cold_search_cannot_skip_fatal_input_or_escape_its_bounds() {
    let _serial = plx_base::testlock::serial();
    let _ff = super::image_subtitle_tests::bind_host_ffmpeg();
    assert!(plx_net::net::global_init());
    crate::curlio::boot();
    let dir = Scratch::new();
    entry_fixture(&dir.0, "20");
    let header = "#EXTM3U\n#EXT-X-TARGETDURATION:1\n#EXT-X-MEDIA-SEQUENCE:1\n";
    let pair = format!("{header}#EXTINF:1,\npart-1.ts\n#EXTINF:1,\npart-2.ts\n#EXT-X-ENDLIST\n");
    std::fs::write(dir.0.join("pair.m3u8"), &pair).unwrap();
    std::fs::write(dir.0.join("independent-cold.m3u8"),
        pair.replacen("#EXTM3U", "#EXTM3U\n#EXT-X-INDEPENDENT-SEGMENTS", 1)).unwrap();
    std::fs::write(dir.0.join("end-cold.m3u8"),
        format!("{header}#EXTINF:1,\npart-1.ts\n#EXT-X-ENDLIST\n")).unwrap();
    std::fs::write(dir.0.join("bound-cold.m3u8"),
        format!("{header}{}#EXTINF:1,\npart-2.ts\n#EXT-X-ENDLIST\n",
            "#EXTINF:1,\npart-1.ts\n".repeat(3))).unwrap();
    for (name, seek) in [("independent-cold.m3u8", -1), ("end-cold.m3u8", -1),
        ("bound-cold.m3u8", -1), ("pair.m3u8", 0)] {
        let http = HttpFixture::start(dir.0.clone());
        let (result, aus, _) = entry_demux(&http, name, seek);
        assert!(matches!(result, Err(HlsExit::Failed("HLS segment does not begin with an IDR"))),
            "{name} must preserve strict entry/exhaustion behavior: {result:?}");
        assert!(aus.is_empty(), "dependent pictures cannot enter playback queues");
        assert!(!http.requests.lock().unwrap().iter().any(|path| path == "/part-2.ts"));
    }
    for malformed in [false, true] {
        let body = if malformed { b"not-a-transport-stream".to_vec() }
            else { std::fs::read(dir.0.join("part-1.ts")).unwrap() };
        let size = body.len() + if malformed { 0 } else { 188 };
        let response = [format!("HTTP/1.1 200 OK\r\nContent-Length: {size}\r\nConnection: close\r\n\r\n").into_bytes(), body].concat();
        let http = HttpFixture::start_with(dir.0.clone(), move |head|
            head.starts_with("GET /part-1.ts ").then(|| response.clone()));
        let (result, aus, _) = entry_demux(&http, "pair.m3u8", -1);
        assert!(result.is_err(), "malformed/truncated input is fatal");
        assert!(aus.is_empty());
        assert!(!http.requests.lock().unwrap().iter().any(|path| path == "/part-2.ts"),
            "fatal transport/container errors must not become random-access search");
    }
    let http = HttpFixture::start(dir.0.clone());
    let (result, aus, _) = entry_demux_tracks_with_abort(&http, "pair.m3u8", -1, false, true, None);
    assert!(matches!(result, Err(HlsExit::Aborted)));
    assert!(aus.is_empty());
    assert!(http.requests.lock().unwrap().is_empty(), "cancelled startup cannot issue HTTP");
}

#[test]
#[ignore = "requires bundled host FFmpeg, ffmpeg/libx264 and ffprobe CLI; synthetic localhost only"]
fn external_hls_cold_entry_deadline_bounds_http_without_shortening_normal_acquisition() {
    let _serial = plx_base::testlock::serial();
    let _ff = super::image_subtitle_tests::bind_host_ffmpeg();
    assert!(plx_net::net::global_init());
    crate::curlio::boot();
    let dir = Scratch::new();
    entry_fixture(&dir.0, "20");
    let http = HttpFixture::start_with(dir.0.clone(), |head| {
        if head.starts_with("GET /part-2.ts ") {
            std::thread::sleep(Duration::from_millis(150));
        }
        None
    });
    SHARED.reset_session();
    let feed = FEED_AUDIO.swap(false, Ordering::Relaxed);
    for bounded in [true, false] {
        let mut video = crate::aq::aq_new(1024 * 1024);
        let mut hs = plx_net::stream::http_stream_boxed();
        let mut net = HlsNet { hs: &mut *hs, curl: None };
        let mut clock = crate::hls::SegmentTimeline::default().begin(Duration::from_secs(1)).unwrap();
        let segment = crate::hls::Segment {
            sequence: 2,
            duration: Duration::from_secs(1),
            resource: crate::hls::Resource::new(plx_plex::plex::StreamUrl::parse(&http.url).origin,
                "/part-2.ts").unwrap(),
        };
        let mut state = H264SegmentState::default();
        let result = unsafe { hls_demux_segment_path(&segment,
            HlsSegmentRequest { path: &segment.resource.path, transport: HlsSegmentTransport::External,
                body_limit: Some(64 * 1024 * 1024), audio: None, rendition: None,
                video: HlsVideoEntry::External(&mut state), mp2: None },
            &mut clock, &mut *video, &mut net, "",
            if bounded { SegmentAcquisition::cold_entry(Instant::now() + Duration::from_millis(30)) }
                else { SegmentAcquisition::for_cursor(None) }) };
        if bounded {
            assert!(matches!(result, Err(HlsExit::PrimeExpired)), "the absolute bound must reach HTTP open");
        } else {
            assert!(result.is_ok(), "normal initial acquisition keeps its existing budget: {:?}", result.as_ref().err());
        }
        drop(net);
        plx_net::stream::http_close(&mut *hs);
        crate::aq::aq_destroy(&mut *video);
    }
    assert!(http.requests.lock().unwrap().iter().filter(|path| path.as_str() == "/part-2.ts").count() >= 2,
        "both bounded and normal acquisitions must really reach the HTTP server");
    FEED_AUDIO.store(feed, Ordering::Relaxed);
    SHARED.reset_session();
}
