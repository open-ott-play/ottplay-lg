//! The existing worker retains exclusive ownership of its HLS request context and cookie jar.
//! The decoder accepts one complete, validated object at a time. A rendezvous prevents the
//! reader from starting N+2 before FFmpeg has finished consuming N.
use super::{Control, Error, Hls, Input, Limits, MediaSegment};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    mpsc::{self, Receiver, Sender, SyncSender, TryRecvError},
};

type Object = Result<MediaSegment, Error>;
static READERS: AtomicUsize = AtomicUsize::new(0);
struct Permit;
impl Permit {
    fn acquire() -> Option<Self> {
        let mut current = READERS.load(Ordering::Acquire);
        while current < 4 {
            match READERS.compare_exchange_weak(
                current,
                current + 1,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return Some(Self),
                Err(value) => current = value,
            }
        }
        None
    }
}
impl Drop for Permit {
    fn drop(&mut self) {
        READERS.fetch_sub(1, Ordering::AcqRel);
    }
}

pub(super) struct Consumer {
    request: Sender<()>,
    objects: Receiver<Object>,
    bytes: MediaSegment,
    terminal: Option<Result<(), Error>>,
    pub(super) probe_bytes: usize,
}
impl Consumer {
    pub(super) fn read(&mut self, out: &mut [u8], c: &Control) -> Result<usize, Error> {
        if let Some(terminal) = self.terminal {
            return terminal.map(|()| 0);
        }
        if out.is_empty() {
            return Ok(0);
        }
        if self.bytes.is_empty() {
            let object = self
                .request
                .send(())
                .map_err(|_| Error::Source)
                .and_then(|()| self.objects.recv().unwrap_or(Err(Error::Source)));
            match object {
                Ok(bytes) if !bytes.is_empty() => {
                    self.bytes = bytes;
                }
                terminal => {
                    // libav may read again after an IO error. Keep its original recovery class.
                    let state = terminal.map(|_| ());
                    self.terminal = Some(state);
                    return state.map(|()| 0);
                }
            }
        }
        Ok(self.bytes.read(out, Some(c)))
    }
}

fn serve(hls: &mut Hls, control: &Control, requests: Receiver<()>, responses: SyncSender<Object>) {
    let mut ahead = false;
    loop {
        if !ahead && requests.recv().is_err() {
            return;
        }
        if control.stopped() {
            return;
        }
        // read() validates sequence, URI, object size, content and lease as usual. A media
        // request can only use a segment declared by a parsed playlist. At the live edge
        // its existing poll cadence, timeout and retry budget remain in force.
        let result = loop {
            if !ahead {
                break hls.read(&mut [], control);
            }
            let result = hls.read_available(&mut [], control, super::LIVE_READ_BUDGET, true);
            if result.is_err() || !hls.bytes.is_empty() || hls.playlist.ended {
                break result;
            }
            // A fresh manifest still ends at the current segment. Poll at the
            // normal cadence while decoding continues. Its actual demand owns a
            // fresh timeout, so a long current segment cannot exhaust it early.
            match requests.try_recv() {
                Ok(()) => ahead = false,
                Err(TryRecvError::Empty) => (),
                Err(TryRecvError::Disconnected) => return,
            }
        };
        let object = result.map(|_| std::mem::take(&mut hls.bytes));
        if ahead {
            let requested = match requests.try_recv() {
                Ok(()) => true,
                Err(TryRecvError::Empty) => {
                    if object.as_ref().is_ok_and(|bytes| !bytes.is_empty()) {
                        control.hls_prefetched.fetch_add(1, Ordering::Relaxed);
                    }
                    requests.recv().is_ok()
                }
                Err(TryRecvError::Disconnected) => false,
            };
            if !requested {
                // A decoder that failed independently did not consume a speculative gap.
                hls.reprobe = false;
                return;
            }
        }
        let terminal = object.as_ref().map_or(true, MediaSegment::is_empty);
        let retained = object.as_ref().map_or(0, MediaSegment::capacity);
        // The old allocation is already released when the decoder requests this object.
        control
            .hls_retained_bytes
            .store(retained, Ordering::Release);
        if responses.send(object).is_err() {
            hls.reprobe = false;
            return;
        }
        if terminal {
            return;
        }
        ahead = control.video_decoded.load(Ordering::Acquire)
            && control.max_fps.load(Ordering::Relaxed) > Limits::default().max_fps
            && !control.paused.load(Ordering::Acquire)
            && !hls.playlist.ended;
    }
}

struct DecoderDone<'a>(&'a Control);
impl Drop for DecoderDone<'_> {
    fn drop(&mut self) {
        self.0.reader_abandoned.store(true, Ordering::Release);
        self.0.hls_retained_bytes.store(0, Ordering::Release);
    }
}

/// Keep short-lived FFI pointers on their owning threads. A rejected thread or a full
/// reader budget falls back to the existing synchronous reader including its first bytes.
pub(super) fn try_decode(hls: &mut Hls, limits: Limits, c: &Control) -> Option<Result<(), Error>> {
    let _permit = Permit::acquire()?;
    hls.blocked_media = true;
    if let Err(error) = hls.read(&mut [], c) {
        return Some(Err(error));
    }
    if ![0, 188, 376]
        .iter()
        .all(|&i| hls.bytes.head().get(i) == Some(&0x47))
    {
        hls.blocked_media = false;
        return None;
    }
    let probe_bytes = (hls.bytes.len() / 2).clamp(2048, 1024 * 1024);
    let result = std::thread::scope(|scope| {
        let (request, requests) = mpsc::channel();
        let (responses, objects) = mpsc::sync_channel(0);
        let handle = std::thread::Builder::new()
            .name("local-hls-decode".into())
            .spawn_scoped(scope, move || {
                let _done = DecoderDone(c);
                let mut input = Input::LivePipe(Consumer {
                    request,
                    objects,
                    bytes: MediaSegment::default(),
                    terminal: None,
                    probe_bytes,
                });
                unsafe { super::decode_input(&mut input, limits, c) }
            })
            .ok()?;
        serve(hls, c, requests, responses);
        Some(handle.join().unwrap_or(Err(Error::Decode)))
    });
    hls.blocked_media = false;
    c.hls_retained_bytes.store(0, Ordering::Release);
    c.reader_abandoned.store(false, Ordering::Release);
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::curlio::cookie_tests::{curl, Reply, SessionServer};
    use std::time::{Duration, Instant};

    fn eventually(mut condition: impl FnMut() -> bool) {
        let end = Instant::now() + Duration::from_secs(2);
        while !condition() {
            assert!(Instant::now() < end, "reader synchronization timed out");
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    #[test]
    fn repeated_reads_preserve_terminal_error_and_end_of_file() {
        for error in [
            None,
            Some(Error::LostProviderLease),
            Some(Error::SourceDenied),
            Some(Error::SourceTemporarilyUnavailable),
        ] {
            let (request, requests) = mpsc::channel();
            let (responses, objects) = mpsc::sync_channel(0);
            let mut input = Consumer {
                request,
                objects,
                bytes: MediaSegment::default(),
                terminal: None,
                probe_bytes: 2048,
            };
            let worker = std::thread::spawn(move || {
                requests.recv().unwrap();
                responses
                    .send(error.map_or(Ok(MediaSegment::default()), Err))
                    .unwrap();
            });
            let c = Control::default();
            let expected = error.map_or(Ok(0), Err);
            assert_eq!(input.read(&mut [0; 4], &c), expected);
            worker.join().unwrap();
            assert_eq!(input.read(&mut [0; 4], &c), expected);
        }
    }

    #[test]
    fn one_lookahead_reuses_cookies_and_cancellation_makes_no_further_request() {
        let _serial = curl();
        let c = Control::default();
        c.video_decoded.store(true, Ordering::Release);
        c.max_fps.store(25, Ordering::Release);
        let list = b"#EXTM3U\n#EXTINF:1,\na.ts\n#EXTINF:1,\nb.ts\n";
        let server = SessionServer::new(vec![
            Reply::ok("Set-Cookie: owner=tile; Path=/\r\n", list),
            Reply::ok("Set-Cookie: owner=rotated; Path=/\r\n", b"abcd").requiring("owner=tile"),
            Reply::ok("", b"efgh").requiring("owner=rotated"),
        ]);
        let mut hls = Hls::open(format!("{}/leaf.m3u8", server.base), 1024, &c).unwrap();
        let (request, requests) = mpsc::channel();
        let (responses, objects) = mpsc::sync_channel(0);
        std::thread::scope(|scope| {
            scope.spawn(|| {
                let mut input = Consumer {
                    request,
                    objects,
                    bytes: MediaSegment::default(),
                    terminal: None,
                    probe_bytes: 2048,
                };
                let mut byte = [0];
                assert_eq!(input.read(&mut byte, &c), Ok(1));
                assert_eq!(byte, [b'a']);
                eventually(|| server.heads().len() == 3);
                c.cancel.store(true, Ordering::Release);
            });
            serve(&mut hls, &c, requests, responses);
        });
        let requests = server.heads();
        assert_eq!(requests.len(), 3);
        assert!(requests[2].starts_with("GET /b.ts ") && requests[2].contains("owner=rotated"));
    }

    #[test]
    fn budget_deferral_retains_the_open_response_and_observes_cancellation() {
        let _serial = curl();
        for cancel in [false, true] {
            let c = Control::default();
            let body = vec![0x47; 48 * 1024];
            let server = SessionServer::new(vec![Reply::ok("", &body)]);
            let mut remote =
                super::super::Remote::open(&format!("{}/segment.ts", server.base), false, &c)
                    .unwrap();
            c.hls_retained_bytes.store(40 * 1024, Ordering::Release);
            let result = std::thread::scope(|scope| {
                scope.spawn(|| {
                    eventually(|| c.hls_prefetch_limited.load(Ordering::Relaxed) > 0);
                    if cancel {
                        c.cancel.store(true, Ordering::Release);
                    } else {
                        c.hls_retained_bytes.store(0, Ordering::Release);
                    }
                });
                MediaSegment::collect(&mut remote, 64 * 1024, &c)
            });
            if cancel {
                assert!(matches!(result, Err(Error::Cancelled)));
            } else {
                assert_eq!(result.unwrap().head(), body);
            }
            assert_eq!(c.hls_prefetch_limited.load(Ordering::Relaxed), 1);
            assert_eq!(server.heads().len(), 1);
        }
    }
    #[test]
    fn waiting_for_compressed_buffer_does_not_consume_recovery_budget() {
        let _serial = curl();
        let c = Control::default();
        let playlist = b"#EXTM3U\n#EXTINF:1,\na.ts\n";
        let server = SessionServer::new(vec![
            Reply::ok("", playlist),
            Reply::ok("", super::super::UPSTREAM_TIMEOUT),
            Reply::ok("", playlist),
            Reply::ok("", b"A"),
        ]);
        let mut hls = Hls::open(format!("{}/leaf.m3u8", server.base), 32, &c).unwrap();
        hls.blocked_media = true;
        c.hls_retained_bytes.store(31, Ordering::Release);
        let mut output = [0];
        std::thread::scope(|scope| {
            scope.spawn(|| {
                eventually(|| c.hls_prefetch_limited.load(Ordering::Relaxed) > 0);
                std::thread::sleep(Duration::from_millis(550));
                c.hls_retained_bytes.store(0, Ordering::Release);
            });
            assert_eq!(
                hls.read_with_budget(&mut output, &c, Duration::from_millis(450)),
                Ok(1)
            );
        });
        assert_eq!(output, [b'A']);
        assert_eq!(server.heads().len(), 4);
    }
    #[test]
    fn early_lookup_of_an_unpublished_edge_preserves_the_later_demand() {
        let _serial = curl();
        let c = Control::default();
        let initial = b"#EXTM3U\n#EXT-X-TARGETDURATION:60\n#EXTINF:60,\na.ts\n";
        let published =
            b"#EXTM3U\n#EXT-X-TARGETDURATION:60\n#EXT-X-MEDIA-SEQUENCE:1\n#EXTINF:60,\nb.ts\n";
        let server = SessionServer::new(vec![
            Reply::ok("", initial),
            Reply::ok("", b"A"),
            Reply::ok("", initial),
            Reply::ok("", published),
            Reply::ok("", b"B"),
        ]);
        let mut hls = Hls::open(format!("{}/edge.m3u8", server.base), 1024, &c).unwrap();
        assert_eq!(hls.read(&mut [], &c), Ok(0));
        assert_eq!(std::mem::take(&mut hls.bytes).head(), b"A");
        hls.checked -= Duration::from_secs(2);
        assert_eq!(
            hls.read_available(&mut [], &c, Duration::from_millis(350), true),
            Ok(0)
        );
        assert!(hls.bytes.is_empty() && !hls.playlist.ended);
        assert_eq!(server.heads().len(), 3);
        hls.checked -= Duration::from_secs(2);
        let mut next = [0];
        assert_eq!(
            hls.read_with_budget(&mut next, &c, Duration::from_millis(350)),
            Ok(1)
        );
        assert_eq!(next, [b'B']);
        assert_eq!(server.heads().len(), 5);
    }
    #[test]
    fn two_large_ts_segments_prefetch_before_the_first_ends_within_one_budget() {
        const MIB: usize = 1024 * 1024;
        const CAP: usize = 16 * MIB;
        let make_ts = |size, id| {
            let mut packet = [id; 188];
            packet[0] = 0x47;
            packet.repeat(size / 188)
        };
        let first = make_ts(12 * MIB, 1);
        let second = make_ts(10 * MIB, 2);
        let _serial = curl();
        let server = SessionServer::new(vec![
            Reply::ok(
                "",
                b"#EXTM3U\n#EXT-X-TARGETDURATION:12\n#EXTINF:12,\na.ts\n#EXTINF:12,\nb.ts\n",
            ),
            Reply::ok("", &first),
            Reply::ok("", &second),
        ]);
        let c = Control::default();
        c.video_decoded.store(true, Ordering::Release);
        c.max_fps.store(25, Ordering::Release);
        let mut hls = Hls::open(format!("{}/live.m3u8", server.base), CAP, &c).unwrap();
        hls.blocked_media = true;
        let (request, requests) = mpsc::channel();
        let (responses, objects) = mpsc::sync_channel(0);
        std::thread::scope(|scope| {
            scope.spawn(|| {
                let mut input = Consumer {
                    request,
                    objects,
                    bytes: MediaSegment::default(),
                    terminal: None,
                    probe_bytes: 2048,
                };
                let mut buf = vec![0; 64 * 1024];
                let mut used = input.read(&mut buf, &c).unwrap();
                assert_eq!(buf[..used], first[..used]);
                eventually(|| c.hls_prefetch_limited.load(Ordering::Relaxed) != 0);
                while first.len() - used > 2 * MIB {
                    let n = input.read(&mut buf, &c).unwrap();
                    assert_eq!(buf[..n], first[used..used + n]);
                    used += n;
                }
                // The second segment is completely downloaded while FFmpeg still
                // has almost two MiB of the first left to read.
                eventually(|| c.hls_prefetched.load(Ordering::Relaxed) == 1);
                assert!(c.hls_retained_bytes.load(Ordering::Acquire) >= MIB);
                c.max_fps.store(15, Ordering::Release);
                while used < first.len() {
                    let n = input.read(&mut buf, &c).unwrap();
                    assert_eq!(buf[..n], first[used..used + n]);
                    used += n;
                }
                let mut used = 0;
                while used < second.len() {
                    let n = input.read(&mut buf, &c).unwrap();
                    assert_eq!(buf[..n], second[used..used + n]);
                    used += n;
                }
                c.cancel.store(true, Ordering::Release);
            });
            serve(&mut hls, &c, requests, responses);
        });
        let peak = c.hls_peak_compressed.load(Ordering::Relaxed);
        assert!(
            peak <= CAP && peak > CAP - 256 * 1024,
            "unexpected peak {peak}"
        );
        assert_eq!(c.hls_retained_bytes.load(Ordering::Acquire), 0);
        assert_eq!(server.heads().len(), 3);
    }
}
