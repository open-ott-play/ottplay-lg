//! Automatic restore admission tests. Loaded as a child of ott_usb_runtime.
//! No disk discovery, worker launch, media, or network operations.
use super::*;
use crate::app::ott_local_sources::UsbBinding;

fn fixture(stable: bool) -> Runtime {
    let mut r = Runtime::unloaded(crate::app::ott_companion::Supervisor::inert_for_test());
    r.remote = Remote::disabled_for_test("dev_usb_restore_fixture", "usb");
    r.loaded = true;
    r.resume_source_work();
    r.generation = 7;
    r.provider = ProviderKind::Local;
    r.page = "channels";
    // Retain every shared work slot; test methods never launch an external job.
    r.catalog_done.store(false, Ordering::Release);
    r.metadata_done.store(false, Ordering::Release);
    r.resolver_busy = true;
    let profile = ProviderProfile { id: "local-usb-1".into(), name: "Saved videos".into(),
        source_alias: "Videos".into(), kind: ProviderKind::Local,
        source: "/tmp/usb/old/part/Movies".into(), enabled: true };
    let folders = RememberedFolders::default().with_connected(FolderDescriptor {
        kind: crate::app::ott_local_sources::LocalSourceKind::Folder,
        id: "root".into(), name: "Videos".into(), path: profile.source.clone(),
        usb: stable.then(|| UsbBinding::from_observed("ext4", "fixture-uuid", "/", "Movies").unwrap()),
    }).unwrap();
    folders.put_into(&mut r.saved.settings).unwrap();
    r.saved.profiles.profiles = vec![profile.clone()];
    r.saved.profiles.active_profile = Some(profile.id.clone());
    r.profile_focus = Some("a-different-focus".into());
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
fn begin(r: &mut Runtime) {
    assert!(!r.usb_catalog_allowed());
    assert!(r.local_inventory.usb.automatic && r.local_inventory.usb.waiting);
}
fn take(r: &mut Runtime) {
    assert!(matches!(r.take_usb_task(), Some(ott_catalog::Task::Usb(usb::Task::Restore { .. }))));
}

#[test]
fn automatic_start_uses_saved_binding_without_navigation_or_saved_mutation() {
    let _lock = plx_base::testlock::serial();
    let mut r = fixture(true);
    let saved = serde_json::to_value(&r.saved).unwrap();
    let checkpoints = r.checkpoints.clone();
    let (page, provider, navigation, playback, focus) = (r.page, r.provider, r.navigation_epoch, r.playback_epoch, r.profile_focus.clone());
    begin(&mut r);
    assert_eq!((r.page, r.provider, r.navigation_epoch, r.playback_epoch, r.profile_focus.clone()), (page, provider, navigation, playback, focus));
    assert_eq!(serde_json::to_value(&r.saved).unwrap(), saved);
    assert!(r.checkpoints == checkpoints);
    assert!(!r.saving);
    assert!(r.catalog_next.is_none() && !r.catalog_loading);
    let Some(ott_catalog::Task::Usb(usb::Task::Restore { binding, source })) = r.take_usb_task() else { panic!("restore task"); };
    assert_eq!(source.id, "root");
    assert_eq!(source.generation, r.generation);
    assert_eq!(source.canonical_path.as_deref(), Some("/tmp/usb/old/part/Movies"));
    assert!(!source.available);
    assert_eq!(binding.uuid, "fixture-uuid");
    assert_eq!(binding.filesystem_relative_folder, "Movies");
    assert!(r.take_usb_task().is_none(), "one task per request");
    assert!(!r.catalog_done.load(Ordering::Acquire));
}

#[test]
fn path_only_legacy_profile_never_becomes_an_automatic_grant() {
    let _lock = plx_base::testlock::serial();
    let mut r = fixture(false);
    let generation = r.generation;
    assert!(!r.usb_catalog_allowed());
    assert!(r.local_inventory.usb.pending.is_none());
    assert!(r.catalog_next.is_none() && !r.catalog_loading);
    assert_eq!(r.generation, generation);
    assert!(r.status.contains("identity"));
}

#[test]
fn failed_restore_is_terminal_for_this_catalog_request() {
    let _lock = plx_base::testlock::serial();
    let mut app = app(); let mut r = fixture(true);
    let saved = serde_json::to_value(&r.saved).unwrap();
    begin(&mut r); take(&mut r); let generation = r.generation;
    r.usb_result(&mut app, generation, Err(usb::Error::Unavailable));
    assert!(!r.local_inventory.usb.waiting);
    assert!(r.local_inventory.usb.interrupted.is_none());
    assert!(r.local_inventory.usb.install.is_none());
    for _ in 0..3 {
        assert!(r.take_usb_task().is_none());
        assert!(r.usb_catalog_allowed());
        assert!(r.catalog_next.is_none());
        assert_eq!(r.generation, generation);
    }
    assert_eq!(serde_json::to_value(&r.saved).unwrap(), saved);
    assert!(r.local_inventory.sources.is_empty());
}

#[test]
fn failed_shared_worker_launch_does_not_rearm_automatic_restore() {
    let _lock = plx_base::testlock::serial();
    let mut r = fixture(true); begin(&mut r); take(&mut r);
    r.usb_launch_failed();
    assert!(!r.local_inventory.usb.waiting);
    assert!(r.local_inventory.usb.interrupted.is_none());
    assert!(r.take_usb_task().is_none());
    assert!(r.usb_catalog_allowed());
    assert!(r.catalog_next.is_none());
    assert!(!r.catalog_done.load(Ordering::Acquire), "only the actual worker owns slot completion");
}

#[test]
fn newer_navigation_or_playback_epoch_retires_unlaunched_restore() {
    let _lock = plx_base::testlock::serial();
    for change in 0..2 {
        let mut r = fixture(true); begin(&mut r);
        if change == 0 { r.navigation_epoch = r.navigation_epoch.wrapping_add(1); }
        else { r.playback_epoch = r.playback_epoch.wrapping_add(1); }
        assert!(r.take_usb_task().is_none());
        assert!(!r.local_inventory.usb.waiting);
        assert!(r.local_inventory.usb.interrupted.is_none());
        assert!(!r.catalog_done.load(Ordering::Acquire));
    }
}

#[test]
fn native_ticket_change_retires_restore_even_without_application_epoch_change() {
    let _lock = plx_base::testlock::serial();
    let mut app = app(); let mut r = fixture(true); begin(&mut r);
    let epoch = r.playback_epoch;
    let ticket = r.local_inventory.usb.ticket.unwrap();
    plx_media::route::cancel_play(&mut app.player.session);
    assert!(!plx_media::route::play_request_is_current(ticket));
    assert_eq!(r.playback_epoch, epoch);
    assert!(r.take_usb_task().is_none());
    assert!(!r.local_inventory.usb.waiting);
}

#[test]
fn changed_saved_binding_discards_addressed_completion_before_any_install() {
    let _lock = plx_base::testlock::serial();
    let mut app = app(); let mut r = fixture(true); begin(&mut r); take(&mut r);
    let generation = r.generation;
    let folders = RememberedFolders::read(&r.saved.settings).unwrap();
    let mut next = folders.folders()[0].clone(); next.usb.as_mut().unwrap().uuid = "replacement".into();
    folders.with_reselected(next).unwrap().put_into(&mut r.saved.settings).unwrap();
    let changed = serde_json::to_value(&r.saved).unwrap();
    // This distinct error must not publish because the target no longer matches.
    r.usb_result(&mut app, generation, Err(usb::Error::PermissionDenied));
    assert!(r.local_inventory.usb.error.is_none());
    assert!(!r.local_inventory.usb.waiting);
    assert!(r.local_inventory.usb.install.is_none());
    assert_eq!(serde_json::to_value(&r.saved).unwrap(), changed);
}

#[test]
fn a_nonactive_profile_cannot_reconnect_and_steal_selection() {
    let _lock = plx_base::testlock::serial();
    let mut r = fixture(true);
    let mut other = r.saved.profiles.profiles[0].clone(); other.id = "other-profile".into();
    r.saved.profiles.profiles.push(other.clone());
    r.saved.profiles.active_profile = Some(other.id.clone());
    assert!(!r.usb_catalog_allowed());
    assert!(r.local_inventory.usb.pending.is_none());
    assert_eq!(r.saved.profiles.active_profile.as_deref(), Some(other.id.as_str()));
    assert!(r.catalog_next.is_none());
}
