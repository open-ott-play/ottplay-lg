//! Typed owner-only regressions. No workers, storage or registry are executed.
use super::*;
use super::scenarios::step;
use plx_machine::machine::RequestId;

fn reply() -> ReplyTo { ReplyTo { instance: 7, correlation: 11 } }
fn owner() -> SessionMachine {
    SessionMachine::from_init(SessionInit::captured(PersistedSession::default()))
}
fn ready_owner() -> SessionMachine {
    let mut persisted = PersistedSession { client_id: "client".into(),
        account_token: "same-account".into(), ..Default::default() };
    persisted.server.address = "127.0.0.1".into();
    persisted.server.port = 32400;
    persisted.server.token = "pms".into();
    persisted.user.uuid = "user".into();
    persisted.user.token = "pms".into();
    let mut initial = SessionInit::captured(persisted);
    initial.phase = Phase::Ready;
    initial.active_profile = Some(initial.persisted.user.clone());
    SessionMachine::from_init(initial)
}
fn begin(owner: &mut SessionMachine, id: u64) -> LoginOrigin {
    let fx = step(owner, SessionEvent::Command(Command::StartLoginFrom { origin: id, reply: reply() }));
    let origin = fx.iter().find_map(|fx| match fx {
        SessionFx::LoginOriginStarted { origin, accepted: true, returnable: true, .. } => Some(*origin),
        _ => None,
    }).expect("stable origin accepted");
    assert_eq!(owner.read().0.origin_cancel, Some(origin));
    origin
}
fn cancel(owner: &mut SessionMachine, origin: LoginOrigin) -> Vec<SessionFx> {
    step(owner, SessionEvent::Command(Command::CancelLoginFrom { origin, reply: reply() }))
}
fn returned(fx: &[SessionFx]) -> (u64, ProfileScope) {
    fx.iter().find_map(|fx| match fx {
        SessionFx::LoginOriginCancelled { disposition: LoginCancelDisposition::Returned { epoch, scope }, .. } => Some((*epoch, *scope)),
        _ => None,
    }).expect("explicit returned disposition")
}
fn no_handoff(fx: &[SessionFx]) {
    assert!(!fx.iter().any(|fx| matches!(fx, SessionFx::Ready { .. } | SessionFx::Commit { .. }
        | SessionFx::PublishProfile(_) | SessionFx::BackReply { .. })));
}
fn warning(epoch: u64) -> PersistenceWarning {
    PersistenceWarning { key: PersistenceWarningKey { epoch, req: 1 }, site: PersistenceWarningSite::Final,
        helper: None, candidate_errnos: [None; 8], persistence: None }
}

#[test]
fn unsigned_cancel_initializes_client_id_but_never_fabricates_ready() {
    let mut owner = owner();
    let origin = begin(&mut owner, 1);
    let req = owner.state.next_req;
    step(&mut owner, SessionEvent::Read(SessionReadReply {
        addr: Addr { to: MachineId::Session, req: RequestId(req) }, epoch: origin.flow_epoch,
        value: SessionReadValue::LoginClientId("synthetic-client".into()),
    }));
    assert_eq!(owner.read().0.origin_cancel, Some(origin));
    let fx = cancel(&mut owner, origin);
    let (epoch, scope) = returned(&fx);
    no_handoff(&fx);
    assert_eq!(owner.state.phase, Phase::Idle);
    assert!(!owner.state.apply_pending);
    assert!(owner.state.pending.is_empty());
    assert_eq!(owner.state.persisted.client_id, "synthetic-client");
    assert!(owner.state.persisted.account_token.is_empty());
    assert!(owner.origin_return_is_current(origin, epoch, scope));
    let duplicate = cancel(&mut owner, origin);
    assert!(duplicate.iter().any(|fx| matches!(fx, SessionFx::LoginOriginCancelled {
        disposition: LoginCancelDisposition::Stale, .. })));
    no_handoff(&duplicate);
    assert!(owner.origin_return_is_current(origin, epoch, scope));
}

#[test]
fn settled_same_account_cancel_returns_without_reinstalling_registry() {
    let mut owner = ready_owner();
    let origin = begin(&mut owner, 2);
    let fx = cancel(&mut owner, origin);
    let (epoch, scope) = returned(&fx);
    no_handoff(&fx);
    assert_eq!(owner.state.phase, Phase::Ready);
    assert!(!owner.state.apply_pending);
    assert!(owner.origin_return_is_current(origin, epoch, scope));
    assert!(!owner.origin_return_is_current(origin, epoch + 1, scope));
}

#[test]
fn warning_or_pending_erase_refuses_addressed_start_without_mutation() {
    for kind in 0..3 {
        let mut owner = owner();
        match kind {
            0 => owner.state.persistence_warning = Some(warning(owner.state.epoch)),
            1 => owner.state.pending_erase = Some((owner.state.epoch, false)),
            _ => owner.state.unconfirmed_fresh_prior = Some(owner.state.disk_identity.clone()),
        }
        let before = owner.snapshot_init().hash();
        let fx = step(&mut owner, SessionEvent::Command(Command::StartLoginFrom { origin: 3, reply: reply() }));
        assert_eq!(owner.snapshot_init().hash(), before);
        assert!(fx.iter().any(|fx| matches!(fx, SessionFx::LoginOriginStarted { accepted: false, returnable: false, .. })));
        no_handoff(&fx);
        assert!(!fx.iter().any(|fx| matches!(fx, SessionFx::Work { .. } | SessionFx::Capture { .. } | SessionFx::Cancel { .. })));
    }
}

#[test]
fn auth03_and_admitted_write_block_cancel_without_clearing_their_evidence() {
    for kind in 0..2 {
        let mut owner = owner();
        let origin = begin(&mut owner, 4);
        if kind == 0 { owner.state.persistence_warning = Some(warning(origin.flow_epoch)); }
        else { owner.state.admitted_persistence = Some(AdmittedPersistence {
            req: 1, epoch: origin.flow_epoch, arrival: 1, revision: 1,
            purpose: Some(PersistencePurpose::Final), fresh: true, site: PersistenceWarningSite::Final,
        }); }
        let warning = owner.state.persistence_warning;
        let admitted = owner.state.admitted_persistence;
        let fx = cancel(&mut owner, origin);
        assert!(fx.iter().any(|fx| matches!(fx, SessionFx::LoginOriginCancelled {
            disposition: LoginCancelDisposition::Blocked, .. })));
        assert_eq!(owner.state.persistence_warning, warning);
        assert_eq!(owner.state.admitted_persistence, admitted);
        no_handoff(&fx);
        assert!(!fx.iter().any(|fx| matches!(fx, SessionFx::Cancel { .. })));
    }
}

#[test]
fn same_identity_authorization_consumes_the_origin() {
    let mut owner = ready_owner();
    let origin = begin(&mut owner, 5);
    let req = owner.state.next_req;
    let envelope = SessionEnvelope { addr: Addr { to: MachineId::Session, req: RequestId(req) },
        key: owner.state.pending[&req].key, admission: AdmissionId(req), arrival: 1,
        terminal: false, lifecycle: None,
        outcome: SessionArrival::Data(Arc::new(super::super::observation::Observation::Login(
            super::super::LoginProgress::Authorized { epoch: origin.flow_epoch, token: "same-account".into() }))),
    };
    assert!(owner.apply_qr_observation(&envelope, &mut |_| {}));
    assert!(owner.read().0.origin_cancel.is_none());
    let fx = cancel(&mut owner, origin);
    assert!(fx.iter().any(|fx| matches!(fx, SessionFx::LoginOriginCancelled {
        disposition: LoginCancelDisposition::Stale, .. })));
    no_handoff(&fx);
}

#[test]
fn retry_and_new_flow_invalidate_old_address_and_return_receipt() {
    let mut owner = owner();
    let first = begin(&mut owner, 6);
    step(&mut owner, SessionEvent::Command(Command::Retry));
    assert!(owner.read().0.origin_cancel.is_none());
    assert!(cancel(&mut owner, first).iter().any(|fx| matches!(fx,
        SessionFx::LoginOriginCancelled { disposition: LoginCancelDisposition::Stale, .. })));
    // A new fresh stable process state models the next independently invoked flow.
    let mut owner = SessionMachine::from_init(SessionInit::captured(PersistedSession::default()));
    let second = begin(&mut owner, 7);
    let (epoch, scope) = returned(&cancel(&mut owner, second));
    begin(&mut owner, 8);
    assert!(!owner.origin_return_is_current(second, epoch, scope));
}

#[test]
fn dev_account_start_keeps_legacy_boundary_and_no_return_origin() {
    let mut initial = SessionInit::captured_boot(PersistedSession::default(),
        Some(plx_plex::plex::session::ServerRef { token: "dev".into(), ..Default::default() }), Vec::new());
    initial.phase = Phase::Ready;
    let mut owner = SessionMachine::from_init(initial);
    let fx = step(&mut owner, SessionEvent::Command(Command::StartLoginFrom { origin: 9, reply: reply() }));
    assert!(fx.iter().any(|fx| matches!(fx, SessionFx::LoginOriginStarted {
        accepted: true, returnable: false, .. })));
    assert!(fx.iter().any(|fx| matches!(fx, SessionFx::Commit { .. })));
    assert!(owner.read().0.origin_cancel.is_none());
}

#[test]
fn origin_and_returned_marker_round_trip_in_logical_state_and_hash() {
    let mut owner = owner();
    let origin = begin(&mut owner, 10);
    for returned_state in [false, true] {
        if returned_state { cancel(&mut owner, origin); }
        let state = owner.snapshot_init();
        let encoded = serde_json::to_vec(&state).unwrap();
        let decoded: SessionInit = serde_json::from_slice(&encoded).unwrap();
        assert_eq!(state.hash(), decoded.hash());
        let restored = SessionMachine::from_init(decoded);
        assert_eq!(owner.read().0.origin_cancel, restored.read().0.origin_cancel);
        if returned_state {
            assert!(restored.origin_return_is_current(origin, state.epoch, state.profile_scope));
        }
        let mut stripped = state.clone(); stripped.login_origin = None;
        assert_ne!(state.hash(), stripped.hash());
    }
}

#[test]
fn protected_picker_and_nonstable_entry_keep_legacy_login_without_origin() {
    let mut owner = ready_owner();
    owner.state.phase = Phase::Profiles;
    owner.state.picker = Picker::ChangeProfile;
    let fx = step(&mut owner, SessionEvent::Command(Command::StartLoginFrom { origin: 11, reply: reply() }));
    assert!(fx.iter().any(|fx| matches!(fx, SessionFx::LoginOriginStarted {
        accepted: true, returnable: false, .. })));
    assert!(owner.read().0.origin_cancel.is_none());
}

#[test]
fn changed_identity_or_scope_cannot_return_and_does_not_restore_a_snapshot() {
    for identity in [true, false] {
        let mut owner = ready_owner();
        let origin = begin(&mut owner, 12);
        if identity { owner.state.persisted.account_token = "different-account".into(); }
        else { owner.state.profile_scope.0 += 1; }
        let expected = serde_json::to_value(&owner.state.persisted).unwrap();
        let fx = cancel(&mut owner, origin);
        assert!(fx.iter().any(|fx| matches!(fx, SessionFx::LoginOriginCancelled {
            disposition: LoginCancelDisposition::Blocked, .. })));
        no_handoff(&fx);
        assert_eq!(serde_json::to_value(&owner.state.persisted).unwrap(), expected);
    }
}

#[test]
fn registry_commit_admission_retires_active_origin_before_it_can_return() {
    let mut owner = ready_owner();
    let origin = begin(&mut owner, 13);
    let req = owner.state.next_req;
    let plan = CommitPlan { registry_client_id: owner.state.persisted.client_id.clone(),
        expected_disk: owner.state.disk_identity.clone(), credentials: None, lifecycle: None,
        registry: vec![RegistryPlan::Revoke], purpose: PersistencePurpose::Background,
        writes_durable: false, authority: plx_plex::plex::session::SaveAuthority::Routine };
    assert!(owner.begin_commit(req, 1, false, plan, CommitDelta::default(), &mut |_| {}));
    assert!(owner.read().0.origin_cancel.is_none());
    let hash = owner.state.hash();
    let fx = cancel(&mut owner, origin);
    assert!(fx.iter().any(|fx| matches!(fx, SessionFx::LoginOriginCancelled {
        disposition: LoginCancelDisposition::Stale, .. })));
    assert_eq!(owner.state.hash(), hash);
    no_handoff(&fx);
    let fx = step(&mut owner, SessionEvent::Command(Command::StartLoginFrom { origin: 14, reply: reply() }));
    assert!(fx.iter().any(|fx| matches!(fx, SessionFx::LoginOriginStarted { accepted: false, .. })));
    assert_eq!(owner.state.hash(), hash);
}

#[test]
fn signout_erase_and_originless_start_retire_old_cancel_addresses() {
    for command in [Command::SignOut, Command::EraseLocal, Command::StartLogin] {
        let mut owner = ready_owner();
        let origin = begin(&mut owner, 15);
        step(&mut owner, SessionEvent::Command(command));
        let expected = owner.state.hash();
        let fx = cancel(&mut owner, origin);
        assert!(fx.iter().any(|fx| matches!(fx, SessionFx::LoginOriginCancelled {
            disposition: LoginCancelDisposition::Stale, .. })));
        assert_eq!(owner.state.hash(), expected);
        no_handoff(&fx);
    }
}

#[test]
fn a_client_id_read_cannot_promote_a_changed_account_into_return_evidence() {
    let mut owner = owner();
    let origin = begin(&mut owner, 16);
    let req = owner.state.next_req;
    owner.state.persisted.account_token = "changed".into();
    let before = owner.state.hash();
    // Existing pending.expected already rejects this read before the origin hook.
    assert!(step(&mut owner, SessionEvent::Read(SessionReadReply {
        addr: Addr { to: MachineId::Session, req: RequestId(req) }, epoch: origin.flow_epoch,
        value: SessionReadValue::LoginClientId("unexpected-client".into()),
    })).is_empty());
    assert_eq!(owner.state.hash(), before);
    let fx = cancel(&mut owner, origin);
    assert!(fx.iter().any(|fx| matches!(fx, SessionFx::LoginOriginCancelled {
        disposition: LoginCancelDisposition::Blocked, .. })));
    assert_eq!(owner.state.persisted.account_token, "changed");
    assert!(owner.state.persisted.client_id.is_empty());
    no_handoff(&fx);
}

#[test]
fn seated_ready_after_profile_switch_is_not_a_still_open_protected_picker() {
    let mut owner = ready_owner();
    owner.state.picker = Picker::ChangeProfile;
    let origin = begin(&mut owner, 17);
    let fx = cancel(&mut owner, origin);
    let (epoch, scope) = returned(&fx);
    no_handoff(&fx);
    assert_eq!(owner.state.phase, Phase::Ready);
    assert!(owner.state.picker == Picker::ChangeProfile);
    assert!(owner.origin_return_is_current(origin, epoch, scope));
}


fn assert_only_origin_removed(owner: &SessionMachine, mut before: SessionInit) {
    before.login_origin = None;
    // Avoid assert_eq! on serialized state: failure output must not dump credentials.
    assert!(serde_json::to_vec(&owner.snapshot_init()).unwrap()
        == serde_json::to_vec(&before).unwrap(), "boundary changed state outside login origin");
    assert_eq!(owner.subhash(), before.hash());
    assert!(owner.read().0.origin_cancel.is_none());
}

#[test]
fn network_boundary_synchronously_retires_active_origin_before_queued_cancel() {
    let mut owner = ready_owner();
    let origin = begin(&mut owner, 21);
    let before = owner.snapshot_init();
    let old_hash = owner.subhash();
    let publication = owner.publication();
    owner.take_logical_dirty();
    assert!(owner.observe_login_origin_boundary(LoginOriginBoundary::NetworkChanged));
    assert_only_origin_removed(&owner, before);
    assert_ne!(owner.subhash(), old_hash);
    assert!(owner.take_logical_dirty());
    assert!(!Arc::ptr_eq(&publication, &owner.publication()));
    let effects = cancel(&mut owner, origin);
    assert!(matches!(effects.as_slice(), [SessionFx::LoginOriginCancelled {
        disposition: LoginCancelDisposition::Stale, ..
    }]));
    no_handoff(&effects);
}

#[test]
fn network_boundary_synchronously_retires_returned_receipt_and_replays_canonically() {
    let mut owner = ready_owner();
    let origin = begin(&mut owner, 22);
    let (epoch, scope) = returned(&cancel(&mut owner, origin));
    assert!(owner.origin_return_is_current(origin, epoch, scope));
    let before = owner.snapshot_init();
    let serialized = serde_json::to_vec(&before).unwrap();
    let mut replay = SessionMachine::from_init(serde_json::from_slice(&serialized).unwrap());
    owner.take_logical_dirty();
    assert!(owner.observe_login_origin_boundary(LoginOriginBoundary::NetworkChanged));
    assert!(replay.observe_login_origin_boundary(LoginOriginBoundary::NetworkChanged));
    assert!(!owner.origin_return_is_current(origin, epoch, scope));
    assert!(!replay.origin_return_is_current(origin, epoch, scope));
    assert_only_origin_removed(&owner, before);
    assert_eq!(owner.subhash(), replay.subhash());
    assert!(owner.take_logical_dirty());
    let restored = SessionMachine::from_init(
        serde_json::from_slice(&serde_json::to_vec(&owner.snapshot_init()).unwrap()).unwrap());
    assert_eq!(restored.subhash(), owner.subhash());
    assert!(!restored.origin_return_is_current(origin, epoch, scope));
}

#[test]
fn network_boundary_without_origin_is_idempotent_and_does_not_dirty_state() {
    let mut owner = owner();
    let before = owner.snapshot_init();
    let publication = owner.publication();
    owner.take_logical_dirty();
    for _ in 0..2 {
        assert!(!owner.observe_login_origin_boundary(LoginOriginBoundary::NetworkChanged));
        assert_only_origin_removed(&owner, before.clone());
        assert!(Arc::ptr_eq(&publication, &owner.publication()));
        assert!(!owner.take_logical_dirty());
    }
    begin(&mut owner, 23);
    assert!(owner.observe_login_origin_boundary(LoginOriginBoundary::NetworkChanged));
    owner.take_logical_dirty();
    assert!(!owner.observe_login_origin_boundary(LoginOriginBoundary::NetworkChanged));
    assert!(!owner.take_logical_dirty());
}

#[test]
fn network_boundary_preserves_warning_and_admitted_persistence_evidence() {
    let mut owner = owner();
    let origin = begin(&mut owner, 24);
    let mut state = owner.snapshot_init();
    state.persistence_warning = Some(warning(origin.flow_epoch));
    state.admitted_persistence = Some(AdmittedPersistence {
        req: 1, epoch: origin.flow_epoch, arrival: 1, revision: 1,
        purpose: Some(PersistencePurpose::Final), fresh: true, site: PersistenceWarningSite::Final,
    });
    let mut owner = SessionMachine::from_init(state.clone());
    assert!(owner.observe_login_origin_boundary(LoginOriginBoundary::NetworkChanged));
    assert_only_origin_removed(&owner, state);
    let effects = cancel(&mut owner, origin);
    assert!(matches!(effects.as_slice(), [SessionFx::LoginOriginCancelled {
        disposition: LoginCancelDisposition::Stale, ..
    }]));
    no_handoff(&effects);
}
