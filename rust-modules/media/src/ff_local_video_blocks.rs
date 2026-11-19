//! Compressed media ownership during a live HLS handoff. The transport fills each
//! block directly. FFmpeg releases consumed blocks without copying the remainder.
use super::{Control, Error, Remote};
use std::{
    collections::VecDeque,
    sync::atomic::Ordering,
    time::{Duration, Instant},
};

const BLOCK: usize = 128 * 1024;

#[derive(Default)]
pub(super) struct Segment {
    blocks: VecDeque<Vec<u8>>,
    offset: usize,
    len: usize,
    reserved: usize,
}

impl Segment {
    pub(super) fn from_vec(bytes: Vec<u8>) -> Self {
        let mut segment = Self::default();
        if !bytes.is_empty() {
            segment.push(bytes);
        }
        segment
    }

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
    pub(super) fn capacity(&self) -> usize {
        self.reserved
    }
    pub(super) fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub(super) fn read(&mut self, out: &mut [u8], retained: Option<&Control>) -> usize {
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
                if let Some(c) = retained {
                    c.hls_retained_bytes.fetch_sub(capacity, Ordering::AcqRel);
                }
            }
        }
        done
    }

    pub(super) fn collect(remote: &mut Remote, cap: usize, c: &Control) -> Result<Self, Error> {
        let mut out = Self::default();
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
            let max_retained = remaining - size;
            let mut retained = c.hls_retained_bytes.load(Ordering::Acquire);
            if retained > max_retained {
                if !deferred {
                    c.hls_prefetch_limited.fetch_add(1, Ordering::Relaxed);
                    deferred = true;
                }
                let parked = Instant::now();
                while retained > max_retained {
                    if !c.wait(Duration::from_millis(5)) {
                        return Err(Error::Cancelled);
                    }
                    retained = c.hls_retained_bytes.load(Ordering::Acquire);
                }
                let delay = parked.elapsed();
                expires += delay;
                if let Some(end) = &mut remote.deadline {
                    *end += delay;
                }
            }
            let mut block = Vec::new();
            block.try_reserve_exact(size).map_err(|_| Error::Budget)?;
            if block.capacity() > remaining - retained {
                return Err(Error::Budget);
            }
            block.resize(size, 0);
            #[cfg(test)]
            c.hls_peak_compressed.fetch_max(
                out.reserved + block.capacity() + retained,
                Ordering::Relaxed,
            );
            block[0] = first[0];
            let mut filled = 1;
            while filled < size {
                let n = checked_read(remote, &mut block[filled..], expires, c)?;
                if n == 0 {
                    block.truncate(filled);
                    out.push(block);
                    return Ok(out);
                }
                filled += n;
            }
            out.push(block);
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
    use std::io::{Read, Write};

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
