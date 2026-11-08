//! Typed private-record operations over the existing authenticated helper transport.
//! A reply lost after dispatch is uncertain until the operation ledger proves otherwise.
use super::{error, generation_text, ClientError, NativeTransport, Transport};
use crate::storage::{
    state::{self, Generation},
    wire::{self, ott, CommitStatus, ReconcileStatus, Request, Response},
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    Applied(ott::Receipt),
    Conflict,
    Uncertain,
    Revoked,
}

fn valid_decimal(value: &str) -> bool {
    value
        .parse::<u64>()
        .ok()
        .is_some_and(|number| number > 0 && number.to_string() == value)
}

pub(crate) fn valid_status(status: &ott::Status) -> bool {
    valid_decimal(&status.db_rev)
        && status.revision > 0
        && status.epoch > 0
        && (status.private_initialized || (status.barrier == ott::Barrier::Open
            && !status.control_present && !status.state_present))
        && (status.barrier != ott::Barrier::Pending
            || (!status.control_present && !status.state_present))
}

fn valid_value(value: &wire::SecretString) -> bool {
    ott::validate_envelope(&value.0).is_ok()
}

fn valid_control_value(value: &wire::SecretString) -> bool {
    if wire::control::is_protected(&value.0) {
        wire::control::Envelope::parse(&value.0, plx_base::paths::app_id()).is_ok()
    } else { valid_value(value) }
}

fn valid_state_value(value: &wire::SecretString) -> bool {
    if wire::saved::is_protected(&value.0) {
        wire::saved::Envelope::parse(&value.0, plx_base::paths::app_id()).is_ok()
    } else { valid_value(value) }
}

#[cfg(test)]
mod saved_envelope_tests {
    use super::*;

    #[test]
    fn generic_snapshot_accepts_state_v2_but_generic_put_cannot_inject_it() {
        let mut envelope = wire::saved::Envelope {
            version: 2, install: plx_base::paths::app_id().into(), namespace: "state".into(),
            cipher: wire::SecretString(String::new()), legacy: None,
        };
        envelope.cipher = wire::SecretString(serde_json::json!({
            "format":"keymanager3-aes256-gcm-v1","key_name":wire::saved::KEY_NAME,
            "iv":"AQ==","ciphertext":"Ag==","context":envelope.context()
        }).to_string());
        let value = wire::SecretString(envelope.encode().unwrap());
        let mut transport = |request| {
            assert!(matches!(request, Request::OttLoad {}));
            Ok(Response::OttLoaded { snapshot: Some(ott::Snapshot {
                status: ott::Status { db_rev: "2".into(), revision: 2, epoch: 1,
                    barrier: ott::Barrier::Open, control_present: false, state_present: true,
                    private_initialized: true },
                control: None, state: Some(value.clone()), public_preferences: None,
            }) })
        };
        assert_eq!(load_with(&mut transport).unwrap().unwrap().state, Some(value.clone()));
        assert!(!valid_mutation(&ott::Mutation::Put { namespace: ott::Namespace::State, value }));
        envelope.namespace = "control".into();
        let invalid = wire::SecretString(serde_json::to_string(&envelope).unwrap());
        assert!(!valid_state_value(&invalid));
    }
}

fn valid_mutation(mutation: &ott::Mutation) -> bool {
    match mutation {
        ott::Mutation::CompletePrivateImport { control, state, barrier }
        | ott::Mutation::Initialize {
            control,
            state,
            barrier,
        } => {
            (control.as_ref().is_none_or(valid_value) && state.as_ref().is_none_or(valid_value))
                && (*barrier != ott::Barrier::Pending || (control.is_none() && state.is_none()))
        }
        ott::Mutation::PatchPublic { patch } => patch.validate().is_ok(),
        ott::Mutation::Put { value, .. } => valid_value(value),
        ott::Mutation::BeginErase {} | ott::Mutation::FinishErase {} => true,
    }
}

pub fn expected(status: Option<&ott::Status>) -> ott::Expected {
    match status {
        Some(status) => ott::Expected::Present {
            db_rev: status.db_rev.clone(),
            revision: status.revision,
            epoch: status.epoch,
        },
        None => ott::Expected::Missing {},
    }
}

pub fn status() -> Result<Option<ott::Status>, ClientError> {
    status_with(&mut NativeTransport)
}

pub fn status_with(transport: &mut dyn Transport) -> Result<Option<ott::Status>, ClientError> {
    match transport.transact(Request::OttStatus {})? {
        Response::OttStatus { status } if status.as_ref().is_none_or(valid_status) => Ok(status),
        Response::Error { code } => Err(error(code)),
        _ => Err(ClientError::Protocol),
    }
}

pub fn load() -> Result<Option<ott::Snapshot>, ClientError> {
    load_with(&mut NativeTransport)
}

pub fn load_with(transport: &mut dyn Transport) -> Result<Option<ott::Snapshot>, ClientError> {
    match transport.transact(Request::OttLoad {})? {
        Response::OttLoaded { snapshot } => {
            if let Some(snapshot) = &snapshot {
                if !valid_status(&snapshot.status)
                    || snapshot.status.barrier == ott::Barrier::Pending
                    || snapshot.status.control_present != snapshot.control.is_some()
                    || snapshot.status.state_present != snapshot.state.is_some()
                    || !snapshot.control.as_ref().is_none_or(valid_control_value)
                    || !snapshot.state.as_ref().is_none_or(valid_state_value)
                    || snapshot.public_preferences.as_ref().is_some_and(|prefs| prefs.validate().is_err())
                {
                    return Err(ClientError::Corrupt);
                }
            }
            Ok(snapshot)
        }
        Response::Error { code } => Err(error(code)),
        _ => Err(ClientError::Protocol),
    }
}

fn receipt_matches(receipt: &ott::Receipt, operation_id: &str, digest: &str) -> bool {
    receipt.operation_id == operation_id
        && receipt.digest == digest
        && receipt.revision > 0
        && receipt.epoch > 0
}

/// `can_write` is checked before every dispatch that can write, including a retry.
/// Revocation after an admitted write does not invalidate an already durable result.
pub fn commit_with(
    transport: &mut dyn Transport,
    expected: ott::Expected,
    operation: Generation,
    mutation: ott::Mutation,
    can_write: &dyn Fn() -> bool,
) -> Result<Outcome, ClientError> {
    if !valid_mutation(&mutation)
        || matches!(&expected,
        ott::Expected::Present { db_rev, revision, epoch }
            if !valid_decimal(db_rev) || *revision == 0 || *epoch == 0)
    {
        return Err(ClientError::Invalid);
    }
    let operation_id = generation_text(operation);
    let mut request = Request::OttCommit {
        expected,
        operation_id: operation_id.clone(),
        digest: String::new(),
        mutation,
    };
    let digest = state::digest_bytes(&wire::request_digest_bytes(&request).map_err(error)?);
    if let Request::OttCommit { digest: target, .. } = &mut request {
        *target = digest.clone();
    }
    if !can_write() {
        return Ok(Outcome::Revoked);
    }
    let first = transport.transact(request.clone());
    if let Some(result) = commit_result(first.as_ref().ok(), &operation_id, &digest) {
        return result;
    }
    // Any transport/protocol failure after dispatch may hide a committed write.
    // Never turn it into Err: private callers interpret Err as definitely not applied.
    match transport.transact(Request::OttReconcile {
        operation_id: operation_id.clone(),
        digest: digest.clone(),
    }) {
        Ok(Response::OttReconcile {
            status: ReconcileStatus::Applied,
            receipt: Some(mut receipt),
        }) if receipt_matches(&receipt, &operation_id, &digest) => {
            receipt.verified = false; // Historical evidence cannot authorize legacy cleanup.
            Ok(Outcome::Applied(receipt))
        }
        Ok(Response::OttReconcile {
            status: ReconcileStatus::NotApplied,
            receipt: None,
        }) => {
            if !can_write() {
                return Ok(Outcome::Revoked);
            }
            let retry = transport.transact(request);
            commit_result(retry.as_ref().ok(), &operation_id, &digest)
                .unwrap_or(Ok(Outcome::Uncertain))
        }
        _ => Ok(Outcome::Uncertain),
    }
}

fn commit_result(
    response: Option<&Response>,
    operation: &str,
    digest: &str,
) -> Option<Result<Outcome, ClientError>> {
    match response? {
        Response::OttCommit {
            status: CommitStatus::Committed,
            receipt: Some(receipt),
        } if receipt_matches(receipt, operation, digest) => {
            Some(Ok(Outcome::Applied(receipt.clone())))
        }
        Response::OttCommit {
            status: CommitStatus::Conflict,
            receipt: None,
        } => Some(Ok(Outcome::Conflict)),
        Response::OttCommit {
            status: CommitStatus::Invalid,
            receipt: None,
        } => Some(Err(ClientError::Invalid)),
        Response::Error {
            code: wire::ErrorCode::Invalid,
        } => Some(Err(ClientError::Invalid)),
        Response::Error {
            code: wire::ErrorCode::Corrupt,
        } => Some(Err(ClientError::Corrupt)),
        Response::Error {
            code: wire::ErrorCode::Authentication,
        } => Some(Err(ClientError::Authentication)),
        _ => None,
    }
}

#[cfg(test)]
#[path = "client_ott_tests.rs"]
mod tests;
