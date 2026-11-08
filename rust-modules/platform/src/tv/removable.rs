//! Pure parsing of a bounded, complete mount snapshot supplied by the port's
//! existing off-frame IO runner. No discovery thread, filesystem read or grant.
//!
//! A path in the platform's USB mount tree is only a candidate. The runner must
//! separately establish readable traversal and volume identity. Mount IDs,
//! device numbers and mount paths are not persistent identities and may recur.

pub mod identity;

use std::collections::HashSet;
use std::fmt;

pub(crate) const INPUT_LIMIT: usize = 1024 * 1024;
const MOUNT_LIMIT: usize = 4096;
pub(crate) const PATH_LIMIT: usize = 4096;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SnapshotError {
    TooLarge,
    TooManyMounts,
    InvalidRecord,
    DuplicateMount,
    InvalidPath,
}

/// Receiver-private observation, not a source registration or playback grant.
/// Debug deliberately omits all paths and device identifiers.
#[derive(Clone, PartialEq, Eq)]
pub struct MountCandidate {
    mount_id: u64,
    parent_id: u64,
    device: (u32, u32),
    root: String,
    mount_point: String,
    filesystem: String,
}

impl fmt::Debug for MountCandidate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MountCandidate").finish_non_exhaustive()
    }
}

impl MountCandidate {
    /// Synthetic observation only: tests still need actual RootFD acquisition.
    #[cfg(any(test, feature = "test-support"))]
    pub fn directory_for_test(directory: &str, mount_id: u64) -> Result<Self, SnapshotError> {
        Self::fixture_directory(directory, mount_id, (8, 1))
    }
    /// Synthetic mount metadata with the actual fixture directory's device.
    #[cfg(any(test, feature = "test-support"))]
    pub fn directory_device_for_test(directory: &str, mount_id: u64, device: (u32, u32)) -> Result<Self, SnapshotError> {
        Self::fixture_directory(directory, mount_id, device)
    }
    /// Explicit host fixture metadata, never a filesystem access grant.
    #[cfg(any(test, feature = "test-support", all(feature = "hostsim", feature = "devtriggers", any(target_os = "macos", target_os = "linux"))))]
    pub(in crate::tv) fn fixture_directory(directory: &str, mount_id: u64, device: (u32, u32)) -> Result<Self, SnapshotError> {
        if mount_id == 0 { return Err(SnapshotError::InvalidRecord); }
        let escaped = directory.replace('\\', "\\134").replace(' ', "\\040");
        let directory = path(&escaped)?;
        Ok(Self { mount_id, parent_id: 1, device, root: "/".into(), mount_point: directory, filesystem: "ext4".into() })
    }
    pub fn mount_id(&self) -> u64 {
        self.mount_id
    }
    pub fn parent_id(&self) -> u64 {
        self.parent_id
    }
    pub fn device(&self) -> (u32, u32) {
        self.device
    }
    pub fn root(&self) -> &str {
        &self.root
    }
    pub fn mount_point(&self) -> &str {
        &self.mount_point
    }
    pub fn filesystem(&self) -> &str {
        &self.filesystem
    }
}

fn number<T: std::str::FromStr>(value: &str) -> Result<T, SnapshotError> {
    if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(SnapshotError::InvalidRecord);
    }
    value.parse().map_err(|_| SnapshotError::InvalidRecord)
}

fn path(value: &str) -> Result<String, SnapshotError> {
    if value.len() > PATH_LIMIT * 4 {
        return Err(SnapshotError::InvalidPath);
    }
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len().min(PATH_LIMIT));
    let mut index = 0;
    while index < bytes.len() {
        let byte = if bytes[index] == b'\\' {
            let escape = bytes
                .get(index + 1..index + 4)
                .ok_or(SnapshotError::InvalidPath)?;
            index += 4;
            match escape {
                b"040" => b' ',
                b"011" => b'\t',
                b"012" => b'\n',
                b"134" => b'\\',
                _ => return Err(SnapshotError::InvalidPath),
            }
        } else {
            let byte = bytes[index];
            index += 1;
            byte
        };
        if decoded.len() == PATH_LIMIT {
            return Err(SnapshotError::InvalidPath);
        }
        decoded.push(byte);
    }
    let decoded = String::from_utf8(decoded).map_err(|_| SnapshotError::InvalidPath)?;
    if !decoded.starts_with('/')
        || decoded.chars().any(char::is_control)
        || (decoded != "/"
            && decoded
                .split('/')
                .skip(1)
                .any(|part| matches!(part, "" | "." | "..")))
    {
        return Err(SnapshotError::InvalidPath);
    }
    Ok(decoded)
}

fn usb_partition_root(value: &str) -> bool {
    let Some(relative) = value.strip_prefix("/tmp/usb/") else {
        return false;
    };
    let mut parts = relative.split('/');
    matches!((parts.next(), parts.next(), parts.next()),
        (Some(disk), Some(partition), None) if !disk.is_empty() && !partition.is_empty())
}

/// Parse `/proc/self/mountinfo` supplied by the existing IO worker. Input must
/// be a complete snapshot, not a truncated prefix. Any malformed record rejects
/// the snapshot; an error must not be interpreted as proof all disks vanished.
///
/// Only complete partition-root paths in `/tmp/usb/<disk>/<partition>` are
/// returned. A bind root is retained as evidence rather than presumed to be `/`.
/// Nested descendants and unrelated mounts are not promoted into additional
/// sources. Neither a filesystem type nor a mount prefix proves read access.
pub fn candidates(input: &[u8]) -> Result<Vec<MountCandidate>, SnapshotError> {
    if input.len() > INPUT_LIMIT {
        return Err(SnapshotError::TooLarge);
    }
    let text = std::str::from_utf8(input).map_err(|_| SnapshotError::InvalidRecord)?;
    let mut ids = HashSet::new();
    let mut result = Vec::new();
    for (index, line) in text.lines().enumerate() {
        if index >= MOUNT_LIMIT {
            return Err(SnapshotError::TooManyMounts);
        }
        // Kernel fields are separated by ASCII spaces; path whitespace is
        // escaped. Unicode whitespace inside a valid filename is not a field.
        let fields: Vec<_> = line.split(' ').collect();
        if fields.iter().any(|field| field.is_empty()) {
            return Err(SnapshotError::InvalidRecord);
        }
        let split = fields
            .iter()
            .position(|field| *field == "-")
            .ok_or(SnapshotError::InvalidRecord)?;
        if split < 6 || fields.len() != split + 4 {
            return Err(SnapshotError::InvalidRecord);
        }
        let mount_id = number::<u64>(fields[0])?;
        let parent_id = number::<u64>(fields[1])?;
        if mount_id == 0 || !ids.insert(mount_id) {
            return Err(if mount_id == 0 {
                SnapshotError::InvalidRecord
            } else {
                SnapshotError::DuplicateMount
            });
        }
        let (major, minor) = fields[2]
            .split_once(':')
            .ok_or(SnapshotError::InvalidRecord)?;
        let device = (number::<u32>(major)?, number::<u32>(minor)?);
        let root = path(fields[3])?;
        let mount_point = path(fields[4])?;
        let filesystem = fields[split + 1];
        if filesystem.is_empty()
            || filesystem.len() > 64
            || !filesystem
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
        {
            return Err(SnapshotError::InvalidRecord);
        }
        if usb_partition_root(&mount_point) {
            result.push(MountCandidate {
                mount_id,
                parent_id,
                device,
                root,
                mount_point,
                filesystem: filesystem.into(),
            });
        }
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(id: u64, root: &str, mount: &str) -> String {
        format!("{id} 25 8:17 {root} {mount} rw,nosuid shared:2 - vfat /dev/sdb1 rw\n")
    }

    #[test]
    fn only_partition_roots_are_candidates() {
        let text = [
            row(1, "/", "/"),
            row(2, "/", "/tmp/usb"),
            row(3, "/", "/tmp/usb/sdb"),
            row(4, "/", "/tmp/usb/sdb/sdb1"),
            row(5, "/Movies", "/tmp/usb/sdb/sdb2"),
            row(6, "/", "/tmp/usb/sdb/sdb1/nested"),
            row(7, "/", "/media/internal"),
        ]
        .concat();
        let mounts = candidates(text.as_bytes()).unwrap();
        assert_eq!(mounts.len(), 2);
        assert_eq!(mounts[0].mount_id(), 4);
        assert_eq!(mounts[0].parent_id(), 25);
        assert_eq!(mounts[0].device(), (8, 17));
        assert_eq!(mounts[0].filesystem(), "vfat");
        assert_eq!(mounts[0].root(), "/");
        assert_eq!(mounts[0].mount_point(), "/tmp/usb/sdb/sdb1");
        assert_eq!(mounts[1].root(), "/Movies");
        assert_eq!(mounts[1].mount_point(), "/tmp/usb/sdb/sdb2");
    }

    #[test]
    fn disk_letters_and_filesystem_names_are_not_hardcoded() {
        let text = row(1, "/", "/tmp/usb/sdc/sdc3").replace("vfat", "fuseblk");
        assert_eq!(
            candidates(text.as_bytes()).unwrap()[0].filesystem(),
            "fuseblk"
        );
    }

    #[test]
    fn escaped_space_and_unicode_are_not_split_as_fields() {
        let text = row(1, "/", "/tmp/usb/My\\040Disk/Фильмы\u{2003}1");
        assert_eq!(
            candidates(text.as_bytes()).unwrap()[0].mount_point(),
            "/tmp/usb/My Disk/Фильмы\u{2003}1"
        );
    }

    #[test]
    fn malformed_or_unsafe_paths_reject_complete_snapshot() {
        for path in [
            "relative",
            "/tmp/usb/a/../b",
            "/tmp/usb/a//b",
            "/tmp/usb/a/./b",
            "/tmp/usb/a/b/",
            "/tmp/usb/a/b\\012x",
            "/tmp/usb/a/b\\011x",
            "/tmp/usb/a/b\\000x",
            "/tmp/usb/a/b\\13",
        ] {
            assert_eq!(
                candidates(row(1, "/", path).as_bytes()),
                Err(SnapshotError::InvalidPath)
            );
        }
    }

    #[test]
    fn prefix_lookalikes_are_not_usb_candidates() {
        for mount in [
            "/tmp/usbackup/disk/part",
            "/tmp/usb-other/disk/part",
            "/var/tmp/usb/disk/part",
        ] {
            assert!(candidates(row(1, "/", mount).as_bytes())
                .unwrap()
                .is_empty());
        }
    }

    #[test]
    fn duplicates_outside_candidate_tree_also_reject_snapshot() {
        let text = [row(1, "/", "/tmp/usb/a/b"), row(1, "/", "/proc")].concat();
        assert_eq!(
            candidates(text.as_bytes()),
            Err(SnapshotError::DuplicateMount)
        );
    }

    #[test]
    fn malformed_record_is_not_silently_dropped() {
        for text in [
            "1 2 8:1 / /tmp/usb/a/b rw - vfat /dev/sda1",
            "1 2 8:1 / /tmp/usb/a/b rw vfat /dev/sda1 rw",
            "-1 2 8:1 / /tmp/usb/a/b rw - vfat /dev/sda1 rw",
            "1 2 8:1:2 / /tmp/usb/a/b rw - vfat /dev/sda1 rw",
            "1 2 8:1 / /tmp/usb/a/b rw - vfat /dev/sda1 rw extra",
            "1 2 8:1 / /tmp/usb/a/b  rw - vfat /dev/sda1 rw",
        ] {
            assert!(candidates(text.as_bytes()).is_err());
        }
    }

    #[test]
    fn unknown_optional_fields_are_forward_compatible() {
        let text =
            row(1, "/", "/tmp/usb/a/b").replace("shared:2", "master:7 propagate_from:2 future:abc");
        assert_eq!(candidates(text.as_bytes()).unwrap().len(), 1);
    }

    #[test]
    fn size_and_mount_count_are_bounded() {
        assert_eq!(
            candidates(&vec![b' '; INPUT_LIMIT + 1]),
            Err(SnapshotError::TooLarge)
        );
        let text: String = (1..=MOUNT_LIMIT + 1)
            .map(|id| row(id as u64, "/", "/proc"))
            .collect();
        assert_eq!(
            candidates(text.as_bytes()),
            Err(SnapshotError::TooManyMounts)
        );
    }

    #[test]
    fn debug_does_not_disclose_paths_or_device_identity() {
        let value = candidates(row(71, "/", "/tmp/usb/private/private1").as_bytes()).unwrap();
        assert_eq!(format!("{:?}", value[0]), "MountCandidate { .. }");
    }

    #[test]
    fn empty_snapshot_and_readonly_mount_do_not_grant_access() {
        assert!(candidates(b"").unwrap().is_empty());
        let text = row(1, "/", "/tmp/usb/a/b").replace("rw", "ro");
        assert_eq!(candidates(text.as_bytes()).unwrap().len(), 1);
    }
}

/// An unavailable or malformed snapshot is not proof of source removal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReadError {
    Unsupported,
    Unavailable,
    Cancelled,
    Invalid(SnapshotError),
}

/// The application calls this only from its existing Local IO worker.
pub fn observe(
    off: &plx_base::task::OffFrame,
    check: &mut dyn FnMut() -> bool,
) -> Result<Vec<MountCandidate>, ReadError> {
    (super::port().removable_snapshot)(off, check)
}
