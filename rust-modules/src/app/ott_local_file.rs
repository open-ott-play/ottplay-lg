//! Exact-file authority for the existing Local binding worker. The private parent
//! descriptor is used only for identity revalidation, never scanning or assets.
//! Held file identity is not a promise that another writer cannot change bytes.
use super::{checkpoint, observe_directory, open_directory, same_directory, RootError};
use plx_base::task::OffFrame;
use plx_media::ff::HeldLocalFile;
use plx_platform::storage::readonly;
use std::fs::{File, Metadata};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

pub(super) struct SingleFile {
    parent: File,
    parent_path: PathBuf,
    path: PathBuf,
    name: String,
    identity: Metadata,
    held: HeldLocalFile,
    withdrawn: Arc<AtomicBool>,
}

impl SingleFile {
    /// The connection worker has already authorized this exact source and
    /// applied its existing video-extension policy. No sibling is authorized.
    pub(super) fn acquire(
        off: &OffFrame,
        path: &str,
        check: &mut dyn FnMut() -> bool,
    ) -> Result<Self, RootError> {
        checkpoint(check)?;
        let result = (|| {
            let before = observe_regular(path)?;
            let selected = Path::new(path);
            let parent_path = selected.parent().ok_or(RootError::Unavailable)?;
            let parent_text = parent_path.to_str().ok_or(RootError::Unavailable)?;
            let name = selected
                .file_name()
                .and_then(|name| name.to_str())
                .ok_or(RootError::Unavailable)?;
            let parent_before = observe_directory(parent_text)?;
            checkpoint(check)?;
            let parent = open_directory(parent_text)?;
            checkpoint(check)?;
            let parent_opened = parent.metadata().map_err(|_| RootError::Unavailable)?;
            if !same_directory(&parent_before, &parent_opened) {
                return Err(RootError::Unavailable);
            }
            // This existing reader enforces exact normal components, NOFOLLOW,
            // NONBLOCK, regular-file type and cancellation around the open.
            let file = readonly::open_regular(&parent, name, check).map_err(read_error)?;
            let opened = file.metadata().map_err(|_| RootError::Unavailable)?;
            checkpoint(check)?;
            let after = observe_regular(path)?;
            let parent_after = observe_directory(parent_text)?;
            if !same_file(&before, &opened)
                || !same_file(&opened, &after)
                || !same_directory(&parent_opened, &parent_after)
            {
                return Err(RootError::Unavailable);
            }
            checkpoint(check)?;
            let withdrawn = Arc::new(AtomicBool::new(false));
            let held =
                HeldLocalFile::from_authorized(off, file, selected.to_owned(), withdrawn.clone())
                    .map_err(|_| RootError::Unavailable)?;
            Ok(Self {
                parent,
                parent_path: parent_path.to_owned(),
                path: selected.to_owned(),
                name: name.to_owned(),
                identity: opened,
                held,
                withdrawn,
            })
        })();
        // Cancellation wins over a simultaneous failed observation. A discarded
        // candidate owns its own latch and never revokes another live source.
        checkpoint(check)?;
        result
    }

    pub(super) fn is_live(&self) -> bool {
        self.held.is_live()
    }
    pub(super) fn name(&self) -> &str {
        &self.name
    }
    pub(super) fn held(&self) -> HeldLocalFile {
        self.held.clone()
    }
    pub(super) fn withdraw(&self) {
        self.withdrawn.store(true, Ordering::Release);
    }

    /// Reopen only for observing this exact file, never for playback. The media
    /// path retains the original HeldLocalFile, including across seek/reopen.
    pub(super) fn observe(
        &self,
        _off: &OffFrame,
        check: &mut dyn FnMut() -> bool,
    ) -> Result<(), RootError> {
        checkpoint(check)?;
        let result = (|| {
            if !self.is_live() {
                return Err(RootError::Unavailable);
            }
            let parent_text = self.parent_path.to_str().ok_or(RootError::Unavailable)?;
            let path_text = self.path.to_str().ok_or(RootError::Unavailable)?;
            let parent_opened = self.parent.metadata().map_err(|_| RootError::Unavailable)?;
            let parent_before = observe_directory(parent_text)?;
            let before = observe_regular(path_text)?;
            if !same_directory(&parent_opened, &parent_before)
                || !same_file(&self.identity, &before)
            {
                return Err(RootError::Unavailable);
            }
            checkpoint(check)?;
            let observed =
                readonly::open_regular(&self.parent, &self.name, check).map_err(read_error)?;
            let opened = observed.metadata().map_err(|_| RootError::Unavailable)?;
            checkpoint(check)?;
            let after = observe_regular(path_text)?;
            let parent_after = observe_directory(parent_text)?;
            if !self.is_live()
                || !same_file(&self.identity, &opened)
                || !same_file(&opened, &after)
                || !same_directory(&parent_opened, &parent_after)
            {
                return Err(RootError::Unavailable);
            }
            Ok(())
        })();
        checkpoint(check)?;
        result
    }
}

fn read_error(error: readonly::ReadError) -> RootError {
    match error {
        readonly::ReadError::Aborted => RootError::Cancelled,
        _ => RootError::Unavailable,
    }
}

fn observe_regular(path: &str) -> Result<Metadata, RootError> {
    if path.len() > 4096 || path.chars().any(char::is_control) || !Path::new(path).is_absolute() {
        return Err(RootError::Unavailable);
    }
    let canonical = std::fs::canonicalize(path).map_err(|_| RootError::Unavailable)?;
    // Compare spelling too: Path equality normalizes repeated separators/".".
    if canonical.as_os_str() != std::ffi::OsStr::new(path) {
        return Err(RootError::Unavailable);
    }
    let metadata = std::fs::symlink_metadata(path).map_err(|_| RootError::Unavailable)?;
    if !metadata.is_file() {
        return Err(RootError::Unavailable);
    }
    Ok(metadata)
}

fn same_file(first: &Metadata, second: &Metadata) -> bool {
    first.is_file()
        && second.is_file()
        && first.dev() == second.dev()
        && first.ino() == second.ino()
}

#[cfg(test)]
#[path = "ott_local_file_tests.rs"]
mod tests;
