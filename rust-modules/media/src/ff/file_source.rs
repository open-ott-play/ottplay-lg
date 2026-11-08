//! File transport shared by inspection and playback. A held source never opens
//! its display path again; each reader has an independent positional cursor.
use plx_base::task::OffFrame;
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::os::unix::fs::FileExt;
use std::path::{Path, PathBuf};
use std::sync::{atomic::{AtomicBool, Ordering}, Arc};

struct Held {
    file: File,
    size: i64,
    path: PathBuf,
    withdrawn: Arc<AtomicBool>,
}

/// An opened regular file and the existing source owner's withdrawal latch.
/// This is neither serializable nor a grant to reopen its private display path.
#[derive(Clone)]
pub struct HeldLocalFile(Arc<Held>);
impl HeldLocalFile {
    /// The caller opens `file` by no-follow descent from its authorized root on
    /// its IO worker. `path` is only a display/route label, never opened here.
    pub fn from_authorized(
        _off: &OffFrame,
        file: File,
        path: PathBuf,
        withdrawn: Arc<AtomicBool>,
    ) -> io::Result<Self> {
        if !path.is_absolute() || path.components().any(|part| matches!(part,
            std::path::Component::ParentDir | std::path::Component::CurDir))
            || path.to_str().is_none_or(|value| value.chars().any(char::is_control))
        {
            return Err(io::ErrorKind::InvalidInput.into());
        }
        if withdrawn.load(Ordering::Acquire) { return Err(io::ErrorKind::PermissionDenied.into()); }
        let size = regular_size(&file)?;
        if withdrawn.load(Ordering::Acquire) { return Err(io::ErrorKind::PermissionDenied.into()); }
        Ok(Self(Arc::new(Held { file, size, path, withdrawn })))
    }
    pub fn is_live(&self) -> bool { !self.0.withdrawn.load(Ordering::Acquire) }
    pub(crate) fn path(&self) -> &Path { &self.0.path }
}
impl PartialEq for HeldLocalFile {
    fn eq(&self, other: &Self) -> bool { Arc::ptr_eq(&self.0, &other.0) }
}
impl Eq for HeldLocalFile {}
impl std::fmt::Debug for HeldLocalFile {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.write_str("HeldLocalFile(<private>)")
    }
}

enum Backing { Owned(File), Held(HeldLocalFile) }
pub(super) struct FileSource { backing: Backing, size: i64, position: u64 }
fn regular_size(file: &File) -> io::Result<i64> {
    let metadata = file.metadata()?;
    if !metadata.is_file() { return Err(io::ErrorKind::InvalidInput.into()); }
    i64::try_from(metadata.len()).map_err(|_| io::ErrorKind::InvalidInput.into())
}
impl FileSource {
    /// Compatibility path for existing synthetic/developer sources. Application
    /// Local media uses `held` after authorized root-relative descent instead.
    pub(super) fn open(path: &Path) -> io::Result<Self> {
        if !path.is_absolute() { return Err(io::ErrorKind::InvalidInput.into()); }
        let file = File::open(path)?;
        let size = regular_size(&file)?;
        Ok(Self { backing: Backing::Owned(file), size, position: 0 })
    }
    pub(super) fn held(file: &HeldLocalFile) -> io::Result<Self> {
        if !file.is_live() { return Err(io::ErrorKind::PermissionDenied.into()); }
        Ok(Self { backing: Backing::Held(file.clone()), size: file.0.size, position: 0 })
    }
    pub(super) fn size(&self) -> i64 { self.size }
    fn live(&self) -> io::Result<()> {
        match &self.backing {
            Backing::Held(file) if !file.is_live() => Err(io::ErrorKind::PermissionDenied.into()),
            _ => Ok(()),
        }
    }
    fn file(&self) -> &File {
        match &self.backing { Backing::Owned(file) => file, Backing::Held(file) => &file.0.file }
    }
}
impl Read for FileSource {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        self.live()?;
        let remaining = (self.size as u64).saturating_sub(self.position);
        let take = usize::try_from(remaining).unwrap_or(usize::MAX).min(bytes.len());
        let read = self.file().read_at(&mut bytes[..take], self.position)?;
        // A withdrawal racing IO discards that result; it never admits bytes
        // through a newly selected/reinserted source at the old path.
        self.live()?;
        self.position += read as u64;
        Ok(read)
    }
}
impl Seek for FileSource {
    fn seek(&mut self, from: SeekFrom) -> io::Result<u64> {
        self.live()?;
        let next = match from {
            SeekFrom::Start(value) => i128::from(value),
            SeekFrom::End(delta) => i128::from(self.size) + i128::from(delta),
            SeekFrom::Current(delta) => i128::from(self.position) + i128::from(delta),
        };
        if !(0..=i128::from(i64::MAX)).contains(&next) {
            return Err(io::ErrorKind::InvalidInput.into());
        }
        self.position = next as u64;
        Ok(self.position)
    }
}

#[cfg(test)]
#[path = "file_source_tests.rs"]
mod tests;
