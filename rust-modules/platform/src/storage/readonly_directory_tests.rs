//! Uses the parent read-only filesystem fixture; no device or network access.
use super::*;

fn rows(list: &DirectoryListing) -> Vec<(&str, EntryKind)> {
    let mut rows: Vec<_> = list.entries.iter().map(|entry| (entry.name.as_str(), entry.kind)).collect();
    rows.sort_unstable_by_key(|row| row.0);
    rows
}

#[test]
fn immediate_types_are_observed_without_following_links_or_opening_bodies() {
    let f = Fixture::new();
    let outside = Fixture::new();
    f.write("season/film.en.srt", b"text");
    f.write("season/nested/hidden.srt", b"not recursively enumerated");
    outside.write("outside.srt", b"outside");
    symlink(outside.0.join("outside.srt"), f.0.join("season/link.srt")).unwrap();
    let fifo = std::ffi::CString::new(f.0.join("season/pipe.srt").as_os_str().as_bytes()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);
    let result = list_bounded(&f.root(), "season", 8, 1024, &mut || true).unwrap();
    assert!(result.complete);
    assert_eq!(rows(&result), [
        ("film.en.srt", EntryKind::RegularFile),
        ("link.srt", EntryKind::Symlink),
        ("nested", EntryKind::Directory),
        ("pipe.srt", EntryKind::Other),
    ]);
}

#[test]
fn repeated_and_early_stopped_listing_never_consumes_the_root_descriptor_offset() {
    let f = Fixture::new();
    f.write("a.srt", b"a");
    f.write("b.srt", b"b");
    let root = f.root();
    let short = list_bounded(&root, "", 1, 1024, &mut || true).unwrap();
    assert!(!short.complete);
    assert_eq!(short.entries.len(), 1);
    let first = list_bounded(&root, "", 2, 1024, &mut || true).unwrap();
    let again = list_bounded(&root, "", 2, 1024, &mut || true).unwrap();
    assert!(first.complete && again.complete);
    assert_eq!(rows(&first), rows(&again));
    assert_eq!(first.entries.len(), 2);
}

#[test]
fn entry_limit_reports_exact_completion_and_no_truncated_complete_prefix() {
    let f = Fixture::new();
    let root = f.root();
    assert!(list_bounded(&root, "", 0, 0, &mut || true).unwrap().complete);
    f.write("one.srt", b"x");
    let zero = list_bounded(&root, "", 0, 128, &mut || true).unwrap();
    assert!(zero.entries.is_empty() && !zero.complete);
    assert!(list_bounded(&root, "", 1, 128, &mut || true).unwrap().complete);
    f.write("two.srt", b"x");
    let limited = list_bounded(&root, "", 1, 128, &mut || true).unwrap();
    assert_eq!(limited.entries.len(), 1);
    assert!(!limited.complete);
}

#[test]
fn aggregate_name_budget_is_checked_before_retaining_a_long_name() {
    let f = Fixture::new();
    let name = format!("{}.srt", "n".repeat(200));
    f.write(&name, b"x");
    let root = f.root();
    let short = list_bounded(&root, "", 8, name.len() - 1, &mut || true).unwrap();
    assert!(!short.complete && short.entries.is_empty());
    let exact = list_bounded(&root, "", 8, name.len(), &mut || true).unwrap();
    assert!(exact.complete);
    assert_eq!(exact.entries[0].name, name);
}

#[test]
fn cancellation_discards_the_observation_before_and_during_iteration() {
    let f = Fixture::new();
    f.write("a.srt", b"a");
    f.write("b.srt", b"b");
    let root = f.root();
    assert!(matches!(list_bounded(&root, "", 8, 1024, &mut || false), Err(ReadError::Aborted)));
    let mut checks = 0;
    let result = list_bounded(&root, "", 8, 1024, &mut || {
        checks += 1;
        checks < 8
    });
    assert!(matches!(result, Err(ReadError::Aborted)));
    assert!(list_bounded(&root, "", 8, 1024, &mut || true).unwrap().complete);
}

#[test]
fn root_relative_parent_never_follows_a_link_or_traverses_outside() {
    let f = Fixture::new();
    let outside = Fixture::new();
    outside.write("private.srt", b"private");
    symlink(&outside.0, f.0.join("linked")).unwrap();
    let root = f.root();
    for parent in ["linked", "../escape", "/", "a/../b", "a//b"] {
        assert!(list_bounded(&root, parent, 8, 1024, &mut || true).is_err());
    }
}

// Linux filesystems can expose non-UTF-8 names. macOS rejects their creation
// with EILSEQ, so keep this real directory-entry coverage on Linux.
#[cfg(target_os = "linux")]
#[test]
fn invalid_utf8_names_make_the_result_incomplete_instead_of_claiming_absence() {
    use std::os::unix::ffi::OsStrExt;
    let f = Fixture::new();
    fs::write(f.0.join(std::ffi::OsStr::from_bytes(b"bad-\xff.srt")), b"x").unwrap();
    let result = list_bounded(&f.root(), "", 8, 1024, &mut || true).unwrap();
    assert!(!result.complete);
    assert!(result.entries.is_empty());
}

#[test]
fn unrepresentable_names_make_the_result_incomplete_instead_of_claiming_absence() {
    let f = Fixture::new();
    // A real control-character name exercises the same incomplete-observation
    // contract on both macOS and Linux without requiring invalid UTF-8 support.
    f.write("bad-\n.srt", b"x");
    let result = list_bounded(&f.root(), "", 8, 1024, &mut || true).unwrap();
    assert!(!result.complete);
    assert!(result.entries.is_empty());
}

#[test]
fn large_movie_does_not_make_adjacent_asset_inventory_incomplete() {
    let f = Fixture::new();
    f.write("large.en.srt", b"subtitle");
    let file = OpenOptions::new().write(true).create_new(true)
        .open(f.0.join("large.mkv")).unwrap();
    file.set_len(i32::MAX as u64 + 4097).unwrap();
    let listing = list_bounded(&f.root(), "", 4, 1024, &mut || true).unwrap();
    assert!(listing.complete);
    assert_eq!(rows(&listing), [
        ("large.en.srt", EntryKind::RegularFile),
        ("large.mkv", EntryKind::RegularFile),
    ]);
}

#[test]
fn directory_listing_keeps_the_filesystems_actual_inode_range() {
    use std::os::unix::fs::MetadataExt;
    let f = Fixture::new();
    f.write("movie.en.srt", b"subtitle");
    let inode = fs::symlink_metadata(f.0.join("movie.en.srt")).unwrap().ino();
    // Wide inode allocation is a filesystem property; never fake it or change
    // mounts. This records whether this run exercised >32-bit inode numbers.
    eprintln!("readonly inode > u32::MAX exercised: {}", inode > u32::MAX as u64);
    let listing = list_bounded(&f.root(), "", 4, 1024, &mut || true).unwrap();
    assert!(listing.complete);
    assert_eq!(rows(&listing), [("movie.en.srt", EntryKind::RegularFile)]);
}


#[test]
fn streaming_stop_is_not_eof_and_does_not_consume_the_next_observation() {
    let f = Fixture::new();
    f.write("a.mp4", b"a");
    f.write("b.mp4", b"b");
    let root = f.root();
    let mut count = 0;
    let stopped = visit_bounded(&root, "", 8, &mut || true, &mut |entry, bytes| {
        let entry = entry.unwrap();
        assert_eq!(bytes, entry.name.len());
        count += 1;
        Ok(false)
    }).unwrap();
    assert_eq!(count, 1);
    assert!(!stopped.reached_end && !stopped.complete);
    let full = list_bounded(&root, "", 8, 1024, &mut || true).unwrap();
    assert!(full.complete);
    assert_eq!(full.entries.len(), 2);
}

#[test]
fn streaming_uses_the_held_directory_after_its_path_is_replaced() {
    let f = Fixture::new();
    f.write("chosen/a.mp4", b"a");
    f.write("chosen/b.mp4", b"b");
    let root = File::open(f.0.join("chosen")).unwrap();
    let mut names = Vec::new();
    let result = visit_bounded(&root, "", 8, &mut || true, &mut |entry, _| {
        if names.is_empty() {
            fs::rename(f.0.join("chosen"), f.0.join("retained")).unwrap();
            f.write("chosen/replacement.mp4", b"other source");
        }
        names.push(entry.unwrap().name);
        Ok(true)
    }).unwrap();
    assert!(result.reached_end && result.complete);
    names.sort();
    assert_eq!(names, ["a.mp4", "b.mp4"]);
    let again = list_bounded(&root, "", 8, 1024, &mut || true).unwrap();
    assert_eq!(rows(&again), [("a.mp4", EntryKind::RegularFile), ("b.mp4", EntryKind::RegularFile)]);
}

#[test]
fn cancellation_from_the_last_stream_callback_cannot_be_reported_as_eof_or_stop() {
    for keep_going in [false, true] {
        let f = Fixture::new();
        f.write("only.mp4", b"a");
        let live = std::cell::Cell::new(true);
        let mut count = 0;
        let result = visit_bounded(&f.root(), "", 8, &mut || live.get(), &mut |_, _| {
            count += 1;
            live.set(false);
            Ok(keep_going)
        });
        assert_eq!(count, 1);
        assert_eq!(result, Err(ReadError::Aborted));
    }
}
