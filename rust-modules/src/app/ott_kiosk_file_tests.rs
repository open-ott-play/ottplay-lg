//! Exact File capabilities at the existing kiosk Item boundary. No IO worker,
//! Saved writer or decoder is executed; the test grants use real temporary files.
use super::*;
use crate::app::ott_local_root::LocalSource;
use crate::app::ott_local_session as local;
use crate::app::ott_runtime::tests::local_integration as fixture;
use plx_base::task::OffFrame;

fn setup_file() -> (fixture::TempDir, Box<App>, local::BoundSource) {
    let (guard, mut app, mut source) = fixture::setup();
    let folder = std::path::Path::new(source.canonical_path.as_deref().unwrap());
    std::fs::write(folder.join("clip.mp4"), b"held file fixture").unwrap();
    std::fs::write(folder.join("sibling.mp4"), b"not granted").unwrap();
    let path = folder.join("clip.mp4");
    source.name = "One clip".into();
    source.kind = local::SourceKind::File;
    source.canonical_path = Some(path.to_str().unwrap().into());
    let live = LocalSource::acquire_authorized(&OffFrame::for_test(), source.clone(), &mut || true)
        .unwrap();
    let runtime = app.ott.as_mut().unwrap();
    runtime.local_inventory.install(vec![live], 7);
    runtime.saving = false;
    runtime.catalog.entries[0].title = "One clip".into();
    runtime.catalog.entries[0].media = MediaLocator::Local {
        relative_path: "clip.mp4".into(),
    };
    let request = local::LocalRequest::parse(&json!({
        "op":"play", "runtime":runtime.remote.runtime(), "source":source.id,
        "session":true,"progressive":true
    }))
    .unwrap();
    let owner = OwnerToken(7);
    let mut pending = local::PendingLocal::new(request, owner, u64::MAX);
    let items = [local::LocalItem {
        id: "first".into(),
        playable: true,
    }];
    let scan = local::CatalogSnapshot {
        source_id: &source.id,
        authority_ref: source.authority_ref,
        source_generation: source.generation,
        state: local::ScanState::Complete,
        items: &items,
    };
    let context = local::AdmissionContext {
        runtime: runtime.remote.runtime(),
        owner,
        invocation_current: true,
        now_ms: runtime.now_ms(),
        mutations_allowed: true,
    };
    let local::Admission::Propose(context) = pending.evaluate(&context, &source, &scan, 1) else {
        panic!("File session proposal");
    };
    let read = runtime
        .capture_local_source(&runtime.bind_local_selection(&source).unwrap())
        .unwrap();
    runtime.transient.accepted = Some(transient::TransientMedia::Local(transient::LocalSession {
        read,
        context,
        profile: transient::source_profile(&source).unwrap(),
        borrowed: None, kiosk_resume: None,
    }));
    (guard, app, source)
}

#[test]
fn file_item_pin_captures_exact_proof_and_rebinds_without_global_favorites() {
    let _serial = plx_base::testlock::serial();
    let (_guard, mut app, source) = setup_file();
    let mut runtime = app.ott.take().unwrap();
    let prepared = prepare(&runtime, &app, Some("1"), true).unwrap();
    let TargetRef::Item(favorite) = prepared.stored.target() else {
        panic!("Item pin");
    };
    assert!(
        matches!(&favorite.locator, Locator::Local { relative_path, .. } if relative_path == "clip.mp4")
    );
    assert!(runtime.local_inventory.sources[0].root_handle().is_none());
    runtime.saved = save_candidate(&runtime, &prepared).unwrap();
    assert!(runtime.saved.favorites.entries().is_empty());
    let typed = pinned::prepare(&runtime).unwrap();
    let Activation::Pinned(selection) = typed.activation else {
        panic!("typed Item pin");
    };
    let (request, profile, entry, favorite) =
        runtime.prepare_play_request(selection.request()).unwrap();
    assert_eq!(
        request.local_selection.as_ref().unwrap().source_id,
        source.id
    );
    assert!(entry.is_none());
    assert!(favorite.is_some());
    admit_bound(
        &runtime,
        &app,
        BoundCandidate {
            request: &request,
            profile: &profile,
            entry: entry.as_ref(),
            favorite: favorite.as_ref(),
        },
    )
    .unwrap();
    assert!(runtime.resolver_next.is_none());
    assert!(!runtime.saving);
}

#[test]
fn file_source_never_becomes_folder_pin_or_sibling_item() {
    let _serial = plx_base::testlock::serial();
    let (_guard, mut app, source) = setup_file();
    let mut runtime = app.ott.take().unwrap();
    assert!(folder_fingerprint(&runtime, &source.id).is_err());
    assert!(prepare(&runtime, &app, Some("folder:One clip"), true).is_err());
    runtime.catalog.entries[0].media = MediaLocator::Local {
        relative_path: "sibling.mp4".into(),
    };
    assert!(prepare(&runtime, &app, Some("1"), true).is_err());
    assert!(runtime.saved.favorites.entries().is_empty());
    assert!(stored::read(&runtime.saved.settings).unwrap().is_none());
}

#[test]
fn file_prepared_and_typed_pin_reject_same_generation_reopened_capability() {
    let _serial = plx_base::testlock::serial();
    let (_guard, mut app, source) = setup_file();
    let mut runtime = app.ott.take().unwrap();
    let prepared = prepare(&runtime, &app, Some("1"), true).unwrap();
    runtime.saved = save_candidate(&runtime, &prepared).unwrap();
    let typed = pinned::prepare(&runtime).unwrap();
    let Activation::Pinned(selection) = &typed.activation else {
        panic!("typed Item pin");
    };
    let old = runtime.bind_local_selection(&source).unwrap();
    let replacement =
        LocalSource::acquire_authorized(&OffFrame::for_test(), source.clone(), &mut || true)
            .unwrap();
    runtime.local_inventory.install(vec![replacement], 7);
    assert_eq!(
        old,
        runtime.bind_local_selection(&source).unwrap(),
        "descriptor/receipt stay equal"
    );
    assert!(!still_current(&runtime, &app, &prepared));
    assert!(save_candidate(&runtime, &prepared).is_err());
    assert!(runtime.prepare_play_request(selection.request()).is_err());
    assert!(
        pinned::permit(
            &runtime,
            selection,
            &transient::source_profile(&source).unwrap()
        )
        .is_err()
    );
    assert!(
        prepared == prepared.clone(),
        "retained proof equality remains structural after withdrawal"
    );
}

#[test]
fn file_queue_membership_does_not_forge_a_plex_queue_owner() {
    let _serial = plx_base::testlock::serial();
    let (_guard, mut app, _) = setup_file();
    let mut runtime = app.ott.take().unwrap();
    let prepared = prepare(&runtime, &app, Some("1"), true).unwrap();
    runtime.saved = save_candidate(&runtime, &prepared).unwrap();
    let favorite = prepared.stored.pins()[0].clone();
    runtime.saved.settings.insert(
        stored::KEY.into(),
        Stored::queue(vec![favorite], true)
            .unwrap()
            .encode()
            .unwrap(),
    );
    let (request, profile, entry, favorite) = runtime
        .prepare_play_request(PlaybackRequest::ordinary("first", 0))
        .unwrap();
    assert!(
        admit_bound(
            &runtime,
            &app,
            BoundCandidate {
                request: &request,
                profile: &profile,
                entry: entry.as_ref(),
                favorite: favorite.as_ref(),
            }
        )
        .is_err()
    );
}

#[test]
fn imported_file_item_pin_cannot_replace_receiving_device_attestation() {
    let _serial = plx_base::testlock::serial();
    let (_guard, mut app, _) = setup_file();
    let mut runtime = app.ott.take().unwrap();
    let prepared = prepare(&runtime, &app, Some("1"), true).unwrap();
    // Persisting the target alone deliberately omits the proof normally added
    // by save_candidate. A portable favorite is not a local read grant.
    runtime
        .saved
        .settings
        .insert(stored::KEY.into(), prepared.encoded().unwrap());
    assert!(pinned::prepare(&runtime).is_err());
    let (request, profile, entry, favorite) = runtime
        .prepare_play_request(PlaybackRequest::ordinary("first", 0))
        .unwrap();
    assert!(
        admit_bound(
            &runtime,
            &app,
            BoundCandidate {
                request: &request,
                profile: &profile,
                entry: entry.as_ref(),
                favorite: favorite.as_ref(),
            }
        )
        .is_err()
    );
}

struct RouteReset;
impl Drop for RouteReset {
    fn drop(&mut self) {
        plx_media::player::SHARED.reset_session();
        plx_media::player::TX.reset();
        plx_media::route::reset_player_control_for_test(&plx_media::route::PlaybackSession::IDLE);
    }
}
fn install_file(app: &mut App, source: &local::BoundSource) {
    let runtime = app.ott.as_ref().unwrap();
    let selection = runtime.bind_local_selection(source).unwrap();
    let read = runtime.capture_local_source(&selection).unwrap();
    let media =
        plx_media::ff::ExternalSource::HeldLocal(read.source.single_file("clip.mp4").unwrap());
    let entry = runtime.catalog.entries[0].clone();
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
    plx_media::route::request_external_source(
        &mut app.player.session,
        &info,
        media.clone(),
        "first",
        "One clip",
        plx_platform::devcaps::assumed_shared(),
    )
    .unwrap();
    let identity = plx_media::route::playback_identity(&app.player.session);
    assert!(identity.is_some());
    app.ott.as_mut().unwrap().playing = Some(Playing { channel_capture: None,
        archive: None,
        kiosk_selection: None,
        transient_owner: Some(OwnerToken(7)),
        local_selection: Some(selection),
        identity,
        profile: transient::source_profile(source).unwrap(),
        entry,
        selection_id: "first".into(),
        source: Some(media),
        advance_on_failure: false,
        requires_server_lease: false,
    });
}

#[test]
fn observed_file_item_requires_the_installed_held_file() {
    let _serial = plx_base::testlock::serial();
    let _reset = RouteReset;
    let (_guard, mut app, source) = setup_file();
    install_file(&mut app, &source);
    let mut runtime = app.ott.take().unwrap();
    let observed = observe(&runtime, &app)
        .unwrap()
        .expect("exact installed File");
    let prepared = capture_prepared(&runtime, &observed, true).unwrap();
    assert!(matches!(prepared.stored.target(), TargetRef::Item(_)));
    runtime.saved = save_candidate(&runtime, &prepared).unwrap();
    assert!(owned_pinned_target(&runtime, &app).unwrap().is_some());
    let replacement =
        LocalSource::acquire_authorized(&OffFrame::for_test(), source, &mut || true).unwrap();
    runtime.local_inventory.install(vec![replacement], 7);
    assert!(!observe(&runtime, &app).is_ok_and(|observed| observed.is_some()));
    assert!(!owned_pinned_target(&runtime, &app).is_ok_and(|observed| observed.is_some()));
}
