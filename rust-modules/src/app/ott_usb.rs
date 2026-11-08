//! USB observations and explicit grant acquisition on the existing Local worker.
//! Nothing here mounts a drive, copies media or treats a pathname as volume ID.
use super::ott_local_root::{LocalSource, RootError};
use super::ott_local_session::{BoundSource, SourceKind};
use super::ott_local_sources::UsbBinding;
use plx_platform::tv::removable::identity::{self, IdentityError, IdentityObservation};

#[path = "ott_usb_stable.rs"]
mod stable;
#[path = "ott_usb_browser.rs"]
mod browser;
pub(super) use browser::{BrowsePage, BrowseTask, PAGE_ROWS};
#[path = "ott_usb_identity.rs"]
mod remembered_identity;
pub(super) use remembered_identity::{ConfirmedFolder, ConfirmFolderError};
use super::ott_removable as policy;
use plx_base::task::OffFrame;
use plx_platform::tv::removable::{self, MountCandidate};
pub(super) use stable::VerifiedRelocation;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Error {
    Cancelled,
    Unavailable,
    MissingFolder,
    PermissionDenied,
    Changed,
    Invalid,
    TooMany,
}
impl Error {
    pub fn message(self) -> &'static str {
        match self {
            Self::Cancelled => "USB selection was cancelled",
            Self::Unavailable => "USB drive or folder is unavailable; reconnect it and refresh",
            Self::MissingFolder => "The saved folder is missing on this USB drive",
            Self::PermissionDenied => "The USB folder cannot be read by this application",
            Self::Changed => "USB drive changed; refresh and select it again",
            Self::Invalid => "Invalid USB selection",
            Self::TooMany => "Too many USB volumes",
        }
    }
}

/// Private observation, never an access grant. Cloning retains exact evidence.
#[derive(Clone)]
pub(super) struct Choice {
    mount: MountCandidate,
    readable: bool,
}
impl Choice {
    pub fn label(&self, index: usize) -> String {
        format!("USB {} ({})", index + 1, self.mount.filesystem())
    }
    pub fn readable(&self) -> bool {
        self.readable
    }
    /// Convert saved filesystem-relative intent only after a mount is chosen.
    /// This is addressing, not identity verification or a directory grant.
    pub fn saved_relative_folder(&self, binding: &UsbBinding) -> Result<String, &'static str> {
        binding.relative_folder_for_mount_root(self.mount.root()).map(str::to_owned)
    }
}

#[derive(Clone)]
pub(super) struct Witness {
    mount: MountCandidate,
    binding: policy::Binding,
    stable: Option<UsbBinding>,
}
impl Witness {
    pub fn stable_binding(&self) -> Option<&UsbBinding> {
        self.stable.as_ref()
    }

    pub fn is_observed(&self, mounts: &[MountCandidate], source: &BoundSource) -> bool {
        if self.binding.source_id() != source.id
            || Some(self.binding.canonical_path()) != source.canonical_path.as_deref()
            || self.binding.grant().authority_ref != source.authority_ref
            || self.binding.grant().generation != source.generation
            || !unique_mount(mounts, &self.mount)
        {
            return false;
        }
        let path = Path::new(self.binding.canonical_path());
        let Ok(relative) = path.strip_prefix(self.mount.mount_point()) else {
            return false;
        };
        let Some(relative) = relative.to_str() else {
            return false;
        };
        let remembered = policy::RememberedSource {
            source_id: source.id.clone(),
            volume: stable::volume_key(self.stable.as_ref()),
            relative_folder: relative.into(),
        };
        let observation = policy::MountObservation {
            volume: stable::volume_key(self.stable.as_ref()),
            incarnation: format!("local:{}", source.generation),
            canonical_mount: self.mount.mount_point().into(),
            relative_folder: relative.into(),
            folder: policy::FolderObservation::Readable {
                canonical_path: self.binding.canonical_path().into(),
            },
        };
        matches!(
            policy::assess(
                &remembered,
                &policy::InventorySnapshot {
                    generation: source.generation,
                    complete: true,
                    mounts: &[observation]
                },
                policy::Selection::Automatic,
                Some(policy::PreviousBinding {
                    binding: &self.binding,
                    retired: false
                })
            ),
            Ok(policy::Assessment::Current)
        )
    }
}

pub(super) enum Task {
    InspectChoices(Vec<Choice>),
    VerifySource(LocalSource),
    ConfirmFolder {
        task: BrowseTask,
        remembered: crate::app::ott_local_sources::RememberedFolders,
        known: Vec<LocalSource>,
    },
    Browse(BrowseTask),
    /// The descriptor carries the caller's fresh generation and saved old path.
    /// It is not a retained live grant; the worker opens a new root.
    Restore {
        binding: UsbBinding,
        source: BoundSource,
    },
    Discover {
        known: Vec<LocalSource>,
    },
    Select {
        choice: Choice,
        source: BoundSource,
        relative_folder: String,
    },
}
pub(super) enum Answer {
    Verified(LocalSource),
    ConfirmedFolder(ConfirmedFolder),
    /// Positive loss observation for this exact held grant; Runtime owns withdrawal.
    SourceLost(LocalSource),
    Browse(BrowsePage),
    Choices {
        choices: Vec<Choice>,
        observed: Vec<LocalSource>,
    },
    Selected(LocalSource),
    Restored {
        source: LocalSource,
        relocation: VerifiedRelocation,
    },
}

fn check(cancel: &AtomicBool) -> Result<(), Error> {
    if cancel.load(Ordering::Acquire) {
        Err(Error::Cancelled)
    } else {
        Ok(())
    }
}
pub(super) fn mounts(off: &OffFrame, cancel: &AtomicBool) -> Result<Vec<MountCandidate>, Error> {
    let rows =
        removable::observe(off, &mut || !cancel.load(Ordering::Acquire)).map_err(|e| match e {
            removable::ReadError::Cancelled => Error::Cancelled,
            _ => Error::Unavailable,
        })?;
    if rows.len() > policy::OBSERVATION_LIMIT {
        return Err(Error::TooMany);
    }
    Ok(rows)
}

/// Returns the exact relative folder for a known native USB mount layout.
/// This classifies an address; it does not validate or authorize the volume.
pub(super) fn relative_folder(path: &str) -> Option<String> {
    let mut parts = path.strip_prefix("/tmp/usb/")?.splitn(3, '/');
    let (disk, partition) = (parts.next()?, parts.next()?);
    if disk.is_empty()
        || partition.is_empty()
        || matches!(disk, "." | "..")
        || matches!(partition, "." | "..")
    {
        return None;
    }
    let relative = parts.next().unwrap_or("").to_owned();
    (relative.is_empty() || plx_data::ott::valid_local_relative_path(&relative)).then_some(relative)
}
pub(super) fn is_usb_path(path: &str) -> bool {
    path == "/tmp/usb" || path.starts_with("/tmp/usb/")
}

fn unique_mount(mounts: &[MountCandidate], selected: &MountCandidate) -> bool {
    let mut same_path = mounts
        .iter()
        .filter(|mount| mount.mount_point() == selected.mount_point());
    same_path.next() == Some(selected) && same_path.next().is_none()
}

fn io_error(error: std::io::Error) -> Error {
    match error.kind() {
        std::io::ErrorKind::NotFound => Error::MissingFolder,
        std::io::ErrorKind::PermissionDenied => Error::PermissionDenied,
        _ => Error::Unavailable,
    }
}
fn folder(mount: &MountCandidate, relative: &str, cancel: &AtomicBool) -> Result<PathBuf, Error> {
    if !relative.is_empty() && !plx_data::ott::valid_local_relative_path(relative) {
        return Err(Error::Invalid);
    }
    check(cancel)?;
    let base = Path::new(mount.mount_point());
    let mut current = base.to_path_buf();
    let metadata = std::fs::symlink_metadata(&current).map_err(io_error)?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(Error::Unavailable);
    }
    if !relative.is_empty() {
        for part in relative.split('/') {
            check(cancel)?;
            current.push(part);
            let metadata = std::fs::symlink_metadata(&current).map_err(io_error)?;
            if !metadata.is_dir() || metadata.file_type().is_symlink() {
                return Err(Error::Unavailable);
            }
        }
    }
    let canonical = std::fs::canonicalize(&current).map_err(io_error)?;
    if canonical != current || !canonical.starts_with(base) {
        return Err(Error::Changed);
    }
    let mut entries = std::fs::read_dir(&canonical).map_err(io_error)?;
    if entries.next().transpose().map_err(io_error)?.is_some() { /* traversal permission checked */
    }
    check(cancel)?;
    Ok(canonical)
}

type MountProbe<'a> = dyn FnMut(&OffFrame, &AtomicBool) -> Result<Vec<MountCandidate>, Error> + 'a;
type IdentityProbe<'a> = dyn FnMut(
        &OffFrame,
        &[MountCandidate],
        &AtomicBool,
    ) -> Result<Vec<IdentityObservation>, IdentityError>
    + 'a;
type RootProbe<'a> =
    dyn FnMut(&OffFrame, &LocalSource, &MountCandidate, &AtomicBool) -> Result<(), Error> + 'a;

pub(super) fn execute(off: &OffFrame, task: Task, cancel: &AtomicBool) -> Result<Answer, Error> {
    execute_probed(
        off,
        task,
        cancel,
        &mut |off, cancel| mounts(off, cancel),
        &mut |off, rows, cancel| {
            identity::identity_snapshot(off, rows, &mut || !cancel.load(Ordering::Acquire))
        },
        &mut stable::check_root_device,
    )
}

// Existing fixtures inject synthetic mount observations but still acquire real
// directory FDs. Neither libudev nor a real block device is involved in them.
#[cfg(test)]
pub(super) fn execute_with(
    off: &OffFrame,
    task: Task,
    cancel: &AtomicBool,
    observe: &mut MountProbe<'_>,
) -> Result<Answer, Error> {
    execute_probed(
        off,
        task,
        cancel,
        observe,
        &mut |_, _, _| Err(IdentityError::Unsupported),
        &mut |_, _, _, _| Ok(()),
    )
}

/// Fixture-only transport injection that retains the production held-FD device
/// correlation. Callers construct actual temporary roots and matching mount
/// evidence; no receipt or RootHandle constructor is exposed.
#[cfg(test)]
pub(in crate::app) fn execute_with_identity(
    off: &OffFrame,
    task: Task,
    cancel: &AtomicBool,
    observe: &mut dyn FnMut(&OffFrame, &AtomicBool) -> Result<Vec<MountCandidate>, Error>,
    identify: &mut dyn FnMut(
        &OffFrame,
        &[MountCandidate],
        &AtomicBool,
    ) -> Result<Vec<IdentityObservation>, IdentityError>,
) -> Result<Answer, Error> {
    execute_probed(
        off,
        task,
        cancel,
        observe,
        identify,
        &mut stable::check_root_device,
    )
}

fn execute_probed(
    off: &OffFrame,
    task: Task,
    cancel: &AtomicBool,
    observe: &mut MountProbe<'_>,
    identify: &mut IdentityProbe<'_>,
    verify_root: &mut RootProbe<'_>,
) -> Result<Answer, Error> {
    let task = match task {
        Task::Browse(task) => return browser::execute(off, task, cancel, observe, identify, verify_root),
        Task::ConfirmFolder { task, remembered, known } => {
            let Answer::Selected(source) = browser::execute(off, task, cancel, observe, identify, verify_root)? else {
                return Err(Error::Changed);
            };
            return match remembered_identity::reuse_with(off, source, remembered, &known, cancel, observe, identify, verify_root) {
                Ok(confirmed) => Ok(Answer::ConfirmedFolder(confirmed)),
                Err(ConfirmFolderError::Rejected(error)) => Err(error),
                Err(ConfirmFolderError::SourceLost(source)) => Ok(Answer::SourceLost(source)),
            };
        }
        Task::VerifySource(ref source) if !source.is_live() || source.usb_witness().is_none() => return Err(Error::Changed),
        other => other,
    };
    check(cancel)?;
    let observed = observe(off, cancel)?;
    if observed.len() > policy::OBSERVATION_LIMIT {
        return Err(Error::TooMany);
    }
    let (choice, mut source, relative_folder, stable_binding, old_root) = match task {
        Task::VerifySource(source) => {
            let witness = source.usb_witness().ok_or(Error::Changed)?;
            if !witness.is_observed(&observed, source.descriptor()) { check(cancel)?; return Ok(Answer::SourceLost(source)); }
            let current = source.observe(off, &mut || !cancel.load(Ordering::Acquire)).map_err(|_| Error::Cancelled)?;
            if !current.is_live() || !current.same_grant(&source) { check(cancel)?; return Ok(Answer::SourceLost(source)); }
            verify_root(off, &current, &witness.mount, cancel)?;
            if let Some(expected) = witness.stable_binding() {
                match stable::verify(off, &observed, &witness.mount, expected, cancel, identify) {
                    Err(Error::Changed) => { check(cancel)?; return Ok(Answer::SourceLost(source)); }
                    other => other?,
                }
            }
            let after = observe(off, cancel)?;
            if !witness.is_observed(&after, current.descriptor()) { check(cancel)?; return Ok(Answer::SourceLost(source)); }
            check(cancel)?;
            return Ok(Answer::Verified(current));
        }
        Task::InspectChoices(choices) => {
            if choices.iter().any(|choice| !unique_mount(&observed, &choice.mount)) { return Err(Error::Changed); }
            return Ok(Answer::Choices { choices, observed: Vec::new() });
        }
        Task::Browse(_) | Task::ConfirmFolder {..} => unreachable!("browse delegated above"),
        Task::Discover { mut known } => {
            if known.len() > 32 {
                return Err(Error::TooMany);
            }
            let mut choices = Vec::with_capacity(observed.len());
            for mount in &observed {
                check(cancel)?;
                let readable = unique_mount(&observed, mount) && folder(mount, "", cancel).is_ok();
                choices.push(Choice {
                    mount: mount.clone(),
                    readable,
                });
            }
            for source in &mut known {
                check(cancel)?;
                if !source
                    .descriptor()
                    .canonical_path
                    .as_deref()
                    .is_some_and(is_usb_path)
                    && !source.requires_usb_verification()
                {
                    continue;
                }
                *source = if source
                    .usb_witness()
                    .is_some_and(|witness| witness.is_observed(&observed, source.descriptor()))
                {
                    source
                        .observe(off, &mut || !cancel.load(Ordering::Acquire))
                        .map_err(|_| Error::Cancelled)?
                } else {
                    source.unavailable_observation()
                };
            }
            check(cancel)?;
            return Ok(Answer::Choices {
                choices,
                observed: known,
            });
        }
        Task::Select {
            choice,
            source,
            relative_folder,
        } => {
            if !choice.readable || !unique_mount(&observed, &choice.mount) {
                return Err(Error::Changed);
            }
            let binding = stable::capture(
                off,
                &observed,
                &choice.mount,
                &relative_folder,
                cancel,
                identify,
            )?;
            (choice, source, relative_folder, binding, None)
        }
        Task::Restore { binding, source } => {
            binding.validate().map_err(|_| Error::Invalid)?;
            super::ott_local_sources::FolderDescriptor {
                kind: crate::app::ott_local_sources::LocalSourceKind::Folder,
                id: source.id.clone(),
                name: source.name.clone(),
                path: source.canonical_path.clone().ok_or(Error::Invalid)?,
                usb: Some(binding.clone()),
            }
            .validate()
            .map_err(|_| Error::Invalid)?;
            if source.kind != SourceKind::Folder {
                return Err(Error::Invalid);
            }
            let rows = stable::snapshot(off, &observed, cancel, identify)?;
            let mount = stable::locate(&rows, &binding)?.clone();
            let relative = binding
                .relative_folder_for_mount_root(mount.root())
                .map_err(|_| Error::MissingFolder)?
                .to_owned();
            let old = source.clone();
            (
                Choice {
                    mount,
                    readable: true,
                },
                source,
                relative,
                Some(binding),
                Some(old),
            )
        }
    };
    let observed_folder = match folder(&choice.mount, &relative_folder, cancel) {
        Ok(path) => policy::FolderObservation::Readable {
            canonical_path: path.to_str().ok_or(Error::Invalid)?.to_owned(),
        },
        Err(Error::MissingFolder) => policy::FolderObservation::Missing,
        Err(Error::PermissionDenied) => policy::FolderObservation::PermissionDenied,
        Err(error) => return Err(error),
    };
    let path = match &observed_folder {
        policy::FolderObservation::Readable { canonical_path } => canonical_path.clone(),
        _ => Path::new(choice.mount.mount_point())
            .join(&relative_folder)
            .to_str()
            .ok_or(Error::Invalid)?
            .to_owned(),
    };
    source.kind = SourceKind::Folder;
    source.canonical_path = Some(path.clone());
    source.available = true;
    super::ott_local_sources::FolderDescriptor {
        kind: crate::app::ott_local_sources::LocalSourceKind::Folder,
        id: source.id.clone(),
        name: source.name.clone(),
        path: path.clone(),
        usb: stable_binding.clone(),
    }
    .validate()
    .map_err(|_| Error::Invalid)?;
    let incarnation = format!("local:{}", source.generation);
    let remembered = policy::RememberedSource {
        source_id: source.id.clone(),
        volume: stable::volume_key(stable_binding.as_ref()),
        relative_folder: relative_folder.clone(),
    };
    let observation = policy::MountObservation {
        volume: stable::volume_key(stable_binding.as_ref()),
        incarnation: incarnation.clone(),
        canonical_mount: choice.mount.mount_point().into(),
        relative_folder,
        folder: observed_folder,
    };
    let first = [observation.clone()];
    let snapshot = policy::InventorySnapshot {
        generation: source.generation,
        complete: true,
        mounts: &first,
    };
    let plan = match policy::assess(
        &remembered,
        &snapshot,
        if old_root.is_some() {
            policy::Selection::Automatic
        } else {
            policy::Selection::Explicit {
                incarnation: &incarnation,
            }
        },
        None,
    )
    .map_err(|_| Error::Invalid)?
    {
        policy::Assessment::Revalidate(plan) => plan,
        policy::Assessment::Unavailable(reason) => {
            return Err(match reason {
                policy::Unavailable::FolderMissing => Error::MissingFolder,
                policy::Unavailable::PermissionDenied => Error::PermissionDenied,
                _ => Error::Changed,
            })
        }
        policy::Assessment::Current => return Err(Error::Changed),
    };
    check(cancel)?;
    let opened = LocalSource::acquire_authorized(off, source.clone(), &mut || {
        !cancel.load(Ordering::Acquire)
    })
    .map_err(|error| match error {
        RootError::Cancelled => Error::Cancelled,
        _ => Error::Unavailable,
    })?;
    verify_root(off, &opened, &choice.mount, cancel)?;
    // This second complete snapshot is after the actual FD acquisition.
    // Never publish a grant if the selected mount changed meanwhile.
    let after = observe(off, cancel)?;
    if after.len() > policy::OBSERVATION_LIMIT {
        return Err(Error::TooMany);
    }
    if !unique_mount(&after, &choice.mount)
        || folder(&choice.mount, &remembered.relative_folder, cancel)?
            != Path::new(
                opened
                    .descriptor()
                    .canonical_path
                    .as_deref()
                    .ok_or(Error::Invalid)?,
            )
    {
        return Err(Error::Changed);
    }
    if let Some(expected) = &stable_binding {
        // Once identity was established, no missing/change fallback is
        // allowed, even for an explicitly selected folder.
        stable::verify(off, &after, &choice.mount, expected, cancel, identify)?;
    }
    // Re-observe the same opened directory after all metadata IO. A matching
    // pathname/mount table alone cannot detect replacement during that IO.
    let opened = opened
        .observe(off, &mut || !cancel.load(Ordering::Acquire))
        .map_err(|_| Error::Cancelled)?;
    if opened.root_handle().is_none() {
        return Err(Error::Changed);
    }
    verify_root(off, &opened, &choice.mount, cancel)?;
    let fresh = [observation];
    let binding = policy::admit(
        plan,
        &policy::InventorySnapshot {
            generation: source.generation,
            complete: true,
            mounts: &fresh,
        },
        policy::GrantStamp {
            authority_ref: source.authority_ref,
            generation: source.generation,
        },
    )
    .map_err(|_| Error::Changed)?;
    check(cancel)?;
    let source = opened.with_usb_witness(Witness {
        mount: choice.mount,
        binding,
        stable: stable_binding,
    });
    if let Some(old) = old_root {
        let relocation = stable::verified_relocation(old, &source)?;
        Ok(Answer::Restored { source, relocation })
    } else {
        Ok(Answer::Selected(source))
    }
}

#[cfg(test)]
#[path = "ott_usb_tests.rs"]
mod tests;
