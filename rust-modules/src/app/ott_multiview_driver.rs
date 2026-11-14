//! Single-owner native/service bridge. No capability or native picture is admitted by this module.
//! Service requests carry server opaque references plus exact Favorite digest, never native URLs.
use super::*;
use super::{multiview as scene, multiview_service as service, multiview_sources as sources};
use crate::app::ott_remote::multiview as wire;
use plx_base::task::{self, time::Deadline, OffFrame};
use plx_data::ott::multiview::{
    AdmissionOutcome, AudioPolicy, Layout, ResolveTicket, SceneId, SlotId,
};
use plx_media::route;
use std::{
    collections::VecDeque,
    sync::mpsc,
    thread::JoinHandle,
    time::{Duration, Instant},
};

type Client = super::multiview_connection::NativeClient;
trait ServiceOps: Send + Sync {
    fn create(
        &self,
        off: &OffFrame,
        native: service::NativeExpected,
        profile: &str,
        geometry: service::Geometry,
        id: service::RequestId,
        deadline: Deadline,
        cancel: &AtomicBool,
    ) -> Result<service::StatusResponse, service::ServiceError>;
    fn sources(
        &self,
        off: &OffFrame,
        context: service::Binding,
        cursor: Option<String>,
        deadline: Deadline,
        cancel: &AtomicBool,
    ) -> Result<service::SourcesResponse, service::ServiceError>;
    fn status(
        &self,
        off: &OffFrame,
        epoch: &str,
        session: &str,
        native: service::NativeExpected,
        deadline: Deadline,
        cancel: &AtomicBool,
    ) -> Result<service::StatusResponse, service::ServiceError>;
    fn play(
        &self,
        off: &OffFrame,
        context: service::Binding,
        item: &service::CatalogItem,
        next_native: Option<service::NativeExpected>,
        catalog: &str,
        remaining: u32,
        grant_id: service::RequestId,
        play_id: service::RequestId,
        deadline: Deadline,
        cancel: &AtomicBool,
    ) -> Result<Played, service::ServiceError>;
    fn control(
        &self,
        off: &OffFrame,
        context: service::Binding,
        operation: service::ControlOp,
        next_native: Option<service::NativeExpected>,
        id: service::RequestId,
        deadline: Deadline,
        cancel: &AtomicBool,
    ) -> Result<service::StatusResponse, service::ServiceError>;
}
struct ClientIo {
    client: Arc<Client>,
    capabilities: service::Capabilities,
}
impl ServiceOps for ClientIo {
    fn create(
        &self,
        off: &OffFrame,
        native: service::NativeExpected,
        profile: &str,
        geometry: service::Geometry,
        id: service::RequestId,
        deadline: Deadline,
        cancel: &AtomicBool,
    ) -> Result<service::StatusResponse, service::ServiceError> {
        self.client.create_session(
            off,
            &self.capabilities,
            native,
            profile,
            geometry,
            id,
            deadline,
            cancel,
        )
    }
    fn sources(
        &self,
        off: &OffFrame,
        context: service::Binding,
        cursor: Option<String>,
        deadline: Deadline,
        cancel: &AtomicBool,
    ) -> Result<service::SourcesResponse, service::ServiceError> {
        self.client
            .sources(off, context, cursor, &self.capabilities, deadline, cancel)
    }
    fn status(
        &self,
        off: &OffFrame,
        epoch: &str,
        session: &str,
        native: service::NativeExpected,
        deadline: Deadline,
        cancel: &AtomicBool,
    ) -> Result<service::StatusResponse, service::ServiceError> {
        self.client
            .status(off, epoch, session, native, deadline, cancel)
    }
    fn play(
        &self,
        off: &OffFrame,
        context: service::Binding,
        item: &service::CatalogItem,
        next_native: Option<service::NativeExpected>,
        catalog: &str,
        remaining: u32,
        grant_id: service::RequestId,
        play_id: service::RequestId,
        deadline: Deadline,
        cancel: &AtomicBool,
    ) -> Result<Played, service::ServiceError> {
        let grant = self.client.input_grant(
            off,
            context.clone(),
            item,
            next_native,
            catalog,
            remaining,
            &self.capabilities,
            grant_id,
            deadline,
            cancel,
        )?;
        let committed = self.client.control(
            off,
            grant.context.clone(),
            service::ControlOp::Play {
                input_grant: grant.input_grant,
            },
            None,
            &self.capabilities,
            play_id,
            deadline,
            cancel,
        )?;
        if !committed.accepted {
            return Err(service::ServiceError::RevisionConflict);
        }
        let status = self.client.status(
            off,
            &committed.context.server_epoch,
            &committed.context.session_id,
            committed.context.native_expected.clone(),
            deadline,
            cancel,
        )?;
        Ok(Played {
            committed: committed.context,
            status,
        })
    }
    fn control(
        &self,
        off: &OffFrame,
        context: service::Binding,
        operation: service::ControlOp,
        next_native: Option<service::NativeExpected>,
        id: service::RequestId,
        deadline: Deadline,
        cancel: &AtomicBool,
    ) -> Result<service::StatusResponse, service::ServiceError> {
        let committed = self.client.control(
            off,
            context.clone(),
            operation,
            next_native,
            &self.capabilities,
            id,
            deadline,
            cancel,
        )?;
        self.client.status(
            off,
            &committed.context.server_epoch,
            &committed.context.session_id,
            committed.context.native_expected,
            deadline,
            cancel,
        )
    }
}
#[derive(Clone)]
struct Origin {
    client: Arc<Client>,
    capabilities: service::Capabilities,
    io: Arc<dyn ServiceOps>,
}
const RECEIPTS: usize = 128;
const EFFECTS: usize = 32;
const BUDGET: Duration = Duration::from_secs(30);

#[derive(Clone, PartialEq, Eq)]
struct Fence {
    runtime: String,
    connection: u64,
    authority: u64,
    profiles: u64,
}
impl Fence {
    fn capture(runtime: &Runtime) -> Self {
        Self {
            runtime: runtime.remote.runtime().into(),
            connection: runtime.multiview_connection.generation(),
            authority: runtime.multiview_authority_epoch,
            profiles: runtime.profile_edit_epoch,
        }
    }
    fn current(&self, runtime: &Runtime) -> bool {
        self.authority != 0
            && self.profiles != 0
            && self.runtime == runtime.remote.runtime()
            && self.connection == runtime.multiview_connection.generation()
            && self.authority == runtime.multiview_authority_epoch
            && self.profiles == runtime.profile_edit_epoch
            && runtime.multiview_connection.qualified().is_some()
    }
}
#[derive(Clone)]
struct Joined {
    token: String,
    expires: u64,
    fence: Fence,
    expected: wire::Context,
    slot: wire::SlotBinding,
    source: sources::SourceHandle,
    source_id: String,
    item: service::CatalogItem,
    catalog: String,
}
struct Session {
    bindings: [Option<service::Binding>; 9],
    capacity: u8,
    status: service::StatusResponse,
    client: Arc<Client>,
    capabilities: service::Capabilities,
    io: Arc<dyn ServiceOps>,
    fence: Fence,
    lease_deadline: Option<Deadline>,
    admission_deadline: Option<Deadline>,
}
struct Pending {
    invocation: Option<Invocation>,
    request: Option<wire::Request>,
    fence: Fence,
    receiver: mpsc::Receiver<Result<Reply, wire::Reason>>,
    thread: Option<JoinHandle<()>>,
    cancel: Arc<AtomicBool>,
    mutating: bool,
    kind: Kind,
    reply: Option<Result<Reply, wire::Reason>>,
    origin: Option<Origin>,
    sample: Option<task::time::ClockSample>,
    admission_deadline: Option<Deadline>,
}
enum Kind {
    Create {
        capacity: u8,
        initial_audio: Option<scene::Effect>,
    },
    Sources {
        slot: wire::SlotBinding,
        filters: wire::SourceFilters,
    },
    Metadata {
        joined: Joined,
        ticket: ResolveTicket,
    },
    Play {
        input: Box<sources::AuthorizedInput>,
        ticket: ResolveTicket,
        joined: Joined,
    },
    Poll,
    Off,
    Cleanup {
        session_id: String,
        server_epoch: String,
    },
    Effect(EffectAck),
}
enum EffectAck {
    Focus,
    Audio {
        scene: SceneId,
        revision: u64,
        owner: Option<SlotId>,
    },
    Stop(ResolveTicket),
    Layout,
    Renew,
}
struct Played {
    committed: service::Binding,
    status: service::StatusResponse,
}
enum Reply {
    Played(Played),
    Status(service::StatusResponse),
    Sources(service::SourcesResponse),
    Metadata(sources::PreparedMetadata),
}
impl Reply {
    fn into_status(self) -> Option<service::StatusResponse> {
        match self {
            Self::Status(status) => Some(status),
            Self::Played(played) => Some(played.status),
            _ => None,
        }
    }
}

struct Cleanup {
    origin: Origin,
    status: service::StatusResponse,
    fence: Fence,
}

pub(super) struct Driver {
    shutting_down: bool,
    retiring: bool,
    admitted_picture: Option<route::PlaybackIdentity>,
    initial_audio_pending: bool,
    pending: Option<Box<Pending>>,
    session: Option<Box<Session>>,
    receipts: VecDeque<Joined>,
    cleanup: VecDeque<Cleanup>,
    inputs: [Option<Box<sources::AuthorizedInput>>; 9],
    origin: Option<Origin>,
    #[cfg(test)]
    metadata_fixture:
        Option<fn(sources::SourceWork) -> Result<sources::PreparedMetadata, sources::Error>>,
    clock_now_ms: u64,
    clock_sample: Option<task::time::ClockSample>,
    last_poll: Instant,
    last_renew: Instant,
    audio_mode: service::AudioPolicy,
    selected: [Option<Box<Joined>>; 9],
    serial: u64,
    output_offered: Option<String>,
    uncertain: bool,
}
impl Default for Driver {
    fn default() -> Self {
        Self {
            shutting_down: false,
            retiring: false,
            admitted_picture: None,
            initial_audio_pending: false,
            pending: None,
            session: None,
            receipts: VecDeque::new(),
            cleanup: VecDeque::new(),
            inputs: std::array::from_fn(|_| None),
            origin: None,
            #[cfg(test)]
            metadata_fixture: None,
            clock_now_ms: 0,
            clock_sample: None,
            last_poll: Instant::now(),
            last_renew: Instant::now(),
            audio_mode: service::AudioPolicy::FollowsFocus,
            selected: std::array::from_fn(|_| None),
            serial: 1,
            output_offered: None,
            uncertain: false,
        }
    }
}
impl Driver {
    fn origin_for(&self, runtime: &Runtime) -> Option<Origin> {
        runtime.multiview_connection.qualified().map(|q| Origin {
            client: q.client.clone(),
            capabilities: q.capabilities.clone(),
            io: Arc::new(ClientIo {
                client: q.client.clone(),
                capabilities: q.capabilities.clone(),
            }),
        })
    }
    fn fence(&self, runtime: &Runtime) -> Fence {
        Fence::capture(runtime)
    }
    fn nonce(&mut self, runtime: &Runtime) -> Result<String, wire::Reason> {
        let next = self
            .serial
            .checked_add(1)
            .ok_or(wire::Reason::LimitExceeded)?;
        let entropy = runtime.entropy.ok_or(wire::Reason::ReceiptUnavailable)?;
        let bytes = serde_json::to_vec(&(entropy, runtime.remote.runtime(), self.serial))
            .map_err(|_| wire::Reason::Unavailable)?;
        self.serial = next;
        Ok(plx_data::ott::favorites_wire::hash(&bytes)[..32].into())
    }
    pub fn command(
        &mut self,
        runtime: &mut Runtime,
        app: &App,
        invocation: &Invocation,
    ) -> Option<Value> {
        if invocation.action != "multiview" {
            return None;
        }
        self.clock_sample = runtime.clock_sample;
        self.clock_now_ms = runtime.now_ms();
        let request = wire::Request::decode(invocation.multiview_projection()?).ok()?;
        let reply_runtime = runtime.remote.runtime().to_owned();
        let reject = |reason| request.rejected(&reply_runtime, reason).ok();
        if self.shutting_down || self.retiring {
            return reject(wire::Reason::Unavailable);
        }
        if !runtime.remote.invocation_is_current(invocation) {
            return reject(wire::Reason::Expired);
        }
        if runtime.now_ms() >= invocation.deadline_ms {
            return reject(wire::Reason::Expired);
        }
        if input_policy::remote(runtime, invocation).is_err() {
            return reject(wire::Reason::PolicyRestricted);
        }
        if request
            .runtime()
            .is_some_and(|value| value != runtime.remote.runtime())
        {
            return reject(wire::Reason::RuntimeChanged);
        }
        if matches!(request.operation(), wire::Operation::Status) {
            return request
                .status_response(runtime.remote.runtime(), snapshot(runtime, self.audio_mode))
                .ok();
        }
        if self.initial_audio_pending && !matches!(request.operation(), wire::Operation::Off(_)) {
            return reject(wire::Reason::Busy);
        }
        if self.pending.is_some()
            || !self.cleanup.is_empty()
            || self.uncertain
            || runtime.multiview_effects.len() >= EFFECTS
        {
            return reject(wire::Reason::Busy);
        }
        if request.expected() != Some(&context(runtime)) {
            return reject(wire::Reason::SceneChanged);
        }
        if let Some(slot) = request.slot() {
            if !slot_current(runtime, slot) {
                return reject(wire::Reason::SlotChanged);
            }
        }
        match self.dispatch(runtime, app, invocation.clone(), request.clone()) {
            Ok(()) if self.pending.is_none() => request
                .accepted(runtime.remote.runtime(), wire::Outcome::Requested)
                .ok(),
            Ok(()) => None,
            Err(reason) => request.rejected(runtime.remote.runtime(), reason).ok(),
        }
    }
    fn dispatch(
        &mut self,
        runtime: &mut Runtime,
        app: &App,
        invocation: Invocation,
        request: wire::Request,
    ) -> Result<(), wire::Reason> {
        let fence = self.fence(runtime);
        if !fence.current(runtime) {
            if matches!(request.operation(), wire::Operation::Layout(_)) {
                runtime.multiview_connection.refresh();
            }
            return Err(wire::Reason::Unavailable);
        }
        self.origin = self.origin_for(runtime);
        let remaining =
            Duration::from_millis(invocation.deadline_ms.saturating_sub(runtime.now_ms()))
                .min(BUDGET);
        let end = Instant::now() + remaining;
        match request.operation().clone() {
            wire::Operation::Layout(layout) => {
                if self.session.is_some() {
                    runtime
                        .multiview
                        .validate_active_owner(runtime, app)
                        .map_err(|_| wire::Reason::PolicyRestricted)?;
                    let transition = runtime
                        .multiview
                        .set_layout(data_layout(layout))
                        .map_err(|_| wire::Reason::Busy)?;
                    return queue_effects(runtime, transition);
                }
                let origin = self.origin.clone().ok_or(wire::Reason::Unavailable)?;
                let io = origin.io.clone();
                let capabilities = origin.capabilities.clone();
                let data_layout = data_layout(layout);
                let profile = capabilities
                    .output_profiles
                    .iter()
                    .find(|profile| {
                        profile.max_inputs > 0
                            && profile.geometry_modes.contains(&if matches!(
                                layout,
                                wire::Layout::Pip
                            ) {
                                service::GeometryMode::Pip
                            } else {
                                service::GeometryMode::Disjoint
                            })
                    })
                    .ok_or(wire::Reason::Unsupported)?
                    .clone();
                if capabilities.max_inputs == 0 {
                    return Err(wire::Reason::Unsupported);
                }
                let mut coordinator = std::mem::take(&mut runtime.multiview);
                let activation = coordinator
                    .activate(runtime, app, data_layout)
                    .map_err(|_| wire::Reason::PolicyRestricted);
                runtime.multiview = coordinator;
                let activation = activation?;
                let initial_audio = activation.effects.into_iter().find(|effect| {
                    matches!(
                        effect,
                        scene::Effect::Backend(scene::BackendIntent::AudioOwner { .. })
                    )
                });
                self.initial_audio_pending = true;
                self.audio_mode = coordinator_audio_policy(runtime)?;
                let native = native_expected(runtime, SlotId::SLOT_0)?;
                let geometry = geometry(layout, &profile.canvas)?;
                let id = service::RequestId::parse(&self.nonce(runtime)?)
                    .map_err(|_| wire::Reason::Unavailable)?;
                self.spawn(
                    Some(invocation),
                    Some(request),
                    fence,
                    Kind::Create {
                        capacity: profile.max_inputs.min(capabilities.max_inputs),
                        initial_audio,
                    },
                    true,
                    move |off, cancel| {
                        io.create(
                            off,
                            native,
                            &profile.profile_id,
                            geometry,
                            id,
                            deadline(off, end),
                            cancel,
                        )
                        .map(Reply::Status)
                        .map_err(|_| wire::Reason::Unavailable)
                    },
                )
            }
            wire::Operation::Sources { slot, filters } => {
                let session = self.session.as_ref().ok_or(wire::Reason::Unavailable)?;
                let binding = binding(
                    runtime,
                    session,
                    SlotId::new(slot.slot_id).ok_or(wire::Reason::InvalidRequest)?,
                )?;
                let io = session.io.clone();
                let cursor = filters.cursor.clone();
                self.spawn(
                    Some(invocation),
                    Some(request),
                    fence,
                    Kind::Sources { slot, filters },
                    false,
                    move |off, cancel| {
                        io.sources(off, binding, cursor, deadline(off, end), cancel)
                            .map(Reply::Sources)
                            .map_err(|_| wire::Reason::Unavailable)
                    },
                )
            }
            wire::Operation::Play {
                slot,
                source_ticket,
            } => {
                let joined = self
                    .receipts
                    .iter()
                    .find(|row| row.token == source_ticket)
                    .cloned()
                    .ok_or(wire::Reason::SourceExpired)?;
                if runtime.multiview.source_id(joined.source).as_deref() != Some(&joined.source_id)
                    || joined.expires <= runtime.now_ms()
                    || !joined.fence.current(runtime)
                    || joined.expected != context(runtime)
                    || joined.slot != slot
                {
                    return Err(wire::Reason::SourceChanged);
                }
                if !joined.item.protection.known || joined.item.protection.restricted {
                    return Err(wire::Reason::PolicyRestricted);
                }
                let slot = SlotId::new(slot.slot_id).ok_or(wire::Reason::InvalidRequest)?;
                if runtime.multiview.slot_status(slot).ok()
                    != Some(plx_data::ott::multiview::SlotStatus::Empty)
                {
                    return Err(wire::Reason::Unsupported);
                }
                let capacity = self
                    .session
                    .as_ref()
                    .ok_or(wire::Reason::Unavailable)?
                    .capacity;
                if self
                    .inputs
                    .iter()
                    .enumerate()
                    .filter(|(index, input)| *index != slot.index() && input.is_some())
                    .count()
                    >= usize::from(capacity)
                {
                    return Err(wire::Reason::LimitExceeded);
                }
                let mut coordinator = std::mem::take(&mut runtime.multiview);
                let transition = (|| {
                    coordinator
                        .validate_active_owner(runtime, app)
                        .map_err(|_| wire::Reason::PolicyRestricted)?;
                    coordinator
                        .bind_source(slot, joined.source)
                        .map_err(|_| wire::Reason::Busy)?;
                    coordinator
                        .begin_play(runtime, app, slot)
                        .map_err(|_| wire::Reason::PolicyRestricted)
                })();
                runtime.multiview = coordinator;
                let mut transition = transition?;
                let effect = transition.effects.pop().ok_or(wire::Reason::Unavailable)?;
                let scene::Effect::PrepareMetadata { ticket, work } = effect else {
                    return Err(wire::Reason::Unavailable);
                };
                let work_cancel = work.cancellation();
                #[cfg(test)]
                let fixture = self.metadata_fixture;
                let started = self.spawn(
                    Some(invocation),
                    Some(request),
                    fence,
                    Kind::Metadata { joined, ticket },
                    false,
                    move |off, _| {
                        #[cfg(test)]
                        if let Some(fixture) = fixture {
                            return fixture(work)
                                .map(Reply::Metadata)
                                .map_err(|_| wire::Reason::SourceUnavailable);
                        }
                        work.prepare_metadata(off)
                            .map(Reply::Metadata)
                            .map_err(|_| wire::Reason::SourceUnavailable)
                    },
                );
                if started.is_ok() {
                    if let Some(pending) = self.pending.as_mut() {
                        pending.cancel = work_cancel;
                    }
                }
                started
            }
            wire::Operation::Focus(slot) => {
                runtime
                    .multiview
                    .validate_active_owner(runtime, app)
                    .map_err(|_| wire::Reason::PolicyRestricted)?;
                let transition = runtime
                    .multiview
                    .focus(SlotId::new(slot.slot_id).ok_or(wire::Reason::InvalidRequest)?)
                    .map_err(|_| wire::Reason::Busy)?;
                queue_effects(runtime, transition)
            }
            wire::Operation::Audio(policy) => {
                runtime
                    .multiview
                    .validate_active_owner(runtime, app)
                    .map_err(|_| wire::Reason::PolicyRestricted)?;
                let (model, mode) = match policy {
                    wire::AudioPolicy::FollowsFocus => (
                        AudioPolicy::FollowsFocus,
                        service::AudioPolicy::FollowsFocus,
                    ),
                    wire::AudioPolicy::Muted => {
                        (AudioPolicy::Explicit(None), service::AudioPolicy::Muted)
                    }
                    wire::AudioPolicy::Slot(slot) => (
                        AudioPolicy::Explicit(Some(
                            SlotId::new(slot.slot_id).ok_or(wire::Reason::InvalidRequest)?,
                        )),
                        service::AudioPolicy::Slot,
                    ),
                };
                let transition = runtime
                    .multiview
                    .set_audio_policy(model)
                    .map_err(|_| wire::Reason::Busy)?;
                self.audio_mode = mode;
                queue_effects(runtime, transition)
            }
            wire::Operation::Stop(slot) => {
                runtime
                    .multiview
                    .validate_active_owner(runtime, app)
                    .map_err(|_| wire::Reason::PolicyRestricted)?;
                let transition = runtime
                    .multiview
                    .close_slot(SlotId::new(slot.slot_id).ok_or(wire::Reason::InvalidRequest)?)
                    .map_err(|_| wire::Reason::Busy)?;
                queue_effects(runtime, transition)
            }
            wire::Operation::Off(_) => {
                let session = self.session.as_ref().ok_or(wire::Reason::Unavailable)?;
                let context = binding(
                    runtime,
                    session,
                    runtime.multiview.active_slot().unwrap_or(SlotId::SLOT_0),
                )?;
                let io = session.io.clone();
                let id = service::RequestId::parse(&self.nonce(runtime)?)
                    .map_err(|_| wire::Reason::Unavailable)?;
                self.spawn(
                    Some(invocation),
                    Some(request),
                    fence,
                    Kind::Off,
                    true,
                    move |off, cancel| {
                        io.control(
                            off,
                            context,
                            service::ControlOp::Off {},
                            None,
                            id,
                            deadline(off, end),
                            cancel,
                        )
                        .map(Reply::Status)
                        .map_err(|_| wire::Reason::Unavailable)
                    },
                )
            }
            // No native per-slot transport or unimplemented state transition is advertised.
            _ => Err(wire::Reason::Unsupported),
        }
    }
    fn spawn(
        &mut self,
        invocation: Option<Invocation>,
        request: Option<wire::Request>,
        fence: Fence,
        kind: Kind,
        mutating: bool,
        execute: impl FnOnce(&OffFrame, &AtomicBool) -> Result<Reply, wire::Reason> + Send + 'static,
    ) -> Result<(), wire::Reason> {
        if self.pending.is_some() {
            return Err(wire::Reason::Busy);
        }
        let cancel = Arc::new(AtomicBool::new(false));
        let token = cancel.clone();
        let (sender, receiver) = mpsc::sync_channel(1);
        let thread = task::spawn_off_frame_keeping("multiview driver", move |off| {
            let result = execute(off, &token);
            let _ = sender.send(result);
        })
        .ok_or(wire::Reason::Unavailable)?;
        let admission_deadline = self
            .clock_sample
            .zip(invocation.as_ref())
            .map(|(sample, inv)| {
                sample.deadline_at_unix_ms(
                    sample
                        .unix_ms()
                        .saturating_add(inv.deadline_ms.saturating_sub(self.clock_now_ms)),
                )
            });
        self.pending = Some(Box::new(Pending {
            invocation,
            request,
            fence,
            receiver,
            thread: Some(thread),
            cancel,
            mutating,
            kind,
            reply: None,
            origin: self.origin.clone(),
            sample: self.clock_sample,
            admission_deadline,
        }));
        Ok(())
    }
    /// Called from the actual runtime frame owner. This method never starts a native decoder.
    pub fn tick(&mut self, runtime: &mut Runtime, app: &mut App) {
        if !self.shutting_down
            && !self.retiring
            && self.admitted_picture.as_ref().is_some_and(|identity| {
                !super::multiview_output::admitted_route_current(runtime, app, identity)
            })
        {
            self.retiring = true;
            self.begin_retirement(runtime, app);
        }
        if self.shutting_down || self.retiring {
            self.tick_shutdown(runtime, app);
            return;
        }
        self.clock_sample = runtime.clock_sample;
        self.clock_now_ms = runtime.now_ms();
        // Cancellation has priority over slow metadata/network completion. Only the exact
        // captured ticket and this driver's worker token may be affected.
        runtime.multiview_effects.retain(|effect| match effect {
            scene::Effect::CancelResolve {
                ticket,
                cancellation,
            } => {
                self.cancel_owned_metadata(*ticket, cancellation);
                false
            }
            _ => true,
        });
        self.receipts
            .retain(|row| row.expires > runtime.now_ms() && row.fence.current(runtime));
        let policy_revoked = self.session.is_some() && self.authorize_inputs(runtime, app).is_err();
        if policy_revoked
            || self
                .session
                .as_ref()
                .is_some_and(|s| !s.fence.current(runtime))
            || self
                .inputs
                .iter()
                .flatten()
                .any(|input| !input.current_for(runtime))
        {
            self.retiring = true;
            self.begin_retirement(runtime, app);
            self.tick_shutdown(runtime, app);
            return;
        }
        let Some(pending) = self.pending.as_mut() else {
            if !self.cleanup.is_empty() {
                self.cleanup_one();
            } else if self.drain_effect(runtime).is_err() {
                self.uncertain = true;
                if self.initial_audio_pending {
                    self.retiring = true;
                    self.begin_retirement(runtime, app);
                }
            }
            if self.pending.is_none() && self.cleanup.is_empty() {
                self.poll_service(runtime);
            }
            return;
        };
        let valid = pending.fence.current(runtime)
            && pending.invocation.as_ref().is_none_or(|inv| {
                runtime.now_ms() < inv.deadline_ms && runtime.remote.invocation_is_current(inv)
            });
        if !valid && !pending.mutating {
            pending.cancel.store(true, Ordering::Release);
        }
        if pending.reply.is_none() {
            pending.reply = match pending.receiver.try_recv() {
                Ok(reply) => Some(reply),
                Err(mpsc::TryRecvError::Empty) => None,
                Err(mpsc::TryRecvError::Disconnected) => Some(Err(wire::Reason::Unavailable)),
            };
        }
        if pending
            .thread
            .as_ref()
            .is_some_and(|thread| !thread.is_finished())
            || pending.reply.is_none()
        {
            return;
        }
        let mut pending = self.pending.take().unwrap();
        if let Some(thread) = pending.thread.take() {
            let _ = thread.join();
        }
        let reply = pending.reply.take().unwrap();
        if matches!(&pending.kind, Kind::Cleanup { .. }) {
            match &reply {
                Ok(Reply::Status(status)) if status.state == service::SessionState::Closed => {
                    settle_closed_scene(runtime, status)
                }
                _ => self.uncertain = true,
            }
            return;
        }
        if !valid {
            if let Kind::Play {
                input,
                ticket,
                joined,
            } = pending.kind
            {
                self.inputs[ticket.slot().index()] = Some(input);
                self.selected[ticket.slot().index()] = Some(Box::new(joined));
            }
            self.retiring = true;
            self.begin_retirement(runtime, app);
            if let Some(status) = reply.ok().and_then(Reply::into_status) {
                if let Some(origin) = pending.origin.clone() {
                    self.enqueue_cleanup(Cleanup {
                        origin,
                        status,
                        fence: pending.fence.clone(),
                    });
                } else {
                    self.uncertain = true;
                }
            }
            self.finish(
                runtime,
                pending.invocation,
                pending.request,
                Err(wire::Reason::Expired),
            );
            return;
        }
        self.complete(runtime, app, *pending, reply);
    }
    fn complete(
        &mut self,
        runtime: &mut Runtime,
        app: &mut App,
        pending: Pending,
        reply: Result<Reply, wire::Reason>,
    ) {
        let reply = match reply {
            Ok(Reply::Played(played)) => {
                self.complete_play(runtime, app, pending, played);
                return;
            }
            other => other,
        };
        let result = match reply {
            Err(reason) => {
                // A lost Play reply cannot release the native authorization while the
                // originating service may already be composing this exact input.
                if let Kind::Play {
                    input,
                    ticket,
                    joined,
                } = pending.kind
                {
                    self.inputs[ticket.slot().index()] = Some(input);
                    self.selected[ticket.slot().index()] = Some(Box::new(joined));
                }
                if pending.mutating {
                    self.uncertain = true;
                    self.retiring = true;
                    self.begin_retirement(runtime, app);
                }
                Err(reason)
            }
            Ok(Reply::Status(status)) => {
                let staged = if matches!(
                    &pending.kind,
                    Kind::Create { .. } | Kind::Off | Kind::Cleanup { .. }
                ) {
                    None
                } else {
                    match self
                        .session
                        .as_ref()
                        .and_then(|session| stage_status(session, &status).ok())
                    {
                        Some(bindings) => Some(bindings),
                        None => {
                            self.retiring = true;
                            self.begin_retirement(runtime, app);
                            if let Some(origin) = pending.origin.clone() {
                                self.enqueue_cleanup(Cleanup {
                                    origin,
                                    status,
                                    fence: pending.fence.clone(),
                                });
                            } else {
                                self.uncertain = true;
                            }
                            self.finish(
                                runtime,
                                pending.invocation,
                                pending.request,
                                Err(wire::Reason::SourceChanged),
                            );
                            return;
                        }
                    }
                };
                let candidate_bindings = staged
                    .as_ref()
                    .cloned()
                    .unwrap_or_else(|| std::array::from_fn(|_| None));
                if !matches!(&pending.kind, Kind::Off | Kind::Cleanup { .. })
                    && !self.candidate_source_pairs(runtime, &status, &candidate_bindings, None)
                {
                    self.retiring = true;
                    self.begin_retirement(runtime, app);
                    if let Some(origin) = pending.origin {
                        self.enqueue_cleanup(Cleanup {
                            origin,
                            status,
                            fence: pending.fence,
                        });
                    } else {
                        self.uncertain = true;
                    }
                    self.finish(
                        runtime,
                        pending.invocation,
                        pending.request,
                        Err(wire::Reason::SourceChanged),
                    );
                    return;
                }
                match pending.kind {
                    Kind::Create {
                        capacity,
                        initial_audio,
                    } => {
                        if let Some(origin) = pending.origin.clone() {
                            let lease_deadline = output_deadline(pending.sample, &status);
                            let mut bindings = std::array::from_fn(|_| None);
                            let initial = binding_from_status(&status);
                            let initial_slot = usize::from(initial.native_expected.slot_id);
                            bindings[initial_slot] = Some(initial);
                            self.session = Some(Box::new(Session {
                                bindings,
                                capacity,
                                status,
                                client: origin.client,
                                capabilities: origin.capabilities,
                                io: origin.io,
                                fence: pending.fence.clone(),
                                lease_deadline,
                                admission_deadline: pending.admission_deadline,
                            }));
                            if let Some(effect) = initial_audio
                                .filter(|effect| runtime.multiview.effect_current(runtime, effect))
                            {
                                if runtime.multiview_effects.len() >= EFFECTS {
                                    self.retiring = true;
                                    self.begin_retirement(runtime, app);
                                    self.finish(
                                        runtime,
                                        pending.invocation,
                                        pending.request,
                                        Err(wire::Reason::Busy),
                                    );
                                    return;
                                }
                                runtime.multiview_effects.push_back(effect);
                            }
                            Ok(None)
                        } else {
                            Err(wire::Reason::Unavailable)
                        }
                    }
                    Kind::Off => {
                        if status.state != service::SessionState::Closed {
                            Err(wire::Reason::Busy)
                        } else {
                            self.session = None;
                            self.initial_audio_pending = false;
                            self.inputs = std::array::from_fn(|_| None);
                            self.receipts.clear();
                            self.output_offered = None;
                            settle_closed_scene(runtime, &status);
                            if let (Some(invocation), Some(request)) =
                                (&pending.invocation, &pending.request)
                            {
                                if let Ok(reply) = request
                                    .accepted(runtime.remote.runtime(), wire::Outcome::Applied)
                                {
                                    runtime.remote.complete(invocation.clone(), reply);
                                    return;
                                }
                            }
                            Ok(None)
                        }
                    }
                    Kind::Poll => {
                        for index in 0..9 {
                            let ready = self.inputs[index]
                                .as_ref()
                                .zip(self.selected[index].as_ref())
                                .is_some_and(|(input, joined)| {
                                    input.current_for(runtime)
                                        && status.slots.iter().any(|slot| {
                                            usize::from(slot.slot_id) == index
                                                && slot.state == service::SlotState::Playing
                                                && slot.frames > 0
                                                && slot.source_ref.as_deref()
                                                    == Some(joined.item.source_ref.as_str())
                                                && slot.item_ref.as_deref()
                                                    == Some(joined.item.item_ref.as_str())
                                        })
                                });
                            if ready {
                                if let Some(input) = &self.inputs[index] {
                                    if let Ok(transition) =
                                        runtime.multiview.backend_start_completed(
                                            input.ticket,
                                            AdmissionOutcome::Admitted,
                                        )
                                    {
                                        let _ = queue_effects(runtime, transition);
                                    }
                                }
                            }
                        }
                        if let Some(s) = self.session.as_mut() {
                            s.lease_deadline = output_deadline(pending.sample, &status);
                            s.bindings = staged.clone().unwrap();
                            s.status = status;
                        }
                        Ok(None)
                    }
                    Kind::Effect(ack) => {
                        match ack {
                            EffectAck::Audio {
                                scene,
                                revision,
                                owner,
                            } => {
                                let configured = status.audio.policy == self.audio_mode;
                                let matches = status.audio.slot_id
                                    == owner.map(|slot| slot.index() as u8)
                                    && status.audio.policy == self.audio_mode;
                                let acknowledged = runtime
                                    .multiview
                                    .audio_route_completed(scene, revision, owner, matches)
                                    .is_ok();
                                if configured && acknowledged {
                                    self.initial_audio_pending = false;
                                }
                            }
                            EffectAck::Stop(ticket) => {
                                let closed = status
                                    .slots
                                    .iter()
                                    .find(|slot| usize::from(slot.slot_id) == ticket.slot().index())
                                    .is_some_and(|slot| {
                                        matches!(
                                            slot.state,
                                            service::SlotState::Closed | service::SlotState::Empty
                                        )
                                    });
                                if closed {
                                    self.inputs[ticket.slot().index()] = None;
                                    self.selected[ticket.slot().index()] = None;
                                    let _ = runtime
                                        .multiview
                                        .stop_completed(ticket, scene::StopResult::Stopped);
                                } else {
                                    self.uncertain = true;
                                }
                            }
                            EffectAck::Renew => {
                                self.last_renew = Instant::now();
                                self.output_offered = None;
                            }
                            EffectAck::Layout => {
                                self.output_offered = None;
                            }
                            EffectAck::Focus => {}
                        }
                        if let Some(session) = self.session.as_mut() {
                            session.lease_deadline = output_deadline(pending.sample, &status);
                            session.bindings = staged.clone().unwrap();
                            session.status = status;
                        }
                        Ok(None)
                    }
                    Kind::Cleanup { .. } => Ok(None),
                    _ => Err(wire::Reason::Unavailable),
                }
            }
            Ok(Reply::Played(_)) => unreachable!("Played dispatched before generic completion"),
            Ok(Reply::Sources(response)) => {
                if let Kind::Sources { slot, filters } = pending.kind {
                    if !self.session.as_mut().is_some_and(|session| {
                        store_binding(session, response.context.clone()).is_ok()
                    }) {
                        Err(wire::Reason::SourceChanged)
                    } else {
                        self.join_sources(runtime, &slot, &filters, &response)
                            .map(|items| Some((items, response.next_cursor)))
                    }
                } else {
                    Err(wire::Reason::Unavailable)
                }
            }
            Ok(Reply::Metadata(metadata)) => {
                let Kind::Metadata { joined, ticket } = pending.kind else {
                    self.finish(
                        runtime,
                        pending.invocation,
                        pending.request,
                        Err(wire::Reason::Unavailable),
                    );
                    return;
                };
                let mut coordinator = std::mem::take(&mut runtime.multiview);
                let transition = coordinator.metadata_succeeded(metadata, runtime, app);
                runtime.multiview = coordinator;
                let input = transition
                    .ok()
                    .and_then(|mut t| t.effects.pop())
                    .and_then(|e| {
                        if !runtime.multiview.effect_current(runtime, &e) {
                            return None;
                        }
                        match e {
                            scene::Effect::Backend(scene::BackendIntent::Load {
                                ticket: effect_ticket,
                                input,
                            }) if effect_ticket == ticket && input.ticket == effect_ticket => {
                                Some(input)
                            }
                            _ => None,
                        }
                    });
                let Some(input) = input else {
                    self.finish(
                        runtime,
                        pending.invocation,
                        pending.request,
                        Err(wire::Reason::PolicyRestricted),
                    );
                    return;
                };
                if input.identity_scheme() != joined.item.identity_version
                    || input.identity_digest() != joined.item.favorite_sha256
                {
                    self.finish(
                        runtime,
                        pending.invocation,
                        pending.request,
                        Err(wire::Reason::SourceChanged),
                    );
                    return;
                }
                if let Err(reason) = self.play_service(
                    runtime,
                    pending.invocation.clone(),
                    pending.request.clone(),
                    joined,
                    ticket,
                    Box::new(input),
                ) {
                    self.finish(runtime, pending.invocation, pending.request, Err(reason));
                }
                return;
            }
        };
        self.finish(runtime, pending.invocation, pending.request, result);
    }
    fn complete_play(
        &mut self,
        runtime: &mut Runtime,
        app: &mut App,
        pending: Pending,
        played: Played,
    ) {
        let Kind::Play {
            input,
            ticket,
            joined,
        } = pending.kind
        else {
            self.uncertain = true;
            self.finish(
                runtime,
                pending.invocation,
                pending.request,
                Err(wire::Reason::Unavailable),
            );
            return;
        };
        let effect = scene::Effect::Backend(scene::BackendIntent::Load {
            ticket,
            input: *input,
        });
        let ticket_current = runtime.multiview.effect_current(runtime, &effect);
        let scene::Effect::Backend(scene::BackendIntent::Load { input, .. }) = effect else {
            unreachable!()
        };
        let input = Box::new(input);
        let staged = self
            .session
            .as_ref()
            .and_then(|session| stage_status(session, &played.status).ok());
        let source_current = ticket_current
            && input.reauthorize(runtime, app).is_ok()
            && played_source_matches(runtime, &played, ticket, &joined);
        let playing = played.status.slots.iter().any(|slot| {
            usize::from(slot.slot_id) == ticket.slot().index()
                && slot.state == service::SlotState::Playing
                && slot.frames > 0
        });
        let mut transition = None;
        let accepted = source_current
            && staged.as_ref().is_some_and(|bindings| {
                self.candidate_source_pairs(
                    runtime,
                    &played.status,
                    bindings,
                    Some((input.as_ref(), &joined, &played.committed)),
                )
            })
            && (!playing || {
                transition = runtime
                    .multiview
                    .backend_start_completed(ticket, AdmissionOutcome::Admitted)
                    .ok();
                transition.is_some()
            });
        if !accepted {
            // The service may have attached; keep the original source proof in custody
            // until exact original-session closure, without publishing it as admitted.
            self.inputs[ticket.slot().index()] = Some(input);
            self.selected[ticket.slot().index()] = Some(Box::new(joined));
            self.retiring = true;
            self.begin_retirement(runtime, app);
            if let Some(origin) = pending.origin {
                self.enqueue_cleanup(Cleanup {
                    origin,
                    status: played.status,
                    fence: pending.fence,
                });
            } else {
                self.uncertain = true;
            }
            self.finish(
                runtime,
                pending.invocation,
                pending.request,
                Err(wire::Reason::SourceChanged),
            );
            return;
        }
        self.inputs[ticket.slot().index()] = Some(input);
        self.selected[ticket.slot().index()] = Some(Box::new(joined));
        self.output_offered = None;
        if let Some(session) = self.session.as_mut() {
            session.bindings = staged.unwrap();
            session.lease_deadline = output_deadline(pending.sample, &played.status);
            session.admission_deadline = pending.admission_deadline.or(session.admission_deadline);
            session.status = played.status;
        }
        if let Some(transition) = transition {
            if queue_effects(runtime, transition).is_err() {
                self.retiring = true;
                self.begin_retirement(runtime, app);
                self.finish(
                    runtime,
                    pending.invocation,
                    pending.request,
                    Err(wire::Reason::Busy),
                );
                return;
            }
        }
        self.finish(runtime, pending.invocation, pending.request, Ok(None));
    }
    fn play_service(
        &mut self,
        runtime: &Runtime,
        invocation: Option<Invocation>,
        request: Option<wire::Request>,
        joined: Joined,
        ticket: ResolveTicket,
        input: Box<sources::AuthorizedInput>,
    ) -> Result<(), wire::Reason> {
        if !input.current_for(runtime) {
            return Err(wire::Reason::SourceChanged);
        }
        let session = self.session.as_ref().ok_or(wire::Reason::Unavailable)?;
        let context = binding(runtime, session, ticket.slot())?;
        let next_native = native_expected(runtime, ticket.slot())?;
        if next_native.expected != context.native_expected.expected {
            return Err(wire::Reason::Busy);
        }
        let io = session.io.clone();
        let item = joined.item.clone();
        let catalog = joined.catalog.clone();
        let end = Instant::now()
            + Duration::from_millis(
                invocation
                    .as_ref()
                    .ok_or(wire::Reason::Expired)?
                    .deadline_ms
                    .saturating_sub(runtime.now_ms()),
            )
            .min(BUDGET);
        let grant_id = service::RequestId::parse(&self.nonce(runtime)?)
            .map_err(|_| wire::Reason::Unavailable)?;
        let play_id = service::RequestId::parse(&self.nonce(runtime)?)
            .map_err(|_| wire::Reason::Unavailable)?;
        self.spawn(
            invocation,
            request,
            self.fence(runtime),
            Kind::Play {
                input,
                ticket,
                joined,
            },
            true,
            move |off, cancel| {
                let remaining = end
                    .saturating_duration_since(Instant::now())
                    .as_millis()
                    .min(30_000) as u32;
                if remaining == 0 {
                    return Err(wire::Reason::Expired);
                }
                io.play(
                    off,
                    context,
                    &item,
                    Some(next_native),
                    &catalog,
                    remaining,
                    grant_id,
                    play_id,
                    deadline(off, end),
                    cancel,
                )
                .map(Reply::Played)
                .map_err(|_| wire::Reason::SourceUnavailable)
            },
        )
    }
    fn join_sources(
        &mut self,
        runtime: &Runtime,
        slot: &wire::SlotBinding,
        filters: &wire::SourceFilters,
        response: &service::SourcesResponse,
    ) -> Result<Vec<Value>, wire::Reason> {
        let options = runtime.multiview.source_options();
        let rows = runtime.multiview.source_rows();
        let mut out = Vec::new();
        for option in options {
            if filters
                .provider
                .as_deref()
                .is_some_and(|v| v != option.provider)
                || filters
                    .profile
                    .as_deref()
                    .is_some_and(|v| v != option.profile)
                || filters
                    .query
                    .as_deref()
                    .is_some_and(|v| !option.label.to_lowercase().contains(&v.to_lowercase()))
            {
                continue;
            }
            let matching: Vec<_> = response
                .items
                .iter()
                .filter(|item| {
                    item.identity_version == option.identity_scheme
                        && item.favorite_sha256 == option.favorite_sha256
                })
                .collect();
            if matching.len() != 1 {
                continue;
            }
            let source = rows
                .iter()
                .find(|row| {
                    runtime.multiview.source_id(row.handle).as_deref() == Some(&option.source_id)
                })
                .ok_or(wire::Reason::SourceChanged)?
                .handle;
            let token = format!("s{}", self.nonce(runtime)?);
            let ticket = Joined {
                token: token.clone(),
                expires: runtime.now_ms().saturating_add(30_000),
                fence: self.fence(runtime),
                expected: context(runtime),
                slot: slot.clone(),
                source,
                source_id: option.source_id.clone(),
                item: matching[0].clone(),
                catalog: response.catalog_revision.clone(),
            };
            while self.receipts.len() >= RECEIPTS {
                self.receipts.pop_front();
            }
            self.receipts.push_back(ticket);
            out.push(json!({"source_id":option.source_id,"label":option.label,"provider":option.provider,"profile":option.profile,"kind":option.kind,"ticket":token}));
        }
        Ok(out)
    }
    fn finish(
        &self,
        runtime: &mut Runtime,
        invocation: Option<Invocation>,
        request: Option<wire::Request>,
        result: Result<Option<(Vec<Value>, Option<String>)>, wire::Reason>,
    ) {
        let (Some(invocation), Some(request)) = (invocation, request) else {
            return;
        };
        let response = match result {
            Ok(Some((items, cursor))) => {
                request.sources_response(runtime.remote.runtime(), items, cursor.as_deref())
            }
            Ok(None) => request.accepted(runtime.remote.runtime(), wire::Outcome::Requested),
            Err(reason) => request.rejected(runtime.remote.runtime(), reason),
        };
        if let Ok(response) = response {
            runtime.remote.complete(invocation, response);
        }
    }
    fn cancel_owned_metadata(&self, ticket: ResolveTicket, cancellation: &Arc<AtomicBool>) {
        if self.pending.as_ref().is_some_and(|pending| {
            matches!(&pending.kind,Kind::Metadata {ticket:owned,..} if *owned==ticket)
                && Arc::ptr_eq(&pending.cancel, cancellation)
        }) {
            cancellation.store(true, Ordering::Release);
        }
    }
    fn drain_effect(&mut self, runtime: &mut Runtime) -> Result<(), wire::Reason> {
        let Some(session) = self.session.as_ref() else {
            return Ok(());
        };
        self.audio_mode = coordinator_audio_policy(runtime)?;
        let effect = runtime.multiview_effects.pop_front();
        if let Some(scene::Effect::CancelResolve {
            ticket,
            cancellation,
        }) = &effect
        {
            self.cancel_owned_metadata(*ticket, cancellation);
            return Ok(());
        }
        if effect
            .as_ref()
            .is_some_and(|effect| !runtime.multiview.effect_current(runtime, effect))
        {
            return Ok(());
        }
        let (slot, op, ack) = match effect {
            Some(scene::Effect::Backend(scene::BackendIntent::Focus { slot, .. })) => {
                (slot, service::ControlOp::Focus {}, EffectAck::Focus)
            }
            Some(scene::Effect::Backend(scene::BackendIntent::AudioOwner {
                scene,
                audio_revision,
                owner,
            })) => (
                owner
                    .or_else(|| runtime.multiview.active_slot())
                    .unwrap_or(SlotId::SLOT_0),
                service::ControlOp::Audio {
                    policy: self.audio_mode,
                },
                EffectAck::Audio {
                    scene,
                    revision: audio_revision,
                    owner,
                },
            ),
            Some(scene::Effect::Backend(scene::BackendIntent::Stop { ticket })) => (
                ticket.slot(),
                service::ControlOp::Stop {},
                EffectAck::Stop(ticket),
            ),
            Some(scene::Effect::Backend(scene::BackendIntent::Layout { layout, .. })) => {
                let wire_layout = model_wire_layout(layout);
                let mut geometry = geometry(wire_layout, &session.status.geometry.canvas)?;
                geometry.revision = session
                    .status
                    .geometry
                    .revision
                    .checked_add(1)
                    .ok_or(wire::Reason::LimitExceeded)?;
                (
                    runtime.multiview.active_slot().unwrap_or(SlotId::SLOT_0),
                    service::ControlOp::Layout { geometry },
                    EffectAck::Layout,
                )
            }
            Some(_) => return Err(wire::Reason::Unsupported),
            None if self.last_renew.elapsed() >= Duration::from_secs(20) => (
                runtime.multiview.active_slot().unwrap_or(SlotId::SLOT_0),
                service::ControlOp::Renew {},
                EffectAck::Renew,
            ),
            None => return Ok(()),
        };
        let context = binding(runtime, session, slot)?;
        let next_native = match op {
            service::ControlOp::Focus {}
            | service::ControlOp::Audio { .. }
            | service::ControlOp::Layout { .. }
            | service::ControlOp::Stop {} => Some(native_expected(runtime, slot)?),
            _ => None,
        };
        let io = session.io.clone();
        let fence = session.fence.clone();
        self.origin = Some(Origin {
            client: session.client.clone(),
            capabilities: session.capabilities.clone(),
            io: session.io.clone(),
        });
        let budget = if self.initial_audio_pending {
            session
                .admission_deadline
                .zip(runtime.clock_sample)
                .map(|(deadline, sample)| {
                    deadline
                        .remaining_at_sample(sample)
                        .min(Duration::from_secs(5))
                })
                .unwrap_or(Duration::ZERO)
        } else {
            Duration::from_secs(5)
        };
        if budget.is_zero() {
            return Err(wire::Reason::Expired);
        }
        let id = service::RequestId::parse(&self.nonce(runtime)?)
            .map_err(|_| wire::Reason::Unavailable)?;
        let end = Instant::now() + budget;
        self.spawn(
            None,
            None,
            fence,
            Kind::Effect(ack),
            true,
            move |off, cancel| {
                io.control(
                    off,
                    context,
                    op,
                    next_native,
                    id,
                    deadline(off, end),
                    cancel,
                )
                .map(Reply::Status)
                .map_err(|_| wire::Reason::Unavailable)
            },
        )
    }
    fn poll_service(&mut self, runtime: &Runtime) {
        if self.last_poll.elapsed() < Duration::from_millis(500) {
            return;
        }
        let Some(session) = self.session.as_ref() else {
            return;
        };
        if !session.fence.current(runtime) {
            return;
        }
        let slot = runtime.multiview.active_slot().unwrap_or(SlotId::SLOT_0);
        let Ok(prior) = binding(runtime, session, slot) else {
            return;
        };
        let native = prior.native_expected;
        let io = session.io.clone();
        let epoch = session.status.server_epoch.clone();
        let id = session.status.session_id.clone();
        let fence = session.fence.clone();
        self.origin = Some(Origin {
            client: session.client.clone(),
            capabilities: session.capabilities.clone(),
            io: session.io.clone(),
        });
        self.last_poll = Instant::now();
        let end = Instant::now() + Duration::from_secs(5);
        let _ = self.spawn(None, None, fence, Kind::Poll, false, move |off, cancel| {
            io.status(off, &epoch, &id, native, deadline(off, end), cancel)
                .map(Reply::Status)
                .map_err(|_| wire::Reason::Unavailable)
        });
    }
    fn enqueue_cleanup(&mut self, cleanup: Cleanup) {
        let same = |origin: &Origin, session: &str, epoch: &str| {
            Arc::ptr_eq(&origin.client, &cleanup.origin.client)
                && session == cleanup.status.session_id
                && epoch == cleanup.status.server_epoch
        };
        if let Some(index) = self.cleanup.iter().position(|queued| {
            same(
                &queued.origin,
                &queued.status.session_id,
                &queued.status.server_epoch,
            )
        }) {
            if cleanup_status_advances(&self.cleanup[index].status, &cleanup.status) {
                self.cleanup[index] = cleanup;
            }
            return;
        }
        if self
            .pending
            .as_ref()
            .is_some_and(|pending| match (&pending.kind, &pending.origin) {
                (
                    Kind::Cleanup {
                        session_id,
                        server_epoch,
                    },
                    Some(origin),
                ) => same(origin, session_id, server_epoch),
                _ => false,
            })
        {
            return;
        }
        self.cleanup.push_back(cleanup);
    }
    fn cleanup_one(&mut self) {
        let Some(cleanup) = self.cleanup.pop_front() else {
            return;
        };
        let native = cleanup.status.native_expected.clone();
        let slot = cleanup
            .status
            .slots
            .iter()
            .find(|s| s.slot_id == native.slot_id)
            .map_or(0, |s| s.source_revision);
        let context = service::Binding {
            server_epoch: cleanup.status.server_epoch.clone(),
            session_id: cleanup.status.session_id.clone(),
            backend_expected: cleanup.status.backend_expected.clone(),
            native_expected: native,
            backend_slot_revision: slot,
        };
        let original = self.origin.take();
        self.origin = Some(cleanup.origin.clone());
        let spawned = self.spawn(
            None,
            None,
            cleanup.fence,
            Kind::Cleanup {
                session_id: context.session_id.clone(),
                server_epoch: context.server_epoch.clone(),
            },
            true,
            move |off, cancel| {
                let digest = plx_data::ott::favorites_wire::hash(
                    format!("off:{}:{}", context.server_epoch, context.session_id).as_bytes(),
                );
                let id = service::RequestId::parse(&digest[..32])
                    .map_err(|_| wire::Reason::Unavailable)?;
                // A prior mutating reply may have been lost; read current authority from
                // this same originating server before the exact Off CAS, never retry a write.
                let current = cleanup
                    .origin
                    .io
                    .status(
                        off,
                        &context.server_epoch,
                        &context.session_id,
                        context.native_expected.clone(),
                        Deadline::after(off, Duration::from_secs(3)),
                        cancel,
                    )
                    .map_err(|_| wire::Reason::Unavailable)?;
                if current.state == service::SessionState::Closed {
                    return Ok(Reply::Status(current));
                }
                cleanup
                    .origin
                    .io
                    .control(
                        off,
                        binding_from_status(&current),
                        service::ControlOp::Off {},
                        None,
                        id,
                        Deadline::after(off, Duration::from_secs(3)),
                        cancel,
                    )
                    .map(Reply::Status)
                    .map_err(|_| wire::Reason::Unavailable)
            },
        );
        self.origin = original;
        if spawned.is_err() {
            self.uncertain = true;
        }
    }
    /// Terminal admission boundary. The caller owns the one overall shutdown deadline.
    pub fn begin_shutdown(&mut self, runtime: &mut Runtime, app: &mut App) {
        if self.shutting_down {
            return;
        }
        self.shutting_down = true;
        self.begin_retirement(runtime, app);
    }
    fn begin_retirement(&mut self, runtime: &mut Runtime, app: &mut App) {
        self.receipts.clear();
        self.output_offered = None;
        runtime.multiview_effects.clear();
        if self
            .admitted_picture
            .as_ref()
            .is_some_and(|identity| runtime.multiview_picture.as_ref() == Some(identity))
        {
            runtime.multiview_picture = None;
        }
        app.bridge.publish_multiview_offer(None);
        if let Some(pending) = &self.pending {
            if !pending.mutating {
                pending.cancel.store(true, Ordering::Release);
            }
        }
        // Keep input witnesses owned until the originating service is actually closed.
        for input in self
            .inputs
            .iter()
            .flatten()
            .map(Box::as_ref)
            .chain(self.pending_input())
        {
            input.cancellation().store(true, Ordering::Release);
        }
        if let Some(session) = self.session.take() {
            self.enqueue_cleanup(Cleanup {
                origin: Origin {
                    client: session.client,
                    capabilities: session.capabilities,
                    io: session.io,
                },
                status: session.status,
                fence: session.fence,
            });
        }
    }
    /// Polls only already-owned work and exact-origin cleanup. Never starts reads, renew,
    /// metadata or new scene work. The Runtime's original deadline bounds this drain.
    pub fn tick_shutdown(&mut self, runtime: &mut Runtime, _app: &mut App) {
        if !self.shutting_down && !self.retiring {
            return;
        }
        let Some(pending) = self.pending.as_mut() else {
            if !self.cleanup.is_empty() {
                self.cleanup_one();
            }
            return;
        };
        if pending.reply.is_none() {
            pending.reply = match pending.receiver.try_recv() {
                Ok(reply) => Some(reply),
                Err(mpsc::TryRecvError::Empty) => None,
                Err(mpsc::TryRecvError::Disconnected) => Some(Err(wire::Reason::Unavailable)),
            };
        }
        if pending.reply.is_none()
            || pending
                .thread
                .as_ref()
                .is_some_and(|thread| !thread.is_finished())
        {
            return;
        }
        let mut pending = self.pending.take().unwrap();
        if let Some(thread) = pending.thread.take() {
            let _ = thread.join();
        }
        let reply = pending.reply.take().unwrap().map(|reply| match reply {
            Reply::Played(played) => Reply::Status(played.status),
            other => other,
        });
        let cleanup = matches!(&pending.kind, Kind::Cleanup { .. });
        if let Kind::Play {
            input,
            ticket,
            joined,
        } = pending.kind
        {
            self.inputs[ticket.slot().index()] = Some(input);
            self.selected[ticket.slot().index()] = Some(Box::new(joined));
        }
        match reply {
            Ok(Reply::Status(status)) if status.state == service::SessionState::Closed => {
                settle_closed_scene(runtime, &status);
                self.inputs = std::array::from_fn(|_| None);
                self.selected = std::array::from_fn(|_| None);
                self.uncertain = false;
                self.initial_audio_pending = false;
                self.retiring = false;
                self.admitted_picture = None;
            }
            Ok(Reply::Status(_)) if cleanup => self.uncertain = true,
            Ok(Reply::Status(status)) if pending.mutating => {
                if let Some(origin) = pending.origin.clone() {
                    self.enqueue_cleanup(Cleanup {
                        origin,
                        status,
                        fence: pending.fence.clone(),
                    });
                } else {
                    self.uncertain = true;
                }
            }
            Err(_) if pending.mutating => self.uncertain = true,
            _ => {}
        }
        // No success acknowledgement is fabricated for an interrupted original operation.
        self.finish(
            runtime,
            pending.invocation,
            pending.request,
            Err(wire::Reason::Unavailable),
        );
    }
    pub fn shutdown_pending(&self) -> bool {
        // Uncertainty is not an executable I/O task. It remains closed to new
        // work, but must not spin the caller's drain after all workers retired.
        self.pending.is_some() || !self.cleanup.is_empty()
    }
    pub fn shutdown_confirmed(&self) -> bool {
        !self.uncertain
            && !self.shutdown_pending()
            && self.session.is_none()
            && self.inputs.iter().all(Option::is_none)
            && self.pending_input().is_none()
    }
    /// Called only by the actual output adapter after successful native admission.
    pub fn picture_admitted(
        &mut self,
        runtime: &Runtime,
        app: &App,
        identity: &route::PlaybackIdentity,
        generation: u64,
        session: &str,
        epoch: &str,
        output_generation: &str,
    ) -> Result<(), &'static str> {
        if self.shutting_down
            || self.retiring
            || !super::multiview_output::admitted_route_current(runtime, app, identity)
            || runtime.multiview_picture.as_ref() != Some(identity)
            || !self.output_current(runtime, generation, session, epoch, output_generation)
        {
            return Err("Multiview picture ownership changed");
        }
        self.admitted_picture = Some(identity.clone());
        Ok(())
    }
    /// Root owns native admission; this only transfers the exact Ready service output receipt.
    pub fn take_ready_output(
        &mut self,
        runtime: &Runtime,
    ) -> Option<super::multiview_output::ReadyOutput> {
        if self.shutting_down || self.retiring || self.initial_audio_pending {
            return None;
        }
        let session = self.session.as_ref()?;
        if !session.fence.current(runtime)
            || session.status.state != service::SessionState::Ready
            || self.uncertain
            || !self.inputs_current(runtime)
        {
            return None;
        }
        let generation = session.status.output.as_ref()?.output_generation.clone();
        if self.output_offered.as_ref() == Some(&generation) {
            return None;
        }
        let lease_deadline = session.lease_deadline?;
        let admission_deadline = session.admission_deadline?;
        self.output_offered = Some(generation);
        Some(super::multiview_output::ReadyOutput {
            client: session.client.clone(),
            capabilities: session.capabilities.clone(),
            status: session.status.clone(),
            connection_generation: session.fence.connection,
            lease_deadline,
            admission_deadline,
        })
    }
    fn authorize_inputs(&self, runtime: &mut Runtime, app: &App) -> Result<(), &'static str> {
        let session = self
            .session
            .as_ref()
            .ok_or("Multiview session is unavailable")?;
        if !session.fence.current(runtime) {
            return Err("Multiview service authority changed");
        }
        runtime
            .multiview
            .validate_active_owner(runtime, app)
            .map_err(|_| "Multiview scene is unavailable")?;
        if !self.inputs_current(runtime) {
            return Err("Multiview source proof is unavailable");
        }
        for input in self
            .inputs
            .iter()
            .flatten()
            .map(Box::as_ref)
            .chain(self.pending_input())
        {
            input
                .reauthorize(runtime, app)
                .map_err(|_| "Multiview source authorization changed")?;
        }
        Ok(())
    }
    fn pending_input(&self) -> Option<&sources::AuthorizedInput> {
        match self.pending.as_ref().map(|p| &p.kind) {
            Some(Kind::Play { input, .. }) => Some(input.as_ref()),
            _ => None,
        }
    }
    /// Validate the WHOLE candidate against independently retained source receipts.
    /// A recovery status read never creates a new source/backend revision witness.
    fn candidate_source_pairs(
        &self,
        runtime: &Runtime,
        status: &service::StatusResponse,
        staged: &[Option<service::Binding>; 9],
        prospective: Option<(&sources::AuthorizedInput, &Joined, &service::Binding)>,
    ) -> bool {
        status.slots.iter().all(|slot| {
            let source_bearing = slot.source_ref.is_some()
                || slot.item_ref.is_some()
                || matches!(
                    slot.state,
                    service::SlotState::Playing
                        | service::SlotState::Preparing
                        | service::SlotState::Stalled
                );
            if !source_bearing {
                return true;
            }
            let index = usize::from(slot.slot_id);
            let owned = prospective
                .filter(|(input, _, _)| input.ticket.slot().index() == index)
                .or_else(|| {
                    self.inputs
                        .get(index)
                        .and_then(Option::as_ref)
                        .zip(self.selected.get(index).and_then(Option::as_ref))
                        .zip(
                            self.session
                                .as_ref()
                                .and_then(|session| session.bindings.get(index))
                                .and_then(Option::as_ref),
                        )
                        .map(|((input, joined), witness)| {
                            (input.as_ref(), joined.as_ref(), witness)
                        })
                });
            let Some((input, joined, witness)) = owned else {
                return false;
            };
            let Some(candidate) = staged.get(index).and_then(Option::as_ref) else {
                return false;
            };
            input.current_for(runtime)
                && input.ticket.slot().index() == index
                && input.identity_scheme() == joined.item.identity_version
                && input.identity_digest() == joined.item.favorite_sha256
                && slot.source_revision == witness.backend_slot_revision
                && candidate.backend_slot_revision == witness.backend_slot_revision
                && candidate.server_epoch == witness.server_epoch
                && candidate.session_id == witness.session_id
                && native_source_identity_same(&candidate.native_expected, &witness.native_expected)
                && native_expected(runtime, input.ticket.slot())
                    .ok()
                    .is_some_and(|native| {
                        native_source_identity_same(&native, &witness.native_expected)
                    })
                && slot.source_ref.as_deref() == Some(joined.item.source_ref.as_str())
                && slot.item_ref.as_deref() == Some(joined.item.item_ref.as_str())
        })
    }
    fn inputs_current(&self, runtime: &Runtime) -> bool {
        if self
            .inputs
            .iter()
            .flatten()
            .any(|input| !input.current_for(runtime))
            || self
                .pending_input()
                .is_some_and(|input| !input.current_for(runtime))
        {
            return false;
        }
        self.session.as_ref().is_some_and(|session| {
            self.candidate_source_pairs(runtime, &session.status, &session.bindings, None)
        })
    }
    /// Retaining an already admitted old canvas cannot retain a removed or
    /// replaced input. Geometry/output generations are checked by the output owner.
    pub fn retained_output_sources_current(
        &self,
        runtime: &Runtime,
        previous: &service::StatusResponse,
    ) -> bool {
        let Some(session) = &self.session else {
            return false;
        };
        !self.shutting_down
            && !self.retiring
            && !self.uncertain
            && session.fence.current(runtime)
            && self.inputs_current(runtime)
            && previous.server_epoch == session.status.server_epoch
            && previous.session_id == session.status.session_id
            && previous.native_expected.runtime == session.status.native_expected.runtime
            && previous.native_expected.expected.scene
                == session.status.native_expected.expected.scene
            && self.candidate_source_pairs(runtime, previous, &session.bindings, None)
            && previous
                .slots
                .iter()
                .find(|slot| slot.slot_id == previous.native_expected.slot_id)
                .filter(|slot| slot.source_ref.is_some() || slot.item_ref.is_some())
                .is_none_or(|slot| {
                    session
                        .bindings
                        .get(usize::from(slot.slot_id))
                        .and_then(Option::as_ref)
                        .is_some_and(|binding| {
                            native_source_identity_same(
                                &previous.native_expected,
                                &binding.native_expected,
                            )
                        })
                })
    }
    pub fn admitted_geometry(&self, runtime: &Runtime) -> Option<&service::Geometry> {
        let session = self.session.as_ref()?;
        (!self.initial_audio_pending
            && session.fence.current(runtime)
            && session.status.state == service::SessionState::Ready
            && self.inputs_current(runtime))
        .then_some(&session.status.geometry)
    }
    pub fn output_current(
        &self,
        runtime: &Runtime,
        generation: u64,
        session_id: &str,
        epoch: &str,
        output_generation: &str,
    ) -> bool {
        !self.shutting_down
            && !self.retiring
            && !self.initial_audio_pending
            && self.session.as_ref().is_some_and(|session| {
                session.fence.current(runtime)
                    && session.fence.connection == generation
                    && session.status.session_id == session_id
                    && session.status.server_epoch == epoch
                    && session.status.state == service::SessionState::Ready
                    && session
                        .status
                        .output
                        .as_ref()
                        .is_some_and(|output| output.output_generation == output_generation)
                    && session
                        .lease_deadline
                        .zip(runtime.clock_sample)
                        .is_some_and(|(deadline, sample)| {
                            !deadline.remaining_at_sample(sample).is_zero()
                        })
                    && !self.uncertain
                    && self.inputs_current(runtime)
            })
    }
}
impl Drop for Driver {
    fn drop(&mut self) {
        if let Some(pending) = &self.pending {
            pending.cancel.store(true, Ordering::Release);
        }
        for input in self
            .inputs
            .iter()
            .flatten()
            .map(Box::as_ref)
            .chain(self.pending_input())
        {
            input.cancellation().store(true, Ordering::Release);
        }
    }
}

fn coordinator_audio_policy(runtime: &Runtime) -> Result<service::AudioPolicy, wire::Reason> {
    match runtime.multiview.audio_policy() {
        Some(AudioPolicy::FollowsFocus) => Ok(service::AudioPolicy::FollowsFocus),
        Some(AudioPolicy::Explicit(None)) => Ok(service::AudioPolicy::Muted),
        Some(AudioPolicy::Explicit(Some(_))) => Ok(service::AudioPolicy::Slot),
        None => Err(wire::Reason::Unavailable),
    }
}
fn deadline(off: &OffFrame, end: Instant) -> Deadline {
    Deadline::after(off, end.saturating_duration_since(Instant::now()))
}
fn data_layout(layout: wire::Layout) -> Layout {
    match layout {
        wire::Layout::Single => Layout::Single,
        wire::Layout::Pip => Layout::PictureInPicture,
        wire::Layout::Grid4 => Layout::Grid4,
        wire::Layout::Grid9 => Layout::Grid9,
        wire::Layout::Perimeter { satellites } => Layout::Perimeter { satellites },
    }
}
fn context(runtime: &Runtime) -> wire::Context {
    runtime.multiview.scene_status(runtime).map_or(
        wire::Context {
            scene: "off".into(),
            layout_revision: "off".into(),
            focus_revision: "off".into(),
            audio_revision: "off".into(),
        },
        |s| wire::Context {
            scene: s.scene,
            layout_revision: s.layout_revision,
            focus_revision: s.focus_revision,
            audio_revision: s.audio_revision,
        },
    )
}
fn slot_current(runtime: &Runtime, slot: &wire::SlotBinding) -> bool {
    runtime
        .multiview
        .scene_status(runtime)
        .is_some_and(|status| {
            status.slots.iter().any(|s| {
                s.slot.index() == usize::from(slot.slot_id)
                    && s.slot_generation == slot.slot_generation
                    && s.source_revision == slot.source_revision
            })
        })
}
fn native_expected(
    runtime: &Runtime,
    slot: SlotId,
) -> Result<service::NativeExpected, wire::Reason> {
    let status = runtime
        .multiview
        .scene_status(runtime)
        .ok_or(wire::Reason::SceneChanged)?;
    let row = status
        .slots
        .iter()
        .find(|s| s.slot == slot)
        .ok_or(wire::Reason::SlotChanged)?;
    Ok(service::NativeExpected {
        runtime: status.runtime.clone(),
        expected: service::NativeRevisions {
            scene: status.scene,
            layout_revision: status.layout_revision,
            focus_revision: status.focus_revision,
            audio_revision: status.audio_revision,
        },
        slot_id: slot.index() as u8,
        slot_generation: row.slot_generation.clone(),
        source_revision: row.source_revision.clone(),
    })
}
fn binding(
    runtime: &Runtime,
    session: &Session,
    slot: SlotId,
) -> Result<service::Binding, wire::Reason> {
    if let Some(prior) = &session.bindings[slot.index()] {
        return Ok(prior.clone());
    }
    let row = session
        .status
        .slots
        .iter()
        .find(|s| usize::from(s.slot_id) == slot.index())
        .ok_or(wire::Reason::SlotChanged)?;
    // Only untouched empty slots may be enrolled lazily, using server-authoritative globals.
    if row.source_revision != 0 || row.state != service::SlotState::Empty {
        return Err(wire::Reason::SourceChanged);
    }
    let mut native = native_expected(runtime, slot)?;
    native.expected = session.status.native_expected.expected.clone();
    Ok(service::Binding {
        server_epoch: session.status.server_epoch.clone(),
        session_id: session.status.session_id.clone(),
        backend_expected: session.status.backend_expected.clone(),
        native_expected: native,
        backend_slot_revision: 0,
    })
}
fn binding_from_status(status: &service::StatusResponse) -> service::Binding {
    service::Binding {
        server_epoch: status.server_epoch.clone(),
        session_id: status.session_id.clone(),
        backend_expected: status.backend_expected.clone(),
        native_expected: status.native_expected.clone(),
        backend_slot_revision: status
            .slots
            .iter()
            .find(|slot| slot.slot_id == status.native_expected.slot_id)
            .map_or(0, |slot| slot.source_revision),
    }
}
fn cleanup_status_advances(old: &service::StatusResponse, next: &service::StatusResponse) -> bool {
    old.server_epoch == next.server_epoch
        && old.session_id == next.session_id
        && old.native_expected.runtime == next.native_expected.runtime
        && shared_bundle_advances(
            &old.backend_expected,
            &old.native_expected.expected,
            &next.backend_expected,
            &next.native_expected.expected,
        )
        && old.slots.iter().all(|slot| {
            next.slots.iter().any(|new| {
                new.slot_id == slot.slot_id && new.source_revision >= slot.source_revision
            })
        })
}
fn native_source_identity_same(a: &service::NativeExpected, b: &service::NativeExpected) -> bool {
    a.runtime == b.runtime
        && a.expected.scene == b.expected.scene
        && a.slot_id == b.slot_id
        && a.slot_generation == b.slot_generation
        && a.source_revision == b.source_revision
}
fn native_source_same(a: &service::NativeExpected, b: &service::NativeExpected) -> bool {
    a.runtime == b.runtime
        && a.expected.scene == b.expected.scene
        && a.expected.layout_revision == b.expected.layout_revision
        && a.slot_id == b.slot_id
        && a.slot_generation == b.slot_generation
        && a.source_revision == b.source_revision
}
fn played_source_matches(
    runtime: &Runtime,
    played: &Played,
    ticket: ResolveTicket,
    joined: &Joined,
) -> bool {
    let committed = &played.committed;
    let status = &played.status;
    let Some(native) = native_expected(runtime, ticket.slot()).ok() else {
        return false;
    };
    committed.server_epoch == status.server_epoch
        && committed.session_id == status.session_id
        && committed.native_expected.slot_id as usize == ticket.slot().index()
        && native_source_same(&native, &committed.native_expected)
        && native_source_same(&committed.native_expected, &status.native_expected)
        && committed.backend_expected.runtime == status.backend_expected.runtime
        && committed.backend_expected.scene == status.backend_expected.scene
        && committed.backend_expected.geometry_revision == status.backend_expected.geometry_revision
        && shared_bundle_advances(
            &committed.backend_expected,
            &committed.native_expected.expected,
            &status.backend_expected,
            &status.native_expected.expected,
        )
        && status.slots.iter().any(|slot| {
            slot.slot_id == committed.native_expected.slot_id
                && slot.source_revision == committed.backend_slot_revision
                && slot.source_ref.as_deref() == Some(joined.item.source_ref.as_str())
                && slot.item_ref.as_deref() == Some(joined.item.item_ref.as_str())
                && matches!(
                    slot.state,
                    service::SlotState::Playing
                        | service::SlotState::Preparing
                        | service::SlotState::Stalled
                )
        })
}
fn shared_bundle_advances(
    old: &service::BackendExpected,
    old_native: &service::NativeRevisions,
    next: &service::BackendExpected,
    next_native: &service::NativeRevisions,
) -> bool {
    let pair = |a: u64, b: u64, x: &str, y: &str| b >= a && (a != b || x == y);
    old.runtime == next.runtime
        && old.scene == next.scene
        && old_native.scene == next_native.scene
        && pair(
            old.geometry_revision,
            next.geometry_revision,
            &old_native.layout_revision,
            &next_native.layout_revision,
        )
        && pair(
            old.focus_revision,
            next.focus_revision,
            &old_native.focus_revision,
            &next_native.focus_revision,
        )
        && pair(
            old.audio_revision,
            next.audio_revision,
            &old_native.audio_revision,
            &next_native.audio_revision,
        )
}
fn store_binding(session: &mut Session, binding: service::Binding) -> Result<(), wire::Reason> {
    // A Sources response is not a new scene snapshot: it must carry this exact
    // already observed shared bundle and the known actual target revision.
    let slot = usize::from(binding.native_expected.slot_id);
    if binding.server_epoch != session.status.server_epoch
        || binding.session_id != session.status.session_id
        || binding.backend_expected != session.status.backend_expected
        || binding.native_expected.runtime != session.status.native_expected.runtime
        || binding.native_expected.expected != session.status.native_expected.expected
        || !session.status.slots.iter().any(|row| {
            usize::from(row.slot_id) == slot && row.source_revision == binding.backend_slot_revision
        })
        || session.bindings[slot]
            .as_ref()
            .is_some_and(|old| old != &binding)
    {
        return Err(wire::Reason::SourceChanged);
    }
    session.bindings[slot] = Some(binding);
    Ok(())
}
fn stage_status(
    session: &Session,
    status: &service::StatusResponse,
) -> Result<[Option<service::Binding>; 9], wire::Reason> {
    if status.server_epoch != session.status.server_epoch
        || status.session_id != session.status.session_id
        || status.native_expected.runtime != session.status.native_expected.runtime
        || !shared_bundle_advances(
            &session.status.backend_expected,
            &session.status.native_expected.expected,
            &status.backend_expected,
            &status.native_expected.expected,
        )
        || (status.backend_expected.geometry_revision
            == session.status.backend_expected.geometry_revision
            && status.geometry != session.status.geometry)
    {
        return Err(wire::Reason::SourceChanged);
    }
    let target = binding_from_status(status);
    let index = usize::from(target.native_expected.slot_id);
    if session.bindings[index]
        .as_ref()
        .is_some_and(|old| target.backend_slot_revision < old.backend_slot_revision)
    {
        return Err(wire::Reason::SourceChanged);
    }
    let mut bindings = session.bindings.clone();
    for (i, prior) in bindings.iter_mut().enumerate() {
        if i == index {
            continue;
        }
        let unchanged = prior.as_ref().is_some_and(|old| {
            status.slots.iter().any(|row| {
                usize::from(row.slot_id) == i && row.source_revision == old.backend_slot_revision
            })
        });
        if !unchanged {
            *prior = None;
            continue;
        }
        if let Some(prior) = prior {
            // One authenticated coherent bundle; source fields remain backed by
            // the same snapshot's unchanged per-slot revision, never a new pair.
            prior.backend_expected = status.backend_expected.clone();
            prior.native_expected.expected = status.native_expected.expected.clone();
        }
    }
    bindings[index] = Some(target);
    Ok(bindings)
}

fn geometry(
    layout: wire::Layout,
    canvas: &service::Canvas,
) -> Result<service::Geometry, wire::Reason> {
    use plx_appkit::multiview_layout::{Layout as Geometry, Mode};
    if matches!(layout, wire::Layout::Single) {
        return Ok(service::Geometry {
            canvas: canvas.clone(),
            revision: 1,
            mode: service::GeometryMode::Disjoint,
            rects: vec![service::Rect {
                slot_id: 0,
                x: 0,
                y: 0,
                width: canvas.width,
                height: canvas.height,
                z: 0,
            }],
        });
    }
    let mode = match layout {
        wire::Layout::Pip => Mode::Pip,
        wire::Layout::Grid4 => Mode::Grid4,
        wire::Layout::Grid9 => Mode::Grid9,
        wire::Layout::Single => Mode::Perimeter { satellites: 0 },
        wire::Layout::Perimeter { satellites } => Mode::Perimeter { satellites },
    };
    let raster = Geometry::new(
        plx_ui::Rect::new(0.0, 0.0, canvas.width as f32, canvas.height as f32),
        mode,
        1.0,
    )
    .and_then(|layout| layout.rasterize_even(1))
    .map_err(|_| wire::Reason::Unsupported)?;
    if raster.slots().len() != usize::from(layout.slots()) {
        return Err(wire::Reason::Unsupported);
    }
    Ok(service::Geometry {
        canvas: canvas.clone(),
        revision: 1,
        mode: if matches!(layout, wire::Layout::Pip) {
            service::GeometryMode::Pip
        } else {
            service::GeometryMode::Disjoint
        },
        rects: raster
            .slots()
            .iter()
            .map(|r| service::Rect {
                slot_id: r.id,
                x: r.x,
                y: r.y,
                width: r.width,
                height: r.height,
                z: r.layer,
            })
            .collect(),
    })
}
fn snapshot(runtime: &Runtime, policy: service::AudioPolicy) -> Value {
    let Some(status) = runtime.multiview.scene_status(runtime) else {
        return json!({"context":context_json(&context(runtime)),
        "layout":"off","satellites":0,"focus_slot":null,"audio_policy":"muted","audio_slot":null,"slots":[]});
    };
    let wire_layout = model_wire_layout(runtime.multiview.layout().unwrap_or(Layout::Single));
    let layout = wire_layout.name();
    let satellites = match wire_layout {
        wire::Layout::Perimeter { satellites } => satellites,
        _ => 0,
    };
    let audio = runtime.multiview.audio_route_state();
    json!({"context":context_json(&context(runtime)),"layout":layout,"satellites":satellites,
        "focus_slot":runtime.multiview.active_slot().map(|s|s.index()),"audio_policy":match policy {service::AudioPolicy::FollowsFocus=>"follows_focus",service::AudioPolicy::Muted=>"muted",service::AudioPolicy::Slot=>"slot"},
        "audio_slot":audio.applied.map(|s|s.index()),"slots":status.slots.iter().map(|s|json!({"slot_id":s.slot.index(),
            "slot_generation":s.slot_generation,"source_revision":s.source_revision,"state":s.status,"error":s.error,
            "source":s.source.as_ref().map(|v|json!({"source_id":v.source_id,"label":v.label,"provider":v.provider,"profile":v.profile,"kind":v.kind}))})).collect::<Vec<_>>()})
}
fn context_json(context: &wire::Context) -> Value {
    json!({"scene":context.scene,"layout_revision":context.layout_revision,
    "focus_revision":context.focus_revision,"audio_revision":context.audio_revision})
}

#[cfg(all(test, feature = "hostsim"))]
#[path = "ott_multiview_driver_tests.rs"]
mod tests;

fn output_deadline(
    sample: Option<task::time::ClockSample>,
    status: &service::StatusResponse,
) -> Option<Deadline> {
    let sample = sample?;
    let output = status.output.as_ref()?;
    Some(
        sample.deadline_at_unix_ms(
            sample
                .unix_ms()
                .saturating_add(u64::from(output.lease_expires_in_ms)),
        ),
    )
}

fn queue_effects(runtime: &mut Runtime, transition: scene::Transition) -> Result<(), wire::Reason> {
    if runtime
        .multiview_effects
        .len()
        .saturating_add(transition.effects.len())
        > EFFECTS
    {
        return Err(wire::Reason::Busy);
    }
    runtime.multiview_effects.extend(transition.effects);
    Ok(())
}
fn model_wire_layout(layout: Layout) -> wire::Layout {
    match layout {
        Layout::Single => wire::Layout::Single,
        Layout::PictureInPicture => wire::Layout::Pip,
        Layout::Grid4 => wire::Layout::Grid4,
        Layout::Grid9 => wire::Layout::Grid9,
        Layout::Perimeter { satellites } => wire::Layout::Perimeter { satellites },
    }
}

/// Called by the output owner at initial offer/final native Load and active policy checks.
pub(super) fn authorize_output(runtime: &mut Runtime, app: &App) -> Result<(), &'static str> {
    let driver = std::mem::take(&mut runtime.multiview_driver);
    let result = driver.authorize_inputs(runtime, app);
    runtime.multiview_driver = driver;
    result
}

fn settle_closed_scene(runtime: &mut Runtime, status: &service::StatusResponse) {
    if status.state != service::SessionState::Closed
        || !runtime.multiview.matches_scene_owner(
            &status.native_expected.runtime,
            &status.native_expected.expected.scene,
        )
    {
        return;
    }
    let Ok(transition) = runtime.multiview.close_scene() else {
        return;
    };
    for effect in transition.effects {
        match effect {
            scene::Effect::Backend(scene::BackendIntent::Stop { ticket }) => {
                // A Closed session confirms every input in THAT owned backend session is stopped.
                let _ = runtime
                    .multiview
                    .stop_completed(ticket, scene::StopResult::Stopped);
            }
            _ => {}
        }
    }
    runtime.multiview_effects.clear();
}
