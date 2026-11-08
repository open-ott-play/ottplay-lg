//! Filesystem tests for the descriptor primitive. Written only; execution is
//! reserved for the coordinated native host-test slot.
use super::*;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::symlink;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

#[path = "readonly_directory_tests.rs"]
mod listing_tests;

static NEXT: AtomicU64 = AtomicU64::new(0);
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "readonly-assets-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn root(&self) -> File {
        File::open(&self.0).unwrap()
    }
    fn write(&self, relative: &str, bytes: &[u8]) {
        let path = self.0.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, bytes).unwrap();
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn nested_original_is_read_without_copy_or_permission_changes() {
    let f = Fixture::new();
    f.write("season/film.en.srt", b"subtitle text");
    let path = f.0.join("season/film.en.srt");
    let before = fs::metadata(&path).unwrap().permissions();
    assert_eq!(
        read_bounded(&f.root(), "season/film.en.srt", 13, &mut || true).unwrap(),
        b"subtitle text"
    );
    assert_eq!(fs::read(&path).unwrap(), b"subtitle text");
    assert_eq!(fs::metadata(path).unwrap().permissions(), before);
}

#[test]
fn invalid_paths_are_rejected_without_calling_the_io_checkpoint() {
    let f = Fixture::new();
    let root = f.root();
    let too_deep = vec!["a"; MAX_DIRECTORY_DEPTH + 2].join("/");
    let too_long = "a".repeat(MAX_PATH_BYTES + 1);
    for path in [
        "",
        "/absolute",
        "../escape",
        "a/../b",
        "./file",
        "a/./b",
        "a//b",
        "a/",
        "C:movie.srt",
        "a\\b",
        "nul\0file",
        "line\nfile",
        too_deep.as_str(),
        too_long.as_str(),
    ] {
        assert_eq!(
            read_bounded(&root, path, 32, &mut || panic!("invalid path reached IO")),
            Err(ReadError::InvalidPath)
        );
    }
}

#[test]
fn a_regular_file_cannot_be_used_as_root() {
    let f = Fixture::new();
    f.write("not-root", b"x");
    let root = File::open(f.0.join("not-root")).unwrap();
    assert_eq!(
        read_bounded(&root, "child", 8, &mut || true),
        Err(ReadError::InvalidRoot)
    );
}

#[test]
fn final_directory_and_symlinks_are_not_read() {
    let f = Fixture::new();
    let outside = Fixture::new();
    outside.write("other.srt", b"outside");
    f.write("folder/inside.srt", b"inside");
    symlink(outside.0.join("other.srt"), f.0.join("final.srt")).unwrap();
    symlink(&outside.0, f.0.join("escape")).unwrap();
    assert_eq!(
        read_bounded(&f.root(), "folder", 32, &mut || true),
        Err(ReadError::NotRegular)
    );
    assert!(read_bounded(&f.root(), "final.srt", 32, &mut || true).is_err());
    assert!(read_bounded(&f.root(), "escape/other.srt", 32, &mut || true).is_err());
}

#[test]
fn fifo_is_opened_nonblocking_then_rejected_as_nonregular() {
    let f = Fixture::new();
    let path = f.0.join("pipe.srt");
    let path = std::ffi::CString::new(path.as_os_str().as_bytes()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(path.as_ptr(), 0o600) }, 0);
    assert_eq!(
        read_bounded(&f.root(), "pipe.srt", 32, &mut || true),
        Err(ReadError::NotRegular)
    );
}

#[test]
fn root_path_replaced_with_symlink_does_not_redirect_held_authority() {
    let f = Fixture::new();
    let outside = Fixture::new();
    f.write("selected/film.srt", b"authorized");
    outside.write("film.srt", b"outside");
    let root = File::open(f.0.join("selected")).unwrap();
    fs::rename(f.0.join("selected"), f.0.join("retained")).unwrap();
    symlink(&outside.0, f.0.join("selected")).unwrap();
    assert_eq!(
        read_bounded(&root, "film.srt", 32, &mut || true).unwrap(),
        b"authorized"
    );
}

#[test]
fn child_swapped_for_symlink_after_root_capture_is_rejected() {
    let f = Fixture::new();
    let outside = Fixture::new();
    f.write("folder/film.srt", b"authorized");
    outside.write("film.srt", b"outside");
    let root = f.root();
    fs::rename(f.0.join("folder"), f.0.join("retained")).unwrap();
    symlink(&outside.0, f.0.join("folder")).unwrap();
    assert!(read_bounded(&root, "folder/film.srt", 32, &mut || true).is_err());
}

#[test]
fn filename_replaced_after_open_reads_the_original_descriptor() {
    let f = Fixture::new();
    f.write("film.srt", b"authorized");
    let file = open_regular(&f.root(), "film.srt", &mut || true).unwrap();
    fs::rename(f.0.join("film.srt"), f.0.join("original.srt")).unwrap();
    f.write("film.srt", b"replacement");
    assert_eq!(
        read_open_file(file, 32, &mut || true).unwrap(),
        b"authorized"
    );
}

#[test]
fn zero_exact_and_one_byte_over_limits_are_distinct() {
    let f = Fixture::new();
    f.write("empty.srt", b"");
    f.write("exact.srt", b"1234");
    f.write("over.srt", b"12345");
    assert_eq!(
        read_bounded(&f.root(), "empty.srt", 0, &mut || true).unwrap(),
        b""
    );
    assert_eq!(
        read_bounded(&f.root(), "exact.srt", 4, &mut || true).unwrap(),
        b"1234"
    );
    assert_eq!(
        read_bounded(&f.root(), "over.srt", 4, &mut || true),
        Err(ReadError::TooLarge)
    );
    assert_eq!(
        read_bounded(&f.root(), "exact.srt", 0, &mut || true),
        Err(ReadError::TooLarge)
    );
}

#[test]
fn growth_after_size_check_is_caught_by_the_read_bound() {
    let f = Fixture::new();
    f.write("growing.srt", b"1234");
    let file = open_regular(&f.root(), "growing.srt", &mut || true).unwrap();
    let mut grew = false;
    // read_open_file checks metadata before its first checkpoint. Appending at
    // that checkpoint proves the limit belongs to the read, not only fstat.
    let result = read_open_file(file, 4, &mut || {
        if !grew {
            OpenOptions::new()
                .append(true)
                .open(f.0.join("growing.srt"))
                .unwrap()
                .write_all(b"5")
                .unwrap();
            grew = true;
        }
        true
    });
    assert!(grew);
    assert_eq!(result, Err(ReadError::TooLarge));
}

#[test]
fn cancellation_before_open_and_after_read_does_not_return_bytes() {
    let f = Fixture::new();
    f.write("film.srt", b"secret fixture");
    assert_eq!(
        read_bounded(&f.root(), "missing.srt", 32, &mut || false),
        Err(ReadError::Aborted)
    );
    let file = open_regular(&f.root(), "film.srt", &mut || true).unwrap();
    let mut checkpoints = 0;
    assert_eq!(
        read_open_file(file, 32, &mut || {
            checkpoints += 1;
            checkpoints == 1
        }),
        Err(ReadError::Aborted)
    );
    assert_eq!(checkpoints, 2);
}

#[test]
fn error_debug_never_contains_a_private_path() {
    let f = Fixture::new();
    let error = read_bounded(&f.root(), "private-missing-name.srt", 32, &mut || true).unwrap_err();
    assert!(matches!(error, ReadError::Io(io::ErrorKind::NotFound)));
    assert!(!format!("{error:?}").contains("private-missing-name"));
    assert!(!format!("{error:?}").contains(f.0.to_str().unwrap()));
}

#[test]
fn exact_directory_depth_boundary_matches_local_scan() {
    let f = Fixture::new();
    let at_limit = format!("{}/film.srt", vec!["a"; MAX_DIRECTORY_DEPTH].join("/"));
    f.write(&at_limit, b"boundary");
    assert_eq!(
        read_bounded(&f.root(), &at_limit, 8, &mut || true).unwrap(),
        b"boundary"
    );
    let over_limit = format!("a/{at_limit}");
    assert_eq!(
        read_bounded(&f.root(), &over_limit, 8, &mut || panic!(
            "over-depth path reached IO"
        )),
        Err(ReadError::InvalidPath)
    );
}

#[test]
fn cancellation_while_descending_precedes_missing_leaf_lookup() {
    let f = Fixture::new();
    fs::create_dir(f.0.join("inner")).unwrap();
    let mut checkpoints = 0;
    let result = read_bounded(&f.root(), "inner/missing.srt", 32, &mut || {
        checkpoints += 1;
        // Initial root checks, then before/after opening the first directory.
        // Abort there: attempting the absent leaf would instead return NotFound.
        checkpoints < 4
    });
    assert_eq!(result, Err(ReadError::Aborted));
    assert_eq!(checkpoints, 4);
}

#[test]
fn oversized_regular_file_reaches_the_byte_limit_without_32_bit_open_overflow() {
    let f = Fixture::new();
    let path = f.0.join("large.mkv");
    let file = OpenOptions::new().write(true).create_new(true).open(&path).unwrap();
    // Sparse logical length; no multi-gigabyte allocation or file-content copy.
    file.set_len(i32::MAX as u64 + 4097).unwrap();
    assert_eq!(
        read_bounded(&f.root(), "large.mkv", 64, &mut || true),
        Err(ReadError::TooLarge)
    );
}
