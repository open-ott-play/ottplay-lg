//! Typed named-key failures through the real save facade and in-memory helper.
//! Keymanager RPC behavior is tested in that module, across the port fence.
//! This file performs no TV, diagnostic worker, filesystem store, or crypto IO.
use super::*;
use crate::storage::wire::{
    ErrorCode, KeymanagerFailure, KeymanagerFailureCategory, KeymanagerOperation, KeymanagerStage,
};
use std::cell::{Cell, RefCell};

thread_local! {
    static REJECTION: Cell<Option<KeymanagerFailure>> = const { Cell::new(None) };
    static SEAL_CALLS: RefCell<Vec<Namespace>> = const { RefCell::new(Vec::new()) };
}
struct Sealer;
impl Sealer {
    fn new(stage: KeymanagerStage) -> Self {
        REJECTION.with(|value| {
            value.set(Some(KeymanagerFailure {
                operation: KeymanagerOperation::Seal,
                stage,
                code: ErrorCode::Capability,
                category: KeymanagerFailureCategory::ServiceRejected,
                service_code: Some(-20030),
                hub_error: None,
            }))
        });
        SEAL_CALLS.with(|calls| calls.borrow_mut().clear());
        Self
    }
    fn calls(&self) -> Vec<Namespace> {
        SEAL_CALLS.with(|calls| calls.borrow().clone())
    }
}
impl Drop for Sealer {
    fn drop(&mut self) {
        REJECTION.with(|value| value.set(None));
        SEAL_CALLS.with(|calls| calls.borrow_mut().clear());
    }
}
fn reject_seal(namespace: Namespace, bytes: &[u8]) -> Result<Sealed, KeymanagerFailure> {
    assert!(
        !bytes.is_empty(),
        "the facade must prepare its actual protected value"
    );
    SEAL_CALLS.with(|calls| calls.borrow_mut().push(namespace));
    Err(REJECTION
        .with(|value| value.get())
        .expect("configured external-sealer rejection"))
}

#[test]
fn named_key_failure_preserves_both_canonical_namespaces_without_a_commit() {
    let _serial = plx_base::testlock::serial();
    for expected_stage in [
        KeymanagerStage::Generate,
        KeymanagerStage::Begin,
        KeymanagerStage::Finish,
    ] {
        let mut helper = Helper::initialized();
        let mut old = Old::empty();
        let permit = WritePermit::new();
        // Establish a real facade-validated canonical image first. Its bytes
        // are synthetic ciphertext; the existing test codec is not crypto.
        for (namespace, value) in [
            (
                Namespace::OttControl,
                serde_json::json!({"control":"old-control"}),
            ),
            (
                Namespace::OttState,
                serde_json::json!({"profile":"old-profile"}),
            ),
        ] {
            assert_eq!(
                save_with(
                    &mut helper,
                    &mut old,
                    namespace,
                    &value,
                    &permit,
                    seal,
                    open
                )
                .unwrap(),
                CommitReceipt::Durable
            );
        }
        let before = helper.snapshot.clone();
        let mutations = helper.mutations.len();
        let hook = Sealer::new(expected_stage);
        let result = save_with_detailed(
            &mut helper,
            &mut old,
            Namespace::OttState,
            &serde_json::json!({"profile":"replacement-profile"}),
            &permit,
            reject_seal,
            open,
        );
        let Err(Error::ProtectionFailed(failure)) = result else {
            panic!("the original typed named-key failure must reach the save facade");
        };
        assert_eq!(failure.operation, KeymanagerOperation::Seal);
        assert_eq!(failure.stage, expected_stage);
        assert_eq!(failure.service_code, Some(-20030));
        assert_eq!(hook.calls(), vec![Namespace::OttState]);
        assert_eq!(helper.mutations.len(), mutations, "no candidate OttCommit");
        assert_eq!(
            helper.snapshot, before,
            "both encrypted slots and revision stay exact"
        );
        assert_eq!(old.reads, 0, "canonical state needs no legacy migration");
        assert_eq!(old.erases, 0);
        assert_eq!(
            load_with(&mut helper, &mut old, Namespace::OttState, open).unwrap(),
            Some(serde_json::json!({"profile":"old-profile"}))
        );
        assert_eq!(
            load_with(&mut helper, &mut old, Namespace::OttControl, open).unwrap(),
            Some(serde_json::json!({"control":"old-control"}))
        );
    }
}

#[test]
fn preseal_refusal_never_calls_named_crypto_or_changes_the_existing_image() {
    let _serial = plx_base::testlock::serial();
    for revoked in [false, true] {
        let mut helper = Helper::initialized();
        let mut old = Old::empty();
        let permit = WritePermit::new();
        for (namespace, value) in [
            (
                Namespace::OttControl,
                serde_json::json!({"control":"old-control"}),
            ),
            (
                Namespace::OttState,
                serde_json::json!({"profile":"old-profile"}),
            ),
        ] {
            assert_eq!(
                save_with(
                    &mut helper,
                    &mut old,
                    namespace,
                    &value,
                    &permit,
                    seal,
                    open
                )
                .unwrap(),
                CommitReceipt::Durable
            );
        }
        let before = helper.snapshot.clone();
        let mutations = helper.mutations.len();
        if revoked {
            permit.revoke();
        } else {
            helper.unavailable = true;
        }
        // Any sealer call would be visible and return the configured failure.
        let hook = Sealer::new(KeymanagerStage::Generate);
        let result = save_with_detailed(
            &mut helper,
            &mut old,
            Namespace::OttState,
            &serde_json::json!({"profile":"replacement-profile"}),
            &permit,
            reject_seal,
            open,
        );
        assert_eq!(
            result,
            Err(if revoked {
                Error::Revoked
            } else {
                Error::Store(StoreError::HelperUnavailable)
            })
        );
        assert!(hook.calls().is_empty());
        assert_eq!(helper.mutations.len(), mutations);
        assert_eq!(helper.snapshot, before);
        assert_eq!(old.reads, 0);
        assert_eq!(old.erases, 0);
    }
}

#[test]
fn typed_save_owns_only_this_attempts_helper_failure() {
    let _serial = plx_base::testlock::serial();
    use crate::storage::wire::failure::{self, HelperFailure, Stage};
    let sealer = Sealer::new(KeymanagerStage::Generate);
    let mut legacy = Old::empty();
    let permit = WritePermit::new();
    for current_failure in [false, true] {
        failure::remember(Stage::Db8, Some(-999));
        let mut transport = |request| {
            assert!(matches!(request, Request::OttStatus {}));
            if current_failure {
                failure::remember(Stage::Db8, Some(-3963));
            }
            Err(client::ClientError::Unavailable)
        };
        let result = save_with_detailed(
            &mut transport,
            &mut legacy,
            Namespace::OttState,
            &serde_json::json!({"profile":"candidate"}),
            &permit,
            reject_seal,
            open,
        );
        // Later transactions cannot replace evidence already owned by the result.
        failure::remember(Stage::Unknown, Some(-7));
        let expected = if current_failure {
            Error::HelperFailed {
                source: StoreError::HelperUnavailable,
                failure: HelperFailure::new(Stage::Db8, Some(-3963)),
            }
        } else {
            Error::Store(StoreError::HelperUnavailable)
        };
        assert_eq!(result, Err(expected));
    }
    assert!(sealer.calls().is_empty());
    assert_eq!(legacy.reads, 0);
    assert_eq!(legacy.erases, 0);
    failure::clear();
}
