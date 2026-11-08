//! The retired helper owns its data until its worker has reaped the child.
//! No selected media path enters this adapter. Cleanup is fd-relative inside
//! the fixed installation-owned ott-companion directory and never follows links.
use super::*;
use std::ffi::CStr;
use std::os::fd::FromRawFd;
use std::path::Component;
use std::sync::mpsc::TryRecvError;

const MAX_ENTRIES: usize = 20_000;
const MAX_DEPTH: usize = 16;
const POLL: Duration = Duration::from_millis(10);
const PENDING: Error = Error("companion_erasure_pending");
const FAILED: Error = Error("companion_erasure_failed");
const UNSAFE: Error = Error("companion_erasure_unsafe");

struct Retired {
    worker: Option<JoinHandle<()>>,
    root: Option<PathBuf>,
}

/// Keep this value for retry after a timeout. Only one cleanup worker can run
/// for it at a time; a timed-out caller does not detach another concurrent wipe.
pub struct ErasureStop {
    retired: Arc<Mutex<Retired>>,
    ticket: Option<Receiver<Result<(), Error>>>,
    complete: bool,
}

impl ErasureStop {
    /// Delete before Runtime construction still owns the install's helper data.
    /// The caller supplies the same trusted root used by Supervisor::new.
    pub(crate) fn owned_data_without_worker(root: PathBuf) -> Self {
        Self::new(None, Some(root))
    }

    pub(super) fn new(worker: Option<JoinHandle<()>>, root: Option<PathBuf>) -> Self {
        let complete = worker.is_none() && root.is_none();
        Self {
            retired: Arc::new(Mutex::new(Retired { worker, root })),
            ticket: None,
            complete,
        }
    }

    /// Storage-worker boundary, never a frame callback. Both waiting and cleanup
    /// have a fixed allowance. A filesystem syscall cannot be interrupted, so
    /// its ticket is retained until completion instead of spawning a duplicate.
    pub fn finish(&mut self, allowance: Duration) -> Result<(), Error> {
        if self.complete {
            return Ok(());
        }
        if let Some(ticket) = &self.ticket {
            match ticket.try_recv() {
                Ok(Ok(())) => {
                    self.complete = true;
                    self.ticket = None;
                    return Ok(());
                }
                Ok(Err(_)) | Err(TryRecvError::Disconnected) => self.ticket = None,
                Err(TryRecvError::Empty) => {}
            }
        }
        if allowance.is_zero() {
            return Err(PENDING);
        }
        if self.ticket.is_none() {
            let retired = self.retired.clone();
            let (sender, receiver) = mpsc::sync_channel(1);
            if !plx_base::task::spawn_off_frame("ott-companion-erasure", move |off| {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    finish_retired(off, &retired, allowance)
                }))
                .unwrap_or(Err(FAILED));
                let _ = sender.send(result);
                plx_machine::idle::invalidate();
            }) {
                return Err(Error("companion_erasure_worker_unavailable"));
            }
            self.ticket = Some(receiver);
        }
        let mut result = None;
        plx_base::task::time::drain_for(
            self.ticket.as_ref().expect("cleanup ticket"),
            allowance,
            |value| {
                result = Some(value);
                false
            },
        );
        match result {
            Some(value) => {
                self.ticket = None;
                self.complete = value.is_ok();
                value
            }
            None => Err(PENDING),
        }
    }
}

fn finish_retired(
    off: &OffFrame,
    retired: &Mutex<Retired>,
    allowance: Duration,
) -> Result<(), Error> {
    let budget = Budget::start(off, allowance);
    // This mutex belongs only to the single cleanup worker, not the UI thread.
    let mut retired = lock(retired);
    while retired
        .worker
        .as_ref()
        .is_some_and(|worker| !worker.is_finished())
    {
        if !budget.wait(off, POLL, POLL, &|| false) {
            return Err(PENDING);
        }
    }
    if let Some(worker) = retired.worker.take() {
        plx_base::task::join("ott-companion-erased-owner", worker);
    }
    if let Some(root) = &retired.root {
        erase_root(off, budget, root)?;
    }
    retired.root = None;
    Ok(())
}

fn check_budget(off: &OffFrame, budget: Budget) -> Result<(), Error> {
    if budget.expired(off) {
        Err(PENDING)
    } else {
        Ok(())
    }
}

fn open_directory(parent: &File, name: &CStr) -> io::Result<File> {
    let fd = unsafe {
        libc::openat(
            parent.as_raw_fd(),
            name.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        )
    };
    if fd < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(unsafe { File::from_raw_fd(fd) })
    }
}

fn entry_stat(parent: &File, name: &CStr) -> io::Result<libc::stat> {
    let mut result = std::mem::MaybeUninit::<libc::stat>::uninit();
    let status = unsafe {
        libc::fstatat(
            parent.as_raw_fd(),
            name.as_ptr(),
            result.as_mut_ptr(),
            libc::AT_SYMLINK_NOFOLLOW,
        )
    };
    if status == 0 {
        Ok(unsafe { result.assume_init() })
    } else {
        Err(io::Error::last_os_error())
    }
}

struct Ancestors {
    directories: Vec<File>,
    names: Vec<CString>,
}

impl Ancestors {
    fn parent(&self) -> &File {
        self.directories
            .last()
            .expect("filesystem root is retained")
    }

    fn validate(&self, off: &OffFrame, budget: Budget) -> Result<(), Error> {
        // An fd still opens the old directory after a rename. Check every named
        // edge back to /, not only the root name inside the retained parent fd.
        for index in (0..self.names.len()).rev() {
            check_budget(off, budget)?;
            same_directory(
                &self.directories[index],
                &self.names[index],
                &self.directories[index + 1],
            )?;
        }
        Ok(())
    }

    fn absent(&self, off: &OffFrame, budget: Budget, name: &CStr) -> Result<(), Error> {
        self.validate(off, budget)?;
        match entry_stat(self.parent(), name) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            _ => return Err(UNSAFE),
        }
        self.validate(off, budget)
    }
}

fn same_directory(parent: &File, name: &CStr, held: &File) -> Result<(), Error> {
    let actual = held.metadata().map_err(|_| FAILED)?;
    let named = entry_stat(parent, name).map_err(|_| UNSAFE)?;
    if named.st_dev as u64 != actual.dev()
        || named.st_ino as u64 != actual.ino()
        || named.st_mode & libc::S_IFMT != libc::S_IFDIR
    {
        return Err(UNSAFE);
    }
    Ok(())
}

fn open_ancestors(off: &OffFrame, budget: Budget, root: &Path) -> Result<Option<Ancestors>, Error> {
    // Only the helper's own fixed directory is eligible, never a Local profile.
    if !root.is_absolute() || root.file_name().is_none_or(|name| name != "ott-companion") {
        return Err(UNSAFE);
    }
    let components: Vec<_> = root.components().collect();
    if components.len() > 64 || !matches!(components.first(), Some(Component::RootDir)) {
        return Err(UNSAFE);
    }
    let mut ancestors = Ancestors {
        directories: vec![File::open("/").map_err(|_| FAILED)?],
        names: Vec::new(),
    };
    for component in &components[1..components.len() - 1] {
        check_budget(off, budget)?;
        let Component::Normal(name) = component else {
            return Err(UNSAFE);
        };
        let name = CString::new(name.as_bytes()).map_err(|_| UNSAFE)?;
        let directory = match open_directory(ancestors.parent(), &name) {
            Ok(directory) => directory,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                ancestors.absent(off, budget, &name)?;
                return Ok(None);
            }
            Err(_) => return Err(UNSAFE),
        };
        ancestors.directories.push(directory);
        ancestors.names.push(name);
    }
    ancestors.validate(off, budget)?;
    Ok(Some(ancestors))
}

fn erase_root(off: &OffFrame, budget: Budget, root: &Path) -> Result<(), Error> {
    let Some(ancestors) = open_ancestors(off, budget, root)? else {
        return Ok(());
    };
    check_budget(off, budget)?;
    let name = CString::new("ott-companion").expect("fixed directory name");
    let directory = match open_directory(ancestors.parent(), &name) {
        Ok(directory) => directory,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            ancestors.parent().sync_all().map_err(|_| FAILED)?;
            return ancestors.absent(off, budget, &name);
        }
        Err(_) => return Err(UNSAFE),
    };
    erase_opened_root(off, budget, &ancestors, &name, &directory)
}

fn erase_opened_root(
    off: &OffFrame,
    budget: Budget,
    ancestors: &Ancestors,
    name: &CStr,
    directory: &File,
) -> Result<(), Error> {
    ancestors.validate(off, budget)?;
    same_directory(ancestors.parent(), name, directory)?;
    let metadata = directory.metadata().map_err(|_| FAILED)?;
    if metadata.uid() != unsafe { libc::geteuid() } || metadata.mode() & 0o022 != 0 {
        return Err(UNSAFE);
    }
    let mut count = 0;
    erase_directory(off, budget, directory, metadata.dev(), 0, &mut count)?;
    ancestors.validate(off, budget)?;
    remove_directory(off, budget, ancestors.parent(), name, directory)?;
    ancestors.parent().sync_all().map_err(|_| FAILED)?;
    ancestors.absent(off, budget, name)
}

struct Directory(*mut libc::DIR);
impl Drop for Directory {
    fn drop(&mut self) {
        unsafe { libc::closedir(self.0) };
    }
}

fn directory_stream(directory: &File) -> Result<Directory, Error> {
    let duplicate = unsafe { libc::fcntl(directory.as_raw_fd(), libc::F_DUPFD_CLOEXEC, 0) };
    if duplicate < 0 {
        return Err(FAILED);
    }
    let stream = unsafe { libc::fdopendir(duplicate) };
    if stream.is_null() {
        unsafe { libc::close(duplicate) };
        return Err(FAILED);
    }
    Ok(Directory(stream))
}

fn next_entry(directory: &Directory) -> Result<Option<CString>, Error> {
    // POSIX distinguishes end-of-directory from an error through errno.
    unsafe {
        #[cfg(target_os = "macos")]
        let errno = libc::__error();
        #[cfg(not(target_os = "macos"))]
        let errno = libc::__errno_location();
        *errno = 0;
        let entry = libc::readdir(directory.0);
        if entry.is_null() {
            return if *errno == 0 { Ok(None) } else { Err(FAILED) };
        }
        Ok(Some(CStr::from_ptr((*entry).d_name.as_ptr()).to_owned()))
    }
}

fn erase_directory(
    off: &OffFrame,
    budget: Budget,
    directory: &File,
    device: u64,
    depth: usize,
    count: &mut usize,
) -> Result<(), Error> {
    if depth > MAX_DEPTH {
        return Err(UNSAFE);
    }
    let entries = directory_stream(directory)?;
    loop {
        check_budget(off, budget)?;
        let Some(name) = next_entry(&entries)? else {
            break;
        };
        if matches!(name.to_bytes(), b"." | b"..") {
            continue;
        }
        *count += 1;
        if *count > MAX_ENTRIES {
            return Err(UNSAFE);
        }
        let metadata = match entry_stat(directory, &name) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(_) => return Err(FAILED),
        };
        if metadata.st_uid != unsafe { libc::geteuid() } || metadata.st_dev as u64 != device {
            return Err(UNSAFE);
        }
        if metadata.st_mode & libc::S_IFMT == libc::S_IFDIR {
            let child = open_directory(directory, &name).map_err(|_| UNSAFE)?;
            let actual = child.metadata().map_err(|_| FAILED)?;
            if actual.dev() != device || actual.ino() != metadata.st_ino as u64 {
                return Err(UNSAFE);
            }
            erase_directory(off, budget, &child, device, depth + 1, count)?;
            remove_directory(off, budget, directory, &name, &child)?;
        } else {
            // unlinkat removes a symlink itself, never the file it names.
            check_budget(off, budget)?;
            if unsafe { libc::unlinkat(directory.as_raw_fd(), name.as_ptr(), 0) } != 0 {
                return Err(FAILED);
            }
        }
    }
    Ok(())
}

fn remove_directory(
    off: &OffFrame,
    budget: Budget,
    parent: &File,
    name: &CStr,
    directory: &File,
) -> Result<(), Error> {
    check_budget(off, budget)?;
    let actual = directory.metadata().map_err(|_| FAILED)?;
    let named = entry_stat(parent, name).map_err(|_| FAILED)?;
    if named.st_dev as u64 != actual.dev()
        || named.st_ino as u64 != actual.ino()
        || named.st_mode & libc::S_IFMT != libc::S_IFDIR
    {
        return Err(UNSAFE);
    }
    if unsafe { libc::unlinkat(parent.as_raw_fd(), name.as_ptr(), libc::AT_REMOVEDIR) } != 0 {
        return Err(FAILED);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replaced_parent_or_ancestor_cannot_report_success_for_the_old_root() {
        for replace_parent in [true, false] {
            let base = plx_base::testscratch::process_dir(if replace_parent {
                "companion-erase-parent-swap"
            } else {
                "companion-erase-ancestor-swap"
            })
            .canonicalize()
            .unwrap();
            let top = base.join("top");
            let parent = top.join("parent");
            let root = parent.join("ott-companion");
            fs::create_dir_all(&root).unwrap();
            fs::write(root.join("old-data"), b"old owned data").unwrap();
            let off = OffFrame::for_test();
            let budget = Budget::start(&off, Duration::from_secs(2));
            let ancestors = open_ancestors(&off, budget, &root).unwrap().unwrap();
            let name = CString::new("ott-companion").unwrap();
            let held = open_directory(ancestors.parent(), &name).unwrap();

            // The previously opened descriptor remains valid, but no longer
            // names the installation path the user asked us to erase.
            let retired = base.join("retired");
            fs::rename(if replace_parent { &parent } else { &top }, &retired).unwrap();
            fs::create_dir_all(&root).unwrap();
            fs::write(root.join("new-data"), b"replacement data").unwrap();
            assert_eq!(
                erase_opened_root(&off, budget, &ancestors, &name, &held),
                Err(UNSAFE)
            );
            assert_eq!(
                fs::read(root.join("new-data")).unwrap(),
                b"replacement data"
            );
            let old_root = if replace_parent {
                retired.join("ott-companion")
            } else {
                retired.join("parent/ott-companion")
            };
            assert_eq!(
                fs::read(old_root.join("old-data")).unwrap(),
                b"old owned data"
            );
        }
    }

    #[test]
    fn root_recreated_after_removal_is_not_proven_absent() {
        let base = plx_base::testscratch::process_dir("companion-erase-root-recreated")
            .canonicalize()
            .unwrap();
        let root = base.join("ott-companion");
        fs::create_dir(&root).unwrap();
        let off = OffFrame::for_test();
        let budget = Budget::start(&off, Duration::from_secs(2));
        let ancestors = open_ancestors(&off, budget, &root).unwrap().unwrap();
        let name = CString::new("ott-companion").unwrap();
        let held = open_directory(ancestors.parent(), &name).unwrap();
        remove_directory(&off, budget, ancestors.parent(), &name, &held).unwrap();
        fs::create_dir(&root).unwrap();
        fs::write(root.join("new-data"), b"replacement data").unwrap();
        assert_eq!(ancestors.absent(&off, budget, &name), Err(UNSAFE));
        assert_eq!(
            fs::read(root.join("new-data")).unwrap(),
            b"replacement data"
        );
    }
}
