//! Device-backed protection for the persisted Plex session, as the storage layer sees it: seal a
//! secret, open it again, drop its key. The platform's key manager is behind the port; the
//! envelope types live here so the on-disk shape does not depend on which platform sealed it.
use serde::{Deserialize, Serialize};

#[cfg(all(feature = "hostsim", feature = "devtriggers", any(target_os = "macos", target_os = "linux")))]
pub mod fixture;

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Backend {
    Keymanager3,
    PalmKeymanager,
    #[cfg(all(feature = "hostsim", feature = "devtriggers", any(target_os = "macos", target_os = "linux")))]
    HostFixture,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct Sealed {
    pub backend: Backend,
    pub key: String,
    pub iv: String,
    pub data: String,
}

/// A host test's replacement for the platform key manager, installed per thread.
#[cfg(any(test, feature = "test-support"))]
pub struct TestStore {
    pub seal: fn(&[u8]) -> Option<Sealed>,
    pub open: fn(&Sealed) -> Option<Vec<u8>>,
    pub remove: fn(&Backend, &str),
}

#[cfg(any(test, feature = "test-support"))]
std::thread_local! {
    pub static STORE_FOR_TEST: std::cell::Cell<Option<&'static TestStore>> =
        const { std::cell::Cell::new(None) };
}

pub fn seal(plain: &[u8]) -> Option<Sealed> {
    #[cfg(any(test, feature = "test-support"))]
    if let Some(store) = STORE_FOR_TEST.with(|cell| cell.get()) {
        return (store.seal)(plain);
    }
    (super::port().seal)(plain)
}

pub fn open(sealed: &Sealed) -> Option<Vec<u8>> {
    #[cfg(any(test, feature = "test-support"))]
    if let Some(store) = STORE_FOR_TEST.with(|cell| cell.get()) {
        return (store.open)(sealed);
    }
    (super::port().open)(sealed)
}

pub fn remove(backend: &Backend, key: &str) {
    #[cfg(any(test, feature = "test-support"))]
    if let Some(store) = STORE_FOR_TEST.with(|cell| cell.get()) {
        return (store.remove)(backend, key);
    }
    (super::port().remove)(backend, key)
}

/// Separate installation-scoped keys: no Plex sign-out can revoke these records.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Namespace { OttControl, OttState }
impl Namespace {
    pub fn key_name(self) -> String {
        let suffix = match self { Self::OttControl => "control", Self::OttState => "state" };
        format!("{}.ott.{suffix}.v1", plx_base::paths::app_id())
    }
}

pub fn seal_named(namespace: Namespace, plain: &[u8]) -> Option<Sealed> {
    seal_named_detailed(namespace, plain).ok()
}
pub fn seal_named_detailed(namespace: Namespace, plain: &[u8]) -> Result<Sealed, crate::storage::wire::KeymanagerFailure> {
    (super::port().seal_named)(namespace, plain)
}
/// No installed protection provider is not proof of a firmware service failure.
pub fn protection_unavailable() -> crate::storage::wire::KeymanagerFailure {
    use crate::storage::wire::{ErrorCode, KeymanagerFailure, KeymanagerFailureCategory, KeymanagerOperation, KeymanagerStage};
    KeymanagerFailure { operation: KeymanagerOperation::Seal, stage: KeymanagerStage::Validate,
        code: ErrorCode::Unavailable, category: KeymanagerFailureCategory::Unavailable, service_code: None, hub_error: None }
}
pub fn open_named(namespace: Namespace, sealed: &Sealed) -> Option<Vec<u8>> {
    (super::port().open_named)(namespace, sealed)
}

#[cfg(all(test, not(all(feature = "hostsim", feature = "devtriggers", any(target_os = "macos", target_os = "linux")))))]
#[test]
fn shipping_codec_refuses_host_fixture_backend() {
    let encoded = r#"{"backend":"host_fixture","key":"fixture","iv":"","data":""}"#;
    assert!(serde_json::from_str::<Sealed>(encoded).is_err());
}
