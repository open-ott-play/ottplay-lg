//! Real held-root receipts; only mount and UUID observations are synthetic.
use super::*;
use crate::app::ott_local_root::LocalSource;
use crate::app::ott_local_session::{AuthorityRef, BoundSource, SourceKind};
use crate::app::ott_local_sources::{FolderDescriptor, RememberedFolders, UsbBinding};
use crate::app::ott_usb::{self as usb, VerifiedRelocation};
use plx_base::task::OffFrame;
use plx_data::ott::favorites::Origin;
use plx_platform::tv::removable::{identity, MountCandidate};
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::AtomicU64;

static NEXT: AtomicU64 = AtomicU64::new(0);
struct Directory(PathBuf);
impl Drop for Directory {
    fn drop(&mut self) { let _ = fs::remove_dir_all(&self.0); }
}
fn fixture() -> (Directory, App, Runtime, LocalSource, VerifiedRelocation) {
    let app = crate::app::run::lifecycle_regression_tests::app();
    let mut runtime = Runtime::unloaded(crate::app::ott_companion::Supervisor::inert_for_test());
    runtime.loaded = true;
    runtime.resume_source_work();
    runtime.generation = 7;
    runtime.catalog_done.store(false, Ordering::Release);
    runtime.metadata_done.store(false, Ordering::Release);
    runtime.resolver_busy = true;
    let old = BoundSource {
        id: "usb-library".into(), name: "Saved videos".into(), kind: SourceKind::Folder,
        available: false, authority_ref: AuthorityRef(1), generation: 7,
        canonical_path: Some("/tmp/usb/old/part/Movies".into()),
    };
    let binding = UsbBinding::from_observed("ext4", "fixture-uuid", "/", "Movies").unwrap();
    RememberedFolders::default().with_connected(FolderDescriptor {
        kind: crate::app::ott_local_sources::LocalSourceKind::Folder,
        id: old.id.clone(), name: old.name.clone(), path: old.canonical_path.clone().unwrap(),
        usb: Some(binding.clone()),
    }).unwrap().put_into(&mut runtime.saved.settings).unwrap();
    let policy = Stored::local_folder(Origin {
        provider: "local".into(), profile: "Movies".into(), subprofile: None,
        source_alias: old.name.clone(),
    }, true, old.id.clone(), selection::local_root_fingerprint(&old).unwrap()).unwrap();
    runtime.saved.settings.insert(stored::KEY.into(), policy.encode().unwrap());
    assert!(runtime.stage_kiosk_startup(&app));
    if let Some(Recovery::Preparation(Preparation::Startup(startup))) =
        runtime.source_lifecycle.recovery.as_mut()
    { startup.retry_at = Some(123_456); }
    let path = std::env::temp_dir().join(format!("ott-usb-startup-{}-{}",
        std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
    fs::create_dir(&path).unwrap();
    fs::create_dir(path.join("Movies")).unwrap();
    let directory = Directory(fs::canonicalize(path).unwrap());
    let off = OffFrame::for_test();
    let file = fs::File::open(&directory.0).unwrap();
    let device = identity::directory_device(&off, &file).unwrap();
    let mount = MountCandidate::directory_device_for_test(directory.0.to_str().unwrap(), 7, device).unwrap();
    let answer = usb::execute_with_identity(&off, usb::Task::Restore { binding, source: old },
        &AtomicBool::new(false), &mut |_, _| Ok(vec![mount.clone()]),
        &mut |_, mounts, _| Ok(mounts.iter().map(|mount| identity::IdentityObservation::for_test(
            mount.clone(), identity::StableIdentity::new("fixture-uuid", mount.filesystem()).unwrap(),
        )).collect())).unwrap();
    let usb::Answer::Restored { source, relocation } = answer else { panic!("verified relocation"); };
    assert!(relocation.is_current(&source));
    (directory, app, runtime, source, relocation)
}
fn capture(runtime: &Runtime, app: &App, source: &LocalSource, receipt: &VerifiedRelocation) -> UsbStartup {
    runtime.capture_usb_startup(app, receipt, source).expect("current startup and real receipt")
}
fn install(runtime: &mut Runtime, snapshot: &UsbStartup, source: &LocalSource) {
    // Model the synchronous verified Saved boundary without a writer or scan.
    let mut saved = runtime.saved.clone();
    saved.settings.insert(stored::KEY.into(), snapshot.rebased.encode().unwrap());
    runtime.reconcile_kiosk_policy(&saved);
    runtime.saved = saved;
    runtime.local_inventory.install(vec![source.clone()], runtime.generation);
}

#[test]
fn verified_install_restores_same_startup_ticket_and_retry_deadline() {
    let _lock = plx_base::testlock::serial();
    let (_directory, app, mut runtime, source, receipt) = fixture();
    let snapshot = capture(&runtime, &app, &source, &receipt);
    let ticket = snapshot.startup.ticket;
    let policy = snapshot.rebased.clone();
    install(&mut runtime, &snapshot, &source);
    assert!(runtime.source_lifecycle.recovery.is_none(), "ordinary policy reconciliation retired old path");
    assert!(runtime.finish_usb_startup(&app, snapshot));
    let Some(Recovery::Preparation(Preparation::Startup(current))) = runtime.source_lifecycle.recovery.as_ref()
    else { panic!("same startup restored"); };
    assert_eq!(current.ticket, ticket);
    assert_eq!(current.navigation, runtime.navigation_epoch);
    assert_eq!(current.playback, runtime.playback_epoch);
    assert_eq!(current.retry_at, Some(123_456));
    assert!(current.policy == policy);
    assert!(runtime.startup_current(&app, current));
    assert!(runtime.play_target.is_none() && runtime.transient.candidate.is_none());
}

#[test]
fn capture_refuses_a_different_pinned_folder_or_revoked_root() {
    let _lock = plx_base::testlock::serial();
    let (_directory, app, mut runtime, mut source, receipt) = fixture();
    let policy = stored::read(&runtime.saved.settings).unwrap().unwrap();
    let other = Stored::local_folder(policy.origin().clone(), true, "different".into(),
        selection::local_root_fingerprint(receipt.old_root()).unwrap()).unwrap();
    runtime.saved.settings.insert(stored::KEY.into(), other.encode().unwrap());
    assert!(runtime.capture_usb_startup(&app, &receipt, &source).is_none());
    runtime.saved.settings.insert(stored::KEY.into(), policy.encode().unwrap());
    source.withdraw();
    assert!(runtime.capture_usb_startup(&app, &receipt, &source).is_none());
}

#[test]
fn finish_requires_installed_same_live_root_not_just_a_matching_policy() {
    let _lock = plx_base::testlock::serial();
    for withdrawn in [false, true] {
        let (_directory, app, mut runtime, mut source, receipt) = fixture();
        let snapshot = capture(&runtime, &app, &source, &receipt);
        if withdrawn {
            install(&mut runtime, &snapshot, &source);
            source.withdraw();
        } else {
            runtime.saved.settings.insert(stored::KEY.into(), snapshot.rebased.encode().unwrap());
            runtime.source_lifecycle.recovery = None;
        }
        assert!(!runtime.finish_usb_startup(&app, snapshot));
        assert!(runtime.source_lifecycle.recovery.is_none());
    }
}

#[test]
fn newer_navigation_playback_or_native_ticket_cannot_be_rearmed() {
    let _lock = plx_base::testlock::serial();
    for changed in 0..3 {
        let (_directory, mut app, mut runtime, source, receipt) = fixture();
        let snapshot = capture(&runtime, &app, &source, &receipt);
        install(&mut runtime, &snapshot, &source);
        match changed {
            0 => runtime.navigation_epoch = runtime.navigation_epoch.wrapping_add(1),
            1 => runtime.playback_epoch = runtime.playback_epoch.wrapping_add(1),
            _ => plx_media::route::cancel_play(&mut app.player.session),
        }
        assert!(!runtime.finish_usb_startup(&app, snapshot));
        assert!(runtime.source_lifecycle.recovery.is_none());
    }
}

#[test]
fn changed_saved_target_or_strictness_is_not_overwritten() {
    let _lock = plx_base::testlock::serial();
    let (_directory, app, mut runtime, source, receipt) = fixture();
    let snapshot = capture(&runtime, &app, &source, &receipt);
    install(&mut runtime, &snapshot, &source);
    let changed = snapshot.rebased.with_strict(false);
    runtime.saved.settings.insert(stored::KEY.into(), changed.encode().unwrap());
    assert!(!runtime.finish_usb_startup(&app, snapshot));
    assert!(stored::read(&runtime.saved.settings).unwrap().unwrap() == changed);
    assert!(runtime.source_lifecycle.recovery.is_none());
}
