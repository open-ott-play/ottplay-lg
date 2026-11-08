//! Real temporary file capabilities at parental and lifecycle boundaries.
//! No media decoder, catalog worker, network request or Saved writer runs.
use super::*;
use std::path::Path;

fn setup_file() -> (fixture::TempDir, Box<App>, BoundSource) {
    let (guard, mut app, mut source) = fixture::setup();
    let path = Path::new(source.canonical_path.as_deref().unwrap()).join("clip.mp4");
    std::fs::write(&path, b"temporary file authority fixture").unwrap();
    let parent = path.parent().unwrap();
    std::fs::write(parent.join("other.mp4"), b"unrelated video").unwrap();
    std::fs::write(parent.join("clip.srt"), b"ungranted subtitle").unwrap();
    std::fs::create_dir(parent.join(".ottplay")).unwrap();
    std::fs::write(parent.join(".ottplay/metadata.json"), b"{}").unwrap();
    source.name = "clip.mp4".into();
    source.kind = local::SourceKind::File;
    source.canonical_path = Some(path.to_str().unwrap().into());
    let held = LocalSource::acquire_authorized(&OffFrame::for_test(), source.clone(), &mut || true)
        .unwrap();
    assert!(held.is_live());
    assert!(held.root_handle().is_none());
    let runtime = app.ott.as_mut().unwrap();
    runtime.local_inventory.install(vec![held], 7);
    accept_local(runtime, &source, 7);
    (guard, app, source)
}

#[test]
fn single_file_parental_authority_has_no_directory_or_sibling_grant() {
    let _serial = plx_base::testlock::serial();
    let (_guard, app, source) = setup_file();
    let runtime = app.ott.as_ref().unwrap();
    let Authority::Local(read) = capture(runtime).0 else {
        panic!("Local authority");
    };
    assert!(read.is_live());
    assert!(read.folder_read().is_none());
    assert!(read.source.root_handle().is_none());
    assert!(read.source.single_file("clip.mp4").is_some());
    for sibling in [
        "other.mp4",
        "../clip.mp4",
        "clip.srt",
        ".ottplay/metadata.json",
    ] {
        assert!(read.source.single_file(sibling).is_none(), "{sibling}");
    }
    let selection = runtime.bind_local_selection(&source).unwrap();
    assert!(runtime.capture_local_source(&selection).is_some());
    assert!(runtime.capture_local_root(&selection).is_none());
}

#[test]
fn single_file_inventory_refresh_keeps_parental_authority_but_reopen_does_not() {
    let _serial = plx_base::testlock::serial();
    let (_guard, mut app, source) = setup_file();
    let runtime = app.ott.as_mut().unwrap();
    let first = capture(runtime);
    runtime
        .local_inventory
        .install(runtime.local_inventory.sources.clone(), 8);
    assert!(first.same_authority(&capture(runtime)));
    let reopened =
        LocalSource::acquire_authorized(&OffFrame::for_test(), source.clone(), &mut || true)
            .unwrap();
    runtime.local_inventory.install(vec![reopened], 9);
    assert!(
        parental_source_authority(runtime).is_err(),
        "accepted old file cannot rebind by path"
    );
    assert!(
        first.same_authority(&first.clone()),
        "identity equality survives withdrawal"
    );
    accept_local(runtime, &source, 9);
    assert!(!first.same_authority(&capture(runtime)));
}

#[test]
fn single_file_loss_cannot_fall_back_to_a_parent_directory_authority() {
    let _serial = plx_base::testlock::serial();
    let (_guard, mut app, source) = setup_file();
    let runtime = app.ott.as_mut().unwrap();
    let first = capture(runtime);
    let mut parent = source.clone();
    parent.kind = local::SourceKind::Folder;
    parent.canonical_path = Some(
        Path::new(source.canonical_path.as_deref().unwrap())
            .parent()
            .unwrap()
            .to_str()
            .unwrap()
            .into(),
    );
    let held = LocalSource::acquire_authorized(&OffFrame::for_test(), parent.clone(), &mut || true)
        .unwrap();
    runtime.local_inventory.install(vec![held], 8);
    assert!(parental_source_authority(runtime).is_err());
    accept_local(runtime, &parent, 8);
    assert!(!first.same_authority(&capture(runtime)));
}

#[test]
fn single_file_lifecycle_requires_ready_inventory_and_the_same_live_capability() {
    let _serial = plx_base::testlock::serial();
    let (_guard, mut app, source) = setup_file();
    let runtime = app.ott.as_mut().unwrap();
    let profile = source_profile(&source).unwrap();
    let selection = runtime.bind_local_selection(&source).unwrap();
    assert!(lifecycle_target_ready(runtime, &profile, Some(&selection)));
    assert!(lifecycle_target_ready(runtime, &profile, None));
    runtime.local_inventory.phase = InventoryPhase::Loading(8);
    assert!(!lifecycle_target_ready(runtime, &profile, Some(&selection)));
    assert!(!lifecycle_target_ready(runtime, &profile, None));
    runtime.local_inventory.phase = InventoryPhase::Ready(8);
    runtime.local_inventory.sources[0].withdraw();
    assert!(!lifecycle_target_ready(runtime, &profile, Some(&selection)));
    assert!(!lifecycle_target_ready(runtime, &profile, None));
    assert!(parental_source_authority(runtime).is_err());
}

#[test]
fn single_file_resume_rebinds_receipt_only_after_unchanged_grant_revalidation() {
    let _serial = plx_base::testlock::serial();
    let (_guard, mut app, source) = setup_file();
    let runtime = app.ott.as_mut().unwrap();
    let selection = runtime.bind_local_selection(&source).unwrap();
    let original = runtime.capture_local_source(&selection).unwrap();
    let mut request = PlaybackRequest::ordinary("clip.mp4", 12_000);
    request.local_selection = Some(selection.clone());
    runtime.resume_pending = Some(request);
    runtime.local_inventory.phase = InventoryPhase::Loading(8);
    assert_eq!(
        lifecycle_resume_ready(runtime, runtime.resume_pending.as_ref().unwrap()),
        Ok(false)
    );
    rebind_lifecycle_resume(runtime);
    assert!(
        runtime
            .resume_pending
            .as_ref()
            .unwrap()
            .local_selection
            .as_ref()
            == Some(&selection)
    );
    runtime
        .local_inventory
        .install(runtime.local_inventory.sources.clone(), 8);
    rebind_lifecycle_resume(runtime);
    let resumed = runtime.resume_pending.as_ref().unwrap();
    let rebound = resumed.local_selection.as_ref().unwrap();
    assert!(*rebound != selection);
    assert!(
        runtime
            .capture_local_source(rebound)
            .unwrap()
            .same_grant(&original)
    );
    assert_eq!(lifecycle_resume_ready(runtime, resumed), Ok(true));
    assert_eq!(resumed.position_ms, 12_000);
}

#[test]
fn withdrawn_single_file_cannot_resume_or_supply_a_parental_authority() {
    let _serial = plx_base::testlock::serial();
    let (_guard, mut app, source) = setup_file();
    let runtime = app.ott.as_mut().unwrap();
    let selection = runtime.bind_local_selection(&source).unwrap();
    let mut request = PlaybackRequest::ordinary("clip.mp4", 0);
    request.local_selection = Some(selection.clone());
    runtime.resume_pending = Some(request);
    runtime.local_inventory.sources[0].withdraw();
    rebind_lifecycle_resume(runtime);
    assert!(
        runtime
            .resume_pending
            .as_ref()
            .unwrap()
            .local_selection
            .as_ref()
            == Some(&selection)
    );
    assert!(lifecycle_resume_ready(runtime, runtime.resume_pending.as_ref().unwrap()).is_err());
    assert!(parental_source_authority(runtime).is_err());
    assert!(runtime.capture_local_source(&selection).is_none());
}

#[test]
fn reopened_single_file_cannot_rebind_or_resume_the_previous_session_grant() {
    let _serial = plx_base::testlock::serial();
    let (_guard, mut app, source) = setup_file();
    let runtime = app.ott.as_mut().unwrap();
    let selection = runtime.bind_local_selection(&source).unwrap();
    let mut request = PlaybackRequest::ordinary("clip.mp4", 12_000);
    request.local_selection = Some(selection.clone());
    runtime.resume_pending = Some(request);
    let reopened =
        LocalSource::acquire_authorized(&OffFrame::for_test(), source.clone(), &mut || true)
            .unwrap();
    runtime.local_inventory.install(vec![reopened], 8);
    rebind_lifecycle_resume(runtime);
    assert_eq!(
        runtime
            .resume_pending
            .as_ref()
            .unwrap()
            .local_selection
            .as_ref(),
        Some(&selection)
    );
    assert!(lifecycle_resume_ready(runtime, runtime.resume_pending.as_ref().unwrap()).is_err());
    // Readiness itself must reject a newly supplied receipt as well. Neither
    // parental policy nor a prior call to the rebinder is the source fence.
    let current = runtime.bind_local_selection(&source).unwrap();
    let mut direct = runtime.resume_pending.as_ref().unwrap().clone();
    direct.local_selection = Some(current);
    assert!(lifecycle_resume_ready(runtime, &direct).is_err());
}

#[test]
fn pending_single_file_without_a_retained_witness_cannot_resume() {
    let _serial = plx_base::testlock::serial();
    let (_guard, mut app, source) = setup_file();
    let runtime = app.ott.as_mut().unwrap();
    let selection = runtime.bind_local_selection(&source).unwrap();
    let mut request = PlaybackRequest::ordinary("clip.mp4", 12_000);
    request.local_selection = Some(selection.clone());
    runtime.resume_pending = Some(request);
    runtime.transient.accepted = None;
    assert!(runtime.playing.is_none());
    runtime
        .local_inventory
        .install(runtime.local_inventory.sources.clone(), 8);
    rebind_lifecycle_resume(runtime);
    assert_eq!(
        runtime
            .resume_pending
            .as_ref()
            .unwrap()
            .local_selection
            .as_ref(),
        Some(&selection)
    );
    let mut direct = runtime.resume_pending.as_ref().unwrap().clone();
    direct.local_selection = Some(runtime.bind_local_selection(&source).unwrap());
    assert!(lifecycle_resume_ready(runtime, &direct).is_err());
}

struct FileRouteReset;
impl Drop for FileRouteReset {
    fn drop(&mut self) {
        plx_media::player::SHARED.reset_session();
        plx_media::player::TX.reset();
        plx_media::route::reset_player_control_for_test(&plx_media::route::PlaybackSession::IDLE);
    }
}

/// Commit a real route identity with the held file; no decoder or IO worker runs.
fn install_file_favorite(app: &mut App, source: &BoundSource) -> PlaybackRequest {
    let runtime = app.ott.as_ref().unwrap();
    let selection = runtime.bind_local_selection(source).unwrap();
    let read = runtime.capture_local_source(&selection).unwrap();
    let media =
        plx_media::ff::ExternalSource::HeldLocal(read.source.single_file("clip.mp4").unwrap());
    let mut entry = runtime.catalog.entries[0].clone();
    entry.media = MediaLocator::Local {
        relative_path: "clip.mp4".into(),
    };
    playback::replace_player(
        &mut app.player,
        &mut app.adapters.player,
        &mut app.repause_at,
        &mut app.refresh_hubs,
        &mut app.pages,
    );
    plx_media::route::reset_player_control_for_test(&app.player.session);
    let info = plx_media::ff::inspect::MediaInfo {
        tracks: Default::default(),
        resolved_source: None,
        hls_playback: None,
        video_codec: "h264".into(),
        audio_codec: "aac".into(),
        width: 1280,
        height: 720,
        fps: 24.0,
        duration_ns: 30_000_000_000,
        audio_channels: 2,
        audio_sample_rate: 48_000,
        audio_bit_depth: 16,
        dovi: Default::default(),
    };
    let id = "favorite:retained-file";
    plx_media::route::request_external_source(
        &mut app.player.session,
        &info,
        media.clone(),
        id,
        "File favorite",
        plx_platform::devcaps::assumed_shared(),
    )
    .unwrap();
    let identity = plx_media::route::playback_identity(&app.player.session);
    assert!(identity.is_some());
    assert_eq!(
        plx_media::route::external_source(&app.player.session),
        Some(media.clone())
    );
    let mut request = PlaybackRequest::ordinary(id, 12_000);
    request.local_selection = Some(selection.clone());
    request.continuation = identity.clone();
    let runtime = app.ott.as_mut().unwrap();
    runtime.transient.accepted = None;
    runtime.playing = Some(Playing { channel_capture: None,
        archive: None,
        kiosk_selection: None,
        transient_owner: None,
        local_selection: Some(selection),
        identity,
        profile: source_profile(source).unwrap(),
        entry,
        selection_id: id.into(),
        source: Some(media),
        advance_on_failure: false,
        requires_server_lease: false,
    });
    request
}

#[test]
fn installed_single_file_favorite_requires_its_original_held_media_identity() {
    let _serial = plx_base::testlock::serial();
    let _reset = FileRouteReset;
    let (_guard, mut app, source) = setup_file();
    let request = install_file_favorite(&mut app, &source);
    let runtime = app.ott.as_mut().unwrap();
    runtime.resume_pending = Some(request);
    runtime
        .local_inventory
        .install(runtime.local_inventory.sources.clone(), 8);
    rebind_lifecycle_resume(runtime);
    let resumed = runtime.resume_pending.as_ref().unwrap().clone();
    assert_eq!(lifecycle_resume_ready(runtime, &resumed), Ok(true));
    assert_eq!(resumed.position_ms, 12_000);
    let mut other = resumed.clone();
    other.id = "favorite:other-file".into();
    assert!(lifecycle_resume_ready(runtime, &other).is_err());
    let mut other = resumed.clone();
    other.continuation = None;
    assert!(lifecycle_resume_ready(runtime, &other).is_err());
    let mut other = resumed.clone();
    other.transient_owner = Some(local::OwnerToken(99));
    assert!(lifecycle_resume_ready(runtime, &other).is_err());
    let reopened =
        LocalSource::acquire_authorized(&OffFrame::for_test(), source.clone(), &mut || true)
            .unwrap();
    runtime.local_inventory.install(vec![reopened], 9);
    rebind_lifecycle_resume(runtime);
    assert_eq!(
        runtime.resume_pending.as_ref().unwrap().local_selection,
        resumed.local_selection
    );
    let mut direct = resumed;
    direct.local_selection = Some(runtime.bind_local_selection(&source).unwrap());
    assert!(lifecycle_resume_ready(runtime, &direct).is_err());
}
