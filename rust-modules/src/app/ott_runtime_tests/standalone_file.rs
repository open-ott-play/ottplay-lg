//! Runtime authority/catalogue dispatch using actual temporary file grants.
//! No catalogue worker, protected writer, decoder, or network service is started.
use super::*;
use crate::app::ott_local_root::LocalSource;
use crate::app::ott_local_session::{AuthorityRef, BoundSource, SourceKind};
use plx_base::task::OffFrame;
use std::path::PathBuf;

struct Files(PathBuf);
impl Files {
    fn new() -> Self {
        for n in 0..128 {
            let path = std::env::temp_dir().join(format!("ott-runtime-file-{}-{n}", std::process::id()));
            match std::fs::create_dir(&path) {
                Ok(()) => {
                    let path = std::fs::canonicalize(path).unwrap();
                    std::fs::write(path.join("movie.mp4"), b"synthetic fixture bytes").unwrap();
                    std::fs::write(path.join("sibling.mp4"), b"unselected fixture bytes").unwrap();
                    return Self(path);
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(e) => panic!("temporary directory: {e}"),
            }
        }
        panic!("temporary directory slots exhausted")
    }
    fn descriptor(&self, kind: SourceKind) -> BoundSource {
        let path = if kind == SourceKind::File { self.0.join("movie.mp4") } else { self.0.clone() };
        BoundSource { id: "selected-source".into(), name: "Selected media".into(), kind,
            available: true, authority_ref: AuthorityRef(11), generation: 7,
            canonical_path: Some(path.to_str().unwrap().into()) }
    }
    fn acquire(&self, kind: SourceKind) -> LocalSource {
        LocalSource::acquire_authorized(&OffFrame::for_test(), self.descriptor(kind), &mut || true)
            .expect("real canonical source grant")
    }
    fn profile(&self, kind: SourceKind) -> ProviderProfile {
        let source = self.descriptor(kind);
        ProviderProfile { id: "local-profile".into(), name: "Local".into(), kind: ProviderKind::Local,
            source: source.canonical_path.unwrap(), source_alias: source.name, enabled: true }
    }
}
impl Drop for Files { fn drop(&mut self) { plx_base::testscratch::remove_tree(&self.0); } }
fn runtime(source: LocalSource) -> Runtime {
    let mut runtime = runtime_fixture();
    runtime.loaded = true;
    runtime.generation = 7;
    runtime.resume_source_work();
    runtime.local_inventory.install(vec![source], 7);
    runtime
}

#[test]
fn inventory_refresh_retains_file_grant_then_replacement_revokes_the_old_reader() {
    let _serial = plx_base::testlock::serial();
    let files = Files::new();
    let original = files.acquire(SourceKind::File);
    let held = original.single_file("movie.mp4").unwrap();
    let mut runtime = runtime(original.clone());
    let selected = runtime.bind_local_selection(original.descriptor()).unwrap();
    let read = runtime.capture_local_source(&selected).unwrap();
    let observed = original.observe(&OffFrame::for_test(), &mut || true).unwrap();
    runtime.local_inventory.install(vec![observed], 8);
    assert!(held.is_live() && original.is_live());
    assert!(runtime.local_inventory.sources[0].same_grant(&original));
    assert!(runtime.local_source_read_is_current(&read));

    std::fs::rename(files.0.join("movie.mp4"), files.0.join("original.mp4")).unwrap();
    std::fs::write(files.0.join("movie.mp4"), b"replacement fixture").unwrap();
    let mut descriptor = files.descriptor(SourceKind::File);
    descriptor.generation = 9;
    let replacement = LocalSource::acquire_authorized(&OffFrame::for_test(), descriptor, &mut || true).unwrap();
    assert!(!replacement.same_grant(&original));
    runtime.local_inventory.install(vec![replacement.clone()], 9);
    assert!(!held.is_live() && !original.is_live());
    assert!(replacement.is_live());
    assert!(!runtime.local_source_read_is_current(&read));
    assert!(runtime.capture_local_source(&selected).is_none());
    let fresh = runtime.bind_local_selection(replacement.descriptor()).unwrap();
    assert!(runtime.capture_local_source(&fresh).is_some());
}

#[test]
fn file_selection_has_generic_capture_but_never_directory_or_sibling_authority() {
    let _serial = plx_base::testlock::serial();
    let files = Files::new(); let source = files.acquire(SourceKind::File);
    let runtime = runtime(source.clone());
    let selected = runtime.bind_local_selection(source.descriptor()).unwrap();
    assert!(runtime.current_local_source(&selected).is_some());
    let read = runtime.capture_local_source(&selected).unwrap();
    assert!(read.is_live() && read.source.same_grant(&source));
    assert!(runtime.local_source_read_is_current(&read));
    assert!(read.source.single_file("movie.mp4").is_some());
    assert!(read.source.single_file("sibling.mp4").is_none());
    assert!(read.source.single_file("../movie.mp4").is_none());
    assert!(read.source.root_handle().is_none());
    assert!(read.folder_read().is_none());
    assert!(runtime.current_local_root(&selected).is_none());
    assert!(runtime.capture_local_root(&selected).is_none());
}

#[test]
fn exact_file_profile_dispatches_the_held_source_to_the_one_row_catalogue_task() {
    let _serial = plx_base::testlock::serial();
    let files = Files::new(); let source = files.acquire(SourceKind::File);
    let runtime = runtime(source.clone()); let expected = files.profile(SourceKind::File);
    let task = runtime.local_catalog_task(expected.clone()).unwrap();
    match task {
        ott_catalog::Task::LocalFile { profile, source: granted } => {
            assert!(profile == expected);
            assert!(granted.same_grant(&source) && granted.is_live());
            assert_eq!(granted.single_file_name(), Some("movie.mp4"));
            assert!(granted.root_handle().is_none());
        }
        _ => panic!("exact-file source must not fall back to folder/path-only catalogue IO"),
    }
}

#[test]
fn unavailable_or_ambiguous_file_source_never_falls_back_to_provider_io() {
    let _serial = plx_base::testlock::serial();
    let files = Files::new(); let source = files.acquire(SourceKind::File);
    let mut runtime = runtime(source.clone()); let profile = files.profile(SourceKind::File);
    runtime.local_inventory.install(vec![LocalSource::unavailable(source.descriptor().clone())], 8);
    assert!(!source.is_live());
    assert!(runtime.local_catalog_task(profile.clone()).is_err());

    let first = files.acquire(SourceKind::File);
    let mut other = files.descriptor(SourceKind::File);
    other.id = "other-source".into(); other.authority_ref = AuthorityRef(12);
    let second = LocalSource::acquire_authorized(&OffFrame::for_test(), other, &mut || true).unwrap();
    runtime.local_inventory.install(vec![first, second], 9);
    assert!(runtime.local_catalog_task(profile).is_err(), "duplicate path grants cannot choose an arbitrary source");
}

#[test]
fn folder_source_keeps_directory_capture_in_its_catalogue_task() {
    let _serial = plx_base::testlock::serial();
    let files = Files::new(); let source = files.acquire(SourceKind::Folder);
    let runtime = runtime(source.clone());
    let selected = runtime.bind_local_selection(source.descriptor()).unwrap();
    let generic = runtime.capture_local_source(&selected).unwrap();
    let root = runtime.capture_local_root(&selected).unwrap();
    assert!(runtime.current_local_root(&selected).is_some());
    assert!(runtime.local_root_read_is_current(&root));
    let via_generic = generic.folder_read().unwrap();
    assert!(Arc::ptr_eq(&root.root, &via_generic.root));
    assert!(generic.source.single_file("movie.mp4").is_none());
    let profile = files.profile(SourceKind::Folder);
    assert!(matches!(runtime.local_catalog_task(profile.clone()),
        Ok(ott_catalog::Task::LocalFolder { profile: actual, source: held })
            if actual == profile && held.same_grant(&source) && held.is_live()));
}

#[test]
fn dropping_the_runtime_inventory_revokes_retained_file_captures() {
    let _serial = plx_base::testlock::serial();
    let files = Files::new(); let source = files.acquire(SourceKind::File);
    let runtime = runtime(source.clone());
    let selected = runtime.bind_local_selection(source.descriptor()).unwrap();
    let read = runtime.capture_local_source(&selected).unwrap();
    let held = read.source.single_file("movie.mp4").unwrap();
    assert!(read.is_live() && held.is_live());
    drop(runtime);
    assert!(!source.is_live() && !read.is_live() && !held.is_live());
    assert!(read.source.single_file("movie.mp4").is_none());
}

#[path = "standalone_file_admission.rs"]
mod admission;
