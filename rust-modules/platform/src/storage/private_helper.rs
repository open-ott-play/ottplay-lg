//! Application-side crypto and migration; helper-side storage remains opaque.
use super::*;
use crate::storage::{
    client::{self, ott as remote, Transport},
    state::Generation,
    wire::{self, ott, SecretString},
};

type Open = fn(Namespace, &Sealed) -> Option<Vec<u8>>;
#[cfg(test)]
type Seal = fn(Namespace, &[u8]) -> Option<Sealed>;

#[path = "private_absence.rs"]
mod absence;

#[derive(Clone)]
struct LegacyImage {
    present: bool,
    barrier: ott::Barrier,
    control: Option<SecretString>,
    state: Option<SecretString>,
}

trait Legacy {
    fn read(&mut self) -> Result<LegacyImage, Error>;
    fn barrier(&mut self) -> Result<ott::Barrier, Error>;
    fn erase(&mut self) -> Vec<String>;
}

struct Files;
impl Legacy for Files {
    fn read(&mut self) -> Result<LegacyImage, Error> {
        read_legacy_at(&root())
    }
    fn barrier(&mut self) -> Result<ott::Barrier, Error> {
        erasure_state_at(&root()).map(legacy_barrier)
    }
    fn erase(&mut self) -> Vec<String> {
        let mut errors = Vec::new();
        match existing_store(&root()) {
            Ok(Some(store)) => {
                errors.extend(erase_store_with(&store, || {
                    erase_managed_at(plx_base::paths::app_dir())
                }));
            }
            Ok(None) => {
                // Absence must be synced even on retry: unlink may have succeeded
                // before an earlier parent fsync failed. Never create or chmod the
                // bundle, and keep Pending if its parent cannot be validated.
                errors.extend(erase_no_private(&root(), || {
                    erase_managed_at(plx_base::paths::app_dir())
                }));
            }
            Err(error) => errors.push(format!("OTT legacy private root: {error:?}")),
        }
        errors
    }
}

fn erase_no_private(
    private_root: &Path,
    managed_cleanup: impl FnOnce() -> Vec<String>,
) -> Vec<String> {
    let absent = || {
        matches!(std::fs::symlink_metadata(private_root),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound)
    };
    if !absent() {
        return vec!["OTT legacy private root appeared during cleanup".into()];
    }
    let mut errors = managed_cleanup();
    if !absent() {
        errors.push("OTT legacy private root appeared during cleanup".into());
    }
    errors
}

fn erase_managed_at(path: &Path) -> Vec<String> {
    match JsonStore::new(path.to_owned()) {
        Ok(directory) => erase_managed(&directory),
        Err(StoreError::RootUnsafeMode) => match absence::prove(path) {
            Ok(()) => Vec::new(),
            Err(error) => vec![format!("OTT managed fallback absence: {error:?}")],
        },
        Err(error) => vec![format!("OTT managed fallback directory: {error:?}")],
    }
}

fn erase_managed(directory: &JsonStore) -> Vec<String> {
    let mut errors = Vec::new();
    for name in ["ottplay-control.json", "ottplay-profile.json"] {
        if let Err(error) = remove_owned_file(directory, name, false) {
            errors.push(format!("OTT managed {name}: {error:?}"));
        }
    }
    errors
}

fn read_legacy_at(path: &Path) -> Result<LegacyImage, Error> {
    let Some(store) = existing_store(path)? else {
        return Ok(LegacyImage {
            present: false,
            barrier: ott::Barrier::Open,
            control: None,
            state: None,
        });
    };
    let barrier = legacy_barrier(erasure_state(&store)?);
    let control = store.load(RecordKey::OttControl)?;
    let state = store.load(RecordKey::OttState)?;
    let present = control.is_some() || state.is_some() || barrier != ott::Barrier::Open;
    let payload = |record: Option<Record>| match record {
        Some(Record {
            state: RecordState::Data { payload },
            ..
        }) => Some(SecretString(payload)),
        _ => None,
    };
    // Complete still suppresses managed imports, but baseline clients may
    // explicitly save fresh records after erasure. Only Pending discards values.
    Ok(LegacyImage {
        present,
        barrier,
        control: if barrier != ott::Barrier::Pending {
            payload(control)
        } else {
            None
        },
        state: if barrier != ott::Barrier::Pending {
            payload(state)
        } else {
            None
        },
    })
}

fn existing_store(path: &Path) -> Result<Option<JsonStore>, Error> {
    match std::fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(_) => Err(Error::Invalid),
        Ok(_) => Ok(Some(JsonStore::new(path.to_owned())?)),
    }
}

fn legacy_barrier(value: ErasureState) -> ott::Barrier {
    match value {
        ErasureState::Absent => ott::Barrier::Open,
        ErasureState::Pending => ott::Barrier::Pending,
        ErasureState::Complete => ott::Barrier::Complete,
    }
}

pub(super) fn client_error(error: client::ClientError) -> Error {
    Error::Store(match error {
        client::ClientError::Unavailable => StoreError::HelperUnavailable,
        client::ClientError::Authentication => StoreError::HelperAuthentication,
        client::ClientError::Protocol => StoreError::HelperProtocol,
        client::ClientError::Corrupt | client::ClientError::Invalid => StoreError::InvalidSchema,
    })
}

fn uncertain() -> CommitReceipt {
    CommitReceipt::Uncertain {
        stage: CommitStage::Readback,
        errno: 0,
    }
}
fn operation() -> Result<Generation, Error> {
    Generation::random().map_err(|_| Error::Invalid)
}
fn wire_namespace(namespace: Namespace) -> ott::Namespace {
    match namespace {
        Namespace::OttControl => ott::Namespace::Control,
        Namespace::OttState => ott::Namespace::State,
    }
}
fn slot(snapshot: &ott::Snapshot, namespace: Namespace) -> &Option<SecretString> {
    match namespace {
        Namespace::OttControl => &snapshot.control,
        Namespace::OttState => &snapshot.state,
    }
}
fn decode(namespace: Namespace, value: &SecretString, open: Open) -> Result<Value, Error> {
    ott::validate_envelope(&value.0).map_err(|_| Error::Invalid)?;
    decode_payload(namespace, &value.0, open)?.ok_or(Error::Invalid)
}
#[cfg(test)]
fn encode(namespace: Namespace, value: &Value, seal: Seal) -> Result<SecretString, Error> {
    encode_with(namespace, value, |namespace, bytes| seal(namespace, bytes).ok_or(Error::ProtectionUnavailable))
}
fn encode_with(namespace: Namespace, value: &Value,
    seal: impl FnOnce(Namespace, &[u8]) -> Result<Sealed, Error>) -> Result<SecretString, Error> {
    let bytes = encode_plain(namespace, value)?;
    let sealed = seal(namespace, &bytes)?;
    if sealed.backend != secure::Backend::Keymanager3 || sealed.key != namespace.key_name() {
        return Err(Error::ProtectionUnavailable);
    }
    let text =
        serde_json::to_string(&Envelope { version: 1, sealed }).map_err(|_| Error::Invalid)?;
    ott::validate_envelope(&text).map_err(|_| Error::TooLarge)?;
    Ok(SecretString(text))
}

fn commit(
    transport: &mut dyn Transport,
    status: Option<&ott::Status>,
    mutation: ott::Mutation,
    permit: &WritePermit,
) -> Result<remote::Outcome, Error> {
    remote::commit_with(
        transport,
        remote::expected(status),
        operation()?,
        mutation,
        &|| permit.is_valid(),
    )
    .map_err(client_error)
}

fn require_applied(outcome: remote::Outcome) -> Result<ott::Receipt, Error> {
    match outcome {
        remote::Outcome::Applied(receipt) => Ok(receipt),
        remote::Outcome::Conflict => Err(Error::Store(StoreError::Conflict)),
        remote::Outcome::Revoked => Err(Error::Revoked),
        remote::Outcome::Uncertain => Err(Error::Store(StoreError::HelperUnavailable)),
    }
}

/// Only authoritative canonical absence admits a legacy read. Both records are
/// authenticated before the atomic import. Retain encrypted legacy files: an old
/// running binary cannot participate in the helper CAS or this process's lock.
fn ready(
    transport: &mut dyn Transport,
    legacy: &mut dyn Legacy,
    open: Open,
    permit: &WritePermit,
) -> Result<Option<ott::Status>, Error> {
    ready_checked(transport, legacy, open, permit, None)
}

/// Explicit Control coordinator only. Validate the legacy logical schema before
/// importing either private slot, including when public preferences already exist.
pub(super) fn ready_for_control(
    transport: &mut dyn Transport,
    permit: &WritePermit,
    decode: control::LegacyDecoder,
) -> Result<Option<ott::Status>, Error> {
    ready_checked(transport, &mut Files, secure::open_named, permit, Some(decode))
}

/// Caller holds WRITER. State's typed door validates its legacy logical schema
/// before the shared import can make either retained encrypted slot canonical.
pub(super) fn ready_for_saved_state(
    transport: &mut dyn Transport,
    permit: &WritePermit,
) -> Result<Option<ott::Status>, Error> {
    ready_checked_with_state(transport, &mut Files, secure::open_named, permit, None, true)
}

fn ready_checked(
    transport: &mut dyn Transport,
    legacy: &mut dyn Legacy,
    open: Open,
    permit: &WritePermit,
    decode_control: Option<control::LegacyDecoder>,
) -> Result<Option<ott::Status>, Error> {
    ready_checked_with_state(transport, legacy, open, permit, decode_control, false)
}

fn ready_checked_with_state(
    transport: &mut dyn Transport,
    legacy: &mut dyn Legacy,
    open: Open,
    permit: &WritePermit,
    decode_control: Option<control::LegacyDecoder>,
    validate_state: bool,
) -> Result<Option<ott::Status>, Error> {
    let prior = remote::status_with(transport).map_err(client_error)?;
    if prior.as_ref().is_some_and(|status| status.private_initialized) {
        return Ok(prior);
    }
    let image = legacy.read()?;
    if !image.present && prior.is_none() {
        return Ok(None);
    }
    for (namespace, value) in [
        (Namespace::OttControl, &image.control),
        (Namespace::OttState, &image.state),
    ] {
        if let Some(value) = value {
            let value = decode(namespace, value, open)?;
            if validate_state && matches!(namespace, Namespace::OttState) {
                saved_state::validate_document(&value)?;
            }
            if let Some(decode) = decode_control.filter(|_| matches!(namespace, Namespace::OttControl)) {
                control::decode_config(value, decode)?;
            }
        }
    }
    let mutation = if prior.is_some() {
        ott::Mutation::CompletePrivateImport {
            control: image.control.clone(), state: image.state.clone(), barrier: image.barrier,
        }
    } else {
        ott::Mutation::Initialize {
            control: image.control.clone(), state: image.state.clone(), barrier: image.barrier,
        }
    };
    let outcome = commit(transport, prior.as_ref(), mutation, permit)?;
    if outcome == remote::Outcome::Conflict {
        // A concurrent initializer won. Its canonical result, never the old
        // files, is now the only authority.
        let winner = remote::status_with(transport).map_err(client_error)?
            .ok_or(Error::Store(StoreError::HelperProtocol))?;
        if !winner.private_initialized { return Err(Error::Store(StoreError::Conflict)); }
        return Ok(Some(winner));
    }
    let receipt = require_applied(outcome)?;
    let status = remote::status_with(transport)
        .map_err(client_error)?
        .ok_or(Error::Store(StoreError::HelperProtocol))?;
    if status.revision != receipt.revision
        || status.epoch != receipt.epoch
        || status.barrier != image.barrier
        || !status.private_initialized
    {
        return Err(Error::Store(StoreError::Conflict));
    }
    if image.barrier != ott::Barrier::Pending {
        let snapshot = remote::load_with(transport)
            .map_err(client_error)?
            .ok_or(Error::Store(StoreError::HelperProtocol))?;
        if snapshot.status != status
            || snapshot.control != image.control
            || snapshot.state != image.state
        {
            return Err(Error::Store(StoreError::Conflict));
        }
        for (namespace, value) in [
            (Namespace::OttControl, &snapshot.control),
            (Namespace::OttState, &snapshot.state),
        ] {
            if let Some(value) = value {
                decode(namespace, value, open)?;
            }
        }
    }
    Ok(Some(status))
}

// This deliberately bypasses ready(): inspection must never initialize or migrate.
fn inspect_with(
    transport: &mut dyn Transport,
    legacy_present: impl FnOnce() -> Result<bool, Error>,
    namespace: Namespace,
    open: Open,
) -> Result<Option<NamespaceSnapshot>, InspectError> {
    let Some(snapshot) = remote::load_with(transport).map_err(client_error)? else {
        return if legacy_present()? {
            Err(InspectError::LegacyOnly)
        } else {
            Ok(None)
        };
    };
    if !snapshot.status.private_initialized { return Err(InspectError::LegacyOnly); }
    slot(&snapshot, namespace)
        .as_ref()
        .map(|value| inspect_payload(namespace, &value.0, open))
        .transpose()
        .map_err(InspectError::from)
}

pub(super) fn legacy_present_readonly(path: &Path) -> Result<bool, Error> {
    let Some(store) = existing_store(path)? else {
        return Ok(false);
    };
    let mut present = false;
    for key in [RecordKey::OttErasure, RecordKey::OttControl, RecordKey::OttState] {
        present |= store.load_readonly(key)?.is_some();
    }
    store.check_root()?;
    Ok(present)
}

pub(super) fn inspect(namespace: Namespace) -> Result<Option<NamespaceSnapshot>, InspectError> {
    let _writer = WRITER.lock().unwrap_or_else(|error| error.into_inner());
    inspect_with(
        &mut client::ExistingTransport,
        || legacy_present_readonly(&root()),
        namespace,
        secure::open_named,
    )
}

fn load_with(
    transport: &mut dyn Transport,
    legacy: &mut dyn Legacy,
    namespace: Namespace,
    open: Open,
) -> Result<Option<Value>, Error> {
    let Some(status) = ready(transport, legacy, open, &WritePermit::new())? else {
        return Ok(None);
    };
    if status.barrier == ott::Barrier::Pending {
        return Err(Error::Revoked);
    }
    let snapshot = remote::load_with(transport)
        .map_err(client_error)?
        .ok_or(Error::Store(StoreError::HelperProtocol))?;
    slot(&snapshot, namespace)
        .as_ref()
        .map(|value| decode(namespace, value, open))
        .transpose()
}

pub(super) fn load(namespace: Namespace) -> Result<Option<Value>, Error> {
    let _writer = WRITER.lock().unwrap_or_else(|e| e.into_inner());
    load_with(
        &mut client::NativeTransport,
        &mut Files,
        namespace,
        secure::open_named,
    )
}

#[cfg(test)]
fn save_with(
    transport: &mut dyn Transport,
    legacy: &mut dyn Legacy,
    namespace: Namespace,
    value: &Value,
    permit: &WritePermit,
    seal: Seal,
    open: Open,
) -> Result<CommitReceipt, Error> {
    save_with_sealer(transport, legacy, namespace, value, permit,
        |namespace, bytes| seal(namespace, bytes).ok_or(Error::ProtectionUnavailable), open)
}

fn save_with_detailed(
    transport: &mut dyn Transport, legacy: &mut dyn Legacy, namespace: Namespace,
    value: &Value, permit: &WritePermit,
    seal: fn(Namespace, &[u8]) -> Result<Sealed, wire::KeymanagerFailure>, open: Open,
) -> Result<CommitReceipt, Error> {
    // Only a failure observed during this same writer operation may be attached.
    // The returned value owns the evidence before another transaction clears TLS.
    wire::failure::clear();
    save_with_sealer(transport, legacy, namespace, value, permit,
        |namespace, bytes| seal(namespace, bytes).map_err(Error::ProtectionFailed), open)
        .map_err(|error| match error {
            Error::Store(source) => wire::failure::last()
                .map_or(Error::Store(source), |failure| Error::HelperFailed { source, failure }),
            error => error,
        })
}

fn save_with_sealer(
    transport: &mut dyn Transport, legacy: &mut dyn Legacy, namespace: Namespace,
    value: &Value, permit: &WritePermit,
    seal: impl FnOnce(Namespace, &[u8]) -> Result<Sealed, Error>, open: Open,
) -> Result<CommitReceipt, Error> {
    if !permit.is_valid() {
        return Err(Error::Revoked);
    }
    let status = ready(transport, legacy, open, permit)?;
    if status
        .as_ref()
        .is_some_and(|s| s.barrier == ott::Barrier::Pending)
    {
        return Err(Error::Revoked);
    }
    if !permit.is_valid() {
        return Err(Error::Revoked);
    }
    let encrypted = encode_with(namespace, value, seal)?;
    let mutation = if status.is_some() {
        ott::Mutation::Put {
            namespace: wire_namespace(namespace),
            value: encrypted.clone(),
        }
    } else {
        let (control, state) = match namespace {
            Namespace::OttControl => (Some(encrypted.clone()), None),
            Namespace::OttState => (None, Some(encrypted.clone())),
        };
        ott::Mutation::Initialize {
            control,
            state,
            barrier: ott::Barrier::Open,
        }
    };
    let mut outcome = commit(transport, status.as_ref(), mutation, permit)?;
    if status.is_none() && outcome == remote::Outcome::Conflict {
        // Two fresh clients may initialize different namespaces simultaneously.
        // Repair this one admission race without replaying over a newer value or
        // across Erase: its first Open epoch and our absent slot must still hold.
        if !permit.is_valid() {
            return Err(Error::Revoked);
        }
        let mut winner = remote::load_with(transport)
            .map_err(client_error)?
            .ok_or(Error::Store(StoreError::Conflict))?;
        if !winner.status.private_initialized {
            let winner_status = ready(transport, legacy, open, permit)?
                .ok_or(Error::Store(StoreError::Conflict))?;
            winner = remote::load_with(transport).map_err(client_error)?
                .ok_or(Error::Store(StoreError::Conflict))?;
            if winner.status != winner_status { return Err(Error::Store(StoreError::Conflict)); }
        }
        if winner.status.barrier != ott::Barrier::Open
            || winner.status.epoch != 1
            || slot(&winner, namespace).is_some()
        {
            return Err(Error::Store(StoreError::Conflict));
        }
        outcome = commit(
            transport,
            Some(&winner.status),
            ott::Mutation::Put {
                namespace: wire_namespace(namespace),
                value: encrypted.clone(),
            },
            permit,
        )?;
    }
    let receipt = match outcome {
        remote::Outcome::Applied(receipt) => receipt,
        remote::Outcome::Uncertain => return Ok(uncertain()),
        remote::Outcome::Conflict => return Err(Error::Store(StoreError::Conflict)),
        remote::Outcome::Revoked => return Err(Error::Revoked),
    };
    // After dispatch every failed check is uncertain, never definitely rejected.
    let verified = (|| {
        let snapshot = remote::load_with(transport)
            .map_err(client_error)?
            .ok_or(Error::Invalid)?;
        if snapshot.status.epoch != receipt.epoch
            || snapshot.status.revision < receipt.revision
            || slot(&snapshot, namespace).as_ref() != Some(&encrypted)
            || decode(namespace, &encrypted, open)? != *value
        {
            return Err(Error::Invalid);
        }
        Ok(())
    })();
    Ok(if verified.is_ok() {
        CommitReceipt::Durable
    } else {
        uncertain()
    })
}

pub(super) fn save(
    namespace: Namespace,
    value: &Value,
    permit: &WritePermit,
) -> Result<CommitReceipt, Error> {
    let _writer = WRITER.lock().unwrap_or_else(|e| e.into_inner());
    save_with_detailed(
        &mut client::NativeTransport,
        &mut Files,
        namespace,
        value,
        permit,
        secure::seal_named_detailed,
        secure::open_named,
    )
}

fn barrier_with(
    transport: &mut dyn Transport,
    legacy: &mut dyn Legacy,
) -> Result<ott::Barrier, Error> {
    match remote::status_with(transport).map_err(client_error)? {
        Some(status) if status.private_initialized => Ok(status.barrier),
        Some(_) | None => legacy.barrier(),
    }
}
pub(super) fn imports_suppressed() -> Result<bool, Error> {
    let _writer = WRITER.lock().unwrap_or_else(|e| e.into_inner());
    barrier_with(&mut client::NativeTransport, &mut Files).map(|b| b != ott::Barrier::Open)
}
pub(super) fn erasure_pending() -> Result<bool, Error> {
    let _writer = WRITER.lock().unwrap_or_else(|e| e.into_inner());
    barrier_with(&mut client::NativeTransport, &mut Files).map(|b| b == ott::Barrier::Pending)
}

fn begin_with(transport: &mut dyn Transport) -> Result<(), Error> {
    let status = remote::status_with(transport).map_err(client_error)?;
    if status
        .as_ref()
        .is_some_and(|s| s.barrier == ott::Barrier::Pending)
    {
        return Ok(());
    }
    require_applied(commit(
        transport,
        status.as_ref(),
        ott::Mutation::BeginErase {},
        &WritePermit::new(),
    )?)?;
    let next = remote::status_with(transport)
        .map_err(client_error)?
        .ok_or(Error::Invalid)?;
    if next.barrier != ott::Barrier::Pending || next.control_present || next.state_present {
        return Err(Error::Invalid);
    }
    Ok(())
}
pub(super) fn begin_erasure() -> Result<(), Error> {
    let _writer = WRITER.lock().unwrap_or_else(|e| e.into_inner());
    begin_with(&mut client::NativeTransport)
}
pub(super) fn erase_owned_pending() -> Vec<String> {
    let _writer = WRITER.lock().unwrap_or_else(|e| e.into_inner());
    if let Err(error) = begin_with(&mut client::NativeTransport) {
        return vec![format!("OTT canonical erasure barrier: {error:?}")];
    }
    Files.erase()
}
fn finish_with(transport: &mut dyn Transport, legacy: &mut dyn Legacy) -> Result<(), Error> {
    let status = remote::status_with(transport)
        .map_err(client_error)?
        .ok_or(Error::Invalid)?;
    if status.barrier != ott::Barrier::Pending {
        return Err(Error::Invalid);
    }
    if !legacy.erase().is_empty() {
        return Err(Error::Store(StoreError::RootChanged));
    }
    require_applied(commit(
        transport,
        Some(&status),
        ott::Mutation::FinishErase {},
        &WritePermit::new(),
    )?)?;
    let next = remote::status_with(transport)
        .map_err(client_error)?
        .ok_or(Error::Invalid)?;
    if next.barrier != ott::Barrier::Complete || next.control_present || next.state_present {
        return Err(Error::Invalid);
    }
    Ok(())
}
pub(super) fn finish_erasure() -> Result<(), Error> {
    let _writer = WRITER.lock().unwrap_or_else(|e| e.into_inner());
    finish_with(&mut client::NativeTransport, &mut Files)
}

// Public reads do not decrypt or initialize private state. A legacy pending
// erase still denies access before the canonical private-import marker is set.
fn public_current(transport: &mut dyn Transport, legacy: &mut dyn Legacy)
    -> Result<Option<ott::Snapshot>, Error>
{
    let snapshot = remote::load_with(transport).map_err(client_error)?;
    if snapshot.as_ref().is_none_or(|snapshot| !snapshot.status.private_initialized)
        && legacy.barrier()? == ott::Barrier::Pending
    { return Err(Error::Revoked); }
    Ok(snapshot)
}
fn public_snapshot(snapshot: &ott::Snapshot) -> Option<public_preferences::Snapshot> {
    snapshot.public_preferences.clone().map(|value| public_preferences::Snapshot {
        value, revision: snapshot.status.revision, epoch: snapshot.status.epoch,
    })
}
pub(super) fn public_load() -> Result<Option<public_preferences::Snapshot>, Error> {
    let _writer = WRITER.lock().unwrap_or_else(|error| error.into_inner());
    public_current(&mut client::NativeTransport, &mut Files)
        .map(|snapshot| snapshot.as_ref().and_then(public_snapshot))
}
fn public_patch_with(
    transport: &mut dyn Transport, legacy: &mut dyn Legacy,
    patch: ott::PublicPatch, permit: &WritePermit,
) -> Result<(public_preferences::Snapshot, CommitReceipt), public_preferences::SaveError> {
    use public_preferences::SaveError;
    patch.validate().map_err(|_| Error::Invalid)?;
    if !permit.is_valid() { return Err(Error::Revoked.into()); }
    let before = public_current(transport, legacy)?;
    let expected_value = before.as_ref().and_then(|snapshot| snapshot.public_preferences.clone())
        .unwrap_or_default().apply(&patch).map_err(|_| Error::Invalid)?;
    let outcome = commit(transport, before.as_ref().map(|snapshot| &snapshot.status),
        ott::Mutation::PatchPublic { patch }, permit)?;
    let receipt = match outcome {
        remote::Outcome::Applied(receipt) => receipt,
        remote::Outcome::Uncertain => return Err(SaveError::Uncertain),
        remote::Outcome::Conflict => return Err(Error::Store(StoreError::Conflict).into()),
        remote::Outcome::Revoked => return Err(Error::Revoked.into()),
    };
    let after = remote::load_with(transport).map_err(|_| SaveError::Uncertain)?
        .ok_or(SaveError::Uncertain)?;
    let observed = public_snapshot(&after).ok_or(SaveError::Uncertain)?;
    // Exact latest revision excludes a superseding write/erase; a historical
    // operation receipt alone cannot publish a stale preference or identity.
    if observed.revision != receipt.revision || observed.epoch != receipt.epoch
        || observed.value != expected_value || !permit.is_valid()
    { return Err(SaveError::Uncertain); }
    Ok((observed, CommitReceipt::Durable))
}
pub(super) fn public_patch(patch: ott::PublicPatch, permit: &WritePermit)
    -> Result<(public_preferences::Snapshot, CommitReceipt), public_preferences::SaveError>
{
    let _writer = WRITER.lock().unwrap_or_else(|error| error.into_inner());
    public_patch_with(&mut client::NativeTransport, &mut Files, patch, permit)
}

#[cfg(test)]
#[path = "private_helper_tests.rs"]
mod tests;

#[cfg(test)]
mod control_import_tests {
    use super::*;

    struct LegacyControl;
    impl Legacy for LegacyControl {
        fn read(&mut self) -> Result<LegacyImage, Error> {
            Ok(LegacyImage { present: true, barrier: ott::Barrier::Open, state: None,
                control: Some(SecretString(serde_json::json!({"version":1,"sealed":{
                    "backend":"keymanager3", "key":Namespace::OttControl.key_name(),
                    "iv":"AQ==", "data":"Ag=="
                }}).to_string())) })
        }
        fn barrier(&mut self) -> Result<ott::Barrier, Error> { Ok(ott::Barrier::Open) }
        fn erase(&mut self) -> Vec<String> { panic!("import must retain legacy files") }
    }
    fn open_invalid_control(namespace: Namespace, _: &Sealed) -> Option<Vec<u8>> {
        encode_plain(namespace, &serde_json::json!({"version":9,"device_id":"fixture-device"})).ok()
    }
    fn reject_invalid_control(value: Value) -> Result<control::Config, ()> {
        assert_eq!(value["version"], 9);
        Err(())
    }

    #[test]
    fn public_only_state_does_not_skip_typed_control_validation_before_import() {
        let mut calls = 0;
        let mut transport = |request| {
            calls += 1;
            assert!(matches!(request, wire::Request::OttStatus {}));
            Ok(wire::Response::OttStatus { status: Some(ott::Status {
                db_rev: "1".into(), revision: 1, epoch: 1, barrier: ott::Barrier::Open,
                control_present: false, state_present: false, private_initialized: false,
            }) })
        };
        assert_eq!(ready_checked(&mut transport, &mut LegacyControl, open_invalid_control,
            &WritePermit::new(), Some(reject_invalid_control)), Err(Error::Invalid));
        assert_eq!(calls, 1, "invalid Control must fail before any import commit");
    }
}
