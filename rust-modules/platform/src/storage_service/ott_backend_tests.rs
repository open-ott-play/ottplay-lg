//! Real helper dispatch with an in-memory external DB8 boundary; no KMR/FS/TV IO.
use super::*;
use std::collections::BTreeMap;

#[derive(Default)]
struct Db {
    objects: BTreeMap<String, Value>,
    calls: Vec<(String, Value)>,
    lose_ack: bool,
    reject_put: bool,
    race_put: Option<Value>,
    fail_readback: bool,
    put_seen: bool,
}
impl Rpc for Db {
    fn call(&mut self, uri: &str, payload: &Value) -> Result<Value, ErrorCode> {
        self.calls.push((uri.to_owned(), payload.clone()));
        match uri {
            "luna://com.palm.db/get" => {
                if self.fail_readback && self.put_seen {
                    return Err(ErrorCode::Unavailable);
                }
                let id = payload["ids"][0].as_str().unwrap();
                Ok(
                    json!({"returnValue":true,"results": self.objects.get(id).cloned().into_iter().collect::<Vec<_>>()}),
                )
            }
            "luna://com.palm.db/put" => {
                self.put_seen = true;
                if let Some(winner) = self.race_put.take() {
                    self.objects
                        .insert(winner["_id"].as_str().unwrap().to_owned(), winner);
                }
                if self.reject_put {
                    return Ok(json!({"returnValue":false,"errorCode":-3961}));
                }
                let mut object = payload["objects"][0].clone();
                let id = object["_id"].as_str().unwrap().to_owned();
                let old = self.objects.get(&id).and_then(|v| v["_rev"].as_u64());
                if old != object["_rev"].as_u64() {
                    return Ok(json!({"returnValue":false,"errorCode":-3961}));
                }
                object["_rev"] = json!(old.unwrap_or(0) + 1);
                self.objects.insert(id, object);
                if self.lose_ack {
                    Err(ErrorCode::Timeout)
                } else {
                    Ok(json!({"returnValue":true}))
                }
            }
            _ => panic!("OTT persistence must use only fixed-ID DB8 get/put"),
        }
    }
}
fn backend() -> Backend<Db> {
    Backend::new(Db::default(), Flavor::Stable, "fixture.service".into())
}
fn envelope(namespace: ott::Namespace, data: &str) -> SecretString {
    let name = match namespace {
        ott::Namespace::Control => "control",
        ott::Namespace::State => "state",
    };
    SecretString(json!({"version":1,"sealed":{"backend":"keymanager3",
        "key":format!("com.openottplay.player.native.ott.{name}.v1"),"iv":"aXZpdmlyaXZpdmly","data":data}}).to_string())
}
fn request(expected: ott::Expected, n: u64, mutation: ott::Mutation) -> Request {
    let mut r = Request::OttCommit {
        expected,
        operation_id: format!("{n:032x}"),
        digest: String::new(),
        mutation,
    };
    let hash = state::digest_bytes(&wire::request_digest_bytes(&r).unwrap());
    if let Request::OttCommit { digest, .. } = &mut r {
        *digest = hash;
    }
    r
}
fn status(b: &mut Backend<Db>) -> ott::Status {
    match b.dispatch(Request::OttStatus {}) {
        Response::OttStatus { status: Some(s) } => s,
        other => panic!("{other:?}"),
    }
}
fn expected(b: &mut Backend<Db>) -> ott::Expected {
    let s = status(b);
    ott::Expected::Present {
        db_rev: s.db_rev,
        revision: s.revision,
        epoch: s.epoch,
    }
}
fn initialize(b: &mut Backend<Db>) -> Request {
    let r = request(
        ott::Expected::Missing {},
        1,
        ott::Mutation::Initialize {
            control: Some(envelope(ott::Namespace::Control, "Y29udHJvbA==")),
            state: Some(envelope(ott::Namespace::State, "c3RhdGU=")),
            barrier: ott::Barrier::Open,
        },
    );
    assert!(matches!(
        b.dispatch(r.clone()),
        Response::OttCommit {
            status: CommitStatus::Committed,
            receipt: Some(ott::Receipt { verified: true, .. })
        }
    ));
    r
}
fn snapshot(b: &mut Backend<Db>) -> ott::Snapshot {
    match b.dispatch(Request::OttLoad {}) {
        Response::OttLoaded { snapshot: Some(s) } => s,
        other => panic!("{other:?}"),
    }
}
fn puts(b: &Backend<Db>) -> usize {
    b.rpc
        .calls
        .iter()
        .filter(|(uri, _)| uri.ends_with("/put"))
        .count()
}

#[test]
fn ott_missing_is_distinct_from_unavailable_and_corrupt() {
    let mut b = backend();
    assert!(matches!(
        b.dispatch(Request::OttLoad {}),
        Response::OttLoaded { snapshot: None }
    ));
    b.rpc.fail_readback = true;
    b.rpc.put_seen = true;
    assert!(matches!(
        b.dispatch(Request::OttLoad {}),
        Response::Error {
            code: ErrorCode::Unavailable
        }
    ));
    b.rpc.fail_readback = false;
    b.rpc.objects.insert(
        "ottpriv.stable".into(),
        json!({"_id":"ottpriv.stable","_kind":"fixture.service:1","_rev":1,"state":"{}"}),
    );
    assert!(matches!(
        b.dispatch(Request::OttStatus {}),
        Response::Error {
            code: ErrorCode::Corrupt
        }
    ));
}
#[test]
fn ott_atomic_import_keeps_ciphertext_and_never_calls_keymanager() {
    let mut b = backend();
    initialize(&mut b);
    let s = snapshot(&mut b);
    assert_eq!(
        s.control,
        Some(envelope(ott::Namespace::Control, "Y29udHJvbA=="))
    );
    assert_eq!(s.state, Some(envelope(ott::Namespace::State, "c3RhdGU=")));
    assert_eq!(puts(&b), 1);
    assert!(b
        .rpc
        .calls
        .iter()
        .all(|(uri, _)| uri.starts_with("luna://com.palm.db/")));
    let again = request(
        ott::Expected::Missing {},
        2,
        ott::Mutation::Initialize {
            control: None,
            state: None,
            barrier: ott::Barrier::Open,
        },
    );
    assert!(matches!(
        b.dispatch(again),
        Response::OttCommit {
            status: CommitStatus::Conflict,
            ..
        }
    ));
    assert_eq!(puts(&b), 1);
}
#[test]
fn ott_put_preserves_other_ciphertext_and_requires_whole_snapshot_cas() {
    let mut b = backend();
    initialize(&mut b);
    let old = snapshot(&mut b);
    let e = expected(&mut b);
    let r = request(
        e.clone(),
        2,
        ott::Mutation::Put {
            namespace: ott::Namespace::State,
            value: envelope(ott::Namespace::State, "bmV3"),
        },
    );
    assert!(matches!(
        b.dispatch(r),
        Response::OttCommit {
            status: CommitStatus::Committed,
            ..
        }
    ));
    assert_eq!(snapshot(&mut b).control, old.control);
    let stale = request(
        e,
        3,
        ott::Mutation::Put {
            namespace: ott::Namespace::Control,
            value: envelope(ott::Namespace::Control, "bmV3"),
        },
    );
    assert!(matches!(
        b.dispatch(stale),
        Response::OttCommit {
            status: CommitStatus::Conflict,
            ..
        }
    ));
    assert_eq!(puts(&b), 2);
}
#[test]
fn ott_replay_uses_ledger_without_rewriting_or_claiming_current_verification() {
    let mut b = backend();
    let r = initialize(&mut b);
    assert!(matches!(
        b.dispatch(r.clone()),
        Response::OttCommit {
            status: CommitStatus::Committed,
            receipt: Some(ott::Receipt {
                verified: false,
                ..
            })
        }
    ));
    assert_eq!(puts(&b), 1);
    let Request::OttCommit {
        operation_id,
        digest,
        ..
    } = r
    else {
        unreachable!()
    };
    assert!(matches!(
        b.dispatch(Request::OttReconcile {
            operation_id: operation_id.clone(),
            digest
        }),
        Response::OttReconcile {
            status: ReconcileStatus::Applied,
            receipt: Some(ott::Receipt {
                verified: false,
                ..
            })
        }
    ));
    assert!(matches!(
        b.dispatch(Request::OttReconcile {
            operation_id,
            digest: "0".repeat(64)
        }),
        Response::Error {
            code: ErrorCode::Invalid
        }
    ));
}
#[test]
fn ott_lost_ack_is_verified_only_by_exact_readback() {
    let mut b = backend();
    b.rpc.lose_ack = true;
    initialize(&mut b);
    assert_eq!(puts(&b), 1);
    let e = expected(&mut b);
    b.rpc.fail_readback = true;
    b.rpc.put_seen = false;
    let r = request(
        e,
        2,
        ott::Mutation::Put {
            namespace: ott::Namespace::State,
            value: envelope(ott::Namespace::State, "bmV3"),
        },
    );
    assert!(matches!(
        b.dispatch(r.clone()),
        Response::OttCommit {
            status: CommitStatus::Unavailable,
            receipt: None
        }
    ));
    b.rpc.fail_readback = false;
    let Request::OttCommit {
        operation_id,
        digest,
        ..
    } = r
    else {
        unreachable!()
    };
    assert!(matches!(
        b.dispatch(Request::OttReconcile {
            operation_id,
            digest
        }),
        Response::OttReconcile {
            status: ReconcileStatus::Applied,
            ..
        }
    ));
}
#[test]
fn ott_rejected_put_does_not_change_existing_snapshot() {
    let mut b = backend();
    initialize(&mut b);
    let old = snapshot(&mut b);
    let e = expected(&mut b);
    b.rpc.reject_put = true;
    assert!(matches!(
        b.dispatch(request(e, 2, ott::Mutation::BeginErase {})),
        Response::OttCommit {
            status: CommitStatus::Unavailable,
            ..
        }
    ));
    assert_eq!(snapshot(&mut b), old);
}
#[test]
fn ott_erasure_barrier_blocks_reads_writes_and_prevents_aba() {
    let mut b = backend();
    initialize(&mut b);
    let stale = expected(&mut b);
    let e = expected(&mut b);
    assert!(matches!(
        b.dispatch(request(e, 2, ott::Mutation::BeginErase {})),
        Response::OttCommit {
            status: CommitStatus::Committed,
            ..
        }
    ));
    let pending = status(&mut b);
    assert_eq!(pending.barrier, ott::Barrier::Pending);
    assert_eq!(pending.epoch, 2);
    assert!(!pending.control_present && !pending.state_present);
    assert!(matches!(
        b.dispatch(Request::OttLoad {}),
        Response::Error {
            code: ErrorCode::Unavailable
        }
    ));
    let e = expected(&mut b);
    assert!(matches!(
        b.dispatch(request(
            e,
            3,
            ott::Mutation::Put {
                namespace: ott::Namespace::State,
                value: envelope(ott::Namespace::State, "bmV3")
            }
        )),
        Response::Error {
            code: ErrorCode::Invalid
        }
    ));
    let e = expected(&mut b);
    b.dispatch(request(e, 4, ott::Mutation::FinishErase {}));
    assert_eq!(status(&mut b).barrier, ott::Barrier::Complete);
    assert!(matches!(
        b.dispatch(request(
            stale,
            5,
            ott::Mutation::Put {
                namespace: ott::Namespace::State,
                value: envelope(ott::Namespace::State, "c3RhdGU=")
            }
        )),
        Response::OttCommit {
            status: CommitStatus::Conflict,
            ..
        }
    ));
    let e = expected(&mut b);
    b.dispatch(request(
        e,
        6,
        ott::Mutation::Put {
            namespace: ott::Namespace::State,
            value: envelope(ott::Namespace::State, "c3RhdGU="),
        },
    ));
    assert_eq!(status(&mut b).barrier, ott::Barrier::Complete);
    assert_eq!(status(&mut b).epoch, 2);
}
#[test]
fn ott_erase_missing_creates_tombstone_and_repeated_begin_keeps_epoch() {
    let mut b = backend();
    b.dispatch(request(
        ott::Expected::Missing {},
        1,
        ott::Mutation::BeginErase {},
    ));
    assert_eq!(status(&mut b).epoch, 1);
    let e = expected(&mut b);
    b.dispatch(request(e, 2, ott::Mutation::BeginErase {}));
    assert_eq!(status(&mut b).epoch, 1);
    assert_eq!(status(&mut b).revision, 2);
}
#[test]
fn ott_invalid_envelopes_scope_and_digest_never_write() {
    for value in [
        SecretString("plaintext".into()),
        envelope(ott::Namespace::Control, "c3RhdGU="),
        SecretString(
            envelope(ott::Namespace::State, "c3RhdGU=")
                .0
                .replace("player.native.ott", "player.native.debug.ott"),
        ),
        SecretString(
            envelope(ott::Namespace::State, "c3RhdGU=")
                .0
                .replace("keymanager3", "palm_keymanager"),
        ),
    ] {
        let mut b = backend();
        let r = request(
            ott::Expected::Missing {},
            1,
            ott::Mutation::Initialize {
                control: None,
                state: Some(value),
                barrier: ott::Barrier::Open,
            },
        );
        assert!(matches!(
            b.dispatch(r),
            Response::Error {
                code: ErrorCode::Invalid
            }
        ));
        assert_eq!(puts(&b), 0);
    }
    let mut b = backend();
    let mut r = request(ott::Expected::Missing {}, 1, ott::Mutation::BeginErase {});
    if let Request::OttCommit { digest, .. } = &mut r {
        *digest = "0".repeat(64);
    }
    assert!(matches!(
        b.dispatch(r),
        Response::Error {
            code: ErrorCode::Invalid
        }
    ));
    assert_eq!(puts(&b), 0);
}
#[test]
fn ott_namespaces_and_plex_use_separate_fixed_objects() {
    let mut b = backend();
    b.rpc.objects.insert(
        "ottstate.stable".into(),
        json!({"fixture":"plex state unchanged"}),
    );
    initialize(&mut b);
    let e = expected(&mut b);
    b.dispatch(request(e, 2, ott::Mutation::BeginErase {}));
    assert_eq!(
        b.rpc.objects["ottstate.stable"],
        json!({"fixture":"plex state unchanged"})
    );
    assert_eq!(model::object_id(Flavor::Stable), "ottpriv.stable");
    assert_eq!(model::object_id(Flavor::Debug), "ottpriv.debug");
    assert_eq!(model::object_id(Flavor::Nightly), "ottpriv.night");
}
#[test]
fn ott_pruned_or_missing_ledger_never_claims_not_applied() {
    let mut b = backend();
    let original = initialize(&mut b);
    for n in 2..=19 {
        let e = expected(&mut b);
        b.dispatch(request(
            e,
            n,
            ott::Mutation::Put {
                namespace: ott::Namespace::State,
                value: envelope(ott::Namespace::State, "c3RhdGU="),
            },
        ));
    }
    let Request::OttCommit {
        operation_id,
        digest,
        ..
    } = original
    else {
        unreachable!()
    };
    assert!(matches!(
        b.dispatch(Request::OttReconcile {
            operation_id,
            digest
        }),
        Response::OttReconcile {
            status: ReconcileStatus::Unknown,
            receipt: None
        }
    ));
}
#[test]
fn ott_wire_rejects_unknown_fields_plaintext_and_oversized_envelope() {
    assert!(
        serde_json::from_value::<Request>(json!({"type":"ott_status","owner":"other"})).is_err()
    );
    assert!(serde_json::from_value::<ott::Mutation>(
        json!({"type":"begin_erase","value":"secret"})
    )
    .is_err());
    assert!(ott::validate_envelope(&"x".repeat(ott::ENVELOPE_BYTES + 1)).is_err());
    let secret = envelope(ott::Namespace::Control, "Y29udHJvbA==");
    assert!(!format!("{secret:?}").contains("Y29udHJvbA"));
    let r = request(
        ott::Expected::Missing {},
        1,
        ott::Mutation::Initialize {
            control: Some(secret),
            state: None,
            barrier: ott::Barrier::Open,
        },
    );
    assert!(!format!("{r:?}").contains("Y29udHJvbA"));
}

#[test]
fn ott_two_maximal_envelopes_fit_atomic_request_and_readback_bounds() {
    let mut control = envelope(ott::Namespace::Control, "AAAA");
    let mut state_value = envelope(ott::Namespace::State, "AAAA");
    for value in [&mut control, &mut state_value] {
        let base = value.0.len() - 4;
        let data = "A".repeat(((ott::ENVELOPE_BYTES - base) / 4) * 4);
        value.0 = value.0.replace("\"AAAA\"", &format!("\"{data}\""));
        assert!(value.0.len() <= ott::ENVELOPE_BYTES);
        assert!(value.0.len() >= ott::ENVELOPE_BYTES - 3);
        assert!(ott::validate_envelope(&value.0).is_ok());
        let too_large = value.0.replacen("\"data\":\"", "\"data\":\"AAAA", 1);
        assert!(ott::validate_envelope(&too_large).is_err());
    }
    let mut b = backend();
    let r = request(
        ott::Expected::Missing {},
        1,
        ott::Mutation::Initialize {
            control: Some(control.clone()),
            state: Some(state_value.clone()),
            barrier: ott::Barrier::Open,
        },
    );
    assert!(wire::check_frame_size(&r).is_ok());
    assert!(matches!(
        b.dispatch(r),
        Response::OttCommit {
            status: CommitStatus::Committed,
            ..
        }
    ));
    let loaded = snapshot(&mut b);
    assert_eq!(loaded.control, Some(control));
    assert_eq!(loaded.state, Some(state_value));
    assert!(wire::check_frame_size(&loaded).is_ok());
}

#[test]
fn ott_pending_initialization_cannot_smuggle_records_or_clear_existing_barrier() {
    let mut b = backend();
    let r = request(
        ott::Expected::Missing {},
        1,
        ott::Mutation::Initialize {
            control: Some(envelope(ott::Namespace::Control, "AAAA")),
            state: None,
            barrier: ott::Barrier::Pending,
        },
    );
    assert!(matches!(
        b.dispatch(r),
        Response::Error {
            code: ErrorCode::Invalid
        }
    ));
    assert_eq!(puts(&b), 0);
    let pending = request(
        ott::Expected::Missing {},
        2,
        ott::Mutation::Initialize {
            control: None,
            state: None,
            barrier: ott::Barrier::Pending,
        },
    );
    assert!(matches!(
        b.dispatch(pending),
        Response::OttCommit {
            status: CommitStatus::Committed,
            ..
        }
    ));
    let e = expected(&mut b);
    let reset = request(
        e,
        3,
        ott::Mutation::Initialize {
            control: None,
            state: None,
            barrier: ott::Barrier::Open,
        },
    );
    assert!(matches!(
        b.dispatch(reset),
        Response::Error {
            code: ErrorCode::Invalid
        }
    ));
    assert_eq!(status(&mut b).barrier, ott::Barrier::Pending);
}

#[test]
fn ott_concurrent_put_conflict_requires_authoritative_changed_snapshot() {
    let mut b = backend();
    initialize(&mut b);
    let old = expected(&mut b);
    let mut winner = backend();
    winner.rpc.objects = b.rpc.objects.clone();
    let winning = request(
        old.clone(),
        2,
        ott::Mutation::Put {
            namespace: ott::Namespace::Control,
            value: envelope(ott::Namespace::Control, "bmV3"),
        },
    );
    assert!(matches!(
        winner.dispatch(winning),
        Response::OttCommit {
            status: CommitStatus::Committed,
            ..
        }
    ));
    b.rpc.race_put = Some(winner.rpc.objects["ottpriv.stable"].clone());
    let losing = request(
        old,
        3,
        ott::Mutation::Put {
            namespace: ott::Namespace::State,
            value: envelope(ott::Namespace::State, "bmV3"),
        },
    );
    assert!(matches!(
        b.dispatch(losing),
        Response::OttCommit {
            status: CommitStatus::Conflict,
            receipt: None
        }
    ));
    assert_eq!(snapshot(&mut b), snapshot(&mut winner));
}

#[test]
fn ott_noncompact_envelopes_obey_separate_encoded_image_limit_before_write() {
    let mut control = envelope(ott::Namespace::Control, "AAAA");
    let mut state_value = envelope(ott::Namespace::State, "AAAA");
    for value in [&mut control, &mut state_value] {
        value.0 = "\n".repeat(ott::ENVELOPE_BYTES - value.0.len()) + &value.0;
        assert_eq!(value.0.len(), ott::ENVELOPE_BYTES);
        assert!(ott::validate_envelope(&value.0).is_ok());
    }
    let r = request(
        ott::Expected::Missing {},
        1,
        ott::Mutation::Initialize {
            control: Some(control),
            state: Some(state_value),
            barrier: ott::Barrier::Open,
        },
    );
    assert!(wire::check_frame_size(&r).is_ok());
    let mut b = backend();
    assert!(matches!(
        b.dispatch(r),
        Response::Error {
            code: ErrorCode::Invalid
        }
    ));
    assert_eq!(puts(&b), 0);
    assert!(b.rpc.objects.is_empty());
}

#[test]
fn ott_public_patch_fitting_put_but_not_get_is_rejected_before_publication() {
    fn prepared(control: SecretString, state_value: SecretString) -> (State, Request) {
        let initial = request(ott::Expected::Missing {}, 1, ott::Mutation::Initialize {
            control:Some(control), state:Some(state_value), barrier:ott::Barrier::Open,
        });
        let Request::OttCommit { operation_id, digest, mutation, .. } = initial else {
            panic!("expected initialization");
        };
        let stored = State::candidate(None, Flavor::Stable, mutation, &operation_id, &digest).unwrap();
        let change = request(ott::Expected::Present { db_rev:"9".into(),
            revision:stored.revision, epoch:stored.epoch }, 2,
            ott::Mutation::PatchPublic { patch:ott::PublicPatch::InterfaceMode(1) });
        (stored, change)
    }
    fn candidate_object(stored: &State, request: &Request) -> Value {
        let Request::OttCommit { operation_id, digest, mutation, .. } = request else {
            panic!("expected OTT mutation");
        };
        let candidate = State::candidate(Some(stored), Flavor::Stable,
            mutation.clone(), operation_id, digest).unwrap();
        json!({"_id":"ottpriv.stable", "_kind":"fixture.service:1", "_rev":9,
            "state":candidate.encode().unwrap()})
    }
    // Exercise both a near-boundary accepted update and the PUT-only false positive.
    for reserve in [128, 0] {
        let mut control = envelope(ott::Namespace::Control, "AAAA");
        let mut state_value = envelope(ott::Namespace::State, "AAAA");
        let (small, change) = prepared(control.clone(), state_value.clone());
        let overhead = serde_json::to_vec(&json!({"objects":[candidate_object(&small, &change)]})).unwrap().len();
        let padding = wire::MAX_FRAME - reserve - overhead;
        // Each newline occupies three bytes after the envelope's State JSON
        // string is nested in the DB8 object. Spaces fill the exact remainder.
        let newlines = padding / 3;
        control.0 = "\n".repeat(newlines / 2) + &" ".repeat(padding % 3) + &control.0;
        state_value.0 = "\n".repeat(newlines - newlines / 2) + &state_value.0;
        for value in [&control, &state_value] {
            assert!(value.0.len() <= ott::ENVELOPE_BYTES);
            assert!(ott::validate_envelope(&value.0).is_ok());
        }
        let (stored, change) = prepared(control, state_value);
        assert!(wire::check_frame_size(&change).is_ok()); // Tiny metadata request.
        let object = candidate_object(&stored, &change);
        let put = json!({"objects":[&object]});
        assert_eq!(serde_json::to_vec(&put).unwrap().len(), wire::MAX_FRAME - reserve);
        assert!(wire::check_frame_size(&put).is_ok());
        let mut get_object = object;
        get_object["_rev"] = json!(u64::MAX);
        let get_fits = wire::check_frame_size(&json!({"returnValue":true,"results":[get_object]})).is_ok();
        assert_eq!(get_fits, reserve != 0);

        let mut b = backend();
        let original = json!({"_id":"ottpriv.stable", "_kind":"fixture.service:1", "_rev":9,
            "state":stored.encode().unwrap()});
        // The original record is valid and readable before the metadata change.
        assert!(wire::check_frame_size(&json!({"returnValue":true,"results":[&original]})).is_ok());
        b.rpc.objects.insert("ottpriv.stable".into(), original.clone());
        let result = b.dispatch(change);
        if reserve == 0 {
            assert!(matches!(result, Response::Error { code:ErrorCode::Invalid }));
            assert_eq!(puts(&b), 0);
            assert_eq!(b.rpc.objects.get("ottpriv.stable"), Some(&original));
        } else {
            assert!(matches!(result, Response::OttCommit { status:CommitStatus::Committed,
                receipt:Some(ott::Receipt { verified:true, .. }) }));
            assert_eq!(puts(&b), 1);
            let sent = b.rpc.calls.iter().find(|(uri,_)| uri.ends_with("/put")).unwrap();
            assert_eq!(sent.1["objects"][0]["_rev"], 9);
            assert_eq!(b.rpc.objects["ottpriv.stable"]["_rev"], 10);
        }
    }
}

#[test]
fn ott_shared_put_guard_reserves_revision_width_without_changing_cas() {
    for revision in [None, Some(9_u64), Some(999_999)] {
        let mut object = json!({"_id":"ottpriv.stable", "_kind":"fixture.service:1", "state":"{}"});
        if let Some(revision) = revision { object["_rev"] = json!(revision); }
        assert_eq!(checked_put_payload(object.clone()).unwrap(), json!({"objects":[&object]}));
        let overhead = serde_json::to_vec(&json!({"objects":[&object]})).unwrap().len();
        object["state"] = json!("x".repeat(wire::MAX_FRAME - overhead + 2));
        let put = json!({"objects":[&object]});
        assert_eq!(serde_json::to_vec(&put).unwrap().len(), wire::MAX_FRAME);
        assert!(wire::check_frame_size(&put).is_ok());
        assert!(matches!(checked_put_payload(object), Err(ErrorCode::Invalid)));
    }
}

#[test]
fn ott_completed_erasure_import_preserves_explicitly_reconfigured_records() {
    let mut b = backend();
    let control = envelope(ott::Namespace::Control, "bmV3");
    let state_value = envelope(ott::Namespace::State, "bmV3");
    let r = request(
        ott::Expected::Missing {},
        1,
        ott::Mutation::Initialize {
            control: Some(control.clone()),
            state: Some(state_value.clone()),
            barrier: ott::Barrier::Complete,
        },
    );
    assert!(matches!(
        b.dispatch(r),
        Response::OttCommit {
            status: CommitStatus::Committed,
            receipt: Some(ott::Receipt { verified: true, .. })
        }
    ));
    let loaded = snapshot(&mut b);
    assert_eq!(loaded.status.barrier, ott::Barrier::Complete);
    assert_eq!(loaded.control, Some(control));
    assert_eq!(loaded.state, Some(state_value));
    let e = expected(&mut b);
    assert!(matches!(
        b.dispatch(request(e, 2, ott::Mutation::BeginErase {})),
        Response::OttCommit {
            status: CommitStatus::Committed,
            ..
        }
    ));
    let erased = status(&mut b);
    assert_eq!(erased.barrier, ott::Barrier::Pending);
    assert!(!erased.control_present && !erased.state_present);
}

#[test]
fn ott_empty_struct_variants_reject_extra_fields_and_preserve_wire_json() {
    for (kind, mutation) in [
        ("begin_erase", ott::Mutation::BeginErase {}),
        ("finish_erase", ott::Mutation::FinishErase {}),
    ] {
        let json = json!({"type":kind});
        assert_eq!(serde_json::to_value(&mutation).unwrap(), json);
        assert_eq!(
            serde_json::from_value::<ott::Mutation>(json).unwrap(),
            mutation
        );
        assert!(
            serde_json::from_value::<ott::Mutation>(json!({"type":kind,"value":"secret"})).is_err()
        );
    }
    assert_eq!(
        serde_json::to_value(ott::Expected::Missing {}).unwrap(),
        json!({"type":"missing"})
    );
    assert_eq!(
        serde_json::from_value::<ott::Expected>(json!({"type":"missing"})).unwrap(),
        ott::Expected::Missing {}
    );
    assert!(
        serde_json::from_value::<ott::Expected>(json!({"type":"missing","db_rev":"1"})).is_err()
    );
    for (expected, mutation) in [
        (
            json!({"type":"missing","db_rev":"1"}),
            json!({"type":"begin_erase"}),
        ),
        (
            json!({"type":"missing"}),
            json!({"type":"begin_erase","value":"secret"}),
        ),
        (
            json!({"type":"missing"}),
            json!({"type":"finish_erase","value":"secret"}),
        ),
    ] {
        assert!(serde_json::from_value::<Request>(json!({"type":"ott_commit","expected":expected,
            "operation_id":"00000000000000000000000000000001","digest":"0".repeat(64),"mutation":mutation})).is_err());
    }
}

#[test]
fn public_preferences_patch_preserves_other_fields_and_exact_ciphertext() {
    let mut b = backend();
    initialize(&mut b);
    let original = snapshot(&mut b);
    for (n, patch) in [
        (2, ott::PublicPatch::RemoteAddress("https://control.invalid:443/team".into())),
        (3, ott::PublicPatch::EnsureDeviceId("existing-device".into())),
        (4, ott::PublicPatch::InterfaceMode(0)),
        (5, ott::PublicPatch::EnsureDeviceId("must-not-replace".into())),
    ] {
        let e = expected(&mut b);
        assert!(matches!(b.dispatch(request(e, n, ott::Mutation::PatchPublic { patch })),
            Response::OttCommit { status: CommitStatus::Committed,
                receipt: Some(ott::Receipt { verified: true, .. }) }));
    }
    let after = snapshot(&mut b);
    assert_eq!(after.control, original.control);
    assert_eq!(after.state, original.state);
    assert!(after.status.private_initialized);
    let prefs = after.public_preferences.unwrap();
    assert_eq!(prefs.device_id.as_deref(), Some("existing-device"));
    assert_eq!(prefs.remote_address.as_deref(), Some("https://control.invalid:443/team"));
    assert_eq!(prefs.interface_mode, Some(0));
    assert_eq!(puts(&b), 5, "even existing identity receives a fresh confirmed commit");
}

#[test]
fn public_only_initialization_keeps_private_import_pending_until_exact_import() {
    let mut b = backend();
    assert!(matches!(b.dispatch(request(ott::Expected::Missing {}, 1,
        ott::Mutation::PatchPublic { patch: ott::PublicPatch::InterfaceMode(0) })),
        Response::OttCommit { status: CommitStatus::Committed, .. }));
    let public = snapshot(&mut b);
    assert!(!public.status.private_initialized);
    assert!(public.control.is_none() && public.state.is_none());
    let control = envelope(ott::Namespace::Control, "b2xk");
    let state = envelope(ott::Namespace::State, "b2xk");
    let e = expected(&mut b);
    let put = b.dispatch(request(e, 2, ott::Mutation::Put {
        namespace: ott::Namespace::Control, value: control.clone(),
    }));
    assert!(matches!(put, Response::Error { code: ErrorCode::Invalid }));
    assert_eq!(puts(&b), 1, "ordinary private Put cannot bypass import");
    let e = expected(&mut b);
    assert!(matches!(b.dispatch(request(e, 3, ott::Mutation::CompletePrivateImport {
        control: Some(control.clone()), state: Some(state.clone()), barrier: ott::Barrier::Open,
    })), Response::OttCommit { status: CommitStatus::Committed, .. }));
    let after = snapshot(&mut b);
    assert!(after.status.private_initialized);
    assert_eq!(after.control, Some(control));
    assert_eq!(after.state, Some(state));
    assert_eq!(after.public_preferences, public.public_preferences);
}

#[test]
fn public_preferences_erase_clears_slot_and_rejects_stale_cas() {
    let mut b = backend();
    b.dispatch(request(ott::Expected::Missing {}, 1, ott::Mutation::PatchPublic {
        patch: ott::PublicPatch::EnsureDeviceId("device-before-erase".into()),
    }));
    let stale = expected(&mut b);
    let e = expected(&mut b);
    b.dispatch(request(e, 2, ott::Mutation::BeginErase {}));
    let s = status(&mut b);
    assert!(s.private_initialized);
    assert_eq!(s.barrier, ott::Barrier::Pending);
    let record = load(&mut b).unwrap().unwrap().1;
    assert!(record.public_preferences.is_none() && record.control.is_none() && record.state.is_none());
    assert!(matches!(b.dispatch(request(stale, 3, ott::Mutation::PatchPublic {
        patch: ott::PublicPatch::InterfaceMode(1),
    })), Response::OttCommit { status: CommitStatus::Conflict, .. }));
    let e = expected(&mut b);
    assert!(matches!(b.dispatch(request(e, 4, ott::Mutation::PatchPublic {
        patch: ott::PublicPatch::InterfaceMode(1),
    })), Response::Error { code: ErrorCode::Invalid }));
    assert_eq!(puts(&b), 2);
}

#[test]
fn old_private_state_without_public_fields_remains_initialized_and_byte_preserved() {
    let mut b = backend();
    initialize(&mut b);
    let original = snapshot(&mut b);
    let object = b.rpc.objects.get_mut("ottpriv.stable").unwrap();
    let mut text: Value = serde_json::from_str(object["state"].as_str().unwrap()).unwrap();
    text.as_object_mut().unwrap().remove("private_initialized");
    text.as_object_mut().unwrap().remove("public_preferences");
    object["state"] = json!(serde_json::to_string(&text).unwrap());
    let old = snapshot(&mut b);
    assert!(old.status.private_initialized);
    assert_eq!(old.control, original.control);
    assert_eq!(old.state, original.state);
    assert_eq!(puts(&b), 1, "read never rewrites old snapshots");
}

#[test]
fn public_preferences_reject_secrets_invalid_urls_and_non_modes_before_db8_put() {
    for value in [
        json!({"version":1,"token":"do-not-store"}),
        json!({"version":1,"profiles":[]}),
        json!({"version":1,"nonce":"do-not-store"}),
    ] { assert!(serde_json::from_value::<ott::PublicPreferences>(value).is_err()); }
    for patch in [
        ott::PublicPatch::RemoteAddress("https://user:password@control.invalid:443".into()),
        ott::PublicPatch::RemoteAddress("https://control.invalid:443/?token=secret".into()),
        ott::PublicPatch::RemoteAddress("https://control.invalid:443/#secret".into()),
        ott::PublicPatch::RemoteAddress("https://control.invalid:443/%73ecret".into()),
        ott::PublicPatch::RemoteAddress("https://control.invalid:443/../escape".into()),
        ott::PublicPatch::RemoteAddress("http://127.0.0.1:80".into()),
        ott::PublicPatch::RemoteAddress("https://control.invalid:443/\n".into()),
        ott::PublicPatch::InterfaceMode(2),
        ott::PublicPatch::EnsureDeviceId(String::new()),
    ] {
        let mut b = backend();
        assert!(matches!(b.dispatch(request(ott::Expected::Missing {}, 1,
            ott::Mutation::PatchPublic { patch })), Response::Error { code: ErrorCode::Invalid }));
        assert_eq!(puts(&b), 0);
    }
}
