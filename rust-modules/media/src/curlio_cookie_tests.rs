//! Memory-only source-owned HTTP context. These fixtures never contact a provider.
use super::*;
use std::io::{Read, Write};
use std::sync::atomic::AtomicUsize;
use std::time::{Duration, Instant};

pub(crate) struct Reply {
    pub status: u16,
    pub headers: String,
    pub body: Vec<u8>,
    pub required: Option<&'static str>,
}
impl Reply {
    pub(crate) fn ok(headers: &str, body: &[u8]) -> Self {
        Self { status: 200, headers: headers.into(), body: body.into(), required: None }
    }
    pub(crate) fn requiring(mut self, cookie: &'static str) -> Self {
        self.required = Some(cookie);
        self
    }
}

pub(crate) struct SessionServer {
    pub base: String,
    pub accepts: Arc<AtomicUsize>,
    pub requests: Arc<Mutex<Vec<String>>>,
    stop: Arc<AtomicBool>,
    worker: Option<std::thread::JoinHandle<()>>,
}
impl SessionServer {
    pub(crate) fn new(replies: Vec<Reply>) -> Self {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        listener.set_nonblocking(true).unwrap();
        for host in ["session-cookie.invalid", "other-cookie.invalid"] {
            super::test_fixtures::pin_to_loopback(host, port);
        }
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = Arc::clone(&stop);
        let accepts = Arc::new(AtomicUsize::new(0));
        let accepted = Arc::clone(&accepts);
        let requests = Arc::new(Mutex::new(Vec::new()));
        let seen = Arc::clone(&requests);
        let replies = Arc::new(replies);
        let worker = std::thread::spawn(move || {
            let mut connections = Vec::new();
            while !stopped.load(Ordering::Acquire) {
                let Ok((mut socket, _)) = listener.accept() else {
                    std::thread::sleep(Duration::from_millis(2));
                    continue;
                };
                accepted.fetch_add(1, Ordering::AcqRel);
                socket.set_read_timeout(Some(Duration::from_millis(50))).unwrap();
                socket.set_write_timeout(Some(Duration::from_secs(2))).unwrap();
                let stopped = Arc::clone(&stopped);
                let seen = Arc::clone(&seen);
                let replies = Arc::clone(&replies);
                connections.push(std::thread::spawn(move || {
                    while !stopped.load(Ordering::Acquire) {
                        let mut head = Vec::new();
                        let mut byte = [0];
                        while !head.ends_with(b"\r\n\r\n") {
                            if stopped.load(Ordering::Acquire) || head.len() > 16384 { return; }
                            match socket.read(&mut byte) {
                                Ok(0) => return,
                                Ok(_) => head.push(byte[0]),
                                Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => {},
                                Err(_) => return,
                            }
                        }
                        let request = String::from_utf8(head).unwrap();
                        let index = {
                            let mut requests = seen.lock().unwrap();
                            let index = requests.len();
                            requests.push(request.clone());
                            index
                        };
                        let Some(reply) = replies.get(index) else { return; };
                        if reply.required.is_some_and(|cookie| !request.lines().any(|line|
                            line.strip_prefix("Cookie: ").is_some_and(|value| value.split("; ").any(|part| part == cookie))))
                        {
                            let _ = socket.write_all(b"HTTP/1.1 403 Fixture\r\nContent-Length: 0\r\nConnection: keep-alive\r\n\r\n");
                            continue;
                        }
                        let header = format!("HTTP/1.1 {} Fixture\r\n{}Content-Length: {}\r\nConnection: keep-alive\r\n\r\n",
                            reply.status, reply.headers, reply.body.len());
                        if socket.write_all(header.as_bytes()).is_err() || socket.write_all(&reply.body).is_err() { return; }
                    }
                }));
            }
            for worker in connections { worker.join().unwrap(); }
        });
        Self { base: format!("http://session-cookie.invalid:{port}"), accepts, requests, stop, worker: Some(worker) }
    }
    pub(crate) fn heads(&self) -> Vec<String> { self.requests.lock().unwrap().clone() }
}
impl Drop for SessionServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        self.worker.take().unwrap().join().unwrap();
    }
}

pub(crate) fn curl() -> plx_base::testlock::Serial {
    let serial = plx_base::testlock::serial();
    assert!(plx_net::net::global_init() && available(), "host libcurl is required");
    serial
}
pub(crate) fn drain(source: &mut CurlSource) -> Vec<u8> {
    let deadline = Instant::now() + Duration::from_secs(3);
    let mut bytes = Vec::new();
    let mut chunk = [0; 128];
    loop {
        let n = source.read_until(&mut chunk, Some(deadline), &mut NoCheckpoint);
        assert!(n >= 0);
        if n == 0 { return bytes; }
        bytes.extend_from_slice(&chunk[..n as usize]);
    }
}
fn reopen(source: &mut CurlSource, url: &str) {
    source.reopen_until(url, Some(Instant::now() + Duration::from_secs(3)), &mut NoCheckpoint).unwrap();
    drain(source);
}
fn cookies(head: &str) -> &str {
    head.lines().find_map(|line| line.strip_prefix("Cookie: ")).unwrap_or("")
}

#[test]
fn private_cookie_reopen_keeps_active_playback_abort_target() {
    let _serial = curl();
    let active = CurlSource::reserve_open().unwrap();
    let abort = Arc::clone(active.abort.as_ref().unwrap());
    let server = SessionServer::new(vec![Reply::ok("Set-Cookie: sid=private-cookie-sentinel; Path=/\r\n", b"a"), Reply::ok("", b"b")]);
    let mut source = CurlSource::new_hls_private(&server.base).unwrap();
    reopen(&mut source, &format!("{}/root", server.base));
    reopen(&mut source, &format!("{}/child", server.base));
    assert!(lock_active().as_ref().is_some_and(|a| Arc::ptr_eq(a, &abort)));
    assert!(!abort.is_set());
    assert!(cookies(&server.heads()[1]).contains("sid=private-cookie-sentinel"));
    assert_eq!(server.accepts.load(Ordering::Acquire), 1);
}

#[test]
fn cookie_context_survives_manual_redirect_and_fresh_connection_cache() {
    let _serial = curl();
    let server = SessionServer::new(vec![
        Reply { status: 302, headers: "Set-Cookie: sid=redirect-cookie; Path=/\r\nLocation: /child\r\n".into(), body: vec![], required: None },
        Reply::ok("Set-Cookie: next=segment-cookie; Path=/\r\n", b"body"), Reply::ok("", b"again"),
    ]);
    let url = format!("{}/root", server.base);
    let mut source = CurlSource::open_hls_private_checked(&url, Some(Instant::now() + Duration::from_secs(3)), &mut NoCheckpoint).unwrap();
    assert_eq!(drain(&mut source), b"body");
    source.redial_until(&format!("{}/refresh", server.base), Some(Instant::now() + Duration::from_secs(3)), &mut NoCheckpoint).unwrap();
    assert_eq!(drain(&mut source), b"again");
    let heads = server.heads();
    assert!(cookies(&heads[1]).contains("sid=redirect-cookie"));
    assert!(cookies(&heads[2]).contains("sid=redirect-cookie"));
    assert!(cookies(&heads[2]).contains("next=segment-cookie"));
    assert_eq!(server.accepts.load(Ordering::Acquire), 2);
}

#[test]
fn cookies_keep_host_path_secure_and_expiry_scope_across_handles() {
    let _serial = curl();
    let server = SessionServer::new(vec![
        Reply::ok("Set-Cookie: good=yes; Path=/\r\nSet-Cookie: path=no; Path=/private\r\nSet-Cookie: tls=no; Secure; Path=/\r\nSet-Cookie: expired=no; Max-Age=0; Path=/\r\n", b"a"),
        Reply::ok("", b"b"), Reply::ok("", b"c"),
    ]);
    let mut source = CurlSource::new_hls_private(&server.base).unwrap();
    reopen(&mut source, &format!("{}/root", server.base));
    reopen(&mut source, &format!("{}/public", server.base));
    let other = server.base.replace("session-cookie.invalid", "other-cookie.invalid");
    reopen(&mut source, &format!("{other}/public"));
    let heads = server.heads();
    assert_eq!(cookies(&heads[1]), "good=yes");
    assert_eq!(cookies(&heads[2]), "");
}

#[test]
fn fresh_context_and_default_transport_do_not_inherit_cookies() {
    let _serial = curl();
    let server = SessionServer::new(vec![Reply::ok("Set-Cookie: sid=isolated; Path=/\r\n", b"a"), Reply::ok("", b"b"), Reply::ok("", b"c")]);
    let url = format!("{}/root", server.base);
    { let mut source = CurlSource::new_hls_private(&url).unwrap(); reopen(&mut source, &url); }
    { let mut source = CurlSource::new_hls_private(&url).unwrap(); reopen(&mut source, &url); }
    { let (mut source, _) = CurlSource::open_private_checked(&url, 0, Some(Instant::now() + Duration::from_secs(3)), &mut NoCheckpoint).unwrap(); drain(&mut source); }
    let heads = server.heads();
    assert!(heads.iter().all(|head| cookies(head).is_empty()));
}

#[test]
fn cancelled_expired_and_stopped_contexts_issue_no_extra_request() {
    let _serial = curl();
    let server = SessionServer::new(vec![Reply::ok("", b"a")]);
    let url = format!("{}/root", server.base);
    let mut source = CurlSource::new_hls_private(&url).unwrap();
    reopen(&mut source, &url);
    assert_eq!(source.reopen_until(&url, Some(Instant::now()), &mut NoCheckpoint), Err(OpenErr::Deadline));
    let mut stopped = checkpoint::TestCheckpoint::stopping_after(0, Duration::from_millis(1));
    assert_eq!(source.reopen_until(&url, None, &mut stopped), Err(OpenErr::Stopped));
    source.abort();
    assert_eq!(source.reopen_until(&url, None, &mut NoCheckpoint), Err(OpenErr::Aborted));
    assert_eq!(server.heads().len(), 1);
}

fn collect(rows: &[CString]) -> Result<Vec<CString>, ()> { collect_with_psl(rows, true) }
fn collect_with_psl(rows: &[CString], has_psl: bool) -> Result<Vec<CString>, ()> {
    let mut nodes: Vec<CookieNode> = rows.iter().map(|row| CookieNode { data: row.as_ptr(), next: std::ptr::null() }).collect();
    for i in 1..nodes.len() { nodes[i - 1].next = &nodes[i]; }
    unsafe { collect_cookie_rows(nodes.first().map_or(std::ptr::null(), |node| node as *const _), has_psl) }
}
#[test]
fn cookie_export_bounds_and_netscape_policy_fail_closed() {
    let valid = CString::new("#HttpOnly_fixture.invalid\tFALSE\t/\tFALSE\t0\tsid\tvalue").unwrap();
    assert_eq!(collect(&[valid.clone()]).unwrap(), vec![valid.clone()]);
    assert!(collect(&vec![valid.clone(); COOKIE_MAX_ROWS + 1]).is_err());
    let large = CString::new(format!("fixture.invalid\tFALSE\t/\tFALSE\t0\tsid\t{}", "x".repeat(COOKIE_MAX_ROW_BYTES))).unwrap();
    assert!(collect(&[large]).is_err());
    let medium = CString::new(format!("fixture.invalid\tFALSE\t/\tFALSE\t0\tsid\t{}", "x".repeat(4096))).unwrap();
    assert!(collect(&vec![medium; 16]).is_err());
    for row in ["ALL", "Set-Cookie: sid=unscoped", "\tFALSE\t/\tFALSE\t0\tsid\tx", "fixture.invalid\tFALSE\t/\tFALSE\t0\tsid\tx\r\n"] {
        assert!(collect(&[CString::new(row).unwrap()]).is_err());
    }
}

#[test]
fn failed_cookie_context_cannot_reopen_or_fall_back() {
    let _serial = curl();
    let server = SessionServer::new(vec![]);
    let mut source = CurlSource::new_hls_private(&server.base).unwrap();
    source.cookies.as_mut().unwrap().failed = true;
    assert_eq!(source.reopen_until(&server.base, None, &mut NoCheckpoint), Err(OpenErr::Local));
    assert_eq!(source.redial_until(&server.base, None, &mut NoCheckpoint), Err(OpenErr::Local));
    assert!(server.heads().is_empty());
}

#[test]
fn cookie_header_budget_survives_informational_blocks_and_trailers() {
    let mut xfer = Xfer::new();
    xfer.cookie_headers = Some(CookieHeaderBudget::default());
    let mut header = |line: &[u8]| {
        header_cb(line.as_ptr() as *mut std::ffi::c_char, 1, line.len(), (&mut xfer as *mut Xfer).cast())
    };
    assert_eq!(header(b"HTTP/1.1 103 Early Hints\r\n"), 26);
    for _ in 0..COOKIE_MAX_ROWS / 2 {
        let line = b"Set-Cookie: sid=value; Path=/\r\n";
        assert_eq!(header(line), line.len());
    }
    header(b"\r\n");
    header(b"HTTP/1.1 200 OK\r\n");
    for _ in COOKIE_MAX_ROWS / 2..COOKIE_MAX_ROWS {
        let line = b"Set-Cookie: sid=value; Path=/\r\n";
        assert_eq!(header(line), line.len());
    }
    header(b"\r\n");
    assert_eq!(header(b"Set-Cookie: trailer=overflow\r\n"), 0);
    assert_eq!(header(b"Content-Length: 0\r\n"), 0, "refusal is latched for the whole attempt");
    assert!(xfer.cookie_headers.as_ref().unwrap().failed);
    let mut long = CookieHeaderBudget::default();
    assert!(!long.admit(format!("Set-Cookie: sid={}", "x".repeat(COOKIE_MAX_ROW_BYTES)).as_bytes()));
    let mut total = CookieHeaderBudget::default();
    let line = format!("Set-Cookie: sid={}", "x".repeat(4096));
    for _ in 0..15 { assert!(total.admit(line.as_bytes())); }
    assert!(!total.admit(line.as_bytes()));
}

#[test]
fn oversized_cookie_response_is_a_latched_nontransient_refusal() {
    let _serial = curl();
    let headers = "Set-Cookie: sid=value; Path=/\r\n".repeat(COOKIE_MAX_ROWS + 1);
    let server = SessionServer::new(vec![Reply::ok(&headers, b"")]);
    let mut source = CurlSource::new_hls_private(&server.base).unwrap();
    assert_eq!(source.reopen_until(&server.base, Some(Instant::now() + Duration::from_secs(3)), &mut NoCheckpoint), Err(OpenErr::Local));
    assert!(source.cookies.as_ref().unwrap().failed);
    assert_eq!(source.redial_until(&server.base, None, &mut NoCheckpoint), Err(OpenErr::Local));
    assert_eq!(server.heads().len(), 1);
}

#[test]
fn no_or_unknown_psl_keeps_host_only_and_drops_domain_cookie_scope() {
    let host_only = CString::new("#HttpOnly_fixture.invalid\tFALSE\t/\tFALSE\t0\tsid\thost").unwrap();
    let domain = CString::new(".fixture.invalid\tTRUE\t/\tFALSE\t0\tsid\tdomain").unwrap();
    let supercookie = CString::new(".co.uk\tTRUE\t/\tFALSE\t0\tsid\tsupercookie").unwrap();
    let rows = vec![host_only.clone(), domain.clone(), supercookie];
    assert_eq!(collect_with_psl(&rows, false).unwrap(), vec![host_only.clone()]);
    // With PSL, input remains exclusively libcurl's accepted export, not
    // arbitrary caller-supplied rows. Its verified domain cookie is retained.
    assert_eq!(collect_with_psl(&[host_only.clone(), domain.clone()], true).unwrap(), vec![host_only, domain.clone()]);
    assert!(collect_with_psl(&vec![domain; COOKIE_MAX_ROWS + 1], false).is_err(),
        "dropped domain rows still spend the count/byte limits");
}

#[derive(Clone, Copy, Debug)]
enum RetiredResponseRefusal {
    ExpiredReopen,
    ExpiredSeek,
    StoppedReopen,
    StoppedSeek,
    CookieReopen,
    CookieSeek,
}

fn park_old_response(source: &mut CurlSource, exhausted: bool) {
    if exhausted {
        assert_eq!(drain(source), b"retired-response");
        assert!(source.done && source.xfer.pending() == 0);
    } else {
        let deadline = Instant::now() + Duration::from_secs(3);
        let mut first = [0];
        assert_eq!(source.read_until(&mut first, Some(deadline), &mut NoCheckpoint), 1);
        assert_eq!(&first, b"r");
        // Keep actual response bytes buffered across retirement. Receiving the
        // rest through the real multi avoids assuming a TCP packet boundary.
        while source.xfer.pending() == 0 {
            assert!(Instant::now() < deadline, "fixture body must become available");
            source.perform().unwrap();
            if source.xfer.pending() == 0 {
                assert!(!source.done, "old response unexpectedly had no remaining bytes");
                let _ = multi_wait(source.multi, &source.abort, 20);
            }
        }
    }
}

#[test]
fn refused_replacement_cannot_read_retired_buffer_or_eof_under_renewed_allowance() {
    let _serial = curl();
    for hls in [false, true] {
        for exhausted in [false, true] {
            for refusal in [RetiredResponseRefusal::ExpiredReopen, RetiredResponseRefusal::ExpiredSeek,
                RetiredResponseRefusal::StoppedReopen, RetiredResponseRefusal::StoppedSeek,
                RetiredResponseRefusal::CookieReopen, RetiredResponseRefusal::CookieSeek]
            {
                // Immediate checkpoint and cookie-policy guards are opted-in
                // HLS behavior. Both ordinary and HLS paths share expiry guards.
                if !hls && !matches!(refusal, RetiredResponseRefusal::ExpiredReopen | RetiredResponseRefusal::ExpiredSeek) {
                    continue;
                }
                let server = SessionServer::new(vec![
                    Reply::ok("", b"retired-response"), Reply::ok("", b"fresh-response"),
                ]);
                let url = format!("{}/body", server.base);
                let deadline = Instant::now() + Duration::from_secs(3);
                let mut source = if hls {
                    CurlSource::open_hls_private_checked(&url, Some(deadline), &mut NoCheckpoint).unwrap()
                } else {
                    CurlSource::open_private_checked(&url, 0, Some(deadline), &mut NoCheckpoint).unwrap().0
                };
                park_old_response(&mut source, exhausted);
                let deadline = Instant::now() + Duration::from_secs(3);
                let mut stopped = checkpoint::TestCheckpoint::stopping_after(0, Duration::from_millis(1));
                match refusal {
                    RetiredResponseRefusal::ExpiredReopen => assert_eq!(
                        source.reopen_until(&url, Some(Instant::now()), &mut NoCheckpoint), Err(OpenErr::Deadline)),
                    RetiredResponseRefusal::ExpiredSeek => assert!(
                        !source.seek_until(0, Some(Instant::now()), &mut NoCheckpoint)),
                    RetiredResponseRefusal::StoppedReopen => assert_eq!(
                        source.reopen_until(&url, Some(deadline), &mut stopped), Err(OpenErr::Stopped)),
                    RetiredResponseRefusal::StoppedSeek => assert!(
                        !source.seek_until(0, Some(deadline), &mut stopped)),
                    RetiredResponseRefusal::CookieReopen => {
                        source.cookies.as_mut().unwrap().failed = true;
                        assert_eq!(source.reopen_until(&url, Some(deadline), &mut NoCheckpoint), Err(OpenErr::Local));
                    }
                    RetiredResponseRefusal::CookieSeek => {
                        source.cookies.as_mut().unwrap().failed = true;
                        assert!(!source.seek_until(0, Some(deadline), &mut NoCheckpoint));
                    }
                }
                let mut bytes = [0; 64];
                assert!(source.read_until(&mut bytes, Some(Instant::now() + Duration::from_secs(3)), &mut NoCheckpoint) < 0,
                    "retired response became readable: hls={hls} exhausted={exhausted} refusal={refusal:?}");
                assert_eq!(server.heads().len(), 1, "a refused replacement and renewed read must not issue a request");
                assert!(source.body_failure().is_none(), "a pre-request refusal must not invent a transport body failure");
                if matches!(refusal, RetiredResponseRefusal::CookieReopen | RetiredResponseRefusal::CookieSeek) {
                    assert_eq!(source.reopen_until(&url, Some(Instant::now() + Duration::from_secs(3)), &mut NoCheckpoint), Err(OpenErr::Local));
                    assert_eq!(server.heads().len(), 1, "cookie policy failure remains closed");
                    // This terminal context cannot heal. Only a separately
                    // owned fresh source may establish another response.
                    drop(source);
                    source = CurlSource::open_hls_private_checked(&url,
                        Some(Instant::now() + Duration::from_secs(3)), &mut NoCheckpoint).unwrap();
                } else {
                    source.reopen_until(&url, Some(Instant::now() + Duration::from_secs(3)), &mut NoCheckpoint).unwrap();
                }
                assert_eq!(drain(&mut source), b"fresh-response");
                assert_eq!(server.heads().len(), 2, "only an explicit successful open restores readability");
            }
        }
    }
}
