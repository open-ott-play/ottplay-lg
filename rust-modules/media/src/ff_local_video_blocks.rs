//! Compressed media ownership during a live HLS handoff. The transport fills each
//! block directly. FFmpeg releases consumed blocks without copying the remainder.
use super::{Control, Error, Remote};
use std::{collections::VecDeque, sync::Arc, time::Instant};

const BLOCK: usize = 128 * 1024;

#[derive(Default)]
pub(super) struct Segment {
    blocks: VecDeque<Vec<u8>>,
    offset: usize,
    len: usize,
    reserved: usize,
    owner: Option<Arc<crate::local_multiview::CompressedMemory>>,
}

impl Segment {
    fn push(&mut self, bytes: Vec<u8>) {
        self.len += bytes.len();
        self.reserved += bytes.capacity();
        self.blocks.push_back(bytes);
    }

    pub(super) fn head(&self) -> &[u8] {
        self.blocks
            .front()
            .map_or(&[], |bytes| &bytes[self.offset..])
    }
    pub(super) fn len(&self) -> usize {
        self.len
    }
    #[cfg(test)]
    pub(super) fn capacity(&self) -> usize {
        self.reserved
    }
    pub(super) fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub(super) fn read(&mut self, out: &mut [u8]) -> usize {
        let mut done = 0;
        while done < out.len() {
            let Some(block) = self.blocks.front() else {
                break;
            };
            let n = (out.len() - done).min(block.len() - self.offset);
            out[done..done + n].copy_from_slice(&block[self.offset..self.offset + n]);
            done += n;
            self.offset += n;
            self.len -= n;
            if self.offset == block.len() {
                let capacity = block.capacity();
                drop(self.blocks.pop_front());
                self.reserved -= capacity;
                self.offset = 0;
                if let Some(owner) = &self.owner {
                    owner.release(capacity);
                }
            }
        }
        done
    }

    pub(super) fn collect(remote: &mut Remote, cap: usize, c: &Control) -> Result<Self, Error> {
        let mut out = Self::default();
        out.owner = Some(Arc::clone(&c.hls_compressed));
        let mut expires = remote.deadline();
        let mut deferred = false;
        let expected = usize::try_from(remote.source.size()).ok();
        loop {
            // Read a single byte before allocating another block so an EOF exactly at
            // the budget can finish while FFmpeg still owns the preceding segment.
            let mut first = [0];
            if checked_read(remote, &mut first, expires, c)? == 0 {
                return Ok(out);
            }
            let remaining = cap
                .checked_sub(out.reserved)
                .filter(|&n| n != 0)
                .ok_or(Error::Budget)?;
            let declared = expected
                .and_then(|n| n.checked_sub(out.len))
                .filter(|&n| n > 0);
            let size = remaining.min(BLOCK).min(declared.unwrap_or(BLOCK));
            let delay = c
                .hls_compressed
                .wait_for_room(cap - size, c, &mut deferred)?;
            expires += delay;
            if let Some(end) = &mut remote.deadline {
                *end += delay;
            }
            let mut block = Vec::new();
            block.try_reserve_exact(size).map_err(|_| Error::Budget)?;
            if !c.hls_compressed.charge(block.capacity(), cap) {
                return Err(Error::Budget);
            }
            block.resize(size, 0);
            out.push(block);
            let block = out.blocks.back_mut().unwrap();
            block[0] = first[0];
            let mut filled = 1;
            while filled < size {
                let n = checked_read(remote, &mut block[filled..], expires, c)?;
                if n == 0 {
                    block.truncate(filled);
                    out.len -= size - filled;
                    return Ok(out);
                }
                filled += n;
            }
        }
    }
}

impl Drop for Segment {
    fn drop(&mut self) {
        if let Some(owner) = &self.owner {
            if self.reserved != 0 {
                self.blocks.clear();
                owner.release(self.reserved);
            }
        }
    }
}

fn checked_read(
    remote: &mut Remote,
    out: &mut [u8],
    expires: Instant,
    c: &Control,
) -> Result<usize, Error> {
    if Instant::now() >= expires {
        remote.failure = Some(crate::curlio::OpenErr::Deadline);
        return Err(Error::Source);
    }
    remote.read(out, c)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::{Read, Write},
        time::Duration,
    };

    #[test]
    fn nine_live_inputs_can_fill_their_own_budgets_and_cancel_independently() {
        use crate::curlio::cookie_tests::{curl, Reply, SessionServer};
        use std::sync::{atomic::Ordering, mpsc, Arc};
        const MIB: usize = 1024 * 1024;
        let cap = crate::local_multiview::Limits::grid9().max_object_bytes;
        let _serial = curl();
        let replies: Arc<[Reply]> = vec![
            Reply::ok("", &vec![0x47; 12 * MIB]),
            Reply::ok("", &vec![0x4a; 10 * MIB]),
        ]
        .into();
        let servers: Vec<_> = (0..9)
            .map(|_| SessionServer::with_shared_replies(Arc::clone(&replies)))
            .collect();
        let controls: Vec<_> = (0..9).map(|_| Control::default()).collect();
        std::thread::scope(|scope| {
            let mut results = Vec::new();
            let mut firsts = Vec::new();
            for (server, c) in servers.iter().zip(&controls) {
                let (send, receive) = mpsc::channel();
                results.push(scope.spawn(move || {
                    let mut remote = Remote::open(&format!("{}/current", server.base), false, c)?;
                    let first = Segment::collect(&mut remote, cap, c)?;
                    send.send(first).unwrap();
                    remote.reopen(&format!("{}/next", server.base), c)?;
                    Segment::collect(&mut remote, cap, c)
                }));
                firsts.push(receive);
            }
            let mut firsts: Vec<_> = firsts
                .into_iter()
                .map(|rx| rx.recv_timeout(Duration::from_secs(10)).unwrap())
                .collect();
            let end = Instant::now() + Duration::from_secs(10);
            while controls
                .iter()
                .any(|c| c.hls_prefetch_limited.load(Ordering::Relaxed) == 0)
            {
                assert!(
                    Instant::now() < end,
                    "nine compressed input readers did not reach their limits"
                );
                std::thread::sleep(Duration::from_millis(2));
            }
            let used: usize = controls.iter().map(|c| c.hls_compressed.usage().0).sum();
            assert_eq!(used, 9 * cap);
            // The last reader is cancelled while it owns a partial next object. Its
            // current input is retained; all other readers must finish independently.
            controls[8].cancel.store(true, Ordering::Release);
            let cancelled = results.pop().unwrap().join().unwrap();
            assert!(matches!(cancelled, Err(Error::Cancelled)));
            assert_eq!(controls[8].hls_compressed.usage(), (12 * MIB, cap));
            let mut buf = vec![0; BLOCK];
            for (id, first) in firsts.iter_mut().take(8).enumerate() {
                while first.len() > 2 * MIB {
                    assert_eq!(first.read(&mut buf), BLOCK);
                    assert!(buf.iter().all(|&byte| byte == 0x47));
                }
                assert_eq!(controls[id].hls_prefetch_limited.load(Ordering::Relaxed), 1);
            }
            for (id, result) in results.into_iter().enumerate() {
                let next = result.join().unwrap().unwrap();
                assert_eq!(next.len(), 10 * MIB);
                assert_eq!(next.head()[0], 0x4a);
                assert_eq!(controls[id].hls_compressed.usage(), (12 * MIB, cap));
            }
        });
        assert!(controls
            .iter()
            .all(|c| c.hls_compressed.usage() == (0, cap)));
        assert!(servers.iter().all(|server| server.heads().len() == 2));
    }

    #[test]
    fn finite_fragmented_hls_releases_init_and_media_while_reading_on_demand() {
        use crate::curlio::cookie_tests::{curl, Reply, SessionServer};
        let _serial = curl();
        let server = SessionServer::new(vec![
            Reply::ok(
                "",
                b"#EXTM3U\n#EXT-X-MAP:URI=\"init.mp4\"\n#EXTINF:2,\npart.m4s\n#EXT-X-ENDLIST\n",
            ),
            Reply::ok("", b"init"),
            Reply::ok("", &vec![0x42; 2 * BLOCK]),
        ]);
        let c = Control::default();
        let mut hls =
            super::super::Hls::open(format!("{}/vod.m3u8", server.base), 2 * BLOCK, &c).unwrap();
        let mut out = vec![0; BLOCK];
        assert_eq!(hls.read(&mut out[..2], &c), Ok(2));
        assert_eq!(&out[..2], b"in");
        assert_eq!(c.hls_compressed.usage(), (4, 4));
        assert_eq!(hls.read(&mut out, &c), Ok(2));
        assert_eq!(&out[..2], b"it");
        assert_eq!(c.hls_compressed.usage(), (0, 4));
        assert_eq!(server.heads().len(), 2);
        assert_eq!(hls.read(&mut out, &c), Ok(BLOCK));
        assert!(out.iter().all(|byte| *byte == 0x42));
        assert_eq!(c.hls_compressed.usage(), (BLOCK, 2 * BLOCK));
        assert_eq!(hls.read(&mut out, &c), Ok(BLOCK));
        assert_eq!(hls.read(&mut out, &c), Ok(0));
        assert_eq!(c.hls_compressed.usage(), (0, 2 * BLOCK));
        assert_eq!(server.heads().len(), 3);
    }

    #[test]
    fn chunked_body_at_exact_cap_finishes_and_one_more_byte_is_rejected() {
        let _serial = crate::curlio::cookie_tests::curl();
        for excess in [0, 1] {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let url = format!("http://{}/test.ts", listener.local_addr().unwrap());
            let sender = std::thread::spawn(move || {
                let (mut socket, _) = listener.accept().unwrap();
                socket
                    .set_read_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                socket
                    .set_write_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                let mut request = Vec::new();
                while !request.ends_with(b"\r\n\r\n") {
                    let mut byte = [0];
                    socket.read_exact(&mut byte).unwrap();
                    request.push(byte[0]);
                }
                write!(socket, "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n{:x}\r\n", BLOCK + excess).unwrap();
                socket.write_all(&vec![0x47; BLOCK + excess]).unwrap();
                socket.write_all(b"\r\n0\r\n\r\n").unwrap();
            });
            let c = Control::default();
            let mut remote = Remote::open(&url, false, &c).unwrap();
            assert_eq!(remote.source.size(), -1);
            let result = Segment::collect(&mut remote, BLOCK, &c);
            if excess == 0 {
                let segment = result.unwrap();
                assert_eq!(segment.len(), BLOCK);
                assert_eq!(segment.capacity(), BLOCK);
                assert!(segment.head().iter().all(|&v| v == 0x47));
            } else {
                assert!(matches!(result, Err(Error::Budget)));
            }
            sender.join().unwrap();
        }
    }
}
