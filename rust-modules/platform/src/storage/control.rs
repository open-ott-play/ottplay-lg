//! Helper-owned Control storage selected by the native ARM remote-control adapter.
//! Run on a storage worker. Loading may import retained v1 files through the existing
//! private import transaction, but only an explicit save creates or updates v2.
use super::{helper, secure, Namespace, Sealed, WritePermit, WRITER};
use crate::storage::{
    client::{self, Transport}, state::{self, Generation},
    wire::{self, control, ott, CommitStatus, Request, Response}, CommitReceipt, CommitStage,
    StoreError,
};
pub use control::Config;

/// Pure application adapter: use the existing Config::parse (including its URL
/// normalization and legacy defaults), then convert to this wire Config. Required
/// explicitly so this lower layer never invents a second URL parser or a fallback.
pub type LegacyDecoder = fn(serde_json::Value) -> Result<Config, ()>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Storage(super::Error),
    /// A present encrypted record could not be opened. Never interpret as absence.
    Locked(super::Error),
    /// An initial legacy import could not be confirmed. No v2 save was attempted.
    Uncertain,
}
impl From<super::Error> for Error {
    fn from(error: super::Error) -> Self {
        match error {
            super::Error::ProtectionUnavailable | super::Error::ProtectionFailed(_) => Self::Locked(error),
            error => Self::Storage(error),
        }
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct Snapshot {
    pub value: Config,
    pub revision: u64,
    pub epoch: u64,
}
impl std::fmt::Debug for Snapshot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("OttControlSnapshot([REDACTED])")
    }
}

struct Observation {
    status: Option<ott::Status>,
    source: control::Source,
    value: Option<Config>,
}
type Open = fn(Namespace, &Sealed) -> Option<Vec<u8>>;

fn invalid() -> Error { super::Error::Invalid.into() }
fn client_error(error: client::ClientError) -> Error { helper::client_error(error).into() }
fn uncertain() -> CommitReceipt {
    CommitReceipt::Uncertain { stage: CommitStage::Readback, errno: 0 }
}

/// Normalize only the decrypted in-memory value. The source digest and retained
/// v1 bytes always refer to the original encrypted record, never a reserialization.
pub(super) fn decode_config(value: serde_json::Value, decode: LegacyDecoder) -> Result<Config, super::Error> {
    let config = decode(value).map_err(|_| super::Error::Invalid)?;
    config.validate().map_err(|_| super::Error::Invalid)?;
    Ok(config)
}

fn observe(transport: &mut dyn Transport, open: Open, decode: LegacyDecoder) -> Result<Observation, Error> {
    let (status, loaded) = match transport.transact(Request::OttControlLoad {}).map_err(client_error)? {
        Response::OttControlLoaded { status, value } => (status, value),
        Response::KeymanagerError { failure, .. } => return Err(super::Error::ProtectionFailed(failure).into()),
        Response::Error { code } => return Err(client_error(client::error(code))),
        _ => return Err(client_error(client::ClientError::Protocol)),
    };
    if status.as_ref().is_some_and(|status| !client::ott::valid_status(status)) {
        return Err(invalid());
    }
    if status.as_ref().is_some_and(|status| status.barrier == ott::Barrier::Pending) {
        return Err(super::Error::Revoked.into());
    }
    if status.as_ref().is_some_and(|status| !status.private_initialized) {
        return Err(super::Error::Store(StoreError::Conflict).into());
    }
    let present = !matches!(&loaded, control::Loaded::Missing {});
    if status.as_ref().is_some_and(|status| status.control_present != present)
        || (status.is_none() && present)
    { return Err(invalid()); }
    let (source, value) = match loaded {
        control::Loaded::Missing {} => (control::Source::Absent {}, None),
        control::Loaded::Legacy { envelope } => {
            if ott::envelope_app(&envelope.0, ott::Namespace::Control).map_err(|_| invalid())?
                != plx_base::paths::app_id()
            { return Err(invalid()); }
            let value = super::decode_payload(Namespace::OttControl, &envelope.0, open)?
                .ok_or_else(invalid)?;
            let config = decode_config(value, decode)?;
            (control::Source::Legacy { sha256: state::digest_bytes(envelope.0.as_bytes()) }, Some(config))
        }
        control::Loaded::Protected { value, envelope_sha256 } => {
            value.validate().map_err(|_| invalid())?;
            if !control::valid_digest(&envelope_sha256) { return Err(invalid()); }
            (control::Source::Protected { sha256: envelope_sha256 }, Some(value))
        }
    };
    Ok(Observation { status, source, value })
}

fn prepare(transport: &mut dyn Transport, permit: &WritePermit, decode: LegacyDecoder) -> Result<(), Error> {
    if !permit.is_valid() { return Err(super::Error::Revoked.into()); }
    helper::ready_for_control(transport, permit, decode).map_err(|error| match error {
        // The old importer conservatively reports unconfirmed writes as unavailable.
        super::Error::Store(StoreError::HelperUnavailable)
        | super::Error::HelperFailed { source: StoreError::HelperUnavailable, .. } => Error::Uncertain,
        error => error.into(),
    })?;
    Ok(())
}

/// Explicit opt-in. May import existing encrypted files, retaining them unchanged.
/// Does not migrate a stored v1 Control value to v2 or write a new configuration.
/// `decode` must preserve the application's existing legacy parsing semantics.
pub fn load_guarded(permit: &WritePermit, decode: LegacyDecoder) -> Result<Option<Snapshot>, Error> {
    let _writer = WRITER.lock().unwrap_or_else(|error| error.into_inner());
    let transport = &mut client::NativeTransport;
    prepare(transport, permit, decode)?;
    let observed = observe(transport, secure::open_named, decode)?;
    if !permit.is_valid() { return Err(super::Error::Revoked.into()); }
    match (observed.value, observed.status) {
        (Some(value), Some(status)) => Ok(Some(Snapshot { value, revision: status.revision, epoch: status.epoch })),
        (None, _) => Ok(None),
        _ => Err(invalid()),
    }
}

/// Explicit opt-in. One Control commit, no reconciliation or automatic replay.
/// Only a fresh stored decryption matching the exact receipt and Config is durable.
/// `decode` is also required before overwriting or importing any legacy Control.
pub fn save_guarded(value: &Config, permit: &WritePermit, decode: LegacyDecoder) -> Result<CommitReceipt, Error> {
    value.validate().map_err(|_| invalid())?;
    let _writer = WRITER.lock().unwrap_or_else(|error| error.into_inner());
    let transport = &mut client::NativeTransport;
    prepare(transport, permit, decode)?;
    let operation = Generation::random().map_err(|_| invalid())?;
    save_prepared(transport, value, permit, operation, secure::open_named, decode)
}

fn save_prepared(transport: &mut dyn Transport, value: &Config, permit: &WritePermit,
    operation: Generation, open: Open, decode: LegacyDecoder) -> Result<CommitReceipt, Error>
{
    let observed = observe(transport, open, decode)?;
    commit(transport, observed, value, permit, operation, (open, decode))
}

fn commit(transport: &mut dyn Transport, before: Observation, value: &Config,
    permit: &WritePermit, operation: Generation, legacy: (Open, LegacyDecoder)) -> Result<CommitReceipt, Error>
{
    let (open, decode) = legacy;
    value.validate().map_err(|_| invalid())?;
    let operation_id = client::generation_text(operation);
    let mut request = Request::OttControlCommit {
        expected: client::ott::expected(before.status.as_ref()),
        operation_id: operation_id.clone(), digest: String::new(),
        source: before.source, value: value.clone(),
    };
    let digest = state::digest_bytes(&wire::request_digest_bytes(&request).map_err(|_| invalid())?);
    if let Request::OttControlCommit { digest: target, .. } = &mut request { *target = digest.clone(); }
    if !permit.is_valid() { return Err(super::Error::Revoked.into()); }
    let receipt = match transport.transact(request) {
        Ok(Response::OttControlCommit { status: CommitStatus::Committed, receipt: Some(receipt) })
            if receipt.operation_id == operation_id && receipt.digest == digest
                && receipt.revision > 0 && receipt.epoch > 0 => receipt,
        Ok(Response::OttControlCommit { status: CommitStatus::Conflict, receipt: None }) =>
            return Err(super::Error::Store(StoreError::Conflict).into()),
        Ok(Response::OttControlCommit { status: CommitStatus::Invalid, receipt: None }) => return Err(invalid()),
        Ok(Response::KeymanagerError { failure, preservation: wire::AuthPreservation::Unchanged,
            db8_commit_verified: false }) => return Err(super::Error::ProtectionFailed(failure).into()),
        Ok(Response::Error { code: wire::ErrorCode::Invalid }) => return Err(invalid()),
        // Any ambiguous response after dispatch can hide an applied write.
        _ => return Ok(uncertain()),
    };
    let after = match observe(transport, open, decode) {
        Ok(after) => after,
        Err(_) => return Ok(uncertain()),
    };
    let matches = matches!(&after.source, control::Source::Protected { .. })
        && after.status.as_ref().is_some_and(|status|
            status.revision == receipt.revision && status.epoch == receipt.epoch)
        && after.value.as_ref() == Some(value)
        && permit.is_valid();
    Ok(if matches { CommitReceipt::Durable } else { uncertain() })
}

#[cfg(test)]
mod tests {
    use super::*;
    use wire::SecretString;

    fn config() -> Config {
        Config { version: 1, enabled: true, address: "https://control.invalid:443".into(),
            token: SecretString("a".repeat(32)), device_id: "fixture-device".into(), allow_insecure_http: false }
    }
    fn status(revision: u64) -> ott::Status {
        ott::Status { db_rev: revision.to_string(), revision, epoch: 1, barrier: ott::Barrier::Open,
            control_present: true, state_present: false, private_initialized: true }
    }
    fn protected(revision: u64) -> Response {
        Response::OttControlLoaded { status: Some(status(revision)), value: control::Loaded::Protected {
            value: config(), envelope_sha256: "b".repeat(64),
        } }
    }
    fn legacy() -> wire::SecretString {
        SecretString(serde_json::json!({"version":1,"sealed":{
            "backend":"keymanager3", "key":Namespace::OttControl.key_name(),
            "iv":"AQ==", "data":"Ag=="
        }}).to_string())
    }
    fn refuse_open(_: Namespace, _: &Sealed) -> Option<Vec<u8>> { None }
    fn decode_fixture(value: serde_json::Value) -> Result<Config, ()> {
        serde_json::from_value(value).map_err(|_| ())
    }
    fn open_unnormalized_legacy(namespace: Namespace, _: &Sealed) -> Option<Vec<u8>> {
        let mut value = serde_json::to_value(config()).unwrap();
        value["address"] = serde_json::json!(" https://control.invalid/api/webhook/commands/// ");
        super::super::encode_plain(namespace, &value).ok()
    }
    fn decode_legacy_fixture(mut value: serde_json::Value) -> Result<Config, ()> {
        // The actual application adapter must use its existing Config::parse.
        // This fixture pins the boundary: it receives the original logical value
        // before wire validation and returns the normalized typed representation.
        assert_eq!(value["address"], " https://control.invalid/api/webhook/commands/// ");
        value["address"] = serde_json::json!("https://control.invalid:443");
        decode_fixture(value)
    }

    #[test]
    fn locked_legacy_is_never_missing_or_overwritten() {
        let mut calls = 0;
        let mut transport = |request| {
            calls += 1;
            assert!(matches!(request, Request::OttControlLoad {}));
            Ok(Response::OttControlLoaded { status: Some(status(2)),
                value: control::Loaded::Legacy { envelope: legacy() } })
        };
        assert_eq!(save_prepared(&mut transport, &config(), &WritePermit::new(),
            Generation([1; 16]), refuse_open, decode_fixture), Err(Error::Locked(super::super::Error::ProtectionUnavailable)));
        assert_eq!(calls, 1);
    }

    #[test]
    fn legacy_decoder_cannot_bypass_wire_config_validation() {
        let value = serde_json::to_value(config()).unwrap();
        assert!(decode_config(value.clone(), decode_fixture).is_ok());
        let mut bad = value.clone(); bad["unbounded_payload"] = serde_json::json!("x");
        assert!(decode_config(bad, decode_fixture).is_err());
        let mut bad = value; bad["version"] = serde_json::json!(9);
        assert!(decode_config(bad, decode_fixture).is_err());
    }

    #[test]
    fn legacy_normalization_precedes_commit_without_changing_the_source_digest() {
        for wrong_revision in [false, true] {
            let mut calls = 0;
            let source_digest = state::digest_bytes(legacy().0.as_bytes());
            let mut transport = |request: Request| {
                calls += 1;
                let expected_digest = if matches!(&request, Request::OttControlCommit { .. }) {
                    Some(state::digest_bytes(&wire::request_digest_bytes(&request).unwrap()))
                } else { None };
                match request {
                    Request::OttControlLoad {} if calls == 1 => Ok(Response::OttControlLoaded {
                        status: Some(status(2)), value: control::Loaded::Legacy { envelope: legacy() },
                    }),
                    Request::OttControlCommit { expected, operation_id, digest, source, value } => {
                        assert_eq!(expected, client::ott::expected(Some(&status(2))));
                        assert_eq!(source, control::Source::Legacy { sha256: source_digest.clone() });
                        assert_eq!(Some(digest.clone()), expected_digest);
                        assert_eq!(value, config());
                        Ok(Response::OttControlCommit { status: CommitStatus::Committed,
                            receipt: Some(ott::Receipt { operation_id, digest, revision: 3, epoch: 1,
                                verified: false }) })
                    }
                    Request::OttControlLoad {} => Ok(protected(if wrong_revision { 4 } else { 3 })),
                    _ => panic!("unexpected operation"),
                }
            };
            let result = save_prepared(&mut transport, &config(), &WritePermit::new(),
                Generation([1; 16]), open_unnormalized_legacy, decode_legacy_fixture).unwrap();
            assert_eq!(calls, 3);
            assert_eq!(result, if wrong_revision { uncertain() } else { CommitReceipt::Durable });
        }
    }

    #[test]
    fn lost_commit_reply_is_uncertain_and_is_never_replayed() {
        let mut calls = 0;
        let mut transport = |request| {
            calls += 1;
            match request {
                Request::OttControlLoad {} if calls == 1 => Ok(protected(2)),
                Request::OttControlCommit { .. } => Err(client::ClientError::Unavailable),
                _ => panic!("uncertain operation must not be retried"),
            }
        };
        assert_eq!(save_prepared(&mut transport, &config(), &WritePermit::new(),
            Generation([1; 16]), refuse_open, decode_fixture).unwrap(), uncertain());
        assert_eq!(calls, 2);
    }

    #[test]
    fn revocation_after_load_prevents_the_control_write() {
        let permit = WritePermit::new();
        let mut calls = 0;
        let mut transport = |request| {
            calls += 1;
            assert!(matches!(request, Request::OttControlLoad {}));
            permit.revoke();
            Ok(protected(2))
        };
        assert_eq!(save_prepared(&mut transport, &config(), &permit,
            Generation([1; 16]), refuse_open, decode_fixture), Err(Error::Storage(super::super::Error::Revoked)));
        assert_eq!(calls, 1);
    }
}
