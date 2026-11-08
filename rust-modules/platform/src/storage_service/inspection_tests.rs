use super::super::Flavor;
use super::*;
use serde_json::{json, Value};
use std::io::Cursor;

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
struct ReadsOnly {
    calls: Vec<String>,
    fail: bool,
}
impl Rpc for ReadsOnly {
    fn call(&mut self, uri: &str, _: &Value) -> Result<Value, ErrorCode> {
        self.calls.push(uri.into());
        assert_eq!(
            uri, "luna://com.palm.db/get",
            "inspection attempted a mutation/probe"
        );
        if self.fail {
            Err(ErrorCode::Unavailable)
        } else {
            Ok(json!({"returnValue":true,"results":[]}))
        }
    }
}
fn descriptor() -> Descriptor {
    Descriptor {
        protocol: wire::PROTOCOL,
        nonce: "01".repeat(16),
        helper_generation: "02".repeat(16),
        socket: wire::SOCKET_NAME.into(),
        pid: 1,
        uid: 1,
    }
}
fn run(requests: &[Request], fail: bool) -> (Vec<Response>, Vec<String>) {
    let mut bytes = Vec::new();
    for request in requests {
        wire::write_frame(&mut bytes, request).unwrap();
    }
    let mut stream = Frames {
        input: Cursor::new(bytes),
        output: Vec::new(),
    };
    let mut backend = Backend::new(
        ReadsOnly {
            calls: Vec::new(),
            fail,
        },
        Flavor::Debug,
        "synthetic.storage".into(),
    );
    serve(&mut stream, &descriptor(), &mut backend, std::time::Instant::now() + std::time::Duration::from_secs(8)).unwrap();
    let mut output = stream.output.as_slice();
    let mut replies = Vec::new();
    while !output.is_empty() {
        replies.push(wire::read_frame(&mut output).unwrap());
    }
    (replies, backend.rpc.calls)
}
fn hello() -> Request {
    Request::InspectHello {
        protocol: wire::PROTOCOL,
        nonce: descriptor().nonce,
    }
}

#[test]
fn inspection_missing_read_is_one_get_without_setup_or_key_probe() {
    let (replies, calls) = run(&[hello(), Request::OttLoad {}], false);
    assert!(
        matches!(&replies[..], [Response::InspectHello { nonce, helper_generation, .. }, Response::OttLoaded { snapshot: None }]
        if nonce == &descriptor().nonce && helper_generation == &descriptor().helper_generation)
    );
    assert_eq!(calls, ["luna://com.palm.db/get"]);
}
#[test]
fn inspection_rejects_wrong_nonce_protocol_and_ordinary_hello_before_backend() {
    for request in [
        Request::InspectHello {
            protocol: wire::PROTOCOL + 1,
            nonce: descriptor().nonce,
        },
        Request::InspectHello {
            protocol: wire::PROTOCOL,
            nonce: "03".repeat(16),
        },
        Request::Hello {
            protocol: wire::PROTOCOL,
            nonce: descriptor().nonce,
        },
        Request::OttLoad {},
    ] {
        let (replies, calls) = run(&[request], false);
        assert!(matches!(
            &replies[..],
            [Response::Error {
                code: ErrorCode::Protocol
            }]
        ));
        assert!(calls.is_empty());
    }
}
#[test]
fn inspection_rejects_writes_reconciliation_and_other_namespace_reads() {
    for request in [
        Request::OttCommit {
            expected: wire::ott::Expected::Missing {},
            operation_id: "00".repeat(16),
            digest: "00".repeat(32),
            mutation: wire::ott::Mutation::BeginErase {},
        },
        Request::OttReconcile {
            operation_id: "00".repeat(16),
            digest: "00".repeat(32),
        },
        Request::Load {},
        Request::OttControlLoad {},
        Request::OttStateCommit { expected:wire::ott::Expected::Missing {}, operation_id:"01".repeat(16),
            digest:"02".repeat(32), source:wire::saved::Source::Absent {},
            value:wire::saved::Document::try_from(json!({})).unwrap() },
        Request::OttStatus {},
    ] {
        let (replies, calls) = run(&[hello(), request], false);
        assert!(matches!(
            &replies[..],
            [
                Response::InspectHello { .. },
                Response::Error {
                    code: ErrorCode::Protocol
                }
            ]
        ));
        assert!(calls.is_empty());
    }
}
#[test]
fn inspection_read_failure_is_unavailable_never_missing() {
    let (replies, calls) = run(&[hello(), Request::OttLoad {}], true);
    assert!(matches!(
        &replies[..],
        [
            Response::InspectHello { .. },
            Response::Error {
                code: ErrorCode::Unavailable
            }
        ]
    ));
    assert_eq!(calls, ["luna://com.palm.db/get"]);
}
#[test]
fn inspection_invalid_frame_never_reaches_backend() {
    let mut backend = Backend::new(
        ReadsOnly {
            calls: Vec::new(),
            fail: false,
        },
        Flavor::Debug,
        "synthetic.storage".into(),
    );
    let mut stream = Frames {
        input: Cursor::new(vec![0, 0, 0, 1, b'{']),
        output: Vec::new(),
    };
    serve(&mut stream, &descriptor(), &mut backend, std::time::Instant::now() + std::time::Duration::from_secs(8)).unwrap();
    assert!(matches!(
        wire::read_frame::<Response>(&mut stream.output.as_slice()).unwrap(),
        Response::Error {
            code: ErrorCode::Protocol
        }
    ));
    assert!(backend.rpc.calls.is_empty());
}

#[test]
fn inspection_success_preserves_existing_diagnostics() {
    wire::failure::remember(wire::failure::Stage::ActivationSent, Some(17));
    let before = wire::failure::last();
    let (replies, _) = run(&[hello(), Request::OttLoad {}], false);
    assert!(matches!(
        &replies[..],
        [
            Response::InspectHello { .. },
            Response::OttLoaded { snapshot: None }
        ]
    ));
    assert_eq!(wire::failure::last(), before);
}

#[test]
fn expired_inspection_never_starts_a_backend_read() {
    let mut input = Vec::new();
    wire::write_frame(&mut input, &hello()).unwrap();
    wire::write_frame(&mut input, &Request::OttLoad {}).unwrap();
    let mut stream = Frames { input: Cursor::new(input), output: Vec::new() };
    let mut backend = Backend::new(ReadsOnly { calls: Vec::new(), fail: false },
        Flavor::Debug, "synthetic.storage".into());
    let error = serve(&mut stream, &descriptor(), &mut backend, std::time::Instant::now()).unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::TimedOut);
    assert!(backend.rpc.calls.is_empty());
    assert!(stream.output.is_empty());
}

#[test]
fn inspection_typed_state_missing_is_one_get_without_setup_or_key_probe() {
    let (replies, calls) = run(&[hello(), Request::OttStateLoad {}], false);
    assert!(matches!(&replies[..], [Response::InspectHello { .. },
        Response::OttStateLoaded { status:None, value:wire::saved::Loaded::Missing {} }]));
    assert_eq!(calls, ["luna://com.palm.db/get"]);
    let (replies, calls) = run(&[hello(), Request::OttStateLoad {}], true);
    assert!(matches!(&replies[..], [Response::InspectHello { .. },
        Response::Error { code:ErrorCode::Unavailable }]));
    assert_eq!(calls, ["luna://com.palm.db/get"]);
}
