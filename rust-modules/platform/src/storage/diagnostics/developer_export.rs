//! Best-effort developer-only mirror of the existing typed, capped storage snapshot.
//! No secret/state files are read. Visibility across an actual TV's jail is not assumed.
use super::*;

fn denied() -> io::Error {
    io::Error::from_raw_os_error(libc::EPERM)
}

fn child_name(app_id: &str) -> io::Result<CString> {
    let stable = plx_base::paths::STABLE_APP_ID;
    if app_id != stable && !matches!(app_id.strip_prefix(stable), Some(".debug" | ".nightly")) {
        return Err(denied());
    }
    CString::new(format!("{app_id}-storage-diagnostics")).map_err(|_| denied())
}

fn open_directory_at(parent: &File, name: &CString) -> io::Result<File> {
    let fd = unsafe {
        libc::openat(parent.as_raw_fd(), name.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
    };
    if fd < 0 { Err(io::Error::last_os_error()) }
    else { Ok(unsafe { File::from_raw_fd(fd) }) }
}

fn trusted_ancestor(uid: u32, gid: u32, mode: u32, own_uid: u32, shared: bool) -> bool {
    if uid != 0 && uid != own_uid { return false; }
    // The fixed developer ancestor can be root:root 0775 on the TV.
    // Only that administrative group may write a non-shared ancestor.
    shared || (mode & 0o002 == 0 && (mode & 0o020 == 0 || (uid == 0 && gid == 0)))
}

#[cfg(all(feature = "devtriggers", target_os = "linux", target_arch = "arm", not(feature = "hostsim")))]
pub(super) fn shared_parent() -> io::Result<File> {
    let mut parent = directory(Path::new("/"))?;
    // Never create, chmod or follow symlinks in these platform-owned ancestors.
    for (name, shared) in [("media", false), ("developer", false), ("temp", true)] {
        parent = open_directory_at(&parent, &CString::new(name).unwrap())?;
        let meta = parent.metadata()?;
        if !trusted_ancestor(meta.uid(), meta.gid(), meta.mode(), unsafe { libc::geteuid() }, shared) {
            return Err(denied());
        }
    }
    Ok(parent)
}

pub(super) fn owned_directory(parent: &File, app_id: &str, create: bool) -> io::Result<File> {
    let name = child_name(app_id)?;
    if create && unsafe { libc::mkdirat(parent.as_raw_fd(), name.as_ptr(), 0o750) } != 0 {
        let error = io::Error::last_os_error();
        if error.raw_os_error() != Some(libc::EEXIST) { return Err(error); }
    }
    let dir = open_directory_at(parent, &name)?;
    let meta = dir.metadata()?;
    if !meta.is_dir() || meta.uid() != unsafe { libc::geteuid() }
        || meta.gid() != unsafe { libc::getegid() }
    {
        return Err(denied());
    }
    // Never widen a directory: mkdir success cannot identify the later-opened inode
    // in a non-sticky shared parent. A restrictive umask deliberately refuses export.
    if dir.metadata()?.mode() & 0o7777 != 0o750 { return Err(denied()); }
    Ok(dir)
}

fn publish_at(parent: &File, app_id: &str, snapshot: &Snapshot,
    published: &mut Option<Published>) -> io::Result<()> {
    let _block = guard();
    let _publication = PUBLICATION.lock().unwrap_or_else(|e| e.into_inner());
    if DISABLED.load(Ordering::Acquire) {
        return Err(io::Error::from_raw_os_error(libc::ECANCELED));
    }
    let semantic = snapshot.semantic();
    if *published == Some(semantic) { return Ok(()); }
    let dir = owned_directory(parent, app_id, true)?;
    // Same formatter, 16KiB cap, 0640 mode and checked atomic writer as the private sink.
    publish_in(&dir, snapshot.format().as_bytes(), || Ok(()))?;
    *published = Some(semantic);
    Ok(())
}

#[cfg(all(feature = "devtriggers", target_os = "linux", target_arch = "arm", not(feature = "hostsim")))]
pub(super) fn publish_changed(snapshot: &Snapshot, published: &mut Option<Published>) {
    if DISABLED.load(Ordering::Acquire) || *published == Some(snapshot.semantic()) { return; }
    if let Ok(parent) = shared_parent() {
        // Best effort only: export failure cannot turn Save success into failure or vice versa.
        let _ = publish_at(&parent, plx_base::paths::app_id(), snapshot, published);
    }
}

#[cfg(all(feature = "devtriggers", target_os = "linux", target_arch = "arm", not(feature = "hostsim")))]
pub(super) fn publish_canary(
    version: &str, request_id: &str, elapsed_ms: u128,
    result: &Result<super::super::wire::CanaryResult, super::super::client::ClientError>,
) -> io::Result<()> {
    let _block = guard();
    let parent = shared_parent()?;
    publish_canary_at(&parent, plx_base::paths::app_id(), version, request_id, elapsed_ms, result)
}

fn publish_canary_at(
    parent: &File, app_id: &str, version: &str, request_id: &str, elapsed_ms: u128,
    result: &Result<super::super::wire::CanaryResult, super::super::client::ClientError>,
) -> io::Result<()> {
    let _block = guard();
    let bytes = format_canary(app_id, version, request_id, elapsed_ms, result)?;
    let _publication = PUBLICATION.lock().unwrap_or_else(|e| e.into_inner());
    if DISABLED.load(Ordering::Acquire) {
        return Err(io::Error::from_raw_os_error(libc::ECANCELED));
    }
    let dir = owned_directory(parent, app_id, true)?;
    // The same bounded atomic publisher, with a closed separate leaf. The ordinary
    // diagnostic snapshot is neither read nor overwritten; no worker is started.
    publish_artifact_in(&dir, Artifact::Canary, &bytes, || Ok(()))
}

// Caller holds PUBLICATION after disable(). Remove only the fixed owned artifacts;
// never recursively clean the shared parent or the dedicated directory.
pub(super) fn remove_at(parent: &File, app_id: &str) -> io::Result<()> {
    let dir = match owned_directory(parent, app_id, false) {
        Ok(dir) => dir,
        Err(error) if error.raw_os_error() == Some(libc::ENOENT) => return Ok(()),
        Err(error) => return Err(error),
    };
    // Try all owned fixed leaves even if one is unsafe/unavailable. Report the
    // first failure; never erase a symlink target or claim erasure was complete.
    let mut result = Ok(());
    for artifact in [Artifact::Snapshot, Artifact::Canary,
        Artifact::UiSnapshotMetadata, Artifact::UiSnapshotPng] {
        let removed = remove_artifact_in(&dir, artifact);
        if result.is_ok() { result = removed; }
    }
    result
}

#[cfg(all(feature = "devtriggers", target_os = "linux", target_arch = "arm", not(feature = "hostsim")))]
pub(super) fn remove() -> io::Result<()> {
    match shared_parent() {
        Ok(parent) => remove_at(&parent, plx_base::paths::app_id()),
        Err(error) if error.raw_os_error() == Some(libc::ENOENT) => Ok(()),
        Err(error) => Err(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::{symlink, PermissionsExt};

    #[test]
    fn developer_ancestors_allow_only_root_group_writes_outside_shared_temp() {
        assert!(trusted_ancestor(0, 0, 0o775, 6000, false));
        assert!(trusted_ancestor(6000, 5000, 0o555, 6000, false));
        assert!(!trusted_ancestor(0, 5000, 0o775, 6000, false));
        assert!(!trusted_ancestor(6000, 5000, 0o775, 6000, false));
        assert!(!trusted_ancestor(0, 0, 0o777, 6000, false));
        assert!(!trusted_ancestor(6001, 5000, 0o555, 6000, false));
        assert!(trusted_ancestor(0, 0, 0o777, 6000, true));
    }

    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let root = std::env::temp_dir().join(unique().to_str().unwrap());
            std::fs::create_dir(&root).unwrap();
            Self(root)
        }
        fn parent(&self) -> File { directory(&self.0).unwrap() }
        fn slot(&self) -> PathBuf {
            self.0.join(child_name(plx_base::paths::STABLE_APP_ID).unwrap().to_str().unwrap())
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); }
    }
    struct Enabled;
    impl Enabled {
        fn new() -> Self { DISABLED.store(false, Ordering::Release); Self }
    }
    impl Drop for Enabled {
        fn drop(&mut self) { DISABLED.store(false, Ordering::Release); }
    }

    #[test]
    fn developer_export_is_typed_capped_group_readable_and_independent_of_private_sink() {
        let _serial = plx_base::testlock::serial();
        let _enabled = Enabled::new();
        let f = Fixture::new();
        let mut snapshot = super::super::tests::fixture();
        let mut private = None;
        snapshot.publish_changed(&f.0.join("missing-runtime"), &mut private);
        assert!(private.is_none());
        let mut exported = None;
        publish_at(&f.parent(), plx_base::paths::STABLE_APP_ID, &snapshot, &mut exported).unwrap();
        let path = f.slot().join(NAME);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), snapshot.format());
        assert_eq!(std::fs::metadata(f.slot()).unwrap().mode() & 0o7777, 0o750);
        let meta = std::fs::metadata(&path).unwrap();
        assert_eq!(meta.mode() & 0o7777, 0o640);
        assert_eq!((meta.uid(), meta.gid(), meta.nlink()),
            (unsafe { libc::geteuid() }, unsafe { libc::getegid() }, 1));
        assert!(meta.len() <= LIMIT as u64);
        let inode = meta.ino();
        publish_at(&f.parent(), plx_base::paths::STABLE_APP_ID, &snapshot, &mut exported).unwrap();
        assert_eq!(std::fs::metadata(path).unwrap().ino(), inode, "unchanged state is not rewritten");
    }

    #[test]
    fn developer_export_refuses_directory_and_leaf_aliases_without_touching_targets() {
        let _serial = plx_base::testlock::serial();
        let _enabled = Enabled::new();
        let f = Fixture::new();
        let other = Fixture::new();
        symlink(&other.0, f.slot()).unwrap();
        let snapshot = super::super::tests::fixture();
        assert!(publish_at(&f.parent(), plx_base::paths::STABLE_APP_ID, &snapshot, &mut None).is_err());
        assert_eq!(std::fs::read_dir(&other.0).unwrap().count(), 0);
        std::fs::remove_file(f.slot()).unwrap();
        let dir = owned_directory(&f.parent(), plx_base::paths::STABLE_APP_ID, true).unwrap();
        let target = other.0.join("untouched");
        std::fs::write(&target, b"sentinel").unwrap();
        symlink(&target, f.slot().join(NAME)).unwrap();
        assert!(publish_at(&f.parent(), plx_base::paths::STABLE_APP_ID, &snapshot, &mut None).is_err());
        assert!(remove_at(&f.parent(), plx_base::paths::STABLE_APP_ID).is_err());
        assert_eq!(std::fs::read(&target).unwrap(), b"sentinel");
        std::fs::remove_file(f.slot().join(NAME)).unwrap();
        std::fs::hard_link(&target, f.slot().join(NAME)).unwrap();
        assert!(publish_at(&f.parent(), plx_base::paths::STABLE_APP_ID, &snapshot, &mut None).is_err());
        assert!(remove_at(&f.parent(), plx_base::paths::STABLE_APP_ID).is_err());
        assert_eq!(std::fs::read(target).unwrap(), b"sentinel");
        drop(dir);
    }

    #[test]
    fn developer_export_refuses_unsafe_existing_mode_and_unknown_identity() {
        let _serial = plx_base::testlock::serial();
        let _enabled = Enabled::new();
        let f = Fixture::new();
        std::fs::create_dir(f.slot()).unwrap();
        for mode in [0o777, 0o700] {
            std::fs::set_permissions(f.slot(), std::fs::Permissions::from_mode(mode)).unwrap();
            assert!(owned_directory(&f.parent(), plx_base::paths::STABLE_APP_ID, true).is_err());
            assert_eq!(std::fs::metadata(f.slot()).unwrap().mode() & 0o7777, mode);
        }
        for bad in ["../other", "com.openottplay.player.native/other", "other", "com.openottplay.player.native.extra"] {
            assert!(child_name(bad).is_err());
        }
        for id in ["com.openottplay.player.native", "com.openottplay.player.native.debug", "com.openottplay.player.native.nightly"] {
            assert!(child_name(id).is_ok());
        }
    }

    #[test]
    fn developer_export_erase_prevents_republication_and_preserves_unrelated_files() {
        let _serial = plx_base::testlock::serial();
        let _enabled = Enabled::new();
        let f = Fixture::new();
        let mut exported = None;
        let mut snapshot = super::super::tests::fixture();
        publish_at(&f.parent(), plx_base::paths::STABLE_APP_ID, &snapshot, &mut exported).unwrap();
        let unrelated = f.slot().join("unrelated");
        std::fs::write(&unrelated, b"sentinel").unwrap();
        disable();
        {
            let _publication = PUBLICATION.lock().unwrap_or_else(|e| e.into_inner());
            remove_at(&f.parent(), plx_base::paths::STABLE_APP_ID).unwrap();
        }
        snapshot.helper.seen = true;
        assert_eq!(publish_at(&f.parent(), plx_base::paths::STABLE_APP_ID, &snapshot, &mut exported)
            .unwrap_err().raw_os_error(), Some(libc::ECANCELED));
        assert!(!f.slot().join(NAME).exists());
        assert_eq!(std::fs::read(unrelated).unwrap(), b"sentinel");
    }
    #[test]
    fn canary_export_is_synchronous_separate_and_erased_with_diagnostics() {
        let _serial = plx_base::testlock::serial();
        let _enabled = Enabled::new();
        let f = Fixture::new();
        let snapshot = super::super::tests::fixture();
        publish_at(&f.parent(), plx_base::paths::STABLE_APP_ID, &snapshot, &mut None).unwrap();
        let ordinary = std::fs::read(f.slot().join(NAME)).unwrap();
        let request = "0123456789abcdef0123456789abcdef";
        publish_canary_at(&f.parent(), plx_base::paths::STABLE_APP_ID, "0.3.0", request, 31,
            &Err(super::super::super::client::ClientError::Unavailable)).unwrap();
        let path = f.slot().join(CANARY_NAME);
        let value: serde_json::Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(value["request_id"], request);
        assert_eq!(value["outcome"]["cleanup"], "unknown");
        assert_eq!(std::fs::read(f.slot().join(NAME)).unwrap(), ordinary);
        let meta = std::fs::metadata(&path).unwrap();
        assert_eq!(meta.mode() & 0o7777, 0o640);
        assert_eq!((meta.uid(), meta.gid(), meta.nlink()),
            (unsafe { libc::geteuid() }, unsafe { libc::getegid() }, 1));
        assert!(meta.len() <= LIMIT as u64);
        let unrelated = f.slot().join("unrelated");
        std::fs::write(&unrelated, b"sentinel").unwrap();
        disable();
        {
            let _publication = PUBLICATION.lock().unwrap_or_else(|e| e.into_inner());
            remove_at(&f.parent(), plx_base::paths::STABLE_APP_ID).unwrap();
        }
        assert!(!path.exists());
        assert!(!f.slot().join(NAME).exists());
        assert_eq!(std::fs::read(unrelated).unwrap(), b"sentinel");
        assert_eq!(publish_canary_at(&f.parent(), plx_base::paths::STABLE_APP_ID, "0.3.0", request, 31,
            &Err(super::super::super::client::ClientError::Unavailable))
            .unwrap_err().raw_os_error(), Some(libc::ECANCELED));
        assert!(!path.exists());
    }

    #[test]
    fn canary_export_refuses_aliases_and_never_touches_normal_snapshot() {
        let _serial = plx_base::testlock::serial();
        let _enabled = Enabled::new();
        let f = Fixture::new();
        let other = Fixture::new();
        let target = other.0.join("target");
        std::fs::write(&target, b"sentinel").unwrap();
        let snapshot = super::super::tests::fixture();
        publish_at(&f.parent(), plx_base::paths::STABLE_APP_ID, &snapshot, &mut None).unwrap();
        let ordinary = std::fs::read(f.slot().join(NAME)).unwrap();
        let path = f.slot().join(CANARY_NAME);
        for hard_link in [false, true] {
            if hard_link { std::fs::hard_link(&target, &path).unwrap(); }
            else { symlink(&target, &path).unwrap(); }
            assert!(publish_canary_at(&f.parent(), plx_base::paths::STABLE_APP_ID, "0.3.0",
                "0123456789abcdef0123456789abcdef", 0,
                &Err(super::super::super::client::ClientError::Unavailable)).is_err());
            assert_eq!(std::fs::read(&target).unwrap(), b"sentinel");
            assert_eq!(std::fs::read(f.slot().join(NAME)).unwrap(), ordinary);
            std::fs::remove_file(&path).unwrap();
        }
    }

}
