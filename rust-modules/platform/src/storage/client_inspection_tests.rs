use super::*;
use std::io::{self, Cursor, Write};

struct Frames {
    input: Cursor<Vec<u8>>,
    output: Vec<u8>,
}
impl Read for Frames {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        self.input.read(bytes)
    }
}
impl Write for Frames {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.output.write(bytes)
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
fn descriptor() -> Descriptor {
    Descriptor {
        protocol: wire::PROTOCOL,
        nonce: "01".repeat(16),
        helper_generation: "02".repeat(16),
        socket: wire::SOCKET_NAME.into(),
        pid: std::process::id(),
        uid: unsafe { libc::getuid() },
    }
}
fn exchange(
    hello: Response,
    mode: ConnectionMode,
) -> (Result<Response, ClientError>, Vec<Request>) {
    let mut input = Vec::new();
    wire::write_frame(&mut input, &hello).unwrap();
    wire::write_frame(&mut input, &Response::OttLoaded { snapshot: None }).unwrap();
    let mut stream = Frames {
        input: Cursor::new(input),
        output: Vec::new(),
    };
    let result = exchange_frames(
        &mut stream,
        &descriptor(),
        Request::OttLoad {},
        Path::new("unused"),
        None,
        None,
        mode,
    );
    let mut output = stream.output.as_slice();
    let mut requests = Vec::new();
    while !output.is_empty() {
        requests.push(wire::read_frame(&mut output).unwrap());
    }
    (result, requests)
}
#[test]
fn inspection_requires_exact_nonce_generation_and_protocol_before_load() {
    for (protocol, nonce, generation) in [
        (wire::PROTOCOL + 1, "01", "02"),
        (wire::PROTOCOL, "03", "02"),
        (wire::PROTOCOL, "01", "04"),
    ] {
        let (result, requests) = exchange(
            Response::InspectHello {
                protocol,
                nonce: nonce.repeat(16),
                helper_generation: generation.repeat(16),
            },
            ConnectionMode::Inspect,
        );
        assert_eq!(result.unwrap_err(), ClientError::Unavailable);
        assert!(matches!(&requests[..], [Request::InspectHello { .. }]));
    }
}
#[test]
fn inspection_and_ordinary_handshakes_cannot_be_confused() {
    let ordinary = Response::Hello {
        protocol: wire::PROTOCOL,
        nonce: descriptor().nonce,
        helper_generation: descriptor().helper_generation,
        capabilities: wire::Capabilities {
            db8: true,
            keymanager: true,
        },
    };
    let (result, requests) = exchange(ordinary.clone(), ConnectionMode::Inspect);
    assert_eq!(result.unwrap_err(), ClientError::Unavailable);
    assert_eq!(requests.len(), 1);
    let (result, requests) = exchange(ordinary, ConnectionMode::Ordinary);
    assert!(matches!(
        result.unwrap(),
        Response::OttLoaded { snapshot: None }
    ));
    assert!(matches!(
        &requests[..],
        [Request::Hello { .. }, Request::OttLoad {}]
    ));
    let (result, requests) = exchange(
        Response::InspectHello {
            protocol: wire::PROTOCOL,
            nonce: descriptor().nonce,
            helper_generation: descriptor().helper_generation,
        },
        ConnectionMode::Inspect,
    );
    assert!(matches!(
        result.unwrap(),
        Response::OttLoaded { snapshot: None }
    ));
    assert!(matches!(
        &requests[..],
        [Request::InspectHello { .. }, Request::OttLoad {}]
    ));
}
#[test]
fn inspection_transport_rejects_other_operations_before_io() {
    let mut transport = ExistingTransport;
    for request in [
        Request::Load {},
        Request::OttStatus {},
        Request::OttReconcile {
            operation_id: "00".repeat(16),
            digest: "00".repeat(32),
        },
    ] {
        assert_eq!(
            transport.transact(request).unwrap_err(),
            ClientError::Invalid
        );
    }
}

#[cfg(target_os = "linux")]
#[test]
fn missing_inspection_root_never_creates_or_activates_it() {
    let root = plx_base::testscratch::process_dir("inspection-missing-root");
    let missing = root.join("not-created");
    let before = {
        let gate = START_GATE.lock().unwrap();
        (
            gate.failed,
            gate.last_hint,
            gate.activation,
            gate.attempt.as_ref().map(|a| a.nonce().to_owned()),
        )
    };
    assert_eq!(
        inspect_at(&missing, Request::OttLoad {}).unwrap_err(),
        ClientError::Unavailable
    );
    assert!(!missing.exists());
    let gate = START_GATE.lock().unwrap();
    assert_eq!(
        before,
        (
            gate.failed,
            gate.last_hint,
            gate.activation,
            gate.attempt.as_ref().map(|a| a.nonce().to_owned())
        )
    );
}
#[cfg(target_os = "linux")]
#[test]
fn legacy_only_listener_is_never_contacted_or_modified() {
    use std::os::unix::{fs::PermissionsExt, net::UnixListener};
    let root = plx_base::testscratch::process_dir("inspection-legacy-endpoint");
    std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
    let listener = UnixListener::bind(root.join(wire::SOCKET_NAME)).unwrap();
    listener.set_nonblocking(true).unwrap();
    for (name, bytes) in [
        (
            wire::DESCRIPTOR_NAME,
            serde_json::to_vec(&descriptor()).unwrap(),
        ),
        ("last-error", b"preserved".to_vec()),
    ] {
        let path = root.join(name);
        std::fs::write(&path, bytes).unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    std::fs::set_permissions(
        root.join(wire::SOCKET_NAME),
        std::fs::Permissions::from_mode(0o600),
    )
    .unwrap();
    assert_eq!(
        inspect_at(&root, Request::OttLoad {}).unwrap_err(),
        ClientError::Unavailable
    );
    assert_eq!(
        listener.accept().unwrap_err().kind(),
        io::ErrorKind::WouldBlock
    );
    assert_eq!(
        std::fs::read(root.join("last-error")).unwrap(),
        b"preserved"
    );
    assert!(!root.join("activation.json").exists());
    assert!(!root.join(wire::INSPECT_SOCKET_NAME).exists());
    assert_eq!(std::fs::read_dir(&root).unwrap().count(), 3);
}

#[test]
fn fatal_helper_eof_is_unavailable_but_malformed_frames_remain_protocol_errors() {
    for valid_hello in [false, true] {
        let mut input = Vec::new();
        if valid_hello {
            wire::write_frame(&mut input, &Response::InspectHello {
                protocol: wire::PROTOCOL, nonce: descriptor().nonce,
                helper_generation: descriptor().helper_generation,
            }).unwrap();
        }
        let mut stream = Frames { input: Cursor::new(input), output: Vec::new() };
        assert_eq!(exchange_frames(&mut stream, &descriptor(), Request::OttLoad {}, Path::new("unused"),
            None, None, ConnectionMode::Inspect).unwrap_err(), ClientError::Unavailable);
    }
    let mut malformed = Frames { input: Cursor::new(vec![0,0,0,1,b'{']), output: Vec::new() };
    assert_eq!(exchange_frames(&mut malformed, &descriptor(), Request::OttLoad {}, Path::new("unused"),
        None, None, ConnectionMode::Inspect).unwrap_err(), ClientError::Protocol);
}

#[cfg(target_os = "linux")]
#[test]
fn full_inspection_backlog_returns_without_blocking_connect() {
    use std::os::unix::fs::DirBuilderExt;
    use std::os::unix::net::UnixListener;
    struct Root(std::path::PathBuf);
    impl Drop for Root {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    let root = Root(
        std::env::temp_dir().join(format!(
            "inspect-backlog-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        )),
    );
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(&root.0)
        .unwrap();
    let path = root.0.join("inspect.sock");
    let listener = UnixListener::bind(&path).unwrap();
    assert_eq!(unsafe { libc::listen(listener.as_raw_fd(), 0) }, 0);
    let mut clients = Vec::new();
    let mut full = false;
    for _ in 0..32 {
        match connect_inspection_socket(&path, Instant::now() + Duration::from_millis(200)) {
            Ok(stream) => clients.push(stream),
            Err(error) => {
                assert_eq!(error.kind(), io::ErrorKind::WouldBlock);
                full = true;
                break;
            }
        }
    }
    assert!(
        full,
        "the fixture must actually reach a saturated accept queue"
    );
    assert!(!clients.is_empty());
}
