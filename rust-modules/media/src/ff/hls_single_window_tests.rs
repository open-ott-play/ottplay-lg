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
