//! Off-frame HTTP client for the process-owned loopback companion only.
//!
//! A direct socket deliberately bypasses environment HTTP proxies. The capability in the
//! request path must never be forwarded to another host, a redirect, or a proxy. This is a
//! bounded JSON control plane, not a replacement for the application's media transport.
use super::protocol::{self, Endpoint, Error, Reply, Request};
use plx_base::task::{
    time::{Budget, Deadline},
    OffFrame,
};
use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4, TcpStream};
use std::time::Duration;

pub struct Client {
    endpoint: Endpoint,
}

impl Client {
    pub fn new(endpoint: Endpoint) -> Self {
        Self { endpoint }
    }

    pub fn endpoint(&self) -> &Endpoint {
        &self.endpoint
    }

    /// Cancellation is checked at least every 200 ms while the peer is stalled. Cancelling a
    /// Create after sending it may leave a lease whose ID is unknown: its process owner must
    /// stop that helper. Media-only cancellation should instead accept and DELETE the reply.
    pub fn execute_cancellable(
        &self,
        worker: &OffFrame,
        request: &Request,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<Reply, Error> {
        let prepared = request.prepare()?;
        if cancelled() {
            return Err(Error("companion_cancelled"));
        }
        let deadline = Deadline::after(worker, Duration::from_secs(25));
        let address = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, self.endpoint.port()));
        let socket = connect(worker, address, cancelled)?;
        let mut stream = DeadlineStream {
            socket,
            deadline,
            cancelled,
            worker,
        };
        let user_agent = plx_net::net::user_agent_header_line();
        let header = format!("{} {} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\n{user_agent}Accept: application/json\r\nAccept-Encoding: identity\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            prepared.method, self.endpoint.path(&prepared.route), self.endpoint.port(), prepared.body.len());
        stream.write_all(header.as_bytes()).map_err(io_error)?;
        stream.write_all(&prepared.body).map_err(io_error)?;
        let response = read_response(&mut BufReader::with_capacity(8192, stream))?;
        if deadline.expired(worker) {
            return Err(Error("companion_timeout"));
        }
        protocol::decode(&self.endpoint, request, response.status, &response.body)
    }
}

fn connect(
    worker: &OffFrame,
    address: SocketAddr,
    cancelled: &dyn Fn() -> bool,
) -> Result<TcpStream, Error> {
    let budget = Budget::start(worker, Duration::from_secs(2));
    loop {
        if cancelled() {
            return Err(Error("companion_cancelled"));
        }
        let remaining = budget.remaining(worker);
        if remaining.is_zero() {
            return Err(Error("companion_timeout"));
        }
        match TcpStream::connect_timeout(&address, remaining.min(Duration::from_millis(200))) {
            Ok(socket) => return Ok(socket),
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::TimedOut
                        | io::ErrorKind::WouldBlock
                        | io::ErrorKind::Interrupted
                ) => {}
            Err(error) => return Err(io_error(error)),
        }
    }
}

#[derive(Debug)]
struct Cancelled;
impl std::fmt::Display for Cancelled {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("companion_cancelled")
    }
}
impl std::error::Error for Cancelled {}

struct DeadlineStream<'a> {
    socket: TcpStream,
    deadline: Deadline,
    cancelled: &'a dyn Fn() -> bool,
    worker: &'a OffFrame,
}
impl DeadlineStream<'_> {
    fn remaining(&self) -> io::Result<Duration> {
        if (self.cancelled)() {
            return Err(io::Error::new(io::ErrorKind::ConnectionAborted, Cancelled));
        }
        let remaining = self.deadline.remaining(self.worker);
        if remaining.is_zero() {
            Err(io::ErrorKind::TimedOut.into())
        } else {
            Ok(remaining)
        }
    }
}
impl Read for DeadlineStream<'_> {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        loop {
            self.socket
                .set_read_timeout(Some(self.remaining()?.min(Duration::from_millis(200))))?;
            match self.socket.read(bytes) {
                Err(error)
                    if matches!(
                        error.kind(),
                        io::ErrorKind::TimedOut
                            | io::ErrorKind::WouldBlock
                            | io::ErrorKind::Interrupted
                    ) => {}
                result => return result,
            }
        }
    }
}
impl Write for DeadlineStream<'_> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        loop {
            self.socket
                .set_write_timeout(Some(self.remaining()?.min(Duration::from_millis(200))))?;
            match self.socket.write(bytes) {
                Err(error)
                    if matches!(
                        error.kind(),
                        io::ErrorKind::TimedOut
                            | io::ErrorKind::WouldBlock
                            | io::ErrorKind::Interrupted
                    ) => {}
                result => return result,
            }
        }
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

struct Response {
    status: u16,
    body: Vec<u8>,
}

fn io_error(error: io::Error) -> Error {
    if error.get_ref().is_some_and(|inner| inner.is::<Cancelled>()) {
        return Error("companion_cancelled");
    }
    match error.kind() {
        io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock => Error("companion_timeout"),
        io::ErrorKind::UnexpectedEof => Error("companion_response_truncated"),
        _ => Error("companion_unavailable"),
    }
}

/// Read one CRLF-terminated line under a shared byte budget. Unlike read_until, this never
/// allocates an unbounded unterminated line, including for response headers and chunk extensions.
fn line<R: BufRead>(reader: &mut R, remaining: &mut usize) -> Result<Vec<u8>, Error> {
    let mut line = Vec::new();
    loop {
        let available = reader.fill_buf().map_err(io_error)?;
        if available.is_empty() {
            return Err(Error("companion_response_truncated"));
        }
        let end = available.iter().position(|byte| *byte == b'\n');
        let count = end.map_or(available.len(), |position| position + 1);
        if count > *remaining {
            return Err(Error("companion_response_limit"));
        }
        line.extend_from_slice(&available[..count]);
        reader.consume(count);
        *remaining -= count;
        if end.is_some() {
            break;
        }
    }
    if !line.ends_with(b"\r\n") {
        return Err(Error("companion_http_invalid"));
    }
    line.truncate(line.len() - 2);
    Ok(line)
}

fn header(line: &[u8]) -> Result<(&str, &str), Error> {
    let value = std::str::from_utf8(line).map_err(|_| Error("companion_http_invalid"))?;
    let (name, value) = value
        .split_once(':')
        .ok_or(Error("companion_http_invalid"))?;
    if name.is_empty()
        || !name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b))
        || value.bytes().any(|b| b.is_ascii_control() && b != b'\t')
    {
        return Err(Error("companion_http_invalid"));
    }
    Ok((name, value.trim_matches([' ', '\t'])))
}

fn read_response<R: BufRead>(reader: &mut R) -> Result<Response, Error> {
    let mut header_budget = 16 * 1024;
    let status_line = line(reader, &mut header_budget)?;
    let status_line =
        std::str::from_utf8(&status_line).map_err(|_| Error("companion_http_invalid"))?;
    let mut parts = status_line.splitn(3, ' ');
    let version = parts.next().unwrap_or_default();
    let status = parts.next().unwrap_or_default();
    if !matches!(version, "HTTP/1.0" | "HTTP/1.1")
        || status.len() != 3
        || !status.bytes().all(|b| b.is_ascii_digit())
    {
        return Err(Error("companion_http_invalid"));
    }
    let status: u16 = status
        .parse()
        .map_err(|_| Error("companion_http_invalid"))?;
    if !(200..600).contains(&status) {
        return Err(Error("companion_http_invalid"));
    }
    let mut length = None;
    let mut chunked = false;
    loop {
        let bytes = line(reader, &mut header_budget)?;
        if bytes.is_empty() {
            break;
        }
        let (name, value) = header(&bytes)?;
        if name.eq_ignore_ascii_case("content-length") {
            if length.is_some() || value.is_empty() || !value.bytes().all(|b| b.is_ascii_digit()) {
                return Err(Error("companion_http_invalid"));
            }
            let count: usize = value
                .parse()
                .map_err(|_| Error("companion_response_limit"))?;
            if count > protocol::RESPONSE_BYTES {
                return Err(Error("companion_response_limit"));
            }
            length = Some(count);
        } else if name.eq_ignore_ascii_case("transfer-encoding") {
            if chunked || !value.eq_ignore_ascii_case("chunked") {
                return Err(Error("companion_http_invalid"));
            }
            chunked = true;
        } else if name.eq_ignore_ascii_case("content-encoding")
            && !value.eq_ignore_ascii_case("identity")
        {
            return Err(Error("companion_http_invalid"));
        }
    }
    if chunked && length.is_some() {
        return Err(Error("companion_http_invalid"));
    }
    if status == 204 {
        if chunked || length.is_some_and(|n| n != 0) {
            return Err(Error("companion_http_invalid"));
        }
        return Ok(Response {
            status,
            body: Vec::new(),
        });
    }
    let body = if chunked {
        read_chunked(reader)?
    } else if let Some(count) = length {
        let mut bytes = vec![0; count];
        reader.read_exact(&mut bytes).map_err(io_error)?;
        bytes
    } else {
        let mut bytes = Vec::new();
        let mut buffer = [0; 8192];
        loop {
            let count = reader.read(&mut buffer).map_err(io_error)?;
            if count == 0 {
                break;
            }
            if count > protocol::RESPONSE_BYTES.saturating_sub(bytes.len()) {
                return Err(Error("companion_response_limit"));
            }
            bytes.extend_from_slice(&buffer[..count]);
        }
        bytes
    };
    Ok(Response { status, body })
}

fn read_chunked<R: BufRead>(reader: &mut R) -> Result<Vec<u8>, Error> {
    let mut body = Vec::new();
    for _ in 0..8192 {
        let size = line(reader, &mut 128)?;
        let size = std::str::from_utf8(&size).map_err(|_| Error("companion_http_invalid"))?;
        if size.bytes().any(|b| b.is_ascii_control()) {
            return Err(Error("companion_http_invalid"));
        }
        let size = size.split(';').next().unwrap_or_default();
        if size.is_empty() || size.len() > 16 || !size.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(Error("companion_http_invalid"));
        }
        let size =
            usize::from_str_radix(size, 16).map_err(|_| Error("companion_response_limit"))?;
        if size == 0 {
            let mut trailer_budget = 16 * 1024;
            loop {
                let bytes = line(reader, &mut trailer_budget)?;
                if bytes.is_empty() {
                    return Ok(body);
                }
                let (name, _) = header(&bytes)?;
                if ["content-length", "transfer-encoding", "content-encoding"]
                    .iter()
                    .any(|key| name.eq_ignore_ascii_case(key))
                {
                    return Err(Error("companion_http_invalid"));
                }
            }
        }
        if size > protocol::RESPONSE_BYTES.saturating_sub(body.len()) {
            return Err(Error("companion_response_limit"));
        }
        let start = body.len();
        body.resize(start + size, 0);
        reader.read_exact(&mut body[start..]).map_err(io_error)?;
        let mut ending = [0; 2];
        reader.read_exact(&mut ending).map_err(io_error)?;
        if ending != *b"\r\n" {
            return Err(Error("companion_http_invalid"));
        }
    }
    Err(Error("companion_response_limit"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn parse(bytes: &[u8]) -> Result<Response, Error> {
        read_response(&mut BufReader::with_capacity(7, Cursor::new(bytes)))
    }

    #[test]
    fn cancellation_before_connect_does_not_contact_a_listener() {
        let client = Client::new(Endpoint::new(8793, "a".repeat(32)).unwrap());
        assert_eq!(
            client
                .execute_cancellable(&OffFrame::for_test(), &Request::Capabilities, &|| true)
                .err()
                .unwrap()
                .code(),
            "companion_cancelled"
        );
    }

    #[test]
    fn delete_sends_one_canonical_user_agent_between_host_and_accept() {
        plx_net::net::set_user_agent(plx_plex::plex::identity::user_agent()).unwrap();
        let canonical = plx_net::net::user_agent_header_line();
        assert!(!canonical.is_empty());
        let listener = std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        listener.set_nonblocking(true).unwrap();
        let port = listener.local_addr().unwrap().port();
        let peer = std::thread::spawn(move || {
            let worker = OffFrame::for_test();
            let budget = Budget::start(&worker, Duration::from_secs(2));
            let mut socket = loop {
                match listener.accept() {
                    Ok((socket, _)) => break socket,
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                        assert!(budget.wait(
                            &worker,
                            Duration::from_millis(5),
                            Duration::from_millis(5),
                            &|| false,
                        ));
                    }
                    Err(error) => panic!("loopback accept failed: {error}"),
                }
            };
            socket.set_nonblocking(false).unwrap();
            socket
                .set_write_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let mut request = Vec::new();
            while !request.ends_with(b"\r\n\r\n") {
                let remaining = budget.remaining(&worker);
                assert!(!remaining.is_zero(), "request deadline exceeded");
                socket.set_read_timeout(Some(remaining)).unwrap();
                let mut bytes = [0; 512];
                let count = socket.read(&mut bytes).unwrap();
                assert!(count > 0, "request ended before its headers");
                request.extend_from_slice(&bytes[..count]);
                assert!(request.len() <= 16 * 1024, "request headers exceeded limit");
            }
            socket
                .write_all(b"HTTP/1.1 204 No Content\r\nConnection: close\r\n\r\n")
                .unwrap();
            String::from_utf8(request).unwrap()
        });
        let endpoint = Endpoint::new(port, "a".repeat(32)).unwrap();
        let expected_path = endpoint.path("sessions/test-session");
        let reply = Client::new(endpoint).execute_cancellable(
            &OffFrame::for_test(),
            &Request::Delete {
                id: "test-session".into(),
            },
            &|| false,
        );
        let request = peer.join().unwrap();
        assert!(matches!(reply, Ok(Reply::Deleted)));
        let lines: Vec<_> = request.split("\r\n").collect();
        assert_eq!(lines[0], format!("DELETE {expected_path} HTTP/1.1"));
        let user_agents: Vec<_> = lines
            .iter()
            .copied()
            .filter(|line| {
                line.split_once(':')
                    .is_some_and(|(name, _)| name.eq_ignore_ascii_case("user-agent"))
            })
            .collect();
        assert_eq!(user_agents, [canonical.trim_end_matches("\r\n")]);
        let host = format!("Host: 127.0.0.1:{port}");
        assert!(lines.windows(3).any(|window| window
            == [
                host.as_str(),
                canonical.trim_end_matches("\r\n"),
                "Accept: application/json",
            ]));
        assert!(lines.contains(&"Content-Length: 0"));
    }

    #[test]
    fn stalled_socket_observes_cancellation_and_absolute_deadline() {
        for cancel in [true, false] {
            let listener = std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
            let address = listener.local_addr().unwrap();
            let (release, wait) = std::sync::mpsc::channel::<()>();
            let peer = std::thread::spawn(move || {
                let (socket, _) = listener.accept().unwrap();
                let _ = wait.recv_timeout(Duration::from_secs(3));
                drop(socket);
            });
            let socket = TcpStream::connect(address).unwrap();
            let worker = OffFrame::for_test();
            let budget = Budget::start(&worker, Duration::from_secs(2));
            let check = || cancel && budget.spent(&worker) >= Duration::from_millis(20);
            let deadline = Deadline::after(
                &worker,
                if cancel {
                    Duration::from_secs(2)
                } else {
                    Duration::from_millis(50)
                },
            );
            let mut stream = DeadlineStream {
                socket,
                deadline,
                cancelled: &check,
                worker: &worker,
            };
            let error = io_error(stream.read(&mut [0]).unwrap_err());
            let _ = release.send(());
            peer.join().unwrap();
            assert_eq!(
                error.code(),
                if cancel {
                    "companion_cancelled"
                } else {
                    "companion_timeout"
                }
            );
            assert!(budget.spent(&worker) < Duration::from_secs(2));
        }
    }

    #[test]
    fn fixed_length_and_no_content() {
        let response = parse(b"HTTP/1.1 200 OK\r\nContent-Length: 7\r\n\r\n{\"a\":1}").unwrap();
        assert_eq!(response.status, 200);
        assert_eq!(response.body, br#"{"a":1}"#);
        assert!(parse(b"HTTP/1.1 204 No Content\r\n\r\n")
            .unwrap()
            .body
            .is_empty());
    }

    #[test]
    fn chunked_with_trailer_and_close_delimited() {
        let response = parse(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n3;foo=bar\r\nabc\r\n2\r\nde\r\n0\r\nX-Status: done\r\n\r\n").unwrap();
        assert_eq!(response.body, b"abcde");
        assert_eq!(parse(b"HTTP/1.0 200 OK\r\n\r\nabc").unwrap().body, b"abc");
    }

    #[test]
    fn rejects_ambiguous_or_invalid_framing() {
        for input in [
            &b"HTTP/1.1 200 OK\r\nContent-Length: 1\r\nTransfer-Encoding: chunked\r\n\r\n"[..],
            b"HTTP/1.1 200 OK\r\nContent-Length: 1\r\nContent-Length: 1\r\n\r\na",
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: gzip, chunked\r\n\r\n",
            b"HTTP/1.1 200 OK\r\nContent-Encoding: gzip\r\n\r\n",
            b"HTTP/1.1 200 OK\nContent-Length: 0\n\n",
            b"HTTP/1.1 204 No Content\r\nContent-Length: 1\r\n\r\nx",
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n-1\r\n",
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n1\r\naXX0\r\n\r\n",
        ] {
            assert!(parse(input).is_err());
        }
    }

    #[test]
    fn rejects_truncated_and_oversized_responses() {
        for input in [
            &b"HTTP/1.1 200 OK\r\nContent-Length: 3\r\n\r\nab"[..],
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n3\r\nab",
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n0\r\n",
        ] {
            assert_eq!(
                parse(input).err().unwrap().code(),
                "companion_response_truncated"
            );
        }
        let big_length = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n",
            protocol::RESPONSE_BYTES + 1
        );
        assert_eq!(
            parse(big_length.as_bytes()).err().unwrap().code(),
            "companion_response_limit"
        );
        let long_header = format!("HTTP/1.1 200 OK\r\nX-Test: {}", "a".repeat(16 * 1024));
        assert_eq!(
            parse(long_header.as_bytes()).err().unwrap().code(),
            "companion_response_limit"
        );
        let big_chunk = format!(
            "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n{:x}\r\n",
            protocol::RESPONSE_BYTES + 1
        );
        assert_eq!(
            parse(big_chunk.as_bytes()).err().unwrap().code(),
            "companion_response_limit"
        );
    }
}
