//! Classification only: no mount probe, file open, worker, or native playback.
use super::*;
use crate::app::ott_local_root::LocalSource;
use crate::app::ott_local_sources::UsbBinding;

fn fixture(kind: LocalSourceKind, stable: bool) -> (Runtime, ProviderProfile, FolderDescriptor) {
    let mut runtime = Runtime::unloaded(crate::app::ott_companion::Supervisor::inert_for_test());
    runtime.remote = Remote::disabled_for_test("dev_usb_file_boundary", "usb");
    runtime.loaded = true;
    runtime.resume_source_work();
    runtime.generation = 7;
    // Keep real shared slots occupied; classification must not release them.
    runtime.catalog_done.store(false, Ordering::Release);
    runtime.metadata_done.store(false, Ordering::Release);
    runtime.resolver_busy = true;
    let profile = ProviderProfile {
        id: "file-profile".into(), name: "Local sample".into(),
        kind: ProviderKind::Local, source: "/tmp/usb/disk/part/Clip.mp4".into(),
        source_alias: "Sample".into(), enabled: true,
    };
    let descriptor = FolderDescriptor {
        id: "source".into(), name: "Sample".into(), path: profile.source.clone(), kind,
        usb: stable.then(|| UsbBinding::from_observed("ext4", "fixture-uuid", "/", "Clip.mp4").unwrap()),
    };
    RememberedFolders::default().with_connected(descriptor.clone()).unwrap()
        .put_into(&mut runtime.saved.settings).unwrap();
    runtime.saved.profiles.profiles = vec![profile.clone()];
    runtime.saved.profiles.active_profile = Some(profile.id.clone());
    runtime.profile_focus = Some(profile.id.clone());
    runtime.saved.category = "Saved category".into();
    runtime.saved.favorites_selected = true;
    runtime.catalog_next = Some(profile.clone());
    runtime.catalog_loading = true;
    (runtime, profile, descriptor)
}

fn unavailable_file(profile: &ProviderProfile, generation: u64) -> LocalSource {
    LocalSource::unavailable(BoundSource {
        id: "source".into(), name: "Sample".into(), kind: SourceKind::File,
        canonical_path: Some(profile.source.clone()), available: false,
        authority_ref: AuthorityRef(1), generation,
    })
}

#[test]
fn saved_usb_file_keeps_ordinary_catalog_request_without_folder_restore() {
    let _lock = plx_base::testlock::serial();
    let (mut runtime, profile, _) = fixture(LocalSourceKind::File, false);
    let saved = serde_json::to_value(&runtime.saved).unwrap();
    let generation = runtime.generation;
    for _ in 0..2 {
        assert!(runtime.usb_catalog_allowed());
        assert!(runtime.catalog_next.as_ref() == Some(&profile));
        assert!(runtime.catalog_loading);
        assert!(runtime.local_inventory.usb.pending.is_none());
        assert!(!runtime.local_inventory.usb.waiting);
        assert!(runtime.local_inventory.sources.is_empty(), "classification grants nothing");
        assert_eq!(runtime.generation, generation);
    }
    assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), saved);
    assert!(!runtime.catalog_done.load(Ordering::Acquire));
}

#[test]
fn explicit_usb_reselection_and_automatic_restore_reject_a_file_target() {
    let _lock = plx_base::testlock::serial();
    let (runtime, profile, _) = fixture(LocalSourceKind::File, false);
    assert!(runtime.usb_target().is_err());
    assert!(runtime.usb_target_for(Some(profile.clone())).is_err());
    assert!(runtime.usb_restore_target(&profile).is_err());
    assert!(runtime.local_inventory.usb.pending.is_none());
}

#[test]
fn unavailable_temporary_file_is_not_reclassified_as_a_usb_folder() {
    let _lock = plx_base::testlock::serial();
    let (mut runtime, profile, _) = fixture(LocalSourceKind::File, false);
    RememberedFolders::default().put_into(&mut runtime.saved.settings).unwrap();
    runtime.local_inventory.install(vec![unavailable_file(&profile, runtime.generation)], runtime.generation);
    assert!(runtime.usb_target_for(Some(profile.clone())).is_err());
    assert!(runtime.usb_catalog_allowed());
    assert!(runtime.catalog_next.as_ref() == Some(&profile));
    assert!(runtime.local_inventory.sources[0].root_handle().is_none());
    assert!(!runtime.local_inventory.sources[0].is_live());
    assert!(runtime.local_inventory.usb.pending.is_none());
}

#[test]
fn a_folder_named_like_a_video_retains_verified_usb_restore() {
    let _lock = plx_base::testlock::serial();
    let (mut runtime, profile, _) = fixture(LocalSourceKind::Folder, true);
    let target = runtime.usb_target_for(Some(profile)).unwrap();
    assert_eq!(target.relative_folder, "Clip.mp4");
    assert!(!runtime.usb_catalog_allowed());
    let Some(ott_catalog::Task::Usb(usb::Task::Restore { source, binding })) = runtime.take_usb_task()
        else { panic!("existing Folder restore must remain available"); };
    assert!(source.kind == SourceKind::Folder);
    assert_eq!(binding.filesystem_relative_folder, "Clip.mp4");
}

#[test]
fn file_has_no_retained_usb_intent_or_usb_kiosk_restore() {
    let _lock = plx_base::testlock::serial();
    let (mut runtime, profile, _) = fixture(LocalSourceKind::File, false);
    let source = unavailable_file(&profile, runtime.generation);
    assert!(runtime.retained_usb_intent(source.descriptor()).is_none());
    assert!(matches!(runtime.usb_prepare_kiosk_folder("source", "irrelevant"), Ok(false)));
    assert!(runtime.local_inventory.usb.pending.is_none());
    assert!(runtime.local_inventory.sources.is_empty());
}

#[test]
fn known_file_kind_cannot_inherit_a_saved_folder_binding_at_the_same_path() {
    let _lock = plx_base::testlock::serial();
    let (mut runtime, profile, _) = fixture(LocalSourceKind::Folder, true);
    runtime.local_inventory.install(vec![unavailable_file(&profile, runtime.generation)], runtime.generation);
    assert!(runtime.usb_target_for(Some(profile.clone())).is_err());
    assert!(runtime.usb_restore_target(&profile).is_err());
    assert!(runtime.usb_catalog_allowed(), "ordinary Local validation must handle the type conflict");
    assert!(runtime.catalog_next.as_ref() == Some(&profile));
    assert!(runtime.local_inventory.usb.pending.is_none());
    assert!(matches!(runtime.usb_prepare_kiosk_folder("source", "irrelevant"), Ok(false)));
}

#[test]
fn first_temporary_file_uses_only_its_current_scanning_profile_for_usb_bypass() {
    let _lock = plx_base::testlock::serial();
    for operation in ["play", "shuffle"] {
        let mut app = crate::app::run::lifecycle_regression_tests::app();
        let (mut runtime, mut staged, _) = fixture(LocalSourceKind::File, false);
        // Direct fixture dispatch supplies the same clock input as the frame runner.
        runtime.clock_sample = Some(plx_base::task::time::ClockSample::capture_main(
            app.adapters.player.mt(),
        ).expect("USB File fixture clock"));
        RememberedFolders::default().put_into(&mut runtime.saved.settings).unwrap();
        let permanent = ProviderProfile { id: "permanent".into(), name: "Permanent".into(),
            kind: ProviderKind::M3u, source: "https://example.invalid/list.m3u".into(),
            source_alias: "Permanent".into(), enabled: true };
        runtime.saved.profiles.profiles = vec![permanent.clone()];
        runtime.saved.profiles.active_profile = Some(permanent.id.clone());
        runtime.catalog_next = None;
        let now = runtime.now_ms();
        let body = serde_json::json!({"commands":[],"server_time":1000,"requests":[{
            "id":"00000000000000000000000000000001", "action":"local_media",
            "params":{"op":operation,"runtime":runtime.remote.runtime(),"source":staged.source},
            "expires_at":1020
        }]}).to_string();
        let invocations = runtime.remote.raw_poll_for_test(&body, now);
        assert_eq!(invocations.len(), 1);
        assert!(commands::command(&mut runtime, &mut app, &invocations[0]).is_none());
        let owner = crate::app::ott_local_session::OwnerToken(runtime.generation);
        let source = unavailable_file(&staged, runtime.generation);
        staged.id = "transient:source".into();
        let Some(transient::Candidate::Local(candidate)) = runtime.transient.candidate.as_mut()
            else { panic!("real command must create the shared Local candidate"); };
        // Inject only the classification boundary. No held grant is fabricated;
        // ordinary catalogue admission must still reject this unavailable file.
        candidate.inventory = vec![source.clone()];
        candidate.phase = transient::CandidatePhase::Scanning {
            source: source.descriptor().clone(), profile: staged.clone(),
            catalog: Catalog::default(), state: crate::app::ott_local_session::ScanState::Scanning,
            proposed: None,
        };
        runtime.catalog_next = Some(staged.clone());
        let saved = serde_json::to_value(&runtime.saved).unwrap();
        assert!(runtime.local_inventory.sources.is_empty());
        assert!(runtime.usb_catalog_allowed(), "first staged File must not enter folder restore");
        assert!(runtime.catalog_next.as_ref() == Some(&staged));
        assert!(runtime.local_inventory.usb.pending.is_none());
        assert!(runtime.local_catalog_task(staged.clone()).is_err(), "USB bypass is not a file grant");
        assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), saved);

        let Some(transient::Candidate::Local(candidate)) = runtime.transient.candidate.as_mut() else { unreachable!() };
        candidate.owner = crate::app::ott_local_session::OwnerToken(owner.0.wrapping_add(1));
        assert!(!runtime.usb_catalog_allowed(), "a stale candidate cannot classify this request");
        let Some(transient::Candidate::Local(candidate)) = runtime.transient.candidate.as_mut() else { unreachable!() };
        candidate.owner = owner;
        staged.id = "different-profile".into();
        runtime.catalog_next = Some(staged);
        assert!(!runtime.usb_catalog_allowed(), "a different profile cannot borrow staged File evidence");
        assert!(runtime.local_inventory.usb.pending.is_none());
    }
}
