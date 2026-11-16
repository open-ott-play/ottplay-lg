//! Closed read evidence and the actual checked CurlSource -> inspection AVIO seam.
//! No playback session, decoder or provider is started by these tests.
use super::*;
use std::io::{Read, Write};
use std::sync::atomic::{AtomicUsize, Ordering};

fn body(rc: i32) -> crate::curlio::BodyFailure {
    crate::curlio::BodyFailure { rc, bytes: 3, age_ms: 10, reopen_failed: false }
}

#[test]
fn checked_body_read_failure_preserves_only_positive_transient_causes() {
    for (result, failure, expected) in [
        (crate::curlio::READ_DEADLINE, None, Some("Media source request timed out")),
        (-1, Some(body(28)), Some("Media source request timed out")),
        (-1, Some(body(18)), Some("Media source network transport failed")),
        (-1, Some(body(56)), Some("Media source network transport failed")),
        (-1, Some(body(60)), None),
        (-1, Some(body(35)), None),
        (-1, Some(body(999)), None),
        (-1, None, None),
        (crate::curlio::READ_STOPPED, Some(body(56)), None),
        (-1, Some(crate::curlio::BodyFailure { reopen_failed: true, ..body(56) }), None),
    ] {
        let mut evidence = ReadEvidence::None;
        evidence.observe(result, failure, true);
        assert_eq!(evidence.failure_reason(true, false, false), expected, "read={result} failure={failure:?}");
    }
}

#[test]
fn header_sniff_uses_the_same_checked_body_evidence_without_guessing_parse_failures() {
    assert_eq!(initial_read_failure(-1, Some(body(56)), false), "Media source network transport failed");
    assert_eq!(initial_read_failure(crate::curlio::READ_DEADLINE, None, false), "Media source request timed out");
    assert_eq!(initial_read_failure(-1, Some(body(60)), false), "Could not read media source");
    assert_eq!(initial_read_failure(-1, Some(body(56)), true), "Could not read media source");
    assert_eq!(initial_read_failure(0, None, false), "Could not read media source");
}

#[test]
fn successful_partial_probe_cancel_and_byte_cap_never_become_transient_failures() {
    let mut evidence = ReadEvidence::None;
    evidence.observe(-1, Some(body(18)), true);
    assert_eq!(evidence.failure_reason(false, false, false), None,
        "a successful bounded prefix inspection need not consume the whole resource");
    assert_eq!(evidence.failure_reason(true, true, false), None);
    assert_eq!(evidence.failure_reason(true, false, true), None);
    evidence.stopped(true, true, false, true);
    assert_eq!(evidence, ReadEvidence::Refused, "cancel wins over deadline");
    evidence = ReadEvidence::Transport;
    evidence.stopped(true, false, true, true);
    assert_eq!(evidence, ReadEvidence::Refused, "byte cap is not network failure");
    evidence = ReadEvidence::None;
    evidence.observe(-1, Some(body(56)), false);
    assert_eq!(evidence.failure_reason(true, false, false), None, "local/owned inputs do not borrow HTTP retry evidence");
}

#[test]
fn clean_short_or_malformed_body_and_healed_reads_do_not_keep_an_old_network_error() {
    let mut evidence = ReadEvidence::None;
    for result in [3, 0] {
        evidence.observe(result, None, true);
        assert_eq!(evidence.failure_reason(true, false, false), None,
            "a parser rejecting a successfully transferred body stays a parser error");
    }
    evidence.observe(-1, Some(body(56)), true);
    assert_eq!(evidence, ReadEvidence::Transport);
    evidence.observe(16, None, true);
    assert_eq!(evidence.failure_reason(true, false, false), None, "later successful reads withdraw old evidence");
    evidence = ReadEvidence::Refused;
    evidence.observe(crate::curlio::READ_DEADLINE, None, true);
    assert_eq!(evidence, ReadEvidence::Refused, "a later failure cannot conceal a refused reopen");
    evidence.observe(16, None, true);
    evidence.observe(-1, Some(body(56)), true);
    assert_eq!(evidence, ReadEvidence::Transport, "fresh successful IO establishes a new failure scope");
}

struct Reply { status: u16, declared: usize, bytes: &'static [u8], wait_for_release: bool }
impl Reply {
    fn short() -> Self { Self { status: 200, declared: 1024, bytes: b"abc", wait_for_release: true } }
    fn complete(bytes: &'static [u8]) -> Self {
        Self { status: 200, declared: bytes.len(), bytes, wait_for_release: false }
    }
}
struct Server {
    url: String,
    stop: Arc<AtomicBool>,
    requests: Arc<AtomicUsize>,
    release: Arc<AtomicBool>,
    worker: Option<std::thread::JoinHandle<()>>,
}
impl Server {
    fn new(replies: Vec<Reply>) -> Self {
        assert!(!replies.is_empty());
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        listener.set_nonblocking(true).unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = Arc::clone(&stop);
        let requests = Arc::new(AtomicUsize::new(0));
        let seen = Arc::clone(&requests);
        let release = Arc::new(AtomicBool::new(false));
        let released = Arc::clone(&release);
        let worker = std::thread::spawn(move || {
            while !stopped.load(Ordering::Acquire) {
                let Ok((mut socket, _)) = listener.accept() else {
                    std::thread::sleep(Duration::from_millis(2));
                    continue;
                };
                socket.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
                socket.set_write_timeout(Some(Duration::from_secs(2))).unwrap();
                let mut request = Vec::new();
                let mut byte = [0];
                while request.len() < 8192 && !request.ends_with(b"\r\n\r\n") {
                    if socket.read(&mut byte).unwrap_or(0) == 0 { break; }
                    request.push(byte[0]);
                }
                let index = seen.fetch_add(1, Ordering::Release).min(replies.len() - 1);
                let reply = &replies[index];
                let response = format!("HTTP/1.1 {} Fixture\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    reply.status, reply.declared);
                let _ = socket.write_all(response.as_bytes());
                let _ = socket.write_all(reply.bytes);
                while reply.wait_for_release && !released.load(Ordering::Acquire) && !stopped.load(Ordering::Acquire) {
                    std::thread::sleep(Duration::from_millis(2));
                }
            }
        });
        Self { url: format!("http://{address}/inspection"), stop, requests, release, worker: Some(worker) }
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
    assert!(crate::curlio::available(), "loopback inspection tests require host libcurl");
    serial
}
fn input(server: &Server, body_allowance: Duration) -> Input {
    let cancel = Arc::new(AtomicBool::new(false));
    let deadline = Instant::now() + Duration::from_secs(2);
    let (source, _) = crate::curlio::CurlSource::open_private_checked(&server.url, 0, Some(deadline),
        &mut InspectCheckpoint(&cancel)).unwrap();
    assert_eq!(source.status(), 200, "body evidence follows checked successful headers");
    Input { source: InputSource::Http(source), deadline: Instant::now() + body_allowance,
        cancelled: cancel, bytes: 0, offset: 0, parked_at_end: false, prefix: Vec::new(),
        prefix_at: 0, initial_transport: None, checked_http: true, read_evidence: ReadEvidence::None }
}
fn read_input(input: &mut Input) -> i32 {
    let mut bytes = [0; 256];
    read((input as *mut Input).cast(), bytes.as_mut_ptr(), bytes.len() as i32)
}

#[test]
fn checked_after_headers_partial_transfer_reaches_the_real_inspection_read_callback() {
    let _serial = curl();
    let server = Server::new(vec![Reply::short()]);
    let mut input = input(&server, Duration::from_secs(2));
    server.release.store(true, Ordering::Release);
    assert_eq!(read_input(&mut input), 3);
    assert_eq!(read_input(&mut input), AVERROR_IO);
    assert!(input.source.body_failure().is_some_and(|failure| failure.rc == 18 && !failure.reopen_failed));
    assert_eq!(input.primary_read_failure(true), Some("Media source network transport failed"));
    assert_eq!(input.primary_read_failure(false), None, "successful prefix inspection is not a failed transfer verdict");
    input.cancelled.store(true, Ordering::Release);
    assert_eq!(input.primary_read_failure(true), None, "late cancellation still wins");
}

#[test]
fn checked_body_deadline_is_distinct_from_cancellation_and_the_byte_cap() {
    let _serial = curl();
    let server = Server::new(vec![Reply { status: 200, declared: 1024, bytes: b"", wait_for_release: true }]);
    let mut input = input(&server, Duration::from_millis(50));
    assert_eq!(read_input(&mut input), AVERROR_IO);
    assert_eq!(input.primary_read_failure(true), Some("Media source request timed out"));
    input.bytes = INSPECTION_BYTES;
    assert_eq!(read_input(&mut input), AVERROR_IO);
    assert_eq!(input.primary_read_failure(true), None, "limit exhaustion is terminal even at a timed-out read");
}

#[test]
fn refused_reopen_cannot_reuse_the_previous_partial_body_failure() {
    let _serial = curl();
    let server = Server::new(vec![Reply::short(), Reply { status: 403, declared: 0, bytes: b"", wait_for_release: false },
        Reply::complete(b"fresh body")]);
    let mut input = input(&server, Duration::from_secs(2));
    server.release.store(true, Ordering::Release);
    assert_eq!(read_input(&mut input), 3);
    assert_eq!(read_input(&mut input), AVERROR_IO);
    assert_eq!(input.primary_read_failure(true), Some("Media source network transport failed"));
    assert_eq!(seek((&mut input as *mut Input).cast(), 0, SEEK_SET), -1);
    assert_eq!(input.primary_read_failure(true), None, "HTTP 403 is not the prior transfer's failure");
    assert_eq!(seek((&mut input as *mut Input).cast(), 0, SEEK_SET), 0);
    assert_eq!(read_input(&mut input), 10);
    assert_eq!(input.primary_read_failure(true), None, "a healed reopen clears the old error");
    assert_eq!(server.requests.load(Ordering::Acquire), 3);
}

#[test]
fn clean_short_body_does_not_manufacture_transport_retry_evidence() {
    let _serial = curl();
    let server = Server::new(vec![Reply::complete(b"not media")]);
    let mut input = input(&server, Duration::from_secs(2));
    assert_eq!(read_input(&mut input), 9);
    assert_eq!(read_input(&mut input), AVERROR_EOF);
    assert_eq!(input.primary_read_failure(true), None,
        "a later parser rejection of these bytes must retain its original error");
}
