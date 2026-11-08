//! Actual Runtime task capture and catalogue worker over real temporary grants.
//! No decoder, network, mount operation, or protected-state writer is used.
use super::*;
use plx_base::task::{time::Budget, OffFrame};
use std::path::Path;
use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

struct Worker {
    cancel: Arc<AtomicBool>,
    done: Arc<AtomicBool>,
    rx: Receiver<ott_catalog::Event>,
}
impl Worker {
    fn start(task: ott_catalog::Task) -> Self {
        let cancel = Arc::new(AtomicBool::new(false));
        let done = Arc::new(AtomicBool::new(true));
        // Rendezvous makes the first batch observable before any later batch.
        let (tx, rx) = mpsc::sync_channel(0);
        assert!(ott_catalog::start(
            task,
            7,
            cancel.clone(),
            tx,
            done.clone()
        ));
        Self { cancel, done, rx }
    }
    fn next(&self) -> ott_catalog::Event {
        self.rx
            .recv_timeout(Duration::from_secs(5))
            .expect("bounded catalogue event")
    }
}
impl Drop for Worker {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Release);
        let off = OffFrame::for_test();
        let _ = Budget::start(&off, Duration::from_secs(5)).wait(
            &off,
            Duration::from_secs(5),
            Duration::from_millis(1),
            &|| self.done.load(Ordering::Acquire),
        );
    }
}

#[test]
fn queued_folder_task_rejects_replacement_before_the_worker_reads_any_names() {
    let _serial = plx_base::testlock::serial();
    let (_directory, app, root) = local_integration::setup();
    let runtime = app.ott.as_ref().unwrap();
    let path = Path::new(root.canonical_path.as_deref().unwrap());
    std::fs::write(path.join("original.mp4"), b"original fixture").unwrap();
    let profile = runtime.effective_profile().unwrap().clone();
    let task = runtime.local_catalog_task(profile).unwrap();
    // The task has already captured the original grant, before this replacement.
    std::fs::rename(path, path.with_file_name("retained-original")).unwrap();
    std::fs::create_dir(path).unwrap();
    std::fs::write(path.join("replacement.mp4"), b"different source").unwrap();
    let worker = Worker::start(task);
    assert!(
        matches!(worker.next(), ott_catalog::Event::Finished(7, Err(_))),
        "a replaced root must fail before publishing any replacement names"
    );
    assert!(
        runtime.local_inventory.sources[0].is_live(),
        "the worker reports observation loss; it must not revoke runtime authority"
    );
}

#[test]
fn held_folder_task_publishes_first_item_then_bounded_batches_and_actual_completion() {
    let _serial = plx_base::testlock::serial();
    let (_directory, app, root) = local_integration::setup();
    let runtime = app.ott.as_ref().unwrap();
    let path = Path::new(root.canonical_path.as_deref().unwrap());
    std::fs::create_dir(path.join("nested")).unwrap();
    for i in 0..130 {
        std::fs::write(path.join(format!("nested/video-{i}.mp4")), b"synthetic").unwrap();
    }
    let task = runtime
        .local_catalog_task(runtime.effective_profile().unwrap().clone())
        .unwrap();
    let worker = Worker::start(task);
    let ott_catalog::Event::Batch(7, first) = worker.next() else {
        panic!("first progressive batch")
    };
    assert_eq!(first.entries.len(), 1);
    assert!(
        !worker.done.load(Ordering::Acquire),
        "worker still owns the remaining traversal"
    );
    let mut rows = first.entries;
    let mut sizes = vec![1];
    loop {
        match worker.next() {
            ott_catalog::Event::Batch(7, batch) => {
                sizes.push(batch.entries.len());
                rows.extend(batch.entries);
            }
            ott_catalog::Event::Finished(7, Ok(())) => break,
            _ => panic!("unexpected catalogue event"),
        }
    }
    assert_eq!(sizes, [1, 64, 64, 1]);
    assert_eq!(rows.len(), 130);
    let mut ids: Vec<_> = rows.iter().map(|row| row.id.as_str()).collect();
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(ids.len(), 130);
    assert!(rows.iter().all(|row| matches!(&row.media,
        MediaLocator::Local { relative_path } if relative_path.starts_with("nested/"))));
}

#[test]
fn folder_task_refuses_missing_ambiguous_or_withdrawn_grants() {
    let _serial = plx_base::testlock::serial();
    for state in ["missing", "ambiguous", "withdrawn"] {
        let (_directory, mut app, root) = local_integration::setup();
        let runtime = app.ott.as_mut().unwrap();
        let profile = runtime.effective_profile().unwrap().clone();
        match state {
            "missing" => runtime.local_inventory.sources.clear(),
            "ambiguous" => {
                let mut other = root;
                other.id = "second-source".into();
                other.authority_ref = crate::app::ott_local_session::AuthorityRef(2);
                runtime
                    .local_inventory
                    .sources
                    .push(local_integration::acquire_root(other));
            }
            "withdrawn" => runtime.local_inventory.sources[0].withdraw(),
            _ => unreachable!(),
        }
        assert!(
            runtime.local_catalog_task(profile).is_err(),
            "{state} root must not become path-only IO"
        );
    }
}

#[test]
fn http_local_library_retains_its_provider_task() {
    let _serial = plx_base::testlock::serial();
    let (_directory, app, _) = local_integration::setup();
    let runtime = app.ott.as_ref().unwrap();
    let mut profile = runtime.effective_profile().unwrap().clone();
    profile.source = "https://library.invalid/catalog".into();
    assert!(matches!(runtime.local_catalog_task(profile.clone()),
        Ok(ott_catalog::Task::Provider(actual)) if actual == profile));
}
