//! App-owned PIN work and addressed use of the existing secret editor.
//! No rejected remote action is retained for replay after an unlock.
use super::*;
use parental::{Access, Context, Gate, Outcome, Pin, Policy, PolicyEdit, Target};
const LOCKED: &str = "Unlock parental access in Settings, then repeat this action.";
const UNAVAILABLE: &str = "Parental access is unavailable until protected settings are verified.";
const UNLOCK: &str = "parental.unlock";
const CREATE: &str = "parental.create";
const CONFIRM: &str = "parental.confirm";

#[derive(Clone)]
struct Scope {
    profile: Option<ProviderProfile>,
    target: Option<ProviderProfile>,
    target_authority: Option<transient::SourceAuthority>,
    profiles: ProviderProfiles,
    authority: Option<transient::SourceAuthority>,
    roots: Option<Value>,
}
struct PendingVerification {
    request: EditorSession,
    ticket: parental::Ticket,
}
#[derive(Clone)]
struct UiIntent {
    session: (u64, bool),
    owner: InstanceId,
    generation: u64,
    play_generation: u64,
    navigation: u64,
    playback_epoch: u64,
    ticket: plx_media::route::PlayRequestTicket,
    selection: String,
    profile: String,
    favorite: Option<Favorite>,
    effective: Option<String>,
    profiles: String,
    roots: Option<String>,
    authority: Option<transient::SourceAuthority>,
    target_authority: Option<transient::SourceAuthority>,
    policy: Policy,
    selected: Option<String>,
    deadline_ms: u64,
    editor: Option<EditorSession>,
}
struct ForeignPrompt {
    session: (u64, bool),
    origin: Option<InstanceId>,
    destination: Option<InstanceId>,
    generation: u64,
    navigation: u64,
    profile: String,
    effective: Option<String>,
    profiles: String,
    roots: Option<String>,
    authority: Option<transient::SourceAuthority>,
    active_authority: Option<transient::SourceAuthority>,
    policy: Policy,
    deadline_ms: u64,
}
#[derive(Default)]
pub(super) struct State {
    session: Option<(u64, bool)>,
    gate: Option<Gate>,
    scope: Option<Scope>,
    policy: Option<Policy>,
    worker: Option<PendingVerification>,
    first_pin: Option<String>,
    suspended: bool,
    target: Option<ProviderProfile>,
    origin: Option<UiIntent>,
    intent: Option<UiIntent>,
    foreign: Option<ForeignPrompt>,
}
impl State {
    fn cancel(&mut self) {
        if let Some(gate) = self.gate.as_mut() {
            gate.cancel();
        }
        self.worker = None;
        self.first_pin = None;
    }
}
impl Runtime {
    pub(super) fn retire_parental_completion(&mut self, completion: parental::Completion) {
        if let Some(gate) = self.parental.gate.as_mut() {
            gate.retire_completion(completion);
        }
    }
    fn parental_context(&mut self) -> Result<Context, &'static str> {
        if !self.loaded || self.persistence_uncertain {
            return Err(UNAVAILABLE);
        }
        let policy = Policy::read(&self.saved.settings).map_err(|_| UNAVAILABLE)?;
        let authority = match transient::parental_source_authority(self) {
            Ok(authority) => authority,
            Err(_) => {
                self.parental.cancel();
                return Err(UNAVAILABLE);
            }
        };
        let binding_matches = self.parental.scope.as_ref().is_some_and(|scope| {
            scope.profile.as_ref() == self.effective_profile()
                && scope.profiles == self.saved.profiles
                && match (&scope.authority, &authority) {
                    (None, None) => true,
                    (Some(before), Some(current)) => before.same_authority(current),
                    _ => false,
                }
                && scope.roots.as_ref() == self.saved.settings.get("nativeLocalSources")
        });
        let explicit_authority = self.parental.target.as_ref().map(|profile|
            transient::parental_profile_authority(self, profile)).transpose()
            .map_err(|_| UNAVAILABLE)?.flatten();
        let carry = !binding_matches && self.parental.worker.is_none()
            && self.parental.first_pin.is_none() && self.parental.policy.as_ref() == Some(&policy)
            && self.parental.scope.as_ref().is_some_and(|scope|
                self.parental.target.is_some()
                    && self.parental.target.as_ref() == self.effective_profile()
                    && scope.target == self.parental.target
                    && scope.profile.as_ref() != self.effective_profile()
                    && scope.profiles == self.saved.profiles
                    && scope.roots.as_ref() == self.saved.settings.get("nativeLocalSources")
                    && same_authority(scope.target_authority.as_ref(), explicit_authority.as_ref()))
            && {
                let now = self.now_ms();
                self.parental.gate.as_mut().is_some_and(|gate|
                    matches!(gate.access(&Context::new(0, policy.clone()),
                        Target::Channel { marked: true }, now), Access::Allowed))
            };
        if self.parental.scope.is_some() && !binding_matches && !carry {
            self.parental.target = None;
        }
        let target = self.parental.target.clone().or_else(|| self.effective_profile().cloned());
        let target_authority = target.as_ref().map(|profile|
            transient::parental_profile_authority(self, profile)).transpose()
            .map_err(|_| UNAVAILABLE)?.flatten();
        let scope_matches = (binding_matches || carry) && self.parental.scope.as_ref()
            .is_some_and(|scope| scope.target == target
                && same_authority(scope.target_authority.as_ref(), target_authority.as_ref()));
        if self.parental.gate.is_none() {
            self.parental.gate = Some(Gate::new(self.entropy.ok_or(UNAVAILABLE)?));
        }
        if !scope_matches || self.parental.policy.as_ref() != Some(&policy) || carry {
            if !carry { self.parental.cancel(); }
            else {
                // Consume the explicit A -> authorized-B transition without
                // minting a grant or extending its existing expiry. B -> A
                // cannot satisfy this predicate and retires it normally.
                self.parental.origin = None;
                self.parental.intent = None;
                self.parental.foreign = None;
            }
            self.parental.scope = Some(Scope {
                profile: self.effective_profile().cloned(),
                target,
                target_authority,
                profiles: self.saved.profiles.clone(),
                authority,
                roots: self.saved.settings.get("nativeLocalSources").cloned(),
            });
            self.parental.policy = Some(policy.clone());
        }
        // The Gate's own checked generation is retired at every real binding
        // replacement. Catalog repaint/navigation epochs are deliberately absent.
        Ok(Context::new(0, policy))
    }
    pub(super) fn require_parental(&mut self, target: Target) -> Result<(), &'static str> {
        if !self.loaded || self.persistence_uncertain {
            return Err(UNAVAILABLE);
        }
        let policy = Policy::read(&self.saved.settings).map_err(|_| UNAVAILABLE)?;
        if !policy.protects(target) {
            return Ok(());
        }
        let context = self.parental_context()?;
        let now = self.now_ms();
        match self
            .parental
            .gate
            .as_mut()
            .ok_or(UNAVAILABLE)?
            .access(&context, target, now)
        {
            Access::Allowed => Ok(()),
            Access::PinRequired => Err(LOCKED),
            Access::Unavailable => Err(UNAVAILABLE),
        }
    }
    pub(super) fn require_parental_control(&mut self) -> Result<(), &'static str> {
        self.require_parental(Target::Control)
    }
    pub(super) fn require_parental_playback(
        &mut self,
        profile: &ProviderProfile,
    ) -> Result<(), &'static str> {
        self.require_parental_entry(profile, None)
    }
    pub(super) fn require_parental_entry(
        &mut self,
        profile: &ProviderProfile,
        entry: Option<&CatalogEntry>,
    ) -> Result<(), &'static str> {
        profile.validate().map_err(|_| UNAVAILABLE)?;
        let target = Target::Channel { marked: parental_classification::marked(entry) };
        let policy = Policy::read(&self.saved.settings).map_err(|_| UNAVAILABLE)?;
        if policy.protects(target) {
            let _ = self.parental_context()?;
            if self.parental.target.as_ref().or_else(|| self.effective_profile()) != Some(profile) {
                return Err(LOCKED);
            }
        }
        self.require_parental(target)
    }
    fn require_parental_selection(&mut self, id: &str) -> Result<(), &'static str> {
        let (_, profile, entry, favorite) = self.prepare_play_request(
            PlaybackRequest::ordinary(id, 0))?;
        let policy = self.attachment_policy_profile(&profile, favorite.as_ref())?;
        self.require_parental_preparation(&policy, &profile, entry.as_ref(), favorite.as_ref())
    }
    pub(super) fn require_parental_candidate(
        &mut self,
        policy_profile: &ProviderProfile,
        source_profile: &ProviderProfile,
        entry: Option<&CatalogEntry>,
        favorite: Option<&Favorite>,
    ) -> Result<(), &'static str> {
        // Portable labels are not classification evidence. Only an exact,
        // uniquely resolved row in the currently bound source can supply it.
        let candidate = self.parental_candidate_entry(source_profile, entry, favorite);
        self.require_parental_entry(policy_profile, candidate.as_ref())
    }
    fn parental_candidate_entry(&self, source_profile: &ProviderProfile,
        entry: Option<&CatalogEntry>, favorite: Option<&Favorite>) -> Option<CatalogEntry> {
        entry.cloned().or_else(|| {
            if self.effective_profile() != Some(source_profile) { return None; }
            let favorite = favorite?;
            let mut matches = self.catalog.entries.iter()
                .filter(|entry| selection::matches_entry(favorite, entry));
            let entry = matches.next()?;
            matches.next().is_none().then(|| entry.clone())
        })
    }
    pub(super) fn require_parental_preparation(&mut self, policy_profile: &ProviderProfile,
        source_profile: &ProviderProfile, entry: Option<&CatalogEntry>, favorite: Option<&Favorite>)
        -> Result<(), &'static str>
    {
        let candidate = self.parental_candidate_entry(source_profile, entry, favorite);
        if candidate.is_none() && favorite.is_some() {
            // The source binder has validated this Favorite. Metadata lookup
            // grants no native Load; the resolved entry is gated again below.
            return Ok(());
        }
        self.require_parental_entry(policy_profile, candidate.as_ref())
    }
    pub(super) fn require_parental_resolved(&mut self, policy_profile: &ProviderProfile,
        source_profile: &ProviderProfile, current: Option<&CatalogEntry>, favorite: Option<&Favorite>,
        resolved: &CatalogEntry) -> Result<(), &'static str>
    {
        if let Some(current) = self.parental_candidate_entry(source_profile, current, favorite) {
            self.require_parental_entry(policy_profile, Some(&current))?;
        }
        self.require_parental_entry(policy_profile, Some(resolved))
    }
    /// Observe the actual new binding synchronously at every accepted install.
    /// Equal authority preserves the grant; A -> B -> A cannot skip B's fence.
    pub(super) fn parental_source_installed(&mut self) {
        if self.parental_context().is_err() {
            self.parental.cancel();
        }
    }
    pub(super) fn retire_parental_source(&mut self) {
        self.parental.cancel();
        self.parental.target = None;
        self.parental.origin = None;
        self.parental.intent = None;
        self.parental.foreign = None;
        self.parental.scope = None;
    }
    pub(super) fn parental_confirmed(&mut self) {
        let previous = self.parental.policy.clone();
        if let Ok(context) = self.parental_context() {
            if previous != self.parental.policy {
                let now = self.now_ms();
                if let Some(gate) = self.parental.gate.as_mut() {
                    gate.confirmed_policy(&context, now);
                }
            }
        } else {
            self.parental.cancel();
        }
    }
    pub(super) fn parental_authoritative_load(&mut self) {
        if let Ok(context) = self.parental_context() {
            let now = self.now_ms();
            if let Some(gate) = self.parental.gate.as_mut() {
                gate.confirmed_policy(&context, now);
            }
        }
    }
    pub(super) fn parental_uncertain(&mut self) {
        self.parental.cancel();
        if let Some(gate) = self.parental.gate.as_mut() {
            gate.write_uncertain();
        }
    }
}

fn same_authority(a: Option<&transient::SourceAuthority>, b: Option<&transient::SourceAuthority>) -> bool {
    match (a, b) { (None, None) => true, (Some(a), Some(b)) => a.same_authority(b), _ => false }
}
fn opaque(value: &impl serde::Serialize) -> Option<String> {
    Some(plx_data::ott::favorites_wire::hash(&serde_json::to_vec(value).ok()?))
}
fn root_identity(runtime: &Runtime) -> Option<Option<String>> {
    match runtime.saved.settings.get("nativeLocalSources") {
        Some(value) => Some(Some(opaque(value)?)),
        None => Some(None),
    }
}
fn session_identity(app: &App) -> (u64, bool) {
    (app.bridge.auth_read().0.flow_epoch, app.bridge.session_ready_current())
}
/// Session changes retire the existing grant; they never mint or extend one.
pub(super) fn observe_session(runtime: &mut Runtime, app: &App) {
    let current = session_identity(app);
    let changed = runtime.parental.session.replace(current).is_some_and(|old| old != current);
    let plex = runtime.effective_profile().is_some_and(|p| p.kind == ProviderKind::Plex)
        || runtime.parental.target.as_ref().is_some_and(|p| p.kind == ProviderKind::Plex)
        || runtime.parental.scope.as_ref().is_some_and(|scope|
            scope.target.as_ref().is_some_and(|p| p.kind == ProviderKind::Plex));
    if changed && plex {
        runtime.retire_parental_source();
    }
}
fn ui_intent(runtime: &Runtime, app: &App, request: PlaybackRequest) -> Option<UiIntent> {
    let owner = current_ui_owner(app)?;
    if runtime.reply != Some(owner) || !app.running || !playback_allowed(app)
        || app.bridge.is_controlled_replay() { return None; }
    let (request, profile, _, favorite) = runtime.prepare_play_request(request).ok()?;
    managed::guard_profile(runtime, app, &profile).ok()?;
    classic_source::guard_profile(runtime, app, &profile).ok()?;
    if request.archive.is_some() || request.continuation.is_some() || request.position_ms != 0 { return None; }
    Some(UiIntent { session: session_identity(app), owner, generation: runtime.generation, play_generation: runtime.play_generation,
        navigation: runtime.navigation_epoch, playback_epoch: runtime.playback_epoch,
        ticket: plx_media::route::play_request_ticket(), selection: request.id,
        profile: profile_fingerprint(&profile), favorite,
        effective: runtime.effective_profile().map(profile_fingerprint),
        profiles: opaque(&runtime.saved.profiles)?,
        roots: root_identity(runtime)?,
        authority: transient::parental_source_authority(runtime).ok()?,
        target_authority: transient::parental_profile_authority(runtime, &profile).ok()?,
        policy: Policy::read(&runtime.saved.settings).ok()?, selected: runtime.selected.clone(),
        deadline_ms: runtime.now_ms().saturating_add(120_000), editor: None })
}
fn intent_current(runtime: &Runtime, app: &App, intent: &UiIntent) -> bool {
    if !app.running || !playback_allowed(app) || app.bridge.is_controlled_replay()
        || session_identity(app) != intent.session
        || runtime.now_ms() >= intent.deadline_ms || runtime.generation != intent.generation
        || runtime.play_generation != intent.play_generation || runtime.navigation_epoch != intent.navigation
        || runtime.playback_epoch != intent.playback_epoch || runtime.selected != intent.selected
        || !plx_media::route::play_request_is_current(intent.ticket)
        || current_ui_owner(app) != Some(intent.owner) || runtime.reply != Some(intent.owner)
        || runtime.effective_profile().map(profile_fingerprint) != intent.effective
        || opaque(&runtime.saved.profiles).as_ref() != Some(&intent.profiles)
        || root_identity(runtime).as_ref() != Some(&intent.roots)
        || Policy::read(&runtime.saved.settings).ok().as_ref() != Some(&intent.policy)
        || intent.editor.as_ref().is_some_and(|editor| runtime.edit_session.as_ref() != Some(editor))
    { return false; }
    let Ok(authority) = transient::parental_source_authority(runtime) else { return false; };
    if !same_authority(authority.as_ref(), intent.authority.as_ref()) { return false; }
    runtime.prepare_play_request(intent_request(intent)).is_ok_and(|(_, profile, _, favorite)|
        profile_fingerprint(&profile) == intent.profile && favorite == intent.favorite
            && transient::parental_profile_authority(runtime, &profile).is_ok_and(|authority|
                same_authority(authority.as_ref(), intent.target_authority.as_ref())))
}
fn intent_request(intent: &UiIntent) -> PlaybackRequest {
    let mut request = PlaybackRequest::ordinary(&intent.selection, 0);
    request.advance_on_failure = false;
    request
}
pub(super) fn cancel_ui_origin(runtime: &mut Runtime) {
    runtime.parental.origin = None;
    runtime.parental.intent = None;
    runtime.parental.foreign = None;
}
/// Only the local dispatcher may mark a pending request as PIN-interactive.
pub(super) fn mark_ui_origin(runtime: &mut Runtime, app: &App) {
    runtime.parental.origin = runtime.play_target.clone().and_then(|request| ui_intent(runtime, app, request));
}
pub(super) fn seal_ui_origin(runtime: &mut Runtime) {
    if let Some(origin) = runtime.parental.origin.as_mut() {
        if origin.play_generation == runtime.play_generation
            && runtime.play_target.as_ref().is_some_and(|request| request.id == origin.selection)
            && origin.navigation.wrapping_add(1) == runtime.navigation_epoch
        { origin.navigation = runtime.navigation_epoch; }
    }
}
pub(super) fn seal_submission(runtime: &mut Runtime) {
    if let Some(intent) = runtime.parental.intent.as_mut() {
        if intent.navigation.wrapping_add(1) == runtime.navigation_epoch
            && runtime.parental.worker.as_ref().is_some_and(|worker|
                intent.editor.as_ref() == Some(&worker.request))
        { intent.navigation = runtime.navigation_epoch; }
    }
}
pub(super) fn is_pin_required(error: &str) -> bool { error == LOCKED }
fn select_ui_pin_target(runtime: &mut Runtime, intent: &UiIntent) -> Result<(), &'static str> {
    let (_, profile, _, favorite) = runtime.prepare_play_request(intent_request(intent))?;
    let policy = runtime.attachment_policy_profile(&profile, favorite.as_ref())?;
    let _ = runtime.parental_context()?;
    runtime.parental.target = Some(policy);
    runtime.parental_context().map(|_| ())
}
pub(super) fn resolved_pin_required(runtime: &mut Runtime, app: &App) {
    if runtime.pending_play.is_some() { return; }
    let Some(origin) = runtime.parental.origin.take() else { return; };
    if intent_current(runtime, app, &origin) && select_ui_pin_target(runtime, &origin).is_ok() {
        runtime.parental.intent = Some(origin);
    }
}
pub(super) fn present_pending(runtime: &mut Runtime, app: &mut App) {
    let Some(mut intent) = runtime.parental.intent.take() else { return; };
    if !intent_current(runtime, app, &intent) { return; }
    if intent.editor.is_some() || runtime.saving {
        runtime.parental.intent = Some(intent); return;
    }
    match open(runtime, app, UNLOCK, "Parental access PIN") {
        Ok(()) => {
            intent.editor = runtime.edit_session.clone();
            runtime.parental.intent = Some(intent);
        }
        Err(error) => { runtime.status = error.into(); }
    }
}
/// Alternate native screens may navigate to the real Settings/PIN owner.
/// This retains no media request and never replays native or remote playback.
pub(super) fn request_foreign_pin(runtime: &mut Runtime, app: &mut App, profile: &ProviderProfile) {
    observe_session(runtime, app);
    let configured = runtime.saved.profiles.profiles.iter().any(|saved| saved == profile && saved.enabled)
        || classic_source::profile(runtime) == Some(profile) || managed::binding_profile(runtime) == Some(profile)
        || runtime.transient.accepted.as_ref().is_some_and(|source| source.profile() == profile);
    if !configured || !app.running || !playback_allowed(app)
        || app.bridge.is_controlled_replay() { return; }
    if managed::guard_profile(runtime, app, profile).is_err()
        || classic_source::guard_profile(runtime, app, profile).is_err() { return; }
    let prepare = (|| {
        let _ = runtime.parental_context().ok()?;
        runtime.parental.target = Some(profile.clone());
        let _ = runtime.parental_context().ok()?;
        Some(ForeignPrompt { session: session_identity(app), origin: app.pages.top_page(),
            destination: matches!(app.route(), AppArg::Ott).then(|| app.pages.top_page()).flatten(),
            generation: runtime.generation,
            navigation: runtime.navigation_epoch, profile: profile_fingerprint(profile),
            effective: runtime.effective_profile().map(profile_fingerprint),
            profiles: opaque(&runtime.saved.profiles)?, roots: root_identity(runtime)?,
            authority: transient::parental_profile_authority(runtime, profile).ok()?,
            active_authority: transient::parental_source_authority(runtime).ok()?,
            policy: Policy::read(&runtime.saved.settings).ok()?,
            deadline_ms: runtime.now_ms().saturating_add(120_000) })
    })();
    let Some(prompt) = prepare else { return; };
    runtime.cancel_setup();
    runtime.page = "settings";
    runtime.parental.foreign = Some(prompt);
    if !matches!(app.route(), AppArg::Ott) { bridge::nav_push(&mut app.pages, AppArg::Ott); }
    runtime.changed();
}
fn present_foreign(runtime: &mut Runtime, app: &mut App) {
    let Some(mut prompt) = runtime.parental.foreign.take() else { return; };
    if !app.running || !playback_allowed(app) || session_identity(app) != prompt.session
        || runtime.now_ms() >= prompt.deadline_ms
        || runtime.generation != prompt.generation || runtime.navigation_epoch != prompt.navigation
        || runtime.effective_profile().map(profile_fingerprint) != prompt.effective
        || opaque(&runtime.saved.profiles).as_ref() != Some(&prompt.profiles)
        || root_identity(runtime).as_ref() != Some(&prompt.roots)
        || Policy::read(&runtime.saved.settings).ok().as_ref() != Some(&prompt.policy)
    { return; }
    if prompt.destination.is_some() && prompt.destination != app.pages.top_page() { return; }
    if !transient::parental_source_authority(runtime).is_ok_and(|authority|
        same_authority(authority.as_ref(), prompt.active_authority.as_ref())) { return; }
    let Some(profile) = runtime.parental.target.clone()
        .filter(|profile| profile_fingerprint(profile) == prompt.profile) else { return; };
    if !transient::parental_profile_authority(runtime, &profile).is_ok_and(|authority|
        same_authority(authority.as_ref(), prompt.authority.as_ref())) { return; }
    if matches!(app.route(), AppArg::Ott) {
        if prompt.destination.is_none() { prompt.destination = app.pages.top_page(); }
        if runtime.page != "settings" || runtime.edit_session.is_some() { return; }
        if current_ui_owner(app).is_some_and(|owner| runtime.reply == Some(owner)) && !runtime.saving {
            if let Err(error) = open(runtime, app, UNLOCK, "Parental access PIN") {
                runtime.status = error.into();
            }
            return;
        }
    } else if app.pages.top_page() != prompt.origin
        || (!app.pages.has_pending_navigation() && !app.pages.nav.tabs.stack.is_pending()
            && !app.pages.nav.tabs.stack.transition.in_flight()) { return; }
    runtime.parental.foreign = Some(prompt);
}
/// Only the local Play action that opened this prompt may publish its one
/// navigation step. Remote actions never call this presentation adapter.
pub(super) fn seal_foreign_navigation(runtime: &mut Runtime) {
    if let Some(prompt) = runtime.parental.foreign.as_mut() {
        if prompt.generation == runtime.generation
            && prompt.navigation.checked_add(1) == Some(runtime.navigation_epoch)
        { prompt.navigation = runtime.navigation_epoch; }
    }
}
pub(super) fn tick(runtime: &mut Runtime, app: &mut App) {
    observe_session(runtime, app);
    let suspended = !playback_allowed(app);
    if suspended && !runtime.parental.suspended {
        runtime.parental.cancel();
    }
    runtime.parental.suspended = suspended;
    if runtime.parental.worker.as_ref().is_some_and(|request| {
        current_ui_owner(app) != Some(request.request.owner)
            || runtime.reply != Some(request.request.owner)
    }) {
        runtime.parental.cancel();
    }
    // Observe every accepted binding before processing addressed completions.
    if runtime.parental_context().is_err() {
        runtime.parental.cancel();
    }
    if runtime.parental.origin.as_ref().is_some_and(|intent| !intent_current(runtime, app, intent)) {
        runtime.parental.origin = None;
    }
    present_pending(runtime, app);
    present_foreign(runtime, app);
}

fn open(runtime: &mut Runtime, app: &mut App, key: &str, title: &str) -> Result<(), &'static str> {
    observe_session(runtime, app);
    if let Some(error) = runtime.settings_write_refusal() {
        return Err(error);
    }
    let _ = runtime.parental_context()?;
    let owner = runtime
        .reply
        .ok_or("Open parental access from the current Settings screen.")?;
    if current_ui_owner(app) != Some(owner) {
        return Err("Parental editor is no longer current.");
    }
    let id = runtime.edit_serial.checked_add(1).ok_or(UNAVAILABLE)?;
    runtime.edit_serial = id;
    runtime.edit_session = Some(EditorSession {
        id,
        owner,
        key: key.into(),
        purpose: search::EditorPurpose::Setting,
    });
    app.pages.emit(
        MachineId::Nav,
        Fx::Deliver(
            MachineId::Instance(owner),
            Delivery::Screen(plx_ui::screen::ScreenEvent::App(AppMsg::OttEditor(
                Arc::new(OttEditor {
                    session: id,
                    kind: plx_screens::ott::OttEditorKind::Setting,
                    key: key.into(),
                    title: title.into(),
                    value: String::new(),
                    secret: true,
                    limit: 4,
                    text_policy: OttTextPolicy::Literal,
                }),
            ))),
        ),
    );
    Ok(())
}
fn current_request(
    runtime: &Runtime,
    session: u64,
    key: &str,
) -> Result<EditorSession, &'static str> {
    runtime
        .edit_session
        .as_ref()
        .filter(|s| {
            s.id == session
                && s.key == key
                && Some(s.owner) == runtime.reply
                && s.purpose == search::EditorPurpose::Setting
        })
        .cloned()
        .ok_or("Parental editor is no longer current.")
}
fn start_with(
    runtime: &mut Runtime,
    request: EditorSession,
    work: parental::Work,
    spawn: impl FnOnce(parental::Work, Sender<Job>) -> bool,
) -> Result<(), &'static str> {
    let ticket = work.ticket();
    runtime.parental.worker = Some(PendingVerification { request, ticket });
    if !spawn(work, runtime.tx.clone()) {
        runtime.parental.worker = None;
        if let Some(gate) = runtime.parental.gate.as_mut() {
            gate.retire_completion(parental::Completion::failed(ticket));
        }
        return Err("Could not start PIN verification.");
    }
    Ok(())
}
fn start(
    runtime: &mut Runtime,
    request: EditorSession,
    work: parental::Work,
) -> Result<(), &'static str> {
    start_with(runtime, request, work, |work, sender| {
        plx_base::task::spawn_off_frame("ott-pin", move |off_frame| {
            let ticket = work.ticket();
            let result =
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| work.run(off_frame)))
                    .unwrap_or_else(|_| parental::Completion::failed(ticket));
            let _ = sender.send(Job::Parental(result));
            plx_machine::idle::invalidate();
        })
    })
}
fn salt_for(entropy: Option<[u8; 16]>, session: u64) -> Result<[u8; 16], &'static str> {
    if session == 0 {
        return Err(UNAVAILABLE);
    }
    let mut seed = b"ott-native-parental-salt-v1\0".to_vec();
    seed.extend_from_slice(&entropy.ok_or(UNAVAILABLE)?);
    seed.extend_from_slice(&session.to_le_bytes());
    let digest = plx_base::sha256::sha256(&seed);
    let mut salt = [0; 16];
    salt.copy_from_slice(&digest[..16]);
    Ok(salt)
}
fn submit(
    runtime: &mut Runtime,
    app: &mut App,
    request: EditorSession,
    value: &str,
) -> Result<(), &'static str> {
    if let Some(error) = runtime.settings_write_refusal() {
        return Err(error);
    }
    let pin = Pin::parse(value).map_err(|_| "Enter exactly four digits.")?;
    let context = runtime.parental_context()?;
    let now = runtime.now_ms();
    match request.key.as_str() {
        UNLOCK => {
            let work = runtime
                .parental
                .gate
                .as_mut()
                .ok_or(UNAVAILABLE)?
                .verify(&context, pin, now)
                .map_err(|_| "PIN verification is unavailable or already in progress.")?;
            start(runtime, request, work)
        }
        CREATE => {
            runtime.require_parental_control()?;
            runtime.parental.first_pin = Some(value.into());
            open(runtime, app, CONFIRM, "Confirm new PIN")
        }
        CONFIRM => {
            let first = runtime
                .parental
                .first_pin
                .take()
                .ok_or("Enter the new PIN again.")?;
            if !plx_base::sha256::ct_eq(first.as_bytes(), value.as_bytes()) {
                return Err("PINs do not match. Start again.");
            }
            let salt = salt_for(runtime.entropy, request.id)?;
            let work = runtime
                .parental
                .gate
                .as_mut()
                .ok_or(UNAVAILABLE)?
                .create(&context, pin, salt, now)
                .map_err(|_| "PIN change requires current access and an idle verifier.")?;
            start(runtime, request, work)
        }
        _ => Err("Unknown parental editor."),
    }
}
fn save_change(runtime: &mut Runtime, change: parental::Change) -> Result<(), &'static str> {
    let context = runtime.parental_context()?;
    let now = runtime.now_ms();
    let settings = runtime
        .parental
        .gate
        .as_mut()
        .ok_or(UNAVAILABLE)?
        .candidate(&context, change, &runtime.saved.settings, now)
        .map_err(|_| "Parental settings changed. Unlock and try again.")?;
    let mut saved = runtime.saved.clone();
    saved.settings = settings;
    runtime.save(saved, None, false)
}
pub(super) fn completed(runtime: &mut Runtime, app: &mut App, completion: parental::Completion) {
    observe_session(runtime, app);
    let ticket = completion.ticket();
    if !runtime
        .parental
        .worker
        .as_ref()
        .is_some_and(|pending| pending.ticket == ticket)
    {
        runtime.retire_parental_completion(completion);
        return;
    }
    let request = runtime
        .parental
        .worker
        .take()
        .map(|pending| pending.request);
    let context = match runtime.parental_context() {
        Ok(context) => context,
        Err(_) => {
            runtime.parental.cancel();
            if let Some(gate) = runtime.parental.gate.as_mut() {
                gate.retire_completion(completion);
            }
            return;
        }
    };
    let owned = request.as_ref().is_some_and(|request| {
        runtime.edit_session.as_ref() == Some(request)
            && current_ui_owner(app) == Some(request.owner)
            && runtime.reply == Some(request.owner)
            && playback_allowed(app)
    });
    if !owned {
        if let Some(gate) = runtime.parental.gate.as_mut() {
            gate.cancel();
        }
    }
    let now = runtime.now_ms();
    let outcome = runtime
        .parental
        .gate
        .as_mut()
        .unwrap()
        .complete(&context, completion, now);
    if !owned {
        return;
    }
    let request = request.unwrap();
    match outcome {
        Outcome::Granted => {
            let intent = runtime.parental.intent.take().filter(|intent|
                intent.editor.as_ref() == Some(&request) && intent_current(runtime, app, intent));
            runtime.editor_result(app, &request, None);
            if let Some(intent) = intent {
                // Consume this UI-only intent before starting fresh resolution.
                // No ReadySource, worker result or remote invocation is replayed.
                match runtime.play_request_with_policy(app, intent_request(&intent)) {
                    Ok(()) => mark_ui_origin(runtime, app),
                    Err(error) => runtime.status = error.into(),
                }
            } else {
                runtime.status = "Parental access unlocked. Repeat the requested action.".into();
            }
        }
        Outcome::Prepared(change) => match save_change(runtime, change) {
            Ok(()) => runtime.pending_editor_save = Some(request),
            Err(error) => runtime.editor_result(app, &request, Some(error.into())),
        },
        Outcome::Denied => runtime.editor_result(app, &request, Some("Incorrect PIN.".into())),
        Outcome::Unavailable | Outcome::Ignored => runtime.editor_result(
            app,
            &request,
            Some("PIN verification expired. Try again.".into()),
        ),
    }
    runtime.changed();
}

/// A repeated addressed submit cannot acknowledge or replace an active PIN job.
pub(super) fn submission_pending(runtime: &Runtime, owner: InstanceId, session: u64) -> bool {
    runtime.parental.worker.as_ref().is_some_and(|pending|
        pending.request.owner == owner && pending.request.id == session)
}

pub(super) fn action(
    runtime: &mut Runtime,
    app: &mut App,
    action: &OttAction,
) -> Option<Result<(), &'static str>> {
    match action {
        OttAction::EditSetting(key) if key.starts_with("parental.") => Some((|| {
            if key == UNLOCK {
                return open(runtime, app, UNLOCK, "Parental access PIN");
            }
            runtime.require_parental_control()?;
            if key == CREATE {
                return open(runtime, app, CREATE, "New parental PIN");
            }
            let edit = match key.as_str() {
                "parental.disable" => PolicyEdit::Disable,
                "parental.channels" => PolicyEdit::Channels(
                    runtime
                        .saved
                        .settings
                        .get("psChannels")
                        .and_then(Value::as_u64)
                        .unwrap_or(1)
                        == 0,
                ),
                "parental.settings" => PolicyEdit::Settings(
                    runtime
                        .saved
                        .settings
                        .get("psOptions")
                        .and_then(Value::as_u64)
                        != Some(1),
                ),
                "parental.providers" => PolicyEdit::Providers(
                    runtime
                        .saved
                        .settings
                        .get("requirePinForProviderSelection")
                        .and_then(Value::as_u64)
                        != Some(1),
                ),
                _ => return Err("Unknown parental setting."),
            };
            let context = runtime.parental_context()?;
            let now = runtime.now_ms();
            let change = runtime
                .parental
                .gate
                .as_mut()
                .ok_or(UNAVAILABLE)?
                .prepare(&context, edit, now)
                .map_err(|_| LOCKED)?;
            save_change(runtime, change)
        })()),
        OttAction::SubmitSetting {
            session,
            key,
            value,
        } if key.starts_with("parental.") => Some((|| {
            let request = current_request(runtime, *session, key)?;
            let result = submit(runtime, app, request.clone(), value);
            if let Err(error) = result {
                runtime.editor_result(app, &request, Some(error.into()));
            }
            result
        })()),
        OttAction::CancelSetting { session }
            if runtime.edit_session.as_ref().is_some_and(|s| {
                s.id == *session && s.key.starts_with("parental.") && Some(s.owner) == runtime.reply
            }) =>
        {
            runtime.parental.cancel();
            runtime.cancel_editor();
            Some(Ok(()))
        }
        _ => None,
    }
}

pub(super) fn ui_gate(
    runtime: &mut Runtime,
    app: &mut App,
    action: &OttAction,
) -> Result<(), &'static str> {
    observe_session(runtime, app);
    if companion_adapter::owns_generic_play(runtime, action) {
        // These are live adapter-owned session/result rows, rather than
        // catalogue entries. Preserve their existing Settings policy domain.
        return ui_gate(runtime, app, &OttAction::EditSetting("torrent.selection".into()));
    }
    if let OttAction::Play(id) = action {
        return match runtime.require_parental_selection(id) {
            Err(LOCKED) => {
                if current_ui_owner(app).is_none() || current_ui_owner(app) != runtime.reply {
                    let (_, profile, _, favorite) = runtime.prepare_play_request(
                        PlaybackRequest::ordinary(id, 0))?;
                    let profile = runtime.attachment_policy_profile(&profile, favorite.as_ref())?;
                    request_foreign_pin(runtime, app, &profile);
                    return Err(LOCKED);
                }
                let mut request = PlaybackRequest::ordinary(id, 0);
                request.advance_on_failure = false;
                let mut intent = ui_intent(runtime, app, request);
                if let Some(intent) = &intent { select_ui_pin_target(runtime, intent)?; }
                open(runtime, app, UNLOCK, "Parental access PIN")?;
                if let Some(intent) = intent.as_mut() { intent.editor = runtime.edit_session.clone(); }
                runtime.parental.intent = intent;
                Err(LOCKED)
            }
            result => result,
        };
    }
    if matches!(action, OttAction::PlayArchive { .. } | OttAction::ReturnLive { .. }) {
        let target = guide_ui::parental_target(runtime);
        let result = match &target {
            Some((profile, entry)) => runtime.require_parental_entry(profile, Some(entry)),
            None => runtime.require_parental(Target::Channel { marked: true }),
        };
        return match result {
            Err(LOCKED) => {
                if let Some((profile, _)) = target {
                    let _ = runtime.parental_context()?;
                    runtime.parental.target = Some(profile);
                    let _ = runtime.parental_context()?;
                }
                open(runtime, app, UNLOCK, "Parental access PIN")?;
                Err(LOCKED)
            }
            result => result,
        };
    }
    let target = match action {
        OttAction::ShowActions(_) | OttAction::ShowActionsAt { .. } | OttAction::ShuffleOrder(_) | OttAction::ResetOrder(_)
        | OttAction::MenuVisibility { .. } | OttAction::ShowSettings | OttAction::ShowM3uMediaLinks
        | OttAction::EditProfile { .. } | OttAction::EditM3uMediaLink { .. }
        | OttAction::EditSetting(_) | OttAction::SubmitSetting { .. } => {
            Some(Target::Settings)
        }
        OttAction::ShowProviders
        | OttAction::ShowProfiles
        | OttAction::SelectProvider(_)
        | OttAction::SelectProfile(_) | OttAction::SelectManagedPlex { .. }
        | OttAction::ResumeManagedPlex { .. } => Some(Target::Providers),
        _ => None,
    };
    if let Some(target) = target {
        if let Err(error) = runtime.require_parental(target) {
            if error == LOCKED {
                open(runtime, app, UNLOCK, "Parental access PIN")?;
            }
            return Err(error);
        }
    }
    Ok(())
}

#[cfg(all(test, feature = "hostsim"))]
#[path = "ott_parental_runtime_tests.rs"]
mod tests;

#[cfg(all(test, feature = "hostsim"))]
pub(super) fn grant_current_profile_for_test(app: &mut App) {
    let runtime = app.ott.as_mut().unwrap();
    let context = runtime.parental_context().unwrap();
    let now = runtime.now_ms();
    let gate = runtime.parental.gate.as_mut().unwrap();
    let work = gate.verify(&context, Pin::parse("2468").unwrap(), now).unwrap();
    let ticket = work.ticket();
    drop(work); // Model one verified worker; real derivation has separate vector tests.
    assert!(matches!(gate.complete(&context, parental::Completion::verified_fixture(ticket), now), Outcome::Granted));
}

#[cfg(all(test, feature = "hostsim"))]
pub(super) fn expire_current_profile_for_test(app: &mut App) {
    let runtime = app.ott.as_mut().unwrap();
    let context = runtime.parental_context().unwrap();
    let expiry = runtime.now_ms().saturating_add(3_600_000);
    assert!(matches!(runtime.parental.gate.as_mut().unwrap()
        .access(&context, Target::Control, expiry), Access::PinRequired));
}
