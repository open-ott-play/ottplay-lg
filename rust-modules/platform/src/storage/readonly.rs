//! Bounded read-only descent from an already authorized directory descriptor.
//!
//! The caller owns root selection, volume/incarnation evidence, source generation,
//! nested-mount policy and the result's final admission. This module never opens
//! a root pathname or changes ownership/permissions. A held descriptor pins the
//! opened object, not a mount incarnation or an immutable version of its contents.
//! Calls belong on an existing IO worker: a regular-file syscall can still block
//! even when a cancellation/deadline check is made immediately around it.
use std::fs::File;
use std::io::{self, Read};
use std::os::fd::AsRawFd;
use std::path::{Component, Path};

// Match the explicit large-file ABI without enabling time64 (which older webOS
// libc does not provide). No casts between stat/dirent layouts are permitted.
#[cfg(all(target_os = "linux", target_env = "gnu", target_pointer_width = "32"))]
use libc::{fstatat64 as stat_at, stat64 as NativeStat};
#[cfg(not(all(target_os = "linux", target_env = "gnu", target_pointer_width = "32")))]
use libc::{fstatat as stat_at, stat as NativeStat};

// Resource bounds only, matching the native Local path/depth envelope. These
// limits do not identify a source or prove that it is currently authorized.
const MAX_PATH_BYTES: usize = 8192;
const MAX_DIRECTORY_DEPTH: usize = 64;
const READ_CHUNK_BYTES: usize = 64 * 1024;

/// No filenames, root paths or underlying error messages cross this boundary.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReadError {
    InvalidPath,
    InvalidRoot,
    NotRegular,
    TooLarge,
    Aborted,
    AllocationFailed,
    Io(io::ErrorKind),
}
fn io_error(error: io::Error) -> ReadError {
    ReadError::Io(error.kind())
}
impl From<io::Error> for ReadError {
    fn from(error: io::Error) -> Self {
        io_error(error)
    }
}
fn checkpoint(check: &mut dyn FnMut() -> bool) -> Result<(), ReadError> {
    if check() {
        Ok(())
    } else {
        Err(ReadError::Aborted)
    }
}
fn names(relative: &str) -> Result<Vec<&str>, ReadError> {
    if relative.is_empty()
        || relative.len() > MAX_PATH_BYTES
        || relative.contains(['\\', '\0'])
        || relative.chars().any(char::is_control)
        || (relative.len() > 1 && relative.as_bytes()[1] == b':')
    {
        return Err(ReadError::InvalidPath);
    }
    // Path::components normalizes repeated separators and interior '.', so
    // reject those before iterating components rather than accepting aliases.
    let parts: Vec<_> = relative.split('/').collect();
    if parts.len().saturating_sub(1) > MAX_DIRECTORY_DEPTH
        || parts
            .iter()
            .any(|p| p.is_empty() || *p == "." || *p == "..")
        || Path::new(relative)
            .components()
            .any(|c| !matches!(c, Component::Normal(_)))
    {
        return Err(ReadError::InvalidPath);
    }
    Ok(parts)
}
/// Open an existing regular file by no-follow descent from the authorized root.
/// The caller owns the returned descriptor and must retain its source authority
/// for every subsequent read; this function never grants a pathname reopen.
pub fn open_regular(
    root: &File,
    relative: &str,
    check: &mut dyn FnMut() -> bool,
) -> Result<File, ReadError> {
    let parts = names(relative)?;
    checkpoint(check)?;
    if !root.metadata().map_err(io_error)?.is_dir() {
        return Err(ReadError::InvalidRoot);
    }
    checkpoint(check)?;
    let mut parent: Option<File> = None;
    for (index, name) in parts.iter().enumerate() {
        checkpoint(check)?;
        let last = index + 1 == parts.len();
        // Reuse storage's raw descriptor helper, not its owned-record loader.
        // open_at adds O_NOFOLLOW | O_CLOEXEC | O_NONBLOCK to every component.
        let opened = super::open_at(
            parent.as_ref().unwrap_or(root),
            name,
            libc::O_RDONLY | if last { 0 } else { libc::O_DIRECTORY },
            0,
        )
        .map_err(io_error)?;
        checkpoint(check)?;
        let metadata = opened.metadata().map_err(io_error)?;
        if last {
            if !metadata.is_file() {
                return Err(ReadError::NotRegular);
            }
            checkpoint(check)?;
            return Ok(opened);
        }
        if !metadata.is_dir() {
            return Err(ReadError::InvalidRoot);
        }
        parent = Some(opened);
    }
    Err(ReadError::InvalidPath)
}
fn read_open_file(
    mut file: File,
    max_bytes: usize,
    check: &mut dyn FnMut() -> bool,
) -> Result<Vec<u8>, ReadError> {
    let metadata = file.metadata().map_err(io_error)?;
    if !metadata.is_file() {
        return Err(ReadError::NotRegular);
    }
    if metadata.len() > max_bytes as u64 {
        return Err(ReadError::TooLarge);
    }
    let mut bytes = Vec::new();
    let mut chunk = [0u8; READ_CHUNK_BYTES];
    loop {
        checkpoint(check)?;
        // At the boundary read exactly one lookahead byte. Never calculate
        // max_bytes + 1, which could overflow a caller's usize limit.
        let remaining = max_bytes - bytes.len();
        let take = remaining.saturating_add(1).min(chunk.len());
        let length = match file.read(&mut chunk[..take]) {
            Ok(length) => length,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(io_error(error)),
        };
        checkpoint(check)?;
        if length == 0 {
            return Ok(bytes);
        }
        if length > remaining {
            return Err(ReadError::TooLarge);
        }
        let required = bytes.len() + length; // bounded above by max_bytes
        if bytes.capacity() < required {
            let target = bytes
                .capacity()
                .max(READ_CHUNK_BYTES)
                .saturating_mul(2)
                .min(max_bytes)
                .max(required);
            bytes
                .try_reserve_exact(target - bytes.len())
                .map_err(|_| ReadError::AllocationFailed)?;
        }
        bytes.extend_from_slice(&chunk[..length]);
    }
}

/// Read the same regular-file descriptor obtained by no-follow root-relative
/// descent. `max_bytes` is a caller's format-specific hard byte limit. `check`
/// returns true only while its cancellation, deadline and authority remain valid.
/// It runs around each open/read and before returning bytes; it cannot interrupt
/// a blocked filesystem syscall. Callers must also reject stale result stamps.
/// No content is cached, no directory is enumerated, and no path is reopened.
pub fn read_bounded(
    root: &File,
    relative: &str,
    max_bytes: usize,
    check: &mut dyn FnMut() -> bool,
) -> Result<Vec<u8>, ReadError> {
    let file = open_regular(root, relative, check)?;
    read_open_file(file, max_bytes, check)
}

/// Observed type only; a later read still requires its own no-follow descent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EntryKind {
    RegularFile,
    Directory,
    Symlink,
    Other,
    Unreadable,
}

#[derive(Debug)]
pub struct DirectoryEntry {
    pub name: String,
    pub kind: EntryKind,
}

#[derive(Debug)]
pub struct DirectoryListing {
    pub entries: Vec<DirectoryEntry>,
    /// True only on actual EOF, with no omitted names or failed observations.
    pub complete: bool,
}

/// Open one directory by confined no-follow descent from the caller's held
/// root. The returned descriptor pins that directory, not immutable contents;
/// root authority, nested-mount policy and final admission remain the caller's.
pub fn open_directory(
    root: &File,
    relative: &str,
    check: &mut dyn FnMut() -> bool,
) -> Result<File, ReadError> {
    checkpoint(check)?;
    if !root.metadata().map_err(io_error)?.is_dir() {
        return Err(ReadError::InvalidRoot);
    }
    checkpoint(check)?;
    let parts = if relative.is_empty() { Vec::new() } else { names(relative)? };
    if parts.len() > MAX_DIRECTORY_DEPTH {
        return Err(ReadError::InvalidPath);
    }
    // A new open-file description is intentional: dup(root) would share the
    // directory offset, making the next observation start at the previous EOF.
    let mut directory = super::open_at(root, ".", libc::O_RDONLY | libc::O_DIRECTORY, 0)
        .map_err(io_error)?;
    checkpoint(check)?;
    for name in parts {
        directory = super::open_at(&directory, name, libc::O_RDONLY | libc::O_DIRECTORY, 0)
            .map_err(io_error)?;
        checkpoint(check)?;
    }
    Ok(directory)
}

/// Outcome of a streamed directory observation. `reached_end` distinguishes
/// actual EOF from the entry bound or a caller-requested stop. `complete` also
/// requires every observed name/type to be representable and readable.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DirectoryVisit {
    pub reached_end: bool,
    pub complete: bool,
}

/// One owned directory stream. It can move between workers, but no two calls
/// may access its readdir state concurrently. Type probes only share the held FD.
pub struct DirectoryCursor {
    directory: std::sync::Arc<File>,
    stream: *mut libc::DIR,
}
// readdir/closedir operate on this exclusively owned stream; next requires &mut.
unsafe impl Send for DirectoryCursor {}
impl Drop for DirectoryCursor {
    fn drop(&mut self) { unsafe { libc::closedir(self.stream); } }
}

pub struct DirectoryName {
    pub name: Option<String>,
    pub bytes: usize,
}
impl DirectoryCursor {
    pub fn open(root: &File, relative: &str, check: &mut dyn FnMut() -> bool) -> Result<Self, ReadError> {
        let directory = std::sync::Arc::new(open_directory(root, relative, check)?);
        checkpoint(check)?;
        let duplicate = unsafe { libc::fcntl(directory.as_raw_fd(), libc::F_DUPFD_CLOEXEC, 0) };
        if duplicate < 0 { return Err(io_error(io::Error::last_os_error())); }
        let stream = unsafe { libc::fdopendir(duplicate) };
        if stream.is_null() {
            let error = io::Error::last_os_error();
            unsafe { libc::close(duplicate); }
            return Err(io_error(error));
        }
        let cursor = Self { directory, stream };
        checkpoint(check)?;
        Ok(cursor)
    }
    pub fn directory(&self) -> std::sync::Arc<File> { self.directory.clone() }
    /// Read one non-dot name, without stat/open of that child. A slow child
    /// probe therefore cannot hold the iterator needed to find a ready sibling.
    pub fn next(&mut self, check: &mut dyn FnMut() -> bool) -> Result<Option<DirectoryName>, ReadError> {
        loop {
            checkpoint(check)?;
            unsafe {
                #[cfg(target_os = "macos")]
                let errno = libc::__error();
                #[cfg(not(target_os = "macos"))]
                let errno = libc::__errno_location();
                *errno = 0;
            }
            let entry = unsafe { super::read_directory_entry(self.stream) };
            let error = entry.is_null().then(io::Error::last_os_error);
            checkpoint(check)?;
            if let Some(error) = error {
                return if error.raw_os_error().unwrap_or(0) == 0 { Ok(None) } else { Err(io_error(error)) };
            }
            let raw = unsafe { std::ffi::CStr::from_ptr((*entry).d_name.as_ptr()) };
            if raw.to_bytes() == b"." || raw.to_bytes() == b".." { continue; }
            let bytes = raw.to_bytes().len();
            let name = raw.to_str().ok().filter(|name| valid_component(name));
            let name = match name {
                Some(name) => {
                    let mut value = String::new();
                    value.try_reserve_exact(name.len()).map_err(|_| ReadError::AllocationFailed)?;
                    value.push_str(name); Some(value)
                }
                None => None,
            };
            return Ok(Some(DirectoryName { name, bytes }));
        }
    }
}
fn valid_component(name: &str) -> bool {
    !name.is_empty() && name != "." && name != ".." && !name.contains(['/', '\\'])
        && !name.chars().any(char::is_control)
}
/// No-follow observation under the exact directory retained by the cursor.
/// No root path is reconstructed and no body/thumbnail/duration is read.
pub fn probe_kind(directory: &File, name: &str, check: &mut dyn FnMut() -> bool) -> Result<EntryKind, ReadError> {
    if !valid_component(name) { return Err(ReadError::InvalidPath); }
    let raw = std::ffi::CString::new(name).map_err(|_| ReadError::InvalidPath)?;
    checkpoint(check)?;
    let mut stat = std::mem::MaybeUninit::<NativeStat>::uninit();
    let result = unsafe { stat_at(directory.as_raw_fd(), raw.as_ptr(), stat.as_mut_ptr(), libc::AT_SYMLINK_NOFOLLOW) };
    checkpoint(check)?;
    Ok(if result != 0 { EntryKind::Unreadable } else {
        match unsafe { stat.assume_init() }.st_mode & libc::S_IFMT {
            libc::S_IFREG => EntryKind::RegularFile,
            libc::S_IFDIR => EntryKind::Directory,
            libc::S_IFLNK => EntryKind::Symlink,
            _ => EntryKind::Other,
        }
    })
}

/// Visit one directory beneath the held root without collecting its entries.
/// At most `max_entries` non-dot names are delivered. The callback receives
/// `None` for an unrepresentable name and the original name's byte length, so
/// callers can account for skipped entries and impose their own aggregate bound.
/// A failed type observation is delivered as `EntryKind::Unreadable`.
/// Returning false stops immediately. This opens no file bodies and creates no
/// new authority; the caller's current grant/cancellation belongs in `check`.
pub fn visit_bounded(
    root: &File,
    parent_relative: &str,
    max_entries: usize,
    check: &mut dyn FnMut() -> bool,
    visit: &mut dyn FnMut(Option<DirectoryEntry>, usize) -> Result<bool, ReadError>,
) -> Result<DirectoryVisit, ReadError> {
    let mut cursor = DirectoryCursor::open(root, parent_relative, check)?;
    let mut examined = 0usize;
    let mut complete = true;
    while let Some(entry) = cursor.next(check)? {
        if examined == max_entries { return Ok(DirectoryVisit { reached_end: false, complete: false }); }
        examined += 1;
        let value = match entry.name {
            Some(name) => {
                let kind = probe_kind(&cursor.directory, &name, check)?;
                if kind == EntryKind::Unreadable { complete = false; }
                Some(DirectoryEntry { name, kind })
            }
            None => { complete = false; None }
        };
        let keep_going = visit(value, entry.bytes)?;
        checkpoint(check)?;
        if !keep_going { return Ok(DirectoryVisit { reached_end: false, complete: false }); }
    }
    Ok(DirectoryVisit { reached_end: true, complete })
}

/// Observe just one directory beneath an authorized root, never a recursive
/// library scan. Bounds apply to examined names and their aggregate bytes;
/// overflow is an incomplete observation. File bodies are never opened here.
/// Cancellation, deadline and current authority belong in `check`, as for reads.
pub fn list_bounded(
    root: &File,
    parent_relative: &str,
    max_entries: usize,
    max_name_bytes: usize,
    check: &mut dyn FnMut() -> bool,
) -> Result<DirectoryListing, ReadError> {
    let mut entries = Vec::new();
    let mut name_bytes = 0usize;
    let outcome = visit_bounded(root, parent_relative, max_entries, check, &mut |entry, bytes| {
        if bytes > max_name_bytes - name_bytes { return Ok(false); }
        name_bytes += bytes;
        if let Some(entry) = entry {
            entries.try_reserve(1).map_err(|_| ReadError::AllocationFailed)?;
            entries.push(entry);
        }
        Ok(true)
    })?;
    Ok(DirectoryListing { entries, complete: outcome.complete })
}

#[cfg(test)]
#[path = "readonly_tests.rs"]
mod tests;

#[cfg(test)]
mod cursor_tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    struct Fixture(std::path::PathBuf);
    impl Fixture {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!("ott-cursor-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
            std::fs::create_dir(&path).unwrap(); Self(path)
        }
        fn root(&self) -> File { File::open(&self.0).unwrap() }
    }
    impl Drop for Fixture { fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); } }

    #[test]
    fn one_step_cursor_does_not_require_the_previous_child_to_remain_readable() {
        let fixture = Fixture::new();
        for name in ["first.mp4", "second.mp4"] { std::fs::write(fixture.0.join(name), b"media").unwrap(); }
        let root = fixture.root();
        let mut cursor = DirectoryCursor::open(&root, "", &mut || true).unwrap();
        let first = cursor.next(&mut || true).unwrap().unwrap().name.unwrap();
        std::fs::remove_file(fixture.0.join(&first)).unwrap();
        let second = cursor.next(&mut || true).unwrap().unwrap().name.unwrap();
        assert_ne!(first, second);
        assert_eq!(probe_kind(&cursor.directory(), &first, &mut || true), Ok(EntryKind::Unreadable));
        assert_eq!(probe_kind(&cursor.directory(), &second, &mut || true), Ok(EntryKind::RegularFile));
        assert!(cursor.next(&mut || true).unwrap().is_none());
        // A fresh cursor owns an independent offset even after the old EOF.
        let mut fresh = DirectoryCursor::open(&root, "", &mut || true).unwrap();
        assert_eq!(fresh.next(&mut || true).unwrap().unwrap().name.as_deref(), Some(second.as_str()));
    }

    #[test]
    fn probe_is_no_follow_and_rechecks_authority_after_the_observation() {
        let fixture = Fixture::new();
        std::fs::write(fixture.0.join("movie.mp4"), b"media").unwrap();
        std::os::unix::fs::symlink("movie.mp4", fixture.0.join("link.mp4")).unwrap();
        let root = fixture.root();
        assert_eq!(probe_kind(&root, "link.mp4", &mut || true), Ok(EntryKind::Symlink));
        assert_eq!(probe_kind(&root, "../movie.mp4", &mut || true), Err(ReadError::InvalidPath));
        let mut checks = 0;
        assert_eq!(probe_kind(&root, "movie.mp4", &mut || { checks += 1; checks == 1 }), Err(ReadError::Aborted));
    }

    #[test]
    fn bounded_adapter_keeps_early_stop_distinct_from_complete_eof() {
        let fixture = Fixture::new();
        std::fs::write(fixture.0.join("movie.mp4"), b"media").unwrap();
        let root = fixture.root();
        let bounded = visit_bounded(&root, "", 0, &mut || true, &mut |_, _| panic!("zero limit")).unwrap();
        assert!(!bounded.reached_end && !bounded.complete);
        let stopped = visit_bounded(&root, "", 2, &mut || true, &mut |_, _| Ok(false)).unwrap();
        assert!(!stopped.reached_end && !stopped.complete);
        let complete = visit_bounded(&root, "", 2, &mut || true, &mut |_, _| Ok(true)).unwrap();
        assert!(complete.reached_end && complete.complete);
    }
}
