//! Managed imports share the SessionMachine commit owner. This module contains
//! only captured values and transitions; the app adapter retains the actual proof.

use super::*;

#[derive(Clone, Serialize, Deserialize)]
pub struct ManagedActivation {
    pub operation: u64,
    pub expected_epoch: u64,
    pub expected_runtime: CredentialPatch,
    pub expected_disk: Identity,
    pub credentials: CredentialPatch,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct ManagedProvenance {
    pub operation: u64,
    pub req: u32,
    pub epoch: u64,
    pub arrival: u64,
    pub credentials: CredentialPatch,
    pub disk: Identity,
    /// The genuine credential fallback, never an earlier managed overlay.
    pub prior_credentials: CredentialPatch,
}

#[derive(Clone, Serialize, Deserialize)]
pub enum ManagedCommit {
    Activate(ManagedActivation),
    Resume { operation: u64 },
}

impl CredentialPatch {
    /// Compare the complete captured credential value, including opaque fields.
    /// Identity alone does not bind a PMS token, origin, source roster or profile.
    pub fn same_credentials(&self, other: &Self) -> bool {
        if self.user.account_scope != other.user.account_scope
            || !self.profiles.iter().map(|p| p.user.account_scope)
                .eq(other.profiles.iter().map(|p| p.user.account_scope)) { return false; }
        match (serde_json::to_value(self), serde_json::to_value(other)) {
            (Ok(a), Ok(b)) => a == b,
            _ => false,
        }
    }

    pub fn matches_session(&self, session: &PersistedSession) -> bool {
        self.same_credentials(&Self::of(session))
    }
}

fn write_patch(w: &mut Canon, patch: &CredentialPatch) {
    // Unlike the ordinary replay digest, this binding includes opaque fields.
    w.str(&serde_json::to_string(patch).expect("credential patch serialization"));
    w.bool(patch.user.account_scope == plx_plex::plex::session::AccountScope::PmsOnly);
    w.seq(patch.profiles.len());
    for profile in &patch.profiles {
        w.bool(profile.user.account_scope == plx_plex::plex::session::AccountScope::PmsOnly);
    }
}

fn write_identity(w: &mut Canon, identity: &Identity) {
    w.str(&identity.client_id).str(&identity.account_token).str(&identity.profile_uuid);
}

impl ManagedCommit {
    fn operation(&self) -> u64 {
        match self { Self::Activate(value) => value.operation, Self::Resume { operation } => *operation }
    }

    pub(super) fn write(&self, w: &mut Canon) {
        w.str("managed-commit-v1");
        match self {
            Self::Activate(value) => {
                w.u8(0).u64(value.operation).u64(value.expected_epoch);
                write_patch(w, &value.expected_runtime);
                write_identity(w, &value.expected_disk);
                write_patch(w, &value.credentials);
            }
            Self::Resume { operation } => { w.u8(1).u64(*operation); }
        }
    }
}

impl ManagedProvenance {
    pub(super) fn write(&self, w: &mut Canon) {
        w.str("managed-provenance-v1").u64(self.operation).u32(self.req)
            .u64(self.epoch).u64(self.arrival);
        write_patch(w, &self.credentials);
        write_identity(w, &self.disk);
        write_patch(w, &self.prior_credentials);
    }
}

impl SessionInit {
    pub fn managed_operation(&self) -> Option<u64> {
        self.managed.as_ref().filter(|managed|
            managed.disk == self.disk_identity
                && managed.credentials.matches_session(&self.persisted)
                && managed.credentials.same_credentials(&self.committed_credentials))
            .map(|managed| managed.operation)
    }
}

impl SessionMachine {
    pub(super) fn managed_credentials_current(&self) -> bool { self.state.managed_operation().is_some() }

    pub fn managed_operation(&self) -> Option<u64> { self.state.managed_operation() }

    /// The adapter compares its retained preparation with this exact pending
    /// command. A matching operation number alone cannot substitute credentials.
    pub fn pending_managed_activation(&self, req: u32, epoch: u64, arrival: u64)
        -> Option<&ManagedActivation> {
        if !self.commit_is_current(req, epoch, arrival) { return None; }
        match self.state.pending_commit.as_ref()?.delta.managed.as_ref()? {
            ManagedCommit::Activate(value) if self.managed_activation_current(value) => Some(value),
            _ => None,
        }
    }

    /// Publication ordering still has to be checked by the app resource adapter.
    /// This is owner-state evidence, never a page-mounted or playback receipt.
    pub fn managed_publication_is_current(&self, operation: u64, req: u32, epoch: u64, arrival: u64) -> bool {
        self.state.epoch == epoch && self.state.phase == Phase::Ready
            && !self.state.apply_pending && self.state.pending_erase.is_none()
            && self.managed_credentials_current()
            && self.state.managed.as_ref().is_some_and(|value|
                (value.operation, value.req, value.epoch, value.arrival) == (operation, req, epoch, arrival))
    }

    fn managed_activation_current(&self, value: &ManagedActivation) -> bool {
        value.operation != 0 && value.expected_epoch == self.state.epoch
            && value.expected_disk == self.state.disk_identity
            && value.expected_runtime.matches_session(&self.state.persisted)
            && value.expected_runtime.same_credentials(&self.state.committed_credentials)
    }

    pub fn managed_activation_admissible(&self, value: &ManagedActivation) -> bool {
        let patch = &value.credentials;
        // The private proof establishes trust. These shape checks additionally
        // prevent a managed command from becoming an account-token replacement.
        !(!self.managed_activation_current(value)
            || !matches!(self.state.authority, BootstrapAuthority::Account { .. })
            || self.state.pending_erase.is_some() || self.state.pending_commit.is_some()
            || !self.state.pending.is_empty() || self.state.signin_active
            || self.state.persistence_warning.is_some() || self.state.held_handoff.is_some()
            || self.state.admitted_persistence.is_some()
            || self.state.managed.as_ref().is_some_and(|m| m.operation == value.operation)
            || patch.client_id.is_empty() || patch.client_id != value.expected_runtime.client_id
            || !patch.account_token.is_empty() || patch.user.plex_tv_token.is_some()
            || patch.user.account_scope != plx_plex::plex::session::AccountScope::PmsOnly
            || !patch.home_users.is_empty() || !patch.profiles.is_empty()
            || patch.sources.len() != 1 || patch.server.machine_id.is_empty()
            || patch.server.token.is_empty() || patch.user.token != patch.server.token
            || patch.sources[0].machine_id != patch.server.machine_id
            || patch.sources[0].token != patch.server.token
            || patch.sources[0].origin().is_none()
            || patch.sources[0].origin() != plx_plex::plex::Origin::parse(&patch.server.origin_url)
            || !patch.merge_into(&PersistedSession::default()).can_go_local()
            || self.state.next_req.checked_add(1).is_none())
    }

    pub(super) fn activate_managed(&mut self, value: &ManagedActivation,
        emit: &mut impl FnMut(SessionFx)) -> bool {
        if !self.managed_activation_admissible(value) {
            emit(SessionFx::ManagedAdmissionRefused { operation: value.operation, expected_epoch: value.expected_epoch });
            return true;
        }
        let patch = &value.credentials;
        let Some(req) = self.allocate(SessionOp::Managed, None) else { return false };
        let plan = CommitPlan {
            registry_client_id: patch.client_id.clone(), expected_disk: value.expected_disk.clone(),
            credentials: None, lifecycle: None, registry: vec![RegistryPlan::Managed { operation: value.operation }],
            purpose: PersistencePurpose::Background, writes_durable: false,
            authority: plx_plex::plex::session::SaveAuthority::Routine,
        };
        self.begin_commit(req, 0, true, plan, CommitDelta {
            managed: Some(ManagedCommit::Activate(value.clone())), ..Default::default()
        }, emit)
    }

    pub(super) fn resume_managed(&mut self, emit: &mut impl FnMut(SessionFx)) -> bool {
        if !self.managed_credentials_current() || self.state.pending_commit.is_some()
            || self.state.pending_erase.is_some() { return false; }
        let operation = self.state.managed.as_ref().unwrap().operation;
        let Some(req) = self.allocate(SessionOp::Managed, None) else { return false };
        // Re-publication of the already admitted seat must not re-register a
        // borrowed account slot or acquire another persistence authorization.
        let plan = CommitPlan {
            registry_client_id: self.state.persisted.client_id.clone(), expected_disk: self.state.disk_identity.clone(),
            credentials: None, lifecycle: None, registry: Vec::new(),
            purpose: PersistencePurpose::Background, writes_durable: false,
            authority: plx_plex::plex::session::SaveAuthority::Routine,
        };
        self.begin_commit(req, 0, true, plan, CommitDelta {
            managed: Some(ManagedCommit::Resume { operation }), ..Default::default()
        }, emit)
    }

    pub(super) fn apply_managed_commit(&mut self, reply: CommitReply, commit: PendingCommit,
        emit: &mut impl FnMut(SessionFx)) -> bool {
        let managed = commit.delta.managed.expect("managed commit branch");
        let operation = managed.operation();
        let current = match &managed {
            ManagedCommit::Activate(value) => self.managed_activation_current(value),
            ManagedCommit::Resume { operation } => self.managed_operation() == Some(*operation),
        };
        self.state.persistence_purpose = None;
        if reply.admission == CommitAdmission::RegistryOnly && current {
            match managed {
                ManagedCommit::Activate(value) => {
                    let prior_credentials = self.state.managed.as_ref()
                        .map(|m| m.prior_credentials.clone()).unwrap_or_else(|| self.state.committed_credentials.clone());
                    self.state.persisted = value.credentials.merge_into(&self.state.persisted);
                    self.state.committed_credentials = value.credentials.clone();
                    self.state.managed = Some(ManagedProvenance {
                        operation, req: reply.req, epoch: reply.epoch, arrival: reply.arrival,
                        credentials: value.credentials, disk: self.state.disk_identity.clone(), prior_credentials,
                    });
                }
                ManagedCommit::Resume { .. } => {
                    let value = self.state.managed.as_mut().unwrap();
                    value.req = reply.req;
                    value.epoch = reply.epoch;
                    value.arrival = reply.arrival;
                }
            }
            self.state.phase = Phase::Ready;
            self.state.apply_pending = false;
            self.state.authorized_in_flow = false;
            self.state.users.clear();
            self.state.error.clear();
            self.state.pin_denied = false;
            self.state.commit_phase = CommitPhase { admitted: false, durable: false, proves_saved_login: false };
            self.publish_profile(Some(self.state.persisted.user.clone()), emit);
            emit(SessionFx::ManagedPublished { operation, req: reply.req, epoch: reply.epoch, arrival: reply.arrival });
        } else {
            emit(SessionFx::ManagedRefused { operation, req: reply.req, epoch: reply.epoch, arrival: reply.arrival });
        }
        self.retire(reply.req, emit);
        if let Some(receipt) = commit.receipt { emit(SessionFx::Acknowledge(vec![receipt])); }
        self.schedule_pump(emit);
        self.replace_publication();
        true
    }

    /// A new authentication invalidates managed grant authority. Restore the
    /// genuine fallback before ordinary auth/Back/TakeReady can write it.
    pub(super) fn leave_managed_for_auth(&mut self) {
        if let Some(value) = self.state.managed.take() {
            self.state.persisted = value.prior_credentials.merge_into(&self.state.persisted);
            self.state.committed_credentials = value.prior_credentials;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::owner::scenarios::step;

    fn fixture() -> (SessionMachine, ManagedActivation) {
        let prior = PersistedSession {
            client_id: "synthetic-client".into(), account_token: "prior-account".into(),
            user: UserRef { uuid: "prior-profile".into(), token: "prior-pms".into(), ..Default::default() },
            ..Default::default()
        };
        let mut init = SessionInit::captured(prior);
        init.phase = Phase::Ready;
        let source = plx_plex::plex::session::SourceRef {
            machine_id: "managed-machine".into(), name: "Managed server".into(),
            origin_url: "https://managed.invalid:32400".into(), address: "managed.invalid".into(),
            port: 32400, token: "managed-pms".into(),
            tier: Some(plx_plex::plex::probe::Location::Local), ..Default::default()
        };
        let managed = PersistedSession {
            client_id: init.persisted.client_id.clone(), server: crate::auth::server_ref(&source),
            user: UserRef { title: "Managed server".into(), token: source.token.clone(),
                account_scope: plx_plex::plex::session::AccountScope::PmsOnly, ..Default::default() },
            sources: vec![source], ..Default::default()
        };
        let activation = ManagedActivation {
            operation: 17, expected_epoch: init.epoch,
            expected_runtime: CredentialPatch::of(&init.persisted), expected_disk: init.disk_identity.clone(),
            credentials: CredentialPatch::of(&managed),
        };
        (SessionMachine::from_init(init), activation)
    }

    fn commit_of(effects: Vec<SessionFx>) -> (CommitReply, CommitPlan) {
        let mut commits = effects.into_iter().filter_map(|effect| match effect {
            SessionFx::Commit { req, epoch, arrival, plan } => Some((
                CommitReply { req, epoch, arrival, admission: CommitAdmission::RegistryOnly }, plan)),
            _ => None,
        });
        let commit = commits.next().expect("one real owner commit");
        assert!(commits.next().is_none());
        commit
    }

    fn activate(owner: &mut SessionMachine, activation: ManagedActivation) -> CommitReply {
        let (reply, _) = commit_of(step(owner, SessionEvent::Command(Command::ActivateManaged(activation))));
        step(owner, SessionEvent::Commit(reply));
        reply
    }

    #[test]
    fn managed_activation_publishes_only_after_exact_registry_ack_without_a_save() {
        let (mut owner, activation) = fixture();
        let prior = owner.snapshot_init();
        let (reply, plan) = commit_of(step(&mut owner,
            SessionEvent::Command(Command::ActivateManaged(activation.clone()))));
        assert!(plan.credentials.is_none() && !plan.writes_durable);
        assert!(matches!(plan.registry.as_slice(), [RegistryPlan::Managed { operation: 17 }]));
        assert!(activation.expected_runtime.matches_session(&owner.state.persisted));
        assert!(owner.state.managed.is_none());
        assert!(owner.pending_managed_activation(reply.req, reply.epoch, reply.arrival).is_some());

        let effects = step(&mut owner, SessionEvent::Commit(reply));
        let publication = effects.iter().position(|fx| matches!(fx, SessionFx::PublishProfile(_))).unwrap();
        let completion = effects.iter().position(|fx| matches!(fx,
            SessionFx::ManagedPublished { operation: 17, req, epoch, arrival }
                if (*req, *epoch, *arrival) == (reply.req, reply.epoch, reply.arrival))).unwrap();
        assert!(publication < completion);
        assert!(!effects.iter().any(|fx| matches!(fx, SessionFx::Ready { .. } | SessionFx::Commit { .. })));
        assert!(activation.credentials.matches_session(&owner.state.persisted));
        assert!(activation.credentials.same_credentials(&owner.state.committed_credentials));
        assert!(owner.state.disk_identity == prior.disk_identity);
        assert!(!owner.state.commit_phase.admitted && !owner.state.commit_phase.durable);
        assert!(!owner.state.commit_phase.proves_saved_login);
        assert!(owner.managed_publication_is_current(17, reply.req, reply.epoch, reply.arrival));
        assert!(step(&mut owner, SessionEvent::Commit(reply)).is_empty(), "duplicate reply is inert");
    }

    #[test]
    fn managed_activation_requires_complete_runtime_disk_and_epoch_match() {
        for changed in 0..7 {
            let (mut owner, mut activation) = fixture();
            match changed {
                0 => activation.expected_epoch += 1,
                1 => activation.expected_disk.account_token = "another-disk".into(),
                2 => activation.expected_runtime.server.token = "another-pms".into(),
                3 => activation.expected_runtime.server.origin_url = "https://other.invalid".into(),
                4 => activation.credentials.account_token = "not-a-managed-pms-token".into(),
                5 => activation.expected_runtime.user.account_scope = plx_plex::plex::session::AccountScope::PmsOnly,
                _ => activation.credentials.user.account_scope = plx_plex::plex::session::AccountScope::Account,
            }
            let before = owner.subhash();
            assert!(!owner.managed_activation_admissible(&activation));
            let effects = step(&mut owner, SessionEvent::Command(Command::ActivateManaged(activation)));
            assert!(matches!(effects.as_slice(), [SessionFx::ManagedAdmissionRefused { operation: 17, .. }]));
            assert_eq!(owner.subhash(), before, "no owner mutation for variant {changed}");
        }
    }

    #[test]
    fn managed_refusal_and_wrong_correlation_preserve_prior_runtime() {
        let (mut owner, activation) = fixture();
        let (reply, _) = commit_of(step(&mut owner,
            SessionEvent::Command(Command::ActivateManaged(activation.clone()))));
        for wrong in [CommitReply { req: reply.req + 1, ..reply },
            CommitReply { epoch: reply.epoch + 1, ..reply },
            CommitReply { arrival: reply.arrival + 1, ..reply }] {
            assert!(step(&mut owner, SessionEvent::Commit(wrong)).is_empty());
            assert!(owner.pending_managed_activation(reply.req, reply.epoch, reply.arrival).is_some());
        }
        let effects = step(&mut owner, SessionEvent::Commit(CommitReply {
            admission: CommitAdmission::Rejected { revision: None, rejection: RejectionKind::Uninitialized }, ..reply
        }));
        assert!(effects.iter().any(|fx| matches!(fx, SessionFx::ManagedRefused { operation: 17, .. })));
        assert!(!effects.iter().any(|fx| matches!(fx, SessionFx::PublishProfile(_) | SessionFx::ManagedPublished { .. })));
        assert!(activation.expected_runtime.matches_session(&owner.state.persisted));
        assert!(activation.expected_runtime.same_credentials(&owner.state.committed_credentials));
        assert!(owner.state.disk_identity == activation.expected_disk);
        assert!(owner.state.managed.is_none());
        assert!(owner.state.phase == Phase::Ready);
    }

    #[test]
    fn managed_ack_cannot_admit_a_write_or_overlook_a_changed_disk() {
        for changed_disk in [false, true] {
            let (mut owner, activation) = fixture();
            let (mut reply, _) = commit_of(step(&mut owner,
                SessionEvent::Command(Command::ActivateManaged(activation.clone()))));
            if changed_disk {
                owner.state.disk_identity.account_token = "changed-physical-record".into();
                assert!(owner.pending_managed_activation(reply.req, reply.epoch, reply.arrival).is_none(),
                    "resource execution must not see a stale activation permit");
            } else {
                reply.admission = CommitAdmission::Admitted { revision: 1, purpose: PersistencePurpose::Final };
            }
            let effects = step(&mut owner, SessionEvent::Commit(reply));
            assert!(effects.iter().any(|fx| matches!(fx, SessionFx::ManagedRefused { .. })));
            assert!(owner.state.managed.is_none());
            assert!(activation.expected_runtime.matches_session(&owner.state.persisted));
        }
    }

    #[test]
    fn managed_switch_back_and_take_ready_keep_the_nondurable_seat() {
        let (mut owner, activation) = fixture();
        let first = activate(&mut owner, activation.clone());
        let effects = step(&mut owner, SessionEvent::Command(Command::StartSwitch(Picker::ChangeProfile)));
        assert!(!effects.iter().any(|fx| matches!(fx,
            SessionFx::Work { .. } | SessionFx::Capture { .. } | SessionFx::Commit { .. })),
            "a PMS-only managed seat cannot request an account roster");
        assert!(owner.state.phase == Phase::Profiles);
        assert_eq!(owner.managed_operation(), Some(17));
        step(&mut owner, SessionEvent::Command(Command::BackAtRoot { reply: ReplyTo { instance: 1, correlation: 2 } }));
        assert!(owner.state.apply_pending);
        let (reply, plan) = commit_of(step(&mut owner, SessionEvent::Command(Command::TakeReady)));
        assert!(plan.credentials.is_none() && !plan.writes_durable && plan.registry.is_empty());
        let effects = step(&mut owner, SessionEvent::Commit(reply));
        assert!(!effects.iter().any(|fx| matches!(fx, SessionFx::Ready { .. })));
        assert!(owner.managed_publication_is_current(17, reply.req, reply.epoch, reply.arrival));
        assert!(!owner.managed_publication_is_current(17, first.req, first.epoch, first.arrival));
        assert!(activation.credentials.matches_session(&owner.state.persisted));
        assert!(activation.credentials.same_credentials(&owner.state.committed_credentials));
        assert!(owner.state.disk_identity == activation.expected_disk);
        assert!(owner.state.pending.is_empty(), "resume must retire its sole request");
    }

    #[test]
    fn managed_start_login_restores_genuine_fallback_and_preserves_preferences() {
        let (mut owner, activation) = fixture();
        activate(&mut owner, activation.clone());
        owner.state.persisted.language = plx_platform::i18n::Preference::Be;
        step(&mut owner, SessionEvent::Command(Command::StartLogin));
        assert!(owner.state.managed.is_none());
        assert!(activation.expected_runtime.matches_session(&owner.state.persisted));
        assert!(activation.expected_runtime.same_credentials(&owner.state.committed_credentials));
        assert!(owner.state.disk_identity == activation.expected_disk);
        assert!(owner.state.persisted.language == plx_platform::i18n::Preference::Be);
    }

    #[test]
    fn invalidated_managed_provenance_never_falls_through_to_an_ordinary_save() {
        let (mut owner, activation) = fixture();
        activate(&mut owner, activation);
        owner.state.disk_identity.account_token = "new-physical-record".into();
        owner.state.apply_pending = true;
        assert!(!owner.needs_ready_commit());
        let before = owner.state.hash();
        assert!(step(&mut owner, SessionEvent::Command(Command::TakeReady)).is_empty());
        assert!(step(&mut owner, SessionEvent::Command(Command::StartSwitch(Picker::ChangeProfile))).is_empty());
        assert!(step(&mut owner, SessionEvent::Command(Command::ResumeStored)).is_empty());
        assert!(owner.state.pending_commit.is_none());
        assert!(owner.state.pending.is_empty());
        assert_eq!(owner.state.hash(), before);
    }

    #[test]
    fn managed_resume_counter_exhaustion_leaves_no_partial_request() {
        let (mut owner, activation) = fixture();
        activate(&mut owner, activation);
        owner.state.apply_pending = true;
        owner.state.next_req = u32::MAX;
        let before = owner.state.hash();
        assert!(step(&mut owner, SessionEvent::Command(Command::TakeReady)).is_empty());
        assert!(owner.state.pending.is_empty());
        assert_eq!(owner.state.hash(), before);
    }

    #[test]
    fn managed_erase_and_stale_completion_never_republish_the_candidate() {
        let (mut owner, activation) = fixture();
        let (reply, _) = commit_of(step(&mut owner,
            SessionEvent::Command(Command::ActivateManaged(activation))));
        step(&mut owner, SessionEvent::Command(Command::EraseLocal));
        assert!(step(&mut owner, SessionEvent::Commit(reply)).is_empty());
        assert!(owner.state.managed.is_none());
        assert!(owner.state.persisted.server.token.is_empty());
    }

    #[test]
    fn managed_registry_operation_is_refused_without_the_app_proof_adapter() {
        assert!(!crate::auth::execute_session_registry(&RegistryPlan::Managed { operation: 17 }, "synthetic-client"));
    }
}
