//! Candidate-only adjustment after the USB worker proved an exact relocation.
//! No pathname, UUID, or persisted fingerprint independently grants access.
use super::{selection, Saved};
use crate::app::ott_kiosk_stored::{self as stored, Stored, TargetRef};
use crate::app::ott_local_root::LocalSource;
use crate::app::ott_local_session::{BoundSource, SourceKind};
use crate::app::ott_local_sources::{FolderDescriptor, RememberedFolders, UsbBinding};
use crate::app::ott_usb::VerifiedRelocation;

/// Use only in the existing addressed USB install's Saved candidate. The caller
/// must still validate its operation/navigation and sole-writer settlement.
/// Calling again after ordinary candidate reconciliation is safe: the candidate
/// may contain the exact old policy or exactly this receipt's resulting policy.
pub(super) fn rebase_local_folder_pin(
    current: &Saved,
    candidate: &mut Saved,
    source: &LocalSource,
    relocation: &VerifiedRelocation,
) -> Result<bool, &'static str> {
    if !relocation.is_current(source) {
        return Err("USB relocation is no longer current");
    }
    rebase_candidate(
        current,
        candidate,
        relocation.old_root(),
        relocation.new_root(),
        relocation.stable_binding(),
    )
}

fn descriptor_matches(
    descriptor: &FolderDescriptor,
    root: &BoundSource,
    binding: &UsbBinding,
) -> bool {
    descriptor.id == root.id
        && descriptor.name == root.name
        && Some(descriptor.path.as_str()) == root.canonical_path.as_deref()
        && descriptor.usb.as_ref() == Some(binding)
}

/// Pure projection after live receipt validation. Kept private so callers cannot
/// substitute a matching path or UUID for the worker's retained-root evidence.
fn rebase_candidate(
    current: &Saved,
    candidate: &mut Saved,
    old: &BoundSource,
    new: &BoundSource,
    binding: &UsbBinding,
) -> Result<bool, &'static str> {
    let Some(pin) = stored::read(&current.settings)
        .map_err(|_| "Invalid stored kiosk policy")?
    else {
        return Ok(false);
    };
    let TargetRef::LocalFolder {
        source_id,
        source_fingerprint,
    } = pin.target()
    else {
        return Ok(false);
    };
    if source_id != old.id {
        return Ok(false);
    }
    binding.validate()?;
    if old.id != new.id
        || old.name != new.name
        || old.kind != SourceKind::Folder
        || new.kind != SourceKind::Folder
        || old.authority_ref != new.authority_ref
        || old.generation == 0
        || old.generation != new.generation
        || !new.available
    {
        return Err("USB relocation changed the pinned source identity");
    }
    let old_fingerprint = selection::local_root_fingerprint(old)
        .map_err(|_| "Previous USB folder fingerprint is unavailable")?;
    if source_fingerprint != old_fingerprint {
        return Err("Stored kiosk pin does not match the previous USB folder");
    }
    let remembered = RememberedFolders::read(&current.settings)?;
    if !remembered.folders().iter().any(|folder| descriptor_matches(folder, old, binding)) {
        return Err("Stored USB folder does not match the verified relocation");
    }
    let proposed = RememberedFolders::read(&candidate.settings)?;
    if !proposed.folders().iter().any(|folder| descriptor_matches(folder, new, binding)) {
        return Err("Candidate USB folder does not match the verified relocation");
    }
    let fingerprint = selection::local_root_fingerprint(new)
        .map_err(|_| "Relocated USB folder fingerprint is unavailable")?;
    let expected = Stored::local_folder(
        pin.origin().clone(),
        pin.strict(),
        source_id.to_owned(),
        fingerprint,
    )
    .and_then(|pin| pin.encode())
    .map_err(|_| "Relocated kiosk policy could not be encoded")?;
    let existing = candidate.settings.get(stored::KEY);
    if existing != current.settings.get(stored::KEY) && existing != Some(&expected) {
        return Err("Kiosk policy changed while the USB relocation was prepared");
    }
    if existing == Some(&expected) {
        return Ok(false);
    }
    candidate.settings.insert(stored::KEY.into(), expected);
    Ok(true)
}

#[cfg(test)]
#[path = "ott_kiosk_usb_tests.rs"]
mod tests;
