use super::*;
use std::sync::atomic::AtomicU64;

static NEXT: AtomicU64 = AtomicU64::new(0);
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("ott-held-video-{}-{}",
            std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
        std::fs::create_dir(&root).unwrap();
        Self(root)
    }
    fn source(&self, latch: Arc<AtomicBool>) -> HeldLocalFile {
        let path = self.0.join("video.bin");
        std::fs::write(&path, b"original-video").unwrap();
        HeldLocalFile::from_authorized(&OffFrame::for_test(), File::open(&path).unwrap(), path, latch).unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); }
}
fn bytes(mut source: FileSource) -> Vec<u8> {
    let mut bytes = Vec::new();
    source.read_to_end(&mut bytes).unwrap();
    bytes
}

#[test]
fn held_video_reopen_and_seek_do_not_follow_replaced_path() {
    let fixture = Fixture::new();
    let held = fixture.source(Arc::new(AtomicBool::new(false)));
    let mut inspection = FileSource::held(&held).unwrap();
    let mut prefix = [0; 4];
    inspection.read_exact(&mut prefix).unwrap();
    assert_eq!(&prefix, b"orig");
    std::fs::rename(held.path(), fixture.0.join("old.bin")).unwrap();
    std::fs::write(held.path(), b"replacement").unwrap();
    // The old path-only transport demonstrates the regression's negative control.
    assert_eq!(bytes(FileSource::open(held.path()).unwrap()), b"replacement");
    assert_eq!(bytes(FileSource::held(&held).unwrap()), b"original-video");
    inspection.seek(SeekFrom::Start(9)).unwrap();
    let mut suffix = Vec::new();
    inspection.read_to_end(&mut suffix).unwrap();
    assert_eq!(suffix, b"video");
}

#[test]
fn held_readers_have_independent_cursors_on_the_same_file() {
    let fixture = Fixture::new();
    let held = fixture.source(Arc::new(AtomicBool::new(false)));
    let mut probe = FileSource::held(&held).unwrap();
    let mut playback = FileSource::held(&held).unwrap();
    probe.seek(SeekFrom::End(-5)).unwrap();
    let mut start = [0; 4];
    playback.read_exact(&mut start).unwrap();
    assert_eq!(&start, b"orig");
    assert_eq!(probe.stream_position().unwrap(), 9);
    assert_eq!(playback.stream_position().unwrap(), 4);
    assert!(probe.seek(SeekFrom::Current(-10)).is_err());
}

#[test]
fn withdrawal_rejects_existing_and_reopened_readers_without_reviving_on_replacement() {
    let fixture = Fixture::new();
    let withdrawn = Arc::new(AtomicBool::new(false));
    let held = fixture.source(withdrawn.clone());
    let mut reader = FileSource::held(&held).unwrap();
    withdrawn.store(true, Ordering::Release);
    assert!(!held.is_live());
    assert!(FileSource::held(&held).is_err());
    assert!(reader.read(&mut [0; 1]).is_err());
    assert!(reader.seek(SeekFrom::Start(0)).is_err());
    std::fs::remove_file(held.path()).unwrap();
    let replacement = fixture.source(Arc::new(AtomicBool::new(false)));
    assert!(replacement.is_live());
    assert_ne!(held, replacement);
    assert!(FileSource::held(&held).is_err());
    assert_eq!(held, held.clone());
}

#[test]
fn held_video_requires_regular_file_and_does_not_expose_path_in_debug() {
    let fixture = Fixture::new();
    let latch = Arc::new(AtomicBool::new(false));
    assert!(HeldLocalFile::from_authorized(&OffFrame::for_test(),
        File::open(&fixture.0).unwrap(), fixture.0.clone(), latch.clone()).is_err());
    let held = fixture.source(latch);
    assert_eq!(format!("{held:?}"), "HeldLocalFile(<private>)");
}
