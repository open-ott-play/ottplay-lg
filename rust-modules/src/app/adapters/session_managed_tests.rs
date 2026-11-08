//! Authored resource regressions. They use real registry/proof primitives, no network or disk writer.
use super::*;
use plx_session::auth::owner::{self, AdmissionState, CommitDelta, Identity, ManagedCommit, Pending,
    PendingCommit, SessionInit, SessionMachine, SessionOp, SessionWorkKey, StreamPhase};

#[test]
fn active_resource_outlives_receipt_deadlines_but_not_proof_or_lease() {
    let _serial = plx_base::testlock::serial(); let _registry = RegistryGuard::new();
    let (mut adapter, activation, source, proof, cancel) = fixture();
    prepare(&mut adapter, activation.clone(), source.clone(), proof, 60_000, 120_000, cancel).unwrap();
    assert!(admit(&mut adapter, &activation).admission == CommitAdmission::RegistryOnly);
    publish(&mut adapter, &activation);
    assert!(adapter.take_managed_completion().unwrap().accepted);
    adapter.managed.observation = Some(adapter.managed.observation.unwrap()
        .advance_for_test(std::time::Duration::from_millis(120_001)));
    assert!(adapter.managed_active_current(7, &activation.credentials, &activation.expected_disk));
    assert!(adapter.resume_managed(7), "old receipt deadlines cannot expire an active Session");
    assert!(adapter.take_managed_completion().is_none(), "resume is not a second activation result");
    plex::grant::network_changed();
    assert!(!adapter.managed_active_current(7, &activation.credentials, &activation.expected_disk));
    assert!(!adapter.resume_managed(7));
    let refreshed = plex::grant::prove_import(plex::grant::scope(), &source.origin().unwrap(),
        &source.machine_id, &source.machine_id, true).unwrap();
    assert!(adapter.refresh_managed(7, refreshed));
    assert!(adapter.managed_active_current(7, &activation.credentials, &activation.expected_disk));
    adapter.cancel_managed(7);
    assert!(!adapter.managed_active_current(7, &activation.credentials, &activation.expected_disk));
    assert!(!adapter.resume_managed(7), "retired lease cannot be revived");
}

struct RegistryGuard;
impl RegistryGuard {
    fn new() -> Self { plex::reset_servers_for_test(); plex::grant::reset_for_test(); Self }
}
impl Drop for RegistryGuard {
    fn drop(&mut self) { plex::reset_servers_for_test(); plex::grant::reset_for_test(); }
}
fn fixture() -> (SessionAdapter, ManagedActivation, SourceRef, ImportProof, Arc<AtomicBool>) {
    let disk = plex::session::Session { client_id: "install".into(), account_token: "genuine-account".into(), ..Default::default() };
    let origin = plex::Origin::http("127.0.0.1", 32400);
    let source = SourceRef { machine_id: "machine-one".into(), name: "Managed label".into(),
        address: "127.0.0.1".into(), port: 32400, origin_url: origin.base(), token: "pms-token".into(),
        tier: Some(plex::probe::Location::Local), ..Default::default() };
    let proof = plex::grant::prove_import(plex::grant::scope(), &origin, &source.machine_id, &source.machine_id, true).unwrap();
    let desired = plex::session::Session { client_id: disk.client_id.clone(),
        server: plex::session::ServerRef { machine_id: source.machine_id.clone(), name: source.name.clone(),
            address: source.address.clone(), port: source.port, origin_url: source.origin_url.clone(),
            token: source.token.clone(), tier: source.tier, ..Default::default() },
        user: plex::session::UserRef { account_scope: plex::session::AccountScope::PmsOnly, title: source.name.clone(), token: source.token.clone(), ..Default::default() },
        sources: vec![source.clone()], ..Default::default() };
    let activation = ManagedActivation { operation: 7, expected_epoch: 3,
        expected_runtime: CredentialPatch::of(&disk), expected_disk: Identity::of(&disk), credentials: CredentialPatch::of(&desired) };
    (SessionAdapter::fixture_with(disk), activation, source, proof, Arc::new(AtomicBool::new(false)))
}
// The injected sample is observation only: production still stores the exact
// deadline constructed by its caller, and every assertion uses the real adapter.
fn clock_sample() -> (plx_base::task::MainThread, plx_base::task::time::ClockSample) {
    let main = unsafe { plx_base::task::MainThread::assume() };
    let sample = plx_base::task::time::ClockSample::capture_main(&main).unwrap();
    (main, sample)
}
fn prepare(adapter: &mut SessionAdapter, activation: ManagedActivation, source: SourceRef,
    proof: ImportProof, command_ms: u64, context_ms: u64, cancel: Arc<AtomicBool>) -> Result<(), ManagedError> {
    let (main, sample) = clock_sample();
    adapter.managed.observation = Some(sample);
    adapter.prepare_managed(activation, source, proof,
        sample.deadline_at_unix_ms(sample.unix_ms() + command_ms).on_main(&main),
        sample.deadline_at_unix_ms(sample.unix_ms() + context_ms).on_main(&main), cancel)
}
fn owner(activation: &ManagedActivation) -> SessionMachine {
    let disk = activation.expected_runtime.merge_into(&plex::session::Session::default());
    let mut init = SessionInit::captured(disk);
    init.epoch = activation.expected_epoch;
    init.pending.insert(1, Pending { key: SessionWorkKey { epoch: init.epoch, op: SessionOp::Managed },
        expected: activation.expected_disk.clone(), lifecycle: None, last_arrival: Some(0),
        phase: StreamPhase::Running, capture: None, admission: AdmissionState::NotRequested });
    init.pending_commit = Some(PendingCommit { req: 1, epoch: init.epoch, arrival: 0, terminal: true,
        writes_credentials: false, receipt: None,
        delta: CommitDelta { managed: Some(ManagedCommit::Activate(activation.clone())), ..Default::default() },
        admitted_revision: None, purpose: None, fresh: false });
    SessionMachine::from_init(init)
}
fn plan(activation: &ManagedActivation) -> CommitPlan {
    CommitPlan { registry_client_id: activation.credentials.client_id.clone(), expected_disk: activation.expected_disk.clone(),
        credentials: None, lifecycle: None, registry: vec![RegistryPlan::Managed { operation: activation.operation }],
        purpose: plex::session::async_persistence::PersistencePurpose::Background, writes_durable: false,
        authority: plex::session::SaveAuthority::Routine }
}
fn admit(adapter: &mut SessionAdapter, activation: &ManagedActivation) -> CommitReply {
    let owner = owner(activation);
    adapter.begin_managed_commit(owner.commit_permit(1, activation.expected_epoch, 0).unwrap(), &plan(activation), Some(activation))
}
fn publish(adapter: &mut SessionAdapter, activation: &ManagedActivation) {
    let scope = ProfileScope(4);
    adapter.publish_managed_profile(owner::ProfilePublication { epoch: activation.expected_epoch,
        profile: Some(activation.credentials.user.clone()), scope,
        stamp: plex::grant::account_key(&activation.credentials.account_token) });
    adapter.managed_published(activation.operation, 1, activation.expected_epoch, 0,
        &activation.credentials, &activation.expected_disk, scope);
}

#[test]
fn preparation_retains_real_proof_without_registry_or_disk_mutation() {
    let _serial = plx_base::testlock::serial(); let _registry = RegistryGuard::new();
    let (mut adapter, activation, source, proof, cancel) = fixture();
    let before = serde_json::to_value(&adapter.fixture_resources().disk).unwrap();
    prepare(&mut adapter, activation.clone(), source.clone(), proof.clone(), 60000, 120000, cancel.clone()).unwrap();
    assert!(plex::id_of_machine(&source.machine_id).is_none());
    assert_eq!(prepare(&mut adapter, activation.clone(), source, proof, 60000, 120000, cancel), Err(ManagedError::Busy));
    assert_eq!(adapter.cancel_managed(activation.operation), Some(ManagedStage::Prepared));
    assert_eq!(serde_json::to_value(&adapter.fixture_resources().disk).unwrap(), before);
}

#[test]
fn owned_registry_admission_needs_actual_profile_publication_and_settles_once() {
    let _serial = plx_base::testlock::serial(); let _registry = RegistryGuard::new();
    let (mut adapter, activation, source, proof, cancel) = fixture();
    let before = serde_json::to_value(&adapter.fixture_resources().disk).unwrap();
    prepare(&mut adapter, activation.clone(), source.clone(), proof, 60000, 120000, cancel).unwrap();
    assert!(admit(&mut adapter, &activation).admission == CommitAdmission::RegistryOnly);
    assert_eq!(adapter.managed_stage(7), Some(ManagedStage::RegistryApplied));
    adapter.managed_published(7, 1, 3, 0, &activation.credentials, &activation.expected_disk, ProfileScope(4));
    assert!(adapter.take_managed_completion().is_none(), "owner signal alone is not publication");
    publish(&mut adapter, &activation);
    assert!(adapter.take_managed_completion().unwrap().accepted);
    publish(&mut adapter, &activation);
    assert!(adapter.take_managed_completion().is_none());
    assert_eq!(serde_json::to_value(&adapter.fixture_resources().disk).unwrap(), before);
    let id = plex::id_of_machine(&source.machine_id).unwrap();
    assert!(plex::client_for(id).unwrap().uses_token(&source.token));
    assert_eq!(adapter.cancel_managed(7), Some(ManagedStage::Published));
    assert!(!plex::client_for(id).unwrap().uses_token(&source.token));
    assert_eq!(adapter.managed_stage(7), Some(ManagedStage::Retired));
}

#[test]
fn a_borrowed_compatible_slot_keeps_its_external_ownership_on_cancellation() {
    let _serial = plx_base::testlock::serial(); let _registry = RegistryGuard::new();
    let (mut adapter, activation, source, proof, cancel) = fixture();
    let external = plex::register_imported(&proof, &source.token, "install", plex::ConnectionFacts::default(), None).unwrap();
    prepare(&mut adapter, activation.clone(), source.clone(), proof, 60000, 120000, cancel).unwrap();
    assert!(admit(&mut adapter, &activation).admission == CommitAdmission::RegistryOnly);
    adapter.cancel_managed(7);
    assert!(plex::imported_lease_matches(&external, &source.machine_id, &source.origin().unwrap(), &source.token));
    assert!(plex::client_for(external.server_id()).unwrap().uses_token(&source.token));
    assert!(plex::retire_imported(&external));
}

#[test]
fn changed_authorization_cancellation_deadline_and_disk_refuse_before_effects() {
    let _serial = plx_base::testlock::serial(); let _registry = RegistryGuard::new();
    for case in ["cancel", "deadline", "context", "disk", "disk_pms", "command"] {
        let (mut adapter, mut activation, source, proof, cancel) = fixture();
        prepare(&mut adapter, activation.clone(), source.clone(), proof, 60000, 120000, cancel.clone()).unwrap();
        match case {
            "cancel" => cancel.store(true, Ordering::Release),
            "deadline" => adapter.managed.observation = adapter.managed.observation
                .map(|sample| sample.advance_for_test(Duration::from_secs(60))),
            "context" => {
                let (main, _) = clock_sample();
                let sample = adapter.managed.observation.unwrap();
                adapter.managed.record.as_mut().unwrap().context_deadline = sample
                    .deadline_at_unix_ms(sample.unix_ms() + 1).on_main(&main);
                adapter.managed.observation = Some(sample.advance_for_test(Duration::from_millis(1)));
            }
            "disk" => adapter.fixture_resources().disk.account_token = "replaced-account".into(),
            "disk_pms" => adapter.fixture_resources().disk.server.token = "same-identity-new-pms-token".into(),
            "command" => activation.credentials.user.token = "different-command-token".into(),
            _ => unreachable!(),
        }
        assert!(admit(&mut adapter, &activation).admission == CommitAdmission::StaleAuthority, "{case}");
        assert!(plex::id_of_machine(&source.machine_id).is_none(), "{case}");
        assert_eq!(adapter.managed_stage(7), Some(ManagedStage::Prepared));
    }
}

#[test]
fn conflicting_existing_origin_or_token_is_not_replaced() {
    let _serial = plx_base::testlock::serial(); let _registry = RegistryGuard::new();
    let (mut adapter, activation, source, proof, cancel) = fixture();
    let external = plex::register_imported(&proof, "other-token", "install", plex::ConnectionFacts::default(), None).unwrap();
    prepare(&mut adapter, activation.clone(), source.clone(), proof, 60000, 120000, cancel).unwrap();
    assert!(admit(&mut adapter, &activation).admission == CommitAdmission::StaleAuthority);
    assert!(plex::client_for(external.server_id()).unwrap().uses_token("other-token"));
    adapter.managed_refused(7, 1, 3, 0);
    assert_eq!(adapter.take_managed_completion().unwrap().error, Some(ManagedError::AuthorizationConflict));
    assert_eq!(adapter.managed_stage(7), Some(ManagedStage::Refused));
    assert!(plex::retire_imported(&external));
}

#[test]
fn active_network_refresh_reuses_owned_lease_and_cannot_revive_retired_owner() {
    let _serial = plx_base::testlock::serial(); let _registry = RegistryGuard::new();
    let (mut adapter, activation, source, proof, cancel) = fixture();
    prepare(&mut adapter, activation.clone(), source.clone(), proof, 60000, 120000, cancel).unwrap();
    assert!(admit(&mut adapter, &activation).admission == CommitAdmission::RegistryOnly);
    publish(&mut adapter, &activation);
    let id = plex::id_of_machine(&source.machine_id).unwrap();
    plex::grant::network_changed();
    let fresh = plex::grant::prove_import(plex::grant::scope(), &source.origin().unwrap(), &source.machine_id, &source.machine_id, true).unwrap();
    assert!(adapter.refresh_managed(7, fresh.clone()));
    assert_eq!(plex::id_of_machine(&source.machine_id), Some(id));
    assert!(plex::client_for(id).unwrap().uses_token(&source.token));
    adapter.cancel_managed(7);
    assert!(!adapter.refresh_managed(7, fresh));
}

#[test]
fn context_expiry_after_effects_does_not_extend_or_replace_original_deadline() {
    let _serial = plx_base::testlock::serial(); let _registry = RegistryGuard::new();
    for original_expired in [false, true] {
        let (mut adapter, activation, source, proof, cancel) = fixture();
        prepare(&mut adapter, activation.clone(), source, proof, 60000, 120000, cancel).unwrap();
        assert!(admit(&mut adapter, &activation).admission == CommitAdmission::RegistryOnly);
        let (main, _) = clock_sample();
        let sample = adapter.managed.observation.unwrap();
        adapter.managed.record.as_mut().unwrap().context_deadline = sample
            .deadline_at_unix_ms(sample.unix_ms()).on_main(&main); // consumed context is no longer a gate
        if original_expired {
            adapter.managed.observation = Some(sample.advance_for_test(Duration::from_secs(60)));
        }
        publish(&mut adapter, &activation);
        assert_eq!(adapter.take_managed_completion().is_some(), !original_expired);
        adapter.cancel_managed(7);
        plex::reset_servers_for_test();
    }
}

#[test]
fn current_server_retargeted_before_publication_cannot_complete_managed_activation() {
    let _serial = plx_base::testlock::serial(); let _registry = RegistryGuard::new();
    for after_publication in [false, true] {
    let (mut adapter, activation, source, proof, cancel) = fixture();
    prepare(&mut adapter, activation.clone(), source, proof, 60000, 120000, cancel).unwrap();
    assert!(admit(&mut adapter, &activation).admission == CommitAdmission::RegistryOnly);
    if after_publication { publish(&mut adapter, &activation); }
    let origin = plex::Origin::http("127.0.0.1", 32401);
    let other = plex::grant::prove_import(plex::grant::scope(), &origin, "other", "other", true).unwrap();
    let external = plex::register_imported(&other, "other-pms", "install", plex::ConnectionFacts::default(), None).unwrap();
    assert!(plex::set_current(external.server_id()));
    publish(&mut adapter, &activation);
    assert!(adapter.take_managed_completion().is_none());
    assert_eq!(adapter.managed_stage(7), Some(if after_publication {
        ManagedStage::Published } else { ManagedStage::RegistryApplied }));
    adapter.cancel_managed(7);
    assert!(plex::retire_imported(&external));
    plex::reset_servers_for_test();
    }
}
#[test]
fn preallocation_refusal_matches_only_still_prepared_operation_and_epoch() {
    let _serial = plx_base::testlock::serial(); let _registry = RegistryGuard::new();
    let (mut adapter, activation, source, proof, cancel) = fixture();
    prepare(&mut adapter, activation, source, proof, 60000, 120000, cancel).unwrap();
    adapter.managed_admission_refused(8, 3);
    adapter.managed_admission_refused(7, 4);
    assert!(adapter.take_managed_admission_refusal().is_none());
    adapter.managed_admission_refused(7, 3);
    assert_eq!(adapter.take_managed_admission_refusal(), Some(ManagedAdmissionRefusal {
        operation: 7, expected_epoch: 3, error: ManagedError::Stale }));
    assert!(adapter.take_managed_completion().is_none(), "no fabricated request tuple");
    adapter.managed_admission_refused(7, 3);
    assert!(adapter.take_managed_admission_refusal().is_none());
    assert_eq!(adapter.managed_stage(7), Some(ManagedStage::Refused));
}

#[test]
fn queued_original_command_and_context_deadlines_refuse_delayed_prepare() {
    let _serial = plx_base::testlock::serial(); let _registry = RegistryGuard::new();
    for (command_ms, context_ms) in [(1_000, 2_000), (2_000, 1_000)] {
        let (mut adapter, activation, source, proof, cancel) = fixture();
        let (main, original) = clock_sample();
        let command = original.deadline_at_unix_ms(original.unix_ms() + command_ms).on_main(&main);
        let context = original.deadline_at_unix_ms(original.unix_ms() + context_ms).on_main(&main);
        // Simulate delay between the caller's original sample and actual
        // adapter preparation. No test sleeps and no new allowance is issued.
        adapter.managed.observation = Some(original.advance_for_test(Duration::from_millis(1_000)));
        assert_eq!(adapter.prepare_managed(activation, source.clone(), proof, command, context, cancel),
            Err(ManagedError::Expired));
        assert!(adapter.managed.record.is_none());
        assert!(plex::id_of_machine(&source.machine_id).is_none());
    }
}

#[test]
fn delayed_prepare_does_not_renew_pre_effect_deadline() {
    let _serial = plx_base::testlock::serial(); let _registry = RegistryGuard::new();
    for (command_ms, context_ms) in [(1_000, 2_000), (2_000, 1_000)] {
        let (mut adapter, activation, source, proof, cancel) = fixture();
        let (main, original) = clock_sample();
        adapter.managed.observation = Some(original.advance_for_test(Duration::from_millis(400)));
        adapter.prepare_managed(activation.clone(), source.clone(), proof,
            original.deadline_at_unix_ms(original.unix_ms() + command_ms).on_main(&main),
            original.deadline_at_unix_ms(original.unix_ms() + context_ms).on_main(&main), cancel).unwrap();
        adapter.managed.observation = Some(original.advance_for_test(Duration::from_millis(1_000)));
        assert!(admit(&mut adapter, &activation).admission == CommitAdmission::StaleAuthority);
        assert_eq!(adapter.managed.record.as_ref().unwrap().error, Some(ManagedError::Expired));
        assert!(plex::id_of_machine(&source.machine_id).is_none());
    }
}

#[test]
fn original_deadline_gates_both_publication_and_completion_after_staging_delay() {
    let _serial = plx_base::testlock::serial(); let _registry = RegistryGuard::new();
    for delayed_publication in [true, false] {
        let (mut adapter, activation, source, proof, cancel) = fixture();
        let (main, original) = clock_sample();
        adapter.managed.observation = Some(original.advance_for_test(Duration::from_millis(400)));
        adapter.prepare_managed(activation.clone(), source, proof,
            original.deadline_at_unix_ms(original.unix_ms() + 1_000).on_main(&main),
            original.deadline_at_unix_ms(original.unix_ms() + 2_000).on_main(&main), cancel).unwrap();
        adapter.managed.observation = Some(original.advance_for_test(Duration::from_millis(999)));
        assert!(admit(&mut adapter, &activation).admission == CommitAdmission::RegistryOnly);
        if !delayed_publication { publish(&mut adapter, &activation); }
        adapter.managed.observation = Some(original.advance_for_test(Duration::from_millis(1_000)));
        if delayed_publication { publish(&mut adapter, &activation); }
        assert!(adapter.take_managed_completion().is_none());
        assert_eq!(adapter.managed_stage(7), Some(if delayed_publication {
            ManagedStage::RegistryApplied } else { ManagedStage::Published }));
        adapter.cancel_managed(7);
        plex::reset_servers_for_test();
    }
}
