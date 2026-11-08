//! Optional, read-only libudev identity probe in the application's namespace.
//! No raw block opens, subprocess, monitor, filesystem scan or private DB parser.

use crate::tv::removable::identity::{
    resolve_with, IdentityError, IdentityObservation, StableIdentity, FILESYSTEM_LIMIT, UUID_LIMIT,
};
use crate::tv::removable::{MountCandidate, PATH_LIMIT};
use plx_base::task::OffFrame;
use std::io::Read;
use std::os::raw::{c_char, c_int, c_void};
use std::os::unix::fs::{FileTypeExt, MetadataExt};
use std::path::{Path, PathBuf};

plx_base::dynlib! {
    udev: ["libudev.so.1", "libudev.so.0"] {
        fn udev_new() -> *mut c_void;
        fn udev_unref(ctx: *mut c_void) -> *mut c_void;
        fn udev_device_new_from_devnum(ctx: *mut c_void, kind: c_char, dev: libc::dev_t) -> *mut c_void;
        fn udev_device_unref(dev: *mut c_void) -> *mut c_void;
        fn udev_device_get_is_initialized(dev: *mut c_void) -> c_int;
        fn udev_device_get_devnum(dev: *mut c_void) -> libc::dev_t;
        fn udev_device_get_syspath(dev: *mut c_void) -> *const c_char;
        fn udev_device_get_subsystem(dev: *mut c_void) -> *const c_char;
        fn udev_device_get_property_value(dev: *mut c_void, key: *const c_char) -> *const c_char;
    }
}

const CACHE_FILE_LIMIT: u64 = 256 * 1024;
const DEVNUM_TEXT_LIMIT: usize = 22; // two u32 decimal values, ':', newline

fn loaded() -> bool {
    static OK: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *OK.get_or_init(|| udev::load(None).ok())
}

struct Context(*mut c_void);
impl Context {
    fn new() -> Result<Self, IdentityError> {
        if !loaded() {
            return Err(IdentityError::Unsupported);
        }
        let raw = unsafe { udev_new() };
        if raw.is_null() {
            Err(IdentityError::Unavailable)
        } else {
            Ok(Self(raw))
        }
    }
}
impl Drop for Context {
    fn drop(&mut self) {
        unsafe {
            udev_unref(self.0);
        }
    }
}
struct Device<'a> {
    raw: *mut c_void,
    _context: &'a Context,
}
impl<'a> Device<'a> {
    fn new(context: &'a Context, dev: libc::dev_t) -> Result<Self, IdentityError> {
        let raw = unsafe { udev_device_new_from_devnum(context.0, b'b' as c_char, dev) };
        if raw.is_null() {
            Err(IdentityError::Unavailable)
        } else {
            Ok(Self {
                raw,
                _context: context,
            })
        }
    }
    fn property(&self, key: &'static [u8], limit: usize) -> Result<String, IdentityError> {
        // All callers pass a literal NUL-terminated property name. Returned
        // storage belongs to this live Device and is copied before any unref.
        unsafe {
            copy_string(
                udev_device_get_property_value(self.raw, key.as_ptr().cast()),
                limit,
            )
        }
    }
}
impl Drop for Device<'_> {
    fn drop(&mut self) {
        unsafe {
            udev_device_unref(self.raw);
        }
    }
}

/// Safety: ptr must be NULL or a live libudev NUL-terminated string. Copy stops
/// at NUL and never retains a library-owned pointer beyond the owning Device.
unsafe fn copy_string(ptr: *const c_char, limit: usize) -> Result<String, IdentityError> {
    if ptr.is_null() {
        return Err(IdentityError::Unavailable);
    }
    let mut bytes = Vec::new();
    for at in 0..=limit {
        let byte = unsafe { *ptr.add(at) as u8 };
        if byte == 0 {
            return String::from_utf8(bytes).map_err(|_| IdentityError::Invalid);
        }
        if at == limit {
            return Err(IdentityError::Invalid);
        }
        bytes.push(byte);
    }
    Err(IdentityError::Invalid)
}

#[derive(Clone, PartialEq, Eq)]
struct Stamp {
    dev: u64,
    ino: u64,
    len: u64,
    modified: (i64, i64),
}
fn stamp(meta: &std::fs::Metadata) -> Stamp {
    Stamp {
        dev: meta.dev(),
        ino: meta.ino(),
        len: meta.len(),
        modified: (meta.mtime(), meta.mtime_nsec()),
    }
}

fn trusted(meta: &std::fs::Metadata) -> bool {
    trusted_mode(meta.uid(), meta.mode())
}

fn trusted_mode(uid: u32, mode: u32) -> bool {
    uid == 0 && mode & 0o022 == 0
}

fn trusted_directory(path: &Path) -> Result<(), IdentityError> {
    let canonical = std::fs::canonicalize(path).map_err(|_| IdentityError::Unavailable)?;
    // Reject a writable ancestor even when it points at a root-owned target.
    // /var/run may legitimately be a system-owned alias of /run.
    for ancestor in canonical.ancestors() {
        let meta = std::fs::metadata(ancestor).map_err(|_| IdentityError::Unavailable)?;
        if !meta.is_dir() || !trusted(&meta) {
            return Err(IdentityError::Unavailable);
        }
    }
    Ok(())
}

/// Check system ownership without interpreting libudev's private DB format.
/// Both known runtime roots are inspected; a visible unsafe one is not skipped.
/// The library itself chooses its configured root. If that root is not exposed
/// in this namespace it cannot supply initialized properties and we refuse.
fn cache_stamps(device: (u32, u32)) -> Result<Vec<(PathBuf, Stamp)>, IdentityError> {
    let mut result = Vec::new();
    for root in ["/run/udev/data", "/var/run/udev/data"] {
        let base = Path::new(root);
        let meta = match std::fs::metadata(base) {
            Ok(meta) => meta,
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::NotFound | std::io::ErrorKind::PermissionDenied
                ) =>
            {
                continue
            }
            Err(_) => return Err(IdentityError::Unavailable),
        };
        if !meta.is_dir() || !trusted(&meta) {
            return Err(IdentityError::Unavailable);
        }
        trusted_directory(base)?;
        let path = base.join(format!("b{}:{}", device.0, device.1));
        let meta = match std::fs::symlink_metadata(&path) {
            Ok(meta) => meta,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(_) => return Err(IdentityError::Unavailable),
        };
        if !meta.is_file() || !trusted(&meta) || meta.len() > CACHE_FILE_LIMIT {
            return Err(IdentityError::Unavailable);
        }
        result.push((path, stamp(&meta)));
    }
    if result.is_empty() {
        Err(IdentityError::Unavailable)
    } else {
        Ok(result)
    }
}

#[derive(PartialEq, Eq)]
struct SysfsStamp {
    path: PathBuf,
    stamp: Stamp,
}
fn sysfs_stamp(device: (u32, u32)) -> Result<SysfsStamp, IdentityError> {
    let path = std::fs::canonicalize(format!("/sys/dev/block/{}:{}", device.0, device.1))
        .map_err(|_| IdentityError::Unavailable)?;
    let Some(text) = path.to_str() else {
        return Err(IdentityError::Invalid);
    };
    if text.len() > PATH_LIMIT
        || !path.starts_with("/sys/devices")
        || path == Path::new("/sys/devices")
    {
        return Err(IdentityError::Unavailable);
    }
    let meta = std::fs::metadata(&path).map_err(|_| IdentityError::Unavailable)?;
    if !meta.is_dir() || !trusted(&meta) {
        return Err(IdentityError::Unavailable);
    }
    let file = std::fs::File::open(path.join("dev")).map_err(|_| IdentityError::Unavailable)?;
    let mut bytes = Vec::new();
    file.take((DEVNUM_TEXT_LIMIT + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|_| IdentityError::Unavailable)?;
    if bytes.len() > DEVNUM_TEXT_LIMIT || parse_dev(&bytes) != Some(device) {
        return Err(IdentityError::Changed);
    }
    Ok(SysfsStamp {
        path,
        stamp: stamp(&meta),
    })
}

fn parse_dev(bytes: &[u8]) -> Option<(u32, u32)> {
    let text = std::str::from_utf8(bytes).ok()?.strip_suffix('\n')?;
    let (major, minor) = text.split_once(':')?;
    if major.is_empty()
        || minor.is_empty()
        || !major
            .bytes()
            .chain(minor.bytes())
            .all(|b| b.is_ascii_digit())
    {
        return None;
    }
    Some((major.parse().ok()?, minor.parse().ok()?))
}

/// A visible by-UUID alias must agree; a jail hiding the whole directory does
/// not lose the libudev path. A symlink is corroboration, not uniqueness proof.
fn check_alias(identity: &StableIdentity, device: libc::dev_t) -> Result<(), IdentityError> {
    let base = Path::new("/dev/disk/by-uuid");
    let meta = match std::fs::metadata(base) {
        Ok(meta) => meta,
        Err(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::PermissionDenied
            ) =>
        {
            return Ok(())
        }
        Err(_) => return Err(IdentityError::Unavailable),
    };
    if !meta.is_dir() || !trusted(&meta) {
        return Err(IdentityError::Unavailable);
    }
    trusted_directory(base)?;
    let alias = base.join(identity.uuid());
    let link = std::fs::symlink_metadata(&alias).map_err(|_| IdentityError::Unavailable)?;
    if !link.file_type().is_symlink() || link.uid() != 0 {
        return Err(IdentityError::Unavailable);
    }
    let target = std::fs::canonicalize(&alias).map_err(|_| IdentityError::Unavailable)?;
    if !target.starts_with("/dev") {
        return Err(IdentityError::Changed);
    }
    let meta = std::fs::metadata(&target).map_err(|_| IdentityError::Unavailable)?;
    if !meta.file_type().is_block_device() || meta.uid() != 0 || block_number(&meta) != Some(device)
    {
        return Err(IdentityError::Changed);
    }
    Ok(())
}

// Native port implementations are Linux. Keeping its dev_t conversion here
// avoids truncating Linux major/minor values through Darwin's signed dev_t in
// host builds; no Linux library is ever treated as a host device backend.
#[cfg(target_os = "linux")]
fn device_number(device: (u32, u32)) -> Result<libc::dev_t, IdentityError> {
    Ok(libc::makedev(device.0, device.1))
}
#[cfg(not(target_os = "linux"))]
fn device_number(_: (u32, u32)) -> Result<libc::dev_t, IdentityError> {
    Err(IdentityError::Unsupported)
}
#[cfg(target_os = "linux")]
fn block_number(meta: &std::fs::Metadata) -> Option<libc::dev_t> {
    Some(meta.rdev())
}
#[cfg(not(target_os = "linux"))]
fn block_number(_: &std::fs::Metadata) -> Option<libc::dev_t> {
    None
}

fn read_identity(context: &Context, device: (u32, u32)) -> Result<StableIdentity, IdentityError> {
    let before = sysfs_stamp(device)?;
    let cache = cache_stamps(device)?;
    // libc supplies the host/target dev_t layout; no hand-coded Linux bit shifts.
    let dev = device_number(device)?;
    let object = Device::new(context, dev)?;
    if unsafe { udev_device_get_is_initialized(object.raw) } <= 0 {
        return Err(IdentityError::Unavailable);
    }
    if unsafe { udev_device_get_devnum(object.raw) } != dev {
        return Err(IdentityError::Changed);
    }
    let sys_path = unsafe { copy_string(udev_device_get_syspath(object.raw), PATH_LIMIT)? };
    if Path::new(&sys_path) != before.path {
        return Err(IdentityError::Changed);
    }
    let subsystem =
        unsafe { copy_string(udev_device_get_subsystem(object.raw), FILESYSTEM_LIMIT)? };
    if subsystem != "block" || object.property(b"ID_FS_USAGE\0", FILESYSTEM_LIMIT)? != "filesystem"
    {
        return Err(IdentityError::Unavailable);
    }
    let identity = StableIdentity::new(
        &object.property(b"ID_FS_UUID\0", UUID_LIMIT)?,
        &object.property(b"ID_FS_TYPE\0", FILESYSTEM_LIMIT)?,
    )?;
    check_alias(&identity, dev)?;
    if sysfs_stamp(device)? != before || cache_stamps(device)? != cache {
        return Err(IdentityError::Changed);
    }
    Ok(identity)
}

pub fn identity_snapshot(
    _off: &OffFrame,
    mounts: &[MountCandidate],
    check: &mut dyn FnMut() -> bool,
) -> Result<Vec<IdentityObservation>, IdentityError> {
    if !check() {
        return Err(IdentityError::Cancelled);
    }
    let context = Context::new()?;
    resolve_with(mounts, check, &mut |device| read_identity(&context, device))
}

#[cfg(test)]
#[path = "removable_identity_tests.rs"]
mod tests;
