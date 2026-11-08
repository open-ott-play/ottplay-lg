//! Read-only proof for a root-owned bundle that JsonStore correctly refuses to write.
//! This exception never deletes a name and is never available to record saves.
use crate::storage::{CommitStage, StoreError};
use std::{
    fs::{File, OpenOptions},
    io,
    os::{
        fd::AsRawFd,
        unix::fs::{MetadataExt, OpenOptionsExt},
    },
    path::Path,
};

fn trusted_absence_root(uid: u32, gid: u32, mode: u32) -> bool {
    uid == 0 && gid == 0 && mode & 0o002 == 0
}

fn check_root(path: &Path, directory: &File) -> Result<(), StoreError> {
    let named = std::fs::symlink_metadata(path)
        .map_err(|e| crate::storage::io_error(CommitStage::ParentOpen, e))?;
    let held = directory
        .metadata()
        .map_err(|e| crate::storage::io_error(CommitStage::ParentOpen, e))?;
    if !named.is_dir() || !held.is_dir() || named.dev() != held.dev() || named.ino() != held.ino() {
        return Err(StoreError::RootChanged);
    }
    if !trusted_absence_root(held.uid(), held.gid(), held.mode()) {
        return Err(StoreError::RootUnsafeMode);
    }
    Ok(())
}

fn absent(directory: &File) -> Result<(), StoreError> {
    for name in [
        b"ottplay-control.json\0".as_slice(),
        b"ottplay-profile.json\0".as_slice(),
    ] {
        // No open of the child: even special files or dangling symlinks are
        // simply present and refuse the proof. Only ENOENT proves absence.
        let mut metadata = std::mem::MaybeUninit::<libc::stat>::uninit();
        let result = unsafe {
            libc::fstatat(
                directory.as_raw_fd(),
                name.as_ptr().cast(),
                metadata.as_mut_ptr(),
                libc::AT_SYMLINK_NOFOLLOW,
            )
        };
        if result == 0 {
            return Err(StoreError::Conflict);
        }
        let error = io::Error::last_os_error();
        if error.kind() != io::ErrorKind::NotFound {
            return Err(crate::storage::io_error(CommitStage::Readback, error));
        }
    }
    Ok(())
}

fn prove_with(
    directory: &File,
    check: impl Fn() -> Result<(), StoreError>,
    sync: impl Fn(&File) -> io::Result<()>,
) -> Result<(), StoreError> {
    check()?;
    absent(directory)?;
    sync(directory).map_err(|e| crate::storage::io_error(CommitStage::ParentSync, e))?;
    check()?;
    absent(directory)?;
    check()
}

pub(super) fn prove(path: &Path) -> Result<(), StoreError> {
    let directory = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK)
        .open(path)
        .map_err(|e| crate::storage::io_error(CommitStage::ParentOpen, e))?;
    prove_with(&directory, || check_root(path, &directory), File::sync_all)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::{state::Generation, JsonStore};
    struct Directory(std::path::PathBuf);
    impl Directory {
        fn new() -> Self {
            let suffix: String = Generation::random()
                .unwrap()
                .0
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect();
            let path = std::env::temp_dir().join(format!("ott-absence-{suffix}"));
            std::fs::create_dir(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for Directory {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn special_absence_policy_does_not_change_jsonstore_write_policy() {
        assert!(trusted_absence_root(0, 0, 0o40775));
        assert!(!crate::storage::trusted_root(0, 0, 0o40775, 1000));
        for (uid, gid, mode) in [(0, 0, 0o40777), (1000, 0, 0o40775), (0, 1000, 0o40775)] {
            assert!(!trusted_absence_root(uid, gid, mode));
        }
    }

    #[test]
    fn absent_names_still_require_successful_parent_sync_on_retry() {
        let directory = Directory::new();
        let store = JsonStore::new(directory.0.clone()).unwrap();
        for _ in 0..2 {
            assert!(matches!(
                prove_with(
                    &store.directory,
                    || store.check_root(),
                    |_| Err(io::Error::from_raw_os_error(libc::EIO))
                ),
                Err(StoreError::Io {
                    stage: CommitStage::ParentSync,
                    ..
                })
            ));
        }
        prove_with(&store.directory, || store.check_root(), File::sync_all).unwrap();
    }

    #[test]
    fn present_file_symlink_directory_or_fifo_is_not_removed() {
        let directory = Directory::new();
        let store = JsonStore::new(directory.0.clone()).unwrap();
        let name = directory.0.join("ottplay-control.json");
        std::fs::write(&name, b"synthetic retained file").unwrap();
        assert_eq!(
            prove_with(&store.directory, || store.check_root(), File::sync_all),
            Err(StoreError::Conflict)
        );
        assert_eq!(std::fs::read(&name).unwrap(), b"synthetic retained file");
        std::fs::remove_file(&name).unwrap();
        std::os::unix::fs::symlink(directory.0.join("missing"), &name).unwrap();
        assert_eq!(
            prove_with(&store.directory, || store.check_root(), File::sync_all),
            Err(StoreError::Conflict)
        );
        assert!(std::fs::symlink_metadata(&name).unwrap().is_symlink());
        std::fs::remove_file(&name).unwrap();
        std::fs::create_dir(&name).unwrap();
        assert_eq!(
            prove_with(&store.directory, || store.check_root(), File::sync_all),
            Err(StoreError::Conflict)
        );
        assert!(name.is_dir());
        std::fs::remove_dir(&name).unwrap();
        use std::os::unix::ffi::OsStrExt;
        let c_name = std::ffi::CString::new(name.as_os_str().as_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(c_name.as_ptr(), 0o600) }, 0);
        assert_eq!(
            prove_with(&store.directory, || store.check_root(), File::sync_all),
            Err(StoreError::Conflict)
        );
        use std::os::unix::fs::FileTypeExt;
        assert!(std::fs::symlink_metadata(&name)
            .unwrap()
            .file_type()
            .is_fifo());
    }

    #[test]
    fn a_name_appearing_during_sync_invalidates_the_proof() {
        let directory = Directory::new();
        let store = JsonStore::new(directory.0.clone()).unwrap();
        let name = directory.0.join("ottplay-profile.json");
        let result = prove_with(
            &store.directory,
            || store.check_root(),
            |file| {
                std::fs::write(&name, b"late synthetic file")?;
                file.sync_all()
            },
        );
        assert_eq!(result, Err(StoreError::Conflict));
        assert!(name.is_file());
    }

    #[test]
    fn parent_replacement_during_sync_invalidates_the_proof() {
        let directory = Directory::new();
        let app = directory.0.join("app");
        std::fs::create_dir(&app).unwrap();
        let store = JsonStore::new(app.clone()).unwrap();
        let result = prove_with(
            &store.directory,
            || store.check_root(),
            |file| {
                std::fs::rename(&app, directory.0.join("old"))?;
                std::fs::create_dir(&app)?;
                file.sync_all()
            },
        );
        assert_eq!(result, Err(StoreError::RootChanged));
        let regular = directory.0.join("regular");
        std::fs::write(&regular, b"not a directory").unwrap();
        assert!(matches!(
            prove(&regular),
            Err(StoreError::Io {
                stage: CommitStage::ParentOpen,
                errno: libc::ENOTDIR
            })
        ));
        let alias = directory.0.join("alias");
        std::os::unix::fs::symlink(&app, &alias).unwrap();
        assert!(prove(&alias).is_err());
    }
}
