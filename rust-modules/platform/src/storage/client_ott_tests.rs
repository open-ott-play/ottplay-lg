use super::*;
use std::cell::Cell;
use wire::{ErrorCode, SecretString};

fn envelope() -> SecretString {
    SecretString(
        serde_json::json!({"version":1,"sealed":{
        "backend":"keymanager3","key":"com.openottplay.player.native.ott.state.v1",
        "iv":"AA==","data":"AA=="}})
        .to_string(),
    )
}
fn mutation() -> ott::Mutation {
    ott::Mutation::Initialize {
        control: None,
        state: Some(envelope()),
        barrier: ott::Barrier::Open,
    }
}
fn success(request: &Request) -> Response {
    let Request::OttCommit {
        operation_id,
        digest,
        ..
    } = request
    else {
        panic!("commit")
    };
    Response::OttCommit {
        status: CommitStatus::Committed,
        receipt: Some(ott::Receipt {
            operation_id: operation_id.clone(),
            digest: digest.clone(),
            revision: 1,
            epoch: 1,
            verified: true,
        }),
    }
}

#[test]
fn canonical_missing_is_distinct_from_transport_or_corruption() {
    for code in [
        ErrorCode::Unavailable,
        ErrorCode::Authentication,
        ErrorCode::Corrupt,
    ] {
        assert!(load_with(&mut |_| Ok(Response::Error { code })).is_err());
    }
    assert!(
        load_with(&mut |_| Ok(Response::OttLoaded { snapshot: None }))
            .unwrap()
            .is_none()
    );
}

#[test]
fn snapshot_rejects_inconsistent_presence_and_invalid_revisions() {
    let status = ott::Status {
        db_rev: "1".into(),
        revision: 1,
        epoch: 1,
        barrier: ott::Barrier::Open,
        control_present: false,
        state_present: true,
        private_initialized: true,
    };
    assert!(load_with(&mut |_| Ok(Response::OttLoaded {
        snapshot: Some(ott::Snapshot {
            status: status.clone(),
            control: None,
            state: None,
            public_preferences: None,
        })
    }))
    .is_err());
    let mut bad = status;
    bad.db_rev = "01".into();
    assert!(status_with(&mut |_| Ok(Response::OttStatus {
        status: Some(bad.clone())
    }))
    .is_err());
}

#[test]
fn revoked_permit_never_dispatches_a_commit() {
    let result = commit_with(
        &mut |_| panic!("no request"),
        ott::Expected::Missing {},
        Generation([1; 16]),
        mutation(),
        &|| false,
    )
    .unwrap();
    assert_eq!(result, Outcome::Revoked);
}

#[test]
fn lost_ack_reconciles_same_identity_without_repeating_an_applied_write() {
    let mut calls = 0;
    let mut original = None;
    let result = commit_with(
        &mut |request| {
            calls += 1;
            match request {
                Request::OttCommit {
                    operation_id,
                    digest,
                    ..
                } => {
                    original = Some((operation_id, digest));
                    Err(ClientError::Unavailable)
                }
                Request::OttReconcile {
                    operation_id,
                    digest,
                } => {
                    assert_eq!(Some((operation_id.clone(), digest.clone())), original);
                    Ok(Response::OttReconcile {
                        status: ReconcileStatus::Applied,
                        receipt: Some(ott::Receipt {
                            operation_id,
                            digest,
                            revision: 1,
                            epoch: 1,
                            verified: true,
                        }),
                    })
                }
                _ => panic!("unexpected request"),
            }
        },
        ott::Expected::Missing {},
        Generation([2; 16]),
        mutation(),
        &|| true,
    )
    .unwrap();
    assert_eq!(calls, 2);
    assert!(matches!(
        result,
        Outcome::Applied(ott::Receipt {
            verified: false,
            ..
        })
    ));
}

#[test]
fn only_not_applied_repeats_the_identical_commit() {
    let mut requests = Vec::new();
    let result = commit_with(
        &mut |request| {
            requests.push(serde_json::to_value(&request).unwrap());
            match requests.len() {
                1 => Err(ClientError::Unavailable),
                2 => Ok(Response::OttReconcile {
                    status: ReconcileStatus::NotApplied,
                    receipt: None,
                }),
                3 => Ok(success(&request)),
                _ => panic!("bounded retry"),
            }
        },
        ott::Expected::Missing {},
        Generation([3; 16]),
        mutation(),
        &|| true,
    )
    .unwrap();
    assert!(matches!(result, Outcome::Applied(_)));
    assert_eq!(requests[0], requests[2]);
}

#[test]
fn revocation_while_reconciling_prevents_retry() {
    let permit = Cell::new(true);
    let mut calls = 0;
    let result = commit_with(
        &mut |_| {
            calls += 1;
            if calls == 1 {
                Err(ClientError::Unavailable)
            } else {
                permit.set(false);
                Ok(Response::OttReconcile {
                    status: ReconcileStatus::NotApplied,
                    receipt: None,
                })
            }
        },
        ott::Expected::Missing {},
        Generation([4; 16]),
        mutation(),
        &|| permit.get(),
    )
    .unwrap();
    assert_eq!(result, Outcome::Revoked);
    assert_eq!(calls, 2);
}

#[test]
fn wrong_receipt_and_failed_reconcile_remain_uncertain() {
    let mut calls = 0;
    let result = commit_with(
        &mut |request| {
            calls += 1;
            if calls == 1 {
                let mut reply = success(&request);
                if let Response::OttCommit {
                    receipt: Some(receipt),
                    ..
                } = &mut reply
                {
                    receipt.digest = "0".repeat(64);
                }
                Ok(reply)
            } else {
                Err(ClientError::Protocol)
            }
        },
        ott::Expected::Missing {},
        Generation([5; 16]),
        mutation(),
        &|| true,
    )
    .unwrap();
    assert_eq!(result, Outcome::Uncertain);
    assert_eq!(calls, 2);
}

#[test]
fn conflict_does_not_retry_or_overwrite_the_newer_state() {
    let mut calls = 0;
    assert_eq!(
        commit_with(
            &mut |_| {
                calls += 1;
                Ok(Response::OttCommit {
                    status: CommitStatus::Conflict,
                    receipt: None,
                })
            },
            ott::Expected::Missing {},
            Generation([6; 16]),
            mutation(),
            &|| true
        )
        .unwrap(),
        Outcome::Conflict
    );
    assert_eq!(calls, 1);
}
