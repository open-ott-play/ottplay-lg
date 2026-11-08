//! Reconcile an explicitly confirmed folder with exact remembered identity.
//! Runs on the existing USB worker. No persistence, inventory mutation or grant
//! withdrawal occurs here; the adapter must compare the captured Saved rows
//! before publishing and before committing its existing writer transaction.
use super::*;
use crate::app::ott_local_sources::{FolderDescriptor, LocalSourceKind, RememberedFolders};
use std::os::unix::fs::MetadataExt;

/// Only a positive held-path or complete mount observation reports a source loss. The original
/// source/grant is returned so the current Runtime can retire that exact owner;
/// generic snapshot/identity failures and cancellation never revoke a grant.
pub(in crate::app) enum ConfirmFolderError {
    Rejected(Error),
    SourceLost(LocalSource),
}
impl From<Error> for ConfirmFolderError {
    fn from(error: Error) -> Self {
        Self::Rejected(error)
    }
}
impl std::fmt::Debug for ConfirmFolderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Rejected(error) => f.debug_tuple("Rejected").field(error).finish(),
            Self::SourceLost(_) => f.write_str("SourceLost"),
        }
    }
}

/// Receiver-private proof/result. A remembered descriptor is never itself a
/// grant: `source` is either a re-observed existing grant or a fresh acquisition.
/// No Debug/Serialize or public field can turn private paths into wire metadata.
#[derive(Clone)]
pub(in crate::app) struct ConfirmedFolder {
    source: LocalSource,
    remembered: RememberedFolders,
    relocated: bool,
}
impl ConfirmedFolder {
    pub fn source(&self) -> &LocalSource {
        &self.source
    }

    /// Accept exactly the captured before-image or this receipt's own exact
    /// candidate after-image (idempotent Connect after its durable ACK). Changes
    /// to other Saved settings do not matter; any other remembered-row change
    /// requires fresh verification. This is not itself a storage CAS.
    pub fn remembered_is_current(&self, current: &RememberedFolders) -> bool {
        &self.remembered == current || self.prepared().is_ok_and(|after| &after == current)
    }

    /// Only explicit Connect calls this; Select and temporary playback merely
    /// retain source(). Keep this receipt with the handle, and retain normal
    /// current-request/grant checks plus the protected writer's ACK boundary.
    pub fn connect_candidate(
        &self,
        current: &RememberedFolders,
    ) -> Result<RememberedFolders, Error> {
        if !self.source.is_live() {
            return Err(Error::Changed);
        }
        let after = self.prepared()?;
        if current != &self.remembered && current != &after {
            return Err(Error::Changed);
        }
        Ok(after)
    }

    fn prepared(&self) -> Result<RememberedFolders, Error> {
        let source = self.source.descriptor();
        let row = FolderDescriptor {
            id: source.id.clone(),
            name: source.name.clone(),
            path: source.canonical_path.clone().ok_or(Error::Changed)?,
            kind: LocalSourceKind::Folder,
            usb: self
                .source
                .usb_witness()
                .and_then(Witness::stable_binding)
                .cloned(),
        };
        if self.relocated {
            self.remembered.with_reselected(row)
        } else {
            self.remembered.with_connected(row)
        }
        .map_err(|_| Error::Changed)
    }
}

fn same_open_folder(off: &OffFrame, a: &LocalSource, b: &LocalSource) -> Result<(), Error> {
    let metadata = |source: &LocalSource| {
        source
            .root_handle()
            .and_then(|root| root.file_if_live(off))
            .ok_or(Error::Changed)?
            .metadata()
            .map_err(|_| Error::Unavailable)
    };
    let a = metadata(a)?;
    let b = metadata(b)?;
    if a.is_dir() && b.is_dir() && a.dev() == b.dev() && a.ino() == b.ino() {
        Ok(())
    } else {
        Err(Error::Changed)
    }
}

fn observed(
    off: &OffFrame,
    source: &LocalSource,
    cancel: &AtomicBool,
) -> Result<LocalSource, ConfirmFolderError> {
    let current = source
        .observe(off, &mut || !cancel.load(Ordering::Acquire))
        .map_err(|error| {
            ConfirmFolderError::Rejected(match error {
                RootError::Cancelled => Error::Cancelled,
                RootError::Unavailable => Error::Changed,
            })
        })?;
    check(cancel)?;
    if !current.is_live() {
        return Err(ConfirmFolderError::SourceLost(source.clone()));
    }
    if !current.same_grant(source) {
        return Err(Error::Changed.into());
    }
    Ok(current)
}

pub(super) fn reuse_with(
    off: &OffFrame,
    confirmed: LocalSource,
    remembered: RememberedFolders,
    known: &[LocalSource],
    cancel: &AtomicBool,
    observe: &mut MountProbe<'_>,
    identify: &mut IdentityProbe<'_>,
    verify_root: &mut RootProbe<'_>,
) -> Result<ConfirmedFolder, ConfirmFolderError> {
    check(cancel)?;
    remembered.validate().map_err(|_| Error::Invalid)?;
    if known.len() > 32 {
        return Err(Error::TooMany.into());
    }
    if !confirmed.is_live() || confirmed.descriptor().kind != SourceKind::Folder {
        return Err(Error::Changed.into());
    }
    let descriptor = confirmed.descriptor();
    let path = descriptor.canonical_path.as_deref().ok_or(Error::Changed)?;
    let witness = confirmed.usb_witness().ok_or(Error::Changed)?;
    let expected = witness.stable_binding();
    let mut matching = remembered.folders().iter().filter(|old| {
        old.id == descriptor.id
            || old.path == path
            || expected.is_some_and(|binding| old.usb.as_ref() == Some(binding))
    });
    let old = matching.next();
    if matching.next().is_some() {
        return Err(Error::Changed.into());
    }
    if let Some(old) = old {
        if old.kind != LocalSourceKind::Folder || expected.is_none() || old.usb.as_ref() != expected
        {
            return Err(Error::Changed.into());
        }
    }
    let existing = if let Some(old) = old {
        let mut matches = known
            .iter()
            .filter(|source| source.descriptor().id == old.id);
        let existing = matches.next();
        if matches.next().is_some() {
            return Err(Error::Changed.into());
        }
        if let Some(source) = existing {
            if source.descriptor().kind != SourceKind::Folder
                || source.descriptor().name != old.name
                || source.descriptor().canonical_path.as_deref() != Some(old.path.as_str())
            {
                return Err(Error::Changed.into());
            }
        }
        existing.filter(|source| source.is_live())
    } else {
        None
    };
    let relocated = old.is_some_and(|old| old.path != path);
    if relocated && existing.is_some() {
        return Err(Error::Changed.into());
    }
    // A second live logical source for this same binding is ambiguous. The
    // newly confirmed provisional grant itself is expected and is exempt.
    if old.is_some()
        && known.iter().any(|source| {
            source.is_live()
                && !source.same_grant(&confirmed)
                && Some(source.descriptor().id.as_str()) != old.map(|row| row.id.as_str())
                && source.usb_witness().and_then(Witness::stable_binding) == expected
        })
    {
        return Err(Error::Changed.into());
    }
    // The original picker Confirm is not authority to relabel an arbitrary FD.
    // Reuse the exact Select verifier (mount/UUID checks, canonical path, actual
    // acquisition and final observation), then correlate its FD with Confirm.
    let mut requested = descriptor.clone();
    if let Some(old) = old {
        requested.id = old.id.clone();
        requested.name = old.name.clone();
    }
    let relative = Path::new(path)
        .strip_prefix(witness.mount.mount_point())
        .ok()
        .and_then(Path::to_str)
        .ok_or(Error::Changed)?
        .to_owned();
    let answer = execute_probed(
        off,
        Task::Select {
            choice: Choice {
                mount: witness.mount.clone(),
                readable: true,
            },
            source: requested,
            relative_folder: relative,
        },
        cancel,
        observe,
        identify,
        verify_root,
    )?;
    let Answer::Selected(fresh) = answer else {
        return Err(Error::Changed.into());
    };
    if fresh.descriptor().canonical_path.as_deref() != Some(path)
        || fresh.usb_witness().and_then(Witness::stable_binding) != expected
    {
        return Err(Error::Changed.into());
    }
    let confirmed = observed(off, &confirmed, cancel)?;
    same_open_folder(off, &fresh, &confirmed)?;
    let source = if let Some(existing) = existing {
        let old_witness = existing.usb_witness().ok_or(Error::Changed)?;
        // A new mount incarnation cannot inherit a still-live old grant, even
        // if a filesystem later happens to reuse an inode number.
        if old_witness.mount != witness.mount {
            check(cancel)?;
            return Err(ConfirmFolderError::SourceLost(existing.clone()));
        }
        if old_witness.stable_binding() != expected
            || !old_witness.is_observed(std::slice::from_ref(&witness.mount), existing.descriptor())
        {
            return Err(Error::Changed.into());
        }
        let existing = observed(off, existing, cancel)?;
        same_open_folder(off, &fresh, &existing)?;
        verify_root(off, &existing, &witness.mount, cancel)?;
        existing
    } else if old.is_some() {
        fresh
    } else {
        confirmed
    };
    let source = observed(off, &source, cancel)?;
    check(cancel)?;
    Ok(ConfirmedFolder {
        source,
        remembered,
        relocated,
    })
}

#[cfg(test)]
#[path = "ott_usb_identity_tests.rs"]
mod tests;
