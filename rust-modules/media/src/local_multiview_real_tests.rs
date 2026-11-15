//! Explicit real bundled-decoder qualification. Run alone with --ignored --exact.

use super::*;
use crate::ff::{ExternalSource, HeldLocalFile};
use std::{
    collections::HashSet,
    fs::File,
    path::PathBuf,
    time::{Duration, Instant},
};

/// This listener is created only by the explicitly requested ignored fixture test.
/// No arbitrary paths, upstream requests, persistent service or external network.
struct FixtureServer {
    address: std::net::SocketAddr,
    stop: Arc<AtomicBool>,
    done: Arc<AtomicBool>,
    worker: Option<std::thread::JoinHandle<()>>,
    redirects: Arc<std::sync::atomic::AtomicUsize>,
}
impl FixtureServer {
    fn open(root: PathBuf) -> Self {
        let listener = std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).unwrap();
        listener.set_nonblocking(true).unwrap();
        let address = listener.local_addr().unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let done = Arc::new(AtomicBool::new(false));
        let redirects = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let flag = Arc::clone(&stop);
        let finished = Arc::clone(&done);
        let redirected = Arc::clone(&redirects);
        let worker = std::thread::Builder::new()
            .name("fixture-http".into())
            .spawn(move || {
                struct Done(Arc<AtomicBool>);
                impl Drop for Done {
                    fn drop(&mut self) {
                        self.0.store(true, Ordering::Release);
                    }
                }
                let _done = Done(finished);
                let expiry = Instant::now() + Duration::from_secs(45);
                while !flag.load(Ordering::Acquire) && Instant::now() < expiry {
                    match listener.accept() {
                        Ok((socket, _)) => {
                            let _ = Self::serve(socket, &root, &redirected);
                        }
                        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                            std::thread::sleep(Duration::from_millis(2))
                        }
                        Err(_) => break,
                    }
                }
            })
            .unwrap();
        Self {
            address,
            stop,
            done,
            worker: Some(worker),
            redirects,
        }
    }
    fn url(&self, path: &str) -> String {
        format!("http://{}{path}", self.address)
    }
    fn response(
        socket: &mut std::net::TcpStream,
        status: &str,
        headers: &str,
        body: &[u8],
    ) -> std::io::Result<()> {
        use std::io::Write;
        write!(
            socket,
            "HTTP/1.1 {status}\r\nConnection: close\r\nContent-Length: {}\r\n{headers}\r\n",
            body.len()
        )?;
        socket.write_all(body)
    }
    fn serve(
        mut socket: std::net::TcpStream,
        root: &std::path::Path,
        redirects: &std::sync::atomic::AtomicUsize,
    ) -> std::io::Result<()> {
        use std::io::Read;
        socket.set_read_timeout(Some(Duration::from_millis(500)))?;
        socket.set_write_timeout(Some(Duration::from_millis(500)))?;
        let mut header = Vec::new();
        let mut scratch = [0u8; 1024];
        while !header.windows(4).any(|w| w == b"\r\n\r\n") {
            if header.len() >= 8192 {
                return Self::response(&mut socket, "431 Request Header Fields Too Large", "", b"");
            }
            let n = socket.read(&mut scratch)?;
            if n == 0 {
                return Ok(());
            }
            header.extend_from_slice(&scratch[..n]);
        }
        let text = std::str::from_utf8(&header).map_err(|_| std::io::ErrorKind::InvalidData)?;
        let mut request = text.lines().next().unwrap_or("").split_whitespace();
        if request.next() != Some("GET") {
            return Self::response(&mut socket, "405 Method Not Allowed", "", b"");
        }
        let path = request.next().unwrap_or("");
        let mut file = None;
        for index in [1, 3, 5] {
            if path == format!("/files/{index}.mp4") {
                file = Some(root.join(format!("slot{index}.mp4")));
            }
        }
        for index in [2, 6, 7] {
            if path == format!("/hls/{index}/master.m3u8") {
                redirects.fetch_add(1, Ordering::AcqRel);
                return Self::response(
                    &mut socket,
                    "302 Found",
                    &format!("Location: /redirected/{index}/master.m3u8\r\n"),
                    b"",
                );
            }
            if path == format!("/redirected/{index}/master.m3u8") {
                return Self::response(
                    &mut socket,
                    "200 OK",
                    "Content-Type: application/vnd.apple.mpegurl\r\n",
                    b"#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=100000\nmedia.m3u8\n",
                );
            }
            if path == format!("/redirected/{index}/media.m3u8") {
                redirects.fetch_add(1, Ordering::AcqRel);
                return Self::response(
                    &mut socket,
                    "302 Found",
                    &format!("Location: /edge/{index}/media.m3u8\r\n"),
                    b"",
                );
            }
            if path == format!("/edge/{index}/media.m3u8") {
                file = Some(root.join(format!("slot{index}-hls/media.m3u8")));
            }
            if let Some(name) = path.strip_prefix(&format!("/edge/{index}/")) {
                if name.len() == 13
                    && name.starts_with("segment")
                    && name.ends_with(".ts")
                    && name.as_bytes()[7..10].iter().all(u8::is_ascii_digit)
                {
                    file = Some(root.join(format!("slot{index}-hls")).join(name));
                }
            }
        }
        let Some(file) = file else {
            return Self::response(&mut socket, "404 Not Found", "", b"");
        };
        let mut input = File::open(file)?;
        let length = input.metadata()?.len();
        if length > 16 * 1024 * 1024 {
            return Self::response(&mut socket, "413 Content Too Large", "", b"");
        }
        let range = text.lines().find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("range").then_some(value.trim())
        });
        let (start, end, partial) = if let Some(range) = range {
            let Some((start, end)) = range.strip_prefix("bytes=").and_then(|r| r.split_once('-'))
            else {
                return Self::response(&mut socket, "416 Range Not Satisfiable", "", b"");
            };
            let Some(start) = start.parse::<u64>().ok().filter(|n| *n < length) else {
                return Self::response(&mut socket, "416 Range Not Satisfiable", "", b"");
            };
            let end = if end.is_empty() {
                length - 1
            } else {
                match end.parse::<u64>() {
                    Ok(end) if end >= start => end.min(length - 1),
                    _ => return Self::response(&mut socket, "416 Range Not Satisfiable", "", b""),
                }
            };
            (start, end, true)
        } else {
            (0, length.saturating_sub(1), false)
        };
        use std::io::{Seek, SeekFrom};
        input.seek(SeekFrom::Start(start))?;
        let mut body = Vec::new();
        input
            .take(if length == 0 { 0 } else { end - start + 1 })
            .read_to_end(&mut body)?;
        let headers = if partial {
            format!("Accept-Ranges: bytes\r\nContent-Range: bytes {start}-{end}/{length}\r\n")
        } else {
            "Accept-Ranges: bytes\r\n".into()
        };
        Self::response(
            &mut socket,
            if partial {
                "206 Partial Content"
            } else {
                "200 OK"
            },
            &headers,
            &body,
        )
    }
    fn stop(&mut self) -> bool {
        self.stop.store(true, Ordering::Release);
        let end = Instant::now() + Duration::from_secs(2);
        while (!self.done.load(Ordering::Acquire)
            || self.worker.as_ref().is_some_and(|w| !w.is_finished()))
            && Instant::now() < end
        {
            std::thread::sleep(Duration::from_millis(5));
        }
        if self.worker.as_ref().is_some_and(|w| w.is_finished()) {
            if let Some(worker) = self.worker.take() {
                return worker.join().is_ok();
            }
        }
        self.worker.is_none()
    }
}
impl Drop for FixtureServer {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}
struct Readers(Vec<Session>);
impl Drop for Readers {
    fn drop(&mut self) {
        for s in &mut self.0 {
            s.cancel();
        }
        let end = Instant::now() + Duration::from_secs(4);
        while self.0.iter().any(|s| !s.is_retired()) && Instant::now() < end {
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}
#[derive(Default)]
struct Seen {
    frames: usize,
    last: Option<i64>,
    hash: Option<u64>,
    audio: usize,
    failure: bool,
}
fn pump(readers: &mut Readers, seen: &mut [Seen], revoked: Option<usize>) {
    for (index, session) in readers.0.iter_mut().enumerate() {
        for _ in 0..24 {
            let Some(event) = session.poll() else {
                break;
            };
            match event {
                Event::Ready => {}
                Event::Video(frame) => {
                    assert_eq!(
                        (
                            frame.width,
                            frame.height,
                            frame.chroma_width,
                            frame.chroma_height
                        ),
                        (96, 64, 48, 32)
                    );
                    assert_eq!(
                        (frame.y.len(), frame.u.len(), frame.v.len()),
                        (96 * 64, 48 * 32, 48 * 32)
                    );
                    if let Some(last) = seen[index].last {
                        assert!(frame.pts_us > last, "slot {index} PTS did not advance");
                    }
                    seen[index].last = Some(frame.pts_us);
                    seen[index].frames += 1;
                    seen[index].hash.get_or_insert_with(|| {
                        frame
                            .y
                            .iter()
                            .chain(&frame.u)
                            .chain(&frame.v)
                            .fold(1469598103934665603u64, |h, b| {
                                (h ^ u64::from(*b)).wrapping_mul(1099511628211)
                            })
                    });
                }
                Event::Audio(chunk) => {
                    assert!(!chunk.samples.is_empty());
                    assert_eq!(chunk.samples.len() % 2, 0);
                    assert!(chunk.samples.len() <= 2048);
                    assert!(chunk.samples.iter().any(|v| *v != 0));
                    seen[index].audio += 1;
                }
                Event::Failure(error) => {
                    assert_eq!(
                        Some(index),
                        revoked,
                        "unexpected slot {index} failure: {error}"
                    );
                    seen[index].failure = true;
                }
                Event::Ended => {
                    panic!("fixture slot {index} ended before qualification finished")
                }
            }
        }
        let mailbox = session.control.mailbox.lock().unwrap();
        assert!(mailbox.audio.len() <= 12);
        if let Some((_, frame)) = &mailbox.video {
            assert!(
                frame.y.len() + frame.u.len() + frame.v.len() <= Limits::default().max_frame_bytes
            );
        }
    }
}
fn until(
    readers: &mut Readers,
    seen: &mut [Seen],
    duration: Duration,
    revoked: Option<usize>,
    condition: impl Fn(&[Seen]) -> bool,
) {
    let end = Instant::now() + duration;
    while !condition(seen) && Instant::now() < end {
        pump(readers, seen, revoked);
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(condition(seen), "actual decode condition timed out");
}
#[test]
#[ignore = "Explicit real bundled-decoder qualification; requires generated fixtures and PLXNATIVE_APP_DIR"]
fn nine_independent_real_decoders_video_audio_seek_withdrawal_and_retirement() {
    let _serial = plx_base::testlock::serial();
    let root = PathBuf::from(
        std::env::var_os("PLXNATIVE_LOCAL_VIDEO_FIXTURES").expect("fixture directory required"),
    );
    assert!(
        std::env::var_os("PLXNATIVE_APP_DIR").is_some(),
        "new decoder prefix required"
    );
    assert!(plx_net::net::global_init());
    crate::ff::boot();
    assert!(available(), "bundled H264 decoder unavailable");
    let mut server = FixtureServer::open(root.clone());
    let mut readers = Readers(Vec::new());
    let mut latches = Vec::new();
    let off = plx_base::task::OffFrame::for_test();
    for index in 0..9 {
        let latch = Arc::new(AtomicBool::new(false));
        let source = match index {
            0 | 4 | 8 => {
                let path = root.join(format!("slot{index}.mp4"));
                ExternalSource::HeldLocal(
                    HeldLocalFile::from_authorized(
                        &off,
                        File::open(&path).expect("fixture missing"),
                        path,
                        Arc::clone(&latch),
                    )
                    .unwrap(),
                )
            }
            1 | 3 | 5 => ExternalSource::Http(server.url(&format!("/files/{index}.mp4"))),
            2 | 6 | 7 => ExternalSource::Hls(server.url(&format!("/hls/{index}/master.m3u8"))),
            _ => unreachable!(),
        };
        readers
            .0
            .push(Session::open(source, Limits::default()).unwrap());
        latches.push(latch);
    }
    let path = root.join("slot0.mp4");
    let held = HeldLocalFile::from_authorized(
        &off,
        File::open(&path).unwrap(),
        path,
        Arc::new(AtomicBool::new(false)),
    )
    .unwrap();
    assert!(
        matches!(
            Session::open(ExternalSource::HeldLocal(held), Limits::default()),
            Err(Error::Budget)
        ),
        "live worker bound not enforced"
    );
    readers.0[0].set_audio(true);
    let mut seen: Vec<Seen> = (0..9).map(|_| Seen::default()).collect();
    until(&mut readers, &mut seen, Duration::from_secs(6), None, |s| {
        s.iter().all(|s| s.frames >= 3) && s[0].audio > 0
    });
    assert_eq!(
        seen.iter()
            .map(|s| s.hash.unwrap())
            .collect::<HashSet<_>>()
            .len(),
        9,
        "pictures are not independent"
    );
    assert!(
        server.redirects.load(Ordering::Acquire) >= 6,
        "HLS readers did not traverse real master+media redirects"
    );
    eprintln!(
        "mixed real decode: HeldLocal=3 HTTP=3 HLS=3, first PTS={:?}",
        seen.iter().map(|s| s.last).collect::<Vec<_>>()
    );
    let audio_epoch = readers.0[0]
        .control
        .audio_generation
        .load(Ordering::Acquire);
    readers.0[0].set_audio(true);
    assert_eq!(
        readers.0[0]
            .control
            .audio_generation
            .load(Ordering::Acquire),
        audio_epoch
    );
    readers.0[0].set_audio(false);
    readers.0[1].set_audio(true);
    let zero_audio = seen[0].audio;
    until(&mut readers, &mut seen, Duration::from_secs(2), None, |s| {
        s[1].audio > 0
    });
    assert_eq!(
        seen[0].audio, zero_audio,
        "old owner PCM escaped after switch"
    );
    readers.0[2].set_paused(true);
    let paused = seen[2].frames;
    let other = seen[5].frames;
    until(&mut readers, &mut seen, Duration::from_secs(2), None, |s| {
        s[5].frames >= other + 3
    });
    assert_eq!(seen[2].frames, paused, "paused source published frames");
    readers.0[2].set_paused(false);
    until(&mut readers, &mut seen, Duration::from_secs(2), None, |s| {
        s[2].frames > paused
    });
    readers.0[3].seek(2_000_000).unwrap();
    seen[3].last = None;
    let before_seek = seen[3].frames;
    until(&mut readers, &mut seen, Duration::from_secs(2), None, |s| {
        s[3].frames > before_seek && s[3].last.is_some_and(|p| p >= 1_900_000)
    });
    latches[4].store(true, Ordering::Release);
    until(
        &mut readers,
        &mut seen,
        Duration::from_secs(2),
        Some(4),
        |s| s[4].failure,
    );
    for s in &mut readers.0 {
        s.cancel();
    }
    let end = Instant::now() + Duration::from_secs(4);
    while readers.0.iter().any(|s| !s.is_retired()) && Instant::now() < end {
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(
        readers.0.iter().all(Session::is_retired),
        "cancel was incorrectly reported as retirement"
    );
    let path = root.join("slot0.mp4");
    let held = HeldLocalFile::from_authorized(
        &off,
        File::open(&path).unwrap(),
        path,
        Arc::new(AtomicBool::new(false)),
    )
    .unwrap();
    let mut reopened = Session::open(ExternalSource::HeldLocal(held), Limits::default())
        .expect("retired permits leaked");
    reopened.cancel();
    let end = Instant::now() + Duration::from_secs(3);
    while !reopened.is_retired() && Instant::now() < end {
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(reopened.is_retired());
    assert!(server.stop(), "owned fixture HTTP worker did not retire");
}
