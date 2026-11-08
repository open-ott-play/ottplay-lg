//! Filesystem identity observed by the installed port, never a root grant.
//! Call only from the existing Local IO worker with its cancellation predicate.

use super::MountCandidate;
use plx_base::task::OffFrame;
use std::collections::{HashMap, HashSet};
use std::os::unix::fs::MetadataExt;

pub(crate) const UUID_LIMIT: usize = 256;
pub(crate) const FILESYSTEM_LIMIT: usize = 64;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IdentityError {
    Unsupported,
    Unavailable,
    Invalid,
    Ambiguous,
    Changed,
    Cancelled,
}

/// Exact logical filesystem identity. UUIDs can be cloned; this is not hardware
/// authentication. Constructing persisted data does not produce an observation.
#[derive(Clone, PartialEq, Eq)]
pub struct StableIdentity {
    uuid: String,
    filesystem: String,
}

impl StableIdentity {
    pub fn new(uuid: &str, filesystem: &str) -> Result<Self, IdentityError> {
        if !token(uuid, UUID_LIMIT, b"-_.:") || !token(filesystem, FILESYSTEM_LIMIT, b"._-") {
            return Err(IdentityError::Invalid);
        }
        Ok(Self {
            uuid: uuid.into(),
            filesystem: filesystem.into(),
        })
    }
    pub fn uuid(&self) -> &str {
        &self.uuid
    }
    pub fn filesystem(&self) -> &str {
        &self.filesystem
    }
}

fn token(value: &str, limit: usize, punctuation: &[u8]) -> bool {
    !value.is_empty()
        && value.len() <= limit
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || punctuation.contains(&b))
        && !matches!(value, "." | "..")
}

/// One current mount, including its filesystem-relative exposed root, bound to
/// the device's observed UUID/type. No Debug/Serialize implementation: all raw
/// paths and identifiers stay in the worker/receiver's private context.
#[derive(Clone, PartialEq, Eq)]
pub struct IdentityObservation {
    mount: MountCandidate,
    identity: StableIdentity,
}

impl IdentityObservation {
    pub fn mount(&self) -> &MountCandidate {
        &self.mount
    }
    pub fn identity(&self) -> &StableIdentity {
        &self.identity
    }

    /// Fixture evidence only. Tests must still acquire a real RootFD separately.
    #[cfg(any(test, feature = "test-support"))]
    pub fn for_test(mount: MountCandidate, identity: StableIdentity) -> Self {
        Self::fixture_observation(mount, identity)
    }
    #[cfg(any(test, feature = "test-support", all(feature = "hostsim", feature = "devtriggers", any(target_os = "macos", target_os = "linux"))))]
    pub(in crate::tv) fn fixture_observation(mount: MountCandidate, identity: StableIdentity) -> Self {
        Self { mount, identity }
    }
}

/// All identities for a COMPLETE mount snapshot, or one explicit error. Never
/// pass a filtered prefix: unknown identities cannot rule out a cloned UUID.
/// The caller must compare a second complete observation after opening a root.
pub fn identity_snapshot(
    off: &OffFrame,
    mounts: &[MountCandidate],
    check: &mut dyn FnMut() -> bool,
) -> Result<Vec<IdentityObservation>, IdentityError> {
    (crate::tv::port().removable_identities)(off, mounts, check)
}

/// Correlate an already held directory with a mount observation. This is an
/// fstat of the caller's File, not a pathname reopen or a stable volume key.
pub fn directory_device(
    _off: &OffFrame,
    file: &std::fs::File,
) -> Result<(u32, u32), IdentityError> {
    let metadata = file.metadata().map_err(|_| IdentityError::Unavailable)?;
    if !metadata.is_dir() {
        return Err(IdentityError::Invalid);
    }
    device_parts(metadata.dev())
}

#[cfg(target_os = "linux")]
fn device_parts(device: u64) -> Result<(u32, u32), IdentityError> {
    Ok((libc::major(device), libc::minor(device)))
}

#[cfg(target_os = "macos")]
fn device_parts(device: u64) -> Result<(u32, u32), IdentityError> {
    // Darwin's signed dev_t carries the 32-bit device representation. Preserve
    // its bits instead of narrowing Linux values or rejecting a set sign bit.
    let native = if let Ok(bits) = u32::try_from(device) {
        i32::from_ne_bytes(bits.to_ne_bytes())
    } else {
        // MetadataExt can sign-extend the original signed dev_t into u64.
        i32::try_from(i64::from_ne_bytes(device.to_ne_bytes()))
            .map_err(|_| IdentityError::Invalid)?
    };
    let major = u32::try_from(libc::major(native)).map_err(|_| IdentityError::Invalid)?;
    let minor = u32::try_from(libc::minor(native)).map_err(|_| IdentityError::Invalid)?;
    Ok((major, minor))
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn device_parts(_: u64) -> Result<(u32, u32), IdentityError> {
    Err(IdentityError::Unsupported)
}

/// Shared bounded assembly; read has no permission to publish a partial list.
/// Device numbers correlate this snapshot only and never become stable keys.
pub(crate) fn resolve_with(
    mounts: &[MountCandidate],
    check: &mut dyn FnMut() -> bool,
    read: &mut dyn FnMut((u32, u32)) -> Result<StableIdentity, IdentityError>,
) -> Result<Vec<IdentityObservation>, IdentityError> {
    if mounts.len() > super::MOUNT_LIMIT {
        return Err(IdentityError::Invalid);
    }
    if !check() {
        return Err(IdentityError::Cancelled);
    }
    let mut ids = HashSet::new();
    let mut paths = HashSet::new();
    for mount in mounts {
        if mount.device().0 == 0 {
            // FUSE/anonymous filesystem device numbers do not identify a
            // libudev block device. Explicit manual selection can still work.
            return Err(IdentityError::Unsupported);
        }
        if !ids.insert(mount.mount_id()) {
            return Err(IdentityError::Invalid);
        }
        if !paths.insert(mount.mount_point()) {
            return Err(IdentityError::Ambiguous);
        }
    }
    let mut devices: HashMap<(u32, u32), StableIdentity> = HashMap::new();
    let mut uuids = HashMap::new();
    let mut result = Vec::with_capacity(mounts.len());
    for mount in mounts {
        if !check() {
            return Err(IdentityError::Cancelled);
        }
        let dev = mount.device();
        if let std::collections::hash_map::Entry::Vacant(entry) = devices.entry(dev) {
            let value = read(dev)?;
            if !check() {
                return Err(IdentityError::Cancelled);
            }
            if uuids.insert(value.uuid().to_owned(), dev).is_some() {
                // Same-device bind mounts use the cache above, so a repeated
                // UUID here necessarily belongs to a different block device.
                return Err(IdentityError::Ambiguous);
            }
            entry.insert(value);
        }
        let value = &devices[&dev];
        // No guessed FUSE backing device or driver-name equivalence. A port
        // can add an evidenced mapping explicitly in a separate change.
        if value.filesystem() != mount.filesystem() {
            return Err(IdentityError::Changed);
        }
        result.push(IdentityObservation {
            mount: mount.clone(),
            identity: value.clone(),
        });
    }
    if !check() {
        return Err(IdentityError::Cancelled);
    }
    Ok(result)
}

#[cfg(test)]
#[path = "identity_tests.rs"]
mod tests;
