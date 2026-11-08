use super::*;
use crate::app::ott_local_root::LocalSource;
use plx_base::task::OffFrame;
use plx_platform::tv::removable::MountCandidate;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::AtomicU64;

static NEXT: AtomicU64 = AtomicU64::new(0);
struct Directory(PathBuf);
impl Directory {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "ott-usb-runtime-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(fs::canonicalize(path).unwrap())
    }
    fn selected(&self, id: &str, generation: u64, relative: &str) -> LocalSource {
        if !relative.is_empty() {
            fs::create_dir_all(self.0.join(relative)).unwrap();
        }
        let mount = MountCandidate::directory_for_test(self.0.to_str().unwrap(), 8).unwrap();
        let mut observe = |_: &OffFrame, _: &AtomicBool| Ok(vec![mount.clone()]);
        let usb::Answer::Choices { choices, .. } = usb::execute_with(
            &OffFrame::for_test(),
            usb::Task::Discover { known: vec![] },
            &AtomicBool::new(false),
            &mut observe,
        )
        .unwrap() else {
            panic!("choices")
        };
        let source = BoundSource {
            id: id.into(),
            name: "Videos".into(),
            kind: SourceKind::Folder,
            available: false,
            canonical_path: None,
            authority_ref: AuthorityRef(1),
            generation,
        };
        let usb::Answer::Selected(source) = usb::execute_with(
            &OffFrame::for_test(),
            usb::Task::Select {
                choice: choices.into_iter().next().unwrap(),
                source,
                relative_folder: relative.into(),
            },
            &AtomicBool::new(false),
            &mut observe,
        )
        .unwrap() else {
            panic!("source")
        };
        source
    }
}
impl Drop for Directory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn runtime() -> Runtime {
    let mut value = Runtime::unloaded(crate::app::ott_companion::Supervisor::inert_for_test());
    value.remote = Remote::disabled_for_test("dev_usb_fixture", "usb");
    value.loaded = true;
    value.resume_source_work();
    assert!(value.source_work_allowed());
    value.generation = 7;
    value.provider = ProviderKind::Local;
    value.page = "profiles";
    // Hold the real shared slot: these tests deliver addressed worker results.
    value.catalog_done.store(false, Ordering::Release);
    value.metadata_done.store(false, Ordering::Release);
    value.resolver_busy = true;
    value
}
fn app() -> App {
    crate::app::run::lifecycle_regression_tests::app()
}
fn profile() -> ProviderProfile {
    ProviderProfile {
        id: "local-usb-1".into(),
        name: "Saved videos".into(),
        kind: ProviderKind::Local,
        source: "/tmp/usb/old/part/Movies/夏".into(),
        source_alias: "Videos".into(),
        enabled: true,
    }
}
fn configure(runtime: &mut Runtime) {
    let profile = profile();
    let folders = RememberedFolders::default()
        .with_connected(FolderDescriptor {
            kind: crate::app::ott_local_sources::LocalSourceKind::Folder,
            usb: None,
            id: "root".into(),
            name: "Videos".into(),
            path: profile.source.clone(),
        })
        .unwrap();
    folders.put_into(&mut runtime.saved.settings).unwrap();
    runtime.saved.profiles.profiles = vec![profile.clone()];
    runtime.saved.profiles.active_profile = Some(profile.id.clone());
    runtime.profile_focus = Some(profile.id.clone());
    let resume = SavedResume {
        entry_id: "same-relative-file".into(),
        position_ms: 12_345,
        source_fingerprint: profile_fingerprint(&profile),
    };
    runtime
        .saved
        .resume
        .insert(profile.id.clone(), resume.clone());
    runtime.checkpoints.insert(profile.id, resume);
    runtime.saved.category = "Keep category".into();
    runtime.saved.favorites_selected = true;
}
fn mark_current(runtime: &mut Runtime, target: Target) {
    runtime.local_inventory.usb = State {
        generation: runtime.generation,
        navigation: runtime.navigation_epoch,
        target: Some(target),
        ..Default::default()
    };
}

#[test]
fn queued_navigation_cancellation_releases_usb_ui_without_new_worker() {
    let _lock = plx_base::testlock::serial();
    let app = app();
    let mut r = runtime();
    // Starting a manual USB scan also dispatches navigation, so sample first.
    r.clock_sample = Some(
        plx_base::task::time::ClockSample::capture_main(app.adapters.player.mt())
            .expect("USB navigation fixture clock"),
    );
    let target = r.usb_target().unwrap();
    r.usb_begin(usb::Task::Discover { known: vec![] }, target, false)
        .unwrap();
    assert!(r.local_inventory.usb.waiting);
    r.accepted_navigation(false);
    assert!(r.take_usb_task().is_none());
    assert!(!r.local_inventory.usb.waiting);
    assert!(r.usb_rows()[0].enabled);
    assert!(
        !r.catalog_done.load(Ordering::Acquire),
        "slot remains owned by its real worker"
    );
}

#[test]
fn late_navigation_completion_cannot_publish_or_leave_refresh_disabled() {
    let _lock = plx_base::testlock::serial();
    let mut app = app();
    let mut r = runtime();
    // Starting a manual USB scan also dispatches navigation, so sample first.
    r.clock_sample = Some(
        plx_base::task::time::ClockSample::capture_main(app.adapters.player.mt())
            .expect("USB navigation fixture clock"),
    );
    let target = r.usb_target().unwrap();
    r.usb_begin(usb::Task::Discover { known: vec![] }, target, false)
        .unwrap();
    let generation = r.generation;
    r.accepted_navigation(false);
    r.usb_result(
        &mut app,
        generation,
        Ok(usb::Answer::Choices {
            choices: vec![],
            observed: vec![],
        }),
    );
    assert!(!r.local_inventory.usb.waiting);
    assert!(r.usb_rows()[0].enabled);
}

#[test]
fn late_old_generation_observation_cannot_revoke_current_root() {
    let _lock = plx_base::testlock::serial();
    let mut app = app();
    let f = Directory::new();
    let mut r = runtime();
    let source = f.selected("root", r.generation, "");
    let handle = source.root_handle().unwrap().clone();
    r.local_inventory
        .install(vec![source.clone()], r.generation);
    let target = r.usb_target().unwrap();
    mark_current(&mut r, target);
    r.local_inventory.usb.waiting = true;
    let old = r.generation;
    r.generation += 1;
    r.usb_result(
        &mut app,
        old,
        Ok(usb::Answer::Choices {
            choices: vec![],
            observed: vec![LocalSource::unavailable(source.descriptor().clone())],
        }),
    );
    assert!(handle.is_live());
    assert!(r.local_inventory.sources[0].root_handle().is_some());
}

#[test]
fn synchronous_save_refusal_preserves_old_root_saved_and_checkpoint() {
    let _lock = plx_base::testlock::serial();
    let mut app = app();
    let old = Directory::new();
    let next = Directory::new();
    let mut r = runtime();
    configure(&mut r);
    let source = old.selected("root", r.generation, "Movies/夏");
    let handle = source.root_handle().unwrap().clone();
    r.local_inventory.install(vec![source], r.generation);
    let target = r.usb_target().unwrap();
    mark_current(&mut r, target.clone());
    let before = serde_json::to_value(&r.saved).unwrap();
    let checkpoints = r.checkpoints.clone();
    r.saving = true;
    assert!(r
        .usb_install(
            &mut app,
            target,
            next.selected("root", r.generation, "Movies/夏")
        )
        .is_err());
    assert!(handle.is_live());
    assert!(r.checkpoints == checkpoints);
    assert_eq!(serde_json::to_value(&r.saved).unwrap(), before);
    assert!(r.local_inventory.usb.install.is_none());
}

#[test]
fn failed_async_save_discards_proposal_without_withdrawing_current_source() {
    let _lock = plx_base::testlock::serial();
    let old = Directory::new();
    let next = Directory::new();
    let mut r = runtime();
    configure(&mut r);
    let source = old.selected("root", r.generation, "Movies/夏");
    let handle = source.root_handle().unwrap().clone();
    r.local_inventory.install(vec![source], r.generation);
    let target = r.usb_target().unwrap();
    mark_current(&mut r, target.clone());
    let before = serde_json::to_value(&r.saved).unwrap();
    let checkpoints = r.checkpoints.clone();
    r.stage_usb_install(target, next.selected("root", r.generation, "Movies/夏"))
        .unwrap();
    r.usb_save_finished(false);
    assert!(handle.is_live());
    assert!(r.checkpoints == checkpoints);
    assert_eq!(serde_json::to_value(&r.saved).unwrap(), before);
}

#[test]
fn verified_reselection_preserves_ids_relative_bookmark_category_and_favorites() {
    let _lock = plx_base::testlock::serial();
    let next = Directory::new();
    let mut r = runtime();
    configure(&mut r);
    let target = r.usb_target().unwrap();
    assert_eq!(target.relative_folder, "Movies/夏");
    assert_eq!(target.source_id, "root");
    mark_current(&mut r, target.clone());
    let mut saved = r
        .stage_usb_install(target, next.selected("root", r.generation, "Movies/夏"))
        .unwrap();
    // Exactly the existing writer's checkpoint merge, then this operation's
    // addressed rebase. No live checkpoint changes before the verified reply.
    saved.resume.extend(r.checkpoints.clone());
    r.usb_preserve_checkpoint(&mut saved);
    assert_ne!(
        saved.resume["local-usb-1"].source_fingerprint,
        r.checkpoints["local-usb-1"].source_fingerprint
    );
    assert!(r.usb_save_ready(&saved));
    r.saved = saved;
    r.usb_save_finished(true);
    assert_eq!(r.saved.profiles.profiles[0].id, "local-usb-1");
    assert_eq!(r.saved.resume["local-usb-1"].position_ms, 12_345);
    assert_eq!(r.saved.resume["local-usb-1"].entry_id, "same-relative-file");
    assert!(r.checkpoints == r.saved.resume);
    assert_eq!(r.saved.category, "Keep category");
    assert!(r.saved.favorites_selected);
    assert!(r.local_inventory.sources[0].root_handle().is_some());
}

#[test]
fn navigation_before_saved_does_not_install_selected_root() {
    let _lock = plx_base::testlock::serial();
    let next = Directory::new();
    let app = app();
    let mut r = runtime();
    configure(&mut r);
    let target = r.usb_target().unwrap();
    mark_current(&mut r, target.clone());
    let saved = r
        .stage_usb_install(target, next.selected("root", r.generation, "Movies/夏"))
        .unwrap();
    // Direct navigation dispatch supplies the frame runner's clock input.
    r.clock_sample = Some(
        plx_base::task::time::ClockSample::capture_main(app.adapters.player.mt())
            .expect("USB navigation fixture clock"),
    );
    r.accepted_navigation(false);
    assert!(!r.usb_save_ready(&saved));
    r.saved = saved;
    r.usb_save_finished(false);
    assert!(r.local_inventory.sources.is_empty());
}

#[test]
fn cold_boot_remembered_usb_path_cannot_start_catalogue_io() {
    let _lock = plx_base::testlock::serial();
    let mut r = runtime();
    configure(&mut r);
    r.catalog_next = Some(profile());
    r.catalog_loading = true;
    assert!(!r.usb_catalog_allowed());
    assert!(r.catalog_next.is_none());
    assert!(!r.catalog_loading);
    assert!(r.status.contains("Select the USB drive again"));
}

#[test]
fn source_id_search_includes_deleted_profiles_retained_roots() {
    let _lock = plx_base::testlock::serial();
    let mut r = runtime();
    let folders = RememberedFolders::default()
        .with_connected(FolderDescriptor {
            kind: crate::app::ott_local_sources::LocalSourceKind::Folder,
            usb: None,
            id: "local-usb-1".into(),
            name: "Earlier USB".into(),
            path: "/tmp/usb/old/part".into(),
        })
        .unwrap();
    folders.put_into(&mut r.saved.settings).unwrap();
    assert!(r.saved.profiles.profiles.is_empty());
    assert_eq!(r.usb_target().unwrap().profile.id, "local-usb-2");
}
