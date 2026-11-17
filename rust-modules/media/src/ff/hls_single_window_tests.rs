//! Actual localhost single-entry window acquisition and demux contracts.
use super::*;
use std::sync::Mutex;

fn response(sequence: u64, part: usize, ended: bool) -> Vec<u8> {
    let body = format!("#EXTM3U\n#EXT-X-TARGETDURATION:10\n#EXT-X-MEDIA-SEQUENCE:{sequence}\n#EXTINF:1,\npart-{part}.ts\n{}",
        if ended { "#EXT-X-ENDLIST\n" } else { "" });
    format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).into_bytes()
}

#[test]
#[ignore = "requires bundled host FFmpeg and ffmpeg CLI; localhost only"]
fn external_hls_single_window_refreshes_immediately_after_progress() {
    let _serial = plx_base::testlock::serial();
    let _ff = super::super::image_subtitle_tests::bind_host_ffmpeg();
    assert!(plx_net::net::global_init());
    crate::curlio::boot();
    let dir = Scratch::new();
    entry_fixture_with_length(&dir.0, "10", false, "2", 2);
    let seen = Arc::new(Mutex::new(Vec::<Instant>::new()));
    let visits = seen.clone();
    let http = HttpFixture::start_with(dir.0.clone(), move |request| {
        if !request.starts_with("GET /rolling.m3u8 ") { return None; }
        let mut visits = visits.lock().unwrap();
        let now = Instant::now();
        let data = match visits.first() {
            None => response(100, 0, false),
            // The old unconditional three-second wait loses the next window.
            Some(first) if now.duration_since(*first) >= Duration::from_secs(2) => response(102, 1, false),
            Some(_) => response(101, 1, true),
        };
        visits.push(now);
        Some(data)
    });
    let (result, aus, _) = entry_demux(&http, "rolling.m3u8", -1);
    assert!(result.is_ok(), "progressing single-entry window lost: {result:?}");
    assert_eq!(aus.len(), 20, "both real TS segments must reach the AU queues");
    let visits = seen.lock().unwrap();
    assert_eq!(visits.len(), 2);
    assert!(visits[1].duration_since(visits[0]) < Duration::from_secs(2));
}

#[test]
#[ignore = "requires bundled host FFmpeg and ffmpeg CLI; localhost only"]
fn external_hls_single_window_slip_is_typed_but_finite_and_seek_stay_terminal() {
    let _serial = plx_base::testlock::serial();
    let _ff = super::super::image_subtitle_tests::bind_host_ffmpeg();
    assert!(plx_net::net::global_init());
    crate::curlio::boot();
    let dir = Scratch::new();
    entry_fixture_with_length(&dir.0, "10", false, "2", 2);
    for finite in [false, true] {
        let seen = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let visits = seen.clone();
        let http = HttpFixture::start_with(dir.0.clone(), move |request| {
            if !request.starts_with("GET /slip.m3u8 ") { return None; }
            let n = visits.fetch_add(1, Ordering::Relaxed);
            Some(response(if n == 0 { 100 } else { 102 }, 0, finite && n > 0))
        });
        let (result, aus, _) = entry_demux(&http, "slip.m3u8", -1);
        assert_eq!(aus.len(), 10, "a skipped segment must not lend AUs or references");
        if finite {
            assert!(matches!(result, Err(HlsExit::Failed("HLS live window advanced past playback"))),
                "finite mismatch has no live-recovery authority: {result:?}");
        } else {
            assert!(matches!(result, Err(HlsExit::OpenFailed {
                reason: "HLS live window advanced past playback", outcome: TraceOutcome::ServerState })),
                "proven live eviction needs typed authority: {result:?}");
        }
        assert_eq!(seen.load(Ordering::Relaxed), 2);
    }
    std::fs::write(dir.0.join("live-seek.m3u8"),
        "#EXTM3U\n#EXT-X-TARGETDURATION:10\n#EXT-X-MEDIA-SEQUENCE:100\n#EXTINF:1,\npart-0.ts\n").unwrap();
    let http = HttpFixture::start(dir.0.clone());
    let (result, aus, _) = entry_demux(&http, "live-seek.m3u8", 0);
    assert!(matches!(result, Err(HlsExit::Failed("Seeking requires a finite HLS playlist"))));
    assert!(aus.is_empty());
}

#[test]
#[ignore = "requires bundled host FFmpeg and ffmpeg CLI; localhost only"]
fn external_hls_single_window_stale_snapshot_keeps_no_progress_poll_wait() {
    let _serial = plx_base::testlock::serial();
    let _ff = super::super::image_subtitle_tests::bind_host_ffmpeg();
    assert!(plx_net::net::global_init());
    crate::curlio::boot();
    let dir = Scratch::new();
    entry_fixture_with_length(&dir.0, "10", false, "2", 2);
    let seen = Arc::new(Mutex::new(Vec::<Instant>::new()));
    let visits = seen.clone();
    let http = HttpFixture::start_with(dir.0.clone(), move |request| {
        if !request.starts_with("GET /stale.m3u8 ") { return None; }
        let mut visits = visits.lock().unwrap();
        let data = match visits.len() {
            0 => response(100, 0, false),
            // A regressing/cache-stale snapshot is not a forward eviction and
            // supplies no new AU; retain the bounded no-progress poll delay.
            1 => response(99, 0, false),
            _ => response(101, 1, true),
        };
        visits.push(Instant::now());
        Some(data)
    });
    let (result, aus, _) = entry_demux(&http, "stale.m3u8", -1);
    assert!(result.is_ok(), "stale snapshot must wait, not mint recovery: {result:?}");
    assert_eq!(aus.len(), 20);
    let visits = seen.lock().unwrap();
    assert_eq!(visits.len(), 3);
    assert!(visits[2].duration_since(visits[1]) >= Duration::from_secs(3),
        "no-progress reloads must retain target/2 clamped to three seconds");
}

#[test]
#[ignore = "requires bundled host FFmpeg and ffmpeg CLI; localhost only, real 30-second stall guard"]
fn external_hls_single_window_stopped_advancing_has_bounded_recovery() {
    let _serial = plx_base::testlock::serial();
    let _ff = super::super::image_subtitle_tests::bind_host_ffmpeg();
    assert!(plx_net::net::global_init());
    crate::curlio::boot();
    let dir = Scratch::new();
    entry_fixture_with_length(&dir.0, "10", false, "1", 1);
    let seen = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let visits = seen.clone();
    let http = HttpFixture::start_with(dir.0.clone(), move |request| {
        if !request.starts_with("GET /stalled.m3u8 ") { return None; }
        visits.fetch_add(1, Ordering::Relaxed);
        let body = "#EXTM3U\n#EXT-X-TARGETDURATION:1\n#EXT-X-MEDIA-SEQUENCE:100\n#EXTINF:1,\npart-0.ts\n";
        Some(format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).into_bytes())
    });
    let started = Instant::now();
    let (result, aus, _) = entry_demux(&http, "stalled.m3u8", -1);
    assert!(matches!(result, Err(HlsExit::OpenFailed {
        reason: "HLS playlist stopped advancing", outcome: TraceOutcome::ServerState })),
        "a proven bounded live stall needs same-source recovery: {result:?}");
    assert!(started.elapsed() >= Duration::from_secs(30), "retain existing real stall allowance");
    assert_eq!(aus.len(), 10, "unchanged snapshots cannot feed the same AU twice");
    let requests = seen.load(Ordering::Relaxed);
    assert!((2..=64).contains(&requests), "target/2 poll wait must bound requests: {requests}");
}

#[test]
#[ignore = "requires bundled host FFmpeg and ffmpeg CLI; localhost only"]
fn rolling_live_feed_survives_repeated_fragment_outages() {
    check_rolling_fragment_outages(84);
}

#[test]
#[ignore = "two hours of generated media; bundled host FFmpeg and ffmpeg CLI; localhost only"]
fn rolling_live_two_hours_with_fragment_outages() {
    check_rolling_fragment_outages(1440);
}

fn check_rolling_fragment_outages(segments: usize) {
    // Five seconds of media per fragment, consumed in accelerated time.
    const FRAMES_PER_SEGMENT: usize = 10;
    let _serial = plx_base::testlock::serial();
    let _ff = super::super::image_subtitle_tests::bind_host_ffmpeg();
    assert!(plx_net::net::global_init());
    crate::curlio::boot();
    let dir = Scratch::new();
    let seconds = (segments * 5).to_string();
    let pattern = dir.0.join("part-%d.ts");
    let manifest = dir.0.join("generated.m3u8");
    entry_fixture_command("ffmpeg", &[
        "-v", "error", "-nostdin", "-y", "-f", "lavfi", "-i", "testsrc2=size=64x48:rate=2",
        "-t", &seconds, "-an", "-c:v", "libx264", "-preset", "ultrafast", "-tune", "zerolatency",
        "-g", "10", "-keyint_min", "10", "-sc_threshold", "0", "-bf", "0",
        "-hls_time", "5", "-hls_list_size", "0", "-hls_segment_filename",
        pattern.to_str().unwrap(), manifest.to_str().unwrap(),
    ], true);
    let faults = [3, segments / 2, segments - 5];
    let failed = Mutex::new([false; 3]);
    let next_segment = std::sync::atomic::AtomicUsize::new(0);
    let http = HttpFixture::start_with(dir.0.clone(), move |request| {
        let path = request.split_whitespace().nth(1)?;
        if path == "/rolling.m3u8" {
            let i = next_segment.fetch_add(1, Ordering::Relaxed).min(segments - 1);
            let end = if i + 1 == segments { "#EXT-X-ENDLIST\n" } else { "" };
            let body = format!("#EXTM3U\n#EXT-X-TARGETDURATION:5\n#EXT-X-MEDIA-SEQUENCE:{i}\n#EXTINF:5,\npart-{i}.ts\n{end}");
            return Some(format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).into_bytes());
        }
        let fragment = path.strip_prefix("/part-")?.strip_suffix(".ts")?.parse::<usize>().ok()?;
        let index = faults.iter().position(|&i| i == fragment)?;
        if std::mem::replace(&mut failed.lock().unwrap()[index], true) { return None; }
        Some(b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec())
    });
    let (result, aus, _) = entry_demux(&http, "rolling.m3u8", -1);
    assert!(result.is_ok(), "live demux stopped: {result:?}");
    assert_eq!(aus.len(), segments * FRAMES_PER_SEGMENT);
    assert!(aus.windows(2).all(|pair| pair[1].0 - pair[0].0 == 500_000_000),
        "all video samples must keep their half-second spacing across outages");
    let requests = http.requests.lock().unwrap();
    for i in 0..segments {
        let path = format!("/part-{i}.ts");
        assert_eq!(requests.iter().filter(|s| **s == path).count(),
            if faults.contains(&i) { 2 } else { 1 }, "requests for fragment {i}");
    }
}

#[test]
#[ignore = "requires the bundled software decoder, ffmpeg CLI and localhost"]
fn local_window_reprobes_the_same_session_after_a_missing_live_segment() {
    use crate::local_multiview::{Event, Limits, Session};
    use std::sync::atomic::AtomicUsize;

    let _serial = plx_base::testlock::serial();
    let _ff = super::super::image_subtitle_tests::bind_host_ffmpeg();
    assert!(plx_net::net::global_init());
    crate::curlio::boot();
    let dir = Scratch::new();
    entry_fixture_with_length(&dir.0, "10", true, "5", 5);
    let manifests = Arc::new(AtomicUsize::new(0));
    let media = Arc::new(AtomicUsize::new(0));
    let expired = Arc::new(AtomicUsize::new(0));
    let (m, f, e) = (manifests.clone(), media.clone(), expired.clone());
    let http = HttpFixture::start_with(dir.0.clone(), move |request| {
        let error = |status| format!("HTTP/1.1 {status} Error\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").into_bytes();
        match request.split_whitespace().nth(1)? {
            "/window.m3u8" => {
                let count = m.fetch_add(1, Ordering::Relaxed);
                if count == 2 { return Some(error(503)); }
                let (seq, paths, end) = if count == 0 {
                    (50, vec!["part-0.ts"], "")
                } else if count == 1 {
                    (51, vec!["expired.ts"], "")
                } else {
                    (53, vec!["part-3.ts", "part-4.ts"], "#EXT-X-ENDLIST\n")
                };
                let mut body = format!("#EXTM3U\n#EXT-X-TARGETDURATION:1\n#EXT-X-MEDIA-SEQUENCE:{seq}\n");
                for path in paths { body.push_str(&format!("#EXTINF:1,\n{path}\n")); }
                body.push_str(end);
                Some(format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).into_bytes())
            }
            "/expired.ts" => { e.fetch_add(1, Ordering::Relaxed); Some(error(410)) }
            "/part-3.ts" => { f.fetch_add(1, Ordering::Relaxed); None }
            _ => None,
        }
    });
    let mut session = Session::open(ExternalSource::Hls(format!("{}/window.m3u8", http.url)), Limits::default()).unwrap();
    let mut recovered = 0;
    let deadline = Instant::now() + Duration::from_secs(14);
    while Instant::now() < deadline && recovered < 4 {
        match session.poll() {
            Some(Event::Video(frame)) if frame.pts_us >= 4_400_000 => recovered += 1,
            Some(Event::Failure(error)) => panic!("live window lost its slot: {error}"),
            Some(Event::Ended) => break,
            _ => std::thread::sleep(Duration::from_millis(5)),
        }
    }
    session.cancel();
    assert!(recovered >= 4, "fresh keyframes were not presented on the original session");
    assert_eq!(expired.load(Ordering::Relaxed), 1);
    assert_eq!(media.load(Ordering::Relaxed), 1);
    assert_eq!(manifests.load(Ordering::Relaxed), 4);
}

#[test]
#[ignore = "requires the bundled software decoder, ffmpeg CLI and localhost"]
fn local_startup_retries_only_the_recognized_interruption_with_slot_cookies() {
    use crate::local_multiview::{Error, Event, Limits, Session};
    use std::sync::atomic::AtomicUsize;

    let _serial = plx_base::testlock::serial();
    let _ff = super::super::image_subtitle_tests::bind_host_ffmpeg();
    assert!(plx_net::net::global_init());
    crate::curlio::boot();
    let dir = Scratch::new();
    entry_fixture_with_length(&dir.0, "10", false, "2", 2);
    let root_count = Arc::new(AtomicUsize::new(0));
    let child_count = Arc::new(AtomicUsize::new(0));
    let deny_count = Arc::new(AtomicUsize::new(0));
    let other_count = Arc::new(AtomicUsize::new(0));
    let exhausted = Arc::new(AtomicUsize::new(0));
    let (r, m, d, o, e) = (root_count.clone(), child_count.clone(), deny_count.clone(), other_count.clone(), exhausted.clone());
    let http = HttpFixture::start_with(dir.0.clone(), move |request| {
        let has_cookie = request.to_ascii_lowercase().contains("cookie: slot_fixture=ok");
        let interruption = "The request is interrupted by main process";
        let denied = "<h1>Forbidden (No client found)</h1>";
        let body = match request.split_whitespace().nth(1)? {
            "/entry" if r.fetch_add(1, Ordering::Relaxed) == 0 => return Some(
                b"HTTP/1.1 302 Found\r\nLocation: /busy\r\nSet-Cookie: slot_fixture=ok; Path=/\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec()),
            "/busy" => interruption,
            "/entry" if has_cookie => "#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=100000\n/selected.m3u8\n",
            "/entry" => denied,
            "/selected.m3u8" if m.fetch_add(1, Ordering::Relaxed) == 0 => interruption,
            "/selected.m3u8" if has_cookie => "#EXTM3U\n#EXT-X-TARGETDURATION:1\n#EXTINF:1,\npart-0.ts\n#EXTINF:1,\npart-1.ts\n#EXT-X-ENDLIST\n",
            "/deny" => { d.fetch_add(1, Ordering::Relaxed); denied }
            "/unknown" => { o.fetch_add(1, Ordering::Relaxed); "<h1>Unknown reply</h1>" }
            "/alwaysbusy" => { e.fetch_add(1, Ordering::Relaxed); interruption }
            _ => return None,
        };
        Some(format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).into_bytes())
    });
    for (path, expected) in [("/entry", None), ("/deny", Some(Error::SourceDenied)),
        ("/unknown", Some(Error::Playlist)), ("/alwaysbusy", Some(Error::SourceInterrupted))] {
        let url = format!("{}{path}", http.url);
        // The ordinary HTTP case must sniff correctly; explicit HLS must also
        // keep an unknown 200 response terminal.
        let source = if path == "/unknown" { ExternalSource::Hls(url) } else { ExternalSource::Http(url) };
        let mut session = Session::open(source, Limits::default()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(8);
        let result = loop {
            assert!(Instant::now() < deadline, "startup fixture did not terminate");
            match session.poll() {
                Some(Event::Video(_)) => break None,
                Some(Event::Failure(error)) => break Some(error),
                Some(Event::Ended) => panic!("startup fixture ended before presenting a frame"),
                _ => std::thread::sleep(Duration::from_millis(5)),
            }
        };
        session.cancel();
        assert_eq!(result, expected);
    }
    for (counter, expected) in [(root_count, 2), (child_count, 2), (deny_count, 1),
        (other_count, 1), (exhausted, 3)] {
        assert_eq!(counter.load(Ordering::Relaxed), expected);
    }
}
