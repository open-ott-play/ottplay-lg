//! Shallow USB browsing on the existing catalogue worker. The retained root is
//! private to this picker; it is never an accepted media or asset grant.
use super::*;
use plx_base::task::time::Budget;
use plx_platform::storage::readonly::{self, EntryKind};
use std::fs::File;
use std::os::unix::fs::MetadataExt;
use std::sync::Arc;
use std::time::Duration;

const FIRST_ENTRIES: usize = 4096;
const ENTRY_LIMIT: usize = 500_000;
const FOLDER_LIMIT: usize = 4096;
const NAME_BYTES: usize = 1024 * 1024;
pub(in crate::app) const PAGE_ROWS: usize = 64;

#[derive(Clone)]
pub(in crate::app) struct Location {
    choice: Choice,
    root: LocalSource,
    directory: Arc<File>,
    relative: String,
}

pub(in crate::app) enum BrowseTask {
    Inspect { page: BrowsePage },
    Open {
        choice: Choice,
        source: BoundSource,
        initial_relative: String,
    },
    Read {
        location: Location,
        relative: String,
        entries: usize,
    },
    Confirm {
        location: Location,
        source: BoundSource,
    },
}

#[derive(Clone)]
pub(in crate::app) struct BrowsePage {
    location: Location,
    folders: Vec<String>,
    allowance: usize,
    reached_end: bool,
    complete: bool,
    memory_limited: bool,
}
impl BrowsePage {
    pub fn partial(&self) -> bool { !self.complete || !self.reached_end || self.memory_limited }
    pub fn limit_reached(&self) -> bool { self.partial() && !self.can_more() }
    pub fn inspect(&self) -> BrowseTask { BrowseTask::Inspect { page: self.clone() } }
    pub fn relative(&self) -> &str {
        &self.location.relative
    }
    pub fn root_generation(&self) -> u64 {
        self.location.root.descriptor().generation
    }
    pub fn folders(&self) -> &[String] {
        &self.folders
    }
    pub fn is_root(&self) -> bool {
        self.relative().is_empty()
    }
    pub fn is_live(&self) -> bool {
        self.location.root.is_live()
    }
    pub fn retire(&mut self) {
        self.location.root.withdraw();
    }
    pub fn open_child(&self, index: usize) -> Result<BrowseTask, Error> {
        let name = self.folders.get(index).ok_or(Error::Changed)?;
        let relative = if self.is_root() {
            name.clone()
        } else {
            format!("{}/{name}", self.relative())
        };
        if !plx_data::ott::valid_local_relative_path(&relative) {
            return Err(Error::Invalid);
        }
        Ok(self.read(relative, FIRST_ENTRIES))
    }
    pub fn parent(&self) -> BrowseTask {
        self.read(
            self.relative()
                .rsplit_once('/')
                .map_or("", |(parent, _)| parent)
                .into(),
            FIRST_ENTRIES,
        )
    }
    pub fn root(&self) -> BrowseTask {
        self.read(String::new(), FIRST_ENTRIES)
    }
    pub fn refresh(&self) -> BrowseTask {
        self.read(self.relative().into(), self.allowance)
    }
    pub fn can_more(&self) -> bool {
        !self.reached_end && !self.memory_limited && self.allowance < ENTRY_LIMIT
    }
    pub fn more(&self) -> Option<BrowseTask> {
        self.can_more().then(|| {
            self.read(
                self.relative().into(),
                self.allowance.saturating_mul(2).min(ENTRY_LIMIT),
            )
        })
    }
    fn read(&self, relative: String, entries: usize) -> BrowseTask {
        BrowseTask::Read {
            location: self.location.clone(),
            relative,
            entries,
        }
    }
    pub fn confirm(&self, source: BoundSource) -> BrowseTask {
        BrowseTask::Confirm {
            location: self.location.clone(),
            source,
        }
    }
    pub fn status(&self) -> &'static str {
        if self.memory_limited || (!self.reached_end && self.allowance == ENTRY_LIMIT) {
            "Folder list limit reached. Use this folder or enter an exact folder path in the profile."
        } else if !self.reached_end {
            "Partial folder list. Load more to look for additional folders."
        } else if !self.complete {
            "Some folder names could not be read. Refresh or choose an available folder."
        } else if self.folders.is_empty() {
            "No subfolders. Use this folder to include its videos."
        } else {
            "Open a subfolder or use this folder."
        }
    }
}

fn same_directory(a: &File, b: &File) -> Result<bool, Error> {
    let a = a.metadata().map_err(io_error)?;
    let b = b.metadata().map_err(io_error)?;
    Ok(a.is_dir() && b.is_dir() && a.dev() == b.dev() && a.ino() == b.ino())
}
fn reader_error(error: readonly::ReadError) -> Error {
    match error {
        readonly::ReadError::Aborted => Error::Cancelled,
        readonly::ReadError::InvalidPath => Error::Invalid,
        _ => Error::Unavailable,
    }
}
fn verify(
    off: &OffFrame,
    location: &Location,
    cancel: &AtomicBool,
    observe: &mut MountProbe<'_>,
    identify: &mut IdentityProbe<'_>,
    verify_root: &mut RootProbe<'_>,
) -> Result<(), Error> {
    check(cancel)?;
    let rows = observe(off, cancel)?;
    if rows.len() > policy::OBSERVATION_LIMIT || !unique_mount(&rows, &location.choice.mount) {
        return Err(Error::Changed);
    }
    let current = location
        .root
        .observe(off, &mut || !cancel.load(Ordering::Acquire))
        .map_err(|_| Error::Cancelled)?;
    if !current.is_live() || !current.same_grant(&location.root) {
        return Err(Error::Changed);
    }
    verify_root(off, &current, &location.choice.mount, cancel)?;
    if let Some(expected) = location
        .root
        .usb_witness()
        .and_then(Witness::stable_binding)
    {
        stable::verify(
            off,
            &rows,
            &location.choice.mount,
            expected,
            cancel,
            identify,
        )?;
    }
    let device =
        identity::directory_device(off, &location.directory).map_err(|_| Error::Unavailable)?;
    if device != location.choice.mount.device() {
        return Err(Error::Changed);
    }
    let root = current
        .root_handle()
        .and_then(|root| root.file_if_live(off))
        .ok_or(Error::Changed)?;
    let named = readonly::open_directory(root, &location.relative, &mut || {
        !cancel.load(Ordering::Acquire) && current.is_live()
    })
    .map_err(reader_error)?;
    if !same_directory(&named, &location.directory)? {
        return Err(Error::Changed);
    }
    check(cancel)
}

pub(super) fn execute(
    off: &OffFrame,
    task: BrowseTask,
    cancel: &AtomicBool,
    observe: &mut MountProbe<'_>,
    identify: &mut IdentityProbe<'_>,
    verify_root: &mut RootProbe<'_>,
) -> Result<Answer, Error> {
    let (mut location, relative, allowance) = match task {
        BrowseTask::Open {
            choice,
            source,
            initial_relative,
        } => {
            let selected = super::execute_probed(
                off,
                Task::Select {
                    choice: choice.clone(),
                    source,
                    relative_folder: String::new(),
                },
                cancel,
                observe,
                identify,
                verify_root,
            )?;
            let Answer::Selected(root) = selected else {
                return Err(Error::Changed);
            };
            let file = root
                .root_handle()
                .and_then(|root| root.file_if_live(off))
                .ok_or(Error::Changed)?;
            let directory =
                readonly::open_directory(file, "", &mut || !cancel.load(Ordering::Acquire))
                    .map_err(reader_error)?;
            (
                Location {
                    choice,
                    root,
                    directory: Arc::new(directory),
                    relative: String::new(),
                },
                initial_relative,
                FIRST_ENTRIES,
            )
        }
        BrowseTask::Read {
            location,
            relative,
            entries,
        } => {
            if !(FIRST_ENTRIES..=ENTRY_LIMIT).contains(&entries) {
                return Err(Error::Invalid);
            }
            (location, relative, entries)
        }
        BrowseTask::Inspect { page } => {
            verify(off, &page.location, cancel, observe, identify, verify_root)?;
            return Ok(Answer::Browse(page));
        }
        BrowseTask::Confirm { location, source } => {
            verify(off, &location, cancel, observe, identify, verify_root)?;
            let answer = super::execute_probed(
                off,
                Task::Select {
                    choice: location.choice.clone(),
                    source,
                    relative_folder: location.relative.clone(),
                },
                cancel,
                observe,
                identify,
                verify_root,
            )?;
            let Answer::Selected(selected) = &answer else {
                return Err(Error::Changed);
            };
            let file = selected
                .root_handle()
                .and_then(|root| root.file_if_live(off))
                .ok_or(Error::Changed)?;
            if !same_directory(file, &location.directory)? {
                return Err(Error::Changed);
            }
            // Confirmation must keep the established identity, not merely
            // return some identity. Compare in filesystem coordinates for the
            // selected child, so A -> B -> A observations cannot install B.
            if let Some(original) = location
                .root
                .usb_witness()
                .and_then(Witness::stable_binding)
            {
                let expected = UsbBinding::from_observed(
                    &original.filesystem_type,
                    &original.uuid,
                    location.choice.mount.root(),
                    &location.relative,
                )
                .map_err(|_| Error::Changed)?;
                if selected.usb_witness().and_then(Witness::stable_binding) != Some(&expected) {
                    return Err(Error::Changed);
                }
            }
            verify(off, &location, cancel, observe, identify, verify_root)?;
            return Ok(answer);
        }
    };
    check(cancel)?;
    // Validate the mount/root before navigating, but permit Parent/Root if the
    // formerly displayed child vanished. The newly opened child is checked below.
    let rows = observe(off, cancel)?;
    if rows.len() > policy::OBSERVATION_LIMIT || !unique_mount(&rows, &location.choice.mount) {
        return Err(Error::Changed);
    }
    let current = location
        .root
        .observe(off, &mut || !cancel.load(Ordering::Acquire))
        .map_err(|_| Error::Cancelled)?;
    if !current.is_live() || !current.same_grant(&location.root) {
        return Err(Error::Changed);
    }
    let root = current
        .root_handle()
        .and_then(|root| root.file_if_live(off))
        .ok_or(Error::Changed)?;
    let file = readonly::open_directory(root, &relative, &mut || {
        !cancel.load(Ordering::Acquire) && current.is_live()
    })
    .map_err(reader_error)?;
    location.directory = Arc::new(file);
    location.relative = relative;
    verify(off, &location, cancel, observe, identify, verify_root)?;
    let budget = Budget::start(off, Duration::from_secs(8));
    let mut folders = Vec::new();
    let mut bytes = 0usize;
    let mut memory_limited = false;
    let visit = readonly::visit_bounded(
        &location.directory,
        "",
        allowance,
        &mut || !cancel.load(Ordering::Acquire) && location.root.is_live() && !budget.expired(off),
        &mut |entry, _| {
            if let Some(entry) = entry.filter(|entry| entry.kind == EntryKind::Directory) {
                if folders.len() == FOLDER_LIMIT
                    || bytes.saturating_add(entry.name.len()) > NAME_BYTES
                {
                    memory_limited = true;
                    return Ok(false);
                }
                folders
                    .try_reserve(1)
                    .map_err(|_| readonly::ReadError::AllocationFailed)?;
                bytes += entry.name.len();
                folders.push(entry.name);
            }
            Ok(true)
        },
    )
    .map_err(reader_error)?;
    folders.sort_unstable();
    verify(off, &location, cancel, observe, identify, verify_root)?;
    Ok(Answer::Browse(BrowsePage {
        location,
        folders,
        allowance,
        reached_end: visit.reached_end,
        complete: visit.complete,
        memory_limited,
    }))
}

#[cfg(test)]
#[path = "ott_usb_browser_tests.rs"]
mod tests;
