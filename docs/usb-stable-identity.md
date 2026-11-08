# Optional stable USB filesystem identity

This platform extension supplies current filesystem identity evidence to the existing Local source worker. It does not select a source, save a profile, start playback, mount a device, or create a read grant. The integrated Local runtime consumes this evidence for folder reattachment and confirmed persistence, as described in [USB folders](usb-local.md). Those actions remain the runtime's responsibility, not side effects of the identity probe.

## Supported evidence

The native Linux port optionally loads the installed `libudev.so.1` or `libudev.so.0`. A missing library or any missing required symbol returns `Unsupported`. Each call creates a fresh context and fresh device objects; no device identity is cached between calls. A mount must identify a nonzero block-device major number. The device must be initialized, belong to the block subsystem, and expose `ID_FS_USAGE=filesystem`, a bounded `ID_FS_UUID`, and `ID_FS_TYPE` equal to the mount's filesystem type.

The port checks the current canonical `/sys/dev/block/major:minor` target under `/sys/devices`, its `dev` attribute, and its directory identity before and after reading properties. It also checks system ownership, permissions, size, and unchanged metadata of available udev cache records. The cache format is not parsed by the player. When `/dev/disk/by-uuid` is visible, its matching alias must resolve to a root-owned block node with the same device number. A jail that hides that whole directory can still use libudev. A visible contradictory alias is an error.

`identity_snapshot` accepts the **complete** result of the existing bounded mount observer, never a caller-filtered subset. It returns all associated observations or one error. Only the supplied visible USB candidates are covered; this is not enumeration of unmounted devices or other mount namespaces. Unknown identity cannot become a successful prefix. The same UUID on different current devices is ambiguous. Multiple mount aliases of one device share one property read and retain their distinct exposed roots. Duplicate mount paths are ambiguous. UUID spelling is preserved exactly; labels, partition UUIDs, device numbers, mount IDs, and path hashes are not stable filesystem identities.

UUID tokens are nonempty, at most 256 ASCII bytes, and contain letters, digits or `-_.:`; `.` and `..` alone are rejected. Filesystem type uses the mount parser's 64-byte ASCII letters/digits/`._-` envelope. Unsupported spelling is refused rather than normalized or truncated. No driver-name alias is guessed: an `ntfs3` mount with `ID_FS_TYPE=ntfs` is a contradictory type for this backend and is rejected. A FUSE mount without a correlatable block device reports `Unsupported`, allowing explicit manual-only selection. A separately evidenced adapter is needed for automatic identity on those layouts.

## Receiver obligations

`directory_device(off, &file)` checks the already held directory with fstat and returns its native major/minor tuple; it never reopens the pathname.

The receiver must observe mounts and identities again immediately after acquiring the selected root and compare the complete snapshots. It must correlate the held root's device with the observed mount, retain the normal source/generation/cancellation checks, and publish only through the existing Inventory owner. Identity evidence alone never revives a revoked root handle.

For a bind mount exposing `/Media`, selecting its `Movies` child means the filesystem-relative folder `Media/Movies`. Save and later resolve that exact location, not simply `Movies` at whichever mountpoint appears next. A new layout must contain the saved location; missing or incompatible roots cannot fall back to the volume root. Preserve the source/profile IDs and relative-file bookmark, while issuing a fresh root authority on successful reattachment. Legacy path-only records cannot silently gain automatic restore rights.

A protected failed, uncertain, or stale save must not replace the prior root or donate its bookmark to another device. An inactive profile, newer user navigation, Stop, standby, and source revocation keep their existing authority over playback. The platform function creates no scanner, timer, worker, registry, or automatic playback.

## Limits of the evidence

This is a logical filesystem UUID supplied by the operating system, not cryptographic authentication of physical hardware. A sole replacement containing an offline clone of the filesystem UUID cannot be distinguished by UUID alone. Simultaneously visible duplicate UUIDs are rejected.

`udev_device_get_is_initialized` establishes that udev has initialized a record. It does not settle all pending hotplug events or independently read the filesystem superblock. The before/after checks reject observable change during a probe; they do not claim a kernel-atomic snapshot. Missing, stale-looking, inaccessible, inconsistent or incomplete evidence is unavailable. The player never invokes `blkid`, opens a block device, rewrites jail permissions, or interprets the private udev database as an alternate backend.

LG's published native-jail configurations expose `/sys/dev` and `/sys/devices` read-only and `/var/run` read/write. They do not establish that every firmware exposes `/run/udev`, a compatible library, or suitable current properties. The library chooses its own compiled cache path; the player does not redirect it. If the jail exposes only an unusable alias, automatic identity remains unavailable. Device validation is pending.

## Verification status

Twenty-one focused test definitions cover bounds, exact identity preservation, duplicate devices, bind aliases, conflicting filesystem types, cancellation, whole-response refusal, C-string lifetime copying, sysfs device parsing and cache permission policy. At the original identity-backend handoff these tests were written, not compiled or executed. Later qualification must identify the exact source and binary tested; this document does not certify the final 0.3.0 package. Rustfmt and isolated patch checks belong to their corresponding packet manifest. Mac/host fixtures cannot prove the target library ABI, jail visibility, hotplug behavior, or decoder support. This change adds no codec/container support.

## Primary references

- [libudev device constructors and ownership](https://www.freedesktop.org/software/systemd/man/latest/udev_device_new_from_devnum.html)
- [Public libudev v252 header](https://github.com/systemd/systemd/blob/v252/src/libudev/libudev.h)
- [libudev v183 implementation](https://github.com/systemd/systemd/blob/v183/src/libudev/libudev-device.c)
- [v252 device database access and initialization](https://github.com/systemd/systemd/blob/v252/src/libsystemd/sd-device/sd-device.c)
- [Udev rules and public library access](https://www.freedesktop.org/software/systemd/man/latest/udev.html)
- [Kernel sysfs device rules](https://docs.kernel.org/admin-guide/sysfs-rules.html)
- [Kernel sysfs major/minor mapping](https://docs.kernel.org/filesystems/sysfs.html)
- [Mountinfo fields, including exposed root](https://man7.org/linux/man-pages/man5/proc_pid_mountinfo.5.html)
- [Persistent-storage rules that create by-UUID aliases](https://github.com/systemd/systemd/blob/v252/rules.d/60-persistent-storage.rules)
- [LG native-jail configuration, webOS 4.5](https://developer.lge.com/common/file/DownloadFile.dev?sdkVersion=4.5.0&fileType=conf)
