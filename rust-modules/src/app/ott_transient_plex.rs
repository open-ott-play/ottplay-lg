//! Plex commands share Local's transient owner, catalog and resolver slot.
use super::*;
use crate::app::ott_kiosk_stored::{self as stored, BorrowedTarget, TargetRef};
use crate::app::ott_runtime::work_lifecycle::RestoreStartError;
use crate::app::{ott_plex_prepare as prepare, ott_plex_queue as queue};
use plx_base::task::time::Deadline;
use plx_plex::plex::{Client, Metadata};
use queue::{LibraryRequest, PlexRequest, PrepareKind, QueueError, QueueRequest, WireError};

#[derive(Clone, PartialEq, Eq)]
pub(in crate::app::ott_runtime) enum PreparationId {
    Remote(String),
    KioskRestore,
}

#[derive(Clone)]
pub(in crate::app::ott_runtime) enum Purpose {
    Remote {
        invocation: Invocation,
        request: PlexRequest,
    },
    KioskRestore {
        target: stored::Stored,
        ids: queue::RestoreIds,
        expires_ms: u64,
    },
}
impl Purpose {
    pub fn invocation(&self) -> Option<&Invocation> {
        match self {
            Self::Remote { invocation, .. } => Some(invocation),
            _ => None,
        }
    }
    pub fn key(&self) -> PreparationId {
        match self {
            Self::Remote { invocation, .. } => PreparationId::Remote(invocation.id.clone()),
            Self::KioskRestore { .. } => PreparationId::KioskRestore,
        }
    }
    fn is_play(&self) -> bool {
        match self {
            Self::Remote { request, .. } => is_play(request),
            Self::KioskRestore { .. } => true,
        }
    }
    fn current(&self, runtime: &Runtime) -> bool {
        match self {
            Self::Remote { invocation, .. } => {
                runtime.remote.invocation_is_current(invocation)
                    && invocation.deadline_ms > runtime.now_ms()
            }
            Self::KioskRestore {
                target, expires_ms, ..
            } => {
                *expires_ms > runtime.now_ms()
                    && runtime.loaded
                    && !runtime.persistence_uncertain
                    && runtime.source_work_allowed()
                    && !lifecycle::viewer_paused()
                    && durable_queue_matches(runtime, target)
            }
        }
    }
    fn error(&self, runtime: &Runtime, error: WireError) -> Value {
        match self {
            Self::Remote {
                invocation,
                request,
            } => {
                let Ok(version) = queue::state_version(&invocation.action, &invocation.params)
                else {
                    return rejected("Invalid Plex state version");
                };
                error_value_version(runtime.remote.runtime(), request, error, version)
            }
            Self::KioskRestore { .. } => rejected(as_queue_error(error).message()),
        }
    }
    fn complete(self, runtime: &mut Runtime, value: Value) {
        if let Self::Remote { invocation, .. } = self {
            runtime.remote.complete(invocation, value);
        } else {
            if let Some(error) = value
                .get("data")
                .and_then(|data| data.get("error"))
                .and_then(Value::as_str)
            {
                runtime.status = error.into();
            }
            runtime.changed();
        }
    }
}

fn durable_queue_matches(runtime: &Runtime, target: &stored::Stored) -> bool {
    let TargetRef::Queue(expected) = target.target() else {
        return false;
    };
    matches!(stored::borrow_validated(&runtime.saved.settings), Ok(Some(current))
        if matches!(current.target(), BorrowedTarget::Queue(entries)
                if entries.len() == expected.len()
                    && entries.iter().zip(expected).all(|(actual, expected)| actual.equals(expected))))
}

/// Private credential provenance. Native accounts retain only the admitted
/// client receipt and grant scope; their token never enters OTT saved settings.
#[derive(Clone)]
pub(in crate::app::ott_runtime) enum SourceBinding {
    Stored(plx_plex::plex::session::SourceRef),
    NativeAccount(plx_plex::plex::grant::GrantScope),
}

#[derive(Clone)]
pub(in crate::app::ott_runtime) struct Preparation {
    pub owner: OwnerToken,
    pub playback_epoch: u64,
    pub navigation_epoch: u64,
    pub purpose: Purpose,
    pub profile: ProviderProfile,
    pub source: SourceBinding,
    pub client: &'static Client,
    pub authority: Arc<prepare::CapturedAuthority>,
    pub deadline: Deadline,
    pub cancel: Arc<AtomicBool>,
    pub seed: u32,
}
pub(in crate::app::ott_runtime) struct PlexCandidate {
    pub work: Preparation,
    pub ticket: plx_media::route::PlayRequestTicket,
    pub interrupted: Option<InterruptedScan>,
    pub staged: Option<(PlexSession, Catalog)>,
}
impl PlexCandidate {
    pub fn is_play(&self) -> bool {
        self.work.purpose.is_play()
    }
}
pub(in crate::app::ott_runtime) struct PlexSession {
    pub queue: queue::PlexQueue,
    pub authority: Arc<prepare::CapturedAuthority>,
    pub metadata: Arc<Vec<Metadata>>,
    pub occurrences: Vec<usize>,
    pub profile: ProviderProfile,
    pub category: String,
    pub favorites_selected: bool,
    pub borrowed: Option<BorrowedBookmark>,
    pub receipt: Option<LibraryReceipt>,
}
pub(in crate::app::ott_runtime) struct LibraryReceipt {
    pub request: LibraryRequest,
    pub library: queue::LibraryRow,
    pub truncated: bool,
}
#[derive(Clone)]
pub(in crate::app::ott_runtime) struct NativeChoice {
    pub owner: OwnerToken,
    pub index: usize,
    pub authority: Arc<prepare::CapturedAuthority>,
    pub metadata: Arc<Vec<Metadata>>,
    pub metadata_index: usize,
}
impl NativeChoice {
    pub fn item(&self) -> Option<&Metadata> {
        self.metadata.get(self.metadata_index)
    }
    pub fn current(&self) -> bool {
        plx_plex::plex::client_for(self.authority.server)
            .is_some_and(|client| self.authority.matches(client))
    }
}
impl PlexSession {
    pub fn id(&self, index: usize) -> String {
        format!("plex-occurrence:{}:{index}", self.queue.owner().0)
    }
    fn occurrence_index(&self, id: &str) -> Option<usize> {
        let (owner, index) = id.strip_prefix("plex-occurrence:")?.split_once(':')?;
        let canonical = |value: &str| {
            !value.is_empty()
                && value.bytes().all(|b| b.is_ascii_digit())
                && (value.len() == 1 || !value.starts_with('0'))
        };
        if !canonical(owner)
            || !canonical(index)
            || owner.parse::<u64>().ok()? != self.queue.owner().0
        {
            return None;
        }
        let index = index.parse::<usize>().ok()?;
        (index < self.occurrences.len()).then_some(index)
    }
    pub fn choice(&self, id: &str) -> Option<NativeChoice> {
        let index = self.occurrence_index(id)?;
        Some(NativeChoice {
            owner: OwnerToken(self.queue.owner().0),
            index,
            authority: self.authority.clone(),
            metadata: self.metadata.clone(),
            metadata_index: self.occurrences[index],
        })
    }
    fn native_matches(&self, playing: &Playing, app: &App) -> bool {
        self.owns(playing)
            && playing.identity.as_ref().is_some_and(|identity| {
                plx_media::route::playback_identity_matches(&app.player.session, identity)
            })
            && (plx_media::route::playback_request_identity(&app.player.session).is_some_and(
                |(sid, rk)| {
                    sid == self.authority.server
                        && self
                            .queue
                            .records()
                            .get(self.queue.index())
                            .is_some_and(|record| record.id == rk)
                },
            ) || (plx_media::route::cur_sid(&app.player.session) == self.authority.server
                && self
                    .queue
                    .records()
                    .get(self.queue.index())
                    .is_some_and(|record| {
                        record.id == plx_media::route::cur_rk(&app.player.session)
                    })))
    }
    fn owns(&self, playing: &Playing) -> bool {
        playing.transient_owner == Some(OwnerToken(self.queue.owner().0))
            && self.occurrence_index(&playing.selection_id) == Some(self.queue.index())
            && self
                .queue
                .records()
                .get(self.queue.index())
                .is_some_and(|record| {
                    matches!(&playing.entry.media, MediaLocator::Plex { server_id, rating_key }
                    if server_id == &self.authority.machine_id && rating_key == &record.id)
                })
            && playing.profile == self.profile
    }
}

fn is_play(request: &PlexRequest) -> bool {
    matches!(
        request,
        PlexRequest::Queue(QueueRequest::Prepare {
            kind: PrepareKind::Play,
            ..
        }) | PlexRequest::Library(LibraryRequest::Prepare {
            kind: PrepareKind::Play,
            ..
        })
    )
}
fn session(runtime: &Runtime) -> Option<&PlexSession> {
    match runtime.transient.accepted.as_ref()? {
        TransientMedia::Plex(s) => Some(s),
        _ => None,
    }
}
fn session_mut(runtime: &mut Runtime) -> Option<&mut PlexSession> {
    match runtime.transient.accepted.as_mut()? {
        TransientMedia::Plex(s) => Some(s),
        _ => None,
    }
}
fn candidate(runtime: &Runtime) -> Option<&PlexCandidate> {
    match runtime.transient.candidate.as_ref()? {
        Candidate::Plex(c) => Some(c),
        _ => None,
    }
}
fn take_candidate(runtime: &mut Runtime) -> Option<PlexCandidate> {
    if !matches!(runtime.transient.candidate, Some(Candidate::Plex(_))) {
        return None;
    }
    match runtime.transient.candidate.take()? {
        Candidate::Plex(c) => Some(c),
        _ => None,
    }
}
fn error_value_version(
    runtime: &str,
    request: &PlexRequest,
    error: WireError,
    version: queue::StateVersion,
) -> Value {
    // A rejected replacement has no admitted queue to describe. Reporting an
    // empty successful snapshot would break the controller's requested-ID
    // correlation and falsely imply that the previous queue was replaced.
    if matches!(request, PlexRequest::Queue(QueueRequest::Prepare { kind: PrepareKind::Play, .. })) {
        return rejected(as_queue_error(error).message());
    }
    let value = match request {
        PlexRequest::Library(request) => queue::library_error_value(runtime, request, error),
        PlexRequest::Queue(QueueRequest::Prepare {
            kind: PrepareKind::Preview,
            ids,
        }) => queue::preview_error(runtime, ids, as_queue_error(error)),
        _ if version == queue::StateVersion::V1 => {
            queue::empty_error_snapshot(runtime, as_queue_error(error))
        }
        _ => queue::empty_error_snapshot_version(runtime, as_queue_error(error), version),
    };
    value
        .map(ok)
        .unwrap_or_else(|_| rejected("Invalid Plex result"))
}
fn as_queue_error(error: WireError) -> QueueError {
    match error {
        WireError::Queue(error) => error,
        _ => QueueError::StartFailed,
    }
}
fn failure_version(
    version: queue::StateVersion,
    runtime: &str,
    request: &PlexRequest,
    error: QueueError,
) -> Value {
    error_value_version(runtime, request, WireError::Queue(error), version)
}
fn snapshot(runtime: &Runtime, version: queue::StateVersion) -> Value {
    session(runtime)
        .map_or_else(
            || {
                if version == queue::StateVersion::V1 {
                    queue::idle_snapshot(runtime.remote.runtime())
                } else {
                    queue::idle_snapshot_version(runtime.remote.runtime(), version)
                }
            },
            |s| {
                if version == queue::StateVersion::V1 {
                    s.queue.snapshot(runtime.remote.runtime())
                } else {
                    s.queue.snapshot_version(runtime.remote.runtime(), version)
                }
            },
        )
        .map(ok)
        .unwrap_or_else(|_| rejected("Invalid Plex queue state"))
}

pub(in crate::app::ott_runtime) fn command(
    runtime: &mut Runtime,
    app: &mut App,
    inv: &Invocation,
) -> Option<Value> {
    let version = match queue::state_version(&inv.action, &inv.params) {
        Ok(value) => value,
        Err(_) => return Some(rejected("Unsupported or invalid Plex state_version; use integer 2 only for queue state operations")),
    };
    let request = match queue::parse(&inv.action, &inv.params, runtime.remote.runtime()) {
        Ok(value) => value,
        Err(_) => return Some(rejected("Invalid Plex request")),
    };
    if !runtime.remote.invocation_is_current(inv) {
        return Some(failure_version(
            version,
            runtime.remote.runtime(),
            &request,
            QueueError::ContextChanged,
        ));
    }
    // A legacy traversal must not mutate a queue it cannot represent. Public
    // play remains <=500 and may replace it; preview does not traverse it.
    if matches!(
        request,
        PlexRequest::Queue(
            QueueRequest::Status | QueueRequest::Next | QueueRequest::Previous | QueueRequest::Stop
                | QueueRequest::Repeat(_)
        )
    ) && session(runtime).is_some_and(|session| !session.queue.state_fits(version))
    {
        return Some(rejected(
            "This queue requires plex_queue_state_v2 and state_version: 2; upgrade the controller",
        ));
    }
    if matches!(request, PlexRequest::Queue(QueueRequest::Status)) {
        return Some(snapshot(runtime, version));
    }
    if !runtime.loaded || runtime.persistence_uncertain || !playback_allowed(app) {
        return Some(failure_version(
            version,
            runtime.remote.runtime(),
            &request,
            QueueError::Restricted,
        ));
    }
    if let PlexRequest::Queue(QueueRequest::Repeat(repeat)) = request {
        return Some(set_repeat(runtime, app, repeat, version));
    }
    if matches!(request, PlexRequest::Queue(QueueRequest::Stop)) {
        cancel_candidate(runtime, app);
        let owned = runtime
            .playing
            .as_ref()
            .is_some_and(|p| session(runtime).is_some_and(|s| s.native_matches(p, app)));
        if owned {
            runtime.stop_catalog(app);
            plx_media::player::stop_bufferfeed(&mut app.player.session, &mut app.adapters.player);
            plx_media::route::cancel_play(&mut app.player.session);
        }
        if let Some(s) = session_mut(runtime) {
            let _ = s.queue.stop(s.queue.owner());
        }
        runtime.changed();
        return Some(snapshot(runtime, version));
    }
    if let PlexRequest::Queue(QueueRequest::Next | QueueRequest::Previous) = request {
        let direction = if matches!(request, PlexRequest::Queue(QueueRequest::Next)) {
            1
        } else {
            -1
        };
        return step(runtime, app, direction, Some(inv));
    }
    if runtime.transient.candidate.is_some() || runtime.play_target.is_some() {
        return Some(failure_version(
            version,
            runtime.remote.runtime(),
            &request,
            QueueError::Busy,
        ));
    }
    let (profile, source, client, authority) = match remote_binding(runtime) {
        Ok(binding) => binding,
        Err(error) => return Some(failure_version(version, runtime.remote.runtime(), &request, error)),
    };
    let Some(clock) = runtime.clock_sample else {
        return Some(failure_version(
            version,
            runtime.remote.runtime(),
            &request,
            QueueError::Unavailable,
        ));
    };
    let deadline = clock.deadline_at_unix_ms(inv.deadline_ms);
    let purpose = Purpose::Remote {
        invocation: inv.clone(),
        request,
    };
    match begin_preparation(
        runtime,
        purpose.clone(),
        profile,
        source,
        client,
        authority,
        deadline,
    ) {
        Ok(()) => None,
        Err(error) => Some(purpose.error(runtime, error)),
    }
}

/// Accept a guarded attempt, not a claim of native/first-frame playback.
pub(in crate::app::ott_runtime) fn kiosk_restore_queue(
    runtime: &mut Runtime,
    app: &mut App,
    entries: &[Favorite],
) -> Result<(), RestoreStartError> {
    if !runtime.loaded
        || runtime.persistence_uncertain
        || runtime.saving
        || !playback_allowed(app)
        || !runtime.source_work_allowed()
        || lifecycle::viewer_paused()
        || app.player.pending_exit.is_some()
    {
        return Err(RestoreStartError::Busy);
    }
    if runtime.transient.candidate.is_some() || runtime.play_target.is_some() {
        return Err(RestoreStartError::Busy);
    }
    let target = stored::read(&runtime.saved.settings)
        .map_err(|_| RestoreStartError::Invalid)?
        .ok_or(RestoreStartError::ContextChanged)?;
    if !matches!(target.target(), TargetRef::Queue(saved) if saved == entries) {
        return Err(RestoreStartError::ContextChanged);
    }
    let first = entries.first().ok_or(RestoreStartError::Invalid)?;
    let bindings = runtime
        .favorite_bindings(first)
        .map_err(|_| RestoreStartError::Unavailable)?;
    let selection::SelectionOwner::Profile(profile) =
        selection::bind_bound(&bindings, first).map_err(|_| RestoreStartError::ContextChanged)?
    else {
        return Err(RestoreStartError::Invalid);
    };
    if profile.kind != ProviderKind::Plex {
        return Err(RestoreStartError::Invalid);
    }
    let mut ids = Vec::with_capacity(entries.len());
    for entry in entries {
        if entry.origin != first.origin
            || entry.validate().is_err()
            || !matches!(selection::bind_bound(&bindings, entry), Ok(selection::SelectionOwner::Profile(bound)) if bound == profile)
        {
            return Err(RestoreStartError::ContextChanged);
        }
        let plx_data::ott::favorites::Locator::Plex {
            server_id,
            rating_key,
        } = &entry.locator
        else {
            return Err(RestoreStartError::Invalid);
        };
        if server_id != &profile.source {
            return Err(RestoreStartError::ContextChanged);
        }
        ids.push(queue::DecimalId::parse(rating_key).map_err(|_| RestoreStartError::Invalid)?);
    }
    let ids = queue::RestoreIds::new(ids).map_err(|_| RestoreStartError::Invalid)?;
    let profile = profile.clone();
    let sources = runtime
        .saved
        .plex_sources()
        .map_err(|_| RestoreStartError::Unavailable)?;
    let mut matching = sources
        .into_iter()
        .filter(|source| source.machine_id == profile.source);
    let source = matching.next().ok_or(RestoreStartError::Unavailable)?;
    if matching.next().is_some() {
        return Err(RestoreStartError::ContextChanged);
    }
    let client = plx_plex::plex::id_of_machine(&profile.source)
        .and_then(plx_plex::plex::client_for)
        .ok_or(RestoreStartError::Unavailable)?;
    if source.origin().as_ref() != Some(client.origin()) || !client.uses_token(&source.token) {
        return Err(RestoreStartError::ContextChanged);
    }
    let authority = Arc::new(
        prepare::CapturedAuthority::capture(client).map_err(|_| RestoreStartError::Unavailable)?,
    );
    if !authority.matches(client) {
        return Err(RestoreStartError::ContextChanged);
    }
    let clock = runtime.clock_sample.ok_or(RestoreStartError::Unavailable)?;
    let expires_ms = clock.unix_ms().saturating_add(35_000);
    let purpose = Purpose::KioskRestore {
        target,
        ids,
        expires_ms,
    };
    begin_preparation(
        runtime,
        purpose,
        profile,
        SourceBinding::Stored(source),
        client,
        authority,
        clock.deadline_at_unix_ms(expires_ms),
    )
    .map_err(|error| match error {
        WireError::Queue(QueueError::Busy) => RestoreStartError::Busy,
        WireError::Queue(QueueError::Unavailable | QueueError::ServerUnavailable) => {
            RestoreStartError::Unavailable
        }
        WireError::Queue(QueueError::Configuration | QueueError::ContextChanged) => {
            RestoreStartError::ContextChanged
        }
        _ => RestoreStartError::Invalid,
    })
}

// The sole candidate/resolver admission used by both remote and durable restore.
fn begin_preparation(
    runtime: &mut Runtime,
    purpose: Purpose,
    profile: ProviderProfile,
    source: SourceBinding,
    client: &'static Client,
    authority: Arc<prepare::CapturedAuthority>,
    deadline: Deadline,
) -> Result<(), WireError> {
    if runtime.transient.candidate.is_some() || runtime.play_target.is_some() {
        return Err(WireError::Queue(QueueError::Busy));
    }
    let seed = purpose.invocation().map_or(1, |inv| {
        inv.id
            .bytes()
            .fold(
                runtime
                    .entropy
                    .map(|v| u32::from_le_bytes(v[..4].try_into().unwrap()))
                    .unwrap_or(1),
                |seed, byte| seed.rotate_left(5) ^ u32::from(byte),
            )
            .max(1)
    });
    let interrupted = if purpose.is_play() {
        let old = InterruptedScan {
            profile: runtime.effective_profile().cloned(),
            owner: runtime
                .transient
                .accepted
                .as_ref()
                .map(TransientMedia::owner),
            restart: runtime.catalog_loading && !runtime.catalog_complete,
        };
        runtime.playback_epoch = runtime.playback_epoch.wrapping_add(1);
        commands::cancel_pending_play(runtime);
        runtime.cancel_play_request();
        runtime.resume_pending = None;
        runtime.cancel.store(true, Ordering::Release);
        let classic_generation = runtime.generation;
        runtime.generation = runtime.generation.wrapping_add(1).max(1);
        classic_source::rebase(runtime, classic_generation);
        runtime.continuity.rebase(runtime.generation);
        runtime.cancel = Arc::new(AtomicBool::new(false));
        runtime.catalog_next = None;
        runtime.catalog_loading = false;
        Some(old)
    } else {
        None
    };
    let work = Preparation {
        owner: OwnerToken(runtime.generation),
        playback_epoch: runtime.playback_epoch,
        navigation_epoch: runtime.navigation_epoch,
        purpose,
        profile: profile.clone(),
        source,
        client,
        authority,
        deadline,
        cancel: Arc::new(AtomicBool::new(false)),
        seed,
    };
    runtime.transient.candidate = Some(Candidate::Plex(PlexCandidate {
        work: work.clone(),
        ticket: plx_media::route::play_request_ticket(),
        interrupted,
        staged: None,
    }));
    preparation_lifecycle::started(runtime);
    runtime.resolver_next = Some(ResolveRequest {
        local_read: None,
        archive: None,
        generation: runtime.play_generation,
        profile,
        entry: None,
        favorite: None,
        local_proof: None,
        cancel: work.cancel.clone(),
        plex: Some(work),
        plex_choice: None,
    });
    if runtime.launch_resolver().is_err() {
        let candidate = take_candidate(runtime).unwrap();
        restore(runtime, &candidate);
        preparation_lifecycle::failed(
            runtime,
            &candidate,
            WireError::Queue(QueueError::Unavailable),
        );
        return Err(WireError::Queue(QueueError::Unavailable));
    }
    Ok(())
}

pub(in crate::app::ott_runtime) fn run(
    off: &plx_base::task::OffFrame,
    work: &Preparation,
) -> Result<prepare::Proposal, WireError> {
    if !work.authority.matches(work.client) {
        return Err(WireError::Queue(QueueError::ContextChanged));
    }
    let mut permutation = |count| {
        let mut indices: Vec<usize> = (0..count).collect();
        local::shuffle_order(&mut indices, work.seed);
        Ok(indices)
    };
    let mut options = prepare::PrepareOptions {
        normalize: plx_base::unicode::caseless_key,
        permutation: Some(&mut permutation),
    };
    let result = match &work.purpose {
        Purpose::Remote { request, .. } => prepare::prepare(
            off,
            work.client,
            work.deadline,
            &|| work.cancel.load(Ordering::Acquire),
            request,
            queue::OwnerToken(work.owner.0),
            &mut options,
        ),
        Purpose::KioskRestore { ids, .. } => prepare::prepare_restore(
            off,
            work.client,
            work.deadline,
            &|| work.cancel.load(Ordering::Acquire),
            ids,
            queue::OwnerToken(work.owner.0),
        ),
    };
    if !work.authority.matches(work.client) {
        return Err(WireError::Queue(QueueError::ContextChanged));
    }
    result
}

fn restore(runtime: &mut Runtime, candidate: &PlexCandidate) {
    let Some(old) = &candidate.interrupted else {
        return;
    };
    if old.restart
        && runtime.effective_profile() == old.profile.as_ref()
        && runtime
            .transient
            .accepted
            .as_ref()
            .map(TransientMedia::owner)
            == old.owner
    {
        runtime.catalog_next = old.profile.clone();
        runtime.catalog_loading = true;
        runtime.catalog_complete = false;
        if runtime.catalog_work_allowed() {
            runtime.cancel = Arc::new(AtomicBool::new(false));
            runtime.launch_catalog();
        }
    }
}
fn current(runtime: &Runtime, c: &PlexCandidate) -> bool {
    c.work.owner == OwnerToken(runtime.generation)
        && !c.work.cancel.load(Ordering::Acquire)
        && c.work.purpose.current(runtime)
        && plx_media::route::play_request_is_current(c.ticket)
        && runtime
            .saved
            .profiles
            .profiles
            .iter()
            .any(|p| p == &c.work.profile && p.enabled)
        && source_current(runtime, &c.work)
        && c.work.authority.matches(c.work.client)
        && plx_plex::plex::client_for(c.work.authority.server)
            .is_some_and(|v| c.work.authority.matches(v))
}

/// An active Plex profile is an explicit choice. Otherwise every configured
/// enabled Plex profile participates, including unavailable ones: never guess
/// another account just because its server currently answers.
fn selected_profile<'a>(runtime: &'a Runtime, machine: Option<&str>) -> Result<&'a ProviderProfile, QueueError> {
    let eligible = |profile: &&ProviderProfile| profile.enabled && profile.kind == ProviderKind::Plex
        && machine.is_none_or(|id| profile.source == id);
    if let Some(active) = runtime.active().filter(|profile| profile.enabled && profile.kind == ProviderKind::Plex
        && machine.is_none_or(|id| profile.source == id)) {
        let count = runtime.saved.profiles.profiles.iter().filter(eligible)
            .filter(|profile| profile.id == active.id).count();
        return if count == 1 { Ok(active) } else { Err(QueueError::Configuration) };
    }
    let mut profiles = runtime.saved.profiles.profiles.iter().filter(eligible);
    let profile = profiles.next().ok_or(QueueError::Configuration)?;
    if profiles.next().is_some() { return Err(QueueError::Configuration); }
    Ok(profile)
}

fn remote_binding(runtime: &Runtime) -> Result<(ProviderProfile, SourceBinding, &'static Client,
    Arc<prepare::CapturedAuthority>), QueueError>
{
    // Malformed or explicitly imported credentials must never be downgraded to
    // a different native account when the exact stored-source guard refuses.
    let sources = runtime.saved.plex_sources().map_err(|_| QueueError::Configuration)?;
    let native_scope = plx_plex::plex::grant::scope();
    let source = if sources.is_empty() { None }
        else { Some(commands::stored_plex(runtime).ok_or(QueueError::Configuration)?) };
    let profile = selected_profile(runtime, source.as_ref().map(|source| source.machine_id.as_str()))?.clone();
    let client = plx_plex::plex::id_of_machine(&profile.source)
        .and_then(plx_plex::plex::client_for).ok_or(QueueError::ServerUnavailable)?;
    let binding = if let Some(source) = source {
        if source.origin().as_ref() != Some(client.origin()) || !client.uses_token(&source.token) {
            return Err(QueueError::ServerUnavailable);
        }
        SourceBinding::Stored(source)
    } else {
        if !client.has_token()
            || !plx_plex::plex::grant::credential_allowed_for(&profile.source, client.origin()) {
            return Err(QueueError::ServerUnavailable);
        }
        SourceBinding::NativeAccount(native_scope)
    };
    let authority = prepare::CapturedAuthority::capture(client).map_err(|_| QueueError::ServerUnavailable)?;
    if !authority.matches(client) || (matches!(binding, SourceBinding::NativeAccount(_))
        && native_scope != plx_plex::plex::grant::scope()) {
        return Err(QueueError::ServerUnavailable);
    }
    Ok((profile, binding, client, Arc::new(authority)))
}

fn source_current(runtime: &Runtime, work: &Preparation) -> bool {
    let expected = match &work.source {
        SourceBinding::Stored(source) => source,
        SourceBinding::NativeAccount(scope) => return matches!(work.purpose, Purpose::Remote { .. })
            && *scope == plx_plex::plex::grant::scope()
            && runtime.saved.plex_sources().is_ok_and(|sources| sources.is_empty())
            && selected_profile(runtime, None).ok() == Some(&work.profile)
            && work.client.has_token()
            && plx_plex::plex::grant::credential_allowed_for(
                &work.profile.source, work.client.origin()),
    };
    let same = |source: &plx_plex::plex::session::SourceRef| {
        source.machine_id == expected.machine_id
            && source.origin() == expected.origin()
            && source.token == expected.token
    };
    match &work.purpose {
        Purpose::Remote { .. } => commands::stored_plex(runtime).as_ref().is_some_and(same),
        Purpose::KioskRestore { target, .. } => {
            restore_bindings_current(runtime, &work.profile, target)
                && runtime.saved.plex_sources().is_ok_and(|sources| {
                    let mut matches = sources
                        .iter()
                        .filter(|source| source.machine_id == expected.machine_id);
                    matches.next().is_some_and(same) && matches.next().is_none()
                })
        }
    }
}

fn restore_bindings_current(
    runtime: &Runtime,
    profile: &ProviderProfile,
    target: &stored::Stored,
) -> bool {
    let TargetRef::Queue(entries) = target.target() else {
        return false;
    };
    let Some(first) = entries.first() else {
        return false;
    };
    let Ok(bindings) = runtime.favorite_bindings(first) else {
        return false;
    };
    entries.iter().all(|entry| {
        matches!(selection::bind_bound(&bindings, entry),
        Ok(selection::SelectionOwner::Profile(current)) if current == profile)
    })
}

fn staged_restore_matches(c: &PlexCandidate) -> bool {
    let Purpose::KioskRestore { target, .. } = &c.work.purpose else {
        return true;
    };
    let Some((session, catalog)) = &c.staged else {
        return false;
    };
    let TargetRef::Queue(expected) = target.target() else {
        return false;
    };
    expected.len() == catalog.entries.len() && session.queue.records().len() == expected.len()
        && catalog.entries.iter().zip(expected).all(|(actual, expected)|
            matches!((&actual.media, &expected.locator),
                (MediaLocator::Plex { server_id:a, rating_key:b },
                 plx_data::ott::favorites::Locator::Plex { server_id:c, rating_key:d }) if a == c && b == d))
}

/// Pending restore admission, distinct from an already-playing queue snapshot.
/// Owner5 calls this before generic catalog lookup for the new first occurrence.
pub(in crate::app::ott_runtime) fn kiosk_restore_allows(
    runtime: &Runtime,
    id: &str,
    entries: stored::PinEntries<'_>,
) -> bool {
    let Some(c) = candidate(runtime) else {
        return false;
    };
    let Purpose::KioskRestore { target, .. } = &c.work.purpose else {
        return false;
    };
    let Some((session, _)) = &c.staged else {
        return false;
    };
    let TargetRef::Queue(expected) = target.target() else {
        return false;
    };
    id == session.id(0)
        && current(runtime, c)
        && staged_restore_matches(c)
        && entries.len() == expected.len()
        && entries
            .iter()
            .zip(expected)
            .all(|(actual, expected)| actual.equals(expected))
}

pub(in crate::app::ott_runtime) fn build_session(
    profile: ProviderProfile,
    proposal: prepare::Proposal,
) -> Result<(PlexSession, Catalog), WireError> {
    let (selection, receipt) = match proposal.prepared {
        prepare::Prepared::Queue {
            kind: PrepareKind::Play,
            selection,
        } => (selection, None),
        prepare::Prepared::Library {
            request:
                request @ LibraryRequest::Prepare {
                    kind: PrepareKind::Play,
                    ..
                },
            library,
            truncated,
            selection,
        } => (
            selection,
            Some(LibraryReceipt {
                request,
                library,
                truncated,
            }),
        ),
        _ => return Err(WireError::InvalidResult),
    };
    let session = PlexSession {
        queue: queue::PlexQueue::new(proposal.owner, selection.prepared),
        authority: Arc::new(proposal.authority),
        metadata: Arc::new(selection.metadata),
        occurrences: selection.occurrences,
        profile,
        category: String::new(),
        favorites_selected: false,
        borrowed: None,
        receipt,
    };
    let mut catalog = Catalog::default();
    let entries = (0..session.occurrences.len())
        .map(|index| {
            let item = session
                .metadata
                .get(session.occurrences[index])
                .ok_or(WireError::InvalidResult)?;
            if session
                .queue
                .records()
                .get(index)
                .is_none_or(|record| record.id != item.rating_key)
            {
                return Err(WireError::InvalidResult);
            }
            Ok(CatalogEntry {
                synopsis: None,
                id: session.id(index),
                title: item.title.clone(),
                category: item.grandparent_title.clone(),
                description: item.summary.clone(),
                poster: if item.thumb.is_empty() {
                    item.art.clone()
                } else {
                    item.thumb.clone()
                },
                duration_ms: u64::try_from(item.duration).ok().filter(|v| *v > 0),
                provider_id: Some(item.rating_key.clone()),
                provider_id_field: None,
                live: None,
                media: MediaLocator::Plex {
                    server_id: session.authority.machine_id.clone(),
                    rating_key: item.rating_key.clone(),
                },
            })
        })
        .collect::<Result<Vec<_>, WireError>>()?;
    if entries.is_empty() || entries.len() != session.queue.records().len() {
        return Err(WireError::InvalidResult);
    }
    catalog
        .merge(Catalog { hierarchy: None, entries })
        .map_err(|_| WireError::Queue(QueueError::MissingItem))?;
    Ok((session, catalog))
}

pub(in crate::app::ott_runtime) fn prepared(
    runtime: &mut Runtime,
    app: &mut App,
    owner: OwnerToken,
    id: PreparationId,
    result: Result<prepare::Proposal, WireError>,
) {
    let Some(c) = candidate(runtime) else {
        return;
    };
    if c.work.owner != owner || c.work.purpose.key() != id {
        return;
    }
    let mut c = take_candidate(runtime).unwrap();
    let result = if c.is_play() && !playback_allowed(app)
        || (matches!(c.work.purpose, Purpose::KioskRestore { .. })
            && app.player.pending_exit.is_some())
    {
        Err(WireError::Queue(QueueError::ContextChanged))
    } else if !current(runtime, &c) {
        Err(preparation_lifecycle::invalid_error(runtime, &c))
    } else {
        result
    }
    .and_then(|proposal| {
        if proposal.owner.0 != owner.0 || !proposal.authority.matches(c.work.client) {
            Err(WireError::Queue(QueueError::ContextChanged))
        } else {
            Ok(proposal)
        }
    });
    let proposal = match result {
        Ok(value) => value,
        Err(error) => {
            restore(runtime, &c);
            preparation_lifecycle::failed(runtime, &c, error);
            let value = c.work.purpose.error(runtime, error);
            c.work.purpose.complete(runtime, value);
            return;
        }
    };
    if !c.is_play() {
        let response = proposal
            .prepared
            .preview_value(runtime.remote.runtime())
            .map(ok)
            .unwrap_or_else(|error| c.work.purpose.error(runtime, error));
        c.work.purpose.complete(runtime, response);
        return;
    }
    let (session, catalog) = match build_session(c.work.profile.clone(), proposal) {
        Ok(value) => value,
        Err(error) => {
            restore(runtime, &c);
            preparation_lifecycle::failed(runtime, &c, error);
            let value = c.work.purpose.error(runtime, error);
            c.work.purpose.complete(runtime, value);
            return;
        }
    };
    let first = catalog.entries[0].clone();
    let request = PlaybackRequest {
        kiosk_selection: None,
        archive: None,
        transient_owner: Some(owner),
        local_selection: None,
        continuation: None,
        id: first.id.clone(),
        position_ms: 0,
        advance_on_failure: false,
    };
    let profile = c.work.profile.clone();
    let invocation = c.work.purpose.invocation().cloned();
    c.staged = Some((session, catalog));
    if !staged_restore_matches(&c) {
        restore(runtime, &c);
        preparation_lifecycle::failed(runtime, &c, WireError::Queue(QueueError::MissingItem));
        let value = c
            .work
            .purpose
            .error(runtime, WireError::Queue(QueueError::MissingItem));
        c.work.purpose.complete(runtime, value);
        return;
    }
    runtime.transient.candidate = Some(Candidate::Plex(c));
    if runtime
        .queue_play_request(request, profile, Some(first), None, false)
        .is_err()
    {
        let c = take_candidate(runtime).unwrap();
        runtime.cancel_play_request();
        restore(runtime, &c);
        preparation_lifecycle::failed(runtime, &c, WireError::Queue(QueueError::StartFailed));
        let value = c
            .work
            .purpose
            .error(runtime, WireError::Queue(QueueError::StartFailed));
        c.work.purpose.complete(runtime, value);
    } else {
        runtime.pending_play = invocation;
    }
}

pub(in crate::app::ott_runtime) fn choice(runtime: &Runtime, id: &str) -> Option<NativeChoice> {
    candidate(runtime)
        .and_then(|c| c.staged.as_ref())
        .and_then(|(s, _)| s.choice(id))
        .or_else(|| session(runtime).and_then(|s| s.choice(id)))
}
pub(in crate::app::ott_runtime) fn target_current(runtime: &Runtime) -> Option<bool> {
    let target = runtime.play_target.as_ref()?;
    let choice = choice(runtime, &target.id)?;
    Some(target.transient_owner == Some(choice.owner) && choice.current())
}
/// Final source binder for the first staged Plex occurrence. No catalog is published.
pub(in crate::app::ott_runtime) fn staged_native_binding(
    runtime: &Runtime,
    request: &PlaybackRequest,
) -> Option<Result<(ProviderProfile, CatalogEntry), &'static str>> {
    let candidate = candidate(runtime)?;
    let valid = || {
        let (session, catalog) = candidate.staged.as_ref()?;
        if !current(runtime, candidate)
            || !staged_restore_matches(candidate)
            || request.transient_owner != Some(candidate.work.owner)
            || request.id != session.id(0)
            || request.kiosk_selection.is_some()
            || request.local_selection.is_some()
            || request.continuation.is_some()
            || request.position_ms != 0
            || request.advance_on_failure
            || !session
                .choice(&request.id)
                .is_some_and(|choice| choice.current())
        {
            return None;
        }
        let entry = catalog.entries.first()?;
        (entry.id == request.id).then(|| (candidate.work.profile.clone(), entry.clone()))
    };
    Some(valid().ok_or("Staged Plex source changed before native admission"))
}

pub(in crate::app::ott_runtime) fn before_native(
    runtime: &Runtime,
) -> Option<Result<(), &'static str>> {
    let c = candidate(runtime)?;
    Some(
        if current(runtime, c) && c.staged.is_some() && staged_restore_matches(c) {
            Ok(())
        } else {
            Err("Plex preparation was superseded")
        },
    )
}

pub(in crate::app::ott_runtime) fn finish_native(
    runtime: &mut Runtime,
    app: &mut App,
    result: Result<(), &'static str>,
    native: Option<&crate::app::ott_runtime::NativeStartReceipt>,
) -> Option<Value> {
    if result.is_err() && target_current(runtime).is_some() {
        // A replacement worker may already have closed the old server session.
        // Do not retain that decoder as a fallback; the common helper fences
        // retirement to the still-matching logical/native playback owner.
        runtime.retire_closed_source(app);
    }
    if let Some(c) = candidate(runtime) {
        if !runtime.play_target.as_ref().is_some_and(|target| {
            target.transient_owner == Some(c.work.owner)
                && c.staged
                    .as_ref()
                    .is_some_and(|(session, _)| target.id == session.id(0))
                && c.work.purpose.invocation().is_none_or(|expected| {
                    runtime
                        .pending_play
                        .as_ref()
                        .is_some_and(|actual| actual.id == expected.id)
                })
        }) {
            return None;
        }
        let c = take_candidate(runtime).unwrap();
        preparation_lifecycle::native_finished(runtime, app, &c, result, native);
        if result.is_err() {
            restore(runtime, &c);
            let response = c
                .work
                .purpose
                .error(runtime, WireError::Queue(QueueError::StartFailed));
            if c.work.purpose.invocation().is_none() {
                c.work.purpose.complete(runtime, response.clone());
            }
            return Some(response);
        }
        let (mut session, catalog) = c.staged?;
        if matches!(c.work.purpose, Purpose::KioskRestore { .. }) {
            let _ = session
                .queue
                .set_kiosk_repeat(session.queue.owner(), true);
        }
        let version = match &c.work.purpose {
            Purpose::Remote { invocation, .. } => {
                match queue::state_version(&invocation.action, &invocation.params) {
                    Ok(version) => version,
                    Err(_) => return Some(rejected("Invalid Plex state version")),
                }
            }
            // Internal inspection only; no remote Invocation or unsolicited
            // v2 body is published for a restored queue.
            Purpose::KioskRestore { .. } => queue::StateVersion::V2,
        };
        let nested = session
            .queue
            .snapshot_version(runtime.remote.runtime(), version);
        let response = match &c.work.purpose {
            Purpose::Remote {
                request: PlexRequest::Library(request),
                ..
            } => {
                // The validated library receipt is retained separately below.
                library_response(request, runtime.remote.runtime(), nested, &session)
            }
            _ => nested,
        }
        .map(ok)
        .unwrap_or_else(|_| rejected("Invalid Plex result"));
        runtime.catalog = catalog;
        runtime.catalog_complete = true;
        runtime.catalog_loading = false;
        runtime.catalog_revision = runtime.catalog_revision.wrapping_add(1);
        runtime.catalog_dirty = true;
        runtime.selected = Some(session.id(0));
        runtime.page = if runtime.saved.favorites_selected { "favorites" } else { "channels" };
        runtime.transient.accepted = Some(TransientMedia::Plex(session));
        runtime.parental_source_installed();
        runtime.continuity.stop(runtime.generation);
        runtime.changed();
        return Some(response);
    }
    let target = runtime.play_target.as_ref()?;
    let selected = choice(runtime, &target.id)?;
    let retain = runtime.playing.as_ref().is_some_and(|playing| {
        session(runtime).is_some_and(|s| {
            s.queue.playback().is_some()
                && s.native_matches(playing, app)
                && !matches!(
                    plx_media::player::state(&app.player.session),
                    plx_media::player::PlaybackState::Idle
                        | plx_media::player::PlaybackState::Error
                )
        })
    });
    if result.is_err() && !retain {
        let s = session_mut(runtime)?;
        if let Some(token) = s.queue.playback() {
            s.queue
                .fail(s.queue.owner(), token, QueueError::StartFailed);
        } else {
            s.queue
                .fail_pending(s.queue.owner(), QueueError::StartFailed);
        }
    }
    let inv = runtime.pending_play.as_ref()?;
    if inv.action != "plex_queue" && inv.action != "plex_library" {
        return None;
    }
    let _ = selected;
    let Ok(version) = queue::state_version(&inv.action, &inv.params) else {
        return Some(rejected("Invalid Plex state version"));
    };
    if result.is_err() {
        Some(failure_version(
            version,
            runtime.remote.runtime(),
            &PlexRequest::Queue(QueueRequest::Next),
            QueueError::StartFailed,
        ))
    } else {
        Some(snapshot(runtime, version))
    }
}

fn library_response(
    request: &LibraryRequest,
    runtime: &str,
    nested: Result<Value, WireError>,
    session: &PlexSession,
) -> Result<Value, WireError> {
    let receipt = session.receipt.as_ref().ok_or(WireError::InvalidResult)?;
    if request != &receipt.request {
        return Err(WireError::InvalidResult);
    }
    queue::library_prepared_value(
        runtime,
        request,
        &receipt.library,
        nested?,
        receipt.truncated,
    )
}

pub(in crate::app::ott_runtime) fn cancel_candidate(runtime: &mut Runtime, app: &mut App) {
    retire_candidate(runtime, app, WireError::Queue(QueueError::Cancelled));
}
fn retire_candidate(runtime: &mut Runtime, app: &mut App, error: WireError) {
    let Some(c) = take_candidate(runtime) else {
        return;
    };
    c.work.cancel.store(true, Ordering::Release);
    if c.is_play() {
        runtime.retire_closed_source(app);
        if runtime.pending_play.as_ref().is_some_and(|inv| {
            c.work
                .purpose
                .invocation()
                .is_some_and(|expected| inv.id == expected.id)
        }) {
            runtime.pending_play = None;
        }
        runtime.cancel_play_request();
    } else if runtime.resolver_next.as_ref().is_some_and(|r| {
        r.plex
            .as_ref()
            .is_some_and(|w| w.owner == c.work.owner && w.purpose.key() == c.work.purpose.key())
    }) {
        runtime.resolver_next = None;
    }
    restore(runtime, &c);
    preparation_lifecycle::failed(runtime, &c, error);
    let value = c.work.purpose.error(runtime, error);
    c.work.purpose.complete(runtime, value);
}
pub(in crate::app::ott_runtime) fn pending_tick(runtime: &mut Runtime, app: &mut App) {
    if candidate(runtime).is_some_and(|c| {
        !current(runtime, c)
            || (c.is_play() && !playback_allowed(app))
            || (matches!(c.work.purpose, Purpose::KioskRestore { .. })
                && app.player.pending_exit.is_some())
    }) {
        let error = candidate(runtime).map_or(WireError::Queue(QueueError::Cancelled), |c| {
            if !current(runtime, c) {
                preparation_lifecycle::invalid_error(runtime, c)
            } else {
                WireError::Queue(QueueError::Cancelled)
            }
        });
        retire_candidate(runtime, app, error);
    }
    let owned = runtime
        .playing
        .as_ref()
        .is_some_and(|p| session(runtime).is_some_and(|s| s.native_matches(p, app)));
    if owned {
        let state = plx_media::player::state(&app.player.session);
        let s = session_mut(runtime).unwrap();
        let owner = s.queue.owner();
        let Some(token) = s.queue.playback() else {
            return;
        };
        if !plx_plex::plex::client_for(s.authority.server)
            .is_some_and(|client| s.authority.matches(client))
        {
            s.queue.fail(owner, token, QueueError::ContextChanged);
            plx_media::player::stop_bufferfeed(&mut app.player.session, &mut app.adapters.player);
            plx_media::route::cancel_play(&mut app.player.session);
            return;
        }
        let phase = match state {
            plx_media::player::PlaybackState::Playing => Some(queue::PlaybackPhase::Playing),
            plx_media::player::PlaybackState::Paused => Some(queue::PlaybackPhase::Paused),
            _ => None,
        };
        if let Some(phase) = phase {
            s.queue.observe(owner, token, phase);
        }
    }
}

/// Change only an already admitted queue's ordinary repeat intent. The old
/// source/native ownership and the exact bounded reply are checked before the
/// pure setter; no play, pause, Save, source replacement or cursor change occurs.
fn set_repeat(
    runtime: &mut Runtime,
    app: &App,
    repeat: queue::RepeatPolicy,
    version: queue::StateVersion,
) -> Value {
    let request = PlexRequest::Queue(QueueRequest::Repeat(repeat));
    let fail = |error| failure_version(version, runtime.remote.runtime(), &request, error);
    if runtime.transient.candidate.is_some()
        || runtime.play_target.is_some()
        || plx_media::route::play_pending()
    {
        return fail(QueueError::Busy);
    }
    let Some(s) = session(runtime) else {
        return fail(QueueError::Empty);
    };
    if s.queue.stopped() {
        return fail(QueueError::Unavailable);
    }
    if s.borrowed.is_some()
        || s.queue.playback().is_none()
        || !runtime.playing.as_ref().is_some_and(|playing| {
            s.native_matches(playing, app) && !playing_superseded(playing, app)
        })
        || !matches!(parental_source_authority(runtime), Ok(Some(_)))
    {
        return fail(QueueError::ContextChanged);
    }
    if !matches!(
        plx_media::player::state(&app.player.session),
        plx_media::player::PlaybackState::Playing
            | plx_media::player::PlaybackState::Paused
            | plx_media::player::PlaybackState::Buffering
            | plx_media::player::PlaybackState::Seeking
    ) {
        return fail(QueueError::Unavailable);
    }
    let value = match s.queue.snapshot_for_repeat(runtime.remote.runtime(), version, repeat) {
        Ok(value) => value,
        Err(_) => return rejected("Invalid Plex queue state"),
    };
    let s = session_mut(runtime).unwrap();
    if let Err(error) = s.queue.set_repeat(s.queue.owner(), repeat) {
        return failure_version(version, runtime.remote.runtime(), &request, error);
    }
    runtime.changed();
    ok(value)
}

/// Called only after a durable policy is published, or for its exact accepted
/// queue. Removing a force also applies to a parked queue; ordinary intent stays.
pub(in crate::app::ott_runtime) fn set_kiosk_repeat(runtime: &mut Runtime, forced: bool) {
    if let Some(s) = session_mut(runtime) {
        let _ = s.queue.set_kiosk_repeat(s.queue.owner(), forced);
    }
}

pub(in crate::app::ott_runtime) fn step(
    runtime: &mut Runtime,
    app: &mut App,
    offset: i64,
    inv: Option<&Invocation>,
) -> Option<Value> {
    let version = match inv {
        Some(inv) => match queue::state_version(&inv.action, &inv.params) {
            Ok(version) => version,
            Err(_) => return Some(rejected("Invalid Plex state version")),
        },
        None => queue::StateVersion::V1,
    };
    let request = PlexRequest::Queue(if offset > 0 {
        QueueRequest::Next
    } else {
        QueueRequest::Previous
    });
    if runtime.transient.candidate.is_some() || runtime.play_target.is_some() {
        return Some(failure_version(
            version,
            runtime.remote.runtime(),
            &request,
            QueueError::Busy,
        ));
    }
    let Some(s) = session(runtime) else {
        return Some(failure_version(
            version,
            runtime.remote.runtime(),
            &request,
            QueueError::Empty,
        ));
    };
    let index = match s.queue.target_index(s.queue.owner(), offset) {
        Ok(index) => index,
        Err(error) => {
            return Some(failure_version(
                version,
                runtime.remote.runtime(),
                &request,
                error,
            ))
        }
    };
    let id = s.id(index);
    match runtime.play_with_policy(app, &id, false) {
        Ok(()) => {
            if let Some(inv) = inv {
                runtime.pending_play = Some(inv.clone());
            }
            None
        }
        Err(_) => Some(failure_version(
            version,
            runtime.remote.runtime(),
            &request,
            QueueError::StartFailed,
        )),
    }
}
pub(in crate::app::ott_runtime) fn selected(runtime: &mut Runtime, id: &str) {
    let token = queue::PlaybackToken(runtime.play_generation);
    if let Some(Candidate::Plex(c)) = runtime.transient.candidate.as_mut() {
        if let Some((s, _)) = c.staged.as_mut() {
            if let Some(choice) = s.choice(id) {
                let _ = s.queue.begin(s.queue.owner(), token, choice.index);
                return;
            }
        }
    }
}
/// Commit a replacement only after native admission. Failed lookups leave the
/// previous queue cursor and playback proof untouched.
pub(in crate::app::ott_runtime) fn admitted(runtime: &mut Runtime) {
    if runtime.transient.candidate.is_some() {
        return;
    }
    let Some(target) = runtime.play_target.as_ref() else {
        return;
    };
    let id = target.id.clone();
    let owner = target.transient_owner;
    let token = queue::PlaybackToken(runtime.play_generation);
    let Some(s) = session_mut(runtime) else {
        return;
    };
    if owner != Some(OwnerToken(s.queue.owner().0)) {
        return;
    }
    if let Some(choice) = s.choice(&id) {
        let _ = s.queue.begin(s.queue.owner(), token, choice.index);
        s.borrowed = None;
    } else {
        let _ = s.queue.park(s.queue.owner());
    }
}

pub(in crate::app::ott_runtime) fn note_stop(runtime: &mut Runtime) {
    if let Some(s) = session_mut(runtime) {
        let _ = s.queue.stop(s.queue.owner());
    }
}
pub(in crate::app::ott_runtime) fn set_view(
    runtime: &mut Runtime,
    category: &str,
    favorites: bool,
) -> bool {
    let Some(s) = session_mut(runtime) else {
        return false;
    };
    s.category = category.into();
    s.favorites_selected = favorites;
    true
}
pub(in crate::app::ott_runtime) fn refresh(runtime: &mut Runtime, app: &mut App) -> bool {
    if session(runtime).is_none() {
        return false;
    }
    cancel_candidate(runtime, app);
    runtime.changed();
    true
}
pub(in crate::app::ott_runtime) fn checkpoint(
    runtime: &mut Runtime,
    playing: &Playing,
    position_ms: u64,
) -> bool {
    let Some(s) = session_mut(runtime)
        .filter(|s| Some(OwnerToken(s.queue.owner().0)) == playing.transient_owner)
    else {
        return false;
    };
    if s.owns(playing) {
        s.borrowed = None;
    } else {
        let _ = s.queue.park(s.queue.owner());
        s.borrowed = Some(BorrowedBookmark {
            selection_id: playing.selection_id.clone(),
            profile_fingerprint: profile_fingerprint(&playing.profile),
            position_ms,
        });
    }
    true
}
pub(in crate::app::ott_runtime) fn bookmark_position(
    runtime: &Runtime,
    playing: &Playing,
) -> Option<u64> {
    session(runtime)?
        .borrowed
        .as_ref()
        .filter(|b| {
            b.selection_id == playing.selection_id
                && b.profile_fingerprint == profile_fingerprint(&playing.profile)
        })
        .map(|b| b.position_ms)
}
pub(in crate::app::ott_runtime) fn failure_event(runtime: &mut Runtime, app: &App) -> bool {
    let Some(playing) = runtime.playing.as_ref() else {
        return false;
    };
    if !session(runtime).is_some_and(|s| s.native_matches(playing, app)) {
        return false;
    }
    let s = session_mut(runtime).unwrap();
    if let Some(token) = s.queue.playback() {
        s.queue
            .fail(s.queue.owner(), token, QueueError::StartFailed);
    }
    true
}
pub(in crate::app::ott_runtime) fn ended(runtime: &mut Runtime, app: &mut App) -> bool {
    let Some(s) = session(runtime) else {
        return false;
    };
    let Some(playing) = runtime
        .playing
        .as_ref()
        .filter(|p| s.native_matches(p, app))
    else {
        // A direct foreign Favorite is a one-shot borrow unless the user chose
        // Favorites traversal; it must not fall back to the queue catalogue.
        let borrowed = !runtime.saved.favorites_selected
            && runtime.playing.as_ref().is_some_and(|p| {
                p.transient_owner == Some(OwnerToken(s.queue.owner().0))
                    && p.selection_id.starts_with("favorite:")
                    && playing_matches(p, app)
            });
        if borrowed {
            if runtime.play_target.is_none() && !runtime.transient.owns_playback() {
                plx_media::player::stop_bufferfeed(
                    &mut app.player.session,
                    &mut app.adapters.player,
                );
                runtime.continuity.stop(runtime.generation);
            }
            return true;
        }
        return false;
    };
    let _ = playing;
    // An explicit in-flight choice owns the next start. Its old source reaching
    // EOF must not replace that choice with the automatic successor.
    if runtime.play_target.is_some() || runtime.transient.owns_playback() {
        return true;
    }
    let s = session_mut(runtime).unwrap();
    let Some(token) = s.queue.playback() else {
        return true;
    };
    let decision = s.queue.ended(s.queue.owner(), token);
    let next = if let queue::PlexDecision::Start { index } = decision {
        Some(s.id(index))
    } else {
        None
    };
    if decision != queue::PlexDecision::Ignore {
        plx_media::player::stop_bufferfeed(&mut app.player.session, &mut app.adapters.player);
    }
    if let Some(id) = next {
        if runtime.play_with_policy(app, &id, false).is_err() {
            if let Some(s) = session_mut(runtime) {
                s.queue
                    .fail_pending(s.queue.owner(), QueueError::StartFailed);
            }
        }
    }
    runtime.changed();
    true
}

pub(in crate::app::ott_runtime) fn cancel_shutdown(runtime: &Runtime) {
    if let Some(c) = candidate(runtime) {
        c.work.cancel.store(true, Ordering::Release);
    }
}
pub(in crate::app::ott_runtime) fn step_id(
    runtime: &Runtime,
    offset: i64,
) -> Option<Option<String>> {
    let s = session(runtime)?;
    if runtime.saved.favorites_selected || s.queue.stopped() {
        return None;
    }
    Some(
        s.queue
            .target_index(s.queue.owner(), offset)
            .ok()
            .map(|index| s.id(index)),
    )
}

/// The shared worker slot can become available after the command returned.
/// If spawning that queued preparation fails, complete its invocation now.
pub(in crate::app::ott_runtime) fn resolver_failed(runtime: &mut Runtime) {
    if !candidate(runtime).is_some_and(|c| c.staged.is_none()) {
        return;
    }
    let c = take_candidate(runtime).unwrap();
    c.work.cancel.store(true, Ordering::Release);
    restore(runtime, &c);
    preparation_lifecycle::failed(runtime, &c, WireError::Queue(QueueError::Unavailable));
    let value = c
        .work
        .purpose
        .error(runtime, WireError::Queue(QueueError::Unavailable));
    c.work.purpose.complete(runtime, value);
}

#[path = "ott_transient_plex_kiosk.rs"]
mod kiosk;
pub(in crate::app::ott_runtime) use kiosk::{
    kiosk_queue, kiosk_queue_allows, kiosk_queue_for_status, kiosk_queue_is_current, kiosk_queue_matches_current,
    kiosk_repeat_all,
};

#[path = "ott_transient_plex_preparation.rs"]
mod preparation_lifecycle;
pub(in crate::app) use preparation_lifecycle::PlexKioskPreparation;
pub(in crate::app::ott_runtime) use preparation_lifecycle::{
    kiosk_preparation_is_current, kiosk_preparation_resume_policy, retry_kiosk_preparation,
};
