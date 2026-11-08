//! Real bounded catalogue-worker/visitor results, then real Runtime EOF admission.
//! Only synthetic temporary files are touched; resolver/metadata slots stay held,
//! so these tests neither decode media nor perform network/protected-state IO.
use super::*;
use std::path::Path;

struct Reset;
impl Drop for Reset {
    fn drop(&mut self) {
        plx_media::player::SHARED.reset_session();
        plx_media::player::TX.reset();
        plx_media::route::reset_player_control_for_test(&plx_media::route::PlaybackSession::IDLE);
    }
}

fn start_and_finish_scan(app: &mut App, cancelled: bool) {
    let runtime = app.ott.as_mut().unwrap();
    runtime.resume_source_work();
    runtime.catalog = Catalog::default();
    runtime.catalog_loading = true;
    runtime.catalog_complete = false;
    runtime.catalog_next = None;
    runtime.continuity.stop(runtime.generation);
    runtime.cancel = Arc::new(AtomicBool::new(cancelled));
    let cancel = runtime.cancel.clone();
    struct Cancel(Arc<AtomicBool>, Arc<AtomicBool>);
    impl Drop for Cancel {
        fn drop(&mut self) { if !self.1.load(Ordering::Acquire) { self.0.store(true, Ordering::Release); } }
    }
    let done = runtime.catalog_done.clone();
    // setup() holds an empty catalogue slot to prevent incidental workers.
    // There is no producer behind that placeholder; release it before starting
    // the real worker whose completion flag the rest of this test observes.
    assert!(!done.load(Ordering::Acquire));
    done.store(true, Ordering::Release);
    let _cancel_on_exit = Cancel(cancel.clone(), done.clone());
    assert!(ott_catalog::start(runtime.effective_profile().unwrap().clone(),
        runtime.generation, cancel, runtime.catalog_tx.clone(), done.clone()));
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        runtime_tick(app);
        if done.load(Ordering::Acquire) { break; }
        assert!(std::time::Instant::now() < deadline, "bounded local worker did not finish");
        std::thread::sleep(Duration::from_millis(1));
    }
    // The done flag is published after the terminal event, but the frame drains
    // only four events at a time. This fixture emits fewer than four events.
    runtime_tick(app);
    assert!(done.load(Ordering::Acquire));
}

fn files(root: &crate::app::ott_local_session::BoundSource) {
    let path = Path::new(root.canonical_path.as_deref().unwrap());
    std::fs::write(path.join("first.mp4"), b"synthetic fixture, never decoded").unwrap();
    std::fs::write(path.join("second.mp4"), b"synthetic fixture, never decoded").unwrap();
}

fn scan_with_loss(app: &mut App, root: &crate::app::ott_local_session::BoundSource, whole_root: bool) {
    let path = Path::new(root.canonical_path.as_deref().unwrap());
    for directory in ["First", "Second"] {
        std::fs::create_dir(path.join(directory)).unwrap();
        for name in ["one.mp4", "two.mp4"] {
            std::fs::write(path.join(directory).join(name), b"synthetic fixture, never decoded").unwrap();
        }
    }
    let runtime = app.ott.as_mut().unwrap();
    runtime.resume_source_work();
    runtime.catalog = Catalog::default();
    runtime.catalog_next = None;
    runtime.catalog_loading = true;
    runtime.catalog_complete = false;
    runtime.continuity.stop(runtime.generation);
    let mut events = Vec::new();
    let mut removed = false;
    let mut emit = |rows| {
        events.push(ott_catalog::Event::Batch(runtime.generation, Catalog { hierarchy: None, entries: rows }));
        if whole_root && !removed {
            removed = true;
            std::fs::remove_dir_all(path).unwrap();
        }
        Ok(())
    };
    let terminal = if whole_root {
        ott_catalog::visit_local_for_test(runtime.effective_profile().unwrap(),
            runtime.generation, &runtime.cancel, &mut emit)
    } else {
        // Concurrent workers can already hold a sibling cursor when the first
        // batch arrives. Delete Second at its open boundary, after discovery.
        ott_catalog::visit_local_with_lost_child_for_test(runtime.effective_profile().unwrap(),
            runtime.generation, &runtime.cancel, &mut emit)
    };
    assert!(if whole_root { removed } else { !path.join("Second").exists() });
    events.push(terminal);
    assert!(events.len() <= 4);
    for event in events {
        runtime.catalog_tx.send(event).unwrap_or_else(|_| panic!("fixture receiver disappeared"));
    }
    runtime_tick(app);
}

#[test]
fn exhausted_local_scan_with_skipped_subtree_warns_and_wraps_at_eof() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let (_temp, mut app, root) = local_integration::setup();
    scan_with_loss(&mut app, &root, false);
    let runtime = app.ott.as_ref().unwrap();
    assert!(!runtime.catalog_loading);
    assert!(runtime.catalog_complete, "an exhausted readable tree is terminal despite its warning");
    assert_eq!(runtime.catalog.entries.len(), 2);
    assert!(runtime.status.contains("readable videos remain available"));
    let ids = runtime.ordered_ids();
    assert_eq!(ids.len(), 2, "accepted Local order receives the complete readable list");
    let last = ids.last().unwrap().clone();
    local_integration::install(&mut app, Some(&root), &last);
    let runtime = app.ott.as_mut().unwrap();
    runtime.playing.as_mut().unwrap().entry = runtime.catalog.entries.iter()
        .find(|entry| entry.id == last).unwrap().clone();
    assert!(finish_catalog_playback(&mut app));
    assert_eq!(app.ott.as_ref().unwrap().play_target.as_ref().map(|request| &request.id),
        ids.first(), "real EOF queues the first readable row rather than waiting forever");
}

#[test]
fn missing_local_root_is_not_promoted_to_completed_scan() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let (_temp, mut app, root) = local_integration::setup();
    std::fs::remove_dir_all(root.canonical_path.as_deref().unwrap()).unwrap();
    start_and_finish_scan(&mut app, false);
    let runtime = app.ott.as_ref().unwrap();
    assert!(!runtime.catalog_complete);
    assert!(!runtime.catalog_loading);
    assert!(runtime.catalog.entries.is_empty());
    assert!(runtime.status.contains("unavailable"));
    assert!(runtime.play_target.is_none());
}

#[test]
fn cancelled_local_scan_does_not_publish_completion_or_autoplay() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let (_temp, mut app, root) = local_integration::setup();
    files(&root);
    start_and_finish_scan(&mut app, true);
    let runtime = app.ott.as_ref().unwrap();
    assert!(!runtime.catalog_complete);
    assert!(runtime.catalog.entries.is_empty());
    assert!(runtime.play_target.is_none());
}

#[test]
fn provider_error_is_still_incomplete_and_does_not_gain_wrap_permission() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let (_temp, mut app, _root) = local_integration::setup();
    let runtime = app.ott.as_mut().unwrap();
    runtime.catalog_tx.send(ott_catalog::Event::Finished(runtime.generation,
        Err("Local video limit reached"))).unwrap_or_else(|_| panic!("fixture receiver disappeared"));
    runtime_tick(&mut app);
    let runtime = app.ott.as_ref().unwrap();
    assert!(!runtime.catalog_complete);
    assert_eq!(runtime.status, "Local video limit reached");
}

#[test]
fn completed_warning_never_overrides_a_revoked_source_at_eof() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let (_temp, mut app, root) = local_integration::setup();
    scan_with_loss(&mut app, &root, false);
    // Select the real readable prefix, independent of the warning-completion
    // fix: old code rejects the Failed snapshot and retains the fixture's old
    // Local order. This control must reach revocation on both implementations.
    let entry = {
        let runtime = app.ott.as_ref().unwrap();
        assert_eq!(runtime.catalog.entries.len(), 2);
        runtime.catalog.entries.last().expect("readable scan prefix").clone()
    };
    local_integration::install(&mut app, Some(&root), &entry.id);
    let runtime = app.ott.as_mut().unwrap();
    runtime.playing.as_mut().unwrap().entry = entry;
    assert!(runtime.local_inventory.sources[0].is_live());
    runtime.local_inventory.sources[0].withdraw();
    assert!(!runtime.local_inventory.sources[0].is_live());
    assert!(finish_catalog_playback(&mut app));
    assert!(app.ott.as_ref().unwrap().play_target.is_none());
    assert!(app.ott.as_ref().unwrap().resolver_next.is_none());
}

#[test]
fn depth_cutoff_is_incomplete_even_when_some_videos_were_readable() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let (_temp, mut app, root) = local_integration::setup();
    files(&root);
    let mut deep = std::path::PathBuf::from(root.canonical_path.as_deref().unwrap());
    for _ in 0..65 { deep.push("d"); }
    std::fs::create_dir_all(deep).unwrap();
    start_and_finish_scan(&mut app, false);
    let runtime = app.ott.as_ref().unwrap();
    assert_eq!(runtime.catalog.entries.len(), 2);
    assert!(!runtime.catalog_complete);
    assert!(!runtime.catalog_loading);
    assert_eq!(runtime.status, "Local folder depth limit reached");
    assert!(runtime.play_target.is_none());
}

#[test]
fn root_loss_after_first_batch_is_not_an_exhausted_warning() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let (_temp, mut app, root) = local_integration::setup();
    scan_with_loss(&mut app, &root, true);
    let runtime = app.ott.as_ref().unwrap();
    assert!(!runtime.catalog.entries.is_empty());
    assert!(!runtime.catalog_complete);
    assert!(!runtime.catalog_loading);
    assert_eq!(runtime.status, "Local folder is unavailable");
    assert!(runtime.play_target.is_none());
}

#[test]
fn cancellation_from_the_last_partial_batch_cannot_publish_terminal_success() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let (_temp, app, root) = local_integration::setup();
    files(&root);
    let runtime = app.ott.as_ref().unwrap();
    let cancel = AtomicBool::new(false);
    let mut seen = 0;
    let mut batches = 0;
    let terminal = ott_catalog::visit_local_for_test(runtime.effective_profile().unwrap(),
        runtime.generation, &cancel, &mut |rows| {
            seen += rows.len();
            batches += 1;
            // First-video publication is separate; cancel from the final
            // partial flush, after its pre-emit cancellation observation.
            if seen == 2 { cancel.store(true, Ordering::Release); }
            Ok(())
        });
    assert_eq!(seen, 2);
    assert_eq!(batches, 2);
    assert!(cancel.load(Ordering::Acquire));
    assert!(matches!(terminal, ott_catalog::Event::Finished(generation, Err("Catalogue cancelled"))
        if generation == runtime.generation));
}
