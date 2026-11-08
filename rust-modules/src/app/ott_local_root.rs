//! Retained source capabilities for the existing Local inventory. Only its
//! binding worker opens or observes paths; only the addressed runtime consumer
//! withdraws a live grant. A held FD/dev/ino is not a persistent volume identity.
use super::ott_local_session::{BoundSource, SourceKind};
use plx_base::task::OffFrame;
use std::fs::{File, Metadata, OpenOptions};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

#[path = "ott_local_file.rs"]
mod single_file;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum RootError {
    Cancelled,
    Unavailable,
}

/// A private, non-serializable capability. Dropping a clone never revokes others.
pub(super) struct RootHandle {
    file: File,
    withdrawn: Arc<AtomicBool>,
}
impl RootHandle {
    /// Pure cache-probe check; no filesystem access or runtime registry lookup.
    pub fn is_live(&self) -> bool {
        !self.withdrawn.load(Ordering::Acquire)
    }

    /// Share this source owner's one withdrawal fence with an opened media file.
    pub fn withdrawal_fence(&self) -> Arc<AtomicBool> {
        self.withdrawn.clone()
    }

    /// The bounded reader uses this same FD and checks is_live during/after IO.
    pub fn file_if_live(&self, _off: &OffFrame) -> Option<&File> {
        self.is_live().then_some(&self.file)
    }
}

#[derive(Clone)]
enum LocalGrant {
    Folder(Arc<RootHandle>),
    SingleFile(Arc<single_file::SingleFile>),
}

#[derive(Clone)]
pub(super) struct LocalSource {
    descriptor: BoundSource,
    grant: Option<LocalGrant>,
    usb: Option<super::ott_usb::Witness>,
    // Restriction only: retained after a loss even when no live witness remains.
    // It cannot authorize opening a path or reconstruct a source grant.
    requires_usb_verification: bool,
}
impl LocalSource {
    pub fn unavailable(mut descriptor: BoundSource) -> Self {
        descriptor.available = false;
        Self {
            descriptor,
            grant: None,
            usb: None,
            requires_usb_verification: false,
        }
    }

    /// Loss projection for an existing source. Drop authority, retain only the
    /// requirement that a USB source cannot later be reopened as a plain path.
    pub(super) fn unavailable_observation(&self) -> Self {
        let mut lost = Self::unavailable(self.descriptor.clone());
        lost.requires_usb_verification = self.requires_usb_verification;
        lost
    }

    pub(super) fn requires_usb_verification(&self) -> bool {
        self.requires_usb_verification
    }

    pub(super) fn requiring_usb_verification(mut self) -> Self {
        if self.descriptor.kind == SourceKind::Folder {
            self.requires_usb_verification = true;
        }
        self
    }

    /// Caller has already authorized this exact canonical source. This operation
    /// pins its current object, not its physical medium or future contents. The
    /// connection worker separately enforces the supported video-file policy.
    pub fn acquire_authorized(
        off: &OffFrame,
        mut descriptor: BoundSource,
        check: &mut dyn FnMut() -> bool,
    ) -> Result<Self, RootError> {
        checkpoint(check)?;
        if descriptor.generation == 0 {
            return Err(RootError::Unavailable);
        }
        let path = descriptor
            .canonical_path
            .as_deref()
            .ok_or(RootError::Unavailable)?;
        if descriptor.kind == SourceKind::File {
            // Shared lexical policy is not authority; the worker still opens
            // and correlates the exact regular file below.
            if descriptor.single_file_name().is_none() {
                return Err(RootError::Unavailable);
            }
            let file = single_file::SingleFile::acquire(off, path, check)?;
            descriptor.available = true;
            return Ok(Self {
                descriptor,
                grant: Some(LocalGrant::SingleFile(Arc::new(file))),
                usb: None,
                requires_usb_verification: false,
            });
        }
        let before = observe_directory(path)?;
        checkpoint(check)?;
        let file = open_directory(path)?;
        checkpoint(check)?;
        let opened = file.metadata().map_err(|_| RootError::Unavailable)?;
        let after = observe_directory(path)?;
        checkpoint(check)?;
        if !same_directory(&before, &opened) || !same_directory(&opened, &after) {
            return Err(RootError::Unavailable);
        }
        descriptor.available = true;
        Ok(Self {
            descriptor,
            grant: Some(LocalGrant::Folder(Arc::new(RootHandle {
                file,
                withdrawn: Arc::new(AtomicBool::new(false)),
            }))),
            usb: None,
            requires_usb_verification: false,
        })
    }

    /// A worker returns a loss observation without changing any shared latch.
    /// The runtime applies it only after checking the event's current generation.
    pub fn observe(
        &self,
        off: &OffFrame,
        check: &mut dyn FnMut() -> bool,
    ) -> Result<Self, RootError> {
        checkpoint(check)?;
        let observed = (|| {
            if !self.is_live() {
                return Err(RootError::Unavailable);
            }
            if let Some(LocalGrant::SingleFile(file)) = &self.grant {
                return file.observe(off, check);
            }
            let root = self.root_handle().ok_or(RootError::Unavailable)?;
            let file = root.file_if_live(off).ok_or(RootError::Unavailable)?;
            let opened = file.metadata().map_err(|_| RootError::Unavailable)?;
            checkpoint(check)?;
            let path = self
                .descriptor
                .canonical_path
                .as_deref()
                .ok_or(RootError::Unavailable)?;
            let current = observe_directory(path)?;
            checkpoint(check)?;
            if !root.is_live() || !same_directory(&opened, &current) {
                return Err(RootError::Unavailable);
            }
            Ok(())
        })();
        // Even a failed filesystem observation must not turn cancellation into
        // a source-loss report which could retire a valid grant.
        checkpoint(check)?;
        match observed {
            Ok(()) => Ok(self.clone()),
            Err(RootError::Cancelled) => Err(RootError::Cancelled),
            Err(RootError::Unavailable) => Ok(self.unavailable_observation()),
        }
    }

    pub(super) fn with_usb_witness(mut self, witness: super::ott_usb::Witness) -> Self {
        // Removable relocation is a folder permission, never parent access for a file.
        if matches!(&self.grant, Some(LocalGrant::Folder(_))) {
            self.usb = Some(witness);
            self.requires_usb_verification = true;
        }
        self
    }
    pub(super) fn usb_witness(&self) -> Option<&super::ott_usb::Witness> { self.usb.as_ref() }

    pub fn descriptor(&self) -> &BoundSource {
        &self.descriptor
    }

    /// Directory-only authority for folder scans and adjacent assets. A selected
    /// single file never exposes its privately held parent through this method.
    pub fn root_handle(&self) -> Option<&Arc<RootHandle>> {
        match &self.grant {
            Some(LocalGrant::Folder(root)) if self.descriptor.available && root.is_live() => Some(root),
            _ => None,
        }
    }

    pub fn is_live(&self) -> bool {
        self.descriptor.available && match &self.grant {
            Some(LocalGrant::Folder(root)) => root.is_live(),
            Some(LocalGrant::SingleFile(file)) => file.is_live(),
            None => false,
        }
    }

    /// Capability identity only; callers separately compare source ID, kind,
    /// generation and liveness. No path comparison can replace this check.
    pub fn same_grant(&self, other: &Self) -> bool {
        match (&self.grant, &other.grant) {
            (Some(LocalGrant::Folder(a)), Some(LocalGrant::Folder(b))) => Arc::ptr_eq(a, b),
            (Some(LocalGrant::SingleFile(a)), Some(LocalGrant::SingleFile(b))) => Arc::ptr_eq(a, b),
            _ => false,
        }
    }

    pub fn single_file_name(&self) -> Option<&str> {
        match &self.grant {
            Some(LocalGrant::SingleFile(file)) if self.is_live() => Some(file.name()),
            _ => None,
        }
    }

    /// Return the already-open video, never a path to reopen or directory grant.
    pub fn single_file(&self, exact_basename: &str) -> Option<plx_media::ff::HeldLocalFile> {
        match &self.grant {
            Some(LocalGrant::SingleFile(file)) if self.is_live() && file.name() == exact_basename => {
                Some(file.held())
            }
            _ => None,
        }
    }

    /// Runtime-only operation: call before publishing loss/replacement. A stale
    /// worker, discarded candidate or dropped clone must never call this.
    pub fn withdraw(&mut self) {
        match self.grant.take() {
            Some(LocalGrant::Folder(root)) => root.withdrawn.store(true, Ordering::Release),
            Some(LocalGrant::SingleFile(file)) => file.withdraw(),
            None => {},
        }
        self.descriptor.available = false;
    }
}

fn checkpoint(check: &mut dyn FnMut() -> bool) -> Result<(), RootError> {
    if check() {
        Ok(())
    } else {
        Err(RootError::Cancelled)
    }
}

fn open_directory(path: &str) -> Result<File, RootError> {
    OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK)
        .open(path)
        .map_err(|_| RootError::Unavailable)
}

fn observe_directory(path: &str) -> Result<Metadata, RootError> {
    // Match the existing BoundSource path envelope; do not normalize an imported
    // string into an authority or silently follow a different root alias.
    if path.len() > 4096 || path.chars().any(char::is_control) || !Path::new(path).is_absolute() {
        return Err(RootError::Unavailable);
    }
    let canonical = std::fs::canonicalize(path).map_err(|_| RootError::Unavailable)?;
    if canonical != Path::new(path) {
        return Err(RootError::Unavailable);
    }
    let metadata = std::fs::symlink_metadata(path).map_err(|_| RootError::Unavailable)?;
    if !metadata.is_dir() {
        return Err(RootError::Unavailable);
    }
    Ok(metadata)
}

fn same_directory(first: &Metadata, second: &Metadata) -> bool {
    first.is_dir() && second.is_dir() && first.dev() == second.dev() && first.ino() == second.ino()
}

#[cfg(test)]
#[path = "ott_local_root_tests.rs"]
mod tests;
