//! Inspection-only transport limits. Ordinary helper startup/IO keeps its existing policy.
use std::{
    io::{self, Read, Write},
    os::{fd::AsRawFd, unix::net::UnixStream},
    time::Instant,
};

pub fn check(deadline: Instant) -> io::Result<()> {
    if Instant::now() >= deadline {
        Err(io::Error::from(io::ErrorKind::TimedOut))
    } else {
        Ok(())
    }
}

pub fn wait(fd: std::os::fd::RawFd, events: libc::c_short, deadline: Instant) -> io::Result<()> {
    loop {
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .filter(|left| !left.is_zero())
            .ok_or(io::ErrorKind::TimedOut)?;
        let millis = i32::try_from(remaining.as_millis().saturating_add(1)).unwrap_or(i32::MAX);
        let mut descriptor = libc::pollfd {
            fd,
            events,
            revents: 0,
        };
        let result = unsafe { libc::poll(&mut descriptor, 1, millis) };
        if result < 0 {
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(error);
        }
        check(deadline)?;
        if result == 0 {
            return Err(io::ErrorKind::TimedOut.into());
        }
        if descriptor.revents & libc::POLLNVAL != 0 {
            return Err(io::Error::from_raw_os_error(libc::EBADF));
        }
        return Ok(());
    }
}

/// The same absolute deadline is retained through all partial reads and writes.
pub struct Stream {
    stream: UnixStream,
    deadline: Instant,
}
impl Stream {
    pub fn new(stream: UnixStream, deadline: Instant) -> io::Result<Self> {
        check(deadline)?;
        stream.set_nonblocking(true)?;
        Ok(Self { stream, deadline })
    }
}
impl Read for Stream {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        loop {
            check(self.deadline)?;
            match self.stream.read(bytes) {
                Ok(count) => {
                    check(self.deadline)?;
                    return Ok(count);
                }
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    wait(self.stream.as_raw_fd(), libc::POLLIN, self.deadline)?
                }
                Err(error) if error.kind() == io::ErrorKind::Interrupted => (),
                Err(error) => return Err(error),
            }
        }
    }
}
impl Write for Stream {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        loop {
            check(self.deadline)?;
            match self.stream.write(bytes) {
                Ok(count) => {
                    check(self.deadline)?;
                    return Ok(count);
                }
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    wait(self.stream.as_raw_fd(), libc::POLLOUT, self.deadline)?
                }
                Err(error) if error.kind() == io::ErrorKind::Interrupted => (),
                Err(error) => return Err(error),
            }
        }
    }
    fn flush(&mut self) -> io::Result<()> {
        check(self.deadline)
    }
}

/// A borrowed RAII guard restores the previous request policy on return or unwind.
pub fn with_deadline_policy<T, R>(
    target: &mut T,
    deadline: Instant,
    replace: fn(&mut T, Option<Instant>) -> Option<Instant>,
    body: impl FnOnce(&mut T) -> R,
) -> R {
    struct Scope<'a, T> {
        target: &'a mut T,
        previous: Option<Instant>,
        replace: fn(&mut T, Option<Instant>) -> Option<Instant>,
    }
    impl<T> Drop for Scope<'_, T> {
        fn drop(&mut self) {
            (self.replace)(self.target, self.previous);
        }
    }
    let previous = replace(target, Some(deadline));
    let scope = Scope {
        target,
        previous,
        replace,
    };
    body(&mut *scope.target)
}

/// Failed LS2 cancellation cannot release a live stack callback. Inspection suppresses
/// persistent failure publication, but keeps the existing fatal safety boundary.
pub fn cancel_failed(read_only: bool, record_failure: impl FnOnce(), terminate: fn(i32) -> !) -> ! {
    if !read_only {
        record_failure();
    }
    terminate(1)
}

#[cfg(test)]
#[path = "inspection_io_tests.rs"]
mod tests;
