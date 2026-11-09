use super::*;

fn clock(at: i64) -> Clock { Clock { pts_90k: 900_000 + at * 90_000, presentation_ns: at * 1_000_000_000 } }
fn cue(ticks: u64, body: &str) -> String {
    format!("WEBVTT\nX-TIMESTAMP-MAP=LOCAL:00:00.000,MPEGTS:{ticks}\n\n00:00.000 --> 00:02.000\n{body}\n\n")
}
#[test]
fn live_clock_revision_changes_only_for_source_epoch_and_invalidates_without_clock() {
    let timeline = Timeline::default();
    assert_eq!(timeline.revision(), None);
    timeline.commit("source", 7, Some(clock(0)), 2_000_000_000);
    let first = timeline.revision().unwrap();
    timeline.commit("source", 7, Some(clock(2)), 4_000_000_000);
    assert_eq!(timeline.revision(), Some(first));
    timeline.commit("source", 8, None, 6_000_000_000);
    assert_eq!(timeline.revision(), None);
    assert!(timeline.snapshot().is_none());
    timeline.commit("source", 8, Some(clock(6)), 8_000_000_000);
    assert!(timeline.revision().unwrap() > first);
    let second = timeline.revision();
    timeline.commit("replacement", 8, Some(clock(8)), 10_000_000_000);
    assert!(timeline.revision() > second);
}
#[test]
fn live_window_dedupes_overlaps_and_discards_expired_cues_with_packet_clock() {
    let mut assembly = Assembly::new(clock(120));
    let window = Window { start_ns: 100_000_000_000, end_ns: 130_000_000_000 };
    push_window(&mut assembly, &cue(900_000, "Expired"), window, &mut || false).unwrap();
    push_window(&mut assembly, &cue(11_700_000, "Current"), window, &mut || false).unwrap();
    push_window(&mut assembly, &cue(11_700_000, "Current"), window, &mut || false).unwrap();
    let out = String::from_utf8(assembly.finish().unwrap()).unwrap();
    assert!(!out.contains("Expired"));
    assert_eq!(out.matches("Current").count(), 1);
    assert!(out.contains("00:02:00.000 --> 00:02:02.000"));
}
#[test]
fn live_window_maps_33_bit_wrap_from_actual_video_anchor() {
    let mut assembly = Assembly::new(Clock { pts_90k: (1_i64 << 33) - 90_000, presentation_ns: 0 });
    let window = Window { start_ns: 0, end_ns: 4_000_000_000 };
    push_window(&mut assembly, &cue(0, "After wrap"), window, &mut || false).unwrap();
    let out = String::from_utf8(assembly.finish().unwrap()).unwrap();
    assert!(out.contains("00:00:01.000 --> 00:00:03.000"));
}
#[test]
fn live_playlist_preserves_epochs_without_correlating_video_sequence() {
    let text = "#EXTM3U\n#EXT-X-TARGETDURATION:2\n#EXT-X-MEDIA-SEQUENCE:900\n#EXT-X-DISCONTINUITY-SEQUENCE:12\n#EXTINF:2,\na.vtt\n#EXT-X-DISCONTINUITY\n#EXTINF:2,\nb.vtt\n";
    let window = external_hls::subtitle_window("https://example.invalid/subs.m3u8", text).unwrap();
    assert_eq!(window.iter().map(|(s,e)| (s.sequence,*e)).collect::<Vec<_>>(), vec![(900,12),(901,13)]);
    for tag in ["#EXT-X-GAP", "#EXT-X-MAP:URI=\"init\"", "#EXT-X-KEY:METHOD=AES-128,URI=\"key\"", "#EXT-X-BYTERANGE:10@0"] {
        assert!(external_hls::subtitle_window("https://example.invalid/subs.m3u8", &text.replace("#EXTINF:2,", &format!("{tag}\n#EXTINF:2,"))).is_err());
    }
}
#[test]
fn live_playlist_work_and_cancellation_are_bounded_before_network() {
    let text = format!("#EXTM3U\n#EXT-X-TARGETDURATION:2\n{}", "#EXTINF:2,\na.vtt\n".repeat(65));
    assert_eq!(external_hls::subtitle_window("https://example.invalid/subs.m3u8", &text).err(), Some("Too many HLS segments"));
    let timeline = Timeline::default();
    assert_eq!(prepare("https://example.invalid/master.m3u8", "subs.m3u8", &timeline,
        Arc::new(AtomicBool::new(true))).err(), Some("HLS subtitle preparation cancelled or timed out"));
}

#[test]
fn live_restart_rollbacks_advance_epoch_identity_without_a_discontinuity_tag() {
    let timeline = Timeline::default();
    timeline.commit("source", 0, Some(clock(20)), 22_000_000_000);
    let before = timeline.revision();
    timeline.commit("source", 0, Some(clock(0)), 2_000_000_000);
    assert!(timeline.revision() > before);
}

#[test]
fn live_missing_timestamp_map_uses_rfc_zero_mapping() {
    let mut assembly = Assembly::new(Clock { pts_90k: 0, presentation_ns: 5_000_000_000 });
    let window = Window { start_ns: 5_000_000_000, end_ns: 10_000_000_000 };
    push_window(&mut assembly, "WEBVTT\n\n00:01.000 --> 00:02.000\nDefault mapping\n\n", window, &mut || false).unwrap();
    let out = String::from_utf8(assembly.finish().unwrap()).unwrap();
    assert!(out.contains("00:00:06.000 --> 00:00:07.000"));
}

struct Server {
    url: String,
    stop: Arc<AtomicBool>,
    count: Arc<std::sync::atomic::AtomicUsize>,
    thread: Option<std::thread::JoinHandle<()>>,
}
impl Server {
    fn new(respond: impl Fn(usize, &str) -> String + Send + 'static) -> Self {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let url = format!("http://{}/master.m3u8", listener.local_addr().unwrap());
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = stop.clone();
        let count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let worker_count = count.clone();
        let thread = std::thread::spawn(move || {
            while !worker_stop.load(Ordering::Acquire) {
                let Ok((mut stream, _)) = listener.accept() else {
                    std::thread::sleep(Duration::from_millis(2)); continue;
                };
                stream.set_read_timeout(Some(Duration::from_secs(1))).unwrap();
                stream.set_write_timeout(Some(Duration::from_secs(1))).unwrap();
                let mut request = Vec::new();
                let mut byte = [0];
                while request.len() < 8192 && !request.ends_with(b"\r\n\r\n") {
                    if stream.read(&mut byte).unwrap_or(0) == 0 { break; }
                    request.push(byte[0]);
                }
                let n = worker_count.fetch_add(1, Ordering::AcqRel);
                let body = respond(n, &String::from_utf8_lossy(&request));
                let header = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
                let _ = stream.write_all(header.as_bytes());
                let _ = stream.write_all(body.as_bytes());
            }
        });
        Self { url, stop, count, thread: Some(thread) }
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        self.thread.take().unwrap().join().unwrap();
    }
}
fn curl() -> plx_base::testlock::Serial {
    let serial = plx_base::testlock::serial();
    assert!(plx_net::net::global_init());
    assert!(crate::curlio::available());
    serial
}
#[test]
fn live_http_refresh_reuses_overlap_and_replaces_rolling_canonical_window() {
    let _serial = curl();
    let server = Server::new(|request, text| {
        if text.starts_with("GET /subs.m3u8 ") {
            let segments = if request == 0 { "#EXTINF:2,\na.vtt\n" }
                else { "#EXTINF:2,\na.vtt\n#EXTINF:2,\nb.vtt\n" };
            format!("#EXTM3U\n#EXT-X-TARGETDURATION:1\n#EXT-X-MEDIA-SEQUENCE:100\n{segments}")
        } else if text.starts_with("GET /a.vtt ") { cue(900_000, "First") }
        else { assert!(text.starts_with("GET /b.vtt ")); cue(1_080_000, "Second") }
    });
    let timeline = Timeline::default();
    timeline.commit("video", 0, Some(clock(0)), 2_000_000_000);
    let cancel = Arc::new(AtomicBool::new(false));
    let (first, epoch) = prepare(&server.url, "subs.m3u8", &timeline, cancel.clone()).unwrap();
    assert!(String::from_utf8(first).unwrap().contains("First"));
    let _ = prepare(&server.url, "subs.m3u8", &timeline, cancel.clone()).unwrap();
    assert_eq!(server.count.load(Ordering::Acquire), 2, "target cadence prevents an immediate reload");
    timeline.cache.lock().unwrap().as_mut().unwrap().refresh_at = Instant::now();
    timeline.commit("video", 0, Some(clock(2)), 4_000_000_000);
    let (second, next_epoch) = prepare(&server.url, "subs.m3u8", &timeline, cancel).unwrap();
    assert_eq!(epoch, next_epoch, "routine anchor advance retains subtitle epoch");
    let second = String::from_utf8(second).unwrap();
    assert!(second.contains("First") && second.contains("Second"));
    assert_eq!(server.count.load(Ordering::Acquire), 4, "one playlist reload and only the new body");
}
#[test]
fn live_http_epoch_change_during_fetch_rejects_old_snapshot() {
    let _serial = curl();
    let timeline = Arc::new(Timeline::default());
    timeline.commit("video", 0, Some(clock(0)), 2_000_000_000);
    let changed = timeline.clone();
    let server = Server::new(move |request, _| {
        if request == 0 { "#EXTM3U\n#EXT-X-TARGETDURATION:2\n#EXTINF:2,\na.vtt\n".into() }
        else { changed.commit("video", 1, Some(clock(2)), 4_000_000_000); cue(900_000, "Stale") }
    });
    assert_eq!(prepare(&server.url, "subs.m3u8", &timeline, Arc::new(AtomicBool::new(false))).err(),
        Some("HLS subtitle video epoch changed during preparation"));
    assert!(timeline.cache.lock().unwrap().is_none());
}
#[test]
fn live_http_cancel_during_body_returns_no_snapshot_or_cache() {
    let _serial = curl();
    let timeline = Timeline::default();
    timeline.commit("video", 0, Some(clock(0)), 2_000_000_000);
    let cancel = Arc::new(AtomicBool::new(false));
    let stopped = cancel.clone();
    let server = Server::new(move |request, _| {
        if request == 0 { "#EXTM3U\n#EXT-X-TARGETDURATION:2\n#EXTINF:2,\na.vtt\n".into() }
        else { stopped.store(true, Ordering::Release); cue(900_000, "Cancelled") }
    });
    assert!(prepare(&server.url, "subs.m3u8", &timeline, cancel).is_err());
    assert!(timeline.cache.lock().unwrap().is_none());
}

#[test]
fn live_explicit_restart_invalidates_same_source_epoch_mapping_and_cache() {
    let timeline = Timeline::default();
    timeline.commit("source", 4, Some(clock(0)), 1_000_000_000);
    let previous = timeline.revision();
    timeline.invalidate();
    assert_eq!(timeline.revision(), None);
    assert!(timeline.snapshot().is_none());
    assert!(timeline.cache.lock().unwrap().is_none());
    timeline.commit("source", 4, Some(Clock { pts_90k: 1_800_000, presentation_ns: 0 }), 2_000_000_000);
    assert!(timeline.revision() > previous);
}
