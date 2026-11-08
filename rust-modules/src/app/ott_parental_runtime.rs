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
    profiles: ProviderProfiles,
    authority: Option<transient::SourceAuthority>,
    roots: Option<Value>,
}
struct PendingVerification {
    request: EditorSession,
    ticket: parental::Ticket,
}
#[derive(Default)]
pub(super) struct State {
    gate: Option<Gate>,
    scope: Option<Scope>,
    policy: Option<Policy>,
    worker: Option<PendingVerification>,
    first_pin: Option<String>,
    suspended: bool,
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
        let scope_matches = self.parental.scope.as_ref().is_some_and(|scope| {
            scope.profile.as_ref() == self.effective_profile()
                && scope.profiles == self.saved.profiles
                && match (&scope.authority, &authority) {
                    (None, None) => true,
                    (Some(before), Some(current)) => before.same_authority(current),
                    _ => false,
                }
                && scope.roots.as_ref() == self.saved.settings.get("nativeLocalSources")
        });
        if self.parental.gate.is_none() {
            self.parental.gate = Some(Gate::new(self.entropy.ok_or(UNAVAILABLE)?));
        }
        if !scope_matches || self.parental.policy.as_ref() != Some(&policy) {
            self.parental.cancel();
            self.parental.scope = Some(Scope {
                profile: self.effective_profile().cloned(),
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
        profile.validate().map_err(|_| UNAVAILABLE)?;
        let policy = Policy::read(&self.saved.settings).map_err(|_| UNAVAILABLE)?;
        if policy.protects(Target::Channel { marked: true })
            && self.effective_profile() != Some(profile)
        {
            return Err("Select and unlock the target profile before playing protected content.");
        }
        // CatalogEntry has no protection membership yet. Never infer it from a
        // title/category or label an unknown item unprotected. An enabled channel
        // policy therefore requires the current source-bound grant.
        self.require_parental(Target::Channel { marked: true })
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

pub(super) fn tick(runtime: &mut Runtime, app: &App) {
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
}

fn open(runtime: &mut Runtime, app: &mut App, key: &str, title: &str) -> Result<(), &'static str> {
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
            runtime.editor_result(app, &request, None);
            runtime.status = "Parental access unlocked. Repeat the requested action.".into();
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
        OttAction::Play(_) | OttAction::PlayArchive { .. } | OttAction::ReturnLive { .. } => {
            Some(Target::Channel { marked: true })
        }
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
