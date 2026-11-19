//! The existing worker retains exclusive ownership of its HLS request context and cookie jar.
//! The decoder accepts one complete, validated object at a time. A rendezvous prevents the
//! reader from starting N+2 before FFmpeg has finished consuming N.
use super::{Control, Error, Hls, Input, Limits};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    mpsc::{self, Receiver, Sender, SyncSender, TryRecvError},
};

type Object = Result<Vec<u8>, Error>;
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
    bytes: Vec<u8>,
    pos: usize,
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
        if self.pos == self.bytes.len() {
            // Release the consumed object before permitting a new transfer.
            self.bytes = Vec::new();
            c.hls_retained_bytes.store(0, Ordering::Release);
            let object = self
                .request
                .send(())
                .map_err(|_| Error::Source)
                .and_then(|()| self.objects.recv().unwrap_or(Err(Error::Source)));
            match object {
                Ok(bytes) if !bytes.is_empty() => {
                    self.bytes = bytes;
                    self.pos = 0;
                }
                terminal => {
                    // libav may read again after an IO error. Keep its original recovery class.
                    let state = terminal.map(|_| ());
                    self.terminal = Some(state);
                    return state.map(|()| 0);
                }
            }
        }
        let n = out.len().min(self.bytes.len() - self.pos);
        out[..n].copy_from_slice(&self.bytes[self.pos..self.pos + n]);
        self.pos += n;
        Ok(n)
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
        let object = hls.read(&mut [], control).map(|_| {
            hls.pos = 0;
            std::mem::take(&mut hls.bytes)
        });
        let mut ready_in_advance = false;
        if ahead {
            let requested = match requests.try_recv() {
                Ok(()) => true,
                Err(TryRecvError::Empty) => {
                    ready_in_advance = true;
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
        let terminal = object.as_ref().map_or(true, Vec::is_empty);
        let retained = object.as_ref().map_or(0, Vec::capacity);
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
        if ready_in_advance {
            control.hls_prefetched.fetch_add(1, Ordering::Relaxed);
        }
        // If no allocation room remains, wait for demand before opening another transfer.
        // An unexpectedly large next object can also wait inside collect_with_budget.
        ahead = control.video_decoded.load(Ordering::Acquire)
            && control.max_fps.load(Ordering::Relaxed) > Limits::default().max_fps
            && !control.paused.load(Ordering::Acquire)
            && !hls.playlist.ended;
        if ahead && retained >= hls.cap {
            control.hls_prefetch_limited.fetch_add(1, Ordering::Relaxed);
            ahead = false;
        }
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
    if let Err(error) = hls.read(&mut [], c) {
        return Some(Err(error));
    }
    if ![0, 188, 376]
        .iter()
        .all(|&i| hls.bytes.get(i) == Some(&0x47))
    {
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
                    bytes: Vec::new(),
                    pos: 0,
                    terminal: None,
                    probe_bytes,
                });
                unsafe { super::decode_input(&mut input, limits, c) }
            })
            .ok()?;
        serve(hls, c, requests, responses);
        Some(handle.join().unwrap_or(Err(Error::Decode)))
    });
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
                bytes: Vec::new(),
                pos: 0,
                terminal: None,
                probe_bytes: 2048,
            };
            let worker = std::thread::spawn(move || {
                requests.recv().unwrap();
                responses.send(error.map_or(Ok(Vec::new()), Err)).unwrap();
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
                    bytes: Vec::new(),
                    pos: 0,
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
                super::super::collect_with_budget(&mut remote, 64 * 1024, &c, true)
            });
            if cancel {
                assert_eq!(result, Err(Error::Cancelled));
            } else {
                assert_eq!(result.unwrap(), body);
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
}
