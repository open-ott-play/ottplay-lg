use super::*;
use crate::app::ott_local_session::AuthorityRef;
use plx_platform::tv::removable::identity::{IdentityObservation, StableIdentity};
use std::fs::File;
use std::path::PathBuf;
use std::sync::atomic::AtomicU64;

fn rejected(error: ConfirmFolderError) -> Error {
    match error {
        ConfirmFolderError::Rejected(error) => error,
        ConfirmFolderError::SourceLost(_) => panic!("generic failure must not report source loss"),
    }
}

struct Disk {
    base: PathBuf,
    path: PathBuf,
}
impl Disk {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let base = std::env::temp_dir().join(format!(
            "usb-identity-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&base).unwrap();
        // macOS temporary roots can be reached through /var -> /private/var.
        // A fixture mount must use the same canonical identity as the opened
        // directory; otherwise the production remount guard correctly rejects it.
        let base = std::fs::canonicalize(base).unwrap();
        let path = base.join("mount");
        std::fs::create_dir(&path).unwrap();
        Self { base, path }
    }
    fn mount(&self) -> MountCandidate {
        let device =
            identity::directory_device(&OffFrame::for_test(), &File::open(&self.path).unwrap())
                .unwrap();
        MountCandidate::directory_device_for_test(self.path.to_str().unwrap(), 8, device).unwrap()
    }
    fn identity(&self, uuid: &str) -> Vec<IdentityObservation> {
        let mount = self.mount();
        let identity = StableIdentity::new(uuid, mount.filesystem()).unwrap();
        vec![IdentityObservation::for_test(mount, identity)]
    }
    fn select(&self, id: &str, uuid: &str, authority: u64, generation: u64) -> LocalSource {
        let mount = self.mount();
        let answer = execute_probed(
            &OffFrame::for_test(),
            Task::Select {
                choice: Choice {
                    mount,
                    readable: true,
                },
                source: BoundSource {
                    id: id.into(),
                    name: id.into(),
                    kind: SourceKind::Folder,
                    available: false,
                    authority_ref: AuthorityRef(authority),
                    generation,
                    canonical_path: None,
                },
                relative_folder: String::new(),
            },
            &AtomicBool::new(false),
            &mut |_, _| Ok(vec![self.mount()]),
            &mut |_, _, _| Ok(self.identity(uuid)),
            &mut stable::check_root_device,
        )
        .unwrap();
        let Answer::Selected(source) = answer else {
            panic!("selected source")
        };
        source
    }
    fn reuse(
        &self,
        source: LocalSource,
        saved: RememberedFolders,
        known: &[LocalSource],
        uuid: &str,
    ) -> Result<ConfirmedFolder, ConfirmFolderError> {
        reuse_with(
            &OffFrame::for_test(),
            source,
            saved,
            known,
            &AtomicBool::new(false),
            &mut |_, _| Ok(vec![self.mount()]),
            &mut |_, _, _| Ok(self.identity(uuid)),
            &mut stable::check_root_device,
        )
    }
}
impl Drop for Disk {
    fn drop(&mut self) {
        plx_base::testscratch::remove_tree(&self.base);
    }
}
fn row(source: &LocalSource) -> FolderDescriptor {
    FolderDescriptor {
        id: source.descriptor().id.clone(),
        name: source.descriptor().name.clone(),
        kind: LocalSourceKind::Folder,
        path: source.descriptor().canonical_path.clone().unwrap(),
        usb: source
            .usb_witness()
            .and_then(Witness::stable_binding)
            .cloned(),
    }
}
fn remembered(source: &LocalSource) -> RememberedFolders {
    RememberedFolders::default()
        .with_connected(row(source))
        .unwrap()
}

#[test]
fn new_folder_remains_ephemeral_until_explicit_connect_candidate() {
    let disk = Disk::new();
    let selected = disk.select("new", "volume-a", 3, 9);
    let empty = RememberedFolders::default();
    let result = disk
        .reuse(selected.clone(), empty.clone(), &[], "volume-a")
        .unwrap();
    assert!(result.source().same_grant(&selected));
    assert!(empty.folders().is_empty());
    assert!(result.remembered_is_current(&empty));
    let candidate = result.connect_candidate(&empty).unwrap();
    assert_eq!(candidate.folders(), &[row(&selected)]);
    assert!(
        empty.folders().is_empty(),
        "only the returned candidate changes"
    );
}

#[test]
fn repeated_selection_preserves_live_logical_source_and_its_grant_stamp() {
    let disk = Disk::new();
    let old = disk.select("remembered", "volume-a", 1, 4);
    let saved = remembered(&old);
    let selected = disk.select("new-handle", "volume-a", 3, 9);
    let result = disk
        .reuse(
            selected.clone(),
            saved.clone(),
            &[old.clone(), selected],
            "volume-a",
        )
        .unwrap();
    assert_eq!(result.source().descriptor().id, "remembered");
    assert_eq!(result.source().descriptor().name, "remembered");
    assert_eq!(result.source().descriptor().generation, 4);
    assert!(result.source().descriptor().authority_ref == AuthorityRef(1));
    assert!(result.source().same_grant(&old));
    assert_eq!(result.connect_candidate(&saved).unwrap(), saved);
    assert!(old.is_live());
}

#[test]
fn restart_without_old_grant_reuses_logical_identity_with_fresh_authority() {
    let disk = Disk::new();
    let old = disk.select("remembered", "volume-a", 1, 4);
    let saved = remembered(&old);
    let selected = disk.select("new-handle", "volume-a", 3, 9);
    let result = disk
        .reuse(selected, saved.clone(), &[], "volume-a")
        .unwrap();
    assert_eq!(result.source().descriptor().id, "remembered");
    assert_eq!(result.source().descriptor().generation, 9);
    assert!(result.source().descriptor().authority_ref == AuthorityRef(3));
    assert!(!result.source().same_grant(&old));
    assert!(result
        .source()
        .usb_witness()
        .unwrap()
        .is_observed(&[disk.mount()], result.source().descriptor()));
    assert_eq!(result.connect_candidate(&saved).unwrap(), saved);
}

#[test]
fn withdrawn_old_root_is_not_resurrected_by_reselection() {
    let disk = Disk::new();
    let mut old = disk.select("remembered", "volume-a", 1, 4);
    let root = old.root_handle().unwrap().clone();
    let saved = remembered(&old);
    old.withdraw();
    let selected = disk.select("new-handle", "volume-a", 3, 9);
    let result = disk
        .reuse(selected, saved, &[old.clone()], "volume-a")
        .unwrap();
    assert!(result.source().is_live());
    assert_eq!(result.source().descriptor().generation, 9);
    assert!(!result.source().same_grant(&old));
    assert!(!root.is_live());
}

#[test]
fn changed_remembered_snapshot_refuses_connect_candidate() {
    let disk = Disk::new();
    let old = disk.select("remembered", "volume-a", 1, 4);
    let saved = remembered(&old);
    let result = disk
        .reuse(
            disk.select("new", "volume-a", 3, 9),
            saved.clone(),
            &[],
            "volume-a",
        )
        .unwrap();
    let mut changed = row(&old);
    changed.name = "Renamed by later request".into();
    let changed = saved.with_reselected(changed).unwrap();
    assert!(!result.remembered_is_current(&changed));
    assert_eq!(
        result.connect_candidate(&changed).err(),
        Some(Error::Changed)
    );
    assert_eq!(result.connect_candidate(&saved).unwrap(), saved);
}

#[test]
fn same_path_different_volume_and_legacy_path_only_rows_cannot_inherit_identity() {
    let disk = Disk::new();
    let old = disk.select("remembered", "volume-a", 1, 4);
    let saved = remembered(&old);
    assert_eq!(
        disk.reuse(disk.select("new", "volume-b", 3, 9), saved, &[], "volume-b")
            .err()
            .map(rejected),
        Some(Error::Changed)
    );
    let mut legacy = row(&old);
    legacy.usb = None;
    let legacy = RememberedFolders::default().with_connected(legacy).unwrap();
    assert_eq!(
        disk.reuse(
            disk.select("new", "volume-a", 3, 9),
            legacy,
            &[],
            "volume-a"
        )
        .err()
        .map(rejected),
        Some(Error::Changed)
    );
    assert!(old.is_live());
}

#[test]
fn relocated_binding_with_absent_or_dead_grant_only_prepares_explicit_connect() {
    let old_disk = Disk::new();
    let new_disk = Disk::new();
    let mut old = old_disk.select("remembered", "volume-a", 1, 4);
    let saved = remembered(&old);
    old.withdraw();
    for known in [Vec::new(), vec![old.clone()]] {
        let selected = new_disk.select("new", "volume-a", 3, 9);
        let result = new_disk
            .reuse(selected, saved.clone(), &known, "volume-a")
            .unwrap();
        assert_eq!(result.source().descriptor().id, "remembered");
        assert_eq!(result.source().descriptor().name, "remembered");
        assert_eq!(result.source().descriptor().generation, 9);
        assert_eq!(saved.folders()[0].path, old_disk.path.to_str().unwrap());
        let candidate = result.connect_candidate(&saved).unwrap();
        assert_eq!(candidate.folders().len(), 1);
        assert_eq!(candidate.folders()[0].path, new_disk.path.to_str().unwrap());
        assert_eq!(candidate.folders()[0].usb, saved.folders()[0].usb);
    }
}

#[test]
fn relocated_binding_cannot_displace_a_live_old_grant() {
    let old_disk = Disk::new();
    let new_disk = Disk::new();
    let old = old_disk.select("remembered", "volume-a", 1, 4);
    let saved = remembered(&old);
    assert_eq!(
        new_disk
            .reuse(
                new_disk.select("new", "volume-a", 3, 9),
                saved,
                &[old.clone()],
                "volume-a"
            )
            .err()
            .map(rejected),
        Some(Error::Changed)
    );
    assert!(old.is_live());
}

#[test]
fn replaced_directory_reports_exact_loss_without_worker_revocation() {
    let disk = Disk::new();
    let old = disk.select("remembered", "volume-a", 1, 4);
    let saved = remembered(&old);
    std::fs::rename(&disk.path, disk.base.join("old-root")).unwrap();
    std::fs::create_dir(&disk.path).unwrap();
    let current = disk.select("new", "volume-a", 3, 9);
    let results = [
        disk.reuse(current, saved.clone(), &[old.clone()], "volume-a"),
        disk.reuse(old.clone(), saved, &[], "volume-a"),
    ];
    for result in results {
        let Err(ConfirmFolderError::SourceLost(loss)) = result else {
            panic!("exact held-path loss")
        };
        assert!(loss.same_grant(&old));
        assert_eq!(loss.descriptor().id, old.descriptor().id);
        assert_eq!(loss.descriptor().generation, old.descriptor().generation);
    }
    assert!(
        old.is_live(),
        "only the runtime may withdraw the shared latch"
    );
    let mut runtime_owned = old.clone();
    runtime_owned.withdraw();
    std::fs::remove_dir(&disk.path).unwrap();
    std::fs::rename(disk.base.join("old-root"), &disk.path).unwrap();
    assert!(
        !old.is_live(),
        "restoring the pathname cannot revive an applied loss"
    );
}

#[test]
fn ambiguous_live_logical_sources_do_not_choose_one() {
    let disk = Disk::new();
    let old = disk.select("remembered", "volume-a", 1, 4);
    let other = disk.select("another", "volume-a", 2, 5);
    assert_eq!(
        disk.reuse(
            disk.select("new", "volume-a", 3, 9),
            remembered(&old),
            &[old.clone(), other.clone()],
            "volume-a"
        )
        .err()
        .map(rejected),
        Some(Error::Changed)
    );
    assert!(old.is_live() && other.is_live());
}

#[test]
fn ambiguous_same_uuid_on_two_devices_fails_complete_identity_validation() {
    let disk = Disk::new();
    let other = Disk::new();
    let old = disk.select("remembered", "volume-a", 1, 4);
    let selected = disk.select("new", "volume-a", 3, 9);
    let mount = disk.mount();
    let device = mount.device();
    let other_mount = MountCandidate::directory_device_for_test(
        other.path.to_str().unwrap(),
        9,
        (device.0, device.1.wrapping_add(1)),
    )
    .unwrap();
    let rows = vec![mount.clone(), other_mount.clone()];
    let identities = rows
        .iter()
        .map(|m| {
            IdentityObservation::for_test(
                m.clone(),
                StableIdentity::new("volume-a", m.filesystem()).unwrap(),
            )
        })
        .collect::<Vec<_>>();
    let result = reuse_with(
        &OffFrame::for_test(),
        selected,
        remembered(&old),
        &[],
        &AtomicBool::new(false),
        &mut |_, _| Ok(rows.clone()),
        &mut |_, _, _| Ok(identities.clone()),
        &mut stable::check_root_device,
    );
    assert_eq!(result.err().map(rejected), Some(Error::Changed));
    assert!(old.is_live());
}

#[test]
fn failed_or_cancelled_verification_never_returns_a_reusable_source() {
    let disk = Disk::new();
    let old = disk.select("remembered", "volume-a", 1, 4);
    let selected = disk.select("new", "volume-a", 3, 9);
    let saved = remembered(&old);
    let cancel = AtomicBool::new(false);
    let failed = reuse_with(
        &OffFrame::for_test(),
        selected.clone(),
        saved.clone(),
        &[],
        &cancel,
        &mut |_, _| Err(Error::Unavailable),
        &mut |_, _, _| panic!("no identity after failed mount snapshot"),
        &mut stable::check_root_device,
    );
    assert_eq!(failed.err().map(rejected), Some(Error::Unavailable));
    let cancelled = reuse_with(
        &OffFrame::for_test(),
        selected,
        saved,
        &[],
        &cancel,
        &mut |_, _| Ok(vec![disk.mount()]),
        &mut |_, _, _| {
            cancel.store(true, Ordering::Release);
            Ok(disk.identity("volume-a"))
        },
        &mut stable::check_root_device,
    );
    assert_eq!(cancelled.err().map(rejected), Some(Error::Cancelled));
    assert!(old.is_live());
}

#[test]
fn revoked_result_cannot_prepare_a_connect_write() {
    let disk = Disk::new();
    let old = disk.select("remembered", "volume-a", 1, 4);
    let saved = remembered(&old);
    let result = disk
        .reuse(
            disk.select("new", "volume-a", 3, 9),
            saved.clone(),
            &[old.clone()],
            "volume-a",
        )
        .unwrap();
    let mut revoked = old;
    revoked.withdraw();
    assert_eq!(result.connect_candidate(&saved).err(), Some(Error::Changed));
}

#[test]
fn repeat_connect_accepts_only_its_exact_after_image() {
    let disk = Disk::new();
    let before = RememberedFolders::default();
    let result = disk
        .reuse(
            disk.select("new", "volume-a", 3, 9),
            before.clone(),
            &[],
            "volume-a",
        )
        .unwrap();
    let after = result.connect_candidate(&before).unwrap();
    assert!(result.remembered_is_current(&after));
    assert_eq!(result.connect_candidate(&after).unwrap(), after);
    let mut unrelated = after.folders()[0].clone();
    unrelated.id = "unrelated".into();
    unrelated.name = "Unrelated".into();
    unrelated.path.push_str("/other");
    unrelated.usb = None;
    let changed = after.with_connected(unrelated).unwrap();
    assert!(!result.remembered_is_current(&changed));
    assert_eq!(
        result.connect_candidate(&changed).err(),
        Some(Error::Changed)
    );
}
