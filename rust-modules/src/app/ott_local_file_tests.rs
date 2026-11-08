use super::super::{BoundSource, LocalSource, SourceKind};
use super::*;
use crate::app::ott_local_session::AuthorityRef;
use std::fs;
use std::os::unix::fs::{symlink, PermissionsExt};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        for suffix in 0..128 {
            let root = std::env::temp_dir()
                .join(format!("local-single-file-{}-{suffix}", std::process::id()));
            match fs::create_dir(&root) {
                Ok(()) => {
                    let root = fs::canonicalize(root).unwrap();
                    fs::create_dir(root.join("media")).unwrap();
                    fs::write(root.join("media/movie.mp4"), b"original media").unwrap();
                    fs::write(root.join("media/other.mp4"), b"other media").unwrap();
                    return Self(root);
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(error) => panic!("temporary source: {error}"),
            }
        }
        panic!("temporary source slots exhausted")
    }
    fn path(&self) -> PathBuf {
        self.0.join("media/movie.mp4")
    }
    fn descriptor(&self) -> BoundSource {
        BoundSource {
            id: "single-video".into(),
            name: "movie.mp4".into(),
            kind: SourceKind::File,
            available: true,
            authority_ref: AuthorityRef(4),
            generation: 9,
            canonical_path: Some(self.path().to_str().unwrap().into()),
        }
    }
    fn acquire(&self) -> LocalSource {
        LocalSource::acquire_authorized(&OffFrame::for_test(), self.descriptor(), &mut || true)
            .unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        plx_base::testscratch::remove_tree(&self.0);
    }
}

#[test]
fn exact_file_is_live_without_directory_or_sibling_authority() {
    let fixture = Fixture::new();
    let source = fixture.acquire();
    assert!(source.is_live());
    assert!(source.root_handle().is_none());
    assert_eq!(source.single_file_name(), Some("movie.mp4"));
    let held = source.single_file("movie.mp4").unwrap();
    assert!(held.is_live());
    assert_eq!(held, source.single_file("movie.mp4").unwrap());
    for denied in [
        "other.mp4",
        "../movie.mp4",
        "./movie.mp4",
        "media/movie.mp4",
        ".ottplay/metadata.json",
        "movie.srt",
        "",
    ] {
        assert!(source.single_file(denied).is_none(), "{denied}");
    }
    assert!(source
        .single_file(fixture.path().to_str().unwrap())
        .is_none());
    assert!(!format!("{held:?}").contains(fixture.0.to_str().unwrap()));
    assert_eq!(
        source.descriptor().canonical_path,
        fixture.descriptor().canonical_path
    );
    assert_eq!(source.descriptor().generation, 9);
}

#[test]
fn file_acquisition_never_changes_source_permissions() {
    let fixture = Fixture::new();
    fs::set_permissions(fixture.path(), fs::Permissions::from_mode(0o644)).unwrap();
    let before = fs::metadata(fixture.path()).unwrap().permissions().mode();
    let source = fixture.acquire();
    assert!(source.is_live());
    assert_eq!(
        fs::metadata(fixture.path()).unwrap().permissions().mode(),
        before
    );
}

#[test]
fn unchanged_observation_and_dropped_clones_keep_same_grant() {
    let fixture = Fixture::new();
    let source = fixture.acquire();
    let held = source.single_file("movie.mp4").unwrap();
    let observed = source.observe(&OffFrame::for_test(), &mut || true).unwrap();
    assert!(source.same_grant(&observed));
    assert_eq!(held, observed.single_file("movie.mp4").unwrap());
    drop(observed);
    drop(source.clone());
    assert!(source.is_live() && held.is_live());
}

#[test]
fn file_replacement_reports_loss_but_only_runtime_revokes_old_holder() {
    let fixture = Fixture::new();
    let mut source = fixture.acquire();
    let held = source.single_file("movie.mp4").unwrap();
    fs::rename(fixture.path(), fixture.0.join("media/original.mp4")).unwrap();
    fs::write(fixture.path(), b"replacement").unwrap();
    let observed = source.observe(&OffFrame::for_test(), &mut || true).unwrap();
    assert!(!observed.is_live() && !observed.descriptor().available);
    assert!(!source.same_grant(&observed));
    assert!(source.is_live() && held.is_live());
    source.withdraw();
    assert!(!held.is_live());
}

#[test]
fn replacing_parent_is_loss_even_when_leaf_inode_is_the_same() {
    let fixture = Fixture::new();
    let source = fixture.acquire();
    fs::rename(fixture.0.join("media"), fixture.0.join("old-media")).unwrap();
    fs::create_dir(fixture.0.join("media")).unwrap();
    fs::hard_link(fixture.0.join("old-media/movie.mp4"), fixture.path()).unwrap();
    let original = fs::metadata(fixture.0.join("old-media/movie.mp4")).unwrap();
    assert!(same_file(&original, &fs::metadata(fixture.path()).unwrap()));
    let observed = source.observe(&OffFrame::for_test(), &mut || true).unwrap();
    assert!(!observed.is_live());
    assert!(source.is_live(), "a worker does not revoke a shared grant");
}

#[test]
fn explicit_reacquisition_never_revives_old_withdrawn_file() {
    let fixture = Fixture::new();
    let mut source = fixture.acquire();
    let copy = source.clone();
    let old = source.single_file("movie.mp4").unwrap();
    source.withdraw();
    source.withdraw();
    assert!(!source.is_live() && !copy.is_live() && !old.is_live());
    assert!(copy.single_file("movie.mp4").is_none());
    let replacement = fixture.acquire();
    assert!(replacement.is_live());
    assert!(!copy.same_grant(&replacement));
    assert_ne!(old, replacement.single_file("movie.mp4").unwrap());
}

#[test]
fn missing_file_observation_cannot_silently_reacquire_on_reappearance() {
    let fixture = Fixture::new();
    let source = fixture.acquire();
    fs::remove_file(fixture.path()).unwrap();
    let unavailable = source.observe(&OffFrame::for_test(), &mut || true).unwrap();
    fs::write(fixture.path(), b"returned").unwrap();
    let repeated = unavailable
        .observe(&OffFrame::for_test(), &mut || true)
        .unwrap();
    assert!(!repeated.is_live());
    assert!(source.is_live());
}

#[test]
fn symlink_directory_fifo_and_noncanonical_file_inputs_are_rejected() {
    let fixture = Fixture::new();
    symlink(fixture.path(), fixture.0.join("media/link.mp4")).unwrap();
    symlink(fixture.0.join("media"), fixture.0.join("alias")).unwrap();
    let fifo = fixture.0.join("media/pipe.mp4");
    let name = std::ffi::CString::new(fifo.to_str().unwrap()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
    for path in [
        fixture.0.join("media/link.mp4"),
        fixture.0.join("alias/movie.mp4"),
        fixture.0.join("media"),
        fifo,
        fixture.0.join("media/./movie.mp4"),
    ] {
        let mut descriptor = fixture.descriptor();
        descriptor.canonical_path = Some(path.to_str().unwrap().into());
        assert!(matches!(
            LocalSource::acquire_authorized(&OffFrame::for_test(), descriptor, &mut || true),
            Err(RootError::Unavailable)
        ));
    }
}

#[test]
fn replacing_opened_leaf_during_acquisition_is_rejected() {
    let fixture = Fixture::new();
    let mut checks = 0;
    let result =
        LocalSource::acquire_authorized(&OffFrame::for_test(), fixture.descriptor(), &mut || {
            checks += 1;
            // Root check, acquire check, two parent checks, then four shared
            // readonly checks: at eight the leaf FD is open but not admitted.
            if checks == 8 {
                fs::rename(fixture.path(), fixture.0.join("media/original.mp4")).unwrap();
                fs::write(fixture.path(), b"replacement").unwrap();
            }
            true
        });
    assert!(checks >= 8);
    assert!(matches!(result, Err(RootError::Unavailable)));
}

#[test]
fn replacing_opened_parent_during_acquisition_is_rejected() {
    let fixture = Fixture::new();
    let mut checks = 0;
    let result =
        LocalSource::acquire_authorized(&OffFrame::for_test(), fixture.descriptor(), &mut || {
            checks += 1;
            if checks == 4 {
                fs::rename(fixture.0.join("media"), fixture.0.join("old-media")).unwrap();
                fs::create_dir(fixture.0.join("media")).unwrap();
                fs::hard_link(fixture.0.join("old-media/movie.mp4"), fixture.path()).unwrap();
            }
            true
        });
    assert!(checks >= 4);
    assert!(matches!(result, Err(RootError::Unavailable)));
}

#[test]
fn cancellation_at_every_acquisition_checkpoint_never_returns_a_grant() {
    let fixture = Fixture::new();
    let mut total = 0;
    let source =
        LocalSource::acquire_authorized(&OffFrame::for_test(), fixture.descriptor(), &mut || {
            total += 1;
            true
        })
        .unwrap();
    assert!(source.is_live() && total > 5);
    for stop in 1..=total {
        let mut calls = 0;
        assert!(
            matches!(
                LocalSource::acquire_authorized(
                    &OffFrame::for_test(),
                    fixture.descriptor(),
                    &mut || {
                        calls += 1;
                        calls < stop
                    }
                ),
                Err(RootError::Cancelled)
            ),
            "checkpoint {stop}"
        );
    }
}

#[test]
fn cancelled_missing_file_observation_is_not_an_authoritative_loss() {
    let fixture = Fixture::new();
    let source = fixture.acquire();
    fs::remove_file(fixture.path()).unwrap();
    let mut calls = 0;
    let result = source.observe(&OffFrame::for_test(), &mut || {
        calls += 1;
        calls < 3
    });
    assert!(matches!(result, Err(RootError::Cancelled)));
    assert!(source.is_live());
}

#[test]
fn folder_grant_retains_its_directory_api_and_is_not_a_single_file() {
    let fixture = Fixture::new();
    let mut descriptor = fixture.descriptor();
    descriptor.kind = SourceKind::Folder;
    descriptor.canonical_path = Some(fixture.0.join("media").to_str().unwrap().into());
    let source =
        LocalSource::acquire_authorized(&OffFrame::for_test(), descriptor, &mut || true).unwrap();
    assert!(source.is_live() && source.root_handle().is_some());
    assert!(source.single_file_name().is_none());
    assert!(source.single_file("movie.mp4").is_none());
    assert!(source.same_grant(&source.clone()));
    assert!(!source.same_grant(&fixture.acquire()));
}
