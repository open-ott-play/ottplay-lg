//! Application side of the private storage-helper protocol.
//!
//! A missing helper, a stale rendezvous, a refused activation and a corrupt canonical object are
//! all unavailable storage — never an empty database.  Only the helper may decide that DB8 has no
//! canonical object, after an authenticated `Hello` on the same stream as the `Load` reply.
//! A helper that never connects gets one startup window. After that expires, each call tries
//! once and returns unavailable immediately on failure, with activation hints rate-limited.
//! A successful connection restores the normal startup policy.

use super::{
    state::{self, CanonicalState, Expected, Flavor, Generation},
    wire::{
        self, AuthLoad, Descriptor, ErrorCode, Expectation, ProtectionOutcome, ReconcileStatus,
        Request, Response, WireMutation,
    },
};
#[allow(unused_imports)]
// Unix transport is Linux-only; protocol coordinator is host-testable.
use std::{
    fs::OpenOptions,
    io::Read,
    os::{
        fd::AsRawFd,
        unix::{
            fs::{FileTypeExt, MetadataExt, OpenOptionsExt},
            net::UnixStream,
        },
    },
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use wire::failure::{self, Stage};

#[path = "client_ott.rs"]
pub mod ott;

const START_DEADLINE: Duration = Duration::from_secs(12);
const IO_DEADLINE: Duration = Duration::from_secs(8);
const DESCRIPTOR_MAX: u64 = 4096;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClientError {
    Unavailable,
    Authentication,
    Protocol,
    Corrupt,
    Invalid,
}

#[derive(Clone)]
pub struct Snapshot {
    pub db_rev: String,
    pub state: CanonicalState,
    pub auth: AuthLoad,
    pub protection: Option<ProtectionOutcome>,
}

impl std::fmt::Debug for Snapshot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("StorageSnapshot { <redacted> }")
    }
}

pub enum Load {
    Missing,
    Present(Snapshot),
}

impl std::fmt::Debug for Load {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Missing => f.write_str("Missing"),
            Self::Present(_) => f.write_str("Present(<redacted>)"),
        }
    }
}

pub fn flavor() -> Result<Flavor, ClientError> {
    Flavor::from_app_id(plx_base::paths::app_id()).ok_or(ClientError::Invalid)
}

fn service_name() -> String {
    format!("{}.storage", plx_base::paths::app_id())
}

fn runtime_path() -> PathBuf {
    PathBuf::from(format!("/tmp/{}.storage-runtime", plx_base::paths::app_id()))
}

pub(super) fn generation_text(generation: Generation) -> String {
    generation
        .0
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn wire_expectation(expected: Option<(&str, Expected)>) -> Expectation {
    match expected {
        None => Expectation::Missing {},
        Some((db_rev, expected)) => Expectation::Present {
            db_rev: db_rev.to_string(),
            epoch: expected.epoch.to_string(),
            auth_generation: generation_text(expected.auth_generation),
        },
    }
}

fn failed(stage: Stage, errno: Option<i32>, error: ClientError) -> ClientError {
    let activation = failure::last().and_then(|f| f.activation);
    failure::remember(stage, errno);
    failure::update(|f| f.activation = activation);
    error
}

fn io_failed(stage: Stage, error: std::io::Error) -> ClientError {
    failed(stage, error.raw_os_error(), ClientError::Unavailable)
}

pub(super) fn error(code: ErrorCode) -> ClientError {
    if !failure::last().is_some_and(|f| matches!(f.observed.stage, Stage::BackendRejected | Stage::HelloRejected)) {
        failed(Stage::BackendRejected, None, classify_error(code));
        failure::update(|f| f.wire = Some(code));
    }
    classify_error(code)
}

fn classify_error(code: ErrorCode) -> ClientError {
    match code {
        ErrorCode::Authentication => ClientError::Authentication,
        ErrorCode::Protocol => ClientError::Protocol,
        ErrorCode::Corrupt => ClientError::Corrupt,
        ErrorCode::Invalid => ClientError::Invalid,
        ErrorCode::Unavailable | ErrorCode::Timeout | ErrorCode::Capability => {
            ClientError::Unavailable
        }
    }
}

/// Load through an authenticated helper. `Ok(Missing)` is therefore authoritative.
pub fn load() -> Result<Load, ClientError> {
    load_with(&mut NativeTransport)
}

/// Explicit diagnostic only: one connection to an already running helper, no activation/retry.
/// This first-frame operation bypasses ordinary Hello's DB8 setup and capability probing.
pub fn probe_keymanager() -> Result<wire::CanaryResult, ClientError> {
    let _block = plx_base::task::assert_may_block(const { &plx_base::task::BlockingLabel::new("storage keymanager probe") });
    #[cfg(target_os = "linux")]
    {
        let deadline = Instant::now() + IO_DEADLINE;
        let (stream, descriptor) = connect_to_socket(&runtime_path(), ConnectionMode::Ordinary, Some(deadline))?;
        let mut stream = wire::inspection_io::Stream::new(stream, deadline)
            .map_err(|e| inspection_failure(Stage::SocketTimeout, e))?;
        let result = probe_frames(&mut stream, &descriptor);
        wire::inspection_io::check(deadline).map_err(|e| inspection_failure(Stage::SocketTimeout, e))?;
        result
    }
    #[cfg(not(target_os = "linux"))]
    {
        Err(ClientError::Unavailable)
    }
}

#[cfg(any(target_os = "linux", test))]
fn probe_frames(
    stream: &mut (impl Read + std::io::Write), descriptor: &Descriptor,
) -> Result<wire::CanaryResult, ClientError> {
    wire::write_frame(stream, &Request::KeymanagerProbe {
        protocol: wire::PROTOCOL, nonce: descriptor.nonce.clone(),
    }).map_err(|e| inspection_failure(Stage::Wire, e))?;
    match wire::read_frame::<Response>(stream).map_err(|e| inspection_failure(Stage::Wire, e))? {
        Response::KeymanagerProbe { protocol, nonce, helper_generation, result }
            if protocol == wire::PROTOCOL && nonce == descriptor.nonce
                && helper_generation == descriptor.helper_generation => Ok(result),
        Response::Error { code } => Err(classify_error(code)),
        _ => Err(ClientError::Protocol),
    }
}

/// Injectable external I/O boundary, shared by the shipping coordinator and host fault tests.
pub trait Transport {
    fn transact(&mut self, request: Request) -> Result<Response, ClientError>;
}
impl<F: FnMut(Request) -> Result<Response, ClientError>> Transport for F {
    fn transact(&mut self, request: Request) -> Result<Response, ClientError> {
        self(request)
    }
}
/// Inspection must never activate the helper or use its mutating Hello endpoint.
pub(super) struct ExistingTransport;
impl Transport for ExistingTransport {
    fn transact(&mut self, request: Request) -> Result<Response, ClientError> {
        if !matches!(request, Request::OttLoad {} | Request::OttStateLoad {}) {
            return Err(ClientError::Invalid);
        }
        transact_existing(request)
    }
}

#[cfg(any(target_os = "linux", test))]
#[derive(Clone, Copy)]
enum ConnectionMode { Ordinary, Inspect }

pub struct NativeTransport;
impl Transport for NativeTransport {
    fn transact(&mut self, request: Request) -> Result<Response, ClientError> {
        transact(request)
    }
}

pub fn load_with(transport: &mut dyn Transport) -> Result<Load, ClientError> {
    match transport.transact(Request::Load {})? {
        Response::Missing => Ok(Load::Missing),
        Response::Loaded {
            db_rev,
            state: value,
            auth,
            protection,
        } => {
            let bytes = serde_json::to_vec(&value).map_err(|_| ClientError::Corrupt)?;
            let state =
                CanonicalState::decode(&bytes, flavor()?).map_err(|_| ClientError::Corrupt)?;
            if db_rev.parse::<u64>().ok().map(|value| value.to_string()) != Some(db_rev.clone()) {
                return Err(ClientError::Corrupt);
            }
            Ok(Load::Present(Snapshot {
                db_rev,
                state,
                auth,
                protection,
            }))
        }
        Response::Error { code } => Err(error(code)),
        _ => Err(ClientError::Protocol),
    }
}

/// Submit one typed mutation. The digest covers the plaintext request, before the helper seals it.
pub fn commit(
    expected: Option<(&str, Expected)>,
    operation_id: Generation,
    mutation: WireMutation,
) -> Result<Response, ClientError> {
    commit_with(&mut NativeTransport, expected, operation_id, mutation)
}

pub fn commit_with(
    transport: &mut dyn Transport,
    expected: Option<(&str, Expected)>,
    operation_id: Generation,
    mutation: WireMutation,
) -> Result<Response, ClientError> {
    let mut request = Request::Commit {
        expected: wire_expectation(expected),
        operation_id: generation_text(operation_id),
        digest: String::new(),
        mutation,
    };
    let digest = state::digest_bytes(&wire::request_digest_bytes(&request).map_err(|code| failed(Stage::RequestDigest, None, classify_error(code)))?);
    let Request::Commit {
        digest: request_digest,
        ..
    } = &mut request
    else {
        unreachable!()
    };
    *request_digest = digest.clone();
    match transport.transact(request.clone()) {
        Ok(response) => Ok(response),
        Err(first @ (ClientError::Unavailable | ClientError::Protocol)) => {
            match transport.transact(Request::Reconcile {
                operation_id: generation_text(operation_id),
                digest,
            })? {
                response @ Response::Reconcile {
                    status: ReconcileStatus::Applied,
                    ..
                } => Ok(response),
                Response::Reconcile {
                    status: ReconcileStatus::NotApplied,
                    ..
                } => transport.transact(request),
                response @ Response::Reconcile {
                    status: ReconcileStatus::Unknown,
                    ..
                } => Ok(response),
                Response::Reconcile {
                    status: ReconcileStatus::Invalid,
                    ..
                } => Err(ClientError::Invalid),
                Response::Error { code } => Err(error(code)),
                response @ Response::KeymanagerError { .. } => Ok(response),
                _ => Err(first),
            }
        }
        Err(error) => Err(error),
    }
}

pub fn reconcile(operation_id: Generation, digest: String) -> Result<Response, ClientError> {
    transact(Request::Reconcile {
        operation_id: generation_text(operation_id),
        digest,
    })
}

/// Stage B supplies the application resource adapter's nonblocking LS2 activation hint.
/// An absent hint never becomes permission to load a legacy file.
static ACTIVATION_HINT: std::sync::OnceLock<fn(&str)> = std::sync::OnceLock::new();
pub fn install_activation_hint(hint: fn(&str)) -> Result<(), fn(&str)> {
    ACTIVATION_HINT.set(hint)
}

/// The platform's way to wake the helper, installed by the port at boot. With none installed (a
/// host, the simulator) activation is [`Stage::Unsupported`].
#[cfg(target_os = "linux")]
static ACTIVATOR: std::sync::OnceLock<fn(&str) -> failure::Detail> = std::sync::OnceLock::new();
#[cfg(target_os = "linux")]
pub fn install_activator(
    activator: fn(&str) -> failure::Detail,
) -> Result<(), fn(&str) -> failure::Detail> {
    ACTIVATOR.set(activator)
}

/// The helper is a dynamic LS2 service: nothing starts it but a call to it (0.6.6's
/// `activate_storage_helper`). Without this a television with no running helper never answers.
#[cfg(target_os = "linux")]
fn activate(service: &str) -> failure::Detail {
    match ACTIVATOR.get() {
        Some(activator) => activator(service),
        None => failure::Detail::new(Stage::Unsupported, None),
    }
}

#[cfg(not(target_os = "linux"))]
fn transact(_command: Request) -> Result<Response, ClientError> {
    let _block = plx_base::task::assert_may_block(const { &plx_base::task::BlockingLabel::new("storage helper transact") });
    failure::clear();
    let _report = TransactionReport::new();
    Err(failed(Stage::Unsupported, None, ClientError::Unavailable))
}

#[cfg(not(target_os = "linux"))]
fn transact_existing(command: Request) -> Result<Response, ClientError> {
    // The host path is already unsupported and performs no activation or filesystem IO.
    transact(command)
}

#[cfg(target_os = "linux")]
fn transact_existing(command: Request) -> Result<Response, ClientError> {
    let _block = plx_base::task::assert_may_block(const { &plx_base::task::BlockingLabel::new("storage helper inspect") });
    inspect_at(&runtime_path(), command)
}

#[cfg(target_os = "linux")]
fn inspect_at(root: &Path, command: Request) -> Result<Response, ClientError> {
    failure::clear();
    let mut report = TransactionReport::new();
    report.attempted();
    let deadline = Instant::now() + IO_DEADLINE;
    let (stream, descriptor) = connect_to(root, Some(deadline))?;
    let mut stream = wire::inspection_io::Stream::new(stream, deadline)
        .map_err(|e| inspection_failure(Stage::SocketTimeout, e))?;
    let result = exchange_frames(&mut stream, &descriptor, command, root, None, None, ConnectionMode::Inspect);
    wire::inspection_io::check(deadline).map_err(|e| inspection_failure(Stage::SocketTimeout, e))?;
    result
}

struct TransactionReport {
    started: Instant,
    attempts: u32,
}

impl TransactionReport {
    fn new() -> Self { Self { started: Instant::now(), attempts: 0 } }
    fn attempted(&mut self) { self.attempts = self.attempts.saturating_add(1); }
}

impl Drop for TransactionReport {
    fn drop(&mut self) {
        crate::storage::diagnostics::helper(
            failure::last(), self.attempts,
            self.started.elapsed().as_millis().min(u64::MAX as u128) as u64,
        );
    }
}

/// Startup wait policy, kept separate from sockets so deadline and recovery are host-testable.
#[derive(Default)]
struct StartGate {
    failed: bool,
    last_hint: Option<Instant>,
    activation: Option<failure::Detail>,
    attempt: Option<wire::activation::Attempt>,
}

impl StartGate {
    /// Called only after a connect attempt: a failed start never prevents trying a live helper.
    fn wait_after_failed_connect(&mut self, now: Instant, until: Instant) -> bool {
        self.failed |= now >= until;
        !self.failed
    }

    /// Re-hint at most once per startup window, even if several callers keep trying storage.
    fn hint_due(&mut self, now: Instant) -> bool {
        if self
            .last_hint
            .is_some_and(|last| now.saturating_duration_since(last) < START_DEADLINE)
        {
            return false;
        }
        self.last_hint = Some(now);
        self.activation = None;
        self.attempt = None;
        true
    }

    fn connected(&mut self) {
        *self = Self::default();
    }
}

/// Shared by loads, commits and reconciliation. Never held across I/O, activation or sleep.
static START_GATE: std::sync::Mutex<StartGate> = std::sync::Mutex::new(StartGate {
    failed: false,
    last_hint: None,
    activation: None,
    attempt: None,
});

#[cfg(target_os = "linux")]
fn transact(command: Request) -> Result<Response, ClientError> {
    let _block = plx_base::task::assert_may_block(const { &plx_base::task::BlockingLabel::new("storage helper transact") });
    failure::clear();
    let mut report = TransactionReport::new();
    let until = Instant::now() + START_DEADLINE;
    let mut hinted = false;
    loop {
        report.attempted();
        let connected = connect(&runtime_path());
        if let Ok((stream, descriptor)) = connected {
            let (activation, attempt) = {
                let mut gate = START_GATE.lock().unwrap_or_else(|e| e.into_inner());
                let activation = gate.activation;
                let attempt = gate.attempt.clone();
                gate.connected();
                (activation, attempt)
            };
            return exchange_connected(
                stream, descriptor, command, &runtime_path(), activation, attempt, ConnectionMode::Ordinary,
            );
        }
        let (wait, hint, hint_time) = {
            let mut gate = START_GATE.lock().unwrap_or_else(|e| e.into_inner());
            let now = Instant::now();
            let hint = !hinted && gate.hint_due(now);
            (gate.wait_after_failed_connect(now, until), hint, now)
        };
        if hint {
            let activate_helper = move || {
                let attempt = wire::activation::Attempt::begin(&runtime_path());
                {
                    let mut gate = START_GATE.lock().unwrap_or_else(|e| e.into_inner());
                    if gate.last_hint != Some(hint_time) { return; }
                    gate.attempt = attempt;
                }
                let detail = match ACTIVATION_HINT.get() {
                    Some(hint) => {
                        let started = Instant::now();
                        hint(&service_name());
                        let detail = failure::Detail::new(Stage::ActivationSent, None);
                        crate::storage::diagnostics::activation(
                            detail, started.elapsed().as_millis().min(u64::MAX as u128) as u64,
                        );
                        detail
                    }
                    None => activate(&service_name()),
                };
                let mut gate = START_GATE.lock().unwrap_or_else(|e| e.into_inner());
                if gate.last_hint == Some(hint_time) { gate.activation = Some(detail); }
            };
            if wait {
                activate_helper();
            } else {
                // The native fallback hint itself waits for LS2. A memoized failure must
                // return immediately even when that hint needs another platform round trip.
                let _ = plx_base::storage_worker::submit(activate_helper);
            }
            hinted = true;
        }
        if !wait {
            // Keep the connect/authentication/protocol observation beside the historical
            // control-flow value. Never turn a refused peer into an empty database.
            let error = start_unavailable(connected.err().unwrap());
            let (activation, attempt) = {
                let gate = START_GATE.lock().unwrap_or_else(|e| e.into_inner());
                (gate.activation, gate.attempt.clone())
            };
            failure::update(|f| f.activation = activation);
            read_helper_failure(&runtime_path(), None, attempt.as_ref().map(|a| a.nonce()));
            return Err(error);
        }
        std::thread::sleep(Duration::from_millis(25));
    }
}

/// Both connection policies use exactly the same framed authentication exchange.
#[cfg(target_os = "linux")]
fn exchange_connected(
    mut stream: UnixStream,
    descriptor: Descriptor,
    command: Request,
    root: &Path,
    activation: Option<failure::Detail>,
    attempt: Option<wire::activation::Attempt>,
    mode: ConnectionMode,
) -> Result<Response, ClientError> {
    failure::clear();
    failure::remember(Stage::HelloRejected, None);
    failure::update(|f| f.activation = activation);
    stream.set_read_timeout(Some(IO_DEADLINE)).map_err(|e| io_failed(Stage::SocketTimeout, e))?;
    stream.set_write_timeout(Some(IO_DEADLINE)).map_err(|e| io_failed(Stage::SocketTimeout, e))?;
    exchange_frames(&mut stream, &descriptor, command, root, activation, attempt.as_ref(), mode)
}

#[cfg(any(target_os = "linux", test))]
fn exchange_frames(
    stream: &mut (impl Read + std::io::Write),
    descriptor: &Descriptor,
    command: Request,
    root: &Path,
    activation: Option<failure::Detail>,
    attempt: Option<&wire::activation::Attempt>,
    mode: ConnectionMode,
) -> Result<Response, ClientError> {
    failure::clear();
    failure::remember(Stage::HelloRejected, None);
    failure::update(|f| f.activation = activation);
    let hello = match mode {
        ConnectionMode::Ordinary => Request::Hello { protocol: wire::PROTOCOL, nonce: descriptor.nonce.clone() },
        ConnectionMode::Inspect => Request::InspectHello { protocol: wire::PROTOCOL, nonce: descriptor.nonce.clone() },
    };
    wire::write_frame(stream, &hello).map_err(|cause| {
        let error = frame_failure(mode, Stage::Wire, cause);
        read_helper_failure(root, None, attempt.map(|a| a.nonce()));
        error
    })?;
    let hello = wire::read_frame::<Response>(stream).map_err(|cause| {
        let error = frame_failure(mode, Stage::HelloRejected, cause);
        read_helper_failure(root, None, attempt.map(|a| a.nonce()));
        error
    })?;
    let generation = match &hello {
        Response::Hello { protocol, nonce, helper_generation, .. }
        | Response::InspectHello { protocol, nonce, helper_generation }
            if *protocol == wire::PROTOCOL && *nonce == descriptor.nonce
                && *helper_generation == descriptor.helper_generation => Some(helper_generation.as_str()),
        _ => None,
    };
    let validated = match mode {
        ConnectionMode::Ordinary => validate_hello(hello.clone(), descriptor),
        ConnectionMode::Inspect => validate_inspect_hello(hello.clone(), descriptor),
    };
    if let Err(error) = validated {
        if matches!(mode, ConnectionMode::Ordinary) {
            read_helper_failure(root, generation, attempt.map(|a| a.nonce()));
        }
        return Err(error);
    }
    wire::write_frame(stream, &command).map_err(|cause| frame_failure(mode, Stage::Wire, cause))?;
    match mode {
        ConnectionMode::Ordinary => read_response(stream, root, &descriptor.helper_generation),
        ConnectionMode::Inspect => {
            // Do not consume or alter diagnostic files for inspection failures.
            let response = wire::read_frame(stream)
                .map_err(|cause| inspection_failure(Stage::Wire, cause))?;
            failure::clear();
            Ok(response)
        }
    }
}

#[cfg(any(target_os = "linux", test))]
fn inspection_failure(stage: Stage, cause: std::io::Error) -> ClientError {
    let class = if matches!(cause.kind(), std::io::ErrorKind::InvalidData | std::io::ErrorKind::InvalidInput) {
        ClientError::Protocol
    } else {
        ClientError::Unavailable
    };
    failed(stage, cause.raw_os_error(), class)
}

#[cfg(any(target_os = "linux", test))]
fn frame_failure(mode: ConnectionMode, stage: Stage, cause: std::io::Error) -> ClientError {
    match mode {
        ConnectionMode::Ordinary => failed(stage, None, ClientError::Protocol),
        ConnectionMode::Inspect => inspection_failure(stage, cause),
    }
}

#[cfg(any(target_os = "linux", test))]
fn validate_inspect_hello(response: Response, descriptor: &Descriptor) -> Result<(), ClientError> {
    match response {
        Response::InspectHello { protocol, nonce, helper_generation }
            if protocol == wire::PROTOCOL && nonce == descriptor.nonce
                && helper_generation == descriptor.helper_generation => Ok(()),
        Response::Error { code } => Err(error(code)),
        _ => Err(failed(Stage::HelloRejected, None, ClientError::Unavailable)),
    }
}

/// Decode and classify the ordinary command reply; shared with host regression tests.
fn read_response(stream: &mut impl Read, root: &Path, generation: &str) -> Result<Response, ClientError> {
    let response = wire::read_frame::<Response>(stream).map_err(|_| failed(Stage::Wire, None, ClientError::Protocol))?;
    if let Some(code) = response.failure_code() {
        let stage = match response {
            Response::Commit { .. } | Response::Reconcile { .. }
                | Response::OttCommit { .. } | Response::OttReconcile { .. }
                | Response::OttControlCommit { .. } | Response::OttStateCommit { .. } => Stage::BackendUncertain,
            _ => Stage::BackendRejected,
        };
        failed(stage, None, classify_error(code));
        failure::update(|f| f.wire = Some(code));
        read_helper_failure(root, Some(generation), None);
    } else {
        failure::clear();
    }
    Ok(response)
}

fn validate_hello(response: Response, descriptor: &Descriptor) -> Result<(), ClientError> {
    match response {
        Response::Hello { protocol, nonce, helper_generation, capabilities }
            if protocol == wire::PROTOCOL && nonce == descriptor.nonce
                && helper_generation == descriptor.helper_generation && capabilities.db8 => Ok(()),
        Response::Error { code } => {
            let error = failed(Stage::HelloRejected, None, classify_error(code));
            failure::update(|f| f.wire = Some(code));
            Err(error)
        }
        _ => Err(failed(Stage::HelloRejected, None, ClientError::Protocol)),
    }
}

fn peer_rejection(errno: Option<i32>, valid_length: bool, uid_matches: bool, valid_pid: bool) -> Option<failure::Detail> {
    if errno.is_some() || !valid_length || !valid_pid {
        Some(failure::Detail::new(Stage::PeerCredentials, errno))
    } else if !uid_matches {
        Some(failure::Detail::new(Stage::PeerUidMismatch, None))
    } else { None }
}

fn start_unavailable(error: ClientError) -> ClientError {
    if failure::last().is_none() {
        let stage = match error {
            ClientError::Authentication => Stage::PeerCredentials,
            ClientError::Protocol => Stage::DescriptorInvalid,
            _ => Stage::Connect,
        };
        failure::remember(stage, None);
    }
    failure::update(|f| f.start_timeout = true);
    ClientError::Unavailable
}

/// Read only a bounded regular file under the same private directory rules as rendezvous.
/// Consume only records bound to the generation established by a matching Hello.
/// Before Hello, only a failure echoing the current activation nonce can enrich the observation.
fn read_helper_failure(root: &Path, generation: Option<&str>, activation_nonce: Option<&str>) {
    let Some(generation) = generation else {
        if let Some(detail) = activation_nonce.and_then(|nonce| wire::activation::read_failure(root, nonce)) {
            failure::update(|f| f.helper = Some(detail));
        }
        return;
    };
    let Ok(directory) = OpenOptions::new().read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC).open(root) else { return; };
    let Ok(meta) = directory.metadata() else { return; };
    if !meta.is_dir() || meta.uid() != unsafe { libc::getuid() } || meta.mode() & 0o7777 != 0o700 { return; }
    #[cfg(target_os = "linux")]
    let path = PathBuf::from(format!("/proc/self/fd/{}", directory.as_raw_fd())).join("last-error");
    #[cfg(not(target_os = "linux"))]
    let path = root.join("last-error");
    let Ok(mut file) = OpenOptions::new().read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK).open(path) else { return; };
    let Ok(meta) = file.metadata() else { return; };
    if !meta.is_file() || meta.uid() != unsafe { libc::getuid() } || meta.mode() & 0o7777 != 0o600
        || meta.nlink() != 1 || meta.len() > DESCRIPTOR_MAX { return; }
    let mut bytes = Vec::new();
    if Read::by_ref(&mut file).take(DESCRIPTOR_MAX + 1).read_to_end(&mut bytes).is_err() { return; }
    if let Some(detail) = failure::Record::parse(&bytes, generation) {
        failure::update(|f| f.helper = Some(detail));
    }
}

/// A nonblocking AF_UNIX connect cannot wait indefinitely for a full accept queue.
/// Linux's EAGAIN for a full local backlog is an immediate unavailable result.
#[cfg(target_os = "linux")]
fn connect_inspection_socket(path: &std::path::Path, deadline: Instant) -> std::io::Result<UnixStream> {
    use std::{io, os::{fd::FromRawFd, unix::ffi::OsStrExt}};
    wire::inspection_io::check(deadline)?;
    let mut address: libc::sockaddr_un = unsafe { std::mem::zeroed() };
    let bytes = path.as_os_str().as_bytes();
    if bytes.is_empty() || bytes.contains(&0) || bytes.len() >= address.sun_path.len() {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    address.sun_family = libc::AF_UNIX as libc::sa_family_t;
    for (target, byte) in address.sun_path.iter_mut().zip(bytes) {
        *target = *byte as libc::c_char;
    }
    let fd = unsafe {
        libc::socket(
            libc::AF_UNIX,
            libc::SOCK_STREAM | libc::SOCK_NONBLOCK | libc::SOCK_CLOEXEC,
            0,
        )
    };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    let stream = unsafe { UnixStream::from_raw_fd(fd) };
    let length = std::mem::offset_of!(libc::sockaddr_un, sun_path) + bytes.len() + 1;
    let result = unsafe {
        libc::connect(
            fd,
            (&address as *const libc::sockaddr_un).cast(),
            length as libc::socklen_t,
        )
    };
    if result != 0 {
        let error = io::Error::last_os_error();
        if error.raw_os_error() != Some(libc::EINPROGRESS) {
            return Err(error);
        }
        wire::inspection_io::wait(fd, libc::POLLOUT, deadline)?;
        let mut pending: libc::c_int = 0;
        let mut size = std::mem::size_of_val(&pending) as libc::socklen_t;
        if unsafe {
            libc::getsockopt(
                fd,
                libc::SOL_SOCKET,
                libc::SO_ERROR,
                (&mut pending as *mut libc::c_int).cast(),
                &mut size,
            )
        } != 0
        {
            return Err(io::Error::last_os_error());
        }
        if size as usize != std::mem::size_of_val(&pending) {
            return Err(io::ErrorKind::InvalidData.into());
        }
        if pending != 0 {
            return Err(io::Error::from_raw_os_error(pending));
        }
    }
    wire::inspection_io::check(deadline)?;
    Ok(stream)
}

#[cfg(target_os = "linux")]
fn connect(root: &Path) -> Result<(UnixStream, Descriptor), ClientError> {
    connect_to(root, None)
}

#[cfg(target_os = "linux")]
fn connect_to(root: &Path, inspection_deadline: Option<Instant>) -> Result<(UnixStream, Descriptor), ClientError> {
    let mode = if inspection_deadline.is_some() { ConnectionMode::Inspect } else { ConnectionMode::Ordinary };
    connect_to_socket(root, mode, inspection_deadline)
}

/// Endpoint and deadline are independent: a canary is bounded but uses the ordinary socket.
#[cfg(target_os = "linux")]
fn connect_to_socket(root: &Path, mode: ConnectionMode, deadline: Option<Instant>) -> Result<(UnixStream, Descriptor), ClientError> {
    let directory = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(root)
        .map_err(|e| {
            let stage = if e.raw_os_error() == Some(libc::ENOENT) { Stage::RuntimeAbsent } else { Stage::RuntimeInvalid };
            io_failed(stage, e)
        })?;
    let directory_meta = directory.metadata().map_err(|e| io_failed(Stage::RuntimeInvalid, e))?;
    if !directory_meta.is_dir()
        || directory_meta.uid() != unsafe { libc::getuid() }
        || directory_meta.mode() & 0o7777 != 0o700
    {
        return Err(failed(Stage::RuntimeInvalid, None, ClientError::Authentication));
    }
    let anchor = PathBuf::from(format!("/proc/self/fd/{}", directory.as_raw_fd()));
    let descriptor = read_descriptor(&anchor.join(wire::DESCRIPTOR_NAME))?;
    if descriptor.protocol != wire::PROTOCOL
        || descriptor.socket != wire::SOCKET_NAME
        || descriptor.uid != unsafe { libc::getuid() }
        || descriptor.pid == 0
        || !hex_128(&descriptor.nonce)
        || !hex_128(&descriptor.helper_generation)
    {
        return Err(failed(Stage::DescriptorInvalid, None, ClientError::Protocol));
    }
    let socket = anchor.join(match mode {
        ConnectionMode::Inspect => wire::INSPECT_SOCKET_NAME,
        ConnectionMode::Ordinary => wire::SOCKET_NAME,
    });
    let socket_meta = std::fs::symlink_metadata(&socket).map_err(|e| io_failed(
        if e.raw_os_error() == Some(libc::ENOENT) { Stage::SocketAbsent } else { Stage::SocketInvalid }, e))?;
    if !socket_meta.file_type().is_socket()
        || socket_meta.uid() != unsafe { libc::getuid() }
        || socket_meta.mode() & 0o7777 != 0o600
    {
        return Err(failed(Stage::SocketInvalid, None, ClientError::Authentication));
    }
    let stream = match deadline {
        Some(deadline) => connect_inspection_socket(&socket, deadline),
        None => UnixStream::connect(socket),
    }.map_err(|e| io_failed(Stage::Connect, e))?;
    authenticate(&stream)?;
    let named_meta = std::fs::symlink_metadata(root).map_err(|e| io_failed(Stage::RuntimeInvalid, e))?;
    if (named_meta.dev(), named_meta.ino()) != (directory_meta.dev(), directory_meta.ino()) {
        return Err(failed(Stage::RuntimeInvalid, None, ClientError::Authentication));
    }
    Ok((stream, descriptor))
}

fn read_descriptor(path: &Path) -> Result<Descriptor, ClientError> {
    let mut file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK)
        .open(path)
        .map_err(|e| io_failed(Stage::DescriptorInvalid, e))?;
    let meta = file.metadata().map_err(|e| io_failed(Stage::DescriptorInvalid, e))?;
    if !meta.is_file()
        || meta.uid() != unsafe { libc::getuid() }
        || meta.mode() & 0o7777 != 0o600
        || meta.nlink() != 1
        || meta.len() > DESCRIPTOR_MAX
    {
        return Err(failed(Stage::DescriptorInvalid, None, ClientError::Authentication));
    }
    let mut bytes = Vec::new();
    Read::by_ref(&mut file)
        .take(DESCRIPTOR_MAX + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| io_failed(Stage::DescriptorInvalid, e))?;
    if bytes.len() as u64 > DESCRIPTOR_MAX {
        return Err(failed(Stage::DescriptorInvalid, None, ClientError::Protocol));
    }
    serde_json::from_slice(&bytes).map_err(|_| failed(Stage::DescriptorInvalid, None, ClientError::Protocol))
}

#[cfg(target_os = "linux")]
fn authenticate(stream: &UnixStream) -> Result<(), ClientError> {
    let mut credentials: libc::ucred = unsafe { std::mem::zeroed() };
    let mut length = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    let result = unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            &mut credentials as *mut _ as *mut libc::c_void,
            &mut length,
        )
    };
    let errno = (result != 0).then(|| std::io::Error::last_os_error().raw_os_error().unwrap_or(libc::EIO));
    if let Some(detail) = peer_rejection(errno,
        length as usize == std::mem::size_of::<libc::ucred>(),
        credentials.uid == unsafe { libc::getuid() }, credentials.pid > 0) {
        return Err(failed(detail.stage, detail.code, ClientError::Authentication));
    }
    Ok(())
}

fn hex_128(value: &str) -> bool {
    value.len() == 32
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_probe_uses_one_frame_and_requires_its_bound_response() {
        struct Frames { input: std::io::Cursor<Vec<u8>>, output: Vec<u8> }
        impl Read for Frames {
            fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> { self.input.read(bytes) }
        }
        impl std::io::Write for Frames {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                self.output.extend_from_slice(bytes);
                Ok(bytes.len())
            }
            fn flush(&mut self) -> std::io::Result<()> { Ok(()) }
        }
        let descriptor = Descriptor {
            protocol: wire::PROTOCOL, nonce: "01".repeat(16), helper_generation: "02".repeat(16),
            socket: wire::SOCKET_NAME.into(), pid: 1, uid: 1,
        };
        let result = wire::CanaryResult { operation: Ok(()), cleanup: wire::CanaryCleanup::Removed {} };
        for variant in 0..6 {
            let mut input = Vec::new();
            if variant < 4 {
                wire::write_frame(&mut input, &Response::KeymanagerProbe {
                    protocol: wire::PROTOCOL + u32::from(variant == 1),
                    nonce: if variant == 2 { "03".repeat(16) } else { descriptor.nonce.clone() },
                    helper_generation: if variant == 3 { "04".repeat(16) } else { descriptor.helper_generation.clone() },
                    result,
                }).unwrap();
            } else if variant == 4 {
                wire::write_frame(&mut input, &Response::Hello {
                    protocol: wire::PROTOCOL, nonce: descriptor.nonce.clone(),
                    helper_generation: descriptor.helper_generation.clone(),
                    capabilities: wire::Capabilities { db8: true, keymanager: true },
                }).unwrap();
            } // No reply in the final case: no retry or fallback to Hello is permitted.
            let mut stream = Frames { input: std::io::Cursor::new(input), output: Vec::new() };
            let observed = probe_frames(&mut stream, &descriptor);
            if variant == 0 { assert_eq!(observed.unwrap(), result); }
            else { assert!(observed.is_err()); }
            let mut output = stream.output.as_slice();
            assert!(matches!(wire::read_frame::<Request>(&mut output).unwrap(),
                Request::KeymanagerProbe { protocol, nonce }
                    if protocol == wire::PROTOCOL && nonce == descriptor.nonce));
            assert!(output.is_empty(), "probe must never send Hello or another command");
        }
    }

    // Moved from `task::blocking`'s tests: it names this module's `load`, which is a layer above
    // `task`, so the test lives with the call it grades.
    #[test]
    #[should_panic(expected = "main-thread block: storage helper transact")]
    fn a_helper_call_inside_a_frame_is_rejected() {
        let _frame = plx_base::task::FrameScope::enter();
        let _ = load();
    }

    #[test]
    fn decoded_uncertain_reply_without_detail_is_not_a_wire_failure() {
        for response in [
            Response::Commit { status: wire::CommitStatus::Unavailable, db_rev: None,
                state: None, applied: None, verified: false, protection: None },
            Response::Reconcile { status: ReconcileStatus::Unknown, db_rev: None, applied: None, protection: None },
        ] {
            let mut bytes = Vec::new();
            wire::write_frame(&mut bytes, &response).unwrap();
            failure::remember(Stage::HelloRejected, None);
            failure::update(|f| f.activation = Some(failure::Detail::new(Stage::ActivationCall, Some(-13))));
            let root = std::env::temp_dir().join(format!("plx-no-helper-detail-{}", std::process::id()));
            assert!(read_response(&mut bytes.as_slice(), &root, &"01".repeat(16)).is_ok());
            let diagnostic = failure::last().unwrap();
            assert_eq!(diagnostic.helper, None);
            assert_ne!(diagnostic.observed.stage, Stage::Wire);
            assert_eq!(diagnostic.observed.stage, Stage::BackendUncertain);
            assert_eq!(diagnostic.line(), "storage: helper · backend uncertain");
            assert_eq!(serde_json::to_value(diagnostic).unwrap()["observed"]["stage"], "backend-uncertain");
        }
    }

    #[test]
    fn decoded_rejection_and_bad_json_have_distinct_stages() {
        let mut bytes = Vec::new();
        wire::write_frame(&mut bytes, &Response::Error { code: ErrorCode::Authentication }).unwrap();
        read_response(&mut bytes.as_slice(), Path::new("unused"), "unused").unwrap();
        assert_eq!(failure::last().unwrap().observed.stage, Stage::BackendRejected);
        assert_eq!(error(ErrorCode::Authentication), ClientError::Authentication);
        assert_eq!(failure::last().unwrap().observed.stage, Stage::BackendRejected);
        let mut malformed = Vec::from(1u32.to_be_bytes());
        malformed.push(b'{');
        assert!(read_response(&mut malformed.as_slice(), Path::new("unused"), "unused").is_err());
        assert_eq!(failure::last().unwrap().observed.stage, Stage::Wire);
    }

    #[test]
    fn response_frame_io_failure_is_still_wire() {
        struct Broken;
        impl Read for Broken {
            fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
                Err(std::io::Error::from_raw_os_error(libc::ECONNRESET))
            }
        }
        failure::clear();
        assert_eq!(read_response(&mut Broken, Path::new("unused"), "unused").unwrap_err(), ClientError::Protocol);
        assert_eq!(failure::last().unwrap().observed.stage, Stage::Wire);
    }

    #[test]
    fn helper_failure_peer_and_hello_classification() {
        assert_eq!(peer_rejection(Some(libc::EPERM), true, true, true),
            Some(failure::Detail::new(Stage::PeerCredentials, Some(libc::EPERM))));
        assert_eq!(peer_rejection(None, true, false, true).unwrap().stage, Stage::PeerUidMismatch);
        assert_eq!(peer_rejection(None, false, true, true).unwrap().stage, Stage::PeerCredentials);
        assert_eq!(peer_rejection(None, true, true, false).unwrap().stage, Stage::PeerCredentials);
        assert!(peer_rejection(None, true, true, true).is_none());
        let descriptor = Descriptor {
            protocol: wire::PROTOCOL, socket: wire::SOCKET_NAME.into(), uid: 1, pid: 1,
            nonce: "01".repeat(16), helper_generation: "02".repeat(16),
        };
        for code in [ErrorCode::Unavailable, ErrorCode::Timeout, ErrorCode::Capability,
            ErrorCode::Authentication, ErrorCode::Protocol, ErrorCode::Corrupt, ErrorCode::Invalid] {
            failure::clear();
            assert_eq!(validate_hello(Response::Error { code }, &descriptor), Err(classify_error(code)));
            let detail = failure::last().unwrap();
            assert_eq!(detail.observed.stage, Stage::HelloRejected);
            assert_eq!(detail.wire, Some(code));
        }
        for (protocol, nonce, generation, db8) in [
            (0, descriptor.nonce.clone(), descriptor.helper_generation.clone(), true),
            (wire::PROTOCOL, "private-fixture".into(), descriptor.helper_generation.clone(), true),
            (wire::PROTOCOL, descriptor.nonce.clone(), "wrong".into(), true),
            (wire::PROTOCOL, descriptor.nonce.clone(), descriptor.helper_generation.clone(), false),
        ] {
            assert_eq!(validate_hello(Response::Hello { protocol, nonce, helper_generation: generation,
                capabilities: wire::Capabilities { db8, keymanager: false } }, &descriptor), Err(ClientError::Protocol));
        }
    }

    #[test]
    fn helper_failure_connect_error_kept_at_start_deadline() {
        // The old loop discarded these observations and returned only Unavailable.
        for (stage, error, code) in [
            (Stage::RuntimeAbsent, ClientError::Unavailable, Some(libc::ENOENT)),
            (Stage::RuntimeInvalid, ClientError::Authentication, None),
            (Stage::SocketAbsent, ClientError::Unavailable, Some(libc::ENOENT)),
            (Stage::SocketInvalid, ClientError::Authentication, None),
            (Stage::Connect, ClientError::Unavailable, Some(libc::ECONNREFUSED)),
            (Stage::PeerUidMismatch, ClientError::Authentication, None),
            (Stage::PeerCredentials, ClientError::Authentication, Some(libc::EIO)),
            (Stage::DescriptorInvalid, ClientError::Protocol, None),
        ] {
            failure::clear();
            let observed = failed(stage, code, error);
            let mut gate = StartGate::default();
            let now = Instant::now();
            assert!(!gate.wait_after_failed_connect(now, now));
            assert_eq!(start_unavailable(observed), ClientError::Unavailable);
            let detail = failure::last().unwrap();
            assert_eq!(detail.observed, failure::Detail::new(stage, code));
            assert!(detail.start_timeout);
        }
    }

    #[test]
    fn helper_failure_descriptor_validation_and_last_error_are_bounded() {
        use std::os::unix::fs::PermissionsExt;
        let root = std::env::temp_dir().join(format!("plx-helper-diag-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
        let path = root.join("rendezvous.json");
        for (mode, bytes, expected) in [
            (0o644, b"{}".as_slice(), ClientError::Authentication),
            (0o600, b"not-json".as_slice(), ClientError::Protocol),
        ] {
            std::fs::write(&path, bytes).unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).unwrap();
            failure::clear();
            assert_eq!(read_descriptor(&path).err(), Some(expected));
            assert_eq!(failure::last().unwrap().observed.stage, Stage::DescriptorInvalid);
        }
        let last = root.join("last-error");
        let detail = failure::Detail::new(Stage::Db8, Some(-3963));
        std::fs::write(&last, serde_json::to_vec(&failure::Record { helper_generation: "01".repeat(16), detail }).unwrap()).unwrap();
        std::fs::set_permissions(&last, std::fs::Permissions::from_mode(0o600)).unwrap();
        read_helper_failure(&root, Some(&"01".repeat(16)), None);
        assert_eq!(failure::last().unwrap().helper, Some(detail));
        std::fs::write(&last, b"token=private-fixture hostname=private-host").unwrap();
        failure::remember(Stage::HelloRejected, None);
        read_helper_failure(&root, Some(&"01".repeat(16)), None);
        assert_eq!(failure::last().unwrap().helper, None);
        std::fs::remove_dir_all(root).unwrap();
    }

    fn start_failure_fixture(name: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let root = std::env::temp_dir().join(format!("plx-start-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
        root
    }

    fn write_start_fixture(root: &Path, nonce: &str, stage: Stage) {
        use std::os::unix::fs::PermissionsExt;
        let bytes = serde_json::to_vec(&serde_json::json!({
            "activation_nonce": nonce,
            "detail": {"stage": stage, "code": -13},
        })).unwrap();
        let path = root.join("start-error");
        std::fs::write(&path, bytes).unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).unwrap();
    }

    #[test]
    fn current_activation_start_failure_survives_connect_failure() {
        let root = start_failure_fixture("current");
        let nonce = "01".repeat(16);
        for stage in [Stage::BusContext, Stage::BusRegister, Stage::BusAttach, Stage::BusCancel] {
            write_start_fixture(&root, &nonce, stage);
            failure::remember(Stage::Connect, Some(libc::ECONNREFUSED));
            start_unavailable(ClientError::Unavailable);
            read_helper_failure(&root, None, Some(&nonce));
            let current = failure::last().unwrap();
            assert_eq!(current.helper, Some(failure::Detail::new(stage, Some(-13))));
            assert!(current.line().contains("(-13)"));
            assert!(!serde_json::to_string(&current).unwrap().contains(&nonce));
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn earlier_activation_start_failure_is_ignored_after_a_current_record() {
        let root = start_failure_fixture("earlier");
        let nonce = "02".repeat(16);
        // Positive control: ignoring every record would hide the stale-record bug.
        write_start_fixture(&root, &nonce, Stage::BusRegister);
        failure::remember(Stage::Connect, Some(libc::ECONNREFUSED));
        read_helper_failure(&root, None, Some(&nonce));
        assert_eq!(failure::last().unwrap().helper,
            Some(failure::Detail::new(Stage::BusRegister, Some(-13))));
        write_start_fixture(&root, &"01".repeat(16), Stage::BusAttach);
        failure::remember(Stage::Connect, Some(libc::ECONNREFUSED));
        read_helper_failure(&root, None, Some(&nonce));
        let current = failure::last().unwrap();
        assert_eq!(current.helper, None);
        assert_eq!(current.line(), format!("storage: helper · connect refused ({})", libc::ECONNREFUSED));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn previous_helper_failure_cannot_override_current_connect_failure() {
        use std::os::unix::fs::PermissionsExt;
        let root = std::env::temp_dir().join(format!("plx-helper-stale-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
        let last = root.join("last-error");
        std::fs::write(&last, serde_json::to_vec(&failure::Detail::new(Stage::Db8, Some(-3963))).unwrap()).unwrap();
        std::fs::set_permissions(&last, std::fs::Permissions::from_mode(0o600)).unwrap();
        failure::remember(Stage::Connect, Some(libc::ECONNREFUSED));
        read_helper_failure(&root, None, None);
        let current = failure::last().unwrap();
        assert_eq!(current.helper, None);
        assert_eq!(current.line(), format!("storage: helper · connect refused ({})", libc::ECONNREFUSED));
        // Legacy unbound records cannot be attributed even after a successful Hello.
        read_helper_failure(&root, Some(&"02".repeat(16)), None);
        assert_eq!(failure::last().unwrap().helper, None);
        let detail = failure::Detail::new(Stage::Db8, Some(-3963));
        let record = failure::Record { helper_generation: "01".repeat(16), detail };
        std::fs::write(&last, serde_json::to_vec(&record).unwrap()).unwrap();
        // An orphan from an unclean exit, or a successor replacing the file, must not match.
        for generation in [None, Some("02".repeat(16))] {
            read_helper_failure(&root, generation.as_deref(), None);
            assert_eq!(failure::last().unwrap().helper, None);
        }
        read_helper_failure(&root, Some(&record.helper_generation), None);
        assert_eq!(failure::last().unwrap().helper, Some(detail));
        assert_eq!(failure::last().unwrap().line(), "storage: helper · db8 (-3963)");
        let report = serde_json::to_string(&failure::last().unwrap()).unwrap();
        assert!(!report.contains(&record.helper_generation));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn helper_failure_descriptor_open_keeps_errno() {
        use super::wire::failure::{self, Stage};
        failure::clear();
        let missing = std::env::temp_dir().join(format!("plx-helper-missing-{}", std::process::id()));
        assert!(read_descriptor(&missing).is_err());
        assert_eq!(failure::last().map(|f| f.observed),
            Some(failure::Detail::new(Stage::DescriptorInvalid, Some(libc::ENOENT))));
    }

    #[test]
    fn a_failed_start_makes_later_calls_return_without_waiting() {
        let mut gate = StartGate::default();
        let now = Instant::now();
        let until = now + START_DEADLINE;
        assert!(
            gate.wait_after_failed_connect(now, until),
            "the first call allows startup time"
        );
        assert!(!gate.wait_after_failed_connect(until, until));

        let next = until + Duration::from_millis(25);
        assert!(
            !gate.wait_after_failed_connect(next, next + START_DEADLINE),
            "a failed start must not buy another twelve-second wait"
        );
        let later = next + START_DEADLINE * 10;
        assert!(
            !gate.wait_after_failed_connect(later, later + START_DEADLINE),
            "elapsed time alone must not forget an unreachable helper"
        );
    }

    #[test]
    fn a_successful_connect_resets_the_failed_start() {
        let mut gate = StartGate::default();
        let now = Instant::now();
        assert!(!gate.wait_after_failed_connect(now, now));
        assert!(!gate.wait_after_failed_connect(now, now + START_DEADLINE));

        gate.connected();
        assert!(
            gate.wait_after_failed_connect(now, now + START_DEADLINE),
            "a recovered helper gets the normal startup window if it later restarts"
        );
    }

    #[test]
    fn activation_hints_are_rate_limited_without_reopening_the_start_window() {
        let mut gate = StartGate::default();
        let now = Instant::now();
        assert!(
            gate.hint_due(now),
            "the first failed connect hints activation immediately"
        );
        let root = start_failure_fixture("gate");
        gate.attempt = wire::activation::Attempt::begin(&root);
        gate.activation = Some(failure::Detail::new(Stage::ActivationSent, None));
        assert!(gate.attempt.is_some());
        assert!(!gate.hint_due(now + START_DEADLINE / 2));
        assert!(gate.attempt.is_some());
        assert!(!gate.wait_after_failed_connect(now + START_DEADLINE, now + START_DEADLINE));

        let next = now + START_DEADLINE;
        assert!(
            gate.hint_due(next),
            "a later call may re-hint a missing helper"
        );
        // Clear the old attempt before a queued activation worker can publish the new one.
        assert!(gate.attempt.is_none());
        assert!(gate.activation.is_none());
        assert!(
            !gate.hint_due(next),
            "other callers share the hint rate limit"
        );
        assert!(
            !gate.wait_after_failed_connect(next, next + START_DEADLINE),
            "re-hinting must not restart the blocking wait"
        );

        gate.connected();
        assert!(
            gate.hint_due(next),
            "a recovered helper may be activated again if it restarts"
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn generation_and_expectation_never_cross_a_json_float() {
        let generation = Generation([0xff; 16]);
        let Expectation::Present {
            db_rev,
            epoch,
            auth_generation,
        } = wire_expectation(Some((
            &u64::MAX.to_string(),
            Expected {
                epoch: u64::MAX,
                auth_generation: generation,
            },
        )))
        else {
            panic!("present expectation")
        };
        assert_eq!(db_rev, u64::MAX.to_string());
        assert_eq!(epoch, u64::MAX.to_string());
        assert_eq!(auth_generation, "ff".repeat(16));
    }

    #[test]
    fn closed_error_mapping_never_exposes_service_text() {
        assert_eq!(error(ErrorCode::Capability), ClientError::Unavailable);
        assert_eq!(
            error(ErrorCode::Authentication),
            ClientError::Authentication
        );
    }
}

#[cfg(test)]
#[path = "client_inspection_tests.rs"]
mod inspection_tests;
