//! Bounded, authenticated OTT configuration records, separate from the Plex session.
//! On ARM the authenticated helper owns canonical encrypted records; host/test
//! installations retain JsonStore. The typed State door keeps v2 crypto in the helper;
//! retained legacy records still require authenticated application-side decryption.
//! Old encrypted files are retained after migration; only explicit Erase removes them.
use super::{
    CommitReceipt, CommitStage, JsonStore, Record, RecordKey, RecordState, RecordStore, StoreError,
};
use crate::tv::secure::{self, Namespace, Sealed};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::os::fd::AsRawFd;
use std::os::unix::fs::MetadataExt;
use std::{
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
};

const LOGICAL_BYTES: usize = 1024 * 1024;
static WRITER: Mutex<()> = Mutex::new(());
const USE_HELPER: bool = cfg!(all(target_os = "linux", target_arch = "arm",
    not(any(feature = "hostsim", test, feature = "test-support"))));
#[path = "private_helper.rs"]
mod helper;
#[path = "public_preferences.rs"]
pub mod public_preferences;
#[path = "control.rs"]
pub mod control;
#[path = "saved_state.rs"]
mod saved_state;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Store(StoreError),
    HelperFailed { source: StoreError, failure: super::wire::failure::HelperFailure },
    ProtectionUnavailable,
    ProtectionFailed(super::wire::KeymanagerFailure),
    Invalid,
    TooLarge,
    Revoked,
}
impl From<StoreError> for Error {
    fn from(value: StoreError) -> Self {
        Self::Store(value)
    }
}

/// An authenticated namespace observation. The digest covers the exact persisted
/// encrypted VALUE string, not its outer record or the decoded JSON value.
/// There is deliberately no Debug/Serialize or ciphertext accessor.
pub struct NamespaceSnapshot {
    value: Value,
    namespace_sha256: String,
}
impl NamespaceSnapshot {
    pub fn value(&self) -> &Value {
        &self.value
    }
    pub fn namespace_sha256(&self) -> &str {
        &self.namespace_sha256
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InspectError {
    Storage(Error),
    /// The canonical helper store is absent, but legacy state exists. Inspection
    /// never initializes the helper or migrates that state.
    LegacyOnly,
}
impl From<Error> for InspectError {
    fn from(value: Error) -> Self {
        Self::Storage(value)
    }
}
impl From<StoreError> for InspectError {
    fn from(value: StoreError) -> Self {
        Self::Storage(Error::Store(value))
    }
}

/// A runtime lifetime's write authority. Clone before admitting asynchronous work;
/// retirement revokes every clone, including workers already waiting for WRITER.
#[derive(Clone)]
pub struct WritePermit(Arc<AtomicBool>);
impl WritePermit {
    pub fn new() -> Self {
        Self(Arc::new(AtomicBool::new(true)))
    }
    pub fn revoke(&self) {
        self.0.store(false, Ordering::Release);
    }
    pub fn is_valid(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
}
impl Default for WritePermit {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Plain {
    version: u8,
    install: String,
    namespace: String,
    value: Value,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Envelope {
    version: u8,
    sealed: Sealed,
}

fn key(namespace: Namespace) -> RecordKey {
    match namespace {
        Namespace::OttControl => RecordKey::OttControl,
        Namespace::OttState => RecordKey::OttState,
    }
}
fn label(namespace: Namespace) -> &'static str {
    match namespace {
        Namespace::OttControl => "ott-control",
        Namespace::OttState => "ott-state",
    }
}

pub fn root() -> PathBuf {
    #[cfg(all(feature = "hostsim", feature = "devtriggers", any(target_os = "macos", target_os = "linux")))]
    if let Some(root) = secure::fixture::records_root() { return root; }
    if cfg!(any(feature = "hostsim", test, feature = "test-support")) {
        plx_base::paths::persistent_state_root().join("ott-state")
    } else {
        plx_base::paths::in_app_dir("ott-state")
    }
}

/// IO boundary; run on the storage worker, never in a screen's step.
pub fn load(namespace: Namespace) -> Result<Option<Value>, Error> {
    check_fixture_authority()?;
    if USE_HELPER {
        return match namespace {
            Namespace::OttState => saved_state::load(),
            Namespace::OttControl => helper::load(namespace),
        };
    }
    let result = load_at(&root(), namespace, secure::open_named);
    check_fixture_authority()?;
    result
}

/// Read-only IO boundary for an existing storage worker. No initialization,
/// migration, root creation or permission repair. Only an actually missing key
/// is None; a cleared, corrupt or inaccessible record is an error.
pub fn inspect(namespace: Namespace) -> Result<Option<NamespaceSnapshot>, InspectError> {
    check_fixture_authority()?;
    let result = if USE_HELPER {
        match namespace {
            Namespace::OttState => saved_state::inspect(),
            Namespace::OttControl => helper::inspect(namespace),
        }
    } else {
        inspect_at(&root(), namespace, secure::open_named)
    };
    check_fixture_authority()?;
    result
}

fn check_fixture_authority() -> Result<(), Error> {
    #[cfg(all(feature = "hostsim", feature = "devtriggers", any(target_os = "macos", target_os = "linux")))]
    if secure::fixture::active() && !secure::fixture::check() {
        return Err(Error::ProtectionUnavailable);
    }
    Ok(())
}

/// Serialized, authenticated, atomic write. `Uncertain` is not a success receipt.
/// No new write transaction starts for a revoked lifetime. Erasure shares this lock
/// and therefore also waits for any transaction already in progress.
pub fn save_guarded(
    namespace: Namespace,
    value: &Value,
    permit: &WritePermit,
) -> Result<CommitReceipt, Error> {
    if !permit.is_valid() {
        return Err(Error::Revoked);
    }
    if USE_HELPER {
        return match namespace {
            Namespace::OttState => saved_state::save(value, permit),
            Namespace::OttControl => helper::save(namespace, value, permit),
        };
    }
    ensure_parent()?;
    save_guarded_at(&root(), namespace, value, permit, secure::seal_named)
}

fn ensure_parent() -> Result<(), Error> {
    #[cfg(all(feature = "hostsim", feature = "devtriggers", any(target_os = "macos", target_os = "linux")))]
    if secure::fixture::active() {
        return if secure::fixture::check() { Ok(()) } else { Err(Error::ProtectionUnavailable) };
    }
    if cfg!(any(feature = "hostsim", test, feature = "test-support")) {
        plx_base::paths::ensure_persistent_state_root().map_err(|_| Error::Invalid)?;
        // The host state root itself may just have been created. Persist its name
        // too; syncing only the later ott-state child would leave an ancestor gap.
        let state = JsonStore::new(plx_base::paths::persistent_state_root())?;
        let parent = super::open_at(
            &state.directory,
            "..",
            libc::O_RDONLY | libc::O_DIRECTORY,
            0,
        )
        .map_err(|e| super::io_error(CommitStage::ParentOpen, e))?;
        parent
            .sync_all()
            .map_err(|e| super::io_error(CommitStage::ParentSync, e))?;
        state.check_root()?;
    }
    Ok(())
}

/// Independent of encrypted records and keymanager availability. An invalid marker
/// is an error, not permission to import; callers must fail closed on that error.
pub fn imports_suppressed() -> Result<bool, Error> {
    check_fixture_authority()?;
    if USE_HELPER { return helper::imports_suppressed(); }
    let _writer = WRITER.lock().unwrap_or_else(|error| error.into_inner());
    imports_suppressed_at(&root())
}

/// Canonical boot gate for every credential owner. Errors must also fail closed;
/// completion suppresses legacy imports but permits newly configured credentials.
pub fn erasure_pending() -> Result<bool, Error> {
    check_fixture_authority()?;
    if USE_HELPER { return helper::erasure_pending(); }
    let _writer = WRITER.lock().unwrap_or_else(|error| error.into_inner());
    erasure_pending_at(&root())
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ErasureState {
    Absent,
    Pending,
    Complete,
}

fn erasure_state(store: &JsonStore) -> Result<ErasureState, Error> {
    erasure_record(store.load(RecordKey::OttErasure)?)
}

fn erasure_record(record: Option<Record>) -> Result<ErasureState, Error> {
    match record {
        None => Ok(ErasureState::Absent),
        Some(Record {
            state: RecordState::Cleared,
            ..
        }) => Ok(ErasureState::Pending),
        Some(Record {
            state: RecordState::Data { payload },
            ..
        }) if payload == "ott-erasure-complete-v1" => Ok(ErasureState::Complete),
        Some(_) => Err(Error::Invalid),
    }
}

fn erasure_state_at(path: &Path) -> Result<ErasureState, Error> {
    match std::fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(ErasureState::Absent)
        }
        Err(_) => return Err(Error::Invalid),
        Ok(_) => {}
    }
    erasure_state(&JsonStore::new(path.to_owned())?)
}

fn imports_suppressed_at(path: &Path) -> Result<bool, Error> {
    Ok(erasure_state_at(path)? != ErasureState::Absent)
}

fn erasure_pending_at(path: &Path) -> Result<bool, Error> {
    Ok(erasure_state_at(path)? == ErasureState::Pending)
}

/// Install the durable pending barrier before clearing any other local domain.
/// Both private reads and writes remain denied until `finish_erasure` succeeds.
pub fn begin_erasure() -> Result<(), Error> {
    if USE_HELPER { return helper::begin_erasure(); }
    ensure_parent()?;
    let _writer = WRITER.lock().unwrap_or_else(|error| error.into_inner());
    let store = open_or_create(&root())?;
    durable(store.commit(RecordKey::OttErasure, &Record::cleared(1)))?;
    Ok(())
}

/// Clear private records and managed fallbacks, retaining the pending barrier.
/// Retire/revoke the old runtime first. Empty means this domain was cleared, not
/// that other owners completed. Call `finish_erasure` only after ALL owners succeed.
/// Fixed install paths deliberately ignore arbitrary external profile overrides.
pub fn erase_owned_pending() -> Vec<String> {
    if USE_HELPER { return helper::erase_owned_pending(); }
    if let Err(error) = ensure_parent() {
        return vec![format!("OTT private root: {error:?}")];
    }
    erase_pending_at(&root(), plx_base::paths::app_dir())
}

/// Release the pending barrier only after the caller has proved every owned
/// cleanup complete, including the Plex session and the managed helper.
pub fn finish_erasure() -> Result<(), Error> {
    check_fixture_authority()?;
    if USE_HELPER { return helper::finish_erasure(); }
    finish_at(&root())
}

fn finish_at(path: &Path) -> Result<(), Error> {
    let _writer = WRITER.lock().unwrap_or_else(|error| error.into_inner());
    let store = JsonStore::new(path.to_owned())?;
    if erasure_state(&store)? == ErasureState::Absent {
        return Err(Error::Invalid);
    }
    durable(store.commit(
        RecordKey::OttErasure,
        &Record::data(2, "ott-erasure-complete-v1".into()),
    ))?;
    Ok(())
}

fn erase_pending_at(path: &Path, app_dir: &Path) -> Vec<String> {
    let _writer = WRITER.lock().unwrap_or_else(|error| error.into_inner());
    let store = match open_or_create(path) {
        Ok(store) => store,
        Err(error) => return vec![format!("OTT private root: {error:?}")],
    };
    erase_store(&store, app_dir)
}

fn durable(result: Result<CommitReceipt, StoreError>) -> Result<(), StoreError> {
    match result? {
        CommitReceipt::Durable => Ok(()),
        CommitReceipt::Uncertain { stage, errno } => Err(StoreError::Io { stage, errno }),
    }
}

fn erase_store(store: &JsonStore, app_dir: &Path) -> Vec<String> {
    erase_store_with(store, || {
        let mut errors = Vec::new();
        match JsonStore::new(app_dir.to_owned()) {
            Err(error) => errors.push(format!("OTT managed fallback directory: {error:?}")),
            Ok(directory) => {
                for name in ["ottplay-control.json", "ottplay-profile.json"] {
                    if let Err(error) = remove_owned_file(&directory, name, false) {
                        errors.push(format!("OTT managed {name}: {error:?}"));
                    }
                }
            }
        }
        errors
    })
}

fn erase_store_with(store: &JsonStore, managed_cleanup: impl FnOnce() -> Vec<String>) -> Vec<String> {
    // Install the import barrier FIRST. A new encrypted namespace must never erase
    // this independent marker. Do not read/decrypt damaged prior records to clear them.
    if let Err(error) = durable(store.commit(RecordKey::OttErasure, &Record::cleared(1))) {
        return vec![format!("OTT import barrier: {error:?}")];
    }
    let mut errors = Vec::new();
    for key in [RecordKey::OttControl, RecordKey::OttState, RecordKey::OttPreferences] {
        if let Err(error) = durable(store.commit(key, &Record::cleared(1))) {
            errors.push(format!("OTT {key:?}: {error:?}"));
        }
    }
    if let Err(error) = cleanup_owned_temporaries(store) {
        errors.push(format!("OTT private temporary files: {error:?}"));
    }
    errors.extend(managed_cleanup());
    errors
}

/// Use existing fd-relative storage primitives. Unlike ordinary best-effort temp
/// cleanup, any owned-name obstruction is a failed erase and remains retryable.
fn cleanup_owned_temporaries(store: &JsonStore) -> Result<(), StoreError> {
    store.check_root()?;
    // Fresh file description: a retry must not inherit readdir's prior EOF offset.
    let directory = super::open_at(&store.directory, ".", libc::O_RDONLY | libc::O_DIRECTORY, 0)
        .map_err(|e| super::io_error(CommitStage::Cleanup, e))?;
    let names =
        super::directory_names(&directory).map_err(|e| super::io_error(CommitStage::Cleanup, e))?;
    let mut first_error = None;
    for name in names {
        let owned = [
            RecordKey::OttControl,
            RecordKey::OttState,
            RecordKey::OttErasure,
            RecordKey::OttPreferences,
        ]
        .iter()
        .any(|key| super::safe_temp_name(&name, &format!(".{}.new-", key.file_name())));
        if owned {
            if let Err(error) = remove_owned_file(store, &name, true) {
                first_error.get_or_insert(error);
            }
        }
    }
    store.check_root()?;
    match first_error {
        Some(error) => Err(error),
        None => Ok(()),
    }
}

fn remove_owned_file(store: &JsonStore, name: &str, private: bool) -> Result<(), StoreError> {
    store.check_root()?;
    match super::open_at(&store.directory, name, libc::O_RDONLY, 0) {
        Ok(file) => {
            let meta = file
                .metadata()
                .map_err(|e| super::io_error(CommitStage::Cleanup, e))?;
            if !meta.is_file() {
                return Err(StoreError::RecordNotRegular);
            }
            if meta.uid() != unsafe { libc::geteuid() } {
                return Err(StoreError::RecordWrongOwner);
            }
            if private && meta.mode() & 0o077 != 0 {
                return Err(StoreError::RecordUnsafeMode);
            }
            super::unlink_at(&store.directory, name)
                .map_err(|e| super::io_error(CommitStage::Cleanup, e))?;
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(super::io_error(CommitStage::Cleanup, error)),
    }
    // Also sync absence on a retry after an earlier successful unlink/failed sync.
    if let Some(errno) = store.take_failure(CommitStage::ParentSync) {
        return Err(StoreError::Io {
            stage: CommitStage::ParentSync,
            errno,
        });
    }
    store
        .directory
        .sync_all()
        .map_err(|e| super::io_error(CommitStage::ParentSync, e))?;
    match super::open_at(&store.directory, name, libc::O_RDONLY, 0) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(super::io_error(CommitStage::Readback, error)),
        Ok(_) => return Err(StoreError::RootChanged),
    }
    store.check_root()
}

fn open_or_create(path: &Path) -> Result<JsonStore, Error> {
    // Only create the direct child of a trusted existing parent, using its held fd.
    let parent_path = path.parent().ok_or(Error::Invalid)?;
    let name = path
        .file_name()
        .and_then(|v| v.to_str())
        .ok_or(Error::Invalid)?;
    let name = std::ffi::CString::new(name).map_err(|_| Error::Invalid)?;
    let parent = JsonStore::new(parent_path.to_owned())?;
    if unsafe { libc::mkdirat(parent.directory.as_raw_fd(), name.as_ptr(), 0o700) } != 0 {
        let error = std::io::Error::last_os_error();
        if error.kind() != std::io::ErrorKind::AlreadyExists {
            return Err(super::io_error(CommitStage::CreateTemp, error).into());
        }
    }
    let store = JsonStore::new(path.to_owned())?;
    // JsonStore syncs its own directory; persisting a new directory's name also
    // requires this parent sync. Repeat it on retries after an uncertain creation.
    parent
        .directory
        .sync_all()
        .map_err(|e| super::io_error(CommitStage::ParentSync, e))?;
    parent.check_root()?;
    store.check_root()?;
    Ok(store)
}

fn load_at(
    path: &Path,
    namespace: Namespace,
    open: fn(Namespace, &Sealed) -> Option<Vec<u8>>,
) -> Result<Option<Value>, Error> {
    let _writer = WRITER.lock().unwrap_or_else(|error| error.into_inner());
    match std::fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(Error::Invalid),
        Ok(_) => {}
    }
    let store = JsonStore::new(path.to_owned())?;
    if erasure_state(&store)? == ErasureState::Pending {
        return Err(Error::Revoked);
    }
    let Some(record) = store.load(key(namespace))? else {
        return Ok(None);
    };
    let RecordState::Data { payload } = record.state else {
        return Ok(None);
    };
    decode_payload(namespace, &payload, open)
}

fn inspect_at(
    path: &Path,
    namespace: Namespace,
    open: fn(Namespace, &Sealed) -> Option<Vec<u8>>,
) -> Result<Option<NamespaceSnapshot>, InspectError> {
    let _writer = WRITER.lock().unwrap_or_else(|error| error.into_inner());
    match std::fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(Error::Invalid.into()),
        Ok(_) => {}
    }
    let store = JsonStore::new(path.to_owned())?;
    if erasure_record(store.load_readonly(RecordKey::OttErasure)?)? == ErasureState::Pending {
        return Err(Error::Revoked.into());
    }
    let result = match store.load_readonly(key(namespace))? {
        None => None,
        Some(Record { state: RecordState::Cleared, .. }) => return Err(Error::Invalid.into()),
        Some(Record { state: RecordState::Data { payload }, .. }) => {
            Some(inspect_payload(namespace, &payload, open)?)
        }
    };
    store.check_root()?;
    Ok(result)
}

fn inspect_payload(
    namespace: Namespace,
    payload: &str,
    open: fn(Namespace, &Sealed) -> Option<Vec<u8>>,
) -> Result<NamespaceSnapshot, Error> {
    let value = decode_payload(namespace, payload, open)?.ok_or(Error::Invalid)?;
    Ok(NamespaceSnapshot {
        value,
        namespace_sha256: super::state::digest_bytes(payload.as_bytes()),
    })
}

#[cfg(test)]
#[path = "private_snapshot_tests.rs"]
mod snapshot_tests;

fn decode_payload(namespace: Namespace, payload: &str,
    open: fn(Namespace, &Sealed) -> Option<Vec<u8>>) -> Result<Option<Value>, Error>
{
    let outer: Envelope = serde_json::from_str(payload).map_err(|_| Error::Invalid)?;
    if outer.version != 1 || outer.sealed.key != namespace.key_name() {
        return Err(Error::Invalid);
    }
    let bytes = open(namespace, &outer.sealed).ok_or(Error::ProtectionUnavailable)?;
    if bytes.len() > LOGICAL_BYTES {
        return Err(Error::TooLarge);
    }
    let plain: Plain = serde_json::from_slice(&bytes).map_err(|_| Error::Invalid)?;
    if plain.version != 1
        || plain.install != plx_base::paths::app_id()
        || plain.namespace != label(namespace)
    {
        return Err(Error::Invalid);
    }
    Ok(Some(plain.value))
}

#[cfg(all(feature = "hostsim", feature = "devtriggers", any(target_os = "macos", target_os = "linux")))]
pub(crate) fn fixture_commit_receipt(receipt: CommitReceipt, authority_valid: bool) -> CommitReceipt {
    match (receipt, authority_valid) {
        (CommitReceipt::Durable, false) => CommitReceipt::Uncertain {
            stage: CommitStage::Readback, errno: libc::ESTALE,
        },
        (receipt, _) => receipt,
    }
}

/// Validate exactly the logical serialization used by both private save paths.
/// This is pure: it does not seal, access keys, create a root or write a record.
/// Success establishes only the existing logical size/serialization bound;
/// encryption, envelope limits, authority and durable storage can still fail.
pub fn validate_value(namespace: Namespace, value: &Value) -> Result<(), Error> {
    encode_plain(namespace, value).map(|_| ())
}

fn encode_plain(namespace: Namespace, value: &Value) -> Result<Vec<u8>, Error> {
    let plain = Plain {
        version: 1,
        install: plx_base::paths::app_id().to_owned(),
        namespace: label(namespace).to_owned(),
        value: value.clone(),
    };
    let bytes = serde_json::to_vec(&plain).map_err(|_| Error::Invalid)?;
    if bytes.len() > LOGICAL_BYTES {
        return Err(Error::TooLarge);
    }
    Ok(bytes)
}

#[cfg(test)]
#[path = "private_value_tests.rs"]
mod value_tests;

fn save_guarded_at(
    path: &Path,
    namespace: Namespace,
    value: &Value,
    permit: &WritePermit,
    seal: fn(Namespace, &[u8]) -> Option<Sealed>,
) -> Result<CommitReceipt, Error> {
    let bytes = encode_plain(namespace, value)?;
    let _writer = WRITER.lock().unwrap_or_else(|error| error.into_inner());
    if !permit.is_valid() {
        return Err(Error::Revoked);
    }
    let store = open_or_create(path)?;
    if erasure_state(&store)? == ErasureState::Pending {
        return Err(Error::Revoked);
    }
    let revision = store
        .load(key(namespace))?
        .map_or(0, |record| record.revision)
        .checked_add(1)
        .ok_or(Error::Invalid)?;
    let sealed = seal(namespace, &bytes).ok_or(Error::ProtectionUnavailable)?;
    let accepted_backend = sealed.backend == secure::Backend::Keymanager3;
    #[cfg(all(feature = "hostsim", feature = "devtriggers", any(target_os = "macos", target_os = "linux")))]
    let accepted_backend = if secure::fixture::active() {
        sealed.backend == secure::Backend::HostFixture && secure::fixture::check()
    } else { accepted_backend };
    if !accepted_backend || sealed.key != namespace.key_name() {
        return Err(Error::ProtectionUnavailable);
    }
    let payload =
        serde_json::to_string(&Envelope { version: 1, sealed }).map_err(|_| Error::Invalid)?;
    let receipt = store.commit(key(namespace), &Record::data(revision, payload))?;
    // Already committed bytes must never be reported NotApplied merely because
    // fixture authority was replaced during IO. The existing unknown-outcome
    // recovery owns this case; retain any earlier, more precise uncertainty.
    #[cfg(all(feature = "hostsim", feature = "devtriggers", any(target_os = "macos", target_os = "linux")))]
    let receipt = fixture_commit_receipt(receipt,
        !secure::fixture::active() || secure::fixture::check());
    Ok(receipt)
}

#[cfg(test)]
fn erase_at(path: &Path, app_dir: &Path) -> Vec<String> {
    let mut errors = erase_pending_at(path, app_dir);
    if errors.is_empty() {
        if let Err(error) = finish_at(path) {
            errors.push(format!("OTT erasure completion: {error:?}"));
        }
    }
    errors
}

#[cfg(test)]
fn save_at(
    path: &Path,
    namespace: Namespace,
    value: &Value,
    seal: fn(Namespace, &[u8]) -> Option<Sealed>,
) -> Result<CommitReceipt, Error> {
    save_guarded_at(path, namespace, value, &WritePermit::new(), seal)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    fn seal(namespace: Namespace, bytes: &[u8]) -> Option<Sealed> {
        // Synthetic transport tests record framing and domain separation, not cryptography.
        Some(Sealed {
            backend: secure::Backend::Keymanager3,
            key: namespace.key_name(),
            iv: "fixture".into(),
            data: plx_base::b64::encode(bytes),
        })
    }
    fn open(namespace: Namespace, sealed: &Sealed) -> Option<Vec<u8>> {
        (namespace.key_name() == sealed.key)
            .then(|| plx_base::b64::decode(&sealed.data))
            .flatten()
    }
    fn directory(tag: &str) -> PathBuf {
        plx_base::testscratch::process_dir(tag).join("private")
    }
    #[test]
    fn independent_namespaces_roundtrip_and_have_private_permissions() {
        let dir = directory("ott-private-roundtrip");
        let a = serde_json::json!({"token":"fixture-a"});
        let b = serde_json::json!({"profiles":[1,2]});
        assert_eq!(
            save_at(&dir, Namespace::OttControl, &a, seal),
            Ok(CommitReceipt::Durable)
        );
        assert_eq!(
            save_at(&dir, Namespace::OttState, &b, seal),
            Ok(CommitReceipt::Durable)
        );
        assert_eq!(load_at(&dir, Namespace::OttControl, open), Ok(Some(a)));
        assert_eq!(load_at(&dir, Namespace::OttState, open), Ok(Some(b)));
        assert_eq!(
            std::fs::metadata(dir.join("ott-control.json"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        let body = std::fs::read_to_string(dir.join("ott-control.json")).unwrap();
        assert!(!body.contains("fixture-a"));
    }
    #[test]
    fn interface_palette_survives_durable_store_reopen_and_revoked_overwrite() {
        // This is the production host storage path with synthetic protection,
        // not an App/TV restart or a keymanager cryptography test. Keep the
        // complete canonical Ocean value in the application's role order.
        let palette = r##"{"version":1,"colors":{"background":"#061826","surface":"#0C293D","text":"#EFFAFF","muted":"#A3C4D5","accent":"#5EEAD4","selection":"#155E75","selectionText":"#FFFFFF","border":"#3B6C83","epgTime":"#5EEAD4"}}"##;
        let dir = directory("ott-private-palette-reopen");
        assert_eq!(load_at(&dir, Namespace::OttState, open), Ok(None));
        {
            let permit = WritePermit::new();
            let candidate = serde_json::json!({"version":1,"settings":{
                "uiColorConfig":palette,"volumeStep":7,
                "settingRevision:uiColorConfig":"palette-revision"
            }});
            assert_eq!(save_guarded_at(&dir, Namespace::OttState, &candidate, &permit, seal),
                Ok(CommitReceipt::Durable));
        }
        // The writer's candidate and authority are gone. load_at opens a fresh
        // JsonStore/file descriptor and decodes the committed disk envelope.
        let disk_before = std::fs::read(dir.join("ott-state.json")).unwrap();
        assert!(!disk_before.is_empty());
        {
            let reloaded = load_at(&dir, Namespace::OttState, open).unwrap().unwrap();
            let encoded = reloaded["settings"]["uiColorConfig"].as_str().unwrap();
            assert_eq!(encoded, palette, "canonical color bytes must survive reopen");
            let decoded: Value = serde_json::from_str(encoded).unwrap();
            assert_eq!(decoded["version"], 1);
            assert_eq!(decoded["colors"].as_object().unwrap().len(), 9);
            for (role, hex) in [
                ("background", "#061826"), ("surface", "#0C293D"),
                ("text", "#EFFAFF"), ("muted", "#A3C4D5"),
                ("accent", "#5EEAD4"), ("selection", "#155E75"),
                ("selectionText", "#FFFFFF"), ("border", "#3B6C83"),
                ("epgTime", "#5EEAD4"),
            ] {
                assert_eq!(decoded["colors"][role], hex, "lost color role: {role}");
            }
            assert_eq!(reloaded["settings"]["volumeStep"], 7);
            assert_eq!(reloaded["settings"]["settingRevision:uiColorConfig"], "palette-revision");
        }
        let retired = WritePermit::new();
        retired.revoke();
        assert_eq!(save_guarded_at(&dir, Namespace::OttState,
            &serde_json::json!({"version":1,"settings":{"uiColorConfig":""}}),
            &retired, seal), Err(Error::Revoked));
        assert_eq!(std::fs::read(dir.join("ott-state.json")).unwrap(), disk_before,
            "a retired instance cannot erase the saved palette");
        assert_eq!(load_at(&dir, Namespace::OttState, open).unwrap().unwrap()
            ["settings"]["uiColorConfig"], palette);
        // A different storage root still inherits; no process-global value is
        // sufficient to satisfy the reopen check.
        let fresh = directory("ott-private-palette-fresh");
        assert_eq!(load_at(&fresh, Namespace::OttState, open), Ok(None));
    }
    #[test]
    fn denied_protection_preserves_old_record_and_never_writes_plaintext() {
        let dir = directory("ott-private-denied");
        let old = serde_json::json!({"token":"old"});
        save_at(&dir, Namespace::OttControl, &old, seal).unwrap();
        let before = std::fs::read(dir.join("ott-control.json")).unwrap();
        assert_eq!(
            save_at(
                &dir,
                Namespace::OttControl,
                &serde_json::json!({"token":"new"}),
                |_, _| None
            ),
            Err(Error::ProtectionUnavailable)
        );
        assert_eq!(std::fs::read(dir.join("ott-control.json")).unwrap(), before);
        assert_eq!(load_at(&dir, Namespace::OttControl, open), Ok(Some(old)));
    }
    #[test]
    fn swapped_domain_and_symlink_root_are_refused() {
        let dir = directory("ott-private-swap");
        save_at(&dir, Namespace::OttControl, &Value::Null, seal).unwrap();
        let source = dir.join("ott-control.json");
        std::fs::copy(source, dir.join("ott-state.json")).unwrap();
        assert!(load_at(&dir, Namespace::OttState, open).is_err());
        let alias = dir.with_extension("alias");
        std::os::unix::fs::symlink(&dir, &alias).unwrap();
        assert!(save_at(&alias, Namespace::OttControl, &Value::Null, seal).is_err());
    }

    fn app_directory(private: &Path) -> PathBuf {
        let app = private.parent().unwrap().join("app");
        std::fs::create_dir(&app).unwrap();
        std::fs::set_permissions(&app, std::fs::Permissions::from_mode(0o700)).unwrap();
        app
    }

    #[test]
    fn revoked_worker_waiting_for_writer_cannot_recreate_erased_records() {
        let dir = directory("ott-private-revoked-worker");
        let app = app_directory(&dir);
        let permit = WritePermit::new();
        let worker_permit = permit.clone();
        let worker_dir = dir.clone();
        let (ready, admitted) = std::sync::mpsc::channel();
        let writer = WRITER.lock().unwrap_or_else(|e| e.into_inner());
        let worker = std::thread::spawn(move || {
            ready.send(()).unwrap();
            save_guarded_at(
                &worker_dir,
                Namespace::OttState,
                &Value::Null,
                &worker_permit,
                |_, _| panic!("revoked work must not reach keymanager"),
            )
        });
        admitted
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap();
        let revoking_permit = permit.clone();
        let (revoked, revocation) = std::sync::mpsc::channel();
        let revoker = std::thread::spawn(move || {
            revoking_permit.revoke();
            let _ = revoked.send(());
        });
        let outcome = revocation.recv_timeout(std::time::Duration::from_secs(5));
        // Always release WRITER before assertions/join, even if revoke regresses
        // into a blocking operation. A failed regression must not hang the suite.
        drop(writer);
        assert!(
            outcome.is_ok(),
            "revocation blocked behind a storage transaction"
        );
        revoker.join().unwrap();
        assert_eq!(worker.join().unwrap(), Err(Error::Revoked));
        assert!(!dir.exists(), "revoked writer created a storage directory");
        assert_eq!(erasure_pending_at(&dir), Ok(false));
        assert!(erase_at(&dir, &app).is_empty());
        assert_eq!(
            save_guarded_at(&dir, Namespace::OttState, &Value::Null, &permit, seal),
            Err(Error::Revoked)
        );
        assert_eq!(load_at(&dir, Namespace::OttState, open), Ok(None));
    }

    #[test]
    fn corrupt_encrypted_records_are_erased_without_keys_and_marker_survives_new_saves() {
        let dir = directory("ott-private-keyless-erase");
        let app = app_directory(&dir);
        save_at(&dir, Namespace::OttControl, &Value::Null, seal).unwrap();
        for name in ["ott-control.json", "ott-state.json"] {
            std::fs::write(dir.join(name), b"damaged encrypted record").unwrap();
            std::fs::set_permissions(dir.join(name), std::fs::Permissions::from_mode(0o666))
                .unwrap();
        }
        for name in ["ottplay-control.json", "ottplay-profile.json"] {
            std::fs::write(app.join(name), b"old managed credentials").unwrap();
        }
        let external = dir.parent().unwrap().join("external-profile.json");
        std::fs::write(&external, b"external override must survive").unwrap();
        assert!(erase_at(&dir, &app).is_empty());
        assert_eq!(imports_suppressed_at(&dir), Ok(true));
        assert_eq!(
            load_at(&dir, Namespace::OttControl, |_, _| panic!(
                "must not decrypt"
            )),
            Ok(None)
        );
        assert_eq!(
            load_at(&dir, Namespace::OttState, |_, _| panic!("must not decrypt")),
            Ok(None)
        );
        assert!(!app.join("ottplay-control.json").exists());
        assert!(!app.join("ottplay-profile.json").exists());
        assert_eq!(
            std::fs::read(&external).unwrap(),
            b"external override must survive"
        );
        for namespace in [Namespace::OttControl, Namespace::OttState] {
            let value = serde_json::json!({"new":true});
            assert_eq!(
                save_guarded_at(&dir, namespace, &value, &WritePermit::new(), seal),
                Ok(CommitReceipt::Durable)
            );
            assert_eq!(load_at(&dir, namespace, open), Ok(Some(value)));
            assert_eq!(imports_suppressed_at(&dir), Ok(true));
        }
    }

    #[test]
    fn pending_erasure_blocks_surviving_private_record_reads_and_writes_until_retry() {
        let dir = directory("ott-private-partial-erase");
        let app = app_directory(&dir);
        let old = serde_json::json!({"credential":"old"});
        save_at(&dir, Namespace::OttState, &old, seal).unwrap();
        let external = dir.parent().unwrap().join("surviving-encrypted-record");
        std::fs::rename(dir.join("ott-state.json"), &external).unwrap();
        let before = std::fs::read(&external).unwrap();
        std::os::unix::fs::symlink(&external, dir.join("ott-state.json")).unwrap();
        assert!(!erase_at(&dir, &app).is_empty());
        assert_eq!(imports_suppressed_at(&dir), Ok(true));
        // Restore the old regular file as if a failed filesystem mutation survived reboot.
        std::fs::remove_file(dir.join("ott-state.json")).unwrap();
        std::fs::copy(&external, dir.join("ott-state.json")).unwrap();
        assert_eq!(
            load_at(&dir, Namespace::OttState, |_, _| panic!(
                "old record must be gated"
            )),
            Err(Error::Revoked)
        );
        assert_eq!(
            save_guarded_at(
                &dir,
                Namespace::OttState,
                &Value::Null,
                &WritePermit::new(),
                seal
            ),
            Err(Error::Revoked)
        );
        assert_eq!(std::fs::read(&external).unwrap(), before);
        assert!(erase_at(&dir, &app).is_empty());
        assert_eq!(load_at(&dir, Namespace::OttState, open), Ok(None));
        assert_eq!(
            save_guarded_at(
                &dir,
                Namespace::OttState,
                &Value::Null,
                &WritePermit::new(),
                seal
            ),
            Ok(CommitReceipt::Durable)
        );
        assert_eq!(imports_suppressed_at(&dir), Ok(true));
    }

    #[test]
    fn uncertain_import_barrier_never_reports_success_or_clears_old_records() {
        let dir = directory("ott-private-barrier-sync");
        let app = app_directory(&dir);
        save_at(&dir, Namespace::OttState, &Value::Null, seal).unwrap();
        let before = std::fs::read(dir.join("ott-state.json")).unwrap();
        let errors = {
            let _writer = WRITER.lock().unwrap_or_else(|e| e.into_inner());
            let store = JsonStore::new(dir.clone()).unwrap();
            store.inject_failure(CommitStage::ParentSync);
            erase_store(&store, &app)
        };
        assert_eq!(errors.len(), 1);
        assert!(errors[0].contains("import barrier"));
        assert_eq!(std::fs::read(dir.join("ott-state.json")).unwrap(), before);
        assert_eq!(
            load_at(&dir, Namespace::OttState, open),
            Err(Error::Revoked)
        );
        assert_eq!(imports_suppressed_at(&dir), Ok(true));
        assert!(erase_at(&dir, &app).is_empty());
    }

    #[test]
    fn managed_unlink_sync_failure_is_reported_and_absence_retry_is_durable() {
        let dir = directory("ott-private-managed-sync");
        let app = app_directory(&dir);
        let path = app.join("ottplay-control.json");
        std::fs::write(&path, b"old credential").unwrap();
        let store = JsonStore::new(app).unwrap();
        store.inject_failure(CommitStage::ParentSync);
        assert!(remove_owned_file(&store, "ottplay-control.json", false).is_err());
        assert!(!path.exists());
        assert_eq!(
            remove_owned_file(&store, "ottplay-control.json", false),
            Ok(())
        );
    }

    #[test]
    fn erasure_refuses_symlink_roots_and_managed_links_without_touching_targets() {
        let dir = directory("ott-private-erase-links");
        let app = app_directory(&dir);
        let external = dir.parent().unwrap().join("external");
        std::fs::create_dir(&external).unwrap();
        let sentinel = external.join("sentinel");
        std::fs::write(&sentinel, b"untouched").unwrap();
        std::os::unix::fs::symlink(&external, &dir).unwrap();
        assert!(!erase_at(&dir, &app).is_empty());
        assert!(!external.join("ott-erasure.json").exists());
        std::fs::remove_file(&dir).unwrap();
        std::os::unix::fs::symlink(&sentinel, app.join("ottplay-control.json")).unwrap();
        assert!(!erase_at(&dir, &app).is_empty());
        assert_eq!(std::fs::read(&sentinel).unwrap(), b"untouched");
        assert_eq!(
            load_at(&dir, Namespace::OttControl, open),
            Err(Error::Revoked)
        );
        std::fs::remove_file(app.join("ottplay-control.json")).unwrap();
        assert!(erase_at(&dir, &app).is_empty());
    }

    #[test]
    fn temporary_obstructions_are_reported_and_unrelated_files_are_preserved() {
        let dir = directory("ott-private-temporary-erase");
        let app = app_directory(&dir);
        save_at(&dir, Namespace::OttState, &Value::Null, seal).unwrap();
        let owned = dir.join(".ott-state.json.new-00000000000000000000000000000001");
        std::fs::write(&owned, b"abandoned encrypted data").unwrap();
        std::fs::set_permissions(&owned, std::fs::Permissions::from_mode(0o600)).unwrap();
        let unrelated = dir.join("unrelated");
        std::fs::write(&unrelated, b"keep").unwrap();
        let link = dir.join(".ott-control.json.new-00000000000000000000000000000002");
        std::os::unix::fs::symlink(&unrelated, &link).unwrap();
        assert!(!erase_at(&dir, &app).is_empty());
        assert!(!owned.exists());
        assert!(std::fs::symlink_metadata(&link).unwrap().is_symlink());
        assert_eq!(std::fs::read(&unrelated).unwrap(), b"keep");
        std::fs::remove_file(link).unwrap();
        assert!(erase_at(&dir, &app).is_empty());
        assert_eq!(std::fs::read(unrelated).unwrap(), b"keep");
    }

    #[test]
    fn invalid_marker_fails_closed_but_explicit_erase_can_repair_it() {
        let dir = directory("ott-private-invalid-marker");
        let app = app_directory(&dir);
        save_at(&dir, Namespace::OttState, &Value::Null, seal).unwrap();
        let store = JsonStore::new(dir.clone()).unwrap();
        store
            .commit(RecordKey::OttErasure, &Record::data(1, "unknown".into()))
            .unwrap();
        assert_eq!(imports_suppressed_at(&dir), Err(Error::Invalid));
        assert_eq!(erasure_pending_at(&dir), Err(Error::Invalid));
        assert_eq!(
            load_at(&dir, Namespace::OttState, |_, _| panic!("must not decrypt")),
            Err(Error::Invalid)
        );
        assert_eq!(
            save_guarded_at(
                &dir,
                Namespace::OttState,
                &Value::Null,
                &WritePermit::new(),
                seal
            ),
            Err(Error::Invalid)
        );
        assert!(erase_at(&dir, &app).is_empty());
        assert_eq!(imports_suppressed_at(&dir), Ok(true));
        assert_eq!(load_at(&dir, Namespace::OttState, open), Ok(None));
    }

    #[test]
    fn private_cleanup_does_not_release_pending_barrier_for_other_owners() {
        let dir = directory("ott-private-global-completion");
        let app = app_directory(&dir);
        save_at(&dir, Namespace::OttState, &Value::Null, seal).unwrap();
        assert!(erase_pending_at(&dir, &app).is_empty());
        assert_eq!(erasure_pending_at(&dir), Ok(true));
        let store = JsonStore::new(dir.clone()).unwrap();
        assert!(matches!(
            store.load(RecordKey::OttState).unwrap(),
            Some(Record {
                state: RecordState::Cleared,
                ..
            })
        ));
        assert_eq!(
            load_at(&dir, Namespace::OttState, open),
            Err(Error::Revoked)
        );
        assert_eq!(
            save_guarded_at(
                &dir,
                Namespace::OttState,
                &Value::Null,
                &WritePermit::new(),
                seal
            ),
            Err(Error::Revoked)
        );
        assert_eq!(imports_suppressed_at(&dir), Ok(true));
        // A caller still waiting for helper/session cleanup must leave it here.
        finish_at(&dir).unwrap();
        assert_eq!(erasure_pending_at(&dir), Ok(false));
        assert_eq!(load_at(&dir, Namespace::OttState, open), Ok(None));
        assert_eq!(
            save_guarded_at(
                &dir,
                Namespace::OttState,
                &Value::Null,
                &WritePermit::new(),
                seal
            ),
            Ok(CommitReceipt::Durable)
        );
        assert_eq!(imports_suppressed_at(&dir), Ok(true));
        assert_eq!(erasure_pending_at(&dir), Ok(false));
    }
    #[cfg(all(feature = "hostsim", feature = "devtriggers", any(target_os = "macos", target_os = "linux")))]
    #[test]
    fn lost_fixture_authority_after_commit_is_unknown_not_not_applied() {
        assert_eq!(fixture_commit_receipt(CommitReceipt::Durable, true), CommitReceipt::Durable);
        assert_eq!(fixture_commit_receipt(CommitReceipt::Durable, false),
            CommitReceipt::Uncertain { stage: CommitStage::Readback, errno: libc::ESTALE });
        for valid in [true, false] {
            assert_eq!(fixture_commit_receipt(
                CommitReceipt::Uncertain { stage: CommitStage::ParentSync, errno: libc::EIO }, valid),
                CommitReceipt::Uncertain { stage: CommitStage::ParentSync, errno: libc::EIO });
        }
    }

}
