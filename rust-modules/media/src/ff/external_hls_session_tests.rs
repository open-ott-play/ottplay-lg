//! The actual manifest and segment callers share one HTTP context. No decoder,
//! provider, media network or TV is involved in these host-CI loopback tests.
use super::*;
use crate::curlio::cookie_tests::{curl, drain, Reply, SessionServer};

const MASTER: &[u8] = b"#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=100\nselected.m3u8\n";
const MEDIA: &[u8] = b"#EXTM3U\n#EXT-X-TARGETDURATION:1\n#EXT-X-MEDIA-SEQUENCE:7\n#EXTINF:1,\npart.ts\n";

#[test]
fn playback_master_child_segment_and_refresh_share_cookies_and_connection() {
    let _serial = curl();
    let server = SessionServer::new(vec![
        Reply::ok("Set-Cookie: sid=manifest-session; Path=/\r\n", MASTER),
        Reply::ok("", MEDIA).requiring("sid=manifest-session"),
        Reply::ok("Set-Cookie: last=segment-session; Path=/\r\n", b"segment").requiring("sid=manifest-session"),
        Reply::ok("", MEDIA).requiring("last=segment-session"),
    ]);
    let mut queue = crate::aq::aq_new(1024);
    let aq = &mut *queue as *mut AuQueue;
    let mut net = HlsNet { hs: std::ptr::null_mut(), curl: None };
    let deadline = Instant::now() + Duration::from_secs(5);
    let (selected, media) = initial_playback_media(&format!("{}/root.m3u8", server.base), true, deadline,
        &mut PlaybackPlaylist { owner: Owner::Playback(aq), transport: &mut net.curl }).unwrap();
    let resource = &media.segments[0].inner.resource;
    let (src, _, _) = hls_open_segment_source(resource, &resource.path, aq, &mut net, Some(deadline),
        &mut Owner::Playback(aq), HlsSegmentTransport::External).unwrap();
    let Src::Curl(mut source) = src else { panic!("external segment must retain curl context") };
    assert_eq!(drain(&mut source), b"segment");
    hls_recycle_src(Src::Curl(source), &mut net);
    let (landed, text) = fetch_in_session(&selected, &mut Owner::Playback(aq), &mut net.curl,
        deadline, PlaylistStage::Playback, 0).unwrap();
    assert!(matches!(playback_media_refresh(&landed, &text), Ok(_)));
    let heads = server.heads();
    assert_eq!(heads.len(), 4);
    assert!(heads[0].starts_with("GET /root.m3u8 "));
    assert!(heads[1].starts_with("GET /selected.m3u8 "));
    assert!(heads[2].starts_with("GET /part.ts "));
    assert!(heads[3].starts_with("GET /selected.m3u8 "));
    assert_eq!(server.accepts.load(Ordering::Acquire), 1, "all four requests reuse one connection");
    drop(net);
    crate::aq::aq_destroy(aq);
}

// Drive the same open as the segment producer, retaining the manifest's cookie jar.
fn segment_after_status(statuses: &[u16], open_budget: Duration, live: bool)
    -> (Result<Vec<u8>, HlsExit>, usize)
{
    let mut replies = vec![Reply::ok("", MEDIA)];
    for status in statuses {
        let mut failed = Reply::ok("Set-Cookie: segment=retry; Path=/\r\n", b"");
        failed.status = *status;
        replies.push(failed);
    }
    replies.push(Reply::ok("", b"recovered").requiring("segment=retry"));
    let server = SessionServer::new(replies);
    let mut queue = crate::aq::aq_new(1024);
    let aq = &mut *queue as *mut AuQueue;
    let mut net = HlsNet { hs: std::ptr::null_mut(), curl: None };
    let (_, media) = initial_playback_media(&format!("{}/live.m3u8", server.base), true,
        Instant::now() + Duration::from_secs(3),
        &mut PlaybackPlaylist { owner: Owner::Playback(aq), transport: &mut net.curl }).unwrap();
    let part = &media.segments[0].inner.resource;
    let transport = if live { HlsSegmentTransport::ExternalLive } else { HlsSegmentTransport::External };
    let result = hls_open_segment_source(part, &part.path, aq, &mut net,
        Some(Instant::now() + open_budget), &mut Owner::Playback(aq), transport)
        .map(|(source, _, _)| match source {
            Src::Curl(mut curl) => drain(&mut curl),
            _ => panic!("expected HTTP segment"),
        });
    let heads = server.heads();
    assert!(heads[1..].iter().all(|head| head.starts_with("GET /part.ts ")));
    drop(net);
    crate::aq::aq_destroy(aq);
    (result, heads.len() - 1)
}

#[test]
fn external_live_segment_retries_service_error_without_reopening_master_or_decoder() {
    let _serial = curl();
    let (result, attempts) = segment_after_status(&[503], Duration::from_secs(3), true);
    assert_eq!(result.unwrap(), b"recovered");
    assert_eq!(attempts, 2);
}

#[test]
fn external_live_segment_respects_refusals_deadline_and_finite_playback() {
    let _serial = curl();
    for (status, budget, live) in [(403, 3000, true), (429, 3000, true), (503, 100, true), (503, 3000, false)] {
        let (result, attempts) = segment_after_status(&[status], Duration::from_millis(budget), live);
        assert!(result.is_err());
        assert_eq!(attempts, 1);
    }
    let (result, attempts) = segment_after_status(&[503, 502, 504], Duration::from_secs(3), true);
    assert!(matches!(result, Err(HlsExit::OpenFailed { outcome: TraceOutcome::ServerState, .. })));
    assert_eq!(attempts, 3);
}

#[test]
fn first_http_failure_retains_issued_cookie_for_same_manifest_retry() {
    let _serial = curl();
    let mut unavailable = Reply::ok("Set-Cookie: sid=retry-session; Path=/\r\n", b"");
    unavailable.status = 503;
    let server = SessionServer::new(vec![unavailable, Reply::ok("", MEDIA).requiring("sid=retry-session")]);
    let cancelled = AtomicBool::new(false);
    let mut transport = None;
    fetch_in_session(&format!("{}/root.m3u8", server.base), &mut Owner::Inspect(&cancelled), &mut transport,
        Instant::now() + Duration::from_secs(3), PlaylistStage::Initial, 0).unwrap();
    assert_eq!(server.heads().len(), 2);
    assert!(server.heads()[1].contains("Cookie: sid=retry-session\r\n"));
    assert_eq!(server.accepts.load(Ordering::Acquire), 1);
}

#[test]
fn inspection_root_child_and_selected_segment_use_one_private_context() {
    let _serial = curl();
    let mut segment = Reply::ok("", b"").requiring("sid=inspection-session");
    segment.status = 403; // Stop before FFmpeg: this test needs no media build/runtime.
    let server = SessionServer::new(vec![
        Reply::ok("Set-Cookie: sid=inspection-session; Path=/\r\n", MASTER),
        Reply::ok("", MEDIA).requiring("sid=inspection-session"), segment,
    ]);
    assert!(inspect(&format!("{}/root.m3u8", server.base), Arc::new(AtomicBool::new(false))).is_err());
    let heads = server.heads();
    assert_eq!(heads.len(), 3);
    assert!(heads[2].starts_with("GET /part.ts "));
    assert!(heads[1..].iter().all(|head| head.contains("Cookie: sid=inspection-session\r\n")));
    assert_eq!(server.accepts.load(Ordering::Acquire), 1);
}

#[test]
fn sniffed_root_response_is_consumed_once_and_keeps_its_context() {
    let _serial = curl();
    let mut segment = Reply::ok("", b"").requiring("sid=sniff-session");
    segment.status = 403;
    let server = SessionServer::new(vec![
        Reply::ok("Set-Cookie: sid=sniff-session; Path=/\r\n", MASTER),
        Reply::ok("", MEDIA).requiring("sid=sniff-session"), segment,
    ]);
    let cancelled = Arc::new(AtomicBool::new(false));
    let deadline = Instant::now() + Duration::from_secs(3);
    let root = format!("{}/extensionless", server.base);
    let mut source = crate::curlio::CurlSource::open_hls_private_checked(&root, Some(deadline), &mut Owner::Inspect(&cancelled)).unwrap();
    let prefix = drain(&mut source);
    assert!(inspect_opened(&root, source, prefix, deadline, cancelled).is_err());
    let heads = server.heads();
    assert_eq!(heads.len(), 3, "the sniffed root must not be requested a second time");
    assert!(heads[2].starts_with("GET /part.ts "));
    assert!(heads[1..].iter().all(|head| head.contains("Cookie: sid=sniff-session\r\n")));
}

#[test]
fn cancelled_or_expired_manifest_context_makes_no_followup_request() {
    let _serial = curl();
    let server = SessionServer::new(vec![Reply::ok("Set-Cookie: sid=cancel-session; Path=/\r\n", MEDIA)]);
    let cancelled = AtomicBool::new(false);
    let mut transport = None;
    let url = format!("{}/root.m3u8", server.base);
    fetch_in_session(&url, &mut Owner::Inspect(&cancelled), &mut transport,
        Instant::now() + Duration::from_secs(3), PlaylistStage::Initial, 0).unwrap();
    let failure = fetch_in_session(&url, &mut Owner::Inspect(&cancelled), &mut transport,
        Instant::now(), PlaylistStage::Selected, 1).unwrap_err();
    assert!(matches!(failure.outcome, TraceOutcome::Deadline));
    cancelled.store(true, Ordering::Release);
    let failure = fetch_in_session(&url, &mut Owner::Inspect(&cancelled), &mut transport,
        Instant::now() + Duration::from_secs(3), PlaylistStage::Selected, 1).unwrap_err();
    assert!(failure.cancelled);
    assert_eq!(server.heads().len(), 1);
}

#[test]
fn early_segment_input_drop_returns_external_context_for_cold_entry_retry() {
    let _serial = curl();
    let server = SessionServer::new(vec![
        Reply::ok("Set-Cookie: sid=cold-entry-session; Path=/\r\n", b"dependent-segment"),
        Reply::ok("", b"next-segment").requiring("sid=cold-entry-session"),
    ]);
    let deadline = Instant::now() + Duration::from_secs(3);
    let url = format!("{}/part.ts", server.base);
    let mut source = crate::curlio::CurlSource::open_hls_private_checked(&url, Some(deadline), &mut plx_base::checkpoint::NoCheckpoint).unwrap();
    drain(&mut source);
    let mut slot = None;
    {
        // An early decode/probe refusal drops this same owner. Null FFmpeg
        // pointers exercise only the transport return; no FFmpeg ABI is called.
        let _input = HlsInput {
            return_curl: &mut slot,
            state: Box::new(AvioState {
                hls_ts: None, src: Src::Curl(source), aq: std::ptr::null_mut(), off: 0, size: -1,
                io_failed: false, io_failure_outcome: None, body_active_us: 0, body_bytes: 0, first_byte_at: None,
                reserve_deadline: ReserveDeadlineState::new(None, false), transport_watchdog: None,
                acquisition: None, bounce: Vec::new(), bounce_pos: 0, parked_at_end: false,
                heal: HealBudget::default(),
            }),
            avio: std::ptr::null_mut(), fmt: std::ptr::null_mut(), pkt: std::ptr::null_mut(),
        };
    }
    let mut source = slot.expect("an early exit must return the source-owned HTTP context");
    source.reopen_until(&format!("{}/next.ts", server.base), Some(deadline), &mut plx_base::checkpoint::NoCheckpoint).unwrap();
    assert_eq!(drain(&mut source), b"next-segment");
    assert_eq!(server.heads().len(), 2);
    assert!(server.heads()[1].contains("Cookie: sid=cold-entry-session\r\n"));
}
