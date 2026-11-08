//! A remembered kiosk root is independent of the permanent active profile.
use super::*;
use crate::app::ott_kiosk_stored::{self as stored, Stored, TargetRef};
use plx_data::ott::favorites::Origin;

fn kiosk_runtime() -> (Runtime, String) {
    let mut r = runtime();
    let old = BoundSource {
        id: "root".into(), name: "Videos".into(), kind: SourceKind::Folder,
        available: false, authority_ref: AuthorityRef(1), generation: r.generation,
        canonical_path: Some("/tmp/usb/old/part/Movies/夏".into()),
    };
    let fingerprint = selection::local_root_fingerprint(&old).unwrap();
    let policy = Stored::local_folder(Origin {
        provider: "local".into(), profile: "Saved videos".into(), subprofile: None,
        source_alias: old.name.clone(),
    }, true, old.id.clone(), fingerprint.clone()).unwrap();
    r.saved.settings.insert(stored::KEY.into(), policy.encode().unwrap());
    let profile = ProviderProfile {
        id: "permanent-tv".into(), name: "Television".into(), source_alias: "TV".into(),
        kind: ProviderKind::M3u, source: "https://media.invalid/channels.m3u".into(), enabled: true,
    };
    r.saved.profiles.profiles = vec![profile];
    r.saved.profiles.active_profile = Some("permanent-tv".into());
    r.saved.resume.clear();
    r.checkpoints.clear();
    r.provider = ProviderKind::M3u;
    r.catalog_next = None;
    r.catalog_loading = false;
    (r, fingerprint)
}

fn run_queued(r: &mut Runtime, directory: &Directory, unavailable: bool)
    -> Result<usb::Answer, usb::Error>
{
    let Some(ott_catalog::Task::Usb(task)) = r.take_usb_task() else { panic!("queued USB task"); };
    assert!(matches!(&task, usb::Task::Restore { .. }));
    let off = OffFrame::for_test();
    let mount = directory.mount(&off);
    usb::execute_with_identity(&off, task, &AtomicBool::new(false),
        &mut |_, _| Ok(vec![mount.clone()]),
        &mut |_, mounts, _| {
            if unavailable { return Err(identity::IdentityError::Unsupported); }
            Ok(mounts.iter().map(|mount| identity::IdentityObservation::for_test(
                mount.clone(), identity::StableIdentity::new("fixture-uuid", mount.filesystem()).unwrap(),
            )).collect())
        })
}

fn stage_kiosk(r: &mut Runtime, directory: &Directory, fingerprint: &str) -> Saved {
    assert!(matches!(r.usb_prepare_kiosk_folder("root", fingerprint), Ok(true)));
    let target = r.local_inventory.usb.target.clone().unwrap();
    assert!(target.kiosk.is_some() && target.previous.is_none());
    let usb::Answer::Restored { source, relocation } = run_queued(r, directory, false).unwrap()
        else { panic!("actual worker relocation receipt"); };
    assert!(relocation.is_current(&source));
    r.local_inventory.usb.waiting = false;
    let mut saved = r.stage_usb_install_with_relocation(target, source, Some(relocation)).unwrap();
    r.usb_rebase_saved(&mut saved).unwrap();
    saved.validate().unwrap();
    saved
}

#[test]
fn remembered_kiosk_root_stages_without_inserting_or_selecting_a_local_profile() {
    let _lock = plx_base::testlock::serial();
    let directory = Directory::new();
    let _app = app(); // establish native ticket before constructing Runtime
    let (mut r, fingerprint) = kiosk_runtime();
    let profiles = r.saved.profiles.clone();
    let before = serde_json::to_value(&r.saved).unwrap();
    let saved = stage_kiosk(&mut r, &directory, &fingerprint);
    assert!(saved.profiles == profiles);
    assert_eq!(saved.profiles.active_profile.as_deref(), Some("permanent-tv"));
    assert_eq!(saved.profiles.profiles.len(), 1);
    assert_eq!(saved.profiles.profiles[0].kind, ProviderKind::M3u);
    assert_eq!(serde_json::to_value(&r.saved).unwrap(), before);
    assert!(r.local_inventory.sources.is_empty());
    let folders = RememberedFolders::read(&saved.settings).unwrap();
    assert_eq!(folders.folders().len(), 1);
    let folder = &folders.folders()[0];
    assert_eq!(folder.id, "root");
    assert_eq!(folder.name, "Videos");
    assert_eq!(folder.path, directory.0.join("Movies/夏").to_str().unwrap());
    let policy = stored::read(&saved.settings).unwrap().unwrap();
    let root = r.local_inventory.usb.install.as_ref().unwrap().source.descriptor();
    assert!(matches!(policy.target(), TargetRef::LocalFolder { source_id, source_fingerprint }
        if source_id == "root" && source_fingerprint == selection::local_root_fingerprint(root).unwrap()));
    assert!(r.catalog_next.is_none());
}

#[test]
fn kiosk_root_saved_commit_keeps_active_m3u_and_does_not_reload_its_catalogue() {
    let _lock = plx_base::testlock::serial();
    let directory = Directory::new();
    let mut a = app();
    let (mut r, fingerprint) = kiosk_runtime();
    let profiles = r.saved.profiles.clone();
    let saved = stage_kiosk(&mut r, &directory, &fingerprint);
    arm_writer(&mut r);
    r.save_navigation = SaveNavigation::Preserve; // root-only save(reload=false)
    let revision = r.catalog_revision;
    let browse = (r.page, r.provider, r.profile_focus.clone());
    // The real root-only producer writes with reload=false; no synthetic profile
    // is admitted simply to obtain a root receipt.
    r.tx.send(Job::Saved(Ok(saved), None, false, ok(json!({"persisted":true}))))
        .unwrap_or_else(|_| panic!("writer completion receiver"));
    a.ott = Some(r);
    let sample = plx_base::task::time::ClockSample::capture_main(a.adapters.player.mt()).unwrap();
    crate::app::ott_runtime::tick(&mut a, sample);
    let r = a.ott.as_ref().unwrap();
    assert!(r.saved.profiles == profiles);
    assert_eq!(r.saved.profiles.active_profile.as_deref(), Some("permanent-tv"));
    assert_eq!((r.page, r.provider, r.profile_focus.clone()), browse);
    assert_eq!(r.local_inventory.sources.len(), 1);
    let root = &r.local_inventory.sources[0];
    assert_eq!(root.descriptor().id, "root");
    assert_eq!(root.descriptor().name, "Videos");
    assert_eq!(root.descriptor().canonical_path.as_deref(), directory.0.join("Movies/夏").to_str());
    assert!(root.root_handle().is_some_and(|handle| handle.is_live()));
    assert!(r.catalog_next.is_none(), "root-only commit must not load the active TV catalogue");
    assert!(r.local_inventory.usb.pending.is_none());
    assert!(r.local_inventory.usb.install.is_none());
    assert!(r.transient.candidate.is_none() && r.play_target.is_none());
    assert_eq!(r.catalog_revision, revision.wrapping_add(1), "only source invalidation, no extra load_catalog");
}

#[test]
fn failed_kiosk_usb_attempt_is_consumed_once_and_obeys_existing_retry_deadline() {
    let _lock = plx_base::testlock::serial();
    let directory = Directory::new();
    let mut a = app();
    let (mut r, _) = kiosk_runtime();
    // Startup is called directly here, before the normal clock-sampling tick.
    r.clock_sample = Some(plx_base::task::time::ClockSample::capture_main(
        a.adapters.player.mt(),
    ).expect("USB kiosk fixture clock"));
    assert!(r.stage_kiosk_startup(&a));
    let now = r.now_ms();
    r.kiosk_recovery_tick_at(&mut a, now);
    assert!(r.local_inventory.usb.waiting);
    let generation = r.generation;
    let result = run_queued(&mut r, &directory, true);
    assert!(matches!(result, Err(usb::Error::Unavailable)));
    r.usb_result(&mut a, generation, result);
    assert!(r.local_inventory.usb.error.is_some());
    assert!(!r.local_inventory.usb.waiting);
    assert!(r.local_inventory.usb.pending.is_none());
    r.kiosk_recovery_tick_at(&mut a, now);
    assert!(r.local_inventory.usb.error.is_none(), "Unavailable is consumed by the existing Startup retry owner");
    assert!(r.local_inventory.usb.pending.is_none());
    assert_eq!(r.generation, generation);
    r.kiosk_recovery_tick_at(&mut a, now + 9_999);
    assert!(r.local_inventory.usb.pending.is_none());
    assert_eq!(r.generation, generation);
    r.kiosk_recovery_tick_at(&mut a, now + 10_000);
    assert!(matches!(r.local_inventory.usb.pending, Some(usb::Task::Restore { .. })));
    assert!(r.local_inventory.usb.waiting);
    assert_ne!(r.generation, generation);
    assert_eq!(r.saved.profiles.active_profile.as_deref(), Some("permanent-tv"));
    assert_eq!(r.saved.profiles.profiles.len(), 1);
    assert!(r.local_inventory.sources.is_empty());
}

#[path = "ott_usb_kiosk_save_failure_tests.rs"]
mod save_failure_tests;
