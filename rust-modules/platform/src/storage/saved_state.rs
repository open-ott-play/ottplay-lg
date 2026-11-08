//! Typed helper-owned encryption for the existing private OTT State slot.
//! The application still owns Saved validation and write admission. Legacy bytes
//! are authenticated before import and retained; only an explicit save creates v2.
use super::{helper, secure, Error, InspectError, Namespace, NamespaceSnapshot, Sealed, WritePermit, WRITER};
use crate::storage::{
    client::{self, Transport}, state::{self, Generation},
    wire::{self, ott, saved, CommitStatus, Request, Response}, CommitReceipt, CommitStage, StoreError,
};
use serde_json::Value;

type Open = fn(Namespace, &Sealed) -> Option<Vec<u8>>;

struct Observation {
    status: Option<ott::Status>,
    source: saved::Source,
    value: Option<saved::Document>,
}

fn document(value: &Value) -> Result<saved::Document, Error> {
    // Preserve the old bound including its installation/namespace wrapper.
    super::validate_value(Namespace::OttState, value)?;
    saved::Document::try_from(value.clone()).map_err(|_| Error::Invalid)
}

pub(super) fn validate_document(value: &Value) -> Result<(), Error> {
    document(value).map(|_| ())
}

fn uncertain() -> CommitReceipt {
    CommitReceipt::Uncertain { stage: CommitStage::Readback, errno: 0 }
}

fn observe(transport: &mut dyn Transport, open: Open) -> Result<Observation, Error> {
    let (status, loaded) = match transport.transact(Request::OttStateLoad {}).map_err(helper::client_error)? {
        Response::OttStateLoaded { status, value } => (status, value),
        Response::KeymanagerError { failure, .. } => return Err(Error::ProtectionFailed(failure)),
        Response::Error { code } => return Err(helper::client_error(client::error(code))),
        _ => return Err(helper::client_error(client::ClientError::Protocol)),
    };
    if status.as_ref().is_some_and(|status| !client::ott::valid_status(status)) {
        return Err(Error::Invalid);
    }
    if status.as_ref().is_some_and(|status| status.barrier == ott::Barrier::Pending) {
        return Err(Error::Revoked);
    }
    let present = !matches!(&loaded, saved::Loaded::Missing {});
    if status.as_ref().is_some_and(|status| status.state_present != present)
        || (status.is_none() && present)
    { return Err(Error::Invalid); }
    let (source, value) = match loaded {
        saved::Loaded::Missing {} => (saved::Source::Absent {}, None),
        saved::Loaded::Legacy { envelope } => {
            if ott::envelope_app(&envelope.0, ott::Namespace::State).map_err(|_| Error::Invalid)?
                != plx_base::paths::app_id()
            { return Err(Error::Invalid); }
            let value = super::decode_payload(Namespace::OttState, &envelope.0, open)?
                .ok_or(Error::Invalid)?;
            let value = document(&value)?;
            (saved::Source::Legacy { sha256: state::digest_bytes(envelope.0.as_bytes()) }, Some(value))
        }
        saved::Loaded::Protected { value, envelope_sha256 } => {
            value.validate().map_err(|_| Error::Invalid)?;
            super::validate_value(Namespace::OttState, value.as_ref())?;
            let source = saved::Source::Protected { sha256: envelope_sha256 };
            source.validate().map_err(|_| Error::Invalid)?;
            (source, Some(value))
        }
    };
    Ok(Observation { status, source, value })
}

fn require_initialized(observed: &Observation) -> Result<(), Error> {
    if observed.status.as_ref().is_some_and(|status| !status.private_initialized) {
        return Err(Error::Store(StoreError::Conflict));
    }
    Ok(())
}

// This function does not acquire WRITER: all callers already own it.
fn prepare(transport: &mut dyn Transport, permit: &WritePermit) -> Result<(), Error> {
    if !permit.is_valid() { return Err(Error::Revoked); }
    helper::ready_for_saved_state(transport, permit)?;
    if !permit.is_valid() { return Err(Error::Revoked); }
    Ok(())
}

pub(super) fn load() -> Result<Option<Value>, Error> {
    let _writer = WRITER.lock().unwrap_or_else(|error| error.into_inner());
    let transport = &mut client::NativeTransport;
    prepare(transport, &WritePermit::new())?;
    let observed = observe(transport, secure::open_named)?;
    require_initialized(&observed)?;
    Ok(observed.value.map(Value::from))
}

/// Inspection uses only the existing authenticated read endpoint. No prepare,
/// import, key generation, activation, permission repair or mutation is allowed.
pub(super) fn inspect() -> Result<Option<NamespaceSnapshot>, InspectError> {
    let _writer = WRITER.lock().unwrap_or_else(|error| error.into_inner());
    inspect_with(&mut client::ExistingTransport,
        || helper::legacy_present_readonly(&super::root()), secure::open_named)
}

fn inspect_with(transport: &mut dyn Transport,
    legacy_present: impl FnOnce() -> Result<bool, Error>, open: Open,
) -> Result<Option<NamespaceSnapshot>, InspectError> {
    let observed = observe(transport, open)?;
    let Some(status) = &observed.status else {
        return if legacy_present()? { Err(InspectError::LegacyOnly) } else { Ok(None) };
    };
    if !status.private_initialized { return Err(InspectError::LegacyOnly); }
    match (observed.source, observed.value) {
        (saved::Source::Absent {}, None) => Ok(None),
        (saved::Source::Legacy { sha256 } | saved::Source::Protected { sha256 }, Some(value)) => {
            Ok(Some(NamespaceSnapshot { value: Value::from(value), namespace_sha256: sha256 }))
        }
        _ => Err(Error::Invalid.into()),
    }
}

pub(super) fn save(value: &Value, permit: &WritePermit) -> Result<CommitReceipt, Error> {
    let value = document(value)?;
    let _writer = WRITER.lock().unwrap_or_else(|error| error.into_inner());
    let transport = &mut client::NativeTransport;
    match prepare(transport, permit) {
        // The shared legacy importer may have dispatched an unconfirmed commit.
        // Do not claim a definite rejection or attempt the v2 write after it.
        Err(Error::Store(StoreError::HelperUnavailable)
            | Error::HelperFailed { source: StoreError::HelperUnavailable, .. }) => return Ok(uncertain()),
        other => other?,
    }
    let operation = Generation::random().map_err(|_| Error::Invalid)?;
    save_prepared(transport, &value, permit, operation, secure::open_named)
}

fn save_prepared(transport: &mut dyn Transport, value: &saved::Document,
    permit: &WritePermit, operation: Generation, open: Open,
) -> Result<CommitReceipt, Error> {
    if !permit.is_valid() { return Err(Error::Revoked); }
    value.validate().map_err(|_| Error::Invalid)?;
    let before = observe(transport, open)?;
    require_initialized(&before)?;
    let operation_id = client::generation_text(operation);
    let mut request = Request::OttStateCommit {
        expected: client::ott::expected(before.status.as_ref()),
        operation_id: operation_id.clone(), digest: String::new(),
        source: before.source, value: value.clone(),
    };
    let digest = state::digest_bytes(&wire::request_digest_bytes(&request).map_err(|_| Error::Invalid)?);
    if let Request::OttStateCommit { digest: target, .. } = &mut request { *target = digest.clone(); }
    if !permit.is_valid() { return Err(Error::Revoked); }
    let receipt = match transport.transact(request) {
        Ok(Response::OttStateCommit { status: CommitStatus::Committed, receipt: Some(receipt) })
            if receipt.operation_id == operation_id && receipt.digest == digest
                && receipt.revision > 0 && receipt.epoch > 0 => receipt,
        Ok(Response::OttStateCommit { status: CommitStatus::Conflict, receipt: None }) =>
            return Err(Error::Store(StoreError::Conflict)),
        Ok(Response::OttStateCommit { status: CommitStatus::Invalid, receipt: None }) => return Err(Error::Invalid),
        Ok(Response::KeymanagerError { failure, preservation: wire::AuthPreservation::Unchanged,
            db8_commit_verified: false }) => return Err(Error::ProtectionFailed(failure)),
        Ok(Response::Error { code: wire::ErrorCode::Invalid }) => return Err(Error::Invalid),
        // After dispatch a malformed/lost reply can conceal a committed candidate.
        // Never retry, downgrade, or classify such a result as definitely rejected.
        _ => return Ok(uncertain()),
    };
    let after = match observe(transport, open) {
        Ok(after) => after,
        Err(_) => return Ok(uncertain()),
    };
    let matches = matches!(&after.source, saved::Source::Protected { .. })
        && after.status.as_ref().is_some_and(|status| status.private_initialized
            && status.revision == receipt.revision && status.epoch == receipt.epoch)
        && after.value.as_ref() == Some(value)
        && permit.is_valid();
    Ok(if matches { CommitReceipt::Durable } else { uncertain() })
}

#[cfg(test)]
#[path = "private_saved_state_tests.rs"]
mod tests;
