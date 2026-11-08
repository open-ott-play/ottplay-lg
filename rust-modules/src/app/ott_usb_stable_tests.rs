use super::*;
use crate::app::ott_local_session::{AuthorityRef, SourceKind};
use crate::app::ott_usb::{execute_probed, execute_with_identity, Answer, Choice, Task};
use plx_platform::tv::removable::{self, identity::StableIdentity};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT: AtomicU64 = AtomicU64::new(0);
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "ott-usb-stable-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(fs::canonicalize(path).unwrap())
    }
    fn mount(&self, id: u64) -> MountCandidate {
        MountCandidate::directory_for_test(self.0.to_str().unwrap(), id).unwrap()
    }
    fn source(&self) -> BoundSource {
        BoundSource {
            id: "saved-usb".into(),
            name: "Videos".into(),
            kind: SourceKind::Folder,
            available: false,
            authority_ref: AuthorityRef(3),
            generation: 19,
            canonical_path: Some("/tmp/usb/old/old1/Films".into()),
        }
    }
    fn select(&self, relative: &str) -> Task {
        Task::Select {
            choice: Choice {
                mount: self.mount(7),
                readable: true,
            },
            source: self.source(),
            relative_folder: relative.into(),
        }
    }
    fn binding(&self, relative: &str) -> UsbBinding {
        UsbBinding::from_observed("ext4", "uuid-1", "/", relative).unwrap()
    }
    fn restore(&self, relative: &str) -> Task {
        Task::Restore {
            binding: self.binding(relative),
            source: self.source(),
        }
    }
    fn execute(&self, task: Task, identify: &mut IdentityProbe<'_>) -> Result<Answer, Error> {
        execute_probed(
            &OffFrame::for_test(),
            task,
            &AtomicBool::new(false),
            &mut |_, _| Ok(vec![self.mount(7)]),
            identify,
            &mut |_, source, _, _| {
                assert!(
                    source.root_handle().is_some(),
                    "real held directory is required"
                );
                Ok(())
            },
        )
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn rows(mounts: &[MountCandidate], uuid: &str) -> Vec<IdentityObservation> {
    mounts
        .iter()
        .map(|m| {
            IdentityObservation::for_test(
                m.clone(),
                StableIdentity::new(uuid, m.filesystem()).unwrap(),
            )
        })
        .collect()
}
fn good(
    _: &OffFrame,
    mounts: &[MountCandidate],
    _: &AtomicBool,
) -> Result<Vec<IdentityObservation>, IdentityError> {
    Ok(rows(mounts, "uuid-1"))
}

#[test]
fn explicit_selection_records_identity_only_after_matching_second_observation() {
    let f = Fixture::new();
    fs::create_dir(f.0.join("Films")).unwrap();
    let mut calls = 0;
    let Answer::Selected(source) = f
        .execute(f.select("Films"), &mut |_, mounts, _| {
            calls += 1;
            Ok(rows(mounts, "uuid-1"))
        })
        .unwrap()
    else {
        panic!("explicit selection");
    };
    assert_eq!(calls, 2);
    assert_eq!(
        source.usb_witness().unwrap().stable_binding(),
        Some(&f.binding("Films"))
    );
    assert_eq!(source.descriptor().id, "saved-usb");
    assert_eq!(source.descriptor().generation, 19);
}

#[test]
fn unavailable_initial_backend_keeps_manual_selection_without_stable_claim() {
    let f = Fixture::new();
    for error in [IdentityError::Unsupported, IdentityError::Unavailable] {
        let mut calls = 0;
        let Answer::Selected(source) = f
            .execute(f.select(""), &mut |_, _, _| {
                calls += 1;
                Err(error)
            })
            .unwrap()
        else {
            panic!("manual selection");
        };
        assert_eq!(calls, 1);
        assert!(source.root_handle().is_some());
        assert!(source.usb_witness().unwrap().stable_binding().is_none());
    }
}

#[test]
fn malformed_or_ambiguous_initial_metadata_never_downgrades_to_manual() {
    let f = Fixture::new();
    for error in [
        IdentityError::Invalid,
        IdentityError::Ambiguous,
        IdentityError::Changed,
    ] {
        assert!(f.execute(f.select(""), &mut |_, _, _| Err(error)).is_err());
    }
    assert!(matches!(
        f.execute(f.select(""), &mut |_, _, _| Ok(vec![])),
        Err(Error::Changed)
    ));
}

#[test]
fn established_identity_cannot_change_or_disappear_after_root_open() {
    let f = Fixture::new();
    for mode in 0..3 {
        let mut calls = 0;
        let result = f.execute(f.select(""), &mut |_, mounts, _| {
            calls += 1;
            if calls == 1 {
                return Ok(rows(mounts, "uuid-1"));
            }
            match mode {
                0 => Ok(rows(mounts, "uuid-2")),
                1 => Err(IdentityError::Unavailable),
                _ => Ok(vec![]),
            }
        });
        assert!(result.is_err());
        assert_eq!(calls, 2);
    }
}

#[test]
fn restore_relocates_exact_folder_and_preserves_source_identity() {
    let f = Fixture::new();
    fs::create_dir(f.0.join("Films")).unwrap();
    let Answer::Restored { source, relocation } = f.execute(f.restore("Films"), &mut good).unwrap()
    else {
        panic!("restored");
    };
    assert_eq!(
        relocation.old_root().canonical_path.as_deref(),
        Some("/tmp/usb/old/old1/Films")
    );
    assert_eq!(
        relocation.new_root().canonical_path.as_deref(),
        f.0.join("Films").to_str()
    );
    assert_eq!(relocation.stable_binding(), &f.binding("Films"));
    assert_eq!(source.descriptor().id, "saved-usb");
    assert_eq!(source.descriptor().name, "Videos");
    assert_eq!(source.descriptor().authority_ref.0, 3);
    assert_eq!(source.descriptor().generation, 19);
    assert!(relocation.is_current(&source));
    assert!(source
        .usb_witness()
        .unwrap()
        .is_observed(&[f.mount(7)], source.descriptor()));
}

#[test]
fn restore_never_uses_root_or_similar_basename_for_missing_folder() {
    let f = Fixture::new();
    fs::create_dir_all(f.0.join("Other/Films")).unwrap();
    assert!(matches!(
        f.execute(f.restore("Films"), &mut good),
        Err(Error::MissingFolder)
    ));
}

#[test]
fn restore_with_unknown_identity_never_becomes_an_explicit_path_grant() {
    let f = Fixture::new();
    assert!(matches!(
        f.execute(f.restore(""), &mut |_, _, _| Err(
            IdentityError::Unsupported
        )),
        Err(Error::Unavailable)
    ));
    assert!(matches!(
        f.execute(f.restore(""), &mut |_, mounts, _| Ok(rows(mounts, "other"))),
        Err(Error::Unavailable)
    ));
}

#[test]
fn receipt_requires_exact_new_root_handle_and_rejects_withdrawal() {
    let f = Fixture::new();
    let Answer::Restored {
        mut source,
        relocation,
    } = f.execute(f.restore(""), &mut good).unwrap()
    else {
        panic!("restored");
    };
    let other = LocalSource::acquire_authorized(
        &OffFrame::for_test(),
        source.descriptor().clone(),
        &mut || true,
    )
    .unwrap();
    assert!(
        !relocation.is_current(&other),
        "same path/descriptor is not the admitted FD"
    );
    let clone = source.clone();
    drop(clone);
    assert!(relocation.is_current(&source));
    source.withdraw();
    assert!(!relocation.is_current(&source));
}

fn mount(path: &str, root: &str, id: u64, major: u32, minor: u32) -> MountCandidate {
    let text = format!("{id} 1 {major}:{minor} {root} {path} rw - ext4 /dev/fixture rw\n");
    removable::candidates(text.as_bytes())
        .unwrap()
        .pop()
        .unwrap()
}

#[test]
fn duplicate_filesystem_uuid_on_distinct_devices_is_rejected() {
    let first = mount("/tmp/usb/a/a1", "/", 2, 8, 1);
    let second = mount("/tmp/usb/b/b1", "/", 3, 8, 2);
    let mounts = vec![first, second];
    assert_eq!(
        validate_snapshot(&mounts, &rows(&mounts, "same-uuid")),
        Err(Error::Changed)
    );
}

#[test]
fn bind_views_restore_filesystem_coordinates_and_refuse_prefix_lookalikes() {
    let binding = UsbBinding::from_observed("ext4", "uuid-1", "/Archive", "Films").unwrap();
    let good_mount = mount("/tmp/usb/a/a1", "/Archive", 2, 8, 1);
    let bad_mount = mount("/tmp/usb/b/b1", "/Archive/Films2", 3, 8, 1);
    let mounts = vec![good_mount.clone(), bad_mount];
    let evidence = rows(&mounts, "uuid-1");
    assert!(validate_snapshot(&mounts, &evidence).is_ok());
    assert_eq!(locate(&evidence, &binding).unwrap(), &good_mount);
    assert_eq!(
        binding
            .relative_folder_for_mount_root(good_mount.root())
            .unwrap(),
        "Films"
    );
    assert!(matches!(
        locate(&evidence[1..], &binding),
        Err(Error::MissingFolder)
    ));
}

#[test]
fn equivalent_same_device_bind_views_are_deterministic_not_clone_ambiguity() {
    let a = mount("/tmp/usb/a/a1", "/", 2, 8, 1);
    let z = mount("/tmp/usb/z/z1", "/Films", 3, 8, 1);
    let binding = UsbBinding::from_observed("ext4", "uuid-1", "/", "Films").unwrap();
    let mounts = vec![z, a.clone()];
    let evidence = rows(&mounts, "uuid-1");
    assert!(validate_snapshot(&mounts, &evidence).is_ok());
    assert_eq!(locate(&evidence, &binding).unwrap(), &a);
}

#[test]
fn identity_rows_must_match_whole_mount_snapshot_exactly() {
    let a = mount("/tmp/usb/a/a1", "/", 2, 8, 1);
    let changed = mount("/tmp/usb/a/a1", "/Other", 2, 8, 1);
    assert_eq!(
        validate_snapshot(&[a.clone()], &rows(&[changed], "uuid-1")),
        Err(Error::Changed)
    );
    let duplicate = rows(&[a.clone(), a.clone()], "uuid-1");
    assert_eq!(
        validate_snapshot(&[a.clone(), a], &duplicate),
        Err(Error::Changed)
    );
}

#[test]
fn restoration_rejects_mount_change_and_late_cancellation() {
    let f = Fixture::new();
    let mut probes = 0;
    let result = execute_probed(
        &OffFrame::for_test(),
        f.restore(""),
        &AtomicBool::new(false),
        &mut |_, _| {
            probes += 1;
            Ok(vec![f.mount(if probes == 1 { 7 } else { 8 })])
        },
        &mut good,
        &mut |_, _, _, _| Ok(()),
    );
    assert!(matches!(result, Err(Error::Changed)));
    let cancel = AtomicBool::new(false);
    let mut calls = 0;
    let result = execute_probed(
        &OffFrame::for_test(),
        f.restore(""),
        &cancel,
        &mut |_, _| Ok(vec![f.mount(7)]),
        &mut |_, mounts, cancel| {
            calls += 1;
            if calls == 2 {
                cancel.store(true, Ordering::Release);
            }
            Ok(rows(mounts, "uuid-1"))
        },
        &mut |_, _, _, _| Ok(()),
    );
    assert_eq!(calls, 2);
    assert!(matches!(result, Err(Error::Cancelled)));
}

#[test]
fn replacement_during_final_identity_read_cannot_publish_old_fd() {
    let f = Fixture::new();
    fs::create_dir(f.0.join("Films")).unwrap();
    let mut calls = 0;
    let result = f.execute(f.restore("Films"), &mut |_, mounts, _| {
        calls += 1;
        if calls == 2 {
            fs::rename(f.0.join("Films"), f.0.join("Old")).unwrap();
            fs::create_dir(f.0.join("Films")).unwrap();
        }
        Ok(rows(mounts, "uuid-1"))
    });
    assert!(matches!(result, Err(Error::Changed)));
}

#[test]
fn production_device_check_rejects_an_observation_for_another_filesystem() {
    let f = Fixture::new();
    let mut descriptor = f.source();
    descriptor.canonical_path = f.0.to_str().map(str::to_owned);
    let off = OffFrame::for_test();
    let source = LocalSource::acquire_authorized(&off, descriptor, &mut || true).unwrap();
    let file = fs::File::open(&f.0).unwrap();
    let actual = plx_platform::tv::removable::identity::directory_device(&off, &file).unwrap();
    let wrong = mount("/tmp/usb/a/a1", "/", 2, actual.0.wrapping_add(1), actual.1);
    assert_eq!(
        check_root_device(&off, &source, &wrong, &AtomicBool::new(false)),
        Err(Error::Changed)
    );
    assert_eq!(
        source.descriptor().canonical_path.as_deref().map(Path::new),
        Some(f.0.as_path())
    );
}

#[test]
fn discovery_does_not_probe_identity_or_mint_automatic_restore_proof() {
    let f = Fixture::new();
    let answer = f
        .execute(Task::Discover { known: vec![] }, &mut |_, _, _| {
            panic!("discovery should not read identity until a source operation")
        })
        .unwrap();
    assert!(matches!(answer, Answer::Choices { .. }));
}

#[test]
fn public_fixture_seam_returns_only_a_real_device_matched_receipt() {
    let f = Fixture::new();
    let off = OffFrame::for_test();
    let directory = fs::File::open(&f.0).unwrap();
    let device = plx_platform::tv::removable::identity::directory_device(&off, &directory).unwrap();
    let mount =
        MountCandidate::directory_device_for_test(f.0.to_str().unwrap(), 7, device).unwrap();
    let Answer::Restored { source, relocation } = execute_with_identity(
        &off,
        f.restore(""),
        &AtomicBool::new(false),
        &mut |_, _| Ok(vec![mount.clone()]),
        &mut good,
    )
    .unwrap() else {
        panic!("restored with real FD correlation");
    };
    assert!(relocation.is_current(&source));
    assert_eq!(
        relocation.new_root().canonical_path.as_deref(),
        f.0.to_str()
    );
}
