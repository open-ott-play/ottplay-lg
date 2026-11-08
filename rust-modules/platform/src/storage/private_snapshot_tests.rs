//! Synthetic crypto isolates framing and read-only IO; it is not Keymanager evidence.
use super::*;
use std::os::unix::fs::{symlink, PermissionsExt};

struct Directory(PathBuf);
impl Directory {
    fn new(tag: &str) -> Self {
        Self(plx_base::testscratch::process_dir(tag).join("private"))
    }
    fn record(&self) -> PathBuf {
        self.0.join("ott-state.json")
    }
}
impl Drop for Directory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn seal(namespace: Namespace, bytes: &[u8]) -> Option<Sealed> {
    Some(Sealed {
        backend: secure::Backend::Keymanager3,
        key: namespace.key_name(),
        iv: "AA==".into(),
        data: plx_base::b64::encode(bytes),
    })
}
fn open(namespace: Namespace, sealed: &Sealed) -> Option<Vec<u8>> {
    (sealed.key == namespace.key_name())
        .then(|| plx_base::b64::decode(&sealed.data))
        .flatten()
}
fn saved(dir: &Directory) -> Value {
    let value = serde_json::json!({"favorites":["fixture"],"settings":{"unrelated":null}});
    assert_eq!(
        save_at(&dir.0, Namespace::OttState, &value, seal),
        Ok(CommitReceipt::Durable)
    );
    value
}
fn payload(dir: &Directory) -> String {
    match JsonStore::new(dir.0.clone())
        .unwrap()
        .load_readonly(RecordKey::OttState)
        .unwrap()
        .unwrap()
        .state
    {
        RecordState::Data { payload } => payload,
        RecordState::Cleared => panic!("expected data"),
    }
}

#[test]
fn exact_namespace_value_is_hashed_without_outer_record_or_plaintext_reserialization() {
    let dir = Directory::new("ott-inspect-exact-value");
    let value = saved(&dir);
    let original = payload(&dir);
    let first = inspect_at(&dir.0, Namespace::OttState, open)
        .unwrap()
        .unwrap();
    assert_eq!(first.value(), &value);
    assert_eq!(
        first.namespace_sha256(),
        super::super::state::digest_bytes(original.as_bytes())
    );
    let outer = std::fs::read(dir.record()).unwrap();
    assert_ne!(
        first.namespace_sha256(),
        super::super::state::digest_bytes(&outer)
    );
    assert_eq!(std::fs::read(dir.record()).unwrap(), outer);

    // Changing only the outer record revision/formatting does not change VALUE.
    let mut record: Value = serde_json::from_slice(&outer).unwrap();
    record["revision"] = serde_json::json!(99);
    std::fs::write(dir.record(), serde_json::to_vec_pretty(&record).unwrap()).unwrap();
    let outer_only = inspect_at(&dir.0, Namespace::OttState, open)
        .unwrap()
        .unwrap();
    assert_eq!(outer_only.namespace_sha256(), first.namespace_sha256());

    // Semantically identical envelope JSON with different exact bytes is a new observation.
    let changed = format!("\n{original}\n");
    JsonStore::new(dir.0.clone())
        .unwrap()
        .commit(
            RecordKey::OttState,
            &Record {
                revision: 100,
                state: RecordState::Data {
                    payload: changed.clone(),
                },
            },
        )
        .unwrap();
    let changed_snapshot = inspect_at(&dir.0, Namespace::OttState, open)
        .unwrap()
        .unwrap();
    assert_eq!(changed_snapshot.value(), &value);
    assert_ne!(
        changed_snapshot.namespace_sha256(),
        first.namespace_sha256()
    );
    assert_eq!(
        changed_snapshot.namespace_sha256(),
        super::super::state::digest_bytes(changed.as_bytes())
    );
}

#[test]
fn absent_root_and_key_are_absent_without_creating_or_repairing_anything() {
    let dir = Directory::new("ott-inspect-absent");
    assert!(!dir.0.exists());
    assert!(inspect_at(&dir.0, Namespace::OttState, open)
        .unwrap()
        .is_none());
    assert!(!dir.0.exists());
    saved(&dir);
    let before = std::fs::read(dir.record()).unwrap();
    assert!(inspect_at(&dir.0, Namespace::OttControl, open)
        .unwrap()
        .is_none());
    assert_eq!(std::fs::read(dir.record()).unwrap(), before);
    assert!(!dir.0.join("ott-control.json").exists());
}

#[test]
fn widened_record_mode_is_rejected_without_chmod_while_normal_load_keeps_its_policy() {
    let dir = Directory::new("ott-inspect-mode");
    let value = saved(&dir);
    let before = std::fs::read(dir.record()).unwrap();
    std::fs::set_permissions(dir.record(), std::fs::Permissions::from_mode(0o644)).unwrap();
    assert!(matches!(
        inspect_at(&dir.0, Namespace::OttState, open),
        Err(InspectError::Storage(Error::Store(
            StoreError::RecordUnsafeMode
        )))
    ));
    assert_eq!(
        std::fs::metadata(dir.record()).unwrap().mode() & 0o777,
        0o644
    );
    assert_eq!(std::fs::read(dir.record()).unwrap(), before);
    assert_eq!(
        load_at(&dir.0, Namespace::OttState, open).unwrap(),
        Some(value)
    );
    assert_eq!(
        std::fs::metadata(dir.record()).unwrap().mode() & 0o777,
        0o600
    );
}

#[test]
fn cleared_corrupt_and_unauthenticated_records_never_report_absence() {
    let dir = Directory::new("ott-inspect-invalid");
    saved(&dir);
    assert!(matches!(
        inspect_at(&dir.0, Namespace::OttState, |_, _| None),
        Err(InspectError::Storage(Error::ProtectionUnavailable))
    ));
    let store = JsonStore::new(dir.0.clone()).unwrap();
    store
        .commit(
            RecordKey::OttState,
            &Record {
                revision: 2,
                state: RecordState::Cleared,
            },
        )
        .unwrap();
    assert!(matches!(
        inspect_at(&dir.0, Namespace::OttState, open),
        Err(InspectError::Storage(Error::Invalid))
    ));
    store
        .commit(
            RecordKey::OttState,
            &Record {
                revision: 3,
                state: RecordState::Data {
                    payload: "broken envelope".into(),
                },
            },
        )
        .unwrap();
    assert!(matches!(
        inspect_at(&dir.0, Namespace::OttState, open),
        Err(InspectError::Storage(Error::Invalid))
    ));
    std::fs::write(dir.record(), b"not an outer record").unwrap();
    assert!(matches!(
        inspect_at(&dir.0, Namespace::OttState, open),
        Err(InspectError::Storage(Error::Store(_)))
    ));
}

#[test]
fn erasure_barrier_is_checked_without_repair_and_pending_remains_revoked() {
    let dir = Directory::new("ott-inspect-erasure");
    saved(&dir);
    let store = JsonStore::new(dir.0.clone()).unwrap();
    store
        .commit(
            RecordKey::OttErasure,
            &Record {
                revision: 1,
                state: RecordState::Cleared,
            },
        )
        .unwrap();
    assert!(matches!(
        inspect_at(&dir.0, Namespace::OttState, open),
        Err(InspectError::Storage(Error::Revoked))
    ));
    let barrier = dir.0.join(RecordKey::OttErasure.file_name());
    std::fs::set_permissions(&barrier, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert!(matches!(
        inspect_at(&dir.0, Namespace::OttState, open),
        Err(InspectError::Storage(Error::Store(
            StoreError::RecordUnsafeMode
        )))
    ));
    assert_eq!(std::fs::metadata(&barrier).unwrap().mode() & 0o777, 0o644);
}

#[test]
fn symlink_and_nonregular_record_are_errors_without_following_or_blocking() {
    let dir = Directory::new("ott-inspect-nonregular");
    saved(&dir);
    let outside = dir.0.join("outside");
    std::fs::rename(dir.record(), &outside).unwrap();
    let before = std::fs::read(&outside).unwrap();
    symlink(&outside, dir.record()).unwrap();
    assert!(inspect_at(&dir.0, Namespace::OttState, open).is_err());
    assert_eq!(std::fs::read(&outside).unwrap(), before);
    std::fs::remove_file(dir.record()).unwrap();
    std::fs::create_dir(dir.record()).unwrap();
    assert!(matches!(
        inspect_at(&dir.0, Namespace::OttState, open),
        Err(InspectError::Storage(Error::Store(
            StoreError::RecordNotRegular
        )))
    ));
}
