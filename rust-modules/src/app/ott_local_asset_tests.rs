//! Existing Runtime slot and actual local asset worker; source-only until hostsim.
use super::*;
use crate::app::ott_local_session::{AuthorityRef, BoundSource};
use crate::app::ott_runtime::tests::local_integration::{self as local_fixture, TempDir};
use plx_base::task::OffFrame;
use std::path::Path;

struct Reset;
impl Drop for Reset {
    fn drop(&mut self) {
        sidecar::reset_local();
        plx_media::player::SHARED.reset_session();
        plx_media::player::TX.reset();
        route::reset_player_control_for_test(&route::PlaybackSession::IDLE);
    }
}
fn fixture() -> (TempDir, Box<App>, BoundSource) {
    let (guard, mut app, root) = local_fixture::setup();
    std::fs::write(
        Path::new(root.canonical_path.as_deref().unwrap()).join("first.mp4"),
        b"fixture",
    )
    .unwrap();
    local_fixture::install(&mut app, Some(&root), "first");
    route::reset_player_control_for_test(&app.player.session);
    plx_media::player::swap_state_for_test(plx_media::player::PlaybackState::Playing);
    app.ott.as_mut().unwrap().resume_source_work();
    // Retain the real worker's occupied marker so the test intercepts its queued
    // task instead of starting another thread. Work::run is then called directly.
    assert!(!app
        .ott
        .as_ref()
        .unwrap()
        .metadata_done
        .load(Ordering::Acquire));
    (guard, app, root)
}
fn file(root: &BoundSource, name: &str, bytes: &[u8]) {
    let path = Path::new(root.canonical_path.as_deref().unwrap()).join(name);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, bytes).unwrap();
}
fn poll(app: &mut App) {
    let mut runtime = app.ott.take().unwrap();
    runtime.poll_local_assets(app);
    app.ott = Some(runtime);
}
fn next(app: &mut App) -> (u64, Work, Arc<AtomicBool>) {
    let runtime = app.ott.as_mut().unwrap();
    let (_, MetadataSelection::Local(work)) = runtime
        .metadata_next
        .take()
        .expect("queued shared metadata work")
    else {
        panic!("Local task");
    };
    (
        runtime.metadata_generation,
        work,
        runtime.metadata_cancel.clone(),
    )
}
fn accept(app: &mut App, generation: u64, completion: Completion) {
    let mut runtime = app.ott.take().unwrap();
    runtime.accept_local_assets(app, generation, completion);
    app.ott = Some(runtime);
}
fn discover(app: &mut App) {
    plx_media::player::SHARED
        .seen_frame
        .store(true, Ordering::Release);
    poll(app);
    let (generation, work, cancel) = next(app);
    assert!(matches!(&work.kind, WorkKind::Discover { .. }));
    let complete = work.run(&OffFrame::for_test(), &cancel);
    assert!(matches!(&complete.result, Outcome::Discover(Ok(_))));
    accept(app, generation, complete);
    assert!(app.ott.as_ref().unwrap().local_assets.installed.is_some());
}
const SRT: &[u8] = b"1\n00:00:00,000 --> 00:00:02,000\nFixture caption\n\n";
fn read(app: &mut App) -> (u64, Work, Arc<AtomicBool>, sidecar::LocalReadRequest) {
    let tracks = route::external_tracks(&app.player.session).unwrap();
    let choice = tracks
        .local_choice(&tracks.local_subtitles()[0].target)
        .unwrap();
    route::reset_player_control_for_test(&app.player.session);
    assert!(route::apply_external_track(&mut app.player.session, &choice).unwrap());
    assert!(sidecar::peek_local_request().is_some());
    poll(app);
    let (generation, work, cancel) = next(app);
    let WorkKind::Read { request, .. } = &work.kind else {
        panic!("bound local read");
    };
    let request = request.clone();
    (generation, work, cancel, request)
}

#[test]
fn current_video_must_present_before_discovery_uses_the_existing_held_slot() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let (_guard, mut app, root) = fixture();
    file(&root, "first.srt", SRT);
    plx_media::player::SHARED
        .seen_frame
        .store(false, Ordering::Release);
    poll(&mut app);
    assert!(app.ott.as_ref().unwrap().metadata_next.is_none());
    plx_media::player::SHARED
        .seen_frame
        .store(true, Ordering::Release);
    poll(&mut app);
    let runtime = app.ott.as_ref().unwrap();
    assert!(matches!(
        runtime.metadata_next,
        Some((_, MetadataSelection::Local(_)))
    ));
    assert!(!runtime.metadata_done.load(Ordering::Acquire));
    assert!(
        !route::external_tracks(&app.player.session)
            .unwrap()
            .is_complete(),
        "discovery is visibly incomplete before I/O"
    );
}

#[test]
fn actual_worker_merges_metadata_and_filters_adjacent_files_without_reordering_video() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let (_guard, mut app, root) = fixture();
    file(&root, "first.srt", SRT);
    file(
        &root,
        "first.en.vtt",
        b"WEBVTT\n\n00:00.000 --> 00:02.000\nCaption\n",
    );
    file(&root, "another.srt", SRT);
    std::fs::create_dir(Path::new(root.canonical_path.as_deref().unwrap()).join("first.ass"))
        .unwrap();
    std::os::unix::fs::symlink(
        "another.srt",
        Path::new(root.canonical_path.as_deref().unwrap()).join("first.fr.srt"),
    )
    .unwrap();
    file(&root, ".ottplay/metadata.json", br#"{"version":1,"items":[{"relative_path":"first.mp4","title":"Metadata title","description":"First paragraph.\nSecond paragraph.","duration_ms":120000,"poster_relative_path":".ottplay/art/film.jpg"}]}"#);
    plx_media::player::SHARED
        .seen_frame
        .store(true, Ordering::Release);
    poll(&mut app);
    let (generation, work, cancel) = next(&mut app);
    let completion = work.run(&OffFrame::for_test(), &cancel);
    let Outcome::Discover(Ok(data)) = &completion.result else {
        panic!("successful discovery");
    };
    assert_eq!(data.entry.title, "Metadata title");
    assert_eq!(
        data.entry.description,
        "First paragraph.\nSecond paragraph."
    );
    assert_eq!(data.poster.as_deref(), Some(".ottplay/art/film.jpg"));
    let paths: Vec<_> = data.subtitles.iter().map(|row| row.path.as_str()).collect();
    assert_eq!(paths, ["first.en.vtt", "first.srt"]);
    let ids: Vec<_> = app
        .ott
        .as_ref()
        .unwrap()
        .catalog
        .entries
        .iter()
        .map(|row| row.id.clone())
        .collect();
    accept(&mut app, generation, completion);
    let runtime = app.ott.as_ref().unwrap();
    assert_eq!(
        runtime
            .catalog
            .entries
            .iter()
            .map(|row| row.id.clone())
            .collect::<Vec<_>>(),
        ids
    );
    assert_eq!(
        runtime.playing.as_ref().unwrap().entry.title,
        "Metadata title"
    );
}

#[test]
fn discovery_capacity_and_cancellation_are_real_worker_bounds() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let (_guard, mut app, root) = fixture();
    file(&root, "first.en.srt", SRT);
    file(&root, "first.fr.srt", SRT);
    let context = playing_context(app.ott.as_ref().unwrap(), &app).unwrap();
    let work = Work {
        context: context.clone(),
        kind: WorkKind::Discover { capacity: 1 },
    };
    let completion = work.run(&OffFrame::for_test(), &AtomicBool::new(false));
    let Outcome::Discover(Ok(data)) = completion.result else {
        panic!("discovery");
    };
    assert_eq!(data.subtitles.len(), 1);
    assert!(!data.complete);
    let completion = Work {
        context: context.clone(),
        kind: WorkKind::Discover { capacity: 2 },
    }
    .run(&OffFrame::for_test(), &AtomicBool::new(true));
    assert!(matches!(
        completion.result,
        Outcome::Discover(Err(ReadError::Aborted))
    ));
    app.ott.as_mut().unwrap().local_inventory.sources[0].withdraw();
    let withdrawn = Work {
        context,
        kind: WorkKind::Discover { capacity: 2 },
    }
    .run(&OffFrame::for_test(), &AtomicBool::new(false));
    assert!(matches!(
        withdrawn.result,
        Outcome::Discover(Err(ReadError::Aborted))
    ));
    let binding = app
        .ott
        .as_ref()
        .unwrap()
        .playing
        .as_ref()
        .unwrap()
        .local_selection
        .as_ref()
        .unwrap();
    assert!(app
        .ott
        .as_ref()
        .unwrap()
        .capture_local_root(binding)
        .is_none());
}

#[test]
fn late_discovery_after_root_withdrawal_cannot_publish_rows_or_metadata() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let (_guard, mut app, root) = fixture();
    file(&root, "first.srt", SRT);
    plx_media::player::SHARED
        .seen_frame
        .store(true, Ordering::Release);
    poll(&mut app);
    let (generation, work, cancel) = next(&mut app);
    let completion = work.run(&OffFrame::for_test(), &cancel);
    assert!(matches!(&completion.result, Outcome::Discover(Ok(_))));
    app.ott.as_mut().unwrap().local_inventory.sources[0].withdraw();
    accept(&mut app, generation, completion);
    assert!(app.ott.as_ref().unwrap().local_assets.installed.is_none());
    assert!(route::external_tracks(&app.player.session)
        .unwrap()
        .local_subtitles()
        .is_empty());
}

#[test]
fn hover_cannot_cancel_explicit_read_or_release_its_worker_slot() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let (_guard, mut app, root) = fixture();
    file(&root, "first.srt", SRT);
    discover(&mut app);
    let (generation, work, cancel, request) = read(&mut app);
    let mut runtime = app.ott.take().unwrap();
    let profile = runtime.effective_profile().unwrap().clone();
    let mut hover = runtime.catalog.entries[0].clone();
    hover.id = "hover".into();
    hover.media = MediaLocator::Local {
        relative_path: "another.mp4".into(),
    };
    runtime.request_metadata(profile.clone(), hover);
    assert_eq!(runtime.metadata_target, Some((profile.id, "hover".into())));
    assert_eq!(runtime.metadata_generation, generation);
    assert!(Arc::ptr_eq(&runtime.metadata_cancel, &cancel));
    assert!(!cancel.load(Ordering::Acquire));
    assert!(!runtime.metadata_done.load(Ordering::Acquire));
    assert!(matches!(runtime.local_assets.purpose, Purpose::Read(..)));
    app.ott = Some(runtime);
    let completion = work.run(&OffFrame::for_test(), &cancel);
    assert!(matches!(
        &completion.result,
        Outcome::Read { result: Ok(_), .. }
    ));
    accept(&mut app, generation, completion);
    assert!(
        !sidecar::local_request_is_current(&request),
        "successful terminal is consumed once"
    );
    assert!(sidecar::selected());
    assert!(!sidecar::local_failed());
}

#[test]
fn off_rejects_already_prepared_late_local_bytes() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let (_guard, mut app, root) = fixture();
    file(&root, "first.srt", SRT);
    discover(&mut app);
    let (generation, work, cancel, request) = read(&mut app);
    let completion = work.run(&OffFrame::for_test(), &cancel);
    assert!(matches!(
        &completion.result,
        Outcome::Read { result: Ok(_), .. }
    ));
    let off = route::external_tracks(&app.player.session)
        .unwrap()
        .choice(plx_media::ff::inspect::TrackKind::Subtitle, None)
        .unwrap();
    route::apply_external_track(&mut app.player.session, &off).unwrap();
    assert!(!sidecar::local_request_is_current(&request));
    accept(&mut app, generation, completion);
    assert!(!sidecar::selected());
    assert!(!sidecar::local_failed());
}

#[test]
fn new_playback_rejects_late_local_read_and_keeps_new_identity() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let (_guard, mut app, root) = fixture();
    file(&root, "first.srt", SRT);
    discover(&mut app);
    let (generation, work, cancel, _) = read(&mut app);
    let completion = work.run(&OffFrame::for_test(), &cancel);
    assert!(matches!(
        &completion.result,
        Outcome::Read { result: Ok(_), .. }
    ));
    local_fixture::install(&mut app, None, "live");
    let identity = route::playback_identity(&app.player.session);
    accept(&mut app, generation, completion);
    assert_eq!(route::playback_identity(&app.player.session), identity);
    assert!(!sidecar::selected());
}

#[test]
fn suspend_keeps_actual_occupancy_and_hover_target_but_retires_read() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let (_guard, mut app, root) = fixture();
    file(&root, "first.srt", SRT);
    discover(&mut app);
    let (_, _, cancel, request) = read(&mut app);
    let mut runtime = app.ott.take().unwrap();
    let target = (
        runtime.effective_profile().unwrap().id.clone(),
        "hover".into(),
    );
    runtime.metadata_target = Some(target.clone());
    runtime.suspend_source_work(&mut app);
    assert!(!runtime.metadata_done.load(Ordering::Acquire));
    assert_eq!(runtime.metadata_target, Some(target));
    assert!(cancel.load(Ordering::Acquire));
    assert!(!sidecar::local_request_is_current(&request));
    assert!(matches!(runtime.local_assets.purpose, Purpose::Preview));
    app.ott = Some(runtime);
}

#[test]
fn durable_local_profile_without_receipt_rejects_ambiguous_authorized_roots() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let (_guard, mut app, root) = fixture();
    app.ott
        .as_mut()
        .unwrap()
        .playing
        .as_mut()
        .unwrap()
        .local_selection = None;
    assert!(
        playing_context(app.ott.as_ref().unwrap(), &app).is_some(),
        "one actual authorized root can be rebound"
    );
    let mut second = root.clone();
    second.id = "second-root".into();
    second.authority_ref = AuthorityRef(2);
    let second = crate::app::ott_local_root::LocalSource::acquire_authorized(
        &OffFrame::for_test(),
        second,
        &mut || true,
    )
    .unwrap();
    let runtime = app.ott.as_mut().unwrap();
    let mut sources = runtime.local_inventory.sources.clone();
    sources.push(second);
    runtime.local_inventory.install(sources, 7);
    assert!(playing_context(app.ott.as_ref().unwrap(), &app).is_none());
}

#[test]
fn refused_discovery_start_is_terminal_and_does_not_resubmit_each_frame() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let (_guard, mut app, root) = fixture();
    file(&root, "first.srt", SRT);
    plx_media::player::SHARED
        .seen_frame
        .store(true, Ordering::Release);
    poll(&mut app);
    let (_, work, _) = next(&mut app);
    assert!(matches!(&work.kind, WorkKind::Discover { .. }));
    app.ott.as_mut().unwrap().local_asset_start_failed();
    poll(&mut app);
    poll(&mut app);
    let runtime = app.ott.as_ref().unwrap();
    assert!(runtime.local_assets.installed.is_some());
    assert!(runtime.metadata_next.is_none());
    assert!(!route::external_tracks(&app.player.session)
        .unwrap()
        .is_complete());
}

#[test]
fn cancelled_generation_ignores_prepared_discovery_without_restoring_old_state() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let (_guard, mut app, root) = fixture();
    file(&root, "first.srt", SRT);
    plx_media::player::SHARED
        .seen_frame
        .store(true, Ordering::Release);
    poll(&mut app);
    let (old_generation, work, cancel) = next(&mut app);
    let completion = work.run(&OffFrame::for_test(), &cancel);
    assert!(matches!(&completion.result, Outcome::Discover(Ok(_))));
    app.ott.as_mut().unwrap().cancel_local_asset_work();
    let current = app.ott.as_ref().unwrap().metadata_generation;
    assert_ne!(current, old_generation);
    accept(&mut app, old_generation, completion);
    let runtime = app.ott.as_ref().unwrap();
    assert_eq!(runtime.metadata_generation, current);
    assert!(runtime.local_assets.installed.is_none());
    assert!(route::external_tracks(&app.player.session)
        .unwrap()
        .local_subtitles()
        .is_empty());
}

#[test]
fn busy_native_publication_retains_completed_data_in_same_slot_without_restarting_io() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let (_guard, mut app, root) = fixture();
    file(&root, "first.srt", SRT);
    plx_media::player::SHARED
        .seen_frame
        .store(true, Ordering::Release);
    poll(&mut app);
    let (generation, work, cancel) = next(&mut app);
    let completion = work.run(&OffFrame::for_test(), &cancel);
    assert!(matches!(&completion.result, Outcome::Discover(Ok(_))));
    route::request_user_route_intent(&app.player.session, route::UserRouteIntent::AdaptiveReload);
    let action = route::claim_route_action().expect("real pending intent enters Applying");
    accept(&mut app, generation, completion);
    {
        let runtime = app.ott.as_mut().unwrap();
        assert!(matches!(
            &runtime.metadata_next,
            Some((_, MetadataSelection::LocalPublish(_)))
        ));
        assert!(matches!(&runtime.local_assets.purpose, Purpose::Publish(_)));
        assert!(runtime.local_assets.installed.is_none());
        let profile = runtime.effective_profile().unwrap().clone();
        let mut hover = runtime.catalog.entries[0].clone();
        hover.id = "hover-during-publish".into();
        runtime.request_metadata(profile, hover);
    }
    poll(&mut app);
    {
        let runtime = app.ott.as_ref().unwrap();
        assert_eq!(runtime.metadata_generation, generation);
        assert!(Arc::ptr_eq(&runtime.metadata_cancel, &cancel));
        assert!(!cancel.load(Ordering::Acquire));
        assert!(!runtime.metadata_done.load(Ordering::Acquire));
        assert!(matches!(
            &runtime.metadata_next,
            Some((_, MetadataSelection::LocalPublish(_)))
        ));
        assert_eq!(
            runtime.metadata_target.as_ref().unwrap().1,
            "hover-during-publish"
        );
    }
    assert!(route::finish_route_action(
        &mut app.player.session,
        &action,
        route::RouteApplyResult::Cancelled
    ));
    poll(&mut app);
    let runtime = app.ott.as_ref().unwrap();
    assert!(runtime.local_assets.installed.is_some());
    assert!(runtime.metadata_next.is_none());
    assert_eq!(runtime.metadata_generation, generation);
    assert!(!runtime.metadata_done.load(Ordering::Acquire));
    assert_eq!(
        route::external_tracks(&app.player.session)
            .unwrap()
            .local_subtitles()
            .len(),
        1
    );
}

#[test]
fn catalog_cancel_retires_held_read_and_allows_fresh_ticket_without_accepting_old_bytes() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let (_guard, mut app, root) = fixture();
    file(&root, "first.srt", SRT);
    discover(&mut app);
    let (old_generation, work, old_cancel, old_request) = read(&mut app);
    let old_completion = work.run(&OffFrame::for_test(), &old_cancel);
    assert!(matches!(
        &old_completion.result,
        Outcome::Read { result: Ok(_), .. }
    ));

    app.ott.as_mut().unwrap().cancel_catalog();
    assert!(old_cancel.load(Ordering::Acquire));
    assert!(playing_context(app.ott.as_ref().unwrap(), &app).is_some());
    poll(&mut app);
    {
        let runtime = app.ott.as_ref().unwrap();
        assert!(matches!(&runtime.local_assets.purpose, Purpose::Preview));
        assert!(!runtime.metadata_done.load(Ordering::Acquire));
        assert!(!sidecar::local_request_is_current(&old_request));
        assert!(
            sidecar::peek_local_request().is_some(),
            "selected track rearms with a fresh ticket"
        );
    }

    poll(&mut app);
    let (fresh_generation, fresh_work, fresh_cancel) = next(&mut app);
    let WorkKind::Read {
        request: fresh_request,
        ..
    } = &fresh_work.kind
    else {
        panic!("fresh selected-track read");
    };
    assert_ne!(fresh_generation, old_generation);
    assert!(!Arc::ptr_eq(&fresh_cancel, &old_cancel));
    assert!(!fresh_cancel.load(Ordering::Acquire));
    assert!(sidecar::local_request_is_current(fresh_request));

    accept(&mut app, old_generation, old_completion);
    let runtime = app.ott.as_ref().unwrap();
    assert_eq!(runtime.metadata_generation, fresh_generation);
    assert!(
        matches!(&runtime.local_assets.purpose, Purpose::Read(_, request)
        if sidecar::local_request_is_current(request))
    );
    assert!(sidecar::local_request_is_current(fresh_request));
    assert!(!sidecar::local_request_is_current(&old_request));
    assert!(!runtime.metadata_done.load(Ordering::Acquire));
}

#[test]
fn long_whitespace_prefix_keeps_usable_label_and_discovery_does_not_repeat() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let (_guard, mut app, root) = fixture();
    let stem = format!("{}film", " ".repeat(128));
    let media_path = format!("{stem}.mp4");
    let subtitle_path = format!("{stem}.srt");
    file(&root, &media_path, b"fixture");
    file(&root, &subtitle_path, SRT);
    playback::replace_player(
        &mut app.player,
        &mut app.adapters.player,
        &mut app.repause_at,
        &mut app.refresh_hubs,
        &mut app.pages,
    );
    let source = plx_media::ff::ExternalSource::Local(
        Path::new(root.canonical_path.as_deref().unwrap()).join(&media_path),
    );
    let info = plx_media::ff::inspect::MediaInfo {
        tracks: Default::default(),
        resolved_source: None,
        hls_playback: None,
        video_codec: "h264".into(),
        audio_codec: "aac".into(),
        width: 1280,
        height: 720,
        fps: 24.0,
        duration_ns: 10_000_000_000,
        audio_channels: 2,
        audio_sample_rate: 48000,
        audio_bit_depth: 16,
        dovi: Default::default(),
    };
    route::request_external_source(
        &mut app.player.session,
        &info,
        source.clone(),
        "Film",
        "Movies",
        plx_platform::devcaps::assumed_shared(),
    )
    .unwrap();
    route::reset_player_control_for_test(&app.player.session);
    plx_media::player::swap_state_for_test(plx_media::player::PlaybackState::Playing);
    {
        let runtime = app.ott.as_mut().unwrap();
        let playing = runtime.playing.as_mut().unwrap();
        playing.identity = route::playback_identity(&app.player.session);
        playing.source = Some(source);
        playing.entry.media = MediaLocator::Local {
            relative_path: media_path,
        };
        runtime.catalog.entries[0].media = playing.entry.media.clone();
    }
    assert!(playing_context(app.ott.as_ref().unwrap(), &app).is_some());
    discover(&mut app);
    let tracks = route::external_tracks(&app.player.session).unwrap();
    assert_eq!(tracks.local_subtitles().len(), 1);
    assert_eq!(tracks.local_subtitles()[0].title, "film.srt");
    assert!(tracks
        .local_choice(&tracks.local_subtitles()[0].target)
        .is_ok());
    let generation = app.ott.as_ref().unwrap().metadata_generation;
    poll(&mut app);
    poll(&mut app);
    let runtime = app.ott.as_ref().unwrap();
    assert_eq!(runtime.metadata_generation, generation);
    assert!(runtime.metadata_next.is_none());
    assert!(runtime.local_assets.installed.is_some());
    assert!(!runtime.metadata_done.load(Ordering::Acquire));
}

#[test]
fn real_catalog_replacement_rejects_old_discovery_without_suppressing_a_valid_owner() {
    let _serial = plx_base::testlock::serial();
    for replace_logical_playback in [false, true] {
        let _reset = Reset;
        let (_guard, mut app, root) = fixture();
        file(&root, "first.srt", SRT);
        plx_media::player::SHARED.seen_frame.store(true, Ordering::Release);
        poll(&mut app);
        let (old_generation, work, cancel) = next(&mut app);
        let old_catalog = work.context.native.clone().unwrap();
        let identity = work.context.playback.clone().unwrap();
        let completion = work.run(&OffFrame::for_test(), &cancel);
        assert!(matches!(&completion.result, Outcome::Discover(Ok(data))
            if data.subtitles.len() == 1));

        // Publish a new authoritative catalogue through the real route API.
        // The earlier producer result remains valid data for an obsolete token.
        route::install_external_local_subtitles(&mut app.player.session,
            &identity, &old_catalog, Vec::new(), false,
            wire::MAX_TRACKS, wire::MAX_TRACKS_PER_KIND).unwrap();
        let current_catalog = route::external_tracks(&app.player.session).unwrap().clone();
        assert!(!old_catalog.same_session(&current_catalog));
        assert_eq!(route::install_external_local_subtitles(&mut app.player.session,
            &identity, &old_catalog, Vec::new(), true,
            wire::MAX_TRACKS, wire::MAX_TRACKS_PER_KIND),
            Err(route::ExternalTrackApplyError::InvalidChoice(route::ExternalTrackError::StaleSession)));
        accept(&mut app, old_generation, completion);
        assert!(route::external_tracks(&app.player.session).unwrap().same_session(&current_catalog));
        assert!(route::external_tracks(&app.player.session).unwrap().local_subtitles().is_empty());
        assert!(app.ott.as_ref().unwrap().metadata_next.is_none());
        assert!(!app.ott.as_ref().unwrap().metadata_done.load(Ordering::Acquire));

        if replace_logical_playback {
            // The same file selected again is a different logical playback.
            local_fixture::install(&mut app, Some(&root), "first");
            route::reset_player_control_for_test(&app.player.session);
            plx_media::player::swap_state_for_test(plx_media::player::PlaybackState::Playing);
            plx_media::player::SHARED.seen_frame.store(true, Ordering::Release);
            assert_ne!(route::playback_identity(&app.player.session).as_ref(), Some(&identity));
        }
        poll(&mut app);
        let (generation, work, cancel) = next(&mut app);
        assert!(generation > old_generation);
        assert!(matches!(&work.kind, WorkKind::Discover { .. }));
        assert!(work.context.native.as_ref().unwrap().same_session(
            route::external_tracks(&app.player.session).unwrap()));
        assert_eq!(work.context.playback, route::playback_identity(&app.player.session));
        let completion = work.run(&OffFrame::for_test(), &cancel);
        accept(&mut app, generation, completion);
        let tracks = route::external_tracks(&app.player.session).unwrap();
        assert_eq!(tracks.local_subtitles().len(), 1);
        assert_eq!(tracks.local_subtitles()[0].title, "first.srt");
        assert!(app.ott.as_ref().unwrap().metadata_next.is_none());
        assert!(!app.ott.as_ref().unwrap().metadata_done.load(Ordering::Acquire));
    }
}
