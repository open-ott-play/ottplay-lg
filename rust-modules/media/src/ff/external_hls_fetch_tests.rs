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
            assert_eq!(trace.open_error(error), message);
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
