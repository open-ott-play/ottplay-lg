//! Stable identity checks inside the existing USB worker. Persisted identity is
//! only an intent: a complete current observation and a new RootFD are required.
use super::{check, BoundSource, Error, IdentityProbe, LocalSource, MountCandidate, UsbBinding};
use crate::app::ott_local_root::RootHandle;
use plx_base::task::OffFrame;
use plx_platform::tv::removable::identity::{IdentityError, IdentityObservation};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

/// Worker-only proof connecting the exact saved intent to a newly opened root.
/// The old descriptor carries the caller's new generation and saved old path;
/// it does not represent an old live grant. No paths/UUIDs are exposed to wire.
#[derive(Clone)]
pub(in crate::app) struct VerifiedRelocation {
    old: BoundSource,
    new: BoundSource,
    binding: UsbBinding,
    root: Arc<RootHandle>,
}

impl VerifiedRelocation {
    pub fn old_root(&self) -> &BoundSource {
        &self.old
    }
    pub fn new_root(&self) -> &BoundSource {
        &self.new
    }
    pub fn stable_binding(&self) -> &UsbBinding {
        &self.binding
    }

    /// Pure admission check. The caller additionally owns request/navigation,
    /// current source generation, Saved intent and final install authority.
    pub fn is_current(&self, source: &LocalSource) -> bool {
        same_descriptor(&self.new, source.descriptor())
            && self.root.is_live()
            && source
                .root_handle()
                .is_some_and(|root| Arc::ptr_eq(root, &self.root))
            && source
                .usb_witness()
                .and_then(|witness| witness.stable_binding())
                == Some(&self.binding)
    }
}

pub(super) fn volume_key(binding: Option<&UsbBinding>) -> Option<super::policy::VolumeKey> {
    binding.map(|binding| super::policy::VolumeKey {
        namespace: "linux-filesystem-uuid-v1".into(),
        value: binding.uuid.clone(),
    })
}

fn same_descriptor(a: &BoundSource, b: &BoundSource) -> bool {
    a.id == b.id
        && a.name == b.name
        && a.kind == b.kind
        && a.available == b.available
        && a.authority_ref == b.authority_ref
        && a.generation == b.generation
        && a.canonical_path == b.canonical_path
}

pub(super) fn verified_relocation(
    old: BoundSource,
    source: &LocalSource,
) -> Result<VerifiedRelocation, Error> {
    let root = source.root_handle().ok_or(Error::Changed)?.clone();
    let binding = source
        .usb_witness()
        .and_then(|w| w.stable_binding())
        .ok_or(Error::Changed)?
        .clone();
    Ok(VerifiedRelocation {
        old,
        new: source.descriptor().clone(),
        binding,
        root,
    })
}

fn identity_error(error: IdentityError) -> Error {
    match error {
        IdentityError::Cancelled => Error::Cancelled,
        IdentityError::Unsupported | IdentityError::Unavailable => Error::Unavailable,
        IdentityError::Invalid => Error::Invalid,
        IdentityError::Ambiguous | IdentityError::Changed => Error::Changed,
    }
}

/// A test seam can supply bad evidence too. Validate completeness and uniqueness
/// before trusting it, rather than relying on the platform reader's internals.
fn validate_snapshot(mounts: &[MountCandidate], rows: &[IdentityObservation]) -> Result<(), Error> {
    if mounts.len() > super::policy::OBSERVATION_LIMIT || rows.len() != mounts.len() {
        return Err(Error::Changed);
    }
    let mut ids = BTreeSet::new();
    let mut paths = BTreeSet::new();
    let mut devices = BTreeMap::new();
    let mut uuids = BTreeMap::new();
    for row in rows {
        let mount = row.mount();
        let identity = row.identity();
        if !ids.insert(mount.mount_id())
            || !paths.insert(mount.mount_point())
            || mounts.iter().filter(|m| *m == mount).count() != 1
            || identity.filesystem() != mount.filesystem()
        {
            return Err(Error::Changed);
        }
        let key = (identity.uuid(), identity.filesystem());
        if devices
            .insert(mount.device(), key)
            .is_some_and(|old| old != key)
            || uuids
                .insert(identity.uuid(), mount.device())
                .is_some_and(|old| old != mount.device())
        {
            return Err(Error::Changed);
        }
    }
    Ok(())
}

pub(super) fn snapshot(
    off: &OffFrame,
    mounts: &[MountCandidate],
    cancel: &AtomicBool,
    identify: &mut IdentityProbe<'_>,
) -> Result<Vec<IdentityObservation>, Error> {
    check(cancel)?;
    let result = identify(off, mounts, cancel);
    check(cancel)?;
    let rows = result.map_err(identity_error)?;
    validate_snapshot(mounts, &rows)?;
    Ok(rows)
}

/// Initial explicit selection may remain manual-only when no identity backend
/// can serve the complete inventory. Malformed/conflicting evidence is an error.
pub(super) fn capture(
    off: &OffFrame,
    mounts: &[MountCandidate],
    selected: &MountCandidate,
    relative: &str,
    cancel: &AtomicBool,
    identify: &mut IdentityProbe<'_>,
) -> Result<Option<UsbBinding>, Error> {
    check(cancel)?;
    let result = identify(off, mounts, cancel);
    check(cancel)?;
    let rows = match result {
        Err(IdentityError::Unsupported | IdentityError::Unavailable) => return Ok(None),
        Err(error) => return Err(identity_error(error)),
        Ok(rows) => rows,
    };
    validate_snapshot(mounts, &rows)?;
    let row = rows
        .iter()
        .find(|row| row.mount() == selected)
        .ok_or(Error::Changed)?;
    UsbBinding::from_observed(
        row.identity().filesystem(),
        row.identity().uuid(),
        selected.root(),
        relative,
    )
    .map(Some)
    .map_err(|_| Error::Invalid)
}

pub(super) fn locate<'a>(
    rows: &'a [IdentityObservation],
    binding: &UsbBinding,
) -> Result<&'a MountCandidate, Error> {
    binding.validate().map_err(|_| Error::Invalid)?;
    let mut device = None;
    let mut selected: Option<&MountCandidate> = None;
    let mut matched = false;
    for row in rows {
        if row.identity().uuid() != binding.uuid
            || row.identity().filesystem() != binding.filesystem_type
        {
            continue;
        }
        matched = true;
        let mount = row.mount();
        if device
            .replace(mount.device())
            .is_some_and(|old| old != mount.device())
        {
            return Err(Error::Changed);
        }
        if binding
            .relative_folder_for_mount_root(mount.root())
            .is_err()
        {
            continue;
        }
        // Multiple same-device bind views represent the same filesystem path.
        // Deterministic view choice changes neither that path nor the volume.
        if selected.is_none_or(|old| mount.mount_point() < old.mount_point()) {
            selected = Some(mount);
        }
    }
    selected.ok_or(if matched {
        Error::MissingFolder
    } else {
        Error::Unavailable
    })
}

pub(super) fn verify(
    off: &OffFrame,
    mounts: &[MountCandidate],
    selected: &MountCandidate,
    expected: &UsbBinding,
    cancel: &AtomicBool,
    identify: &mut IdentityProbe<'_>,
) -> Result<(), Error> {
    let rows = snapshot(off, mounts, cancel, identify)?;
    let row = rows
        .iter()
        .find(|row| row.mount() == selected)
        .ok_or(Error::Changed)?;
    let relative = expected
        .relative_folder_for_mount_root(selected.root())
        .map_err(|_| Error::Changed)?;
    let current = UsbBinding::from_observed(
        row.identity().filesystem(),
        row.identity().uuid(),
        selected.root(),
        relative,
    )
    .map_err(|_| Error::Changed)?;
    if &current != expected {
        return Err(Error::Changed);
    }
    Ok(())
}

/// The held leaf must be on the observed mounted filesystem, including after
/// path traversal. This rejects a nested mount in the selected relative folder.
/// It is an observation correlation, never a persistent identity by itself.
pub(super) fn check_root_device(
    off: &OffFrame,
    source: &LocalSource,
    mount: &MountCandidate,
    cancel: &AtomicBool,
) -> Result<(), Error> {
    check(cancel)?;
    let file = source
        .root_handle()
        .and_then(|root| root.file_if_live(off))
        .ok_or(Error::Changed)?;
    let device = plx_platform::tv::removable::identity::directory_device(off, file)
        .map_err(identity_error)?;
    check(cancel)?;
    if device != mount.device() {
        return Err(Error::Changed);
    }

    Ok(())
}

#[cfg(test)]
#[path = "ott_usb_stable_tests.rs"]
mod tests;
