//! Installation-bound capability discovery only. Never creates a scene or source.
use super::multiview_service::{
    Capabilities, ServiceBinding, ServiceClient, ServiceError, ServiceRequest, Transport,
    VerifiedHttpsTransport,
};
use plx_base::task::{self, time::Deadline, OffFrame};
use serde_json::Value;
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, TryRecvError},
        Arc,
    },
    thread::JoinHandle,
    time::Duration,
};

const BUDGET: Duration = Duration::from_secs(30);
type Work = Box<dyn FnOnce(&OffFrame) + Send>;

pub(super) struct SharedTransport(Arc<dyn Transport + Send + Sync>);
pub(super) type NativeClient = ServiceClient<SharedTransport>;
impl SharedTransport {
    pub fn new(transport: Arc<dyn Transport + Send + Sync>) -> Self {
        Self(transport)
    }
}
impl Transport for SharedTransport {
    fn send(
        &self,
        worker: &OffFrame,
        binding: &ServiceBinding,
        request: &ServiceRequest,
        deadline: Deadline,
        cancelled: &AtomicBool,
    ) -> Result<Vec<u8>, ServiceError> {
        self.0.send(worker, binding, request, deadline, cancelled)
    }
}

pub(super) trait Retirement {
    fn retired(&self) -> bool;
}
impl Retirement for JoinHandle<()> {
    fn retired(&self) -> bool {
        self.is_finished()
    }
}
pub(super) trait Launcher {
    fn spawn_io(&mut self, work: Work) -> Option<Box<dyn Retirement>>;
}
pub(super) struct NativeLauncher;
impl Launcher for NativeLauncher {
    fn spawn_io(&mut self, work: Work) -> Option<Box<dyn Retirement>> {
        task::spawn_off_frame_keeping("multiview capabilities", work)
            .map(|handle| Box::new(handle) as Box<dyn Retirement>)
    }
}

pub(super) struct Qualified {
    pub generation: u64,
    pub capabilities: Capabilities,
    pub client: Arc<NativeClient>,
}
struct Completion {
    generation: u64,
    result: Result<Qualified, ServiceError>,
}
struct Pending {
    receiver: Receiver<Completion>,
    cancelled: Arc<AtomicBool>,
    retirement: Box<dyn Retirement>,
    reply: Option<Completion>,
    disconnected: bool,
}
#[derive(PartialEq, Eq)]
enum Config {
    Missing,
    Invalid,
    Raw(String),
}
#[derive(PartialEq, Eq)]
struct Observation {
    ready: bool,
    foreground: bool,
    config: Config,
    installation: Option<String>,
}
impl Observation {
    fn capture(
        ready: bool,
        foreground: bool,
        value: Option<&Value>,
        installation: Option<&str>,
    ) -> Self {
        let config = match value {
            None => Config::Missing,
            Some(Value::String(raw)) if raw.is_empty() => Config::Missing,
            Some(Value::String(raw)) if raw.len() <= 4096 => Config::Raw(raw.clone()),
            _ => Config::Invalid,
        };
        Self {
            ready,
            foreground,
            config,
            installation: installation.filter(|s| s.len() <= 256).map(str::to_owned),
        }
    }
}

/// No Debug/Serialize: observations and clients contain protected credentials.
pub(super) struct State {
    pub status: &'static str,
    pub error: Option<&'static str>,
    generation: u64,
    observation: Option<Observation>,
    pending: Option<Pending>,
    qualified: Option<Qualified>,
    attempted: bool,
    shutdown: bool,
    transport: Arc<dyn Transport + Send + Sync>,
    launcher: Box<dyn Launcher>,
}
impl Default for State {
    fn default() -> Self {
        Self::with_io(Arc::new(VerifiedHttpsTransport), Box::new(NativeLauncher))
    }
}
impl State {
    /// Production and owned fixtures share the same state transitions. The
    /// caller supplies IO, never a prequalified client or an admission receipt.
    pub fn with_io(
        transport: Arc<dyn Transport + Send + Sync>,
        launcher: Box<dyn Launcher>,
    ) -> Self {
        Self {
            status: "disabled",
            error: None,
            generation: 1,
            observation: None,
            pending: None,
            qualified: None,
            attempted: false,
            shutdown: false,
            transport,
            launcher,
        }
    }
    pub fn qualified(&self) -> Option<&Qualified> {
        self.qualified.as_ref()
    }
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// A captured client is not authority after refresh, backgrounding or rebinding.
    /// Future scene work must recheck this exact client and epoch before dispatch
    /// and again before publishing a completion.
    pub fn is_current(
        &self,
        generation: u64,
        server_epoch: &str,
        client: &Arc<NativeClient>,
    ) -> bool {
        generation != 0
            && generation == self.generation
            && self.qualified.as_ref().is_some_and(|current| {
                current.generation == generation
                    && current.capabilities.server_epoch == server_epoch
                    && Arc::ptr_eq(&current.client, client)
            })
    }

    /// Call each frame with StateReady, actual foreground and protected settings.
    /// A changed observation immediately invalidates prior authority, even while
    /// a cancelled worker is still retiring. No replacement can overlap it.
    pub fn tick(
        &mut self,
        ready: bool,
        foreground: bool,
        protected: Option<&Value>,
        persisted_installation: Option<&str>,
    ) {
        if self.shutdown {
            self.poll();
            self.status = "idle";
            self.error = None;
            return;
        }
        let observation =
            Observation::capture(ready, foreground, protected, persisted_installation);
        if self.observation.as_ref() != Some(&observation) {
            self.invalidate();
            self.observation = Some(observation);
            self.attempted = false;
        }
        self.poll();
        if self.generation == 0 {
            self.status = "unavailable";
            self.error = Some("generation_exhausted");
            return;
        }
        let observation = self.observation.as_ref().expect("observation captured");
        if !observation.ready || !observation.foreground {
            self.status = "idle";
            self.error = None;
            return;
        }
        let raw = match &observation.config {
            Config::Missing => {
                self.status = "disabled";
                self.error = None;
                return;
            }
            Config::Invalid => {
                self.status = "unavailable";
                self.error = Some("invalid_config");
                return;
            }
            Config::Raw(raw) => raw,
        };
        if self.pending.is_some() {
            self.status = "checking";
            return;
        }
        if self.attempted {
            return;
        }
        self.attempted = true;
        let client = (|| {
            let binding =
                ServiceBinding::from_protected_json(raw)?.ok_or(ServiceError::InvalidConfig)?;
            ServiceClient::new(
                binding,
                observation
                    .installation
                    .as_deref()
                    .ok_or(ServiceError::UnpairedInstallation)?,
                SharedTransport::new(Arc::clone(&self.transport)),
            )
        })();
        let client = match client {
            Ok(client) => Arc::new(client),
            Err(error) => {
                self.fail(error);
                return;
            }
        };
        let generation = self.generation;
        let cancelled = Arc::new(AtomicBool::new(false));
        let cancel_worker = Arc::clone(&cancelled);
        let (sender, receiver) = mpsc::sync_channel(1);
        let work = Box::new(move |worker: &OffFrame| {
            // The one original deadline is shared through all transport work;
            // progress/retries never extend it. No task queue exists here.
            let deadline = Deadline::after(worker, BUDGET);
            let result =
                client
                    .capabilities(worker, deadline, &cancel_worker)
                    .map(|capabilities| Qualified {
                        generation,
                        capabilities,
                        client,
                    });
            let _ = sender.send(Completion { generation, result });
        });
        match self.launcher.spawn_io(work) {
            Some(retirement) => {
                self.pending = Some(Pending {
                    receiver,
                    cancelled,
                    retirement,
                    reply: None,
                    disconnected: false,
                });
                self.status = "checking";
                self.error = None;
            }
            None => self.fail(ServiceError::ResourceLimit),
        }
    }
    /// Explicit capability refresh invalidates the old epoch/client immediately.
    /// An epoch change can therefore never retain a future scene driver binding.
    pub fn refresh(&mut self) {
        if self.shutdown {
            return;
        }
        self.invalidate();
        self.attempted = false;
        self.status = "idle";
    }
    /// Terminal local invalidation. Retain the cancelled task/receiver until
    /// shutdown_pending observes actual retirement; never restart discovery.
    pub fn shutdown(&mut self) {
        if !self.shutdown {
            self.shutdown = true;
            self.invalidate();
            self.observation = None;
            self.attempted = true;
        }
        self.status = "idle";
        self.error = None;
    }
    pub fn shutdown_pending(&mut self) -> bool {
        self.poll();
        self.pending.is_some()
    }

    fn invalidate(&mut self) {
        self.generation = if self.generation == 0 {
            0
        } else {
            self.generation.checked_add(1).unwrap_or(0)
        };
        // Zero is a permanent exhausted sentinel, never recycled.
        if let Some(pending) = &self.pending {
            pending.cancelled.store(true, Ordering::Release);
        }
        self.qualified = None;
        self.error = None;
    }
    fn poll(&mut self) {
        let Some(pending) = self.pending.as_mut() else {
            return;
        };
        if pending.reply.is_none() && !pending.disconnected {
            match pending.receiver.try_recv() {
                Ok(reply) => pending.reply = Some(reply),
                Err(TryRecvError::Disconnected) => pending.disconnected = true,
                Err(TryRecvError::Empty) => {}
            }
        }
        if !pending.retirement.retired() {
            return;
        }
        let mut pending = self.pending.take().expect("retired pending");
        // A worker may publish immediately before exit, after the first poll.
        if pending.reply.is_none() {
            pending.reply = pending.receiver.try_recv().ok();
        }
        if pending.cancelled.load(Ordering::Acquire) {
            return;
        }
        match pending.reply {
            Some(reply) if reply.generation == self.generation && self.generation != 0 => {
                match reply.result {
                    Ok(qualified) => {
                        self.status = if qualified.capabilities.max_inputs == 0 {
                            "backend_unavailable"
                        } else {
                            "capabilities_available"
                        };
                        self.error = None;
                        self.qualified = Some(qualified);
                    }
                    Err(error) => self.fail(error),
                }
            }
            Some(_) => {}
            None => self.fail(ServiceError::Transport),
        }
    }
    fn fail(&mut self, error: ServiceError) {
        self.qualified = None;
        self.status = "unavailable";
        self.error = Some(match error {
            ServiceError::InvalidConfig => "invalid_config",
            ServiceError::UnpairedInstallation => "unpaired_installation",
            ServiceError::Unauthorized | ServiceError::Forbidden => "authentication_failed",
            ServiceError::Cancelled => "cancelled",
            ServiceError::Deadline => "deadline",
            _ => "service_unavailable",
        });
    }
}
impl Drop for State {
    fn drop(&mut self) {
        if let Some(pending) = &self.pending {
            pending.cancelled.store(true, Ordering::Release);
        }
    }
}

#[cfg(test)]
#[path = "ott_multiview_connection_tests.rs"]
mod tests;
