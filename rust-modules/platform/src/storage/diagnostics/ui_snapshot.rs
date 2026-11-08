//! Fixed-leaf, developer-only export of an explicitly admitted UI snapshot.
//! Call on the existing off-frame request worker. The caller owns deadline,
//! sensitive-screen admission, cancellation, and retiring that worker before clear.
//! There is no listener, private settings read, or caller-supplied output path.
use std::io;

/// PNG produced by the trusted screenshot encoder. No Debug implementation exposes pixels.
pub struct Image<'a> {
    pub png: &'a [u8],
    pub width: u32,
    pub height: u32,
    pub source_ticks: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Failure {
    Timeout,
    VideoBound,
    SensitiveUi,
    UnsupportedUi,
    Background,
    InvalidViewport,
    ReadbackFailed,
    Cancelled,
    EncodingFailed,
    PayloadTooLarge,
}

/// Synchronous export. Successful return means publication, not UI/device validation.
pub fn publish(request_id: &str, result: Result<Image<'_>, Failure>) -> io::Result<()> {
    #[cfg(all(feature = "devtriggers", target_os = "linux", target_arch = "arm", not(feature = "hostsim")))]
    { implementation::publish(request_id, result) }
    #[cfg(not(all(feature = "devtriggers", target_os = "linux", target_arch = "arm", not(feature = "hostsim"))))]
    { let _ = (request_id, result); Err(io::ErrorKind::Unsupported.into()) }
}

/// Retire/join this request's worker before calling; this is not an intent ledger.
/// Do not call while holding diagnostics' publication lock. It erases only UI leaves.
pub fn clear() -> io::Result<()> {
    #[cfg(all(feature = "devtriggers", target_os = "linux", target_arch = "arm", not(feature = "hostsim")))]
    { implementation::clear() }
    #[cfg(not(all(feature = "devtriggers", target_os = "linux", target_arch = "arm", not(feature = "hostsim"))))]
    { Err(io::ErrorKind::Unsupported.into()) }
}

#[cfg(any(test, all(feature = "devtriggers", target_os = "linux", target_arch = "arm", not(feature = "hostsim"))))]
mod implementation {
    use super::{Failure, Image};
    use crate::storage::diagnostics::{
        developer_export, guard, publish_artifact_in, remove_artifact_in,
        Artifact, Identifier, DISABLED, PUBLICATION,
    };
    use std::{fs::File, io, sync::atomic::Ordering};

    #[derive(serde::Serialize)]
    #[serde(tag = "state", rename_all = "snake_case")]
    enum Status {
        Captured { sha256: String, width: u32, height: u32, source_ticks: u32 },
        Unavailable { reason: Failure },
    }
    #[derive(serde::Serialize)]
    struct Record<'a> { schema: u8, app_id: String, version: String, request_id: &'a str, pid: u32, status: Status }

    fn metadata(app_id: &str, version: &str, request_id: &str, result: &Result<Image<'_>, Failure>) -> io::Result<Vec<u8>> {
        if request_id.len() != 32
            || !request_id.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        { return Err(io::Error::from_raw_os_error(libc::EINVAL)); }
        let status = match result {
            Ok(image) => {
                if image.png.len() > Artifact::UiSnapshotPng.limit() {
                    return Err(io::Error::from_raw_os_error(libc::EFBIG));
                }
                // The encoder owns complete PNG validity. Check the bounded PNG header
                // and its dimensions here so exported correlation cannot contradict it.
                if image.width == 0 || image.height == 0 || image.width > 1920 || image.height > 1080
                    || image.png.len() < 33 || &image.png[..8] != b"\x89PNG\r\n\x1a\n"
                    || image.png[8..12] != 13u32.to_be_bytes() || &image.png[12..16] != b"IHDR"
                    || image.png[16..20] != image.width.to_be_bytes()
                    || image.png[20..24] != image.height.to_be_bytes()
                { return Err(io::Error::from_raw_os_error(libc::EINVAL)); }
                let digest = plx_base::sha256::sha256(image.png);
                let sha256 = digest.iter().map(|byte| format!("{byte:02x}")).collect();
                Status::Captured { sha256, width: image.width, height: image.height,
                    source_ticks: image.source_ticks }
            }
            Err(reason) => Status::Unavailable { reason: *reason },
        };
        let bytes = serde_json::to_vec(&Record { schema: 1,
            app_id: Identifier::new(app_id).0, version: Identifier::new(version).0, request_id,
            pid: std::process::id(), status }).map_err(io::Error::other)?;
        if bytes.len() > Artifact::UiSnapshotMetadata.limit() {
            return Err(io::Error::from_raw_os_error(libc::EFBIG));
        }
        Ok(bytes)
    }

    #[cfg(all(feature = "devtriggers", target_os = "linux", target_arch = "arm", not(feature = "hostsim")))]
    pub(super) fn publish(request_id: &str, result: Result<Image<'_>, Failure>) -> io::Result<()> {
        let _block = guard();
        let parent = developer_export::shared_parent()?;
        let version = crate::storage::diagnostics::VERSION.get().copied().unwrap_or("unknown");
        publish_at(&parent, plx_base::paths::app_id(), version, request_id, result, || Ok(()))
    }

    fn publish_at(parent: &File, app_id: &str, version: &str, request_id: &str,
        result: Result<Image<'_>, Failure>, before_metadata: impl FnOnce() -> io::Result<()>) -> io::Result<()> {
        let _block = guard();
        let bytes = metadata(app_id, version, request_id, &result)?;
        let _publication = PUBLICATION.lock().unwrap_or_else(|e| e.into_inner());
        if DISABLED.load(Ordering::Acquire) {
            return Err(io::Error::from_raw_os_error(libc::ECANCELED));
        }
        let dir = developer_export::owned_directory(parent, app_id, true)?;
        // Metadata is the commit record. Retire it durably before changing any pixels.
        remove_artifact_in(&dir, Artifact::UiSnapshotMetadata)?;
        dir.sync_all()?;
        let published = (|| {
            match result {
                Ok(image) => publish_artifact_in(&dir, Artifact::UiSnapshotPng, image.png, || Ok(()))?,
                Err(_) => { remove_artifact_in(&dir, Artifact::UiSnapshotPng)?; dir.sync_all()?; }
            }
            before_metadata()?;
            publish_artifact_in(&dir, Artifact::UiSnapshotMetadata, &bytes, || Ok(()))
        })();
        if published.is_err() {
            // This also retires a metadata rename whose subsequent sync failed. Never
            // report failure while intentionally keeping a committed old/new association.
            let _ = clear_in(&dir);
        }
        published
    }

    // Caller holds PUBLICATION. Attempt both leaves even if one is an unsafe alias.
    fn clear_in(dir: &File) -> io::Result<()> {
        let mut result = Ok(());
        for artifact in [Artifact::UiSnapshotMetadata, Artifact::UiSnapshotPng] {
            let removed = remove_artifact_in(dir, artifact);
            if result.is_ok() { result = removed; }
        }
        let synced = dir.sync_all();
        result.and(synced)
    }

    #[cfg(all(feature = "devtriggers", target_os = "linux", target_arch = "arm", not(feature = "hostsim")))]
    pub(super) fn clear() -> io::Result<()> {
        let _block = guard();
        match developer_export::shared_parent() {
            Ok(parent) => clear_at(&parent, plx_base::paths::app_id()),
            Err(error) if error.raw_os_error() == Some(libc::ENOENT) => Ok(()),
            Err(error) => Err(error),
        }
    }

    fn clear_at(parent: &File, app_id: &str) -> io::Result<()> {
        let _block = guard();
        let _publication = PUBLICATION.lock().unwrap_or_else(|e| e.into_inner());
        match developer_export::owned_directory(parent, app_id, false) {
            Ok(dir) => clear_in(&dir),
            Err(error) if error.raw_os_error() == Some(libc::ENOENT) => Ok(()),
            Err(error) => Err(error),
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use crate::storage::diagnostics::{directory, unique, NAME};
        use std::{os::unix::fs::{symlink, MetadataExt, PermissionsExt}, path::PathBuf};

        const REQUEST: &str = "0123456789abcdef0123456789abcdef";
        const NEXT: &str = "fedcba9876543210fedcba9876543210";
        const FIXTURE_VERSION: &str = "0.3.0";
        // Valid one-pixel RGB PNG; no private UI or filesystem input.
        const PNG: &[u8] = &[
            137,80,78,71,13,10,26,10,0,0,0,13,73,72,68,82,0,0,0,1,0,0,0,1,
            8,2,0,0,0,144,119,83,222,0,0,0,12,73,68,65,84,120,156,99,248,207,192,0,
            0,3,1,1,0,201,254,146,239,0,0,0,0,73,69,78,68,174,66,96,130,
        ];
        fn image() -> Image<'static> { Image { png: PNG, width: 1, height: 1, source_ticks: 123 } }
        struct Fixture { root: PathBuf, disabled: bool }
        impl Fixture {
            fn new() -> Self {
                let root = std::env::temp_dir().join(unique().to_str().unwrap());
                std::fs::create_dir(&root).unwrap();
                let disabled = DISABLED.swap(false, Ordering::AcqRel);
                Self { root, disabled }
            }
            fn parent(&self) -> File { directory(&self.root).unwrap() }
            fn slot(&self) -> PathBuf { self.root.join(format!("{}-storage-diagnostics", plx_base::paths::STABLE_APP_ID)) }
            fn leaf(&self, artifact: Artifact) -> PathBuf { self.slot().join(artifact.name().to_str().unwrap()) }
            fn publish(&self, id: &str, result: Result<Image<'_>, Failure>) -> io::Result<()> {
                publish_at(&self.parent(), plx_base::paths::STABLE_APP_ID, FIXTURE_VERSION, id, result, || Ok(()))
            }
            fn record(&self) -> serde_json::Value {
                serde_json::from_slice(&std::fs::read(self.leaf(Artifact::UiSnapshotMetadata)).unwrap()).unwrap()
            }
        }
        impl Drop for Fixture {
            fn drop(&mut self) {
                DISABLED.store(self.disabled, Ordering::Release);
                let _ = std::fs::remove_dir_all(&self.root);
            }
        }

        #[test]
        fn pair_correlates_exact_pixels_request_and_hash_without_overwriting_other_evidence() {
            let _serial = plx_base::testlock::serial();
            let f = Fixture::new();
            developer_export::owned_directory(&f.parent(), plx_base::paths::STABLE_APP_ID, true).unwrap();
            std::fs::write(f.slot().join(NAME), b"ordinary").unwrap();
            std::fs::write(f.leaf(Artifact::Canary), b"canary").unwrap();
            f.publish(REQUEST, Ok(image())).unwrap();
            assert_eq!(std::fs::read(f.leaf(Artifact::UiSnapshotPng)).unwrap(), PNG);
            assert_eq!(f.record(), serde_json::json!({"schema":1,
                "app_id":plx_base::paths::STABLE_APP_ID,"version":FIXTURE_VERSION,"request_id":REQUEST,
                "pid":std::process::id(),"status":{"state":"captured","width":1,"height":1,
                "source_ticks":123,"sha256":"b1ff9c8ea3a780bad09b346c423d2d0e46815926879b18e841d928376a946640"}}));
            assert_eq!(std::fs::read(f.slot().join(NAME)).unwrap(), b"ordinary");
            assert_eq!(std::fs::read(f.leaf(Artifact::Canary)).unwrap(), b"canary");
            for artifact in [Artifact::UiSnapshotPng, Artifact::UiSnapshotMetadata] {
                let meta = std::fs::metadata(f.leaf(artifact)).unwrap();
                assert_eq!(meta.mode() & 0o7777, 0o640);
                assert_eq!((meta.uid(), meta.gid(), meta.nlink()),
                    (unsafe { libc::geteuid() }, unsafe { libc::getegid() }, 1));
            }
        }

        #[test]
        fn every_closed_failure_retires_previous_pixels_and_has_no_image_fields() {
            let _serial = plx_base::testlock::serial();
            let f = Fixture::new();
            for reason in [Failure::Timeout, Failure::VideoBound, Failure::SensitiveUi,
                Failure::UnsupportedUi, Failure::Background, Failure::InvalidViewport,
                Failure::ReadbackFailed, Failure::Cancelled, Failure::EncodingFailed, Failure::PayloadTooLarge] {
                f.publish(REQUEST, Ok(image())).unwrap();
                f.publish(NEXT, Err(reason)).unwrap();
                assert!(!f.leaf(Artifact::UiSnapshotPng).exists());
                assert_eq!(f.record(), serde_json::json!({"schema":1,
                    "app_id":plx_base::paths::STABLE_APP_ID,"version":FIXTURE_VERSION,"request_id":NEXT,
                    "pid":std::process::id(),"status":{"state":"unavailable","reason":reason}}));
            }
        }

        #[test]
        fn rejects_noncanonical_request_ids_and_inconsistent_image_without_new_publication() {
            let _serial = plx_base::testlock::serial();
            let f = Fixture::new();
            f.publish(REQUEST, Ok(image())).unwrap();
            let previous = f.record();
            for bad in ["", "../other", "ABCDEF0123456789abcdef0123456789", "0123456789abcdef0123456789abcdef\n", "g123456789abcdef0123456789abcdef"] {
                assert_eq!(f.publish(bad, Ok(image())).unwrap_err().raw_os_error(), Some(libc::EINVAL));
                assert_eq!(f.record(), previous);
            }
            for (width, height) in [(0, 1), (1921, 1), (1, 1081), (2, 1)] {
                assert!(f.publish(NEXT, Ok(Image { width, height, ..image() })).is_err());
                assert_eq!(f.record(), previous);
            }
            assert!(f.publish(NEXT, Ok(Image { png: b"not PNG", ..image() })).is_err());
            assert_eq!(f.record(), previous);
        }

        #[test]
        fn png_cap_is_separate_from_small_metadata_cap_and_oversize_is_rejected() {
            let _serial = plx_base::testlock::serial();
            let f = Fixture::new();
            let dir = developer_export::owned_directory(&f.parent(), plx_base::paths::STABLE_APP_ID, true).unwrap();
            let _publication = PUBLICATION.lock().unwrap_or_else(|e| e.into_inner());
            let bytes = vec![0u8; 16 * 1024 + 1];
            publish_artifact_in(&dir, Artifact::UiSnapshotPng, &bytes, || Ok(())).unwrap();
            assert_eq!(std::fs::metadata(f.leaf(Artifact::UiSnapshotPng)).unwrap().len(), bytes.len() as u64);
            assert_eq!(publish_artifact_in(&dir, Artifact::UiSnapshotMetadata, &bytes, || Ok(()))
                .unwrap_err().raw_os_error(), Some(libc::EFBIG));
            let oversized = vec![0u8; 8 * 1024 * 1024 + 1];
            assert_eq!(metadata(plx_base::paths::STABLE_APP_ID, FIXTURE_VERSION, NEXT, &Ok(Image { png: &oversized, ..image() }))
                .unwrap_err().raw_os_error(), Some(libc::EFBIG));
            assert_eq!(publish_artifact_in(&dir, Artifact::UiSnapshotPng, &oversized, || Ok(()))
                .unwrap_err().raw_os_error(), Some(libc::EFBIG));
        }

        #[test]
        fn failed_second_publication_leaves_no_stale_metadata_or_uncommitted_pixels() {
            let _serial = plx_base::testlock::serial();
            let f = Fixture::new();
            f.publish(REQUEST, Ok(image())).unwrap();
            let result = publish_at(&f.parent(), plx_base::paths::STABLE_APP_ID, FIXTURE_VERSION, NEXT, Ok(image()), || {
                assert!(!f.leaf(Artifact::UiSnapshotMetadata).exists());
                assert_eq!(std::fs::read(f.leaf(Artifact::UiSnapshotPng)).unwrap(), PNG);
                Err(io::Error::from_raw_os_error(libc::EIO))
            });
            assert_eq!(result.unwrap_err().raw_os_error(), Some(libc::EIO));
            assert!(!f.leaf(Artifact::UiSnapshotMetadata).exists());
            assert!(!f.leaf(Artifact::UiSnapshotPng).exists());
        }

        #[test]
        fn symlink_and_hardlink_destinations_are_refused_without_touching_targets() {
            let _serial = plx_base::testlock::serial();
            let f = Fixture::new();
            developer_export::owned_directory(&f.parent(), plx_base::paths::STABLE_APP_ID, true).unwrap();
            let target = f.root.join("sentinel");
            std::fs::write(&target, b"untouched").unwrap();
            for artifact in [Artifact::UiSnapshotMetadata, Artifact::UiSnapshotPng] {
                for hard in [false, true] {
                    let leaf = f.leaf(artifact);
                    if hard { std::fs::hard_link(&target, &leaf).unwrap(); }
                    else { symlink(&target, &leaf).unwrap(); }
                    assert!(f.publish(REQUEST, Ok(image())).is_err());
                    assert!(clear_at(&f.parent(), plx_base::paths::STABLE_APP_ID).is_err());
                    assert_eq!(std::fs::read(&target).unwrap(), b"untouched");
                    assert!(std::fs::symlink_metadata(&leaf).is_ok());
                    std::fs::remove_file(leaf).unwrap();
                }
            }
        }

        #[test]
        fn public_directory_alias_or_widened_mode_is_never_accepted() {
            let _serial = plx_base::testlock::serial();
            let f = Fixture::new();
            let outside = f.root.join("outside");
            std::fs::create_dir(&outside).unwrap();
            symlink(&outside, f.slot()).unwrap();
            assert!(f.publish(REQUEST, Ok(image())).is_err());
            assert_eq!(std::fs::read_dir(&outside).unwrap().count(), 0);
            std::fs::remove_file(f.slot()).unwrap();
            developer_export::owned_directory(&f.parent(), plx_base::paths::STABLE_APP_ID, true).unwrap();
            std::fs::set_permissions(f.slot(), std::fs::Permissions::from_mode(0o777)).unwrap();
            assert!(f.publish(REQUEST, Ok(image())).is_err());
            assert_eq!(std::fs::metadata(f.slot()).unwrap().mode() & 0o7777, 0o777);
        }

        #[test]
        fn clear_and_disabled_erase_remove_only_fixed_leaves_without_republication() {
            let _serial = plx_base::testlock::serial();
            let f = Fixture::new();
            f.publish(REQUEST, Ok(image())).unwrap();
            std::fs::write(f.slot().join(NAME), b"ordinary").unwrap();
            std::fs::write(f.leaf(Artifact::Canary), b"canary").unwrap();
            std::fs::write(f.slot().join("unrelated"), b"keep").unwrap();
            clear_at(&f.parent(), plx_base::paths::STABLE_APP_ID).unwrap();
            assert!(!f.leaf(Artifact::UiSnapshotPng).exists());
            assert!(!f.leaf(Artifact::UiSnapshotMetadata).exists());
            assert_eq!(std::fs::read(f.slot().join(NAME)).unwrap(), b"ordinary");
            assert_eq!(std::fs::read(f.leaf(Artifact::Canary)).unwrap(), b"canary");
            f.publish(NEXT, Ok(image())).unwrap();
            DISABLED.store(true, Ordering::Release);
            {
                let _publication = PUBLICATION.lock().unwrap_or_else(|e| e.into_inner());
                developer_export::remove_at(&f.parent(), plx_base::paths::STABLE_APP_ID).unwrap();
            }
            for artifact in [Artifact::Snapshot, Artifact::Canary, Artifact::UiSnapshotPng, Artifact::UiSnapshotMetadata] {
                assert!(!f.leaf(artifact).exists());
            }
            assert_eq!(std::fs::read(f.slot().join("unrelated")).unwrap(), b"keep");
            assert_eq!(f.publish(REQUEST, Ok(image())).unwrap_err().raw_os_error(), Some(libc::ECANCELED));
            clear_at(&f.parent(), plx_base::paths::STABLE_APP_ID).unwrap();
        }

        #[test]
        fn identity_metadata_uses_existing_bounded_identifier_sanitizer() {
            for (app_id, version) in [("/private/app", "token=secret"), ("", ""), ("app", &"v".repeat(97))] {
                let bytes = metadata(app_id, version, REQUEST, &Err(Failure::Cancelled)).unwrap();
                let record: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
                assert_eq!(record["app_id"], if app_id == "app" { "app" } else { "invalid" });
                assert_eq!(record["version"], "invalid");
                assert!(!String::from_utf8(bytes).unwrap().contains("secret"));
            }
        }
    }
}
