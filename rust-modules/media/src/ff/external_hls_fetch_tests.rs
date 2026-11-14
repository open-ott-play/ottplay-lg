//! Actual loopback HTTP acquisition; no provider, FFmpeg or device is needed.
use super::*;
use std::io::{Read, Write};
use std::sync::atomic::AtomicUsize;

struct Server {
    url: String,
    count: Arc<AtomicUsize>,
    stop: Arc<AtomicBool>,
    worker: Option<std::thread::JoinHandle<()>>,
}
impl Server {
    fn start(
        status: u16,
        header_delay: Duration,
        body_delay: Duration,
        cancel: Option<Arc<AtomicBool>>,
    ) -> Self {
        Self::script(
            vec![(
                status,
                b"#EXTM3U\n#EXT-X-TARGETDURATION:1\n#EXTINF:1,\npart.ts\n#EXT-X-ENDLIST\n".to_vec(),
            )],
            header_delay,
            body_delay,
            cancel,
        )
    }

    fn script(
        responses: Vec<(u16, Vec<u8>)>,
        header_delay: Duration,
        body_delay: Duration,
        cancel: Option<Arc<AtomicBool>>,
    ) -> Self {
        assert!(!responses.is_empty());
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        listener.set_nonblocking(true).unwrap();
        let count = Arc::new(AtomicUsize::new(0));
        let seen = count.clone();
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = stop.clone();
        let worker = std::thread::spawn(move || {
            while !stopped.load(Ordering::Acquire) {
                let Ok((mut socket, _)) = listener.accept() else {
                    std::thread::sleep(Duration::from_millis(2));
                    continue;
                };
                socket.set_nonblocking(false).unwrap();
                socket
                    .set_read_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                socket
                    .set_write_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                let mut header = Vec::new();
                let mut byte = [0];
                while header.len() < 8192 && !header.ends_with(b"\r\n\r\n") {
                    if socket.read(&mut byte).unwrap_or(0) == 0 {
                        break;
                    }
                    header.push(byte[0]);
                }
                let request = seen.fetch_add(1, Ordering::Release);
                let (status, body) = &responses[request.min(responses.len() - 1)];
                if let Some(cancel) = &cancel {
                    cancel.store(true, Ordering::Release);
                }
                std::thread::sleep(header_delay);
                if *status == 0 {
                    continue;
                } // Close before a response: real transport failure.
                let location = if *status == 302 { "Location: /landing/manifest\r\n" } else { "" };
                let response = format!(
                    "HTTP/1.1 {status} Fixture\r\n{location}Content-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = socket.write_all(response.as_bytes());
                std::thread::sleep(body_delay);
                let _ = socket.write_all(body);
            }
        });
        Self {
            url: format!("http://{addr}/private-path-sentinel?q=private-query-sentinel"),
            count,
            stop,
            worker: Some(worker),
        }
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        self.worker.take().unwrap().join().unwrap();
    }
}

fn curl() -> plx_base::testlock::Serial {
    let serial = plx_base::testlock::serial();
    assert!(plx_net::net::global_init());
    assert!(
        crate::curlio::available(),
        "loopback tests require host libcurl"
    );
    serial
}

fn captured(
    f: impl FnOnce() -> Result<(String, String), &'static str>,
) -> (Result<(String, String), &'static str>, Vec<String>) {
    plx_base::eventlog::with_private_log(|| {
        let result = f();
        let log = std::fs::read_to_string(plx_base::eventlog::events_log()).unwrap_or_default();
        let lines: Vec<_> = log
            .lines()
            .filter(|line| line.contains("hls: playlist stage="))
            .map(str::to_owned)
            .collect();
        for line in &lines {
            assert!(
                !line.contains("127.0.0.1")
                    && !line.contains("private-path-sentinel")
                    && !line.contains("private-query-sentinel")
                    && !line.contains("q="),
                "{line}"
            );
            assert!(line.contains("elapsed_ms="), "{line}");
        }
        (result, lines)
    })
}

#[test]
fn real_inspection_labels_master_then_selected_playlist_failure() {
    let _serial = curl();
    let server = Server::script(
        vec![
            (
                200,
                b"#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=100\n/selected?q=private-query-sentinel\n"
                    .to_vec(),
            ),
            (403, b"private-response-sentinel".to_vec()),
        ],
        Duration::ZERO,
        Duration::ZERO,
        None,
    );
    let (result, lines) = captured(|| {
        inspect(&server.url, Arc::new(AtomicBool::new(false)))
            .map(|_| (String::new(), String::new()))
    });
    assert_eq!(result.unwrap_err(), "HLS playlist server returned HTTP 403");
    assert_eq!(
        server.count.load(Ordering::Acquire),
        2,
        "one master and one selected request, no retry"
    );
    assert!(lines
        .iter()
        .any(|line| line.contains("stage=initial depth=0 phase=complete")));
    assert!(lines
        .iter()
        .any(|line| line.contains("stage=selected depth=1 phase=open")
            && line.contains("kind=http code=403")));
    assert!(!lines
        .iter()
        .any(|line| line.contains("private-response-sentinel")));
}

#[test]
fn playlist_http_rejections_are_distinct_static_and_not_retried() {
    let _serial = curl();
    for (status, message) in [
        (401, "HLS playlist server returned HTTP 401"),
        (403, "HLS playlist server returned HTTP 403"),
        (404, "HLS playlist server returned HTTP 404"),
        (429, "HLS playlist server returned HTTP 429"),
        (503, "HLS playlist server returned a server error"),
    ] {
        let server = Server::start(status, Duration::ZERO, Duration::ZERO, None);
        let cancelled = AtomicBool::new(false);
        let (result, lines) = captured(|| {
            fetch_traced(
                &server.url,
                &mut Owner::Inspect(&cancelled),
                Instant::now() + Duration::from_secs(2),
                PlaylistStage::Selected,
                1,
            )
        });
        assert_eq!(result.unwrap_err(), message);
        assert_eq!(server.count.load(Ordering::Acquire), 1);
        assert!(lines
            .iter()
            .any(|line| line.contains("stage=selected depth=1 phase=open")
                && line.contains(&format!("kind=http code={status}"))));
    }
}

#[test]
fn playlist_header_and_body_deadlines_remain_distinct_and_bounded() {
    let _serial = curl();
    for (header, body, stage, message, phase) in [
        (
            180,
            0,
            PlaylistStage::Initial,
            "Initial HLS playlist request timed out",
            "open",
        ),
        (
            180,
            0,
            PlaylistStage::Selected,
            "Selected HLS playlist request timed out",
            "open",
        ),
        (
            0,
            180,
            PlaylistStage::Selected,
            "HLS playlist body transfer timed out",
            "body",
        ),
    ] {
        let server = Server::start(
            200,
            Duration::from_millis(header),
            Duration::from_millis(body),
            None,
        );
        let cancelled = AtomicBool::new(false);
        let began = Instant::now();
        let (result, lines) = captured(|| {
            fetch_traced(
                &server.url,
                &mut Owner::Inspect(&cancelled),
                began + Duration::from_millis(60),
                stage,
                1,
            )
        });
        assert_eq!(result.unwrap_err(), message);
        assert!(
            began.elapsed() < Duration::from_secs(1),
            "deadline must not wait for the fixture body"
        );
        assert_eq!(server.count.load(Ordering::Acquire), 1);
        assert!(
            lines
                .iter()
                .any(|line| line.contains(&format!("phase={phase}"))
                    && line.contains("kind=deadline"))
        );
    }
}

#[test]
fn cancelled_inspection_keeps_cancellation_without_waiting_or_retrying() {
    let _serial = curl();
    let cancelled = Arc::new(AtomicBool::new(false));
    let server = Server::start(
        200,
        Duration::from_millis(180),
        Duration::ZERO,
        Some(cancelled.clone()),
    );
    let (result, lines) = captured(|| {
        fetch_traced(
            &server.url,
            &mut Owner::Inspect(&cancelled),
            Instant::now() + Duration::from_secs(2),
            PlaylistStage::Selected,
            1,
        )
    });
    assert_eq!(result.unwrap_err(), "HLS request cancelled");
    assert!(lines.iter().any(|line| line.contains("kind=cancelled")));
    assert_eq!(server.count.load(Ordering::Acquire), 1);
    let (again, _) = captured(|| fetch(&server.url, &mut Owner::Inspect(&cancelled)));
    assert_eq!(again.unwrap_err(), "HLS request cancelled");
    assert_eq!(
        server.count.load(Ordering::Acquire),
        1,
        "retired request cannot start another open"
    );
}

#[test]
fn playlist_transport_failure_is_not_http_or_timeout_and_success_is_unchanged() {
    let _serial = curl();
    for status in [0, 200] {
        let server = Server::start(status, Duration::ZERO, Duration::ZERO, None);
        let cancelled = AtomicBool::new(false);
        let (result, lines) = captured(|| fetch(&server.url, &mut Owner::Inspect(&cancelled)));
        if status == 0 {
            assert_eq!(result.unwrap_err(), "HLS playlist network transport failed");
            assert!(lines
                .iter()
                .any(|line| line.contains("phase=open") && line.contains("kind=transport code=")));
        } else {
            let (landed, text) = result.unwrap();
            assert_eq!(landed, server.url);
            assert!(matches!(parse(&landed, &text), Ok(Playlist::Media(_))));
            assert!(
                lines.is_empty(),
                "shared refresh success must not produce ongoing telemetry"
            );
        }
        assert_eq!(server.count.load(Ordering::Acquire), 1);
        if status == 200 {
            let (result, lines) = captured(|| {
                fetch_traced(
                    &server.url,
                    &mut Owner::Inspect(&cancelled),
                    Instant::now() + Duration::from_secs(2),
                    PlaylistStage::Initial,
                    0,
                )
            });
            assert!(result.is_ok());
            assert_eq!(
                lines.len(),
                2,
                "initial acquisition keeps bounded open/complete timings"
            );
            assert!(lines
                .iter()
                .any(|line| line.contains("stage=initial depth=0 phase=complete")));
            assert_eq!(server.count.load(Ordering::Acquire), 2);
        }
    }
}

#[test]
fn open_error_mapping_is_closed_for_every_transport_variant() {
    use crate::curlio::OpenErr;
    let _serial = plx_base::testlock::serial();
    plx_base::eventlog::with_private_log(|| {
        let trace = FetchTrace {
            stage: PlaylistStage::Shared,
            depth: 0,
            started: Instant::now(),
        };
        for (error, message) in [
            (OpenErr::Deadline, "HLS playlist request timed out"),
            (OpenErr::Aborted, "HLS request cancelled"),
            (OpenErr::Stopped, "HLS request cancelled"),
            (
                OpenErr::Status(418),
                "HLS playlist server returned an HTTP error",
            ),
            (
                OpenErr::Transport(6),
                "HLS playlist network transport failed",
            ),
            (OpenErr::Multi(1), "HLS playlist local transport failed"),
            (OpenErr::Local, "HLS playlist local transport setup failed"),
            (OpenErr::Unavailable, "HLS transport unavailable"),
            (
                OpenErr::RangeIgnored,
                "HLS playlist server refused the requested byte range",
            ),
        ] {
            assert_eq!(trace.open_error(error).reason, message);
        }
    });
}

/// Model extensionless sniffing: the transport has already validated headers
/// and consumed a prefix before handing the exact response to HLS inspection.
fn opened_prefix(url: &str, cancel: &Arc<AtomicBool>, deadline: Instant)
    -> (Box<crate::curlio::CurlSource>, Vec<u8>)
{
    let mut owner = Owner::Inspect(cancel);
    let (mut source, _) = crate::curlio::CurlSource::open_private_checked(url, 0, Some(deadline), &mut owner).unwrap();
    let mut prefix = [0; 16];
    let n = source.read_until(&mut prefix, Some(deadline), &mut owner);
    assert!(n > 0);
    (source, prefix[..n as usize].to_vec())
}

#[test]
fn sniffed_master_reuses_its_response_and_fetches_selected_only_once() {
    let _serial = curl();
    let server = Server::script(vec![
        (200, b"#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=100\n/selected\n".to_vec()),
        (403, b"private-response-sentinel".to_vec()),
    ], Duration::ZERO, Duration::ZERO, None);
    let cancel = Arc::new(AtomicBool::new(false));
    let deadline = Instant::now() + Duration::from_secs(2);
    let (source, prefix) = opened_prefix(&server.url, &cancel, deadline);
    let (result, lines) = captured(|| inspect_opened(source, prefix, deadline, cancel)
        .map(|_| (String::new(), String::new())));
    assert_eq!(result.unwrap_err(), "HLS playlist server returned HTTP 403");
    assert_eq!(server.count.load(Ordering::Acquire), 2,
        "one already-open master and one selected playlist; never a master refetch");
    assert!(lines.iter().any(|line| line.contains("stage=initial depth=0 phase=complete")));
    assert!(lines.iter().any(|line| line.contains("stage=selected depth=1 phase=open") && line.contains("code=403")));
}

#[test]
fn sniffed_manifest_still_validates_body_and_honours_cancel_without_refetch() {
    let _serial = curl();
    for cancelled in [false, true] {
        let server = Server::script(vec![(200, b"#EXTM3U\n#EXT-X-TARGETDURATION:not-a-number\n".to_vec())],
            Duration::ZERO, Duration::ZERO, None);
        let cancel = Arc::new(AtomicBool::new(false));
        let deadline = Instant::now() + Duration::from_secs(2);
        let (source, prefix) = opened_prefix(&server.url, &cancel, deadline);
        cancel.store(cancelled, Ordering::Relaxed);
        let result = inspect_opened(source, prefix, deadline, cancel);
        assert_eq!(result.unwrap_err(), if cancelled { "HLS request cancelled" } else { "Invalid HLS target duration" });
        assert_eq!(server.count.load(Ordering::Acquire), 1);
    }
}


#[test]
fn sniffed_redirect_response_keeps_landed_url_without_reopening_it() {
    let _serial = curl();
    let server = Server::script(vec![
        (302, Vec::new()),
        (200, b"#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=100\nselected\n".to_vec()),
        (404, Vec::new()),
    ], Duration::ZERO, Duration::ZERO, None);
    let cancel = Arc::new(AtomicBool::new(false));
    let deadline = Instant::now() + Duration::from_secs(2);
    let (source, prefix) = opened_prefix(&server.url, &cancel, deadline);
    assert!(source.effective_url().ends_with("/landing/manifest"));
    let result = inspect_opened(source, prefix, deadline, cancel);
    assert_eq!(result.unwrap_err(), "HLS playlist server returned HTTP 404");
    assert_eq!(server.count.load(Ordering::Acquire), 3,
        "redirect, its admitted master body, then the selected playlist exactly once");
}

struct ProducerStateReset;
impl Drop for ProducerStateReset {
    fn drop(&mut self) {
        SHARED.reset_session();
        PUSHED_ANY.store(false, Ordering::Relaxed);
    }
}

#[test]
fn live_refresh_http_failure_keeps_checked_cause_through_producer_settlement() {
    let _serial = curl();
    let _reset = ProducerStateReset;
    for (status, expected) in [(503, TraceOutcome::ServerState), (429, TraceOutcome::ServerState),
        (401, TraceOutcome::Refused), (403, TraceOutcome::Refused), (404, TraceOutcome::Refused),
        (0, TraceOutcome::Transport)] {
        SHARED.reset_session();
        let epoch = SHARED.begin_native_session().unwrap();
        // The worker already fed earlier segments. Its next real checked HTTP
        // refresh fails before another segment/decoder is acquired.
        PUSHED_ANY.store(true, Ordering::Relaxed);
        let server = Server::start(status, Duration::ZERO, Duration::ZERO, None);
        let mut video = crate::aq::aq_new(1024);
        let mut audio = crate::aq::aq_new(1024);
        let mut stream = plx_net::stream::http_stream_boxed();
        let result = demux_video(&server.url, "h264", "aac", None,
            &mut *video, &mut *audio, &mut *stream, None);
        plx_net::stream::http_close(&mut *stream);
        crate::aq::aq_destroy(&mut *video);
        crate::aq::aq_destroy(&mut *audio);
        assert!(matches!(&result, Err(HlsExit::OpenFailed { outcome, .. }) if *outcome == expected));
        super::super::settle_hls_demux(epoch, Ok(result));
        assert_eq!(SHARED.io_failure(), Some(IoFailureDetails::Hls {
            exit: DemuxExitClass::HlsPlaylistFetchFailed, outcome: Some(expected),
        }));
        assert!(SHARED.demux_io_failed.load(Ordering::Acquire));
        assert!(!SHARED.demux_failed.load(Ordering::Acquire));
        assert_eq!(server.count.load(Ordering::Acquire), 1,
            "one refresh; no hidden retry, segment request or alternate source");
    }
}

#[test]
fn live_refresh_header_and_body_deadlines_survive_and_retired_owner_cannot_attribute_replacement() {
    let _serial = curl();
    let _reset = ProducerStateReset;
    for body in [false, true] {
        SHARED.reset_session();
        let epoch = SHARED.begin_native_session().unwrap();
        PUSHED_ANY.store(true, Ordering::Relaxed);
        let delay = Duration::from_millis(160);
        let server = Server::start(200, if body { Duration::ZERO } else { delay },
            if body { delay } else { Duration::ZERO }, None);
        let mut queue = crate::aq::aq_new(1024);
        let failure = fetch_traced_failure(&server.url, &mut Owner::Playback(&mut *queue),
            Instant::now() + Duration::from_millis(60), PlaylistStage::Playback, 0).unwrap_err();
        crate::aq::aq_destroy(&mut *queue);
        assert_eq!(failure.outcome, TraceOutcome::Deadline);
        let reason = failure.reason;
        super::super::settle_hls_demux(epoch, Ok(Err(failure.into_hls())));
        assert_eq!(SHARED.io_failure(), Some(IoFailureDetails::Hls {
            exit: DemuxExitClass::HlsPlaylistFetchFailed, outcome: Some(TraceOutcome::Deadline),
        }));
        assert!(SHARED.demux_io_failed.load(Ordering::Acquire));
        SHARED.reset_session();
        let replacement = SHARED.begin_native_session().unwrap();
        assert_ne!(replacement, epoch);
        super::super::settle_hls_demux(epoch, Ok(Err(HlsExit::OpenFailed {
            reason, outcome: TraceOutcome::Deadline,
        })));
        assert_eq!(SHARED.io_failure(), None);
        assert!(!SHARED.demux_io_failed.load(Ordering::Acquire));
    }
}

#[test]
fn live_refresh_trust_range_and_cancel_refusals_never_gain_transient_cause() {
    let trace = FetchTrace { stage: PlaylistStage::Playback, depth: 0, started: Instant::now() };
    use crate::curlio::OpenErr;
    for error in [OpenErr::Transport(60), OpenErr::Transport(47), OpenErr::RangeIgnored,
        OpenErr::Aborted, OpenErr::Stopped, OpenErr::Local] {
        let failure = trace.open_error(error);
        assert!(!matches!(failure.outcome,
            TraceOutcome::Transport | TraceOutcome::Deadline | TraceOutcome::ServerState));
    }
}

#[test]
fn cancelled_playback_refresh_does_not_publish_a_transport_failure() {
    let _serial = curl();
    let _reset = ProducerStateReset;
    SHARED.reset_session();
    let epoch = SHARED.begin_native_session().unwrap();
    PUSHED_ANY.store(true, Ordering::Relaxed);
    let server = Server::start(200, Duration::ZERO, Duration::ZERO, None);
    let mut queue = crate::aq::aq_new(1024);
    crate::aq::aq_abort(&mut *queue);
    let failure = fetch_traced_failure(&server.url, &mut Owner::Playback(&mut *queue),
        Instant::now() + Duration::from_secs(1), PlaylistStage::Playback, 0).unwrap_err();
    crate::aq::aq_destroy(&mut *queue);
    let result = failure.into_hls();
    assert!(matches!(result, HlsExit::Aborted));
    super::super::settle_hls_demux(epoch, Ok(Err(result)));
    assert_eq!(SHARED.io_failure(), None);
    assert!(!SHARED.demux_io_failed.load(Ordering::Acquire));
    assert_eq!(server.count.load(Ordering::Acquire), 0);
}

#[test]
fn malformed_live_refresh_cannot_gain_a_retryable_network_cause() {
    let _serial = curl();
    let _reset = ProducerStateReset;
    for body in [b"not-a-playlist".to_vec(), vec![0xff, 0xfe]] {
        SHARED.reset_session();
        let epoch = SHARED.begin_native_session().unwrap();
        PUSHED_ANY.store(true, Ordering::Relaxed);
        let server = Server::script(vec![(200, body)], Duration::ZERO, Duration::ZERO, None);
        let mut video = crate::aq::aq_new(1024);
        let mut audio = crate::aq::aq_new(1024);
        let mut stream = plx_net::stream::http_stream_boxed();
        let result = demux_video(&server.url, "h264", "aac", None,
            &mut *video, &mut *audio, &mut *stream, None);
        plx_net::stream::http_close(&mut *stream);
        crate::aq::aq_destroy(&mut *video);
        crate::aq::aq_destroy(&mut *audio);
        assert!(result.is_err());
        super::super::settle_hls_demux(epoch, Ok(result));
        assert!(matches!(SHARED.io_failure(), Some(IoFailureDetails::Hls {
            exit: DemuxExitClass::HlsPlaylistInvalid,
            outcome: None | Some(TraceOutcome::Refused),
        })));
        assert_eq!(server.count.load(Ordering::Acquire), 1);
    }
}


#[test]
fn segment_missing_after_bounded_retries_preserves_server_state_but_auth_stays_refused() {
    let _serial = curl();
    let _reset = ProducerStateReset;
    for status in [404, 401, 403] {
        SHARED.reset_session();
        let server = Server::start(status, Duration::ZERO, Duration::ZERO, None);
        let segment = crate::hls::Segment { sequence: 1, duration: Duration::from_millis(100),
            resource: resource(&server.url).unwrap() };
        let mut queue = crate::aq::aq_new(1024);
        let mut stream = plx_net::stream::http_stream_boxed();
        let mut net = HlsNet { hs: &mut *stream, curl: None };
        let mut clock = crate::hls::SegmentTimeline::default().begin(segment.duration).unwrap();
        let mut state = H264SegmentState::default();
        let start = Instant::now();
        let result = unsafe { hls_demux_segment_path(&segment,
            HlsSegmentRequest { path: &segment.resource.path, transport: HlsSegmentTransport::External,
                body_limit: None, audio: None, rendition: None,
                video: HlsVideoEntry::External(&mut state), mp2: None },
            &mut clock, &mut *queue, &mut net, "", SegmentAcquisition::for_cursor(None)) };
        drop(net);
        plx_net::stream::http_close(&mut *stream);
        crate::aq::aq_destroy(&mut *queue);
        if status == 404 {
            assert!(matches!(result, Err(HlsExit::OpenFailed {
                reason: "HLS segment was not produced in time", outcome: TraceOutcome::ServerState })),
                "spent actual 404 retry state must not be erased: {:?}", result.as_ref().err());
            assert!(start.elapsed() >= Duration::from_secs(3), "preserve existing retry budget");
            assert!((2..=20).contains(&server.count.load(Ordering::Acquire)));
        } else {
            assert!(matches!(result, Err(HlsExit::OpenFailed { outcome: TraceOutcome::Refused, .. })),
                "authentication refusal must stay terminal: {:?}", result.as_ref().err());
            assert_eq!(server.count.load(Ordering::Acquire), 1);
        }
    }
}
