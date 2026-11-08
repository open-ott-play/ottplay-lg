//! The production path policy and byte-read boundary, without activating a
//! process-global fixture or bypassing profile parsing and Plex verification.
use super::*;
use std::cell::Cell;
use std::sync::atomic::{AtomicUsize, Ordering};

#[test]
fn active_fixture_never_falls_back_to_an_ordinary_profile() {
    for declared in [
        Ok(Some(PathBuf::from("/synthetic/profile.json"))),
        Ok(None),
        Err("fixture declaration changed"),
    ] {
        let result = profile_path_with(|| Some(declared.clone()), || {
            panic!("active fixture consulted an ambient profile")
        });
        assert_eq!(result, declared);
    }
}

#[test]
fn inactive_fixture_keeps_the_ordinary_profile_selection() {
    let calls = Cell::new(0);
    let path = PathBuf::from("/ordinary/profile.json");
    let result = profile_path_with(|| None, || {
        calls.set(calls.get() + 1);
        path.clone()
    });
    assert_eq!(result, Ok(Some(path)));
    assert_eq!(calls.get(), 1);
}

static NEXT: AtomicUsize = AtomicUsize::new(0);
struct Directory(PathBuf);
impl Directory {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("ott-import-boundary-{}-{}",
            std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
}
impl Drop for Directory {
    fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); }
}

#[test]
fn every_read_validates_its_actual_bytes_before_returning_them() {
    let directory = Directory::new();
    let path = directory.0.join("profile.json");
    let original = br#"{"synthetic":"first"}"#;
    std::fs::write(&path, original).unwrap();
    let calls = Cell::new(0);
    let validate = |observed_path: &std::path::Path, bytes: &[u8]| {
        calls.set(calls.get() + 1);
        assert_eq!(observed_path, path.as_path());
        if bytes == original { Ok(()) } else { Err("fixture bytes changed") }
    };
    assert_eq!(read_configuration_with(&path, validate), Ok(Some(original.to_vec())));
    std::fs::write(&path, br#"{"synthetic":"replacement"}"#).unwrap();
    assert_eq!(read_configuration_with(&path, validate), Err("fixture bytes changed"));
    assert_eq!(calls.get(), 2);
}

#[test]
fn a_removed_profile_remains_absent_and_cannot_be_revalidated() {
    let directory = Directory::new();
    let path = directory.0.join("profile.json");
    std::fs::write(&path, b"synthetic").unwrap();
    std::fs::remove_file(&path).unwrap();
    assert_eq!(read_configuration_with(&path, |_, _| {
        panic!("missing profile supplied bytes to validation")
    }), Ok(None));
}
