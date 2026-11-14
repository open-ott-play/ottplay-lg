//! Real held-directory receipts through the staged Saved admission boundary.
//! Mount/UUID observations are synthetic; the actual RootFD and device check run.
use super::*;
use crate::app::ott_local_root::LocalSource;
use crate::app::ott_local_sources::UsbBinding;
use plx_base::task::OffFrame;
use plx_platform::tv::removable::{identity, MountCandidate};
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::AtomicU64;

static NEXT: AtomicU64 = AtomicU64::new(0);
struct Directory(PathBuf);
impl Directory {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("ott-usb-install-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
        fs::create_dir(&path).unwrap();
        fs::create_dir_all(path.join("Movies/夏")).unwrap();
        Self(fs::canonicalize(path).unwrap())
    }
    fn mount(&self, off: &OffFrame) -> MountCandidate {
        let file = fs::File::open(&self.0).unwrap();
        let device = identity::directory_device(off, &file).unwrap();
        MountCandidate::directory_device_for_test(self.0.to_str().unwrap(), 7, device).unwrap()
    }
}
impl Drop for Directory {
    fn drop(&mut self) { let _ = fs::remove_dir_all(&self.0); }
}
fn runtime() -> Box<Runtime> {
    let mut r = Box::new(Runtime::unloaded(crate::app::ott_companion::Supervisor::inert_for_test()));
    r.remote = Remote::disabled_for_test("dev_usb_install_fixture", "usb");
    r.loaded = true;
    r.resume_source_work();
    r.generation = 7;
    r.provider = ProviderKind::Local;
    r.page = "channels";
    r.catalog_done.store(false, Ordering::Release);
    r.metadata_done.store(false, Ordering::Release);
    r.resolver_busy = true;
    let profile = ProviderProfile { id: "local-usb-1".into(), name: "Saved videos".into(),
        source_alias: "Videos".into(), kind: ProviderKind::Local,
        source: "/tmp/usb/old/part/Movies/夏".into(), enabled: true };
    let folders = RememberedFolders::default().with_connected(FolderDescriptor {
        kind: crate::app::ott_local_sources::LocalSourceKind::Folder,
        id: "root".into(), name: "Videos".into(), path: profile.source.clone(),
        usb: Some(UsbBinding::from_observed("ext4", "fixture-uuid", "/", "Movies/夏").unwrap()),
    }).unwrap();
    folders.put_into(&mut r.saved.settings).unwrap();
    r.saved.profiles.profiles = vec![profile.clone()];
    r.saved.profiles.active_profile = Some(profile.id.clone());
    r.profile_focus = Some("keep-focus".into());
    r.saved.category = "Keep category".into();
    r.saved.favorites_selected = true;
    let resume = SavedResume { entry_id: "same-relative-file".into(), position_ms: 12_345,
        source_fingerprint: profile_fingerprint(&profile) };
    r.saved.resume.insert(profile.id.clone(), resume.clone());
    r.checkpoints.insert(profile.id.clone(), resume);
    r.catalog_next = Some(profile);
    r.catalog_loading = true;
    r
}
fn app() -> App { crate::app::run::lifecycle_regression_tests::app() }
fn receive(r: &mut Runtime, directory: &Directory, mutate: impl FnOnce(&mut BoundSource))
    -> (Target, LocalSource, usb::VerifiedRelocation) {
    assert!(!r.usb_catalog_allowed());
    let target = r.local_inventory.usb.target.clone().unwrap();
    let Some(ott_catalog::Task::Usb(usb::Task::Restore { binding, mut source })) = r.take_usb_task() else { panic!("restore task"); };
    mutate(&mut source);
    let off = OffFrame::for_test();
    let mount = directory.mount(&off);
    let result = usb::execute_with_identity(&off, usb::Task::Restore { binding, source },
        &AtomicBool::new(false), &mut |_, _| Ok(vec![mount.clone()]),
        &mut |_, mounts, _| Ok(mounts.iter().map(|mount| {
            identity::IdentityObservation::for_test(mount.clone(),
                identity::StableIdentity::new("fixture-uuid", mount.filesystem()).unwrap())
        }).collect()));
    let usb::Answer::Restored { source, relocation } = result.unwrap() else { panic!("verified restoration"); };
    assert!(relocation.is_current(&source));
    assert!(r.local_inventory.sources.is_empty(), "worker receipt is not publication");
    (target, source, relocation)
}
fn stage(r: &mut Runtime, directory: &Directory) -> Saved {
    let (target, source, relocation) = receive(r, directory, |_| {});
    // The addressed consumer has accepted the worker reply, before writing.
    r.local_inventory.usb.waiting = false;
    let mut saved = r.stage_usb_install_with_relocation(target, source, Some(relocation)).unwrap();
    // Existing writer ordering: merge current checkpoint, then exact USB rebase.
    saved.resume.extend(r.checkpoints.clone());
    r.usb_preserve_checkpoint(&mut saved);
    saved.validate().unwrap();
    saved
}
fn arm_writer(r: &mut Runtime) {
    r.saving = true;
    r.save_navigation = SaveNavigation::Activate(r.navigation_epoch);
    r.save_playback_epoch = r.playback_epoch;
    r.save_ui_owner = None;
}
fn deliver(mut app: App, r: Box<Runtime>, result: Result<Saved, &'static str>) -> App {
    deliver_into(&mut app, r, result);
    app
}
fn deliver_into(app: &mut App, r: Box<Runtime>, result: Result<Saved, &'static str>) {
    r.tx.send(Job::Saved(result, None, true, ok(json!({"persisted":true}))))
        .unwrap_or_else(|_| panic!("writer completion receiver"));
    app.ott = Some(r);
    let sample = plx_base::task::time::ClockSample::capture_main(app.adapters.player.mt()).unwrap();
    crate::app::ott_runtime::tick(app, sample);
}

#[test]
fn real_worker_receipt_stages_without_publishing_source_or_checkpoint() {
    let _lock = plx_base::testlock::serial(); let directory = Directory::new(); let mut r = runtime();
    let before = serde_json::to_value(&r.saved).unwrap(); let checkpoints = r.checkpoints.clone();
    let saved = stage(&mut r, &directory);
    assert_eq!(serde_json::to_value(&r.saved).unwrap(), before);
    assert!(r.checkpoints == checkpoints);
    assert!(r.local_inventory.sources.is_empty() && !r.saving);
    let install = r.local_inventory.usb.install.as_ref().unwrap();
    assert!(install.relocation.as_ref().unwrap().is_current(&install.source));
    assert_eq!(install.source.descriptor().id, "root");
    assert_eq!(install.source.descriptor().name, "Videos");
    assert_eq!(saved.profiles.active_profile.as_deref(), Some("local-usb-1"));
    assert_eq!(saved.resume["local-usb-1"].entry_id, "same-relative-file");
    assert_eq!(saved.resume["local-usb-1"].position_ms, 12_345);
    assert_ne!(saved.resume["local-usb-1"].source_fingerprint, checkpoints["local-usb-1"].source_fingerprint);
}

#[test]
fn verified_saved_completion_installs_exact_source_and_preserves_relative_bookmark() {
    let _lock = plx_base::testlock::serial(); let directory = Directory::new(); let mut r = runtime(); let a = app();
    let saved = stage(&mut r, &directory); arm_writer(&mut r);
    let expected = saved.profiles.profiles[0].source.clone();
    let handle = r.local_inventory.usb.install.as_ref().unwrap().source.root_handle().unwrap().clone();
    let app = deliver(a, r, Ok(saved)); let r = app.ott.as_ref().unwrap();
    let source = r.local_inventory.sources.iter().find(|source| source.descriptor().id == "root").unwrap();
    assert!(Arc::ptr_eq(source.root_handle().unwrap(), &handle));
    assert_eq!(source.descriptor().name, "Videos");
    assert_eq!(source.descriptor().canonical_path.as_deref(), Some(expected.as_str()));
    assert_eq!(expected, directory.0.join("Movies/夏").to_str().unwrap());
    assert_eq!(r.saved.profiles.active_profile.as_deref(), Some("local-usb-1"));
    assert_eq!(r.saved.resume["local-usb-1"].entry_id, "same-relative-file");
    assert_eq!(r.saved.resume["local-usb-1"].position_ms, 12_345);
    assert!(r.saved.resume == r.checkpoints);
    assert_eq!(r.saved.category, "Keep category"); assert!(r.saved.favorites_selected);
    assert_eq!(r.profile_focus.as_deref(), Some("keep-focus"));
    assert!(r.local_inventory.usb.install.is_none());
    assert!(!r.catalog_done.load(Ordering::Acquire), "completion cannot steal an occupied catalogue slot");
}

#[test]
fn failed_saved_completion_drops_proposal_without_grant_or_checkpoint_change() {
    let _lock = plx_base::testlock::serial(); let directory = Directory::new(); let mut r = runtime(); let a = app();
    let before = serde_json::to_value(&r.saved).unwrap(); let checkpoints = r.checkpoints.clone();
    let _saved = stage(&mut r, &directory); arm_writer(&mut r);
    let app = deliver(a, r, Err("Synthetic protected writer failure")); let r = app.ott.as_ref().unwrap();
    assert_eq!(serde_json::to_value(&r.saved).unwrap(), before);
    assert!(r.checkpoints == checkpoints);
    assert!(r.local_inventory.sources.is_empty()); assert!(r.local_inventory.usb.install.is_none());
    assert!(r.local_inventory.usb.pending.is_none()); assert!(r.catalog_next.is_none());
}

#[test]
fn revoked_root_before_saved_completion_never_installs_a_grant() {
    let _lock = plx_base::testlock::serial(); let directory = Directory::new(); let mut r = runtime(); let a = app();
    let saved = stage(&mut r, &directory); arm_writer(&mut r);
    r.local_inventory.usb.install.as_mut().unwrap().source.withdraw();
    assert!(!r.usb_save_ready(&saved));
    let app = deliver(a, r, Ok(saved)); let r = app.ott.as_ref().unwrap();
    assert!(r.local_inventory.sources.is_empty()); assert!(r.local_inventory.usb.install.is_none());
}

#[test]
fn newer_navigation_before_saved_completion_prevents_installation() {
    let _lock = plx_base::testlock::serial(); let directory = Directory::new(); let mut r = runtime(); let a = app();
    let saved = stage(&mut r, &directory); arm_writer(&mut r);
    // Direct navigation dispatch needs the same clock input as a normal tick.
    r.clock_sample = Some(plx_base::task::time::ClockSample::capture_main(
        a.adapters.player.mt(),
    ).expect("USB navigation fixture clock"));
    r.accepted_navigation(false); r.page = "settings";
    assert!(!r.usb_save_ready(&saved));
    let app = deliver(a, r, Ok(saved)); let r = app.ott.as_ref().unwrap();
    assert!(r.local_inventory.sources.is_empty()); assert!(r.local_inventory.usb.install.is_none());
    assert_eq!(r.page, "settings");
}

#[test]
fn newer_playback_epoch_before_saved_completion_prevents_installation() {
    let _lock = plx_base::testlock::serial(); let directory = Directory::new(); let mut r = runtime(); let a = app();
    let saved = stage(&mut r, &directory); arm_writer(&mut r);
    r.playback_epoch = r.playback_epoch.wrapping_add(1);
    assert!(!r.usb_save_ready(&saved));
    let app = deliver(a, r, Ok(saved)); let r = app.ott.as_ref().unwrap();
    assert!(r.local_inventory.sources.is_empty()); assert!(r.local_inventory.usb.install.is_none());
}

#[test]
fn newer_native_ticket_before_saved_completion_prevents_installation() {
    let _lock = plx_base::testlock::serial(); let directory = Directory::new(); let mut r = runtime(); let mut a = app();
    let saved = stage(&mut r, &directory); arm_writer(&mut r);
    plx_media::route::cancel_play(&mut a.player.session);
    assert!(!r.usb_save_ready(&saved));
    let app = deliver(a, r, Ok(saved)); let r = app.ott.as_ref().unwrap();
    assert!(r.local_inventory.sources.is_empty()); assert!(r.local_inventory.usb.install.is_none());
}

#[test]
fn real_receipt_for_changed_source_name_cannot_rebase_the_saved_source() {
    let _lock = plx_base::testlock::serial(); let directory = Directory::new(); let mut r = runtime();
    let (target, source, relocation) = receive(&mut r, &directory, |source| source.name = "Different source".into());
    let before = serde_json::to_value(&r.saved).unwrap();
    assert!(r.stage_usb_install_with_relocation(target, source, Some(relocation)).is_err());
    assert!(r.local_inventory.usb.install.is_none());
    assert_eq!(serde_json::to_value(&r.saved).unwrap(), before);
}

#[test]
fn real_receipt_for_different_authority_cannot_rebase_the_saved_source() {
    let _lock = plx_base::testlock::serial(); let directory = Directory::new(); let mut r = runtime();
    let (target, source, relocation) = receive(&mut r, &directory, |source| source.authority_ref = AuthorityRef(999));
    assert!(r.stage_usb_install_with_relocation(target, source, Some(relocation)).is_err());
    assert!(r.local_inventory.usb.install.is_none());
}

#[path = "ott_usb_restore_commit_tests.rs"]
mod commit_tests;

#[path = "ott_usb_kiosk_install_tests.rs"]
mod kiosk_tests;

#[path = "ott_usb_foreground_tests.rs"]
mod foreground_tests;

#[path = "ott_usb_favorites_invariant_tests.rs"]
mod favorites_invariants;

#[path = "ott_usb_delayed_mount_tests.rs"]
mod delayed_mount_tests;

#[path = "ott_usb_selected_first_tests.rs"]
mod selected_first_tests;
