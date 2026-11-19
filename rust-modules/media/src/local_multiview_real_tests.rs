//! Explicit real bundled-decoder qualification. Run alone with --ignored --exact.

use super::*;
use crate::ff::{ExternalSource, HeldLocalFile};
use std::{
    collections::HashSet,
    fs::File,
    path::PathBuf,
    sync::atomic::AtomicUsize,
    time::{Duration, Instant},
};

#[derive(Default)]
struct LiveGridFixture {
    last_served: [AtomicUsize; 4],
    segment_timeout_reads: AtomicUsize,
    segment_503_reads: AtomicUsize,
}

/// Loopback listener for explicit fixture tests. All content comes from generated files.
struct FixtureServer {
    address: std::net::SocketAddr,
    stop: Arc<AtomicBool>,
    done: Arc<AtomicBool>,
    worker: Option<std::thread::JoinHandle<()>>,
    redirects: Arc<std::sync::atomic::AtomicUsize>,
    live_grid: Arc<LiveGridFixture>,
}
fn is_segment(name: &str) -> bool {
    name.len() == 13
        && name.starts_with("segment")
        && name.ends_with(".ts")
        && name.as_bytes()[7..10].iter().all(u8::is_ascii_digit)
}

impl FixtureServer {
    fn open(root: PathBuf) -> Self {
        let listener = std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).unwrap();
        listener.set_nonblocking(true).unwrap();
        let address = listener.local_addr().unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let done = Arc::new(AtomicBool::new(false));
        let redirects = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let live_grid = Arc::new(LiveGridFixture::default());
        let live_requests = Arc::clone(&live_grid);
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
                            let _ =
                                Self::serve(socket, &root, &redirected, &live_requests);
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
            live_grid,
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
        live_grid: &LiveGridFixture,
    ) -> std::io::Result<()> {
        use std::io::Read;
        // Darwin may inherit O_NONBLOCK from the listener. Blocking I/O on a
        // single accepted connection is bounded by the two socket timeouts.
        socket.set_nonblocking(false)?;
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
        // Advance each live playlist as its reader consumes segments. A slow test
        // runner cannot skip either injected fault by joining at a later live edge.
        if path.starts_with("/live/") {
            for (slot, index) in [2, 6, 7, 8].into_iter().enumerate() {
                if path == format!("/live/{index}/media.m3u8") {
                    let last = (1 + live_grid.last_served[slot].load(Ordering::Acquire)).min(11);
                    let first = last.saturating_sub(3);
                    let mut playlist = format!(
                        "#EXTM3U\n#EXT-X-TARGETDURATION:2\n#EXT-X-MEDIA-SEQUENCE:{first}\n"
                    );
                    for seq in first..=last {
                        playlist.push_str(&format!("#EXTINF:2,\nsegment{seq:03}.ts\n"));
                    }
                    return Self::response(
                        &mut socket,
                        "200 OK",
                        "Content-Type: application/vnd.apple.mpegurl\r\n",
                        playlist.as_bytes(),
                    );
                }
                if path == format!("/live/{index}/segment002.ts") {
                    if index == 2
                        && live_grid
                            .segment_timeout_reads
                            .fetch_add(1, Ordering::AcqRel)
                            == 0
                    {
                        return Self::response(&mut socket, "200 OK", "", b"ESOCKETTIMEDOUT");
                    }
                    if index == 6 && live_grid.segment_503_reads.fetch_add(1, Ordering::AcqRel) == 0
                    {
                        return Self::response(&mut socket, "503 Service Unavailable", "", b"");
                    }
                }
                if let Some(name) = path.strip_prefix(&format!("/live/{index}/")) {
                    if is_segment(name) {
                        let sequence = name[7..10].parse::<usize>().unwrap();
                        live_grid.last_served[slot].fetch_max(sequence, Ordering::AcqRel);
                        file = Some(root.join(format!("slot{index}-hls")).join(name));
                    }
                }
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
                if is_segment(name) {
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
#[test]
fn fixture_waits_for_the_rest_of_a_split_request() {
    use std::io::{Read, Write};
    use std::net::{Ipv4Addr, TcpListener, TcpStream};

    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let mut client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
    client.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
    let (socket, _) = listener.accept().unwrap();
    // Exercise Darwin's inherited mode on every host.
    socket.set_nonblocking(true).unwrap();
    client.write_all(b"GET /unknown HTTP/1.1\r\n").unwrap();
    let writer = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(40));
        client.write_all(b"\r\n").unwrap();
        let mut response = String::new();
        client.read_to_string(&mut response).unwrap();
        response
    });
    let served = FixtureServer::serve(
        socket,
        std::path::Path::new("."),
        &AtomicUsize::default(),
        &LiveGridFixture::default(),
    );
    let response = writer.join().unwrap();
    served.unwrap();
    assert!(response.starts_with("HTTP/1.1 404 Not Found\r\n"));
}

impl Drop for FixtureServer {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}
struct Readers(Vec<Session>);
impl Readers {
    fn cancel_and_wait(&mut self) -> bool {
        for s in &mut self.0 {
            s.cancel();
        }
        let end = Instant::now() + Duration::from_secs(4);
        while self.0.iter().any(|s| !s.is_retired()) && Instant::now() < end {
            std::thread::sleep(Duration::from_millis(5));
        }
        self.0.iter().all(Session::is_retired)
    }
}
impl Drop for Readers {
    fn drop(&mut self) {
        self.cancel_and_wait();
    }
}
fn open_mixed_readers(
    root: &std::path::Path,
    server: &FixtureServer,
) -> (Readers, Vec<Arc<AtomicBool>>) {
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
    (readers, latches)
}
#[derive(Default)]
struct Seen {
    frames: usize,
    last: Option<i64>,
    hash: Option<u64>,
    audio: usize,
    audio_floor: Option<i64>,
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
                    if let Some(target) = seen[index].audio_floor {
                        assert!(
                            seen[index].last.is_some_and(|pts| pts >= target),
                            "slot{index} PCM escaped before the video landing"
                        );
                        let tolerance =
                            ((chunk.samples.len() / 2 * 1_000_000).div_ceil(48_000)) as i64;
                        assert!(
                            chunk.pts_us >= target - tolerance,
                            "slot{index} preroll PCM escaped: {} before {target}",
                            chunk.pts_us
                        );
                    }
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
fn paused_seek_lands_without_resuming(readers: &mut Readers, seen: &mut [Seen], index: usize) {
    readers.0[index].set_paused(true);
    for position in [4_300_000, 2_300_000] {
        let before = seen[index].frames;
        let audio = seen[index].audio;
        seen[index].last = None;
        if position == 2_300_000 {
            readers.0[index]
                .seek(10_000_000)
                .expect("first queued seek rejected");
        }
        // Hold a genuinely queued accepted seek while changing pause epochs.
        // The worker cannot consume it until the guard is released; no timing
        // sleep or decoder-only test hook creates the race precondition.
        let control = Arc::clone(&readers.0[index].control);
        let mut captured = false;
        for _ in 0..64 {
            readers.0[index]
                .seek(position)
                .expect("seekable paused source rejected seeking");
            let request = control.seek.lock().unwrap();
            if request.is_some() {
                readers.0[index].set_paused(false);
                readers.0[index].set_paused(true);
                assert!(control.paused.load(Ordering::Acquire));
                captured = true;
                break;
            }
        }
        assert!(
            captured,
            "could not retain a pending seek for the epoch regression"
        );
        until(readers, seen, Duration::from_secs(3), None, |s| {
            s[index].frames > before
                && s[index]
                    .last
                    .is_some_and(|p| p >= position && p < position + 500_000)
        });
        let landed = seen[index].frames;
        let deadline = Instant::now() + Duration::from_millis(200);
        while Instant::now() < deadline {
            pump(readers, seen, None);
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(
            landed,
            before + 1,
            "paused seek published more than its landing frame"
        );
        assert_eq!(seen[index].frames, landed, "seek resumed paused playback");
        assert_eq!(
            seen[index].audio, audio,
            "paused seek leaked old or new PCM"
        );
        assert!(readers.0[index].control.paused.load(Ordering::Acquire));
    }
    let paused = seen[index].frames;
    readers.0[index].set_paused(false);
    until(readers, seen, Duration::from_secs(3), None, |s| {
        s[index].frames > paused
    });
}

/// Decode four independent fixture tones through the same held/HTTP/HLS
/// readers as the renderer. Check all frequencies after each focus change;
/// a mixer which forwards only one input cannot pass this measurement.
fn check_four_window_audio(readers: &mut Readers) {
    const CAPTURE: usize = 12 * 1024;
    let mut pcm: [Vec<i16>; 4] = std::array::from_fn(|_| Vec::new());
    for reader in &mut readers.0[..4] {
        reader.set_audio(true);
    }
    let until = Instant::now() + Duration::from_secs(6);
    while pcm.iter().any(|samples| samples.len() < CAPTURE * 2) {
        assert!(Instant::now() < until, "four decoders did not publish audio");
        for (lane, reader) in pcm.iter_mut().zip(&mut readers.0) {
            while let Some(event) = reader.poll() {
                match event {
                    Event::Audio(chunk) => {
                        let remaining = (CAPTURE * 2).saturating_sub(lane.len());
                        lane.extend(chunk.samples.into_iter().take(remaining));
                    }
                    Event::Failure(error) => panic!("four-window qualification: {error}"),
                    Event::Ended => panic!("fixture ended before producing enough audio"),
                    _ => {},
                }
            }
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    for reader in &mut readers.0[1..4] {
        reader.set_audio(false);
    }
    let mut mixer = crate::local_pcm_mix::Mixer::default();
    for focus in [0, 2] {
        let mut output = Vec::new();
        let start = if focus == 0 { 0 } else { CAPTURE / 2 };
        for offset in (start..start + CAPTURE / 2).step_by(1024) {
            for (slot, lane) in pcm.iter().enumerate() {
                mixer.push(slot, PcmChunk {
                    samples: lane[offset * 2..(offset + 1024) * 2].to_vec(),
                    pts_us: (offset as i64) * 1_000_000 / 48_000,
                });
            }
            while let Some(chunk) = mixer.pull(focus, 20, 0) {
                output.extend(chunk.samples.chunks_exact(2).map(|pair| f64::from(pair[0])));
            }
        }
        let steady = &output[1024..]; // omit the focus ramp and AAC entry delay
        let tones: [f64; 4] = std::array::from_fn(|slot| {
            let omega = std::f64::consts::TAU * (300 + slot * 37) as f64 / 48_000.0;
            let (mut re, mut im) = (0.0, 0.0);
            for (i, &sample) in steady.iter().enumerate() {
                let t = i as f64;
                let window = 0.5 - 0.5 * (std::f64::consts::TAU * t / steady.len() as f64).cos();
                re += sample * window * (omega * t).cos();
                im += sample * window * (omega * t).sin();
            }
            re.hypot(im)
        });
        for (slot, amplitude) in tones.iter().enumerate() {
            if slot != focus {
                let ratio = amplitude / tones[focus];
                assert!((0.13..0.29).contains(&ratio), "focus {focus}, slot {slot}, ratio {ratio}");
            }
        }
    }
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
    let (mut readers, latches) = open_mixed_readers(&root, &server);
    let off = plx_base::task::OffFrame::for_test();
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
    check_four_window_audio(&mut readers);
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
    for (index, reader) in readers.0.iter().enumerate() {
        assert_eq!(
            reader.is_seekable(),
            !matches!(index, 2 | 6 | 7),
            "seek capability did not match the opened held/HTTP/HLS reader"
        );
    }
    assert_eq!(readers.0[2].seek(2_000_000), Err(Error::Unsupported));
    paused_seek_lands_without_resuming(&mut readers, &mut seen, 0);
    paused_seek_lands_without_resuming(&mut readers, &mut seen, 3);
    eprintln!(
        "paused seek: held+HTTP each landed forward/backward once without resuming; HLS rejected"
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
    // The active unpaused HTTP audio owner must not expose keyframe preroll
    // while exact video seeking skips to a non-keyframe destination.
    let target = 4_300_000;
    seen[1].last = None;
    seen[1].audio_floor = Some(target);
    let before_seek = seen[1].frames;
    let before_audio = seen[1].audio;
    readers.0[1].seek(target).unwrap();
    until(&mut readers, &mut seen, Duration::from_secs(3), None, |s| {
        s[1].frames > before_seek
            && s[1].last.is_some_and(|p| p >= target)
            && s[1].audio > before_audio
    });
    eprintln!(
        "unpaused non-keyframe seek: video landed before PCM; audio PTS within one chunk of target"
    );
    readers.0[4].set_paused(true);
    readers.0[4].seek(10_000_000).unwrap();
    latches[4].store(true, Ordering::Release);
    until(
        &mut readers,
        &mut seen,
        Duration::from_secs(2),
        Some(4),
        |s| s[4].failure,
    );
    assert!(
        readers.cancel_and_wait(),
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

#[test]
#[ignore = "Explicit real bundled-decoder qualification; requires generated HLS fixtures"]
fn four_live_hls_windows_advance_after_gateway_timeouts() {
    let _serial = plx_base::testlock::serial();
    let root = PathBuf::from(std::env::var_os("PLXNATIVE_LOCAL_VIDEO_FIXTURES").unwrap());
    assert!(plx_net::net::global_init());
    crate::ff::boot();
    assert!(available(), "bundled H264 decoder unavailable");
    let mut server = FixtureServer::open(root);
    let mut readers = Readers(Vec::new());
    for index in [2, 6, 7, 8] {
        let source = ExternalSource::Hls(server.url(&format!("/live/{index}/media.m3u8")));
        readers
            .0
            .push(Session::open(source, Limits::default()).unwrap());
    }
    let mut seen: Vec<Seen> = (0..4).map(|_| Seen::default()).collect();
    until(
        &mut readers,
        &mut seen,
        Duration::from_secs(15),
        None,
        |slots| {
            server
                .live_grid
                .segment_timeout_reads
                .load(Ordering::Acquire)
                >= 2
                && server.live_grid.segment_503_reads.load(Ordering::Acquire) >= 2
                && slots.iter().all(|slot| slot.frames >= 3)
        },
    );
    let at_recovery: Vec<_> = seen.iter().map(|s| s.last.unwrap()).collect();
    until(
        &mut readers,
        &mut seen,
        Duration::from_secs(12),
        None,
        |slots| {
            slots
                .iter()
                .zip(&at_recovery)
                .all(|(slot, start)| slot.last.unwrap() >= start + 4_000_000)
        },
    );
    assert_eq!(
        seen.iter()
            .filter_map(|s| s.hash)
            .collect::<HashSet<_>>()
            .len(),
        4
    );
    eprintln!("four live HLS streams at and after recovery: {at_recovery:?} -> {:?}",
        seen.iter().map(|s| s.last.unwrap()).collect::<Vec<_>>());
    assert!(readers.cancel_and_wait(), "grid workers did not retire");
    assert!(server.stop());
}

#[path = "local_multiview_output_tests.rs"]
mod output;
