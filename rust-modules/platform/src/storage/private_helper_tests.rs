//! Facade/transport boundary regressions. The helper double is not DB8 or crypto evidence.
use super::*;
use crate::storage::wire::{CommitStatus, ReconcileStatus, Request, Response};

struct TestDirectory(PathBuf);
impl TestDirectory {
    fn new() -> Self {
        let nonce: String = Generation::random()
            .unwrap()
            .0
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        let path = std::env::temp_dir().join(format!("ott-helper-facade-{nonce}"));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
}
impl Drop for TestDirectory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn seal(namespace: Namespace, bytes: &[u8]) -> Option<Sealed> {
    Some(Sealed {
        backend: secure::Backend::Keymanager3,
        key: namespace.key_name(),
        iv: "AA==".into(),
        data: bytes.iter().map(|byte| format!("{byte:04x}")).collect(),
    })
}
fn open(namespace: Namespace, sealed: &Sealed) -> Option<Vec<u8>> {
    if sealed.key != namespace.key_name() {
        return None;
    }
    sealed
        .data
        .as_bytes()
        .chunks_exact(4)
        .map(|chunk| u8::from_str_radix(std::str::from_utf8(chunk).ok()?, 16).ok())
        .collect()
}
fn image() -> LegacyImage {
    LegacyImage {
        present: false,
        barrier: ott::Barrier::Open,
        control: None,
        state: None,
    }
}
struct Old {
    image: LegacyImage,
    reads: usize,
    erases: usize,
    erase_fails: bool,
}
impl Old {
    fn empty() -> Self {
        Self {
            image: image(),
            reads: 0,
            erases: 0,
            erase_fails: false,
        }
    }
}
impl Legacy for Old {
    fn read(&mut self) -> Result<LegacyImage, Error> {
        self.reads += 1;
        Ok(self.image.clone())
    }
    fn barrier(&mut self) -> Result<ott::Barrier, Error> {
        Ok(self.image.barrier)
    }
    fn erase(&mut self) -> Vec<String> {
        self.erases += 1;
        if self.erase_fails {
            vec!["fixture cleanup failure".into()]
        } else {
            Vec::new()
        }
    }
}

#[derive(Default)]
struct Helper {
    snapshot: Option<ott::Snapshot>,
    mutations: Vec<ott::Mutation>,
    unavailable: bool,
    lose_ack: bool,
    corrupt_readback: bool,
}
impl Helper {
    fn initialized() -> Self {
        Self {
            snapshot: Some(ott::Snapshot {
                status: ott::Status {
                    db_rev: "1".into(),
                    revision: 1,
                    epoch: 1,
                    barrier: ott::Barrier::Open,
                    control_present: false,
                    state_present: false,
                    private_initialized: true,
                },
                control: None,
                state: None,
                public_preferences: None,
            }),
            ..Self::default()
        }
    }
}
impl Transport for Helper {
    fn transact(&mut self, request: Request) -> Result<Response, client::ClientError> {
        if self.unavailable {
            return Err(client::ClientError::Unavailable);
        }
        match request {
            Request::OttStatus {} => Ok(Response::OttStatus {
                status: self.snapshot.as_ref().map(|s| s.status.clone()),
            }),
            Request::OttLoad {} => {
                let mut snapshot = self.snapshot.clone();
                if self.corrupt_readback {
                    if let Some(snapshot) = &mut snapshot {
                        snapshot.state = Some(
                            encode(
                                Namespace::OttState,
                                &serde_json::json!("different value"),
                                seal,
                            )
                            .unwrap(),
                        );
                    }
                }
                Ok(Response::OttLoaded { snapshot })
            }
            Request::OttCommit {
                expected,
                operation_id,
                digest,
                mutation,
            } => {
                assert_eq!(
                    expected,
                    remote::expected(self.snapshot.as_ref().map(|s| &s.status))
                );
                self.mutations.push(mutation.clone());
                let mut next = self
                    .snapshot
                    .clone()
                    .unwrap_or_else(|| Self::initialized().snapshot.unwrap());
                let previous_revision = self.snapshot.as_ref().map_or(0, |s| s.status.revision);
                match mutation {
                    ott::Mutation::Initialize {
                        control,
                        state,
                        barrier,
                    } => {
                        assert!(self.snapshot.is_none());
                        next.control = control;
                        next.state = state;
                        next.status.barrier = barrier;
                    }
                    ott::Mutation::CompletePrivateImport { control, state, barrier } => {
                        assert!(self.snapshot.is_some() && !next.status.private_initialized);
                        next.control = control;
                        next.state = state;
                        next.status.barrier = barrier;
                        next.status.private_initialized = true;
                        if barrier == ott::Barrier::Pending { next.public_preferences = None; }
                    }
                    ott::Mutation::PatchPublic { patch } => {
                        next.status.private_initialized = self.snapshot.as_ref()
                            .is_some_and(|snapshot| snapshot.status.private_initialized);
                        next.public_preferences = Some(next.public_preferences.clone()
                            .unwrap_or_default().apply(&patch).unwrap());
                    }
                    ott::Mutation::Put { namespace, value } => {
                        assert!(self.snapshot.is_some());
                        assert_ne!(next.status.barrier, ott::Barrier::Pending);
                        match namespace {
                            ott::Namespace::Control => next.control = Some(value),
                            ott::Namespace::State => next.state = Some(value),
                        }
                    }
                    ott::Mutation::BeginErase {} => {
                        next.status.epoch += 1;
                        next.status.barrier = ott::Barrier::Pending;
                        next.control = None;
                        next.state = None;
                        next.public_preferences = None;
                        next.status.private_initialized = true;
                    }
                    ott::Mutation::FinishErase {} => {
                        assert_eq!(next.status.barrier, ott::Barrier::Pending);
                        next.status.barrier = ott::Barrier::Complete;
                    }
                }
                next.status.revision = previous_revision + 1;
                next.status.db_rev = next.status.revision.to_string();
                next.status.control_present = next.control.is_some();
                next.status.state_present = next.state.is_some();
                let receipt = ott::Receipt {
                    operation_id,
                    digest,
                    revision: next.status.revision,
                    epoch: next.status.epoch,
                    verified: true,
                };
                self.snapshot = Some(next);
                if self.lose_ack {
                    Err(client::ClientError::Unavailable)
                } else {
                    Ok(Response::OttCommit {
                        status: CommitStatus::Committed,
                        receipt: Some(receipt),
                    })
                }
            }
            Request::OttReconcile { .. } => Ok(Response::OttReconcile {
                status: ReconcileStatus::Unknown,
                receipt: None,
            }),
            _ => panic!("unrelated protocol request"),
        }
    }
}

#[test]
fn authoritative_canonical_absence_is_required_before_reading_legacy() {
    let mut old = Old::empty();
    let mut missing = Helper::default();
    assert_eq!(
        load_with(&mut missing, &mut old, Namespace::OttState, open).unwrap(),
        None
    );
    assert_eq!(old.reads, 1);
    assert!(missing.mutations.is_empty());
    let mut failed = Helper {
        unavailable: true,
        ..Helper::default()
    };
    assert!(load_with(&mut failed, &mut old, Namespace::OttState, open).is_err());
    assert_eq!(old.reads, 1, "helper failure must not fall back");
    let mut canonical = Helper::initialized();
    old.image.present = true;
    old.image.state = Some(
        encode(
            Namespace::OttState,
            &serde_json::json!("stale legacy"),
            seal,
        )
        .unwrap(),
    );
    assert_eq!(
        load_with(&mut canonical, &mut old, Namespace::OttState, open).unwrap(),
        None
    );
    assert_eq!(
        old.reads, 1,
        "empty canonical namespace must not import old files"
    );
}

#[test]
fn migration_authenticates_both_namespaces_and_copies_ciphertext_atomically() {
    let mut old = Old::empty();
    old.image.present = true;
    old.image.control = Some(
        encode(
            Namespace::OttControl,
            &serde_json::json!({"token":"fixture control"}),
            seal,
        )
        .unwrap(),
    );
    old.image.state = Some(
        encode(
            Namespace::OttState,
            &serde_json::json!({"profile":"fixture state"}),
            seal,
        )
        .unwrap(),
    );
    let mut helper = Helper::default();
    assert_eq!(
        load_with(&mut helper, &mut old, Namespace::OttState, open).unwrap(),
        Some(serde_json::json!({"profile":"fixture state"}))
    );
    assert_eq!(helper.mutations.len(), 1);
    let snapshot = helper.snapshot.unwrap();
    assert_eq!(snapshot.control, old.image.control);
    assert_eq!(snapshot.state, old.image.state);
    assert_eq!(
        old.erases, 0,
        "automatic migration retains old encrypted files"
    );
}

#[test]
fn corrupt_other_namespace_blocks_migration_without_a_partial_write() {
    let mut old = Old::empty();
    old.image.present = true;
    old.image.state = Some(encode(Namespace::OttState, &serde_json::json!("valid"), seal).unwrap());
    old.image.control = old.image.state.clone(); // Wrong namespace/key despite valid encryption shape.
    let mut helper = Helper::default();
    assert!(load_with(&mut helper, &mut old, Namespace::OttState, open).is_err());
    assert!(helper.mutations.is_empty());
    assert_eq!(old.erases, 0);
}

#[test]
fn uncertain_migration_neither_serves_old_data_nor_cleans_it_up() {
    let mut old = Old::empty();
    old.image.present = true;
    old.image.state = Some(encode(Namespace::OttState, &serde_json::json!("kept"), seal).unwrap());
    let mut helper = Helper {
        lose_ack: true,
        ..Helper::default()
    };
    assert!(load_with(&mut helper, &mut old, Namespace::OttState, open).is_err());
    assert_eq!(old.erases, 0);
    assert_eq!(helper.mutations.len(), 1);
}

#[test]
fn successful_save_requires_exact_authenticated_readback() {
    let value = serde_json::json!({"profile":"new"});
    let mut old = Old::empty();
    let permit = WritePermit::new();
    let mut helper = Helper::initialized();
    assert_eq!(
        save_with(
            &mut helper,
            &mut old,
            Namespace::OttState,
            &value,
            &permit,
            seal,
            open
        )
        .unwrap(),
        CommitReceipt::Durable
    );
    helper.corrupt_readback = true;
    assert!(matches!(
        save_with(
            &mut helper,
            &mut old,
            Namespace::OttState,
            &value,
            &permit,
            seal,
            open
        )
        .unwrap(),
        CommitReceipt::Uncertain { .. }
    ));
    assert_eq!(old.reads, 0);
}

#[test]
fn lost_save_ack_is_uncertain_not_definitely_not_applied() {
    let mut helper = Helper {
        lose_ack: true,
        ..Helper::initialized()
    };
    let receipt = save_with(
        &mut helper,
        &mut Old::empty(),
        Namespace::OttState,
        &serde_json::json!(1),
        &WritePermit::new(),
        seal,
        open,
    )
    .unwrap();
    assert!(matches!(receipt, CommitReceipt::Uncertain { .. }));
    assert_eq!(helper.mutations.len(), 1);
}

#[test]
fn revoked_write_does_not_read_or_dispatch() {
    let mut helper = Helper {
        unavailable: true,
        ..Helper::default()
    };
    let mut old = Old::empty();
    let permit = WritePermit::new();
    permit.revoke();
    assert_eq!(
        save_with(
            &mut helper,
            &mut old,
            Namespace::OttState,
            &serde_json::json!(1),
            &permit,
            seal,
            open
        ),
        Err(Error::Revoked)
    );
    assert_eq!(old.reads, 0);
    assert!(helper.mutations.is_empty());
}

#[test]
fn erase_uses_metadata_and_failed_legacy_cleanup_keeps_pending() {
    let mut helper = Helper::initialized();
    helper.snapshot.as_mut().unwrap().state =
        Some(SecretString("unopenable old ciphertext".into()));
    helper.snapshot.as_mut().unwrap().status.state_present = true;
    begin_with(&mut helper).unwrap();
    let mut old = Old {
        erase_fails: true,
        ..Old::empty()
    };
    assert!(finish_with(&mut helper, &mut old).is_err());
    assert_eq!(
        helper.snapshot.as_ref().unwrap().status.barrier,
        ott::Barrier::Pending
    );
    assert_eq!(helper.mutations.len(), 1);
    assert_eq!(old.reads, 0);
    old.erase_fails = false;
    finish_with(&mut helper, &mut old).unwrap();
    assert_eq!(
        helper.snapshot.as_ref().unwrap().status.barrier,
        ott::Barrier::Complete
    );
    assert_eq!(old.reads, 0);
}

#[test]
fn canonical_open_keeps_explicit_managed_provisioning_available() {
    let mut helper = Helper::initialized();
    let mut old = Old::empty();
    old.image.barrier = ott::Barrier::Pending;
    assert_eq!(
        barrier_with(&mut helper, &mut old).unwrap(),
        ott::Barrier::Open
    );
    assert_eq!(old.reads, 0);
}

#[test]
fn legacy_pending_barrier_is_migrated_without_decryption_or_fallback() {
    fn never_open(_: Namespace, _: &Sealed) -> Option<Vec<u8>> {
        panic!("erasure must not require decryption")
    }
    let mut helper = Helper::default();
    let mut old = Old::empty();
    old.image.present = true;
    old.image.barrier = ott::Barrier::Pending;
    assert_eq!(
        load_with(&mut helper, &mut old, Namespace::OttState, never_open),
        Err(Error::Revoked)
    );
    assert_eq!(
        helper.snapshot.as_ref().unwrap().status.barrier,
        ott::Barrier::Pending
    );
    assert_eq!(old.erases, 0);
}

#[test]
fn revocation_during_status_read_prevents_crypto_and_commit() {
    fn never_seal(_: Namespace, _: &[u8]) -> Option<Sealed> {
        panic!("revoked operation must not reach encryption")
    }
    let permit = WritePermit::new();
    let mut helper = Helper::initialized();
    let mut transport = |request| {
        permit.revoke();
        helper.transact(request)
    };
    assert_eq!(
        save_with(
            &mut transport,
            &mut Old::empty(),
            Namespace::OttState,
            &serde_json::json!(1),
            &permit,
            never_seal,
            open
        ),
        Err(Error::Revoked)
    );
    assert!(helper.mutations.is_empty());
}

#[test]
fn concurrent_first_namespace_writer_reloads_and_preserves_the_winner() {
    let control = encode(
        Namespace::OttControl,
        &serde_json::json!({"device":"fixture"}),
        seal,
    )
    .unwrap();
    let mut helper = Helper::default();
    let mut raced = false;
    let mut transport = |request| {
        if matches!(
            &request,
            Request::OttCommit {
                mutation: ott::Mutation::Initialize { .. },
                ..
            }
        ) && !raced
        {
            raced = true;
            let mut winner = Helper::initialized().snapshot.unwrap();
            winner.control = Some(control.clone());
            winner.status.control_present = true;
            helper.snapshot = Some(winner);
            Ok(Response::OttCommit {
                status: CommitStatus::Conflict,
                receipt: None,
            })
        } else {
            helper.transact(request)
        }
    };
    let value = serde_json::json!({"profile":"state writer"});
    assert_eq!(
        save_with(
            &mut transport,
            &mut Old::empty(),
            Namespace::OttState,
            &value,
            &WritePermit::new(),
            seal,
            open
        )
        .unwrap(),
        CommitReceipt::Durable
    );
    assert!(raced);
    assert!(matches!(
        helper.mutations.as_slice(),
        [ott::Mutation::Put {
            namespace: ott::Namespace::State,
            ..
        }]
    ));
    let snapshot = helper.snapshot.unwrap();
    assert_eq!(snapshot.control, Some(control));
    assert_eq!(
        decode(Namespace::OttState, snapshot.state.as_ref().unwrap(), open).unwrap(),
        value
    );
}

#[test]
fn initializer_repair_refuses_erasure_and_already_written_namespaces() {
    for (barrier, epoch, occupied) in [
        (ott::Barrier::Complete, 2, false),
        (ott::Barrier::Open, 2, false),
        (ott::Barrier::Open, 1, true),
    ] {
        let mut helper = Helper::default();
        let mut transport = |request| {
            if matches!(
                &request,
                Request::OttCommit {
                    mutation: ott::Mutation::Initialize { .. },
                    ..
                }
            ) {
                let mut winner = Helper::initialized().snapshot.unwrap();
                winner.status.barrier = barrier;
                winner.status.epoch = epoch;
                if occupied {
                    winner.state = Some(
                        encode(Namespace::OttState, &serde_json::json!("winner"), seal).unwrap(),
                    );
                    winner.status.state_present = true;
                }
                helper.snapshot = Some(winner);
                Ok(Response::OttCommit {
                    status: CommitStatus::Conflict,
                    receipt: None,
                })
            } else {
                helper.transact(request)
            }
        };
        assert_eq!(
            save_with(
                &mut transport,
                &mut Old::empty(),
                Namespace::OttState,
                &serde_json::json!("loser"),
                &WritePermit::new(),
                seal,
                open
            ),
            Err(Error::Store(StoreError::Conflict))
        );
        assert!(helper.mutations.is_empty());
    }
}

#[test]
fn completed_legacy_erasure_preserves_new_records_in_both_namespaces() {
    let directory = TestDirectory::new();
    let private = directory.0.join("ott-state");
    let store = open_or_create(&private).unwrap();
    store
        .commit(
            RecordKey::OttErasure,
            &Record::data(2, "ott-erasure-complete-v1".into()),
        )
        .unwrap();
    let control = serde_json::json!({"token":"new after completed erasure"});
    let state = serde_json::json!({"profile":"new after completed erasure"});
    super::super::save_at(&private, Namespace::OttControl, &control, seal).unwrap();
    super::super::save_at(&private, Namespace::OttState, &state, seal).unwrap();
    let mut old = Old {
        image: read_legacy_at(&private).unwrap(),
        ..Old::empty()
    };
    assert_eq!(old.image.barrier, ott::Barrier::Complete);
    assert!(old.image.control.is_some() && old.image.state.is_some());
    let mut helper = Helper::default();
    assert_eq!(
        load_with(&mut helper, &mut old, Namespace::OttControl, open).unwrap(),
        Some(control)
    );
    assert_eq!(
        load_with(&mut helper, &mut old, Namespace::OttState, open).unwrap(),
        Some(state)
    );
    assert_eq!(
        barrier_with(&mut helper, &mut old).unwrap(),
        ott::Barrier::Complete
    );
    assert_eq!(helper.mutations.len(), 1);
    assert_eq!(old.erases, 0);
}

#[test]
fn erased_managed_file_still_requires_parent_sync_before_finishing_on_retry() {
    struct DiskLegacy(JsonStore);
    impl Legacy for DiskLegacy {
        fn read(&mut self) -> Result<LegacyImage, Error> {
            panic!("erase must not read legacy credentials")
        }
        fn barrier(&mut self) -> Result<ott::Barrier, Error> {
            Ok(ott::Barrier::Pending)
        }
        fn erase(&mut self) -> Vec<String> {
            erase_managed(&self.0)
        }
    }
    let directory = TestDirectory::new();
    let file = directory.0.join("ottplay-control.json");
    std::fs::write(&file, b"synthetic managed value").unwrap();
    let mut legacy = DiskLegacy(JsonStore::new(directory.0.clone()).unwrap());
    let mut helper = Helper::initialized();
    begin_with(&mut helper).unwrap();
    legacy.0.inject_failure(CommitStage::ParentSync);
    assert!(finish_with(&mut helper, &mut legacy).is_err());
    assert!(
        !file.exists(),
        "unlink succeeded before the injected fsync failure"
    );
    assert_eq!(
        helper.mutations.len(),
        1,
        "no Finish after failed durable cleanup"
    );
    legacy.0.inject_failure(CommitStage::ParentSync);
    assert!(
        finish_with(&mut helper, &mut legacy).is_err(),
        "retry must fsync even though the file is absent"
    );
    assert_eq!(helper.mutations.len(), 1);
    assert_eq!(
        helper.snapshot.as_ref().unwrap().status.barrier,
        ott::Barrier::Pending
    );
    finish_with(&mut helper, &mut legacy).unwrap();
    assert_eq!(
        helper.snapshot.as_ref().unwrap().status.barrier,
        ott::Barrier::Complete
    );
}

#[test]
fn managed_cleanup_callback_runs_only_after_a_durable_private_barrier() {
    let directory = TestDirectory::new();
    let store = open_or_create(&directory.0.join("ott-state")).unwrap();
    let called = std::cell::Cell::new(false);
    store.inject_failure(CommitStage::ParentSync);
    let errors = erase_store_with(&store, || {
        called.set(true);
        Vec::new()
    });
    assert!(!errors.is_empty());
    assert!(
        !called.get(),
        "failed barrier must not reach managed cleanup"
    );
    let errors = erase_store_with(&store, || {
        called.set(true);
        assert!(matches!(erasure_state(&store), Ok(ErasureState::Pending)));
        for key in [RecordKey::OttControl, RecordKey::OttState] {
            assert!(matches!(
                store.load(key).unwrap().unwrap().state,
                RecordState::Cleared
            ));
        }
        vec!["synthetic managed cleanup failure".into()]
    });
    assert!(called.get());
    assert_eq!(errors, vec!["synthetic managed cleanup failure"]);
}

#[test]
fn appearing_private_root_prevents_no_legacy_cleanup_success() {
    let directory = TestDirectory::new();
    let private = directory.0.join("ott-state");
    assert!(erase_no_private(&private, Vec::new).is_empty());
    let errors = erase_no_private(&private, || {
        std::fs::create_dir(&private).unwrap();
        Vec::new()
    });
    assert!(!errors.is_empty());
    assert!(private.is_dir());
    assert!(!erase_no_private(&private, || panic!(
        "new private root must be handled first"
    ))
    .is_empty());
}

#[test]
fn wrong_install_and_oversize_plaintext_fail_before_commit() {
    let mut helper = Helper::default();
    let mut old = Old::empty();
    old.image.present = true;
    let plain = serde_json::json!({"version":1,"install":"com.openottplay.player.native.debug","namespace":"ott-state","value":1});
    old.image.state = Some(SecretString(
        serde_json::to_string(&Envelope {
            version: 1,
            sealed: seal(Namespace::OttState, &serde_json::to_vec(&plain).unwrap()).unwrap(),
        })
        .unwrap(),
    ));
    assert!(load_with(&mut helper, &mut old, Namespace::OttState, open).is_err());
    assert!(helper.mutations.is_empty());
    let value = serde_json::json!("x".repeat(LOGICAL_BYTES));
    assert_eq!(
        save_with(
            &mut helper,
            &mut Old::empty(),
            Namespace::OttState,
            &value,
            &WritePermit::new(),
            seal,
            open
        ),
        Err(Error::TooLarge)
    );
    assert!(helper.mutations.is_empty());
}


// Strict transport: inspection may only issue OttLoad, never readiness/migration writes.
struct InspectTransport {
    helper: Helper,
    loads: usize,
}
impl Transport for InspectTransport {
    fn transact(&mut self, request: Request) -> Result<Response, client::ClientError> {
        assert!(matches!(request, Request::OttLoad {}));
        self.loads += 1;
        self.helper.transact(request)
    }
}

#[test]
fn inspection_hashes_the_exact_authenticated_namespace_value_with_one_load() {
    let logical = serde_json::json!({"favorites": ["fixture"]});
    let value = encode(Namespace::OttState, &logical, seal).unwrap();
    let exact = format!("\n{}\n", value.0);
    let mut helper = Helper::initialized();
    let snapshot = helper.snapshot.as_mut().unwrap();
    snapshot.state = Some(SecretString(exact.clone()));
    snapshot.status.state_present = true;
    let mut transport = InspectTransport { helper, loads: 0 };
    let result = inspect_with(
        &mut transport,
        || panic!("canonical data cannot read legacy"),
        Namespace::OttState,
        open,
    )
    .unwrap()
    .unwrap();
    assert_eq!(result.value(), &logical);
    assert_eq!(
        result.namespace_sha256(),
        crate::storage::state::digest_bytes(exact.as_bytes())
    );
    assert_eq!(transport.loads, 1);
    assert!(transport.helper.mutations.is_empty());
    assert_eq!(
        transport
            .helper
            .snapshot
            .as_ref()
            .unwrap()
            .state
            .as_ref()
            .unwrap()
            .0,
        exact
    );
    assert!(inspect_with(
        &mut transport,
        || panic!("canonical empty slot cannot read legacy"),
        Namespace::OttControl,
        open
    )
    .unwrap()
    .is_none());
}

#[test]
fn inspection_reports_missing_and_legacy_only_without_initialization() {
    let mut transport = InspectTransport {
        helper: Helper::default(),
        loads: 0,
    };
    let legacy_reads = std::cell::Cell::new(0);
    assert!(inspect_with(
        &mut transport,
        || {
            legacy_reads.set(legacy_reads.get() + 1);
            Ok(false)
        },
        Namespace::OttState,
        open
    )
    .unwrap()
    .is_none());
    assert!(matches!(
        inspect_with(
            &mut transport,
            || {
                legacy_reads.set(legacy_reads.get() + 1);
                Ok(true)
            },
            Namespace::OttState,
            open
        ),
        Err(InspectError::LegacyOnly)
    ));
    assert_eq!(legacy_reads.get(), 2);
    assert_eq!(transport.loads, 2);
    assert!(transport.helper.snapshot.is_none());
    assert!(transport.helper.mutations.is_empty());
}

#[test]
fn inspection_does_not_fallback_on_unavailable_corrupt_or_pending_helper_state() {
    let mut transport = InspectTransport {
        helper: Helper {
            unavailable: true,
            ..Helper::default()
        },
        loads: 0,
    };
    assert!(matches!(
        inspect_with(
            &mut transport,
            || panic!("failure cannot read legacy"),
            Namespace::OttState,
            open
        ),
        Err(InspectError::Storage(Error::Store(
            StoreError::HelperUnavailable
        )))
    ));
    transport.helper = Helper::initialized();
    transport.helper.snapshot.as_mut().unwrap().status.barrier = ott::Barrier::Pending;
    assert!(inspect_with(
        &mut transport,
        || panic!("pending cannot read legacy"),
        Namespace::OttState,
        open
    )
    .is_err());
    transport.helper.snapshot.as_mut().unwrap().status.barrier = ott::Barrier::Open;
    transport.helper.snapshot.as_mut().unwrap().state = Some(SecretString("invalid".into()));
    transport
        .helper
        .snapshot
        .as_mut()
        .unwrap()
        .status
        .state_present = true;
    assert!(inspect_with(
        &mut transport,
        || panic!("corrupt cannot read legacy"),
        Namespace::OttState,
        open
    )
    .is_err());
    assert!(transport.helper.mutations.is_empty());
}

#[test]
fn inspection_rejects_failed_authentication_without_exposing_a_digest() {
    let mut helper = Helper::initialized();
    helper.snapshot.as_mut().unwrap().state =
        Some(encode(Namespace::OttState, &Value::Null, seal).unwrap());
    helper.snapshot.as_mut().unwrap().status.state_present = true;
    let mut transport = InspectTransport { helper, loads: 0 };
    assert!(matches!(
        inspect_with(
            &mut transport,
            || panic!("canonical cannot read legacy"),
            Namespace::OttState,
            |_, _| None
        ),
        Err(InspectError::Storage(Error::ProtectionUnavailable))
    ));
    assert!(transport.helper.mutations.is_empty());
}

#[test]
fn readonly_legacy_probe_retains_cleared_presence_and_never_repairs_mode() {
    use std::os::unix::fs::PermissionsExt;
    let dir = TestDirectory::new();
    let private = dir.0.join("private");
    assert!(!legacy_present_readonly(&private).unwrap());
    assert!(!private.exists());
    assert_eq!(
        super::super::save_at(&private, Namespace::OttState, &Value::Null, seal).unwrap(),
        CommitReceipt::Durable
    );
    let store = JsonStore::new(private.clone()).unwrap();
    store
        .commit(
            RecordKey::OttState,
            &Record {
                revision: 2,
                state: RecordState::Cleared,
            },
        )
        .unwrap();
    assert!(legacy_present_readonly(&private).unwrap());
    let record = private.join("ott-state.json");
    std::fs::set_permissions(&record, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert_eq!(
        legacy_present_readonly(&private).err(),
        Some(Error::Store(StoreError::RecordUnsafeMode))
    );
    assert_eq!(
        std::fs::metadata(&record).unwrap().permissions().mode() & 0o777,
        0o644
    );
}

#[test]
fn helper_encode_reuses_logical_preflight_before_sealing() {
    let value = Value::String("x".repeat(LOGICAL_BYTES));
    assert_eq!(
        validate_value(Namespace::OttState, &value),
        Err(Error::TooLarge)
    );
    assert!(matches!(
        encode(Namespace::OttState, &value, |_, _| panic!(
            "oversize must not reach the sealer"
        )),
        Err(Error::TooLarge)
    ));
}

#[path = "private_helper_named_tests.rs"]
mod named_protection;

#[test]
fn public_preference_save_works_without_private_crypto_and_retains_migration_obligation() {
    let mut helper = Helper::default();
    let mut old = Old::empty();
    let (saved, receipt) = public_patch_with(&mut helper, &mut old,
        ott::PublicPatch::InterfaceMode(0), &WritePermit::new()).unwrap();
    assert_eq!(receipt, CommitReceipt::Durable);
    assert_eq!(saved.value.interface_mode, Some(0));
    assert_eq!(old.reads, 0, "public save does not read/decrypt old private payloads");
    assert!(!helper.snapshot.as_ref().unwrap().status.private_initialized);
    assert!(matches!(helper.mutations.as_slice(), [ott::Mutation::PatchPublic { .. }]));
}

#[test]
fn notification_policy_helper_cas_requires_readback_and_preserves_control_and_provider_slots() {
    let mut helper = Helper::initialized();
    let mut old = Old::empty();
    let snapshot=helper.snapshot.as_mut().unwrap();
    snapshot.control=Some(encode(Namespace::OttControl,&serde_json::json!({"token":"synthetic-pairing"}),seal).unwrap());
    snapshot.state=Some(encode(Namespace::OttState,&serde_json::json!({"profile":"retained-profile"}),seal).unwrap());
    snapshot.status.control_present=true; snapshot.status.state_present=true;
    let before = helper.snapshot.clone().unwrap();
    let permit = WritePermit::new();
    let (off, receipt) = public_patch_with(&mut helper, &mut old,
        ott::PublicPatch::NotificationsEnabled(false), &permit).unwrap();
    assert_eq!(receipt, CommitReceipt::Durable);
    assert_eq!(off.value.notifications_enabled, Some(false));
    let readback = public_current(&mut helper, &mut old).unwrap().unwrap();
    assert_eq!(readback.public_preferences.as_ref().unwrap().notifications_enabled,Some(false));
    assert_eq!(readback.control,before.control); assert_eq!(readback.state,before.state);
    assert_eq!(old.reads,0,"notification policy never opens provider or pairing secrets");
    helper.lose_ack=true;
    assert_eq!(public_patch_with(&mut helper,&mut old,ott::PublicPatch::NotificationsEnabled(true),&permit),
        Err(public_preferences::SaveError::Uncertain));
    // Even a visible candidate cannot turn a lost acknowledgement into success.
    assert_eq!(helper.snapshot.as_ref().unwrap().public_preferences.as_ref().unwrap().notifications_enabled,Some(true));
    permit.revoke(); let count=helper.mutations.len();
    assert_eq!(public_patch_with(&mut helper,&mut old,ott::PublicPatch::NotificationsEnabled(false),&permit),
        Err(public_preferences::SaveError::NotApplied(Error::Revoked)));
    assert_eq!(helper.mutations.len(),count);
}

#[test]
fn pending_private_import_preserves_preferences_and_refuses_locked_legacy() {
    let mut helper = Helper::default();
    let mut old = Old::empty();
    old.image = LegacyImage { present: true, barrier: ott::Barrier::Open,
        control: Some(encode(Namespace::OttControl, &serde_json::json!({"token":"fixture-secret"}), seal).unwrap()),
        state: Some(encode(Namespace::OttState, &serde_json::json!({"old":true}), seal).unwrap()) };
    public_patch_with(&mut helper, &mut old, ott::PublicPatch::InterfaceMode(0), &WritePermit::new()).unwrap();
    let public = helper.snapshot.clone().unwrap();
    assert!(ready(&mut helper, &mut old, |_, _| None, &WritePermit::new()).is_err());
    assert_eq!(helper.snapshot, Some(public.clone()), "failed authentication changes no canonical data");
    let observed = public_current(&mut helper, &mut old).unwrap().unwrap();
    assert_eq!(observed.public_preferences, public.public_preferences);
    let status = ready(&mut helper, &mut old, open, &WritePermit::new()).unwrap().unwrap();
    assert!(status.private_initialized);
    let after = helper.snapshot.as_ref().unwrap();
    assert_eq!(after.public_preferences, public.public_preferences);
    assert_eq!(after.control, old.image.control);
    assert_eq!(after.state, old.image.state);
    assert_eq!(old.erases, 0, "migration retains encrypted legacy bytes");
}

#[test]
fn typed_state_import_validates_before_commit_and_retains_both_legacy_slots() {
    for invalid in [true, false] {
        let mut helper = Helper::default();
        let mut old = Old::empty();
        let value = if invalid { serde_json::json!({"version":2,"settings":{}}) }
            else { serde_json::json!({"version":1,"settings":{"nativeCompanionRevision":7}}) };
        old.image = LegacyImage { present: true, barrier: ott::Barrier::Open,
            control: Some(encode(Namespace::OttControl, &serde_json::json!({"token":"fixture"}), seal).unwrap()),
            state: Some(encode(Namespace::OttState, &value, seal).unwrap()) };
        public_patch_with(&mut helper, &mut old, ott::PublicPatch::InterfaceMode(0), &WritePermit::new()).unwrap();
        let public = helper.snapshot.clone().unwrap();
        let writes = helper.mutations.len();
        let result = ready_checked_with_state(&mut helper, &mut old, open, &WritePermit::new(), None, true);
        if invalid {
            assert_eq!(result, Err(Error::Invalid));
            assert_eq!(helper.snapshot, Some(public));
            assert_eq!(helper.mutations.len(), writes);
        } else {
            assert!(result.unwrap().unwrap().private_initialized);
            let after = helper.snapshot.as_ref().unwrap();
            assert_eq!(after.public_preferences, public.public_preferences);
            assert_eq!(after.control, old.image.control);
            assert_eq!(after.state, old.image.state);
        }
        assert_eq!(old.erases, 0);
    }
}

#[test]
fn public_existing_identity_is_reconfirmed_and_revocation_stops_dispatch() {
    let mut helper = Helper::default();
    let mut old = Old::empty();
    let permit = WritePermit::new();
    let (first, _) = public_patch_with(&mut helper, &mut old,
        ott::PublicPatch::EnsureDeviceId("original".into()), &permit).unwrap();
    let (second, receipt) = public_patch_with(&mut helper, &mut old,
        ott::PublicPatch::EnsureDeviceId("replacement".into()), &permit).unwrap();
    assert_eq!(receipt, CommitReceipt::Durable);
    assert_eq!(second.value.device_id.as_deref(), Some("original"));
    assert!(second.revision > first.revision);
    permit.revoke();
    let count = helper.mutations.len();
    assert_eq!(public_patch_with(&mut helper, &mut old,
        ott::PublicPatch::InterfaceMode(0), &permit),
        Err(public_preferences::SaveError::NotApplied(Error::Revoked)));
    assert_eq!(helper.mutations.len(), count);
}

#[test]
fn public_legacy_erase_barrier_remains_effective_before_private_import() {
    let mut helper = Helper::default();
    let mut old = Old::empty();
    public_patch_with(&mut helper, &mut old, ott::PublicPatch::InterfaceMode(0), &WritePermit::new()).unwrap();
    old.image.barrier = ott::Barrier::Pending;
    assert_eq!(barrier_with(&mut helper, &mut old).unwrap(), ott::Barrier::Pending);
    let count = helper.mutations.len();
    assert_eq!(public_patch_with(&mut helper, &mut old,
        ott::PublicPatch::RemoteAddress("https://control.invalid:443".into()), &WritePermit::new()),
        Err(public_preferences::SaveError::NotApplied(Error::Revoked)));
    assert_eq!(helper.mutations.len(), count);
}

#[test]
fn public_lost_commit_ack_remains_uncertain_even_when_candidate_is_visible() {
    let mut helper = Helper { lose_ack: true, ..Helper::default() };
    let mut old = Old::empty();
    assert_eq!(public_patch_with(&mut helper, &mut old,
        ott::PublicPatch::InterfaceMode(0), &WritePermit::new()),
        Err(public_preferences::SaveError::Uncertain));
    assert_eq!(helper.snapshot.as_ref().unwrap().public_preferences.as_ref().unwrap().interface_mode, Some(0));
}
