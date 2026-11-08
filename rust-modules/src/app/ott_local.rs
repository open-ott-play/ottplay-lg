//! Read existing local media in bounded batches. Runs on the catalogue worker, never the UI.
//! No imports, thumbnails, duration probes or copies are made while walking the tree.
use plx_data::ott::{self, CatalogEntry, LocalFile, MediaLocator, ProviderKind, ProviderProfile};
use plx_base::task::OffFrame;
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use plx_platform::storage::readonly::{self, EntryKind};
use std::sync::atomic::{AtomicBool, Ordering};

const BATCH_SIZE: usize = 64;
const DIRECTORY_LIMIT: usize = 20_000;
const ENTRY_LIMIT: usize = 500_000;
const PENDING_PATH_BYTES: usize = 8 * 1024 * 1024;
const DEPTH_LIMIT: usize = 64;

/// A cancelled scan has no claim about the unseen remainder of its tree.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct ScanReport {
    pub videos: usize,
    pub directories: usize,
    pub inspected: usize,
    pub skipped: usize,
    pub cancelled: bool,
    pub depth_limited: bool,
}

pub(super) fn visit(
    off: &OffFrame,
    profile: &ProviderProfile,
    cancel: &AtomicBool,
    emit: &mut dyn FnMut(Vec<CatalogEntry>) -> Result<(), &'static str>,
) -> Result<(), &'static str> {
    let report = visit_report(off, profile, cancel, emit)?;
    outcome(report)
}

/// Emit only the selected, already-authorized regular file. This is the same
/// catalogue worker/builder as folders, without walking or granting its parent.
pub(super) fn visit_single_file(
    off: &plx_base::task::OffFrame,
    profile: &ProviderProfile,
    source: &crate::app::ott_local_root::LocalSource,
    cancel: &AtomicBool,
    emit: &mut dyn FnMut(Vec<CatalogEntry>) -> Result<(), &'static str>,
) -> Result<(), &'static str> {
    use crate::app::ott_local_session::SourceKind;
    if cancel.load(Ordering::Acquire) { return Err("Catalogue cancelled"); }
    profile.validate().map_err(|_| "Invalid local profile")?;
    if profile.kind != ProviderKind::Local || source.descriptor().kind != SourceKind::File
        || source.descriptor().canonical_path.as_deref() != Some(profile.source.as_str())
        || !source.is_live()
    {
        return Err("Local file source is unavailable");
    }
    let observed = source.observe(off, &mut || !cancel.load(Ordering::Acquire))
        .map_err(|_| "Catalogue cancelled")?;
    if !observed.is_live() || !source.same_grant(&observed) {
        return Err("Local file source is unavailable");
    }
    let name = observed.single_file_name().ok_or("Local file source is unavailable")?;
    if !ott::valid_local_relative_path(name) || !ott::is_local_video_path(name) {
        return Err("Unsupported local video file");
    }
    let file = LocalFile { relative_path: name.into(), size: 0, duration_ms: None };
    let batch = ott::catalog_from_local_files(&[file], profile)
        .map_err(|_| "Local file has an invalid name")?;
    if batch.entries.len() != 1 { return Err("Local file is not playable"); }
    if cancel.load(Ordering::Acquire) { return Err("Catalogue cancelled"); }
    if !source.is_live() { return Err("Local file source is unavailable"); }
    emit(batch.entries)?;
    if cancel.load(Ordering::Acquire) { return Err("Catalogue cancelled"); }
    if !source.is_live() { return Err("Local file source is unavailable"); }
    Ok(())
}

impl ScanReport {
    /// A warning is not an unfinished scan. Only the caller that received this
    /// final report (rather than an early error) may publish terminal progress.
    pub(super) fn warning(&self) -> Option<&'static str> {
        (!self.cancelled && !self.depth_limited && self.skipped > 0).then_some(
            "Some local folders or files were unavailable; the readable videos remain available")
    }
}

fn outcome(report: ScanReport) -> Result<(), &'static str> {
    if report.depth_limited { return Err("Local folder depth limit reached"); }
    report.warning().map_or(Ok(()), Err)
}

pub(super) fn root(profile: &ProviderProfile) -> Result<PathBuf, &'static str> {
    profile.validate().map_err(|_| "Invalid local profile")?;
    if profile.kind != ProviderKind::Local || !Path::new(&profile.source).is_absolute() {
        return Err("Local source is not a device folder");
    }
    let root = std::fs::canonicalize(&profile.source).map_err(|_| "Local folder is unavailable")?;
    if !root.is_dir() {
        return Err("Local source is not a folder");
    }
    Ok(root)
}

/// Do not collect or sort a whole directory before publishing the first media item.
/// IDs derive from relative names, not the filesystem's unspecified enumeration order.
pub(super) fn visit_report(
    off: &OffFrame,
    profile: &ProviderProfile,
    cancel: &AtomicBool,
    emit: &mut dyn FnMut(Vec<CatalogEntry>) -> Result<(), &'static str>,
) -> Result<ScanReport, &'static str> {
    let mut report = ScanReport::default();
    if cancelled(cancel, &mut report) { return Ok(report); }
    let path = root(profile)?;
    let before = std::fs::symlink_metadata(&path).map_err(|_| "Local folder is unavailable")?;
    let file = std::fs::OpenOptions::new().read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK)
        .open(&path).map_err(|_| "Local folder cannot be read")?;
    check_root_path(&path, &file, &before)?;
    let report = walk(off, profile, &file, cancel, &|| true, emit)?;
    check_root_path(&path, &file, &before)?;
    Ok(report)
}

/// Remove a known child at its real open boundary, independent of worker order.
#[cfg(all(test, feature = "hostsim"))]
pub(super) fn visit_with_lost_child_for_test(
    off: &OffFrame, profile: &ProviderProfile, cancel: &AtomicBool,
    emit: &mut dyn FnMut(Vec<CatalogEntry>) -> Result<(), &'static str>,
) -> Result<ScanReport, &'static str> {
    let path = root(profile)?;
    let root = std::fs::File::open(&path).map_err(|_| "Local folder cannot be read")?;
    let removed = AtomicBool::new(false);
    let report = walk_with(off, profile, &root, cancel, &|| true, emit,
        &|stage, relative, _| {
            if stage == WalkStage::Open && relative == "Second" {
                assert!(!removed.swap(true, Ordering::AcqRel), "child open must occur once");
                std::fs::remove_dir_all(path.join(relative)).expect("remove fixture child before open");
            }
            Ok(())
        })?;
    assert!(removed.load(Ordering::Acquire), "fixture must reach the child open boundary");
    assert_eq!(report.skipped, 1, "real missing-child open must be reported");
    Ok(report)
}

/// The normal folder task carries the already admitted source. No path lookup
/// in the walk can replace that root, including between task queueing and start.
pub(super) fn visit_source_report(
    off: &plx_base::task::OffFrame,
    profile: &ProviderProfile,
    source: &crate::app::ott_local_root::LocalSource,
    cancel: &AtomicBool,
    emit: &mut dyn FnMut(Vec<CatalogEntry>) -> Result<(), &'static str>,
) -> Result<ScanReport, &'static str> {
    use crate::app::ott_local_session::SourceKind;
    profile.validate().map_err(|_| "Invalid local profile")?;
    if profile.kind != ProviderKind::Local || source.descriptor().kind != SourceKind::Folder
        || source.descriptor().canonical_path.as_deref() != Some(profile.source.as_str()) {
        return Err("Local folder source is unavailable");
    }
    let observed = source.observe(off, &mut || !cancel.load(Ordering::Acquire))
        .map_err(|_| "Catalogue cancelled")?;
    if !observed.is_live() || !source.same_grant(&observed) {
        return Err("Local folder source is unavailable");
    }
    let root = observed.root_handle().ok_or("Local folder source is unavailable")?;
    let file = root.file_if_live(off).ok_or("Local folder source is unavailable")?;
    let report = walk(off, profile, file, cancel, &|| source.is_live(), emit)?;
    if report.cancelled { return Ok(report); }
    let after = source.observe(off, &mut || !cancel.load(Ordering::Acquire))
        .map_err(|_| "Catalogue cancelled")?;
    if !after.is_live() || !source.same_grant(&after) {
        return Err("Local folder changed during scan");
    }
    Ok(report)
}

fn check_root_path(path: &Path, file: &std::fs::File, before: &std::fs::Metadata)
    -> Result<(), &'static str> {
    let opened = file.metadata().map_err(|_| "Local folder is unavailable")?;
    let after = std::fs::symlink_metadata(path).map_err(|_| "Local folder is unavailable")?;
    if !opened.is_dir() || !after.is_dir() || before.dev() != opened.dev()
        || before.ino() != opened.ino() || after.dev() != opened.dev() || after.ino() != opened.ino() {
        return Err("Local folder changed during scan");
    }
    Ok(())
}

/// One recursive policy for retained-source and legacy catalogue callers.
/// Only the owner publishes. Four scoped workers retain all admitted I/O until
/// it returns, even after cancellation; the enclosing catalogue cannot finish
/// and admit a replacement before that drain completes.
fn walk(
    off: &OffFrame,
    profile: &ProviderProfile,
    root: &std::fs::File,
    cancel: &AtomicBool,
    live: &(dyn Fn() -> bool + Sync),
    emit: &mut dyn FnMut(Vec<CatalogEntry>) -> Result<(), &'static str>,
) -> Result<ScanReport, &'static str> {
    walk_with(off, profile, root, cancel, live, emit, &|_, _, _| Ok(()))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WalkStage { Open, Next, Probe }
enum WalkTask {
    Open,
    Next(readonly::DirectoryCursor, usize),
    Probe(std::sync::Arc<std::fs::File>, String),
}
struct WalkJob { path: String, depth: usize, task: WalkTask }
impl WalkJob {
    fn directory(&self) -> bool { !matches!(self.task, WalkTask::Probe(..)) }
    fn bytes(&self, root_bytes: usize) -> usize {
        root_bytes.saturating_add(self.path.len()).saturating_add(match &self.task {
            WalkTask::Probe(_, name) => name.len(), _ => 0,
        }).saturating_add(1)
    }
}
enum WalkResult {
    Open(Result<readonly::DirectoryCursor, readonly::ReadError>),
    Next(readonly::DirectoryCursor, usize, Result<Option<readonly::DirectoryName>, readonly::ReadError>),
    Probe(Result<EntryKind, readonly::ReadError>),
    Panic,
}
struct WalkEvent { path: String, depth: usize, directory: bool, result: WalkResult }

fn queue_walk(queue: &mut VecDeque<WalkJob>, bytes: &mut usize, root_bytes: usize, job: WalkJob)
    -> Result<(), &'static str> {
    let size = job.bytes(root_bytes);
    if bytes.saturating_add(size) > PENDING_PATH_BYTES { return Err("Local pending folder limit reached"); }
    *bytes += size;
    if job.directory() { queue.push_back(job); } else { queue.push_front(job); }
    Ok(())
}
fn execute_walk(job: WalkJob, root: &std::fs::File, check: &mut dyn FnMut() -> bool,
    before: &(dyn Fn(WalkStage, &str, usize) -> Result<(), readonly::ReadError> + Sync)) -> WalkEvent {
    let directory = job.directory();
    let WalkJob { path, depth, task } = job;
    let (stage, step) = match &task {
        WalkTask::Open => (WalkStage::Open, 0),
        WalkTask::Next(_, step) => (WalkStage::Next, *step),
        WalkTask::Probe(..) => (WalkStage::Probe, 0),
    };
    let allowed = if check() { before(stage, &path, step) } else { Err(readonly::ReadError::Aborted) }
        .and_then(|_| if check() { Ok(()) } else { Err(readonly::ReadError::Aborted) });
    let result = match task {
        WalkTask::Open => WalkResult::Open(allowed.and_then(|_| readonly::DirectoryCursor::open(root, &path, check))),
        WalkTask::Next(mut cursor, step) => {
            let result = allowed.and_then(|_| cursor.next(check));
            WalkResult::Next(cursor, step, result)
        }
        WalkTask::Probe(directory, name) => WalkResult::Probe(allowed.and_then(|_| readonly::probe_kind(&directory, &name, check))),
    };
    WalkEvent { path, depth, directory, result }
}

// Fault fixtures intercept an actual operation, not a replacement traversal.
fn walk_with(
    off: &OffFrame,
    profile: &ProviderProfile,
    root: &std::fs::File,
    cancel: &AtomicBool,
    live: &(dyn Fn() -> bool + Sync),
    emit: &mut dyn FnMut(Vec<CatalogEntry>) -> Result<(), &'static str>,
    before: &(dyn Fn(WalkStage, &str, usize) -> Result<(), readonly::ReadError> + Sync),
) -> Result<ScanReport, &'static str> {
    use std::sync::{Arc, Mutex, mpsc};
    use plx_base::task::time::Deadline;
    use std::time::Duration;
    const WORKERS: usize = 4;
    const CURSORS: usize = 64;
    let retired = AtomicBool::new(false);
    std::thread::scope(|scope| {
        let (jobs, receiver) = mpsc::sync_channel::<WalkJob>(WORKERS);
        let receiver = Arc::new(Mutex::new(receiver));
        let (events, results) = mpsc::sync_channel::<WalkEvent>(WORKERS);
        for _ in 0..WORKERS {
            let receiver = receiver.clone();
            let events = events.clone();
            let retired = &retired;
            scope.spawn(move || {
                loop {
                    // Drop the receive lock before any filesystem operation.
                    let received = receiver.lock().unwrap().recv();
                    let job = match received { Ok(job) => job, Err(_) => break };
                    let path = job.path.clone();
                    let depth = job.depth;
                    let directory = job.directory();
                    let event = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        execute_walk(job, root, &mut || !retired.load(Ordering::Acquire)
                            && !cancel.load(Ordering::Acquire) && live(), before)
                    })).unwrap_or(WalkEvent { path, depth, directory, result: WalkResult::Panic });
                    if events.send(event).is_err() { break; }
                }
            });
        }
        drop(events);
        let root_bytes = profile.source.len();
        let mut queue = VecDeque::new();
        let mut queued_bytes = 0;
        let mut active = 0usize;
        let mut directory_jobs = 0usize;
        let mut cursors = 0usize;
        let mut found_directories = 1usize;
        let mut report = ScanReport::default();
        let mut files = Vec::with_capacity(BATCH_SIZE);
        let mut terminal = None;
        // Cadence belongs to this worker; it does not limit the scan lifetime.
        let mut next_publish = Deadline::after(off, Duration::from_millis(100));
        queue_walk(&mut queue, &mut queued_bytes, root_bytes,
            WalkJob { path: String::new(), depth: 0, task: WalkTask::Open })?;
        loop {
            if cancel.load(Ordering::Acquire) { report.cancelled = true; retired.store(true, Ordering::Release); }
            if !live() { terminal = Some("Local source was withdrawn"); retired.store(true, Ordering::Release); }
            // A ready next item must not wait for a full batch or unrelated I/O.
            if !retired.load(Ordering::Acquire) && !files.is_empty()
                && next_publish.expired(off) {
                if let Err(error) = flush(profile, cancel, &mut report, &mut files, live, emit) {
                    terminal = Some(error); retired.store(true, Ordering::Release);
                }
                if report.cancelled { retired.store(true, Ordering::Release); }
                next_publish = Deadline::after(off, Duration::from_millis(100));
            }
            if retired.load(Ordering::Acquire) {
                queue.clear(); queued_bytes = 0;
            } else if terminal.is_some() {
                // Already observed names may still produce the readable prefix.
                // Do not discover new names/directories after a terminal failure.
                queue.retain(|job| !job.directory());
                queued_bytes = queue.iter().map(|job| job.bytes(root_bytes)).sum();
            }
            while active < WORKERS && !queue.is_empty() {
                let position = queue.iter().position(|job| {
                    !job.directory() || (directory_jobs < WORKERS - 1
                        && (!matches!(job.task, WalkTask::Open) || cursors < CURSORS))
                });
                let Some(position) = position else { break; };
                let job = queue.remove(position).expect("queued position");
                queued_bytes -= job.bytes(root_bytes);
                if job.directory() { directory_jobs += 1; }
                if matches!(job.task, WalkTask::Open) { cursors += 1; report.directories += 1; }
                jobs.send(job).map_err(|_| "Local scan worker failed")?;
                active += 1;
            }
            if active == 0 {
                if queue.is_empty() { break; }
                terminal = Some("Local directory cursor limit reached");
                continue;
            }
            let event = match results.recv_timeout(Duration::from_millis(50)) {
                Ok(event) => event,
                Err(mpsc::RecvTimeoutError::Timeout) => continue,
                Err(_) => { terminal = Some("Local scan worker failed"); break; }
            };
            active -= 1;
            if event.directory { directory_jobs -= 1; }
            if retired.load(Ordering::Acquire) { continue; }
            let WalkEvent { path, depth, result, .. } = event;
            let processed = (|| -> Result<(), &'static str> {
                let failure = match result {
                    WalkResult::Open(Ok(cursor)) => {
                        if terminal.is_none() { queue_walk(&mut queue, &mut queued_bytes, root_bytes,
                            WalkJob { path, depth, task: WalkTask::Next(cursor, 0) })?; }
                        return Ok(());
                    }
                    WalkResult::Open(Err(error)) => { cursors = cursors.saturating_sub(1); error }
                    WalkResult::Next(cursor, step, Ok(Some(entry))) => {
                        if report.inspected >= ENTRY_LIMIT { return Err("Local directory entry limit reached"); }
                        report.inspected += 1;
                        if let Some(name) = entry.name {
                            let relative = if path.is_empty() { name.clone() } else { format!("{path}/{name}") };
                            queue_walk(&mut queue, &mut queued_bytes, root_bytes, WalkJob {
                                path: relative, depth, task: WalkTask::Probe(cursor.directory(), name),
                            })?;
                        } else { report.skipped += 1; }
                        if terminal.is_none() { queue_walk(&mut queue, &mut queued_bytes, root_bytes,
                            WalkJob { path, depth, task: WalkTask::Next(cursor, step + 1) })?; }
                        return Ok(());
                    }
                    WalkResult::Next(_, _, Ok(None)) => { cursors = cursors.saturating_sub(1); return Ok(()); }
                    WalkResult::Next(_, _, Err(error)) => { cursors = cursors.saturating_sub(1); error }
                    WalkResult::Probe(Ok(kind)) => {
                        match kind {
                            EntryKind::Directory if terminal.is_none() => {
                                if depth >= DEPTH_LIMIT { report.depth_limited = true; report.skipped += 1; }
                                else {
                                    if found_directories >= DIRECTORY_LIMIT { return Err("Local folder limit reached"); }
                                    found_directories += 1;
                                    queue_walk(&mut queue, &mut queued_bytes, root_bytes,
                                        WalkJob { path, depth: depth + 1, task: WalkTask::Open })?;
                                }
                            }
                            EntryKind::RegularFile if ott::is_local_video_path(&path) => {
                                if !ott::valid_local_relative_path(&path) { report.skipped += 1; }
                                else {
                                    if report.videos >= ott::MAX_CATALOG_ENTRIES { return Err("Local video limit reached"); }
                                    report.videos += 1;
                                    files.push(LocalFile { relative_path: path, size: 0, duration_ms: None });
                                    if report.videos == 1 || files.len() >= BATCH_SIZE {
                                        if let Err(error) = flush(profile, cancel, &mut report, &mut files, live, emit) {
                                            retired.store(true, Ordering::Release);
                                            return Err(error);
                                        }
                                        next_publish = Deadline::after(off, Duration::from_millis(100));
                                    }
                                }
                            }
                            EntryKind::Unreadable => report.skipped += 1,
                            _ => {},
                        }
                        return Ok(());
                    }
                    WalkResult::Probe(Err(error)) => error,
                    WalkResult::Panic => return Err("Local scan worker failed"),
                };
                if failure == readonly::ReadError::AllocationFailed { return Err("Local directory allocation failed"); }
                if failure == readonly::ReadError::Aborted {
                    if cancel.load(Ordering::Acquire) { report.cancelled = true; retired.store(true, Ordering::Release); return Ok(()); }
                    return Err("Local source was withdrawn");
                }
                if depth == 0 { return Err("Local folder cannot be read"); }
                report.skipped += 1;
                Ok(())
            })();
            if let Err(error) = processed { terminal = Some(error); }
        }
        // Closing the queue and leaving scope joins every worker, including a
        // provider call that ignored cancellation. No detached replacement work.
        drop(jobs);
        if !retired.load(Ordering::Acquire) { flush(profile, cancel, &mut report, &mut files, live, emit)?; }
        match terminal { Some(error) => Err(error), None => Ok(report) }
    })
}

fn cancelled(cancel: &AtomicBool, report: &mut ScanReport) -> bool {
    report.cancelled = cancel.load(Ordering::Acquire);
    report.cancelled
}


fn flush(
    profile: &ProviderProfile,
    cancel: &AtomicBool,
    report: &mut ScanReport,
    files: &mut Vec<LocalFile>,
    live: &dyn Fn() -> bool,
    emit: &mut dyn FnMut(Vec<CatalogEntry>) -> Result<(), &'static str>,
) -> Result<(), &'static str> {
    if !live() { return Err("Local source was withdrawn"); }
    if cancelled(cancel, report) || files.is_empty() {
        return Ok(());
    }
    let batch = ott::catalog_from_local_files(files, profile)
        .map_err(|_| "Local file has an invalid name")?;
    files.clear();
    if cancelled(cancel, report) {
        return Ok(());
    }
    emit(batch.entries)?;
    if !live() { return Err("Local source was withdrawn"); }
    cancelled(cancel, report);
    Ok(())
}

/// Revalidate at selection: an old catalogue row is not permission to leave the selected root.
pub(super) fn resolve(
    profile: &ProviderProfile,
    entry: &CatalogEntry,
) -> Result<PathBuf, &'static str> {
    let MediaLocator::Local { relative_path } = &entry.media else {
        return Err("Media is not a local file");
    };
    if !ott::valid_local_relative_path(relative_path) {
        return Err("Invalid local media path");
    }
    let root = root(profile)?;
    checked_path(&root, &root.join(relative_path), false)
}

fn checked_path(root: &Path, path: &Path, directory: bool) -> Result<PathBuf, &'static str> {
    let relative = path
        .strip_prefix(root)
        .map_err(|_| "Local path escaped its folder")?;
    let mut cursor = root.to_path_buf();
    for component in relative.components() {
        let std::path::Component::Normal(name) = component else {
            return Err("Invalid local media path");
        };
        cursor.push(name);
        let kind = std::fs::symlink_metadata(&cursor)
            .map_err(|_| "Local media is unavailable")?
            .file_type();
        if kind.is_symlink() {
            return Err("Local media link is outside its registration");
        }
    }
    let canonical = std::fs::canonicalize(path).map_err(|_| "Local media is unavailable")?;
    if !canonical.starts_with(root) {
        return Err("Local path escaped its folder");
    }
    let kind = std::fs::symlink_metadata(&canonical)
        .map_err(|_| "Local media is unavailable")?
        .file_type();
    if (directory && !kind.is_dir()) || (!directory && !kind.is_file()) {
        return Err("Local media type changed");
    }
    Ok(canonical)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicU64;
    static TEMP_ID: AtomicU64 = AtomicU64::new(0);
    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "ott-local-test-{}-{}",
                std::process::id(),
                TEMP_ID.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir(&path).unwrap();
            Self(std::fs::canonicalize(path).unwrap())
        }
        fn file(&self, name: &str) {
            let path = self.0.join(name);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, b"original-media").unwrap();
        }
        fn profile(&self) -> ProviderProfile {
            ProviderProfile {
                id: "local-test".into(),
                name: "Library".into(),
                kind: ProviderKind::Local,
                source: self.0.to_str().unwrap().into(),
                source_alias: "Test".into(),
                enabled: true,
            }
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn first_media_arrives_before_rest_of_directory_and_cancellation_stops_walk() {
        let f = Fixture::new();
        for i in 0..256 {
            f.file(&format!("video-{i}.mp4"));
        }
        let cancel = AtomicBool::new(false);
        let mut batches = Vec::new();
        let report = visit_report(&OffFrame::for_test(), &f.profile(), &cancel, &mut |batch| {
            batches.push(batch);
            cancel.store(true, Ordering::Release);
            Ok(())
        })
        .unwrap();
        assert_eq!(batches.len(), 1);
        assert_eq!(batches[0].len(), 1);
        assert_eq!(report.videos, 1);
        assert!(report.inspected < 256);
        assert!(report.cancelled);
    }

    #[test]
    fn nested_catalogue_is_batched_without_copying_or_following_links() {
        let f = Fixture::new();
        f.file("ignore.txt");
        for i in 0..130 {
            f.file(&format!("Folder/film-{i}.MKV"));
        }
        let other = Fixture::new();
        other.file("escape.mp4");
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&other.0, f.0.join("outside")).unwrap();
            std::os::unix::fs::symlink(&f.0, f.0.join("cycle")).unwrap();
        }
        let mut batches = Vec::new();
        let report = visit_report(&OffFrame::for_test(), &f.profile(), &AtomicBool::new(false), &mut |batch| {
            batches.push(batch);
            Ok(())
        })
        .unwrap();
        assert_eq!(report.videos, 130);
        assert_eq!(report.directories, 2);
        assert_eq!(batches.first().unwrap().len(), 1);
        assert_eq!(batches.iter().map(Vec::len).sum::<usize>(), 130);
        assert!(batches.iter().all(|batch| !batch.is_empty() && batch.len() <= BATCH_SIZE));
        for row in batches.iter().flatten() {
            assert_eq!(row.category, "Folder");
            assert_eq!(
                std::fs::read(resolve(&f.profile(), row).unwrap()).unwrap(),
                b"original-media"
            );
        }
        assert_eq!(std::fs::read_dir(f.0.join("Folder")).unwrap().count(), 130);
    }

    #[test]
    fn disappeared_or_symlinked_queued_child_cannot_publish_external_names() {
        for link in [false, true] {
            let f = Fixture::new(); f.file("First/one.mp4"); f.file("Later/two.mp4");
            let outside = Fixture::new(); outside.file("private.mp4");
            let root = std::fs::File::open(&f.0).unwrap(); let hold = Hold::new(); let mut rows = Vec::new();
            let report = walk_with(&OffFrame::for_test(), &f.profile(), &root, &AtomicBool::new(false), &|| true,
                &mut |batch| {
                    if rows.is_empty() {
                        std::fs::remove_dir_all(f.0.join("Later")).unwrap();
                        if link { std::os::unix::fs::symlink(&outside.0, f.0.join("Later")).unwrap(); }
                        hold.release();
                    }
                    rows.extend(batch); Ok(())
                }, &|stage, path, _| {
                    if stage == WalkStage::Open && path == "Later" { hold.block()?; }
                    if stage == WalkStage::Probe && path == "First/one.mp4" && !hold.wait_entered(1) {
                        return Err(readonly::ReadError::Io(std::io::ErrorKind::TimedOut));
                    }
                    Ok(())
                }).unwrap();
            assert_eq!(report.videos, 1); assert_eq!(report.skipped, 1);
            assert!(rows.iter().all(|row| row.title != "private.mp4"));
        }
    }

    #[test]
    fn resolver_rejects_row_replaced_by_symlink_and_does_not_open_missing_root() {
        let f = Fixture::new();
        f.file("one.mp4");
        let mut rows = Vec::new();
        visit(&OffFrame::for_test(), &f.profile(), &AtomicBool::new(false), &mut |batch| {
            rows.extend(batch);
            Ok(())
        })
        .unwrap();
        std::fs::remove_file(f.0.join("one.mp4")).unwrap();
        #[cfg(unix)]
        {
            let other = Fixture::new();
            other.file("two.mp4");
            std::os::unix::fs::symlink(other.0.join("two.mp4"), f.0.join("one.mp4")).unwrap();
            assert!(resolve(&f.profile(), &rows[0]).is_err());
        }
        let mut profile = f.profile();
        profile.source.push_str("/missing");
        assert!(visit(&OffFrame::for_test(), &profile, &AtomicBool::new(false), &mut |_| Ok(())).is_err());
    }

    #[test]
    fn pre_cancelled_scan_and_closed_receiver_do_not_read_remaining_tree() {
        let f = Fixture::new();
        f.file("one.mp4");
        let report = visit_report(&OffFrame::for_test(), &f.profile(), &AtomicBool::new(true), &mut |_| {
            panic!("cancelled scan published")
        })
        .unwrap();
        assert!(report.cancelled);
        assert_eq!(report.inspected, 0);
        assert_eq!(
            visit(&OffFrame::for_test(), &f.profile(), &AtomicBool::new(false), &mut |_| Err(
                "receiver closed"
            )),
            Err("receiver closed")
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn filesystem_invalid_utf8_entry_uses_the_same_partial_catalogue_policy() {
        use std::os::unix::ffi::OsStringExt;
        let f = Fixture::new();
        f.file("one.mp4");
        f.file("two.mp4");
        std::fs::write(
            f.0.join(std::ffi::OsString::from_vec(b"bad\xff.mp4".to_vec())),
            b"media",
        )
        .unwrap();
        let mut rows = Vec::new();
        let report = visit_report(&OffFrame::for_test(), &f.profile(), &AtomicBool::new(false), &mut |batch| {
            rows.extend(batch);
            Ok(())
        })
        .unwrap();
        assert_eq!(report.videos, 2);
        assert_eq!(report.skipped, 1);
        assert_eq!(rows.len(), 2);
        assert!(visit(&OffFrame::for_test(), &f.profile(), &AtomicBool::new(false), &mut |_| Ok(())).is_err());
    }

    fn folder_grant(f: &Fixture) -> crate::app::ott_local_root::LocalSource {
        use crate::app::ott_local_session::{AuthorityRef, BoundSource, SourceKind};
        crate::app::ott_local_root::LocalSource::acquire_authorized(
            &plx_base::task::OffFrame::for_test(), BoundSource {
                id: "held-root".into(), name: "Held".into(), kind: SourceKind::Folder,
                available: true, authority_ref: AuthorityRef(1), generation: 7,
                canonical_path: Some(f.profile().source),
            }, &mut || true).unwrap()
    }

    #[test]
    fn held_folder_checks_withdrawal_and_cancellation_after_first_emission() {
        for withdraw in [false, true] {
            let f = Fixture::new();
            for i in 0..130 { f.file(&format!("video-{i}.mp4")); }
            let source = folder_grant(&f);
            let mut withdrawal = source.clone();
            let cancel = AtomicBool::new(false);
            let mut batches = Vec::new();
            let result = visit_source_report(&plx_base::task::OffFrame::for_test(),
                &f.profile(), &source, &cancel, &mut |rows| {
                    batches.push(rows.len());
                    if withdraw { withdrawal.withdraw(); }
                    else { cancel.store(true, Ordering::Release); }
                    Ok(())
                });
            assert_eq!(batches, [1]);
            if withdraw { assert!(result.is_err()); }
            else { assert!(result.unwrap().cancelled); }
        }
    }

    #[test]
    fn held_folder_never_switches_to_replacement_created_during_a_batch_callback() {
        let f = Fixture::new();
        f.file("chosen/a.mp4"); f.file("chosen/b.mp4");
        let chosen = Fixture(f.0.join("chosen"));
        let source = folder_grant(&chosen);
        let mut names = Vec::new();
        let result = visit_source_report(&plx_base::task::OffFrame::for_test(),
            &chosen.profile(), &source, &AtomicBool::new(false), &mut |rows| {
                if names.is_empty() {
                    std::fs::rename(&chosen.0, f.0.join("retained")).unwrap();
                    f.file("chosen/replacement.mp4");
                }
                names.extend(rows.into_iter().map(|row| match row.media {
                    MediaLocator::Local { relative_path } => relative_path,
                    _ => panic!("Local row"),
                }));
                Ok(())
            });
        assert!(result.is_err(), "changed named root cannot publish terminal completion");
        assert!(!names.is_empty());
        assert!(names.iter().all(|name| name == "a.mp4" || name == "b.mp4"));
        assert!(source.is_live(), "worker must not withdraw the shared grant");
    }

    struct Hold {
        released: std::sync::Mutex<bool>,
        changed: std::sync::Condvar,
        entered: std::sync::atomic::AtomicUsize,
    }
    impl Hold {
        fn new() -> Self { Self { released: std::sync::Mutex::new(false),
            changed: std::sync::Condvar::new(), entered: std::sync::atomic::AtomicUsize::new(0) } }
        fn block(&self) -> Result<(), readonly::ReadError> {
            let lock = self.released.lock().unwrap();
            self.entered.fetch_add(1, Ordering::Release);
            self.changed.notify_all();
            let (lock, timeout) = self.changed.wait_timeout_while(lock,
                std::time::Duration::from_secs(5), |released| !*released).unwrap();
            if timeout.timed_out() && !*lock { return Err(readonly::ReadError::Io(std::io::ErrorKind::TimedOut)); }
            Ok(())
        }
        fn wait_entered(&self, count: usize) -> bool {
            let lock = self.released.lock().unwrap();
            drop(self.changed.wait_timeout_while(lock, std::time::Duration::from_secs(3),
                |_| self.entered.load(Ordering::Acquire) < count).unwrap());
            self.entered.load(Ordering::Acquire) >= count
        }
        fn release(&self) { *self.released.lock().unwrap() = true; self.changed.notify_all(); }
    }

    #[test]
    fn ready_file_precedes_stalled_file_child_and_parent_operations() {
        for stage in [WalkStage::Probe, WalkStage::Open, WalkStage::Next] {
            let f = Fixture::new();
            f.file("ready/ready.mp4");
            if stage == WalkStage::Probe { f.file("held.mp4"); }
            if stage == WalkStage::Open { f.file("held/blocked.mp4"); }
            let root = std::fs::File::open(&f.0).unwrap();
            let profile = f.profile();
            let hold = Hold::new();
            let cancel = AtomicBool::new(false);
            std::thread::scope(|scope| {
                let (sent, received) = std::sync::mpsc::channel();
                let (profile, root, cancel, hold) = (&profile, &root, &cancel, &hold);
                let run = scope.spawn(move || walk_with(&OffFrame::for_test(), profile, root, cancel, &|| true,
                    &mut |rows| { for row in rows { sent.send(row).unwrap(); } Ok(()) },
                    &|operation, path, step| {
                        let target = match stage {
                            WalkStage::Probe => operation == stage && path == "held.mp4",
                            WalkStage::Open => operation == stage && path == "held",
                            WalkStage::Next => operation == stage && path.is_empty() && step == 1,
                        };
                        if target { hold.block()?; }
                        Ok(())
                    }));
                let entered = hold.wait_entered(1);
                let first = received.recv_timeout(std::time::Duration::from_secs(2));
                cancel.store(true, Ordering::Release);
                let drained_early = run.is_finished();
                hold.release();
                let report = run.join().unwrap().unwrap();
                assert!(entered, "controlled {stage:?} operation was not reached");
                let first = first.expect("ready file must precede release of the stalled operation");
                assert!(matches!(first.media, MediaLocator::Local { ref relative_path } if relative_path == "ready/ready.mp4"));
                assert!(!drained_early, "the catalogue worker must own the stalled raw call");
                assert!(report.cancelled);
            });
        }
    }

    #[test]
    fn ready_tail_is_published_while_an_unrelated_operation_is_still_held() {
        let f = Fixture::new();
        for name in ["first.mp4", "second.mp4", "held.mp4"] { f.file(name); }
        let root = std::fs::File::open(&f.0).unwrap(); let profile = f.profile();
        let hold = Hold::new(); let cancel = AtomicBool::new(false);
        std::thread::scope(|scope| {
            let (sent, received) = std::sync::mpsc::channel();
            let (profile, root, cancel, hold) = (&profile, &root, &cancel, &hold);
            let run = scope.spawn(move || walk_with(&OffFrame::for_test(), profile, root, cancel, &|| true,
                &mut |rows| { sent.send(rows.len()).unwrap(); Ok(()) },
                &|stage, path, _| { if stage == WalkStage::Probe && path == "held.mp4" { hold.block()?; } Ok(()) }));
            let entered = hold.wait_entered(1);
            let first = received.recv_timeout(std::time::Duration::from_secs(2));
            let second = received.recv_timeout(std::time::Duration::from_secs(2));
            let finished = run.is_finished();
            cancel.store(true, Ordering::Release); hold.release();
            let report = run.join().unwrap().unwrap();
            assert!(entered); assert_eq!(first.unwrap(), 1);
            assert_eq!(second.expect("ready tail must flush before the unrelated call drains"), 1);
            assert!(!finished); assert_eq!(report.videos, 2); assert!(report.cancelled);
        });
    }

    #[test]
    fn metadata_saturation_keeps_a_probe_lane_and_drains_before_completion() {
        let f = Fixture::new();
        for name in ["a", "b", "c", "d"] { f.file(&format!("{name}/ready.mp4")); }
        let root = std::fs::File::open(&f.0).unwrap();
        let profile = f.profile();
        let hold = Hold::new();
        let chosen = AtomicBool::new(false);
        let cancel = AtomicBool::new(false);
        std::thread::scope(|scope| {
            let (sent, received) = std::sync::mpsc::channel();
            let (profile, root, cancel, hold, chosen) = (&profile, &root, &cancel, &hold, &chosen);
            let run = scope.spawn(move || walk_with(&OffFrame::for_test(), profile, root, cancel, &|| true,
                &mut |_| Ok(()), &|stage, path, step| {
                    if stage == WalkStage::Next && !path.is_empty() && step == 1 { hold.block()?; }
                    if stage == WalkStage::Probe && path.ends_with("/ready.mp4")
                        && !chosen.swap(true, Ordering::AcqRel) {
                        if !hold.wait_entered(3) { return Err(readonly::ReadError::Io(std::io::ErrorKind::TimedOut)); }
                        sent.send(()).unwrap();
                    }
                    Ok(())
                }));
            let advanced = received.recv_timeout(std::time::Duration::from_secs(4));
            let admitted = hold.entered.load(Ordering::Acquire);
            cancel.store(true, Ordering::Release);
            let drained_early = run.is_finished();
            hold.release();
            let report = run.join().unwrap().unwrap();
            assert!(advanced.is_ok(), "file probe progresses while three metadata operations remain held");
            assert_eq!(admitted, 3, "a fourth metadata operation cannot consume the file lane");
            assert!(!drained_early);
            assert!(report.cancelled);
        });
    }

    #[test]
    fn queued_file_probe_precedes_unrelated_directory_continuation() {
        let f = Fixture::new();
        let root = std::fs::File::open(&f.0).unwrap();
        let mut queue = VecDeque::new(); let mut bytes = 0;
        queue_walk(&mut queue, &mut bytes, 0, WalkJob { path: "later".into(), depth: 1, task: WalkTask::Open }).unwrap();
        queue_walk(&mut queue, &mut bytes, 0, WalkJob { path: "ready.mp4".into(), depth: 0,
            task: WalkTask::Probe(std::sync::Arc::new(root), "ready.mp4".into()) }).unwrap();
        assert!(!queue.front().unwrap().directory());
        assert!(queue.back().unwrap().directory());
        let previous = bytes;
        let oversized = WalkJob { path: "x".repeat(PENDING_PATH_BYTES), depth: 1, task: WalkTask::Open };
        assert_eq!(queue_walk(&mut queue, &mut bytes, 0, oversized), Err("Local pending folder limit reached"));
        assert_eq!(bytes, previous);
        assert_eq!(queue.len(), 2);
    }

    #[test]
    fn cancellation_or_withdrawal_never_finishes_or_publishes_before_raw_drain() {
        for withdraw in [false, true] {
            let f = Fixture::new(); f.file("held.mp4");
            let root = std::fs::File::open(&f.0).unwrap(); let profile = f.profile();
            let hold = Hold::new(); let live = AtomicBool::new(true); let cancel = AtomicBool::new(false);
            let published = std::sync::atomic::AtomicUsize::new(0);
            std::thread::scope(|scope| {
                let run = scope.spawn(|| walk_with(&OffFrame::for_test(), &profile, &root, &cancel,
                    &|| live.load(Ordering::Acquire), &mut |rows| { published.fetch_add(rows.len(), Ordering::Relaxed); Ok(()) },
                    &|stage, path, _| { if stage == WalkStage::Probe && path == "held.mp4" { hold.block()?; } Ok(()) }));
                let entered = hold.wait_entered(1);
                if withdraw { live.store(false, Ordering::Release); } else { cancel.store(true, Ordering::Release); }
                std::thread::sleep(std::time::Duration::from_millis(100));
                let drained_early = run.is_finished();
                hold.release();
                let result = run.join().unwrap();
                assert!(entered); assert!(!drained_early); assert_eq!(published.load(Ordering::Acquire), 0);
                if withdraw { assert!(result.is_err()); } else { assert!(result.unwrap().cancelled); }
            });
        }
    }

    #[test]
    fn read_error_flushes_last_prefix_before_child_warning_or_root_resource_failure() {
        for (child, allocation) in [(false, false), (true, false), (true, true)] {
            let f = Fixture::new();
            let folder = if child { "child/" } else { "" };
            for name in ["first.mp4", "last.mp4"] { f.file(&format!("{folder}{name}")); }
            let root = std::fs::File::open(&f.0).unwrap();
            let mut rows = Vec::new();
            let result = walk_with(&OffFrame::for_test(), &f.profile(), &root, &AtomicBool::new(false),
                &|| true, &mut |batch| { rows.extend(batch); Ok(()) },
                &|stage, relative, step| {
                    if stage == WalkStage::Next && relative == (if child { "child" } else { "" }) && step == 2 {
                        return Err(if allocation { readonly::ReadError::AllocationFailed }
                            else { readonly::ReadError::Io(std::io::ErrorKind::Other) });
                    }
                    Ok(())
                });
            assert_eq!(rows.len(), 2, "the final sub-64 prefix must not disappear");
            if child && !allocation {
                let report = result.unwrap();
                assert_eq!(report.videos, rows.len()); assert_eq!(report.skipped, 1);
                assert!(report.warning().is_some()); assert!(!report.cancelled && !report.depth_limited);
            } else { assert!(result.is_err(), "root/resource failure is incomplete"); }
        }
    }

    #[test]
    fn failed_child_read_does_not_flush_after_cancellation_or_withdrawal() {
        for withdraw in [false, true] {
            let f = Fixture::new(); f.file("child/first.mp4"); f.file("child/buffered.mp4");
            let root = std::fs::File::open(&f.0).unwrap();
            let live = AtomicBool::new(true); let cancel = AtomicBool::new(false); let first = Hold::new();
            let mut rows = Vec::new();
            let result = walk_with(&OffFrame::for_test(), &f.profile(), &root, &cancel,
                &|| live.load(Ordering::Acquire), &mut |batch| { rows.extend(batch); first.release(); Ok(()) },
                &|stage, path, _| {
                    if stage == WalkStage::Probe && path == "child/buffered.mp4" {
                        first.block()?;
                        if withdraw { live.store(false, Ordering::Release); } else { cancel.store(true, Ordering::Release); }
                        return Err(readonly::ReadError::Io(std::io::ErrorKind::Other));
                    }
                    Ok(())
                });
            assert_eq!(rows.len(), 1, "revoked buffered rows must not be emitted");
            if withdraw { assert!(result.is_err()); } else { assert!(result.unwrap().cancelled); }
        }
    }

}

#[cfg(test)]
mod single_file_tests {
    use super::*;
    use crate::app::ott_local_root::LocalSource;
    use crate::app::ott_local_session::{AuthorityRef, BoundSource, SourceKind};
    use plx_base::task::OffFrame;
    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            for n in 0..128 {
                let path = std::env::temp_dir().join(format!("ott-one-row-{}-{n}", std::process::id()));
                match std::fs::create_dir(&path) {
                    Ok(()) => {
                        let path = std::fs::canonicalize(path).unwrap();
                        std::fs::write(path.join("movie.mp4"), b"synthetic fixture").unwrap();
                        std::fs::write(path.join("sibling.mp4"), b"unselected fixture").unwrap();
                        return Self(path);
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
                    Err(e) => panic!("test directory: {e}"),
                }
            }
            panic!("test directory slots exhausted")
        }
        fn profile(&self) -> ProviderProfile {
            ProviderProfile { id: "one-file".into(), name: "Local".into(), kind: ProviderKind::Local,
                source: self.0.join("movie.mp4").to_str().unwrap().into(), source_alias: "Movie".into(), enabled: true }
        }
        fn source(&self) -> LocalSource {
            LocalSource::acquire_authorized(&OffFrame::for_test(), BoundSource {
                id: "one-file".into(), name: "Movie".into(), kind: SourceKind::File, available: true,
                canonical_path: Some(self.profile().source), authority_ref: AuthorityRef(1), generation: 4,
            }, &mut || true).unwrap()
        }
    }
    impl Drop for Fixture { fn drop(&mut self) { plx_base::testscratch::remove_tree(&self.0); } }

    #[test]
    fn selected_file_produces_exactly_one_existing_builder_row() {
        let f = Fixture::new(); let source = f.source(); let mut rows = Vec::new();
        visit_single_file(&OffFrame::for_test(), &f.profile(), &source, &AtomicBool::new(false), &mut |batch| { rows.extend(batch); Ok(()) }).unwrap();
        assert_eq!(rows.len(), 1);
        let expected = ott::catalog_from_local_files(&[LocalFile { relative_path: "movie.mp4".into(), size: 0, duration_ms: None }], &f.profile()).unwrap();
        assert_eq!(rows[0].id, expected.entries[0].id);
        assert!(matches!(&rows[0].media, MediaLocator::Local { relative_path } if relative_path == "movie.mp4"));
        assert!(source.root_handle().is_none());
    }

    #[test]
    fn different_profile_path_cannot_use_the_selected_file_grant() {
        let f = Fixture::new(); let source = f.source(); let mut profile = f.profile();
        profile.source = f.0.join("sibling.mp4").to_str().unwrap().into();
        let mut emitted = false;
        assert!(visit_single_file(&OffFrame::for_test(), &profile, &source, &AtomicBool::new(false), &mut |_| { emitted = true; Ok(()) }).is_err());
        assert!(!emitted);
    }

    #[test]
    fn replaced_or_withdrawn_file_cannot_publish_a_catalogue() {
        let f = Fixture::new(); let mut source = f.source();
        std::fs::rename(&f.profile().source, f.0.join("old.mp4")).unwrap();
        std::fs::write(&f.profile().source, b"replacement").unwrap();
        let mut emitted = false;
        assert!(visit_single_file(&OffFrame::for_test(), &f.profile(), &source, &AtomicBool::new(false), &mut |_| { emitted = true; Ok(()) }).is_err());
        assert!(!emitted);
        source.withdraw();
        assert!(visit_single_file(&OffFrame::for_test(), &f.profile(), &source, &AtomicBool::new(false), &mut |_| { emitted = true; Ok(()) }).is_err());
        assert!(!emitted);
    }

    #[test]
    fn cancellation_before_or_during_emission_is_terminal() {
        let f = Fixture::new(); let source = f.source(); let cancel = AtomicBool::new(true);
        let mut emitted = 0;
        assert!(visit_single_file(&OffFrame::for_test(), &f.profile(), &source, &cancel, &mut |_| { emitted += 1; Ok(()) }).is_err());
        assert_eq!(emitted, 0);
        cancel.store(false, Ordering::Release);
        assert!(visit_single_file(&OffFrame::for_test(), &f.profile(), &source, &cancel, &mut |_| { emitted += 1; cancel.store(true, Ordering::Release); Ok(()) }).is_err());
        assert_eq!(emitted, 1);
    }
}
