//! Optional return from an explicitly addressed login. This is logical owner
//! state, never a credential, destination, persistence grant or registry restore.
use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoginOrigin { pub id: u64, pub flow_epoch: u64 }

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LoginCancelDisposition {
    Returned { epoch: u64, scope: ProfileScope },
    Stale,
    Blocked,
}

/// An external authority boundary observed synchronously by the application.
/// This is a closed observation, not permission to restore or mutate credentials.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LoginOriginBoundary { NetworkChanged }

/// Private comparison evidence captured by this owner, not supplied by the app.
/// No Debug implementation: the identities contain credentials.
#[derive(Clone, Serialize, Deserialize)]
pub struct LoginOriginState {
    origin: LoginOrigin,
    phase: Phase,
    picker: Picker,
    identity: Identity,
    disk_identity: Identity,
    authority: String,
    scope: ProfileScope,
    profile_uuid: Option<String>,
    returned_epoch: Option<u64>,
}

fn authority(state: &SessionInit) -> String {
    serde_json::to_string(&state.authority).expect("typed bootstrap authority serializes")
}
fn blocked(state: &SessionInit) -> bool {
    state.pending_commit.is_some() || state.admitted_persistence.is_some()
        || state.unconfirmed_fresh_prior.is_some() || state.persistence_warning.is_some()
        || state.held_handoff.is_some() || state.pending_erase.is_some()
        || state.persistence_purpose.is_some()
}

impl LoginOriginState {
    fn capture(state: &SessionInit, id: u64) -> Option<Self> {
        if id == 0 || blocked(state) || state.managed.is_some()
            || !matches!(state.authority, BootstrapAuthority::Account { .. })
            || state.apply_pending || state.signin_active
            || !state.pending.is_empty() || !state.inbox.is_empty()
            || (state.commit_phase.admitted && !state.commit_phase.durable) {
            return None;
        }
        let stable = match state.phase {
            Phase::Idle => state.active_profile.is_none() && !state.authorized_in_flow,
            Phase::Ready => state.persisted.can_go_local() && state.active_profile.as_ref()
                .is_some_and(|profile| !profile.uuid.is_empty() && profile.uuid == state.persisted.user.uuid),
            _ => false,
        };
        let identity = state.committed_credentials.identity();
        if !stable || !identity.matches(&state.persisted) || identity != state.disk_identity { return None; }
        Some(Self { origin: LoginOrigin { id, flow_epoch: state.epoch }, phase: state.phase,
            picker: state.picker, identity, disk_identity: state.disk_identity.clone(),
            authority: authority(state), scope: state.profile_scope,
            profile_uuid: state.active_profile.as_ref().map(|profile| profile.uuid.clone()), returned_epoch: None })
    }
    fn unchanged(&self, state: &SessionInit) -> bool {
        !blocked(state) && state.managed.is_none()
            && matches!(state.authority, BootstrapAuthority::Account { .. })
            && self.authority == authority(state) && self.scope == state.profile_scope
            && self.identity.matches(&state.persisted)
            && self.identity == state.committed_credentials.identity()
            && self.disk_identity == state.disk_identity
            && self.profile_uuid.as_deref() == state.active_profile.as_ref().map(|profile| profile.uuid.as_str())
            && !state.apply_pending && !state.authorized_in_flow
    }
    fn cancellable(&self, state: &SessionInit) -> bool {
        self.returned_epoch.is_none() && self.origin.flow_epoch == state.epoch
            && self.unchanged(state) && state.picker == Picker::Boot
            && matches!(state.phase, Phase::Creating | Phase::Waiting | Phase::Error)
            && state.pending.values().all(|pending|
                pending.key.epoch == state.epoch && pending.key.op == SessionOp::Login)
    }
    pub(super) fn write(&self, w: &mut Canon) {
        w.str("login-origin-v1").u64(self.origin.id).u64(self.origin.flow_epoch)
            .u8(match self.phase { Phase::Idle => 0, Phase::Creating => 1, Phase::Waiting => 2,
                Phase::Discovering => 3, Phase::Profiles => 4, Phase::Switching => 5,
                Phase::Ready => 6, Phase::Error => 7, Phase::Deleted => 8 })
            .u8(match self.picker { Picker::Boot => 0, Picker::SignedIn => 1, Picker::ChangeProfile => 2 })
            .str(&self.identity.client_id).str(&self.identity.account_token).str(&self.identity.profile_uuid)
            .str(&self.disk_identity.client_id).str(&self.disk_identity.account_token).str(&self.disk_identity.profile_uuid)
            .str(&self.authority).u32(self.scope.0);
        w.option(self.profile_uuid.as_deref(), |w, id| { w.str(id); });
        w.option(self.returned_epoch, |w, epoch| { w.u64(epoch); });
    }
}

impl SessionInit {
    pub(super) fn origin_cancel(&self) -> Option<LoginOrigin> {
        self.login_origin.as_ref().filter(|origin| origin.cancellable(self)).map(|origin| origin.origin)
    }
}

impl SessionMachine {
    /// Retire both an active cancellation origin and an unconsumed return receipt
    /// before the host changes network grants. This cannot be queued behind a
    /// CancelLoginFrom. The host observes the same boundary on recorded DID
    /// foreground; no platform or registry operation is performed here.
    ///
    /// Returns whether logical state changed. Publication and cached Canon state
    /// are updated synchronously, without changing epoch, work or credentials.
    pub fn observe_login_origin_boundary(&mut self, boundary: LoginOriginBoundary) -> bool {
        match boundary { LoginOriginBoundary::NetworkChanged => {} }
        if self.state.login_origin.is_none() { return false; }
        self.retire_login_origin();
        self.refresh_subhash();
        true
    }

    pub(super) fn retire_login_origin(&mut self) {
        if self.state.login_origin.take().is_some() { self.replace_publication(); }
    }

    pub(super) fn start_login_from(&mut self, id: u64, reply: ReplyTo, emit: &mut impl FnMut(SessionFx)) -> bool {
        if id == 0 || blocked(&self.state) {
            emit(SessionFx::LoginOriginStarted { to: reply,
                origin: LoginOrigin { id, flow_epoch: self.state.epoch }, accepted: false, returnable: false });
            return true;
        }
        let captured = LoginOriginState::capture(&self.state, id);
        // Dev/managed and nonstable entries retain the original login behavior,
        // but never receive comparison evidence for a no-mutation return.
        let accepted = self.restart_login(true, emit);
        let origin = LoginOrigin { id, flow_epoch: self.state.epoch };
        if accepted {
            self.state.login_origin = captured.map(|mut captured| { captured.origin = origin; captured });
            self.replace_publication();
        }
        emit(SessionFx::LoginOriginStarted { to: reply, origin, accepted,
            returnable: accepted && self.state.origin_cancel() == Some(origin) });
        true
    }

    pub(super) fn cancel_login_from(&mut self, origin: LoginOrigin, reply: ReplyTo,
        emit: &mut impl FnMut(SessionFx)) -> bool {
        let current = self.state.login_origin.as_ref().filter(|current|
            current.origin == origin && current.returned_epoch.is_none()
                && origin.flow_epoch == self.state.epoch).cloned();
        let disposition = match current {
            None => LoginCancelDisposition::Stale,
            Some(current) if !current.cancellable(&self.state) || self.state.epoch.checked_add(1).is_none() => {
                self.retire_login_origin();
                LoginCancelDisposition::Blocked
            }
            Some(mut current) => {
                // No write/registry handoff can be pending here. Advance the
                // epoch before retiring queued observations and late PIN work.
                let epoch = self.advance_epoch(emit).expect("origin cancellation epoch preflight");
                if std::mem::take(&mut self.state.signin_active) {
                    emit(SessionFx::Coordinator(CoordinatorAction::SignInCancelled));
                }
                self.state.phase = current.phase;
                self.state.picker = current.picker;
                self.state.apply_pending = false;
                self.state.pin_code.clear();
                self.state.qr_png.clear();
                self.state.qr_gen = 0;
                self.state.users.clear();
                self.state.error.clear();
                self.state.pin_denied = false;
                self.state.authorized_in_flow = false;
                self.state.code_replaced = false;
                self.state.link_trouble = false;
                self.state.plaintext = None;
                self.state.signin_account = None;
                current.returned_epoch = Some(epoch);
                let scope = self.state.profile_scope;
                self.state.login_origin = Some(current);
                self.replace_publication();
                LoginCancelDisposition::Returned { epoch, scope }
            }
        };
        emit(SessionFx::LoginOriginCancelled { to: reply, origin, disposition });
        true
    }

    /// Revalidate just before the app consumes its own one-shot continuation.
    /// This proves only the Session boundary; the app still owns source/route checks.
    pub fn origin_return_is_current(&self, origin: LoginOrigin, epoch: u64, scope: ProfileScope) -> bool {
        self.state.login_origin.as_ref().is_some_and(|current|
            current.origin == origin && current.returned_epoch == Some(epoch)
                && self.state.epoch == epoch && self.state.profile_scope == scope
                && self.state.phase == current.phase && self.state.picker == current.picker
                && !self.state.signin_active && self.state.pending.is_empty() && self.state.inbox.is_empty()
                && current.unchanged(&self.state))
    }

    /// A first unsigned login may receive its installation client id through
    /// the existing typed read. This is not account/profile authorization.
    pub(super) fn origin_client_id_read(&mut self, client_id: &str) {
        let eligible = self.state.login_origin.as_ref().is_some_and(|current|
            current.cancellable(&self.state) && current.phase == Phase::Idle
                && current.identity.client_id.is_empty() && current.disk_identity.client_id.is_empty()
                && current.identity.account_token.is_empty() && current.identity.profile_uuid.is_empty()
                && current.disk_identity.account_token.is_empty() && current.disk_identity.profile_uuid.is_empty());
        if eligible {
            let current = self.state.login_origin.as_mut().expect("checked current origin");
            current.identity.client_id = client_id.into();
            current.disk_identity.client_id = client_id.into();
        } else { self.retire_login_origin(); }
    }
}
