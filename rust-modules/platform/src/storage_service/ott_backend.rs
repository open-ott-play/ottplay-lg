//! Fixed-ID OTT DB8 adapter. Only typed Control and State lanes own helper encryption.
use super::super::wire::{
    self, ott, CommitStatus, ErrorCode, ReconcileStatus, Request, Response, SecretString,
};
use super::{db8_failure, exact_revision, record_put_failure, state, Backend, Flavor, Rpc};
use serde_json::{json, Value};
#[path = "ott_state.rs"]
mod model;
#[path = "ott_control_backend.rs"]
mod protected_control;
#[path = "ott_saved_backend.rs"]
mod protected_saved;
use model::State;

fn load<R: Rpc>(backend: &mut Backend<R>) -> Result<Option<(u64, State)>, ErrorCode> {
    let reply = backend.rpc.call(
        "luna://com.palm.db/get",
        &json!({"ids":[model::object_id(backend.flavor)]}),
    )?;
    if reply["returnValue"] != true {
        return Err(db8_failure(&reply));
    }
    let objects = reply["results"].as_array().ok_or(ErrorCode::Corrupt)?;
    if objects.is_empty() {
        return Ok(None);
    }
    if objects.len() != 1 {
        return Err(ErrorCode::Corrupt);
    }
    let object = objects[0].as_object().ok_or(ErrorCode::Corrupt)?;
    if object.len() != 4
        || object.get("_id") != Some(&json!(model::object_id(backend.flavor)))
        || object.get("_kind") != Some(&json!(format!("{}:1", backend.service)))
    {
        return Err(ErrorCode::Corrupt);
    }
    let rev = object
        .get("_rev")
        .and_then(Value::as_u64)
        .filter(|rev| *rev > 0)
        .ok_or(ErrorCode::Corrupt)?;
    let text = object
        .get("state")
        .and_then(Value::as_str)
        .ok_or(ErrorCode::Corrupt)?;
    Ok(Some((rev, State::decode(text, backend.flavor)?)))
}
fn expected_matches(expected: &ott::Expected, current: Option<&(u64, State)>) -> bool {
    match (expected, current) {
        (ott::Expected::Missing {}, None) => true,
        (
            ott::Expected::Present {
                db_rev,
                revision,
                epoch,
            },
            Some((actual, state)),
        ) => {
            exact_revision(db_rev) == Ok(*actual)
                && *revision == state.revision
                && *epoch == state.epoch
        }
        _ => false,
    }
}
/// Keep every accepted OTT candidate readable through the same bounded DB8 bus.
/// DB8 adds _rev on create and may increase its width on update. Reserve the
/// largest revision only in the GET image; never alter the caller's CAS value.
fn checked_put_payload(object: Value) -> Result<Value, ErrorCode> {
    let mut readback = object.clone();
    readback["_rev"] = json!(u64::MAX);
    wire::check_frame_size(&json!({"returnValue":true,"results":[readback]}))?;
    let payload = json!({"objects":[object]});
    wire::check_frame_size(&payload)?;
    Ok(payload)
}

fn unavailable() -> Response {
    Response::OttCommit {
        status: CommitStatus::Unavailable,
        receipt: None,
    }
}

pub(super) fn dispatch<R: Rpc>(
    backend: &mut Backend<R>,
    request: Request,
) -> Result<Response, ErrorCode> {
    match request {
        request @ (Request::OttStateLoad {} | Request::OttStateCommit { .. }) => {
            protected_saved::dispatch(backend, request)
        }
        request @ (Request::OttControlLoad {} | Request::OttControlCommit { .. }) => {
            protected_control::dispatch(backend, request)
        }
        Request::OttStatus {} => Ok(Response::OttStatus {
            status: load(backend)?.map(|(rev, state)| state.status(rev)),
        }),
        Request::OttLoad {} => {
            let current = load(backend)?;
            if current
                .as_ref()
                .is_some_and(|(_, state)| state.barrier == ott::Barrier::Pending)
            {
                return Err(ErrorCode::Unavailable);
            }
            Ok(Response::OttLoaded {
                snapshot: current.map(|(rev, state)| ott::Snapshot {
                    status: state.status(rev),
                    control: state.control.map(SecretString),
                    state: state.state.map(SecretString),
                    public_preferences: state.public_preferences,
                }),
            })
        }
        Request::OttReconcile {
            operation_id,
            digest,
        } => {
            if !model::valid_hex(&operation_id, 32) || !model::valid_hex(&digest, 64) {
                return Err(ErrorCode::Invalid);
            }
            let receipt = match load(backend)? {
                Some((_, state)) => state.receipt(&operation_id, &digest, false)?,
                None => None,
            };
            // Missing may mean pruned history or an uncertain in-flight write, not NotApplied.
            Ok(Response::OttReconcile {
                status: if receipt.is_some() {
                    ReconcileStatus::Applied
                } else {
                    ReconcileStatus::Unknown
                },
                receipt,
            })
        }
        request @ Request::OttCommit { .. } => commit(backend, request),
        _ => Err(ErrorCode::Protocol),
    }
}
fn commit<R: Rpc>(backend: &mut Backend<R>, request: Request) -> Result<Response, ErrorCode> {
    let bytes = wire::request_digest_bytes(&request)?;
    let Request::OttCommit {
        expected,
        operation_id,
        digest,
        mutation,
    } = request
    else {
        return Err(ErrorCode::Protocol);
    };
    if !model::valid_hex(&operation_id, 32)
        || !model::valid_hex(&digest, 64)
        || state::digest_bytes(&bytes) != digest
    {
        return Err(ErrorCode::Invalid);
    }
    let current = load(backend)?;
    if let Some((_, state)) = &current {
        if let Some(receipt) = state.receipt(&operation_id, &digest, false)? {
            return Ok(Response::OttCommit {
                status: CommitStatus::Committed,
                receipt: Some(receipt),
            });
        }
    }
    if !expected_matches(&expected, current.as_ref()) {
        return Ok(Response::OttCommit {
            status: CommitStatus::Conflict,
            receipt: None,
        });
    }
    let candidate = State::candidate(
        current.as_ref().map(|(_, state)| state),
        backend.flavor,
        mutation,
        &operation_id,
        &digest,
    )?;
    let mut object = json!({"_id":model::object_id(backend.flavor), "_kind":format!("{}:1",backend.service), "state":candidate.encode()?});
    if let Some((rev, _)) = &current {
        object["_rev"] = json!(rev);
    }
    let payload = checked_put_payload(object)?;
    let result = backend.rpc.call("luna://com.palm.db/put", &payload);
    record_put_failure(&result);
    // After the first write attempt, errors are uncertain outcomes, never a definite failure.
    let actual = match load(backend) {
        Ok(actual) => actual,
        Err(_) => return Ok(unavailable()),
    };
    let expectation_changed = !expected_matches(&expected, actual.as_ref());
    if let Some((_, state)) = actual {
        if state == candidate {
            return Ok(Response::OttCommit {
                status: CommitStatus::Committed,
                receipt: state.receipt(&operation_id, &digest, true)?,
            });
        }
        match state.receipt(&operation_id, &digest, false) {
            Ok(Some(receipt)) => {
                return Ok(Response::OttCommit {
                    status: CommitStatus::Committed,
                    receipt: Some(receipt),
                })
            }
            Err(_) => return Ok(unavailable()),
            Ok(None) => (),
        }
    }
    // A rejected put may mean quota, permissions or size, not a CAS conflict.
    // Only the independently read current snapshot proves a changed expectation.
    if expectation_changed && matches!(result, Ok(ref value) if value["returnValue"] == false) {
        Ok(Response::OttCommit {
            status: CommitStatus::Conflict,
            receipt: None,
        })
    } else {
        Ok(unavailable())
    }
}

#[cfg(test)]
#[path = "ott_backend_tests.rs"]
mod tests;
