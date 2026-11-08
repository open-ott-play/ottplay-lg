//! The one accept for a loopback test server whose listener is nonblocking.
//!
//! A fixture makes its listener nonblocking so the acceptor can poll a stop flag or a
//! deadline. Darwin hands the accepted socket the listener's O_NONBLOCK; Linux does not. A
//! reader that assumed blocking I/O then sees `WouldBlock` whenever a parallel suite accepts
//! before the request line is buffered, and a writer can drop a body on a full send buffer —
//! flakes that exist on the Mac only. Every such acceptor accepts through here, so the
//! connection behaves as on Linux and read/write timeouts mean what they say.

/// `listener.accept()`, with the accepted socket put back in blocking mode.
pub fn accept(
    listener: &std::net::TcpListener,
) -> std::io::Result<(std::net::TcpStream, std::net::SocketAddr)> {
    let (socket, peer) = listener.accept()?;
    socket.set_nonblocking(false)?;
    Ok((socket, peer))
}

/// Bounded accept for a fixture that must never hold the suite open waiting for
/// a request. The listener becomes nonblocking; the returned socket is blocking
/// on every host, just as with [`accept`].
pub fn accept_within(
    listener: &std::net::TcpListener,
    timeout: std::time::Duration,
) -> std::io::Result<(std::net::TcpStream, std::net::SocketAddr)> {
    use std::io::ErrorKind;
    use std::time::Duration;
    let worker = crate::task::OffFrame::for_test();
    let budget = crate::task::time::Budget::start(&worker, timeout);
    listener.set_nonblocking(true)?;
    loop {
        if budget.expired(&worker) {
            return Err(ErrorKind::TimedOut.into());
        }
        match accept(listener) {
            Ok(connection) => return Ok(connection),
            Err(error)
                if matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::Interrupted) =>
            {
                budget.wait(
                    &worker,
                    Duration::from_millis(5),
                    Duration::from_millis(5),
                    &|| false,
                );
            }
            Err(error) => return Err(error),
        }
    }
}
