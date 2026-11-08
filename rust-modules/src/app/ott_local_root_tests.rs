use super::*;
use crate::app::ott_local_session::AuthorityRef;
use std::fs;
use std::os::unix::fs::{symlink, PermissionsExt};
use std::path::PathBuf;
use std::sync::atomic::AtomicU64;

static NEXT: AtomicU64 = AtomicU64::new(0);
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "local-root-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(fs::canonicalize(path).unwrap())
    }
    fn descriptor(&self) -> BoundSource {
        BoundSource {
            id: "root-one".into(),
            name: "Videos".into(),
            kind: SourceKind::Folder,
            available: true,
            authority_ref: AuthorityRef(1),
            generation: 7,
            canonical_path: Some(self.0.to_str().unwrap().into()),
        }
    }
    fn acquire(&self) -> LocalSource {
        LocalSource::acquire_authorized(&OffFrame::for_test(), self.descriptor(), &mut || true)
            .unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn acquisition_preserves_permissions_and_exposes_same_directory_fd() {
    let fixture = Fixture::new();
    fs::set_permissions(&fixture.0, fs::Permissions::from_mode(0o755)).unwrap();
    let before = fs::metadata(&fixture.0).unwrap();
    let source = fixture.acquire();
    let opened = source
        .root_handle()
        .unwrap()
        .file_if_live(&OffFrame::for_test())
        .unwrap()
        .metadata()
        .unwrap();
    assert!(same_directory(&before, &opened));
    assert_eq!(before.permissions().mode(), opened.permissions().mode());
    assert_eq!(source.descriptor().generation, 7);
}

#[test]
fn unchanged_observation_retains_the_exact_shared_handle() {
    let fixture = Fixture::new();
    let source = fixture.acquire();
    let observed = source.observe(&OffFrame::for_test(), &mut || true).unwrap();
    assert!(Arc::ptr_eq(
        source.root_handle().unwrap(),
        observed.root_handle().unwrap()
    ));
}

#[test]
fn changed_path_is_a_loss_observation_without_worker_revocation() {
    let fixture = Fixture::new();
    let source = fixture.acquire();
    let handle = source.root_handle().unwrap().clone();
    let moved = fixture.0.with_extension("old");
    fs::rename(&fixture.0, &moved).unwrap();
    fs::create_dir(&fixture.0).unwrap();
    let observed = source.observe(&OffFrame::for_test(), &mut || true).unwrap();
    assert!(!observed.descriptor().available);
    assert!(observed.root_handle().is_none());
    assert!(handle.is_live());
    assert!(same_directory(
        &fs::metadata(&moved).unwrap(),
        &handle.file.metadata().unwrap()
    ));
    fs::remove_dir(moved).unwrap();
}

#[test]
fn missing_directory_does_not_silently_reconnect_when_it_reappears() {
    let fixture = Fixture::new();
    let source = fixture.acquire();
    fs::remove_dir(&fixture.0).unwrap();
    let lost = source.observe(&OffFrame::for_test(), &mut || true).unwrap();
    fs::create_dir(&fixture.0).unwrap();
    let still_lost = lost.observe(&OffFrame::for_test(), &mut || true).unwrap();
    assert!(!still_lost.descriptor().available);
    assert!(still_lost.root_handle().is_none());
    assert!(source.root_handle().unwrap().is_live());
}

#[test]
fn runtime_withdrawal_is_seen_by_every_cloned_holder() {
    let fixture = Fixture::new();
    let mut source = fixture.acquire();
    let candidate = source.clone();
    let handle = source.root_handle().unwrap().clone();
    source.withdraw();
    source.withdraw();
    assert!(!source.descriptor().available);
    assert!(source.root_handle().is_none());
    assert!(!handle.is_live());
    assert!(candidate.root_handle().is_none());
    assert!(handle.file_if_live(&OffFrame::for_test()).is_none());
}

#[test]
fn dropping_candidate_never_withdraws_accepted_or_other_root() {
    let fixture = Fixture::new();
    let other_fixture = Fixture::new();
    let mut source = fixture.acquire();
    let other = other_fixture.acquire();
    drop(source.clone());
    assert!(source.root_handle().unwrap().is_live());
    source.withdraw();
    assert!(other.root_handle().unwrap().is_live());
}

#[test]
fn explicit_reacquisition_has_a_fresh_handle_and_does_not_revive_old_one() {
    let fixture = Fixture::new();
    let mut source = fixture.acquire();
    let old = source.root_handle().unwrap().clone();
    source.withdraw();
    let mut descriptor = fixture.descriptor();
    descriptor.generation = 8;
    let replacement =
        LocalSource::acquire_authorized(&OffFrame::for_test(), descriptor, &mut || true).unwrap();
    assert!(!Arc::ptr_eq(&old, replacement.root_handle().unwrap()));
    assert!(!old.is_live());
    assert!(replacement.root_handle().unwrap().is_live());
    assert!(replacement.descriptor().authority_ref == source.descriptor().authority_ref);
    assert_eq!(replacement.descriptor().generation, 8);
}

#[test]
fn cancelled_observation_is_not_a_loss_report() {
    let fixture = Fixture::new();
    let source = fixture.acquire();
    let mut checks = 0;
    let result = source.observe(&OffFrame::for_test(), &mut || {
        checks += 1;
        checks < 2
    });
    assert!(matches!(result, Err(RootError::Cancelled)));
    assert!(source.root_handle().unwrap().is_live());
}

#[test]
fn cancellation_during_failed_path_observation_takes_priority() {
    let fixture = Fixture::new();
    let source = fixture.acquire();
    fs::remove_dir(&fixture.0).unwrap();
    let mut checks = 0;
    let result = source.observe(&OffFrame::for_test(), &mut || {
        checks += 1;
        checks < 3
    });
    assert!(matches!(result, Err(RootError::Cancelled)));
    assert!(source.root_handle().unwrap().is_live());
}

#[test]
fn directory_replacement_between_open_and_final_observation_is_rejected() {
    let fixture = Fixture::new();
    let moved = fixture.0.with_extension("old");
    let mut checks = 0;
    let result =
        LocalSource::acquire_authorized(&OffFrame::for_test(), fixture.descriptor(), &mut || {
            checks += 1;
            if checks == 3 {
                fs::rename(&fixture.0, &moved).unwrap();
                fs::create_dir(&fixture.0).unwrap();
            }
            true
        });
    assert!(matches!(result, Err(RootError::Unavailable)));
    fs::remove_dir(moved).unwrap();
}

#[test]
fn root_alias_regular_file_and_noncanonical_input_are_refused() {
    let fixture = Fixture::new();
    let alias = fixture.0.join("alias");
    symlink(&fixture.0, &alias).unwrap();
    let file = fixture.0.join("file");
    fs::write(&file, b"not a directory").unwrap();
    for path in [
        alias.to_str().unwrap().to_owned(),
        file.to_str().unwrap().to_owned(),
        format!(
            "{}/../{}",
            fixture.0.display(),
            fixture.0.file_name().unwrap().to_str().unwrap()
        ),
        "relative".into(),
    ] {
        let mut descriptor = fixture.descriptor();
        descriptor.canonical_path = Some(path);
        assert!(matches!(
            LocalSource::acquire_authorized(&OffFrame::for_test(), descriptor, &mut || true),
            Err(RootError::Unavailable)
        ));
    }
}

#[test]
fn caller_cancellation_is_checked_before_any_acquisition() {
    let fixture = Fixture::new();
    assert!(matches!(
        LocalSource::acquire_authorized(&OffFrame::for_test(), fixture.descriptor(), &mut || false),
        Err(RootError::Cancelled)
    ));
}
