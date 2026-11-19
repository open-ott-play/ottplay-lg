//! Nine real H.264/AAC streams under simultaneous live segment pressure.
use super::*;
use std::{
    io::{self, Write},
    net::TcpStream,
    path::Path,
};

const SLOTS: usize = 9;
const PACKET: usize = 188;
const SEGMENT_PACKETS: usize = (10 * 1024 * 1024) / PACKET;

#[derive(Default)]
struct Concurrency {
    waiting: AtomicUsize,
    peak: AtomicUsize,
}

pub(super) struct Origin {
    slot: usize,
    last: AtomicUsize,
    delayed: AtomicBool,
    shared: Arc<Concurrency>,
}

impl Origin {
    pub(super) fn serve(&self, socket: &mut TcpStream, root: &Path, path: &str) -> io::Result<()> {
        if path == "/media.m3u8" {
            let last = (1 + self.last.load(Ordering::Acquire)).min(14);
            let first = last.saturating_sub(3);
            let mut list =
                format!("#EXTM3U\n#EXT-X-TARGETDURATION:2\n#EXT-X-MEDIA-SEQUENCE:{first}\n");
            for seq in first..=last {
                list.push_str(&format!("#EXTINF:2,\nsegment{seq:03}.ts\n"));
            }
            return FixtureServer::response(
                socket,
                "200 OK",
                "Content-Type: application/vnd.apple.mpegurl\r\n",
                list.as_bytes(),
            );
        }
        let Some(name) = path.strip_prefix('/').filter(|name| is_segment(name)) else {
            return FixtureServer::response(socket, "404 Not Found", "", b"");
        };
        let sequence = name[7..10].parse::<usize>().unwrap();
        let input = std::fs::read(root.join(format!("grid9-{}-hls", self.slot)).join(name))?;
        if sequence == 3 && !self.delayed.swap(true, Ordering::AcqRel) {
            let waiting = self.shared.waiting.fetch_add(1, Ordering::AcqRel) + 1;
            self.shared.peak.fetch_max(waiting, Ordering::Relaxed);
            std::thread::sleep(Duration::from_millis(350 + 15 * self.slot as u64));
            self.shared.waiting.fetch_sub(1, Ordering::AcqRel);
        }
        if !(2..=5).contains(&sequence) {
            FixtureServer::response(socket, "200 OK", "Content-Type: video/mp2t\r\n", &input)?;
        } else {
            // A bounded reader may pause a transfer until the preceding segment drains.
            socket.set_write_timeout(Some(Duration::from_secs(5)))?;
            send_large_segment(socket, &input)?;
        }
        self.last.fetch_max(sequence, Ordering::Release);
        Ok(())
    }
}

/// Equal spacing with valid MPEG-TS null packets simulates large constant-rate
/// transports. Only a fixed block is buffered by the fixture HTTP server; the
/// elementary stream, PTS and source-specific pictures remain intact.
fn send_large_segment(socket: &mut TcpStream, input: &[u8]) -> io::Result<()> {
    assert_eq!(input.len() % PACKET, 0);
    let media_packets = input.len() / PACKET;
    assert!((1..SEGMENT_PACKETS).contains(&media_packets));
    write!(socket, "HTTP/1.1 200 OK\r\nConnection: close\r\nContent-Type: video/mp2t\r\nContent-Length: {}\r\n\r\n", SEGMENT_PACKETS * PACKET)?;
    let mut null = [0xff; PACKET];
    null[..4].copy_from_slice(&[0x47, 0x1f, 0xff, 0x10]);
    let mut out = io::BufWriter::with_capacity(64 * 1024, socket);
    for (i, packet) in input.chunks_exact(PACKET).enumerate() {
        assert_eq!(packet[0], 0x47);
        out.write_all(packet)?;
        let next = ((i + 1) * SEGMENT_PACKETS) / media_packets;
        let current = (i * SEGMENT_PACKETS) / media_packets;
        for _ in 1..(next - current) {
            out.write_all(&null)?;
        }
    }
    out.flush()
}

#[test]
#[ignore = "Explicit nine-stream qualification with generated fixtures and bundled decoder"]
fn nine_live_hls_streams_advance_under_buffer_pressure() {
    let _serial = plx_base::testlock::serial();
    let root = PathBuf::from(std::env::var_os("PLXNATIVE_LOCAL_VIDEO_FIXTURES").unwrap());
    boot_real_decoders();
    let shared = Arc::new(Concurrency::default());
    let mut servers = Vec::new();
    let mut readers = Readers(Vec::new());
    let limits = Limits::grid9();
    let started = Instant::now();
    for slot in 0..SLOTS {
        let origin = Origin {
            slot,
            last: AtomicUsize::default(),
            delayed: AtomicBool::default(),
            shared: Arc::clone(&shared),
        };
        let server = FixtureServer::with_state(
            root.clone(),
            LiveGridFixture {
                load: Some(origin),
                ..LiveGridFixture::default()
            },
        );
        let source = ExternalSource::Hls(server.url("/media.m3u8"));
        let session = Session::open_with_output_tile(source, limits, (640, 360)).unwrap();
        readers.0.push(session);
        servers.push(server);
    }
    let mut seen: [Seen; SLOTS] = std::array::from_fn(|_| Seen::default());
    let mut first = [None; SLOTS];
    let mut common_start = None;
    readers.0[0].set_audio(true);
    let mut focus_moved = false;
    let mut first_draw = [None; SLOTS];
    let mut last_draw = [None; SLOTS];
    let mut max_gap = [Duration::ZERO; SLOTS];
    let mut peak_observed_sum = 0;
    let deadline = Instant::now() + Duration::from_secs(40);
    loop {
        let previous: [usize; SLOTS] = std::array::from_fn(|i| seen[i].frames);
        pump_at_size(&mut readers, &mut seen, None, (640, 360));
        for (i, slot) in seen.iter().enumerate() {
            if slot.frames > previous[i] {
                let now = Instant::now();
                first_draw[i].get_or_insert(now);
                if let Some(then) = last_draw[i].replace(now) {
                    max_gap[i] = max_gap[i].max(now.duration_since(then));
                }
            }
        }
        if !focus_moved
            && seen[0].audio > 0
            && first[0]
                .zip(seen[0].last)
                .is_some_and(|(start, pts)| pts - start >= 4_000_000)
        {
            readers.0[0].set_audio(false);
            readers.0[8].set_audio(true);
            focus_moved = true;
        }
        let sum: usize = readers
            .0
            .iter()
            .filter_map(Session::hls_compressed_bytes)
            .map(|(current, _)| current)
            .sum();
        peak_observed_sum = peak_observed_sum.max(sum);
        for (start, slot) in first.iter_mut().zip(&seen) {
            if start.is_none() {
                *start = slot.last;
            }
        }
        if common_start.is_none() && first.iter().all(Option::is_some) {
            common_start = Some(std::array::from_fn::<_, SLOTS, _>(|i| {
                seen[i].last.unwrap()
            }));
        }
        if common_start.is_some_and(|starts| {
            seen.iter()
                .zip(starts)
                .all(|(slot, start)| slot.last.is_some_and(|now| now - start >= 8_200_000))
        }) && servers.iter().all(|server| {
            server
                .live_grid
                .load
                .as_ref()
                .unwrap()
                .last
                .load(Ordering::Acquire)
                >= 5
        }) {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "nine live streams failed to advance: {:?}",
            seen.iter().map(|s| (s.frames, s.last)).collect::<Vec<_>>()
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(
        focus_moved && seen[8].audio > 0,
        "new focus did not produce PCM"
    );
    assert_eq!(
        seen.iter()
            .filter_map(|s| s.hash)
            .collect::<HashSet<_>>()
            .len(),
        SLOTS
    );
    for (index, reader) in readers.0.iter().enumerate() {
        let lane = &seen[index];
        let seconds = (lane.last.unwrap() - first[index].unwrap()) as f64 / 1_000_000.;
        let fps = (lane.frames - 1) as f64 / seconds;
        // Count delivered frames as well as PTS. An occasional frame with far-apart
        // timestamps must not qualify an otherwise stalled pane on a busy CI host.
        assert!(
            fps >= f64::from(limits.max_fps) * 0.85,
            "lane {index} delivered only {fps:.1} fps"
        );
        let (_, peak) = reader.hls_compressed_bytes().unwrap();
        assert!(
            (SEGMENT_PACKETS * PACKET..=limits.max_object_bytes).contains(&peak),
            "lane {index} compressed peak {peak}"
        );
        let (ready, deferrals) = reader.hls_lookahead_counts();
        assert!(ready > 0, "lane {index} never used lookahead");
        let origin = servers[index].live_grid.load.as_ref().unwrap();
        assert!(origin.delayed.load(Ordering::Acquire) && origin.last.load(Ordering::Acquire) >= 5);
        let first_at = first_draw[index].unwrap();
        let last_at = last_draw[index].unwrap();
        let wall_secs = last_at.duration_since(first_at).as_secs_f64();
        let wall_fps = (lane.frames - 1) as f64 / wall_secs;
        let tail = last_at.elapsed();
        eprintln!("nine-HLS slot {}: {} frames, {fps:.1} fps/media, {wall_fps:.1} fps/wall, first at {} ms, measured {wall_secs:.2}s, tail {} ms, peak {peak} bytes, {ready} lookahead hits, {deferrals} budget waits, max observed frame gap {} ms", index + 1, lane.frames, first_at.duration_since(started).as_millis(), tail.as_millis(), max_gap[index].as_millis());
        // Hosted CPUs vary, but repeated second-scale stalls must never pass a PTS-only check.
        assert!(
            wall_fps >= f64::from(limits.max_fps) * 0.5,
            "lane {index} sustained only {wall_fps:.1} fps/wall"
        );
        assert!(
            max_gap[index].max(tail) < Duration::from_millis(1_500),
            "lane {index} stopped delivering during shared playback"
        );
    }
    let overlap = shared.peak.load(Ordering::Relaxed);
    eprintln!("nine-HLS combined sampled compressed high water: {peak_observed_sum} bytes; peak concurrent delayed GETs: {overlap}");
    assert!(
        peak_observed_sum >= 4 * SEGMENT_PACKETS * PACKET
            && peak_observed_sum <= SLOTS * limits.max_object_bytes
    );
    assert!(overlap >= 2, "nine HLS loads ran serially");
    assert!(
        readers.cancel_and_wait(),
        "workers failed to retire after HLS pressure"
    );
    for session in &readers.0 {
        assert_eq!(
            session.hls_compressed_bytes().unwrap().0,
            0,
            "retired worker retained segment bytes"
        );
    }
    assert!(servers.iter_mut().all(FixtureServer::stop));
}
