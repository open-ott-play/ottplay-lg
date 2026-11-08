//! App-owned runner for the one configured, previously verified Plex candidate.
//! SessionMachine remains the authority owner. This runner holds real proof and
//! projects temporary row contexts; no candidate is inserted into Saved.
use super::*;
use crate::app::ott_import::ManagedCandidate;
use crate::app::ott_remote::plex_servers::{Error, Operation, Request, Row};
use plx_session::auth::owner::{CredentialPatch, Identity, ManagedActivation};
use std::time::Duration;
use plx_base::task::time::Deadline;
#[path = "ott_managed_context.rs"]
mod context;
#[path = "ott_managed_reentry.rs"]
mod reentry;
pub(super) use reentry::{binding_profile, browse_active, capture_read, guard_catalog_event,
    guard_metadata_event, guard_profile, owns_retained_profile, record_catalog, record_metadata, resume_ui};

pub(super) type CandidateResult = Result<Option<ManagedCandidate>, &'static str>;
struct Authority { epoch: u64, credentials: CredentialPatch, disk: Identity }
impl Authority {
    fn capture(app: &App) -> Self {
        let state = app.bridge.snapshot_session_init();
        Self { epoch: state.epoch, credentials: CredentialPatch::of(&state.persisted), disk: state.disk_identity }
    }
    fn current(&self, app: &App) -> bool {
        let state = app.bridge.snapshot_session_init();
        state.epoch == self.epoch && state.disk_identity == self.disk
            && self.credentials.matches_session(&state.persisted)
    }
}
struct Pending {
    operation: u64,
    deadline: u64,
    command_deadline: Deadline,
    context_deadline: Deadline,
    navigation: u64,
    expected_epoch: u64,
    generation: u64,
    source: ManagedCandidate,
    binding: context::Binding,
    server_id: String,
    context: String,
    invocation: Option<Invocation>,
    request: Option<Request>,
    cancel: Arc<AtomicBool>,
    worker: Option<Receiver<Result<ManagedCandidate, Error>>>,
    staged: bool,
}
struct Active {
    operation: u64,
    generation: u64,
    profile: ProviderProfile,
    category: String,
    favorites: bool,
    source: ManagedCandidate,
    refresh: Option<Receiver<Result<ManagedCandidate, &'static str>>>,
    cancel: Arc<AtomicBool>,
}
#[derive(Clone)]
struct ProjectedRow { server: String, name: String, context: String, shadow: Option<String> }
#[derive(Default)]
pub(super) struct State {
    started: bool,
    startup: Option<Receiver<CandidateResult>>,
    startup_settled: bool,
    startup_cancel: Option<Arc<AtomicBool>>,
    authority: Option<Authority>,
    candidate: Option<ManagedCandidate>,
    offers: context::OfferStore,
    row: Option<ProjectedRow>,
    failure: Option<Error>,
    pending: Option<Pending>,
    active: Option<Active>,
    retired_profile: Option<ProviderProfile>,
    reentry_ready: bool,
    metadata: Option<(u64, ProviderProfile, crate::app::adapters::session::ManagedRead)>,
    catalog: Option<(u64, ProviderProfile, crate::app::adapters::session::ManagedRead)>,
    scope: Option<plx_plex::plex::grant::GrantScope>,
    scope_tag: u64,
    automatic: bool,
    initial_input: u32,
    refresh_requested: bool,
}
impl State {
    pub(super) fn temporary_machine(&self) -> Option<&str> {
        self.active.as_ref().map(|a| a.source.verified.source.machine_id.as_str())
            .or_else(|| self.pending.as_ref().filter(|p| p.staged).map(|p| p.source.verified.source.machine_id.as_str()))
    }
    pub(super) fn has_candidate(&self) -> bool { self.candidate.is_some() || self.startup.is_some() }
    fn retire(&mut self) {
        if let Some(cancel) = &self.startup_cancel { cancel.store(true, Ordering::Release); }
        if let Some(candidate) = self.candidate.take() { candidate.retired.store(true, Ordering::Release); }
        if let Some(pending) = &self.pending { pending.cancel.store(true, Ordering::Release); }
        self.offers.invalidate();
        self.row = None;
        self.automatic = false;
    }
    fn binding(&self, runtime: &Runtime) -> context::Binding {
        context::Binding { candidate_generation: 1, authority_tag: runtime.remote.authority_generation(), scope_tag: self.scope_tag }
    }
    fn observe(&mut self, runtime: &Runtime, app: &App) -> Result<(), Error> {
        if !self.authority.as_ref().is_some_and(|a| a.current(app)) {
            self.retire(); return Err(Error::SourceChanged);
        }
        let scope = plx_plex::plex::grant::scope();
        if self.scope != Some(scope) {
            self.scope_tag = self.scope_tag.checked_add(1).ok_or(Error::LimitExceeded)?;
            self.scope = Some(scope);
            self.offers.invalidate(); self.row = None;
        }
        if let Some(source) = &self.candidate {
            if source.retired.load(Ordering::Acquire)
                || source.verified.proof.freshness() == plx_plex::plex::grant::ImportFreshness::Revoked {
                self.retire(); return Err(Error::SourceChanged);
            }
            let s = &source.verified.source;
            let origin = s.origin().ok_or(Error::SourceChanged)?;
            crate::app::ott_import::check_existing_authorization(&s.machine_id, &origin, &s.token)
                .map_err(|_| Error::AuthorizationConflict)?;
        }
        if !runtime.loaded || runtime.persistence_uncertain { return Err(Error::Unavailable); }
        Ok(())
    }
    fn list(&mut self, runtime: &mut Runtime, app: &App) -> Result<Vec<Row>, Error> {
        if self.active.is_some() || self.pending.is_some() { return Ok(Vec::new()); }
        self.observe(runtime, app)?;
        if let Some(error) = self.failure { return Err(error); }
        let Some(candidate) = &self.candidate else { return Ok(Vec::new()) };
        let source = &candidate.verified.source;
        let binding = self.binding(runtime);
        let offered = match self.offers.list(runtime.now_ms(), binding, runtime.remote.runtime(), &source.machine_id, "") {
            Ok(row) => row,
            Err(context::ContextError::InvalidRequest) => {
                let token = token(runtime)?;
                self.offers.list(runtime.now_ms(), binding, runtime.remote.runtime(), &source.machine_id, &token).map_err(context_error)?
            }
            Err(error) => return Err(context_error(error)),
        };
        // A Saved string alone is never authorization. Preserve a user label and
        // collapse presentation only when the live registry proves the same source.
        let compatible = source.origin().is_some_and(|origin|
            plx_plex::plex::id_of_machine(&source.machine_id)
                .and_then(plx_plex::plex::client_for)
                .is_some_and(|client| client.origin() == &origin && client.uses_token(&source.token)));
        let existing = compatible.then(|| runtime.saved.profiles.profiles.iter().find(|profile|
            profile.enabled && profile.kind == ProviderKind::Plex && profile.source == source.machine_id)).flatten();
        let name = existing.map_or_else(|| source.name.clone(), |profile| profile.name.clone());
        let row = Row::new(source.machine_id.clone(), name.clone(), offered.context.clone(), offered.expires_in_ms)?;
        self.row = Some(ProjectedRow { server: source.machine_id.clone(), name, context: offered.context,
            shadow: existing.map(|profile| profile.id.clone()) });
        Ok(vec![row])
    }
}
impl Drop for State {
    fn drop(&mut self) {
        self.retire();
        if let Some(active) = &self.active { active.source.retired.store(true, Ordering::Release); active.cancel.store(true, Ordering::Release); }
    }
}
fn context_error(error: context::ContextError) -> Error {
    use context::ContextError as E;
    match error { E::Busy => Error::Busy, E::Stale => Error::Stale, E::SourceChanged => Error::SourceChanged,
        E::Expired => Error::Expired, E::InvalidRequest => Error::InvalidRequest, E::LimitExceeded => Error::LimitExceeded }
}
fn adapter_error(error: crate::app::adapters::session::ManagedError) -> Error {
    use crate::app::adapters::session::ManagedError as E;
    match error { E::Busy => Error::Busy, E::Invalid => Error::InvalidRequest,
        E::Stale => Error::SourceChanged, E::Expired => Error::Expired,
        E::AuthorizationConflict => Error::AuthorizationConflict, E::Unavailable => Error::Unavailable }
}
fn token(runtime: &mut Runtime) -> Result<String, Error> {
    let entropy = runtime.entropy.ok_or(Error::Unavailable)?;
    runtime.uuid_counter = runtime.uuid_counter.checked_add(1).ok_or(Error::LimitExceeded)?;
    let mut input = Vec::from(entropy);
    input.extend_from_slice(b"managed-plex-context-v1");
    input.extend_from_slice(&runtime.uuid_counter.to_be_bytes());
    Ok(plx_base::sha256::sha256(&input).iter().map(|b| format!("{b:02x}")).collect())
}
fn policy(runtime: &mut Runtime, app: &App) -> Result<(), Error> {
    if !runtime.loaded || runtime.persistence_uncertain || runtime.saving || !app.running { return Err(Error::Unavailable); }
    if !super::playback_allowed(app) { return Err(Error::Restricted); }
    input_policy::replace_source(runtime).map_err(|_| Error::Restricted)?;
    runtime.require_parental(parental::Target::Providers).map_err(|_| Error::Restricted)
}

pub(crate) fn start(app: &mut App) {
    let Some(mut runtime) = app.ott.take() else { return };
    start_with(&mut runtime, app, |saved, cancel, tx| {
        plx_base::task::spawn_off_frame("ott-managed-plex", move |worker| {
            let result = crate::app::ott_import::load(worker, &saved, &cancel);
            if !cancel.load(Ordering::Acquire) { let _ = tx.send(result); }
            plx_machine::idle::invalidate();
        })
    });
    app.ott = Some(runtime);
}
/// Production boot and its scheduling tests share this worker-admission seam.
pub(super) fn start_with(runtime: &mut Runtime, app: &App,
    spawn: impl FnOnce(plx_plex::plex::session::Session, Arc<AtomicBool>, Sender<CandidateResult>) -> bool) -> bool {
    if runtime.managed.started { return false; }
    runtime.managed.started = true;
    runtime.managed.authority = Some(Authority::capture(app));
    runtime.managed.scope = Some(plx_plex::plex::grant::scope());
    runtime.managed.initial_input = app.last_input;
    runtime.managed.automatic = true;
    let saved = app.bridge.snapshot_session_init().persisted;
    let cancel = Arc::new(AtomicBool::new(false));
    let (tx, rx) = mpsc::channel();
    runtime.managed.startup_cancel = Some(cancel.clone());
    if spawn(saved, cancel, tx) { runtime.managed.startup = Some(rx); true }
    else { runtime.managed.startup_settled = true; runtime.managed.failure = Some(Error::Unavailable); false }
}

pub(crate) fn foreground(app: &mut App) {
    if let Some(runtime) = app.ott.as_mut() { runtime.managed.refresh_requested = true; }
}
pub(super) fn owns_profile(runtime: &Runtime, profile: &ProviderProfile) -> bool {
    runtime.managed.active.as_ref().is_some_and(|active| active.profile.id == profile.id
        && active.profile.kind == profile.kind && active.profile.source == profile.source)
}
pub(super) fn view_selection(runtime: &Runtime) -> Option<(&str, bool)> {
    runtime.managed.active.as_ref().filter(|active| active.generation == runtime.generation)
        .map(|active| (active.category.as_str(), active.favorites))
}
pub(super) fn set_view(runtime: &mut Runtime, category: &str, favorites: bool) {
    if let Some(active) = runtime.managed.active.as_mut().filter(|active| active.generation == runtime.generation) {
        active.category = category.into(); active.favorites = favorites;
    }
}
pub(super) fn profile(runtime: &Runtime) -> Option<&ProviderProfile> {
    runtime.managed.active.as_ref().filter(|active| active.generation == runtime.generation).map(|active| &active.profile)
}
/// A catalogue restart may advance the runtime generation without replacing
/// the active managed source. Only its original owner can be carried forward.
pub(super) fn rebase_catalog(runtime: &mut Runtime, old_generation: u64) {
    if let Some(active) = runtime.managed.active.as_mut().filter(|active| active.generation == old_generation) {
        active.generation = runtime.generation;
    }
}
pub(super) fn shadows_profile(runtime: &Runtime, profile: &ProviderProfile) -> bool {
    runtime.managed.row.as_ref().and_then(|row| row.shadow.as_ref()) == Some(&profile.id)
}
pub(super) fn row(runtime: &Runtime) -> Option<OttRow> {
    if let Some(row) = reentry::row(runtime) { return Some(row); }
    let row = runtime.managed.row.as_ref()?;
    Some(ui_projection::row(format!("managed/{}", row.server), row.name.clone(), String::new(),
        OttAction::SelectManagedPlex { server_id: row.server.clone(), context: row.context.clone() }, false))
}

pub(super) fn command(runtime: &mut Runtime, app: &mut App, inv: &Invocation) -> Option<Value> {
    let Some(raw) = inv.plex_servers_projection() else { return Some(json!({"status":"rejected","data":{"error":"Invalid managed-server request"}})); };
    let request = match Request::decode(raw) { Ok(request) => request, Err(error) => return Some(error.response()) };
    let error = if !runtime.remote.invocation_is_current(inv) || request.correlation().runtime() != runtime.remote.runtime() {
        Some(Error::Stale)
    } else if runtime.now_ms() >= inv.deadline_ms { Some(Error::Expired) } else { None };
    if let Some(error) = error { return Some(request.correlation().rejected(error)); }
    match request.operation() {
        Operation::List => {
            let mut state = std::mem::take(&mut runtime.managed);
            let result = state.list(runtime, app).and_then(|rows| request.list_response(&rows));
            runtime.managed = state;
            Some(result.unwrap_or_else(|error| request.correlation().rejected(error)))
        }
        Operation::Select { server_id, context } => {
            let server_id = server_id.clone(); let context = context.clone();
            match select(runtime, app, &server_id, &context, Some(inv.clone()), Some(request.clone()), inv.deadline_ms) {
                Ok(()) => None, Err(error) => Some(request.correlation().rejected(error))
            }
        }
    }
}

pub(super) fn select_ui(runtime: &mut Runtime, app: &mut App, server_id: &str, context: &str) -> Result<(), &'static str> {
    let deadline = runtime.now_ms().saturating_add(60_000);
    select(runtime, app, server_id, context, None, None, deadline).map_err(|error| error.code())
}
fn select(runtime: &mut Runtime, app: &mut App, server_id: &str, context: &str,
    invocation: Option<Invocation>, request: Option<Request>, deadline: u64) -> Result<(), Error> {
    policy(runtime, app)?;
    let mut state = std::mem::take(&mut runtime.managed);
    let result = (|| {
        if state.pending.is_some() || state.active.is_some() { return Err(Error::Busy); }
        state.observe(runtime, app)?;
        let source = state.candidate.as_ref().ok_or(Error::Unavailable)?.clone();
        let binding = state.binding(runtime);
        state.offers.check(runtime.now_ms(), binding, runtime.remote.runtime(), server_id, context).map_err(context_error)?;
        if deadline <= runtime.now_ms() { return Err(Error::Expired); }
        let operation = runtime.uuid_counter.checked_add(1).ok_or(Error::LimitExceeded)?;
        runtime.uuid_counter = operation;
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = cancel.clone(); let work = source.clone();
        // Both wire expiries use the runtime's fixed anchor timeline. Convert
        // once before queueing; neither worker start nor adapter staging renews it.
        let anchor = runtime.clock_anchor.or(runtime.clock_sample).ok_or(Error::Unavailable)?;
        let command_deadline = anchor.deadline_at_unix_ms(deadline);
        let context_deadline = anchor.deadline_at_unix_ms(state.offers.offered_deadline().ok_or(Error::Expired)?);
        let (tx, rx) = mpsc::channel();
        if !plx_base::task::spawn_off_frame("ott-managed-select", move |worker| {
            let remaining = || command_deadline.remaining(worker).min(context_deadline.remaining(worker));
            let result = (|| {
                if worker_cancel.load(Ordering::Acquire) { return Err(Error::Cancelled); }
                if remaining().is_zero() { return Err(Error::Expired); }
                work.recheck().map_err(|_| Error::SourceChanged)?;
                if worker_cancel.load(Ordering::Acquire) { return Err(Error::Cancelled); }
                if remaining().is_zero() { return Err(Error::Expired); }
                let selected = match work.verified.proof.freshness() {
                    plx_plex::plex::grant::ImportFreshness::Current => work.clone(),
                    plx_plex::plex::grant::ImportFreshness::NetworkChanged => work.reverify_before(&remaining, &worker_cancel)
                        .map_err(|_| if worker_cancel.load(Ordering::Acquire) { Error::Cancelled }
                            else if remaining().is_zero() { Error::Expired }
                            else if work.recheck().is_err() || work.verified.proof.freshness() == plx_plex::plex::grant::ImportFreshness::Revoked { Error::SourceChanged }
                            else { Error::Unavailable })?,
                    plx_plex::plex::grant::ImportFreshness::Revoked => return Err(Error::SourceChanged),
                };
                selected.recheck().map_err(|_| Error::SourceChanged)?;
                if worker_cancel.load(Ordering::Acquire) { return Err(Error::Cancelled); }
                if remaining().is_zero() { return Err(Error::Expired); }
                Ok(selected)
            })();
            let _ = tx.send(result); plx_machine::idle::invalidate();
        }) { return Err(Error::Unavailable); }
        state.automatic = false;
        state.pending = Some(Pending { operation, deadline, command_deadline, context_deadline, navigation: runtime.navigation_epoch,
            expected_epoch: state.authority.as_ref().ok_or(Error::SourceChanged)?.epoch,
            generation: runtime.generation, source, binding, server_id: server_id.into(), context: context.into(),
            invocation, request, cancel, worker: Some(rx), staged: false });
        state.row = None;
        Ok(())
    })();
    runtime.managed = state;
    result
}

/// Any later accepted foreground intent retires a not-yet-applied selection.
pub(super) fn remote_intent(runtime: &mut Runtime, inv: &Invocation) {
    if !matches!(inv.action.as_str(), "plex_servers" | "notifications") && !input_policy::read_only(inv) {
        navigation(&mut runtime.managed);
    }
}

#[cfg(test)]
#[test]
fn notification_policy_is_not_a_managed_source_navigation_intent() {
    let _serial=plx_base::testlock::serial();
    let mut runtime=Runtime::unloaded(crate::app::ott_companion::Supervisor::inert_for_test());
    runtime.managed.automatic=true;
    let mut inv=runtime.remote.request_for_test("notifications",json!({"operation":"get"}));
    for operation in ["get","set"] {
        inv.params=json!({"operation":operation});
        remote_intent(&mut runtime,&inv);
        assert!(runtime.managed.automatic,"receive policy must not cancel managed startup");
    }
    inv.action="input".into(); inv.params=json!({"key":"right"});
    remote_intent(&mut runtime,&inv);
    assert!(!runtime.managed.automatic,"ordinary viewer navigation still cancels startup");
}
/// Fence queued activation effects before this frame consumes new physical input.
/// Key-up and pointer hover do not supersede a selection.
pub(crate) fn input_fence(app: &mut App) {
    use plx_machine::machine::{Edge, InputKind};
    let intent = app.inputs.iter().any(|event| matches!(&event.kind,
        InputKind::Key { edge: Edge::Down | Edge::Repeat, .. }
        | InputKind::Click { .. } | InputKind::Drag { .. } | InputKind::Wheel { .. }
        | InputKind::Pan { .. } | InputKind::Text(_)));
    if intent { if let Some(runtime) = app.ott.as_mut() { navigation(&mut runtime.managed); } }
}
pub(super) fn navigation(state: &mut State) {
    state.automatic = false;
    if let Some(pending) = &state.pending { pending.cancel.store(true, Ordering::Release); }
}
fn settle(runtime: &mut Runtime, state: &mut State, pending: Pending, error: Option<Error>) {
    if error.is_some() { pending.cancel.store(true, Ordering::Release); }
    state.offers.finish(pending.operation);
    if let (Some(invocation), Some(request)) = (pending.invocation, pending.request) {
        let response = match error { Some(error) => request.correlation().rejected(error),
            None => request.applied_selection_response().unwrap_or_else(|error| request.correlation().rejected(error)) };
        runtime.remote.complete(invocation, response);
    } else if let Some(error) = error { runtime.status = format!("Plex: {}", error.code()); }
    runtime.changed();
}

/// A catalogue only needs resource teardown, including its first pending root.
/// Preserve the existing return policy for Player and other queued navigation.
pub(super) fn retire_for_catalog(app: &mut App) {
    let initial_ott = app.pages.top_arg().is_none()
        && matches!(app.pages.top_or_mounting_arg()
            .or_else(|| app.pages.nav.tabs.stack.pending_dest()), Some(AppArg::Ott));
    if bridge::player(&app.pages).is_some() || (!initial_ott
        && (app.pages.has_pending_navigation() || app.pages.nav.tabs.stack.is_pending())) {
        playback::replace_player(&mut app.player, &mut app.adapters.player, &mut app.repause_at,
            &mut app.refresh_hubs, &mut app.pages);
    } else {
        playback::replace_player_in_place(&mut app.player, &mut app.adapters.player,
            &mut app.repause_at, &mut app.pages);
    }
}

pub(super) fn tick(runtime: &mut Runtime, app: &mut App) {
    tick_with(runtime, app, select_ui);
}

// Keep automatic selection on the normal admission path. Scheduling tests can
// observe this final dispatch without replacing authority or starting a worker.
fn tick_with(runtime: &mut Runtime, app: &mut App,
    select: impl FnOnce(&mut Runtime, &mut App, &str, &str) -> Result<(), &'static str>) {
    let mut start_catalog = false;
    let mut state = std::mem::take(&mut runtime.managed);
    if let Some(rx) = &state.startup {
        match rx.try_recv() {
            Ok(result) => {
                state.startup = None; state.startup_settled = true;
                if state.authority.as_ref().is_some_and(|a| a.current(app)) {
                    match result { Ok(candidate) => state.candidate = candidate, Err(_) => state.failure = Some(Error::Unavailable) }
                } else { state.retire(); }
                runtime.changed();
            }
            Err(mpsc::TryRecvError::Disconnected) => { state.startup = None; state.startup_settled = true; state.failure = Some(Error::Unavailable); }
            Err(mpsc::TryRecvError::Empty) => {}
        }
    }
    if let Some(mut active) = state.active.take() {
        if app.bridge.snapshot_session_init().managed_operation() != Some(active.operation)
            || active.source.verified.proof.freshness() == plx_plex::plex::grant::ImportFreshness::Revoked {
            state.retired_profile = Some(active.profile.clone());
            active.source.retired.store(true, Ordering::Release);
            active.cancel.store(true, Ordering::Release);
            app.bridge.cancel_managed(active.operation);
            let managed_playing = runtime.playing.as_ref().is_some_and(|playing|
                playing.profile.id == active.profile.id && playing.profile.kind == active.profile.kind
                    && playing.profile.source == active.profile.source);
            let owns_native = managed_playing && runtime.playing.as_ref().is_some_and(|playing|
                playing_matches(playing, app) && !playing_superseded(playing, app));
            if runtime.pending_play_profile().is_some_and(|profile| profile.id == active.profile.id
                && profile.kind == active.profile.kind && profile.source == active.profile.source) {
                commands::cancel_pending_play(runtime);
                runtime.cancel_play_request();
            }
            if owns_native {
                playback::replace_player(&mut app.player, &mut app.adapters.player, &mut app.repause_at,
                    &mut app.refresh_hubs, &mut app.pages);
            }
            if managed_playing { runtime.release_active_lease(); runtime.playing = None; }
            // Only a still-bound unrelated source can retain a parked resume.
            // A row from the revoked catalogue cannot borrow the Saved fallback.
            runtime.resume_pending = runtime.resume_pending.take().filter(|request|
                (active.generation != runtime.generation || request.id.starts_with("favorite:"))
                    && runtime.prepare_play_request(request.clone()).is_ok_and(|(_, profile, _, _)|
                        profile.id != active.profile.id || profile.kind != active.profile.kind
                            || profile.source != active.profile.source));
            if active.generation == runtime.generation {
                // Never checkpoint a revoked temporary source into Saved, or
                // leave its rows available under the durable fallback owner.
                let resume = runtime.resume_pending.take();
                let unrelated_playback = runtime.playing.is_some();
                let continuity = unrelated_playback.then(|| std::mem::replace(&mut runtime.continuity,
                    continuity::Continuity::new(runtime.generation, None)));
                runtime.cancel_catalog();
                runtime.resume_pending = resume;
                if let Some(mut continuity) = continuity { continuity.rebase(runtime.generation); runtime.continuity = continuity; }
                runtime.catalog = Catalog::default();
                runtime.catalog_revision = runtime.catalog_revision.wrapping_add(1);
                if !runtime.saved.favorites_selected { runtime.selected = None; runtime.page = "profiles"; }
                runtime.status = "Plex source is no longer available".into();
                runtime.changed();
            }
        } else {
            if state.refresh_requested && active.refresh.is_none()
                && active.source.verified.proof.freshness() == plx_plex::plex::grant::ImportFreshness::NetworkChanged {
                let work = active.source.clone(); let (tx, rx) = mpsc::channel();
                if plx_base::task::spawn_off_frame("ott-managed-refresh", move |worker| {
                    let result = work.reverify(worker, Duration::from_secs(8), &work.retired);
                    let _ = tx.send(result); plx_machine::idle::invalidate();
                }) { active.refresh = Some(rx); }
            }
            if let Some(rx) = &active.refresh {
                match rx.try_recv() {
                    Ok(Ok(candidate)) => {
                        active.refresh = None;
                        if !candidate.retired.load(Ordering::Acquire)
                            && active.source.verified.proof.freshness() != plx_plex::plex::grant::ImportFreshness::Revoked
                            && app.bridge.refresh_managed(active.operation, candidate.verified.proof.clone()) {
                            active.source = candidate;
                        }
                    }
                    Ok(Err(_)) | Err(mpsc::TryRecvError::Disconnected) => active.refresh = None,
                    Err(mpsc::TryRecvError::Empty) => {}
                }
            }
            state.active = Some(active);
        }
    }
    state.refresh_requested = false;
    if let Some(mut pending) = state.pending.take() {
        let preflight = if pending.deadline <= runtime.now_ms() { Err(Error::Expired) }
            else if pending.invocation.as_ref().is_some_and(|inv| !runtime.remote.invocation_is_current(inv)) { Err(Error::Stale) }
            else if pending.cancel.load(Ordering::Acquire) || pending.navigation != runtime.navigation_epoch
                || pending.generation != runtime.generation { Err(Error::Cancelled) }
            else if !pending.staged && !state.authority.as_ref().is_some_and(|authority| authority.current(app)) { Err(Error::SourceChanged) }
            else { policy(runtime, app) };
        if let Err(error) = preflight {
            let applied = app.bridge.managed_stage(pending.operation).is_some_and(|stage|
                matches!(stage, crate::app::adapters::session::ManagedStage::EffectsStarted | crate::app::adapters::session::ManagedStage::RegistryApplied | crate::app::adapters::session::ManagedStage::Published | crate::app::adapters::session::ManagedStage::Retired));
            app.bridge.cancel_managed(pending.operation);
            settle(runtime, &mut state, pending, Some(if applied { Error::OutcomeUnknown } else { error }));
        } else if pending.staged {
            let refused = app.bridge.take_managed_admission_refusal().filter(|refusal|
                refusal.operation == pending.operation && refusal.expected_epoch == pending.expected_epoch);
            let mut completed = None;
            while let Some(receipt) = app.bridge.take_managed_completion() {
                if receipt.operation == pending.operation { completed = Some(receipt); }
            }
            if let Some(refusal) = refused {
                app.bridge.cancel_managed(pending.operation);
                settle(runtime, &mut state, pending, Some(adapter_error(refusal.error)));
            } else if let Some(receipt) = completed {
                if receipt.accepted && app.bridge.snapshot_session_init().managed_operation() == Some(pending.operation) {
                    runtime.stop_catalog(app); runtime.cancel_catalog();
                    runtime.transient.accepted = None;
                    runtime.catalog = Catalog::default(); runtime.catalog_revision = runtime.catalog_revision.wrapping_add(1);
                    runtime.generation = runtime.generation.wrapping_add(1); runtime.selected = None;
                    retire_for_catalog(app);
                    let source = &pending.source.verified.source;
                    let profile = ProviderProfile { id: format!("managed-{}", pending.operation), name: source.name.clone(),
                        kind: ProviderKind::Plex, source: source.machine_id.clone(), source_alias: String::new(), enabled: true };
                    runtime.provider = ProviderKind::Plex; runtime.page = "profiles";
                    state.active = Some(Active { operation: pending.operation, generation: runtime.generation,
                        profile, category: String::new(), favorites: runtime.saved.favorites_selected,
                        source: pending.source.clone(), refresh: None, cancel: pending.cancel.clone() });
                    state.candidate = None; state.row = None;
                    start_catalog = true;
                    settle(runtime, &mut state, pending, None);
                } else {
                    let changed = app.bridge.managed_stage(pending.operation).is_some_and(|stage|
                        matches!(stage, crate::app::adapters::session::ManagedStage::EffectsStarted | crate::app::adapters::session::ManagedStage::RegistryApplied | crate::app::adapters::session::ManagedStage::Published | crate::app::adapters::session::ManagedStage::Retired));
                    app.bridge.cancel_managed(pending.operation);
                    settle(runtime, &mut state, pending, Some(if changed { Error::OutcomeUnknown } else { receipt.error.map(adapter_error).unwrap_or(Error::ActivationFailed) }));
                }
            } else { state.pending = Some(pending); }
        } else {
            let result = pending.worker.as_ref().map(|rx| rx.try_recv());
            match result {
                Some(Ok(Ok(candidate))) => {
                    let ready = (|| {
                        state.observe(runtime, app)?;
                        if state.binding(runtime) != pending.binding { return Err(Error::SourceChanged); }
                        if !candidate.verified.proof.current() { return Err(Error::SourceChanged); }
                        state.offers.check(runtime.now_ms(), pending.binding, runtime.remote.runtime(), &pending.server_id, &pending.context).map_err(context_error)?;
                        let authority = state.authority.as_ref().ok_or(Error::SourceChanged)?;
                        let activation = ManagedActivation { operation: pending.operation, expected_epoch: authority.epoch,
                            expected_runtime: authority.credentials.clone(), expected_disk: authority.disk.clone(), credentials: candidate.credentials.clone() };
                        let command_deadline = pending.command_deadline.on_main(app.adapters.player.mt());
                        let context_deadline = pending.context_deadline.on_main(app.adapters.player.mt());
                        app.bridge.prepare_managed(activation.clone(), candidate.verified.source.clone(), candidate.verified.proof.clone(),
                            command_deadline, context_deadline, pending.cancel.clone())
                            .map_err(adapter_error)?;
                        state.offers.admit(runtime.now_ms(), pending.binding, runtime.remote.runtime(), &pending.server_id, &pending.context,
                            pending.operation, pending.deadline).map_err(context_error)?;
                        bridge::execute_session_command(&mut app.pages, plx_session::auth::SessionCmd::ActivateManaged(activation));
                        Ok(())
                    })();
                    match ready {
                        Ok(()) => { pending.source = candidate; pending.worker = None; pending.staged = true; state.pending = Some(pending); }
                        Err(error) => { app.bridge.cancel_managed(pending.operation); settle(runtime, &mut state, pending, Some(error)); }
                    }
                }
                Some(Ok(Err(error))) => {
                    if error == Error::SourceChanged { state.retire(); state.failure = Some(error); }
                    settle(runtime, &mut state, pending, Some(error));
                }
                Some(Err(mpsc::TryRecvError::Disconnected)) | None => {
                    settle(runtime, &mut state, pending, Some(Error::Unavailable));
                }
                Some(Err(mpsc::TryRecvError::Empty)) => state.pending = Some(pending),
            }
        }
    }
    if state.pending.is_none() && state.active.is_none() && runtime.page == "profiles" && runtime.provider == ProviderKind::Plex {
        if state.list(runtime, app).is_err() { state.row = None; }
    }
    // A persisted provider choice remains an intent even before its first profile
    // is configured. Missing selection is the initial-install Plex bootstrap case;
    // do not infer a choice from Runtime's default M3U presentation.
    let provider_allows_automatic = !matches!(
        runtime.saved.settings.get("selectedProvider").and_then(Value::as_str),
        Some("m3u" | "vportal" | "local" | "offline" | "xtream" | "stalker")
    );
    let auto = state.automatic && provider_allows_automatic
        && state.candidate.is_some() && runtime.loaded && !runtime.persistence_uncertain
        && runtime.remote.configuration_load_settled()
        && runtime.saved.profiles.active_profile.is_none() && !runtime.saved.favorites_selected
        && runtime.generation == 0 && runtime.navigation_epoch == 0 && runtime.playback_epoch == 0
        && runtime.playing.is_none() && runtime.transient.accepted.is_none() && app.last_input == state.initial_input;
    let original_intent = provider_allows_automatic
        && runtime.saved.profiles.active_profile.is_none() && !runtime.saved.favorites_selected
        && runtime.generation == 0 && runtime.navigation_epoch == 0 && runtime.playback_epoch == 0
        && runtime.playing.is_none() && runtime.transient.accepted.is_none() && app.last_input == state.initial_input;
    if state.automatic && runtime.loaded && !original_intent { state.automatic = false; }
    let automatic_row = if auto && state.pending.is_none() && state.active.is_none() {
        let _ = state.list(runtime, app); state.automatic = false; state.row.clone()
    } else { None };
    let fallback = state.automatic && original_intent && state.startup_settled
        && state.candidate.is_none() && state.pending.is_none() && state.active.is_none()
        && runtime.loaded && !runtime.persistence_uncertain
        && !app.bridge.snapshot_session_init().persisted.can_go_local();
    if fallback { state.automatic = false; }
    runtime.managed = state;
    reentry::observe(runtime, app);
    if start_catalog { runtime.managed_catalog(); }
    if fallback { bridge::execute_session_command(&mut app.pages, plx_session::auth::SessionCmd::StartLogin); }
    if let Some(row) = automatic_row { let _ = select(runtime, app, &row.server, &row.context); }
}

#[cfg(all(test, feature = "hostsim"))]
#[path = "ott_managed_startup_tests.rs"]
mod startup_tests;
