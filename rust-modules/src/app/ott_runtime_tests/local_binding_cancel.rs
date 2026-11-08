//! Back/reopen through the real Runtime with a held, real binding producer.
//! Only an empty temporary directory is inspected; no media or state IO runs.
use super::*;
use crate::app::ott_local_sources::{FolderDescriptor, LocalSourceKind, RememberedFolders};

#[test]
fn back_during_initial_binding_does_not_rearm_until_reopen_and_worker_retirement() {
    let _serial = plx_base::testlock::serial();
    let temp = local_integration::TempDir::new();
    let path = temp.folder("Saved");
    let mut app = app_fixture();
    let profile = ProviderProfile { id: "saved-local".into(), name: "Saved local".into(),
        kind: ProviderKind::Local, source: path.clone(), source_alias: "Saved".into(), enabled: true };
    let runtime = app.ott.as_mut().unwrap();
    runtime.loaded = true;
    runtime.resume_source_work();
    runtime.generation = 7;
    runtime.saved.profiles.profiles = vec![profile.clone()];
    runtime.saved.profiles.active_profile = Some(profile.id.clone());
    RememberedFolders::read(&runtime.saved.settings).unwrap()
        .with_connected(FolderDescriptor { id: "saved-root".into(), name: "Saved".into(),
            path, kind: LocalSourceKind::Folder, usb: None }).unwrap()
        .put_into(&mut runtime.saved.settings).unwrap();
    runtime.provider = ProviderKind::Local;
    runtime.page = "channels";
    runtime.resolver_busy = true;
    runtime.metadata_done.store(false, Ordering::Release);
    let initial_generation = runtime.generation;
    let Some(ott_catalog::Task::LocalBindings(request)) = transient::take_binding_task(runtime) else {
        panic!("initial saved root must enter the existing binding worker");
    };
    let (ready_tx, ready_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let cancel = runtime.cancel.clone();
    let tx = runtime.catalog_tx.clone();
    let done = runtime.catalog_done.clone();
    assert!(crate::app::ott_worker::spawn("ott-binding-cancel-test", done.clone(), move |off| {
        let result = ott_catalog::validate_local_bindings_for_test(off, request, &cancel);
        assert!(result.is_ok(), "real initial directory validation");
        ready_tx.send(()).unwrap();
        // Hold an already produced reply. This is the same stale-result race
        // as a reply queued immediately before cancellation, without sleeps.
        if release_rx.recv_timeout(Duration::from_secs(5)).is_ok() {
            let _ = tx.send(ott_catalog::Event::LocalBindings(initial_generation, result));
        }
    }));
    ready_rx.recv_timeout(Duration::from_secs(5)).expect("binding producer reached held reply");
    assert!(!done.load(Ordering::Acquire));
    let mut runtime = app.ott.take().unwrap();
    handle_action(&mut runtime, &mut app, OttAction::Back).unwrap();
    assert_eq!(runtime.page, "profiles");
    assert!(runtime.cancel.load(Ordering::Acquire));
    assert!(matches!(runtime.local_inventory.phase, transient::InventoryPhase::Unloaded));
    assert!(transient::take_binding_task(&mut runtime).is_none(), "Back must not silently restart validation");
    let after_back = runtime.generation;
    runtime.load_catalog();
    assert!(runtime.generation > after_back);
    assert!(matches!(runtime.local_inventory.phase, transient::InventoryPhase::Unloaded));
    assert!(!done.load(Ordering::Acquire), "reopen retains the old producer's occupied slot");
    assert_eq!(runtime.catalog_next.as_ref(), Some(&profile));
    app.ott = Some(runtime);
    release_tx.send(()).unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        runtime_tick(&mut app);
        let runtime = app.ott.as_ref().unwrap();
        if runtime.catalog_complete && done.load(Ordering::Acquire) { break; }
        assert!(std::time::Instant::now() < deadline, "reopened source never completed validation and empty scan");
        std::thread::sleep(Duration::from_millis(1));
    }
    let runtime = app.ott.as_ref().unwrap();
    assert!(matches!(runtime.local_inventory.phase, transient::InventoryPhase::Ready(generation)
        if generation == runtime.generation));
    assert_eq!(runtime.local_inventory.sources.len(), 1);
    let source = &runtime.local_inventory.sources[0];
    assert!(source.is_live());
    assert_eq!(source.descriptor().generation, runtime.generation);
    assert_ne!(source.descriptor().generation, initial_generation, "late old grant was not installed");
    assert!(runtime.play_target.is_none());
}

#[test]
fn back_does_not_discard_a_completed_live_inventory() {
    let _serial = plx_base::testlock::serial();
    let (_temp, mut app, _) = local_integration::setup();
    let mut runtime = app.ott.take().unwrap();
    let before = runtime.local_inventory.sources[0].clone();
    // Exercise inner catalogue Back's cancellation, not an unmounted provider-root exit.
    runtime.page = "channels";
    handle_action(&mut runtime, &mut app, OttAction::Back).unwrap();
    assert_eq!(runtime.page, "profiles");
    assert!(matches!(runtime.local_inventory.phase, transient::InventoryPhase::Ready(_)));
    assert!(runtime.local_inventory.sources[0].is_live());
    assert!(runtime.local_inventory.sources[0].same_grant(&before));
    app.ott = Some(runtime);
}
