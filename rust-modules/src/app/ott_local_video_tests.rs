use super::*;
use std::path::PathBuf;

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        for suffix in 0..64 {
            let path = std::env::temp_dir().join(format!("ott-video-root-{}-{suffix}", std::process::id()));
            match std::fs::create_dir(&path) {
                Ok(()) => return Self(std::fs::canonicalize(path).unwrap()),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => panic!("temporary root: {error}"),
            }
        }
        panic!("temporary root slots exhausted")
    }
    fn binding(&self) -> (LocalSource, LocalSourceRead, ProviderProfile) {
        let descriptor = BoundSource { id: "video-root".into(), name: "Videos".into(),
            kind: local::SourceKind::Folder, available: true, authority_ref: local::AuthorityRef(1),
            generation: 7, canonical_path: Some(self.0.to_str().unwrap().into()) };
        let source = LocalSource::acquire_authorized(&OffFrame::for_test(), descriptor.clone(), &mut || true).unwrap();
        let profiles = plx_data::ott::ProviderProfiles::default();
        let read = capture_source_read(std::slice::from_ref(&source), &profiles, 7, &descriptor).unwrap();
        (source, read, source_profile(&descriptor).unwrap())
    }
}
impl Drop for Fixture {
    fn drop(&mut self) { plx_base::testscratch::remove_tree(&self.0); }
}
fn entry(relative: &str) -> CatalogEntry {
    CatalogEntry { protection: None, synopsis: None, live: None, id: "local-video".into(), title: "Video".into(), category: String::new(),
        description: String::new(), poster: String::new(), duration_ms: None,
        media: MediaLocator::Local { relative_path: relative.into() }, provider_id: None, provider_id_field: None }
}

#[test]
fn actual_local_worker_holds_the_authorized_file_and_uses_the_root_withdrawal() {
    let _serial = plx_base::testlock::serial();
    let fixture = Fixture::new();
    std::fs::write(fixture.0.join("video.mp4"), b"original").unwrap();
    let (mut source, read, profile) = fixture.binding();
    let opened = open_video_source(&OffFrame::for_test(), Some(&read), &profile,
        &entry("video.mp4"), &AtomicBool::new(false)).unwrap();
    let plx_media::ff::ExternalSource::HeldLocal(held) = opened else { panic!("must retain file"); };
    assert!(held.is_live());
    std::fs::rename(fixture.0.join("video.mp4"), fixture.0.join("old.mp4")).unwrap();
    std::fs::write(fixture.0.join("video.mp4"), b"replacement").unwrap();
    assert!(held.is_live(), "renaming is not a new source authority");
    source.withdraw();
    assert!(!held.is_live(), "the existing root owner revokes the media capability");
    assert!(open_video_source(&OffFrame::for_test(), Some(&read), &profile,
        &entry("video.mp4"), &AtomicBool::new(false)).is_err());
}

#[test]
fn actual_local_worker_refuses_symlink_components_and_cancelled_requests() {
    let _serial = plx_base::testlock::serial();
    let fixture = Fixture::new();
    let outside = Fixture::new();
    std::fs::write(outside.0.join("video.mp4"), b"outside").unwrap();
    std::os::unix::fs::symlink(outside.0.join("video.mp4"), fixture.0.join("leaf.mp4")).unwrap();
    std::os::unix::fs::symlink(&outside.0, fixture.0.join("parent")).unwrap();
    std::fs::write(fixture.0.join("inside.mp4"), b"inside").unwrap();
    let (_source, read, profile) = fixture.binding();
    for relative in ["leaf.mp4", "parent/video.mp4", "../video.mp4"] {
        assert!(open_video_source(&OffFrame::for_test(), Some(&read), &profile,
            &entry(relative), &AtomicBool::new(false)).is_err());
    }
    assert!(open_video_source(&OffFrame::for_test(), Some(&read), &profile,
        &entry("inside.mp4"), &AtomicBool::new(true)).is_err());
    assert!(open_video_source(&OffFrame::for_test(), None, &profile,
        &entry("inside.mp4"), &AtomicBool::new(false)).is_err());
}

#[test]
fn server_library_keeps_http_authority_without_a_local_root() {
    let _serial = plx_base::testlock::serial();
    let runtime = Runtime::unloaded(crate::app::ott_companion::Supervisor::inert_for_test());
    let mut profile = ProviderProfile { id: "server-library".into(), name: "Library".into(),
        kind: ProviderKind::Local, source: "https://library.invalid/media/".into(),
        source_alias: "Library".into(), enabled: true };
    let request = PlaybackRequest::ordinary("movie", 0);
    assert!(matches!(capture_video_root(&runtime, &request, &profile), Ok(None)));
    profile.source = "/unregistered/device-folder".into();
    assert!(capture_video_root(&runtime, &request, &profile).is_err());
}

#[test]
fn selected_single_file_reuses_the_admitted_object_and_never_opens_siblings() {
    let _serial = plx_base::testlock::serial();
    let fixture = Fixture::new();
    let path = fixture.0.join("video.mp4");
    std::fs::write(&path, b"original").unwrap();
    std::fs::write(fixture.0.join("neighbor.mp4"), b"neighbor").unwrap();
    let descriptor = BoundSource { id: "single-video".into(), name: "video.mp4".into(),
        kind: local::SourceKind::File, available: false, authority_ref: local::AuthorityRef(1),
        generation: 7, canonical_path: Some(path.to_str().unwrap().into()) };
    let mut source = LocalSource::acquire_authorized(&OffFrame::for_test(), descriptor, &mut || true).unwrap();
    let profile = source_profile(source.descriptor()).unwrap();
    let read = capture_source_read(std::slice::from_ref(&source), &plx_data::ott::ProviderProfiles::default(),
        7, source.descriptor()).unwrap();
    assert!(read.folder_read().is_none());
    let admitted = source.single_file("video.mp4").unwrap();
    std::fs::rename(&path, fixture.0.join("original.mp4")).unwrap();
    std::fs::write(&path, b"replacement").unwrap();
    let opened = open_video_source(&OffFrame::for_test(), Some(&read), &profile,
        &entry("video.mp4"), &AtomicBool::new(false)).unwrap();
    assert!(matches!(opened, plx_media::ff::ExternalSource::HeldLocal(ref held) if held == &admitted));
    for relative in ["neighbor.mp4", "original.mp4", "../video.mp4", "video.mp4/child.mp4"] {
        assert!(open_video_source(&OffFrame::for_test(), Some(&read), &profile,
            &entry(relative), &AtomicBool::new(false)).is_err());
    }
    assert!(open_video_source(&OffFrame::for_test(), Some(&read), &profile,
        &entry("video.mp4"), &AtomicBool::new(true)).is_err());
    source.withdraw();
    assert!(!admitted.is_live());
    assert!(open_video_source(&OffFrame::for_test(), Some(&read), &profile,
        &entry("video.mp4"), &AtomicBool::new(false)).is_err());
}

#[test]
fn single_file_read_cannot_be_used_with_another_profile_path() {
    let _serial = plx_base::testlock::serial();
    let fixture = Fixture::new();
    let path = fixture.0.join("video.mp4");
    std::fs::write(&path, b"original").unwrap();
    let descriptor = BoundSource { id: "single-video".into(), name: "video.mp4".into(),
        kind: local::SourceKind::File, available: false, authority_ref: local::AuthorityRef(1),
        generation: 7, canonical_path: Some(path.to_str().unwrap().into()) };
    let source = LocalSource::acquire_authorized(&OffFrame::for_test(), descriptor, &mut || true).unwrap();
    let mut profile = source_profile(source.descriptor()).unwrap();
    let read = capture_source_read(std::slice::from_ref(&source), &plx_data::ott::ProviderProfiles::default(),
        7, source.descriptor()).unwrap();
    profile.source = fixture.0.join("neighbor.mp4").to_str().unwrap().into();
    assert!(open_video_source(&OffFrame::for_test(), Some(&read), &profile,
        &entry("video.mp4"), &AtomicBool::new(false)).is_err());
}
