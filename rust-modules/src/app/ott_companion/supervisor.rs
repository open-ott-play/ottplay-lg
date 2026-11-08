//! All process, disk and socket work stays on one long-lived application worker.
//! Its lifetime also owns the Linux parent-death signal and every media lease.
use super::{
    client::Client,
    protocol::{Endpoint, Error, Reply, Request, Session},
};
use plx_base::task::time::{Budget, Deadline};
use plx_base::task::OffFrame;
use serde::{Deserialize, Serialize};
#[cfg(target_os = "linux")]
use std::os::unix::process::CommandExt;
use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::CString,
    fs::{self, File},
    io::{self, Read, Write},
    os::unix::{
        ffi::OsStrExt,
        fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt},
        io::AsRawFd,
    },
    path::{Path, PathBuf},
    process::{Child, ChildStderr, Command, Stdio},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{self, Receiver, SyncSender},
        Arc, Mutex,
    },
    thread::JoinHandle,
    time::Duration,
};

const MIB: u64 = 1024 * 1024;
const REQUEST_CAPACITY: usize = 8;
// Leave room for the independent 64 MiB Torznab reference store and the app.
const DISK_RESERVE: u64 = 96 * MIB;

#[path = "erasure.rs"]
mod erasure;
pub use erasure::ErasureStop;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct Configuration {
    pub enabled: bool,
    pub cache_bytes: u64,
}
impl Default for Configuration {
    fn default() -> Self {
        Self {
            enabled: true,
            cache_bytes: 256 * MIB,
        }
    }
}
impl Configuration {
    pub fn validate(&self) -> Result<(), Error> {
        if !(64 * MIB..=512 * MIB).contains(&self.cache_bytes) {
            return Err(Error("companion_invalid_limits"));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub installed: bool,
    pub ready: bool,
    pub local: bool,
    pub torrent: bool,
    pub local_torznab: bool,
    pub local_torznab_children: bool,
    pub plugins: bool,
    pub plugin_network: bool,
    pub plugin_network_default: bool,
    pub state: &'static str,
    pub error: Option<&'static str>,
    pub cache_bytes: u64,
    pub max_sessions: usize,
    pub max_streams: usize,
}
impl Default for Snapshot {
    fn default() -> Self {
        Self {
            installed: false,
            ready: false,
            local: true,
            torrent: false,
            local_torznab: false,
            local_torznab_children: false,
            plugins: false,
            plugin_network: false,
            plugin_network_default: false,
            state: "checking",
            error: None,
            cache_bytes: 0,
            max_sessions: 1,
            max_streams: 2,
        }
    }
}
pub enum Event {
    Reply {
        id: u64,
        result: Result<Reply, Error>,
    },
    Changed,
}
struct Work {
    id: u64,
    epoch: u64,
    lifecycle: u64,
    request: Request,
    deadline: Deadline,
    cancelled: Arc<AtomicBool>,
}
struct Shared {
    desired: Mutex<Configuration>,
    snapshot: Mutex<Snapshot>,
    endpoint: Mutex<Option<Endpoint>>,
    leases: Mutex<BTreeMap<String, u64>>,
    live_generation: AtomicU64,
    lifecycle: AtomicU64,
    media: AtomicU64,
    shutdown: AtomicBool,
}
fn lock<T>(value: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    value.lock().unwrap_or_else(|e| e.into_inner())
}

pub struct Supervisor {
    shared: Arc<Shared>,
    tx: SyncSender<Work>,
    rx: Receiver<Event>,
    pending: BTreeSet<u64>,
    next_id: u64,
    observed: Snapshot,
    worker: Option<JoinHandle<()>>,
    data_root: Option<PathBuf>,
}
impl Supervisor {
    /// Pure test fixture: no filesystem inspection, worker, helper, or global state.
    #[cfg(test)]
    pub(crate) fn inert_for_test() -> Self {
        let (tx, _) = mpsc::sync_channel(REQUEST_CAPACITY);
        let (_, rx) = mpsc::sync_channel(REQUEST_CAPACITY);
        Self {
            shared: Arc::new(Shared {
                desired: Mutex::new(Configuration {
                    enabled: false,
                    ..Configuration::default()
                }),
                snapshot: Mutex::new(Snapshot::default()),
                endpoint: Mutex::new(None),
                leases: Mutex::new(BTreeMap::new()),
                live_generation: AtomicU64::new(0),
                lifecycle: AtomicU64::new(1),
                media: AtomicU64::new(1),
                shutdown: AtomicBool::new(false),
            }),
            tx,
            rx,
            pending: BTreeSet::new(),
            next_id: 1,
            observed: Snapshot::default(),
            worker: None,
            data_root: None,
        }
    }

    /// Accept real submissions into a retained mailbox, without a worker or
    /// helper. Observe each request's captured and current media epochs.
    #[cfg(all(test, feature = "hostsim"))]
    pub(crate) fn ready_mailbox_for_test(&mut self) -> impl FnMut() -> Option<(Request, u64, u64)> {
        let (tx, rx) = mpsc::sync_channel::<Work>(REQUEST_CAPACITY);
        self.tx = tx;
        {
            let mut snapshot = lock(&self.shared.snapshot);
            snapshot.ready = true;
            snapshot.torrent = true;
            snapshot.local_torznab = true;
            snapshot.plugins = true;
        }
        // Ready capabilities are visible only for the current helper lifetime.
        self.shared.live_generation.store(self.shared.lifecycle.load(Ordering::SeqCst), Ordering::SeqCst);
        let shared = self.shared.clone();
        move || rx.try_recv().ok().map(|work| {
            (work.request, work.epoch, shared.media.load(Ordering::SeqCst))
        })
    }

    /// Resolving the paths is the caller's job; filesystem inspection occurs on the worker.
    pub fn new(app_dir: PathBuf, data_root: PathBuf, config: Configuration) -> Self {
        let owned_root = data_root.clone();
        let shared = Arc::new(Shared {
            desired: Mutex::new(config),
            snapshot: Mutex::new(Snapshot::default()),
            endpoint: Mutex::new(None),
            leases: Mutex::new(BTreeMap::new()),
            live_generation: AtomicU64::new(0),
            lifecycle: AtomicU64::new(1),
            media: AtomicU64::new(1),
            shutdown: AtomicBool::new(false),
        });
        let (tx, work) = mpsc::sync_channel(REQUEST_CAPACITY);
        let (results, rx) = mpsc::sync_channel(REQUEST_CAPACITY);
        let state = shared.clone();
        let worker = plx_base::task::spawn_off_frame_keeping("ott-companion", move |off| {
            run(off, app_dir, data_root, state, work, results)
        });
        if worker.is_none() {
            update(&shared, |s| {
                s.state = "unavailable";
                s.error = Some("companion_worker_unavailable");
            });
        }
        Self {
            shared,
            tx,
            rx,
            pending: BTreeSet::new(),
            next_id: 1,
            observed: Snapshot::default(),
            worker,
            data_root: Some(owned_root),
        }
    }
    pub fn snapshot(&self) -> Snapshot {
        let mut snapshot = lock(&self.shared.snapshot).clone();
        if self.shared.live_generation.load(Ordering::SeqCst)
            != self.shared.lifecycle.load(Ordering::SeqCst)
        {
            snapshot.ready = false;
            snapshot.torrent = false;
            snapshot.local_torznab = false;
            snapshot.local_torznab_children = false;
            snapshot.plugins = false;
        }
        snapshot
    }
    pub fn start(&mut self) -> Result<(), Error> {
        if self.worker.is_none() {
            return Err(Error("companion_worker_unavailable"));
        }
        lock(&self.shared.desired).enabled = true;
        self.shared.lifecycle.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
    pub fn stop(&mut self) {
        lock(&self.shared.desired).enabled = false;
        self.shared.lifecycle.fetch_add(1, Ordering::SeqCst);
        self.cancel_media();
        *lock(&self.shared.endpoint) = None;
        update(&self.shared, |s| {
            s.ready = false;
            s.torrent = false;
            s.local_torznab = false;
            s.local_torznab_children = false;
            s.plugins = false;
            s.state = "stopping";
        });
    }
    /// Revoke producers immediately; the returned owner completes shutdown and
    /// erasure off-frame. Dropping this Supervisor cannot lose that join handle.
    pub fn retire_for_erasure(&mut self) -> ErasureStop {
        self.stop();
        self.shared.shutdown.store(true, Ordering::SeqCst);
        self.pending.clear();
        ErasureStop::new(self.worker.take(), self.data_root.take())
    }
    /// Call only after the protected configuration has been durably saved.
    pub fn configure(&mut self, configuration: Configuration) -> Result<(), Error> {
        if self.shared.shutdown.load(Ordering::SeqCst) {
            return Err(Error("companion_cancelled"));
        }
        configuration.validate()?;
        let mut current = lock(&self.shared.desired);
        if *current != configuration {
            *current = configuration;
            self.shared.lifecycle.fetch_add(1, Ordering::SeqCst);
            self.shared.media.fetch_add(1, Ordering::SeqCst);
            *lock(&self.shared.endpoint) = None;
            update(&self.shared, |s| {
                s.ready = false;
                s.torrent = false;
                s.local_torznab = false;
                s.local_torznab_children = false;
                s.plugins = false;
                s.state = "changing";
            });
        }
        Ok(())
    }
    pub fn cancel_media(&mut self) {
        self.shared.media.fetch_add(1, Ordering::SeqCst);
        lock(&self.shared.leases).clear();
    }
    pub fn submit_scoped(
        &mut self,
        request: Request,
        deadline: Deadline,
        cancelled: Arc<AtomicBool>,
    ) -> Result<u64, Error> {
        if cancelled.load(Ordering::SeqCst) {
            return Err(Error("companion_cancelled"));
        }
        if !self.snapshot().ready {
            return Err(Error("companion_not_ready"));
        }
        if self.pending.len() >= REQUEST_CAPACITY {
            return Err(Error("companion_busy"));
        }
        // Validate before queueing, including potentially large caller-supplied strings.
        request.prepare()?;
        let id = self.next_id;
        self.next_id = self
            .next_id
            .checked_add(1)
            .ok_or(Error("companion_request_limit"))?;
        self.tx
            .try_send(Work {
                id,
                epoch: self.shared.media.load(Ordering::SeqCst),
                lifecycle: self.shared.lifecycle.load(Ordering::SeqCst),
                request,
                deadline,
                cancelled,
            })
            .map_err(|_| Error("companion_busy"))?;
        self.pending.insert(id);
        Ok(id)
    }
    pub fn stream_url(&self, session: &Session) -> Result<String, Error> {
        if !self.snapshot().ready {
            return Err(Error("companion_not_ready"));
        }
        if lock(&self.shared.leases).get(&session.id).copied()
            != Some(self.shared.media.load(Ordering::SeqCst))
        {
            return Err(Error("companion_cancelled"));
        }
        let endpoint = lock(&self.shared.endpoint);
        session.stream_url(endpoint.as_ref().ok_or(Error("companion_not_ready"))?)
    }
    pub fn poll(&mut self) -> Vec<Event> {
        let mut events = Vec::with_capacity(REQUEST_CAPACITY + 1);
        while let Ok(event) = self.rx.try_recv() {
            if let Event::Reply { id, .. } = &event {
                self.pending.remove(id);
            }
            events.push(event);
        }
        let snapshot = self.snapshot();
        if self.observed != snapshot {
            self.observed = snapshot;
            events.push(Event::Changed);
        }
        events
    }
}
impl Drop for Supervisor {
    fn drop(&mut self) {
        self.shared.shutdown.store(true, Ordering::SeqCst);
        self.shared.media.fetch_add(1, Ordering::SeqCst);
        // Never join a socket or process on the frame thread. The owner worker
        // reaps its child before exiting; Linux additionally sets PDEATHSIG.
        self.worker.take();
    }
}
fn update(shared: &Shared, f: impl FnOnce(&mut Snapshot)) {
    f(&mut lock(&shared.snapshot));
    plx_machine::idle::invalidate();
}
fn unavailable(shared: &Shared, code: &'static str) {
    *lock(&shared.endpoint) = None;
    update(shared, |s| {
        s.ready = false;
        s.torrent = false;
        s.local_torznab = false;
        s.local_torznab_children = false;
        s.plugins = false;
        s.state = "unavailable";
        s.error = Some(code);
    });
}
fn media_request(request: &Request) -> bool {
    matches!(
        request,
        Request::Create { .. }
            | Request::Select { .. }
            | Request::Session { .. }
            | Request::ResolvePlugin { .. }
    )
}
fn run(
    off: &OffFrame,
    app: PathBuf,
    root: PathBuf,
    shared: Arc<Shared>,
    work: Receiver<Work>,
    results: SyncSender<Event>,
) {
    let binary = app.join("bin/ottplay-companion");
    let installed = valid_binary(&binary).is_ok();
    update(&shared, |s| s.installed = installed);
    let mut process: Option<Process> = None;
    let mut generation = 0;
    let mut media = shared.media.load(Ordering::SeqCst);
    let mut leases = BTreeSet::<String>::new();
    while !shared.shutdown.load(Ordering::SeqCst) {
        let wanted = shared.lifecycle.load(Ordering::SeqCst);
        if generation != wanted {
            *lock(&shared.endpoint) = None;
            lock(&shared.leases).clear();
            process.take();
            leases.clear();
            generation = wanted;
            let config = lock(&shared.desired).clone();
            if !config.enabled {
                update(&shared, |s| {
                    s.ready = false;
                    s.torrent = false;
                    s.local_torznab = false;
                    s.local_torznab_children = false;
                    s.plugins = false;
                    s.state = "stopped";
                    s.error = None;
                });
            } else {
                update(&shared, |s| {
                    s.ready = false;
                    s.state = "starting";
                    s.error = None;
                });
                match Process::launch(off, &binary, &root, &config, &shared, generation) {
                    Ok(child) => process = Some(child),
                    Err(error) => unavailable(&shared, error.code()),
                }
            }
        }
        let epoch = shared.media.load(Ordering::SeqCst);
        if media != epoch {
            let cleanup_deadline = Deadline::after(off, Duration::from_secs(1));
            let mut cleaned = true;
            if let Some(child) = process.as_ref() {
                for id in &leases {
                    cleaned &= child
                        .client
                        .execute_cancellable(off, &Request::Delete { id: id.clone() }, &|| {
                            shared.shutdown.load(Ordering::SeqCst)
                                || generation != shared.lifecycle.load(Ordering::SeqCst)
                                || cleanup_deadline.expired(off)
                        })
                        .is_ok();
                }
            }
            if !cleaned {
                process.take();
                generation = 0;
                unavailable(&shared, "companion_reset_after_cancel");
            }
            leases.clear();
            lock(&shared.leases).clear();
            media = epoch;
        }
        if let Some(child) = process.as_mut() {
            child.output.drain();
            if !matches!(child.child.try_wait(), Ok(None)) {
                process.take();
                leases.clear();
                lock(&shared.leases).clear();
                unavailable(&shared, "companion_exited");
            }
        }
        let job = match work.recv_timeout(Duration::from_millis(50)) {
            Ok(job) => job,
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        };
        let stale = || {
            job.cancelled.load(Ordering::SeqCst)
                || job.deadline.expired(off)
                || job.lifecycle != shared.lifecycle.load(Ordering::SeqCst)
                || (media_request(&job.request) && job.epoch != shared.media.load(Ordering::SeqCst))
                || shared.shutdown.load(Ordering::SeqCst)
        };
        let attempted = !stale() && process.is_some();
        let mut result = if !attempted {
            Err(Error("companion_cancelled"))
        } else {
            process
                .as_ref()
                .ok_or(Error("companion_not_ready"))
                .and_then(|child| child.client.execute_cancellable(off, &job.request, &stale))
        };
        let mut reset = attempted
            && matches!(&job.request, Request::Create { .. })
            && result
                .as_ref()
                .is_err_and(|error| error.code().starts_with("companion_"));
        if let Ok(Reply::Session(session)) = &result {
            if matches!(&job.request, Request::Create { .. }) {
                leases.insert(session.id.clone());
                lock(&shared.leases).insert(session.id.clone(), job.epoch);
            }
        }
        if let Request::Delete { id } = &job.request {
            if result.is_ok() {
                leases.remove(id);
                lock(&shared.leases).remove(id);
            }
        }
        if stale() {
            if let Ok(Reply::Session(session)) = &result {
                if matches!(&job.request, Request::Create { .. }) {
                    if let Some(child) = process.as_ref() {
                        let cleanup_deadline = Deadline::after(off, Duration::from_secs(1));
                        reset |= child
                            .client
                            .execute_cancellable(
                                off,
                                &Request::Delete {
                                    id: session.id.clone(),
                                },
                                &|| {
                                    shared.shutdown.load(Ordering::SeqCst)
                                        || cleanup_deadline.expired(off)
                                },
                            )
                            .is_err();
                    }
                    leases.remove(&session.id);
                    lock(&shared.leases).remove(&session.id);
                }
            }
            result = Err(Error("companion_cancelled"));
        }
        // A lost Create response can conceal a successfully allocated session.
        // Retire the process instead of leaving its only slot permanently busy.
        if reset {
            process.take();
            leases.clear();
            lock(&shared.leases).clear();
            generation = 0;
            unavailable(&shared, "companion_reset_after_cancel");
        }
        // At most eight requests can be outstanding; a full result mailbox is
        // impossible while the owning handle lives. Disconnection ends ownership.
        if results
            .try_send(Event::Reply { id: job.id, result })
            .is_err()
        {
            break;
        }
        plx_machine::idle::invalidate();
    }
    *lock(&shared.endpoint) = None;
    drop(process);
}

struct Process<'a> {
    worker: &'a OffFrame,
    child: Child,
    output: Output,
    client: Client,
}
impl<'a> Process<'a> {
    fn launch(
        off: &'a OffFrame,
        binary: &Path,
        root: &Path,
        config: &Configuration,
        shared: &Shared,
        generation: u64,
    ) -> Result<Self, Error> {
        config.validate()?;
        valid_binary(binary)?;
        private_directory(root)?;
        let cache = disk_budget(config.cache_bytes, free_bytes(root)?)?;
        let mut entropy = [0u8; 32];
        File::open("/dev/urandom")
            .and_then(|mut f| f.read_exact(&mut entropy))
            .map_err(|_| Error("companion_entropy_unavailable"))?;
        let token = entropy
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        let mut command = Command::new(binary);
        command
            .env_clear()
            .env("OTT_COMPANION_DIR", root)
            .env("OTT_COMPANION_TOKEN", &token)
            .env("OTT_COMPANION_PORT", "0")
            .env("OTT_COMPANION_CACHE_BYTES", cache.to_string())
            .env("OTT_COMPANION_MAX_SESSIONS", "1")
            .env("OTT_COMPANION_MAX_STREAMS", "2")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        // The spawning worker remains alive for the complete child lifetime:
        // Linux associates this signal with the creating thread, not just PID.
        #[cfg(target_os = "linux")]
        unsafe {
            let parent = libc::getpid();
            command.pre_exec(move || {
                // Unexpected owner death must not leave an unbounded shutdown
                // behind. Normal application stop still uses graceful SIGTERM.
                if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL) != 0 {
                    return Err(io::Error::last_os_error());
                }
                if libc::getppid() != parent {
                    return Err(io::Error::from_raw_os_error(libc::ESRCH));
                }
                Ok(())
            });
        }
        let mut child = command
            .spawn()
            .map_err(|_| Error("companion_spawn_failed"))?;
        let Some(stderr) = child.stderr.take() else {
            let _ = child.kill();
            let _ = child.wait();
            return Err(Error("companion_start_failed"));
        };
        let output = match Output::new(stderr) {
            Ok(output) => output,
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(error);
            }
        };
        // A temporary endpoint allows one cleanup owner immediately after spawn.
        let mut process = Self {
            worker: off,
            child,
            output,
            client: Client::new(Endpoint::new(1, token.clone())?),
        };
        let budget = Budget::start(off, Duration::from_secs(12));
        let port = loop {
            if shared.shutdown.load(Ordering::SeqCst)
                || generation != shared.lifecycle.load(Ordering::SeqCst)
            {
                return Err(Error("companion_cancelled"));
            }
            if !matches!(process.child.try_wait(), Ok(None)) {
                return Err(Error("companion_start_failed"));
            }
            if let Some(port) = process.output.drain() {
                break port;
            }
            if budget.expired(off) {
                return Err(Error("companion_start_timeout"));
            }
            budget.wait(
                off,
                Duration::from_millis(20),
                Duration::from_millis(20),
                &|| shared.shutdown.load(Ordering::SeqCst),
            );
        };
        process.client = Client::new(Endpoint::new(port, token)?);
        let Reply::Capabilities(caps) =
            process
                .client
                .execute_cancellable(off, &Request::Capabilities, &|| {
                    shared.shutdown.load(Ordering::SeqCst)
                        || generation != shared.lifecycle.load(Ordering::SeqCst)
                })?
        else {
            return Err(Error("companion_invalid_capabilities"));
        };
        if !caps.torrent
            || !caps.local_torznab
            || !caps.file_selection
            || caps.plugin_network_default
            || caps.inbound_peer_listener
            || caps.cache_bytes != cache
            || caps.max_sessions != 1
            || caps.max_streams != 2
        {
            return Err(Error("companion_invalid_capabilities"));
        }
        if shared.shutdown.load(Ordering::SeqCst)
            || generation != shared.lifecycle.load(Ordering::SeqCst)
        {
            return Err(Error("companion_cancelled"));
        }
        *lock(&shared.endpoint) = Some(process.client.endpoint().clone());
        shared.live_generation.store(generation, Ordering::SeqCst);
        update(shared, |s| {
            s.installed = true;
            s.ready = true;
            s.torrent = caps.torrent;
            s.local_torznab = caps.local_torznab;
            s.local_torznab_children = caps.local_torznab_children;
            s.plugins = caps.plugins;
            s.plugin_network = caps.plugin_network;
            s.plugin_network_default = caps.plugin_network_default;
            s.state = "ready";
            s.error = None;
            s.cache_bytes = cache;
        });
        Ok(process)
    }
}
impl Drop for Process<'_> {
    fn drop(&mut self) {
        if matches!(self.child.try_wait(), Ok(Some(_))) {
            return;
        }
        // A child cannot have a recycled PID before wait reaps it.
        unsafe {
            libc::kill(self.child.id() as libc::pid_t, libc::SIGTERM);
        }
        let budget = Budget::start(self.worker, Duration::from_secs(3));
        while !budget.expired(self.worker) {
            self.output.drain();
            if matches!(self.child.try_wait(), Ok(Some(_))) {
                return;
            }
            budget.wait(
                self.worker,
                Duration::from_millis(20),
                Duration::from_millis(20),
                &|| false,
            );
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

struct Output {
    pipe: ChildStderr,
    line: Vec<u8>,
    discard: bool,
}
impl Output {
    fn new(pipe: ChildStderr) -> Result<Self, Error> {
        let fd = pipe.as_raw_fd();
        unsafe {
            let flags = libc::fcntl(fd, libc::F_GETFL);
            if flags < 0 || libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) < 0 {
                return Err(Error("companion_output_unavailable"));
            }
        }
        Ok(Self {
            pipe,
            line: Vec::with_capacity(256),
            discard: false,
        })
    }
    fn drain(&mut self) -> Option<u16> {
        let mut found = None;
        let mut buffer = [0u8; 2048];
        // Discard diagnostics rather than retaining URLs or credentials.
        for _ in 0..32 {
            match self.pipe.read(&mut buffer) {
                Ok(0) => break,
                Ok(count) => {
                    for byte in &buffer[..count] {
                        if *byte == b'\n' {
                            if !self.discard {
                                found = found.or_else(|| ready_port(&self.line));
                            }
                            self.line.clear();
                            self.discard = false;
                        } else if self.line.len() < 1024 && !self.discard {
                            self.line.push(*byte);
                        } else {
                            self.line.clear();
                            self.discard = true;
                        }
                    }
                }
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(_) => break,
            }
        }
        found
    }
}
fn ready_port(line: &[u8]) -> Option<u16> {
    let text = std::str::from_utf8(line).ok()?;
    let port = text.strip_prefix("ottplay-companion: http://127.0.0.1:")?;
    if !port.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    port.parse::<u16>().ok().filter(|port| *port != 0)
}
fn valid_binary(path: &Path) -> Result<(), Error> {
    let metadata = fs::symlink_metadata(path).map_err(|_| Error("companion_not_installed"))?;
    if !path.is_absolute()
        || !metadata.is_file()
        || metadata.file_type().is_symlink()
        || metadata.mode() & 0o111 == 0
        || metadata.mode() & 0o022 != 0
    {
        return Err(Error("companion_invalid_binary"));
    }
    Ok(())
}
fn private_directory(root: &Path) -> Result<(), Error> {
    if !root.is_absolute() {
        return Err(Error("companion_invalid_directory"));
    }
    let parent = root.parent().ok_or(Error("companion_invalid_directory"))?;
    let parent_meta =
        fs::symlink_metadata(parent).map_err(|_| Error("companion_directory_unavailable"))?;
    if !parent_meta.is_dir() || parent_meta.file_type().is_symlink() {
        return Err(Error("companion_invalid_directory"));
    }
    match fs::DirBuilder::new().mode(0o700).create(root) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
        Err(_) => return Err(Error("companion_directory_unavailable")),
    }
    let metadata =
        fs::symlink_metadata(root).map_err(|_| Error("companion_directory_unavailable"))?;
    if !metadata.is_dir()
        || metadata.file_type().is_symlink()
        || metadata.uid() != unsafe { libc::geteuid() }
    {
        return Err(Error("companion_invalid_directory"));
    }
    fs::set_permissions(root, fs::Permissions::from_mode(0o700))
        .map_err(|_| Error("companion_directory_unavailable"))?;
    let mut nonce = [0u8; 16];
    File::open("/dev/urandom")
        .and_then(|mut file| file.read_exact(&mut nonce))
        .map_err(|_| Error("companion_entropy_unavailable"))?;
    let suffix = nonce
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let probe = root.join(format!(".write-probe-{suffix}"));
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(&probe)
        .map_err(|_| Error("companion_directory_unavailable"))?;
    let writable = file
        .write_all(b"local companion\n")
        .and_then(|()| file.sync_all());
    drop(file);
    let removed = fs::remove_file(probe);
    writable
        .and(removed)
        .map_err(|_| Error("companion_directory_unavailable"))
}
fn free_bytes(root: &Path) -> Result<u64, Error> {
    let name = CString::new(root.as_os_str().as_bytes())
        .map_err(|_| Error("companion_invalid_directory"))?;
    let mut stats = std::mem::MaybeUninit::<libc::statvfs>::uninit();
    unsafe {
        if libc::statvfs(name.as_ptr(), stats.as_mut_ptr()) != 0 {
            return Err(Error("companion_disk_unavailable"));
        }
        let stats = stats.assume_init();
        Ok((stats.f_bavail as u64).saturating_mul(stats.f_frsize as u64))
    }
}
fn disk_budget(requested: u64, free: u64) -> Result<u64, Error> {
    let available = free.saturating_sub(DISK_RESERVE) / MIB * MIB;
    let budget = requested.min(available);
    if budget < 64 * MIB {
        return Err(Error("companion_insufficient_space"));
    }
    Ok(budget)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn startup_announcement_accepts_only_exact_loopback() {
        assert_eq!(
            ready_port(b"ottplay-companion: http://127.0.0.1:8793"),
            Some(8793)
        );
        for value in [
            "ottplay-companion: http://0.0.0.0:8793",
            "ottplay-companion: http://127.0.0.1:0",
            "ottplay-companion: http://127.0.0.1:80/path",
            "http://127.0.0.1:80",
            "ottplay-companion: http://127.0.0.1:65536",
        ] {
            assert_eq!(ready_port(value.as_bytes()), None);
        }
    }
    #[test]
    fn small_disks_keep_reference_and_system_reserve() {
        assert_eq!(
            disk_budget(256 * MIB, 76 * MIB),
            Err(Error("companion_insufficient_space"))
        );
        assert_eq!(disk_budget(256 * MIB, 160 * MIB).unwrap(), 64 * MIB);
        assert_eq!(disk_budget(256 * MIB, 200 * MIB).unwrap(), 104 * MIB);
        assert_eq!(disk_budget(256 * MIB, 1024 * MIB).unwrap(), 256 * MIB);
        assert!(disk_budget(256 * MIB, 0).is_err());
    }
    #[test]
    fn configuration_does_not_accept_remote_urls_or_unbounded_caches() {
        assert!(serde_json::from_str::<Configuration>(r#"{"endpoint":"http://server"}"#).is_err());
        for bytes in [0, 64 * MIB - 1, 512 * MIB + 1, u64::MAX] {
            assert!(Configuration {
                enabled: true,
                cache_bytes: bytes
            }
            .validate()
            .is_err());
        }
        assert!(Configuration::default().validate().is_ok());
    }
}

#[cfg(test)]
#[path = "supervisor_tests.rs"]
mod lifecycle_tests;
