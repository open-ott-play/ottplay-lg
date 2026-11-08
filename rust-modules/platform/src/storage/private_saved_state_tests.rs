//! Client contract fixtures only; no service, filesystem, DB8 or keymanager is used.
use super::*;
use serde_json::json;

fn value() -> Value {
    json!({"version":1,"settings":{"nativeCompanion":{"enabled":true,"cacheBytes":268435456},
        "nativeCompanionRevision":2,"privateFixture":"retained"},"receipts":[]})
}
fn status(revision: u64) -> ott::Status {
    ott::Status { db_rev: revision.to_string(), revision, epoch: 1, barrier: ott::Barrier::Open,
        control_present: false, state_present: true, private_initialized: true }
}
fn protected(revision: u64) -> Response {
    Response::OttStateLoaded { status: Some(status(revision)), value: saved::Loaded::Protected {
        value: document(&value()).unwrap(), envelope_sha256: "b".repeat(64),
    } }
}
fn legacy() -> wire::SecretString {
    wire::SecretString(json!({"version":1,"sealed":{
        "backend":"keymanager3", "key":Namespace::OttState.key_name(),
        "iv":"AQ==", "data":"Ag=="
    }}).to_string())
}
fn refuse_open(_: Namespace, _: &Sealed) -> Option<Vec<u8>> { None }
fn open_legacy(namespace: Namespace, _: &Sealed) -> Option<Vec<u8>> {
    assert!(matches!(namespace, Namespace::OttState));
    super::super::encode_plain(namespace, &value()).ok()
}

#[test]
fn locked_legacy_prevents_commit_and_is_not_absence() {
    let mut calls = 0;
    let mut transport = |request| {
        calls += 1;
        assert!(matches!(request, Request::OttStateLoad {}));
        Ok(Response::OttStateLoaded { status: Some(status(2)),
            value: saved::Loaded::Legacy { envelope: legacy() } })
    };
    assert_eq!(save_prepared(&mut transport, &document(&value()).unwrap(), &WritePermit::new(),
        Generation([1; 16]), refuse_open), Err(Error::ProtectionUnavailable));
    assert_eq!(calls, 1);
}

#[test]
fn legacy_commit_binds_exact_envelope_and_requires_current_readback() {
    for mismatch in 0..5 {
        let mut calls = 0;
        let mut transport = |request: Request| {
            calls += 1;
            let digest_check = if matches!(&request, Request::OttStateCommit { .. }) {
                Some(state::digest_bytes(&wire::request_digest_bytes(&request).unwrap()))
            } else { None };
            match request {
                Request::OttStateLoad {} if calls == 1 => Ok(Response::OttStateLoaded {
                    status: Some(status(2)), value: saved::Loaded::Legacy { envelope: legacy() },
                }),
                Request::OttStateCommit { expected, operation_id, digest, source, value: committed } => {
                    assert_eq!(expected, client::ott::expected(Some(&status(2))));
                    assert_eq!(source, saved::Source::Legacy { sha256: state::digest_bytes(legacy().0.as_bytes()) });
                    assert_eq!(Some(digest.clone()), digest_check);
                    assert_eq!(committed.as_ref(), &value());
                    Ok(Response::OttStateCommit { status: CommitStatus::Committed,
                        receipt: Some(ott::Receipt { operation_id, digest, revision: 3, epoch: 1, verified: false }) })
                }
                Request::OttStateLoad {} => {
                    let mut current = status(if mismatch == 1 { 4 } else { 3 });
                    if mismatch == 2 { current.epoch = 2; }
                    let mut found = value();
                    if mismatch == 3 { found["settings"]["nativeCompanionRevision"] = json!(3); }
                    let loaded = if mismatch == 4 { saved::Loaded::Legacy { envelope: legacy() } }
                        else { saved::Loaded::Protected { value: document(&found).unwrap(), envelope_sha256: "c".repeat(64) } };
                    Ok(Response::OttStateLoaded { status: Some(current), value: loaded })
                }
                _ => panic!("unrelated transaction"),
            }
        };
        assert_eq!(save_prepared(&mut transport, &document(&value()).unwrap(), &WritePermit::new(),
            Generation([1; 16]), open_legacy).unwrap(),
            if mismatch == 0 { CommitReceipt::Durable } else { uncertain() });
        assert_eq!(calls, 3);
    }
}

#[test]
fn lost_commit_reply_is_uncertain_without_replay_or_fallback() {
    let mut calls = 0;
    let mut transport = |request| {
        calls += 1;
        match request {
            Request::OttStateLoad {} if calls == 1 => Ok(protected(2)),
            Request::OttStateCommit { .. } => Err(client::ClientError::Unavailable),
            _ => panic!("uncertain write must not be retried"),
        }
    };
    assert_eq!(save_prepared(&mut transport, &document(&value()).unwrap(), &WritePermit::new(),
        Generation([1; 16]), refuse_open).unwrap(), uncertain());
    assert_eq!(calls, 2);
}

#[test]
fn prewrite_keymanager_failure_keeps_the_exact_typed_diagnostic() {
    let failure = wire::KeymanagerFailure { operation: wire::KeymanagerOperation::Seal,
        stage: wire::KeymanagerStage::Generate, code: wire::ErrorCode::Capability,
        category: wire::KeymanagerFailureCategory::ServiceRejected, service_code: Some(-1000), hub_error: Some(false) };
    let mut calls = 0;
    let mut transport = |request| {
        calls += 1;
        match request {
            Request::OttStateLoad {} => Ok(protected(2)),
            Request::OttStateCommit { .. } => Ok(Response::KeymanagerError {
                failure, preservation: wire::AuthPreservation::Unchanged, db8_commit_verified: false,
            }),
            _ => panic!("unrelated transaction"),
        }
    };
    assert_eq!(save_prepared(&mut transport, &document(&value()).unwrap(), &WritePermit::new(),
        Generation([1; 16]), refuse_open), Err(Error::ProtectionFailed(failure)));
    assert_eq!(calls, 2);
}

#[test]
fn revocation_after_observation_prevents_dispatch() {
    let permit = WritePermit::new();
    let mut calls = 0;
    let mut transport = |request| {
        calls += 1;
        assert!(matches!(request, Request::OttStateLoad {}));
        permit.revoke();
        Ok(protected(2))
    };
    assert_eq!(save_prepared(&mut transport, &document(&value()).unwrap(), &permit,
        Generation([1; 16]), refuse_open), Err(Error::Revoked));
    assert_eq!(calls, 1);
}

#[test]
fn inspection_preserves_exact_envelope_hash_without_import_or_commit() {
    for legacy_value in [false, true] {
        let mut calls = 0;
        let mut transport = |request| {
            calls += 1;
            assert!(matches!(request, Request::OttStateLoad {}));
            Ok(if legacy_value { Response::OttStateLoaded { status: Some(status(2)),
                value: saved::Loaded::Legacy { envelope: legacy() } } } else { protected(2) })
        };
        let found = inspect_with(&mut transport,
            || panic!("canonical state must not probe legacy files"), open_legacy).unwrap().unwrap();
        assert_eq!(found.value(), &value());
        assert_eq!(found.namespace_sha256(), if legacy_value {
            state::digest_bytes(legacy().0.as_bytes())
        } else { "b".repeat(64) });
        assert_eq!(calls, 1);
    }
}

#[test]
fn inspection_distinguishes_missing_from_pending_legacy_import() {
    let mut calls = 0;
    let mut transport = |request| {
        calls += 1;
        assert!(matches!(request, Request::OttStateLoad {}));
        Ok(Response::OttStateLoaded { status: None, value: saved::Loaded::Missing {} })
    };
    assert!(inspect_with(&mut transport, || Ok(false), refuse_open).unwrap().is_none());
    assert!(matches!(inspect_with(&mut transport, || Ok(true), refuse_open), Err(InspectError::LegacyOnly)));
    assert_eq!(calls, 2);
    let mut transport = |request| {
        assert!(matches!(request, Request::OttStateLoad {}));
        let mut pending = status(2); pending.state_present = false; pending.private_initialized = false;
        Ok(Response::OttStateLoaded { status: Some(pending), value: saved::Loaded::Missing {} })
    };
    assert!(matches!(inspect_with(&mut transport, || panic!("no legacy read"), refuse_open), Err(InspectError::LegacyOnly)));
}

#[test]
fn document_rejects_wrong_schema_and_preserves_existing_wrapped_size_limit() {
    assert_eq!(validate_document(&json!({"version":2})), Err(Error::Invalid));
    assert_eq!(validate_document(&json!({"unknown":"secret"})), Err(Error::Invalid));
    assert_eq!(validate_document(&json!({"settings":{"large":"x".repeat(super::super::LOGICAL_BYTES)}})), Err(Error::TooLarge));
}
