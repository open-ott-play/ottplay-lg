//! Terminal-consumer regressions; native Error/EOF signals are supplied, not decoded.
use super::*;

fn terminal_fixture() -> Box<App> {
    let mut app = fixture();
    // Occupy the writer before admission too: revoking its permit alone would
    // still let a checkpoint spawn a worker which later refuses the write.
    app.ott.as_mut().unwrap().saving = true;
    app
}

fn mark_eof() {
    plx_media::player::SHARED.ended.store(true, Ordering::Release);
    assert!(plx_media::player::ended());
}

fn seed_bookmark(app: &mut App) {
    app.ott.as_mut().unwrap().checkpoints.insert(profile().id, SavedResume {
        entry_id: "first".into(), position_ms: 4_000,
        source_fingerprint: profile_fingerprint(&profile()),
    });
}

/// Land the already queued automatic successor, without renewing viewer intent.
fn admit_successor(app: &mut App, id: &str) -> u64 {
    let runtime = app.ott.as_mut().unwrap();
    assert_eq!(runtime.play_target.as_ref().unwrap().id, id);
    let queued = runtime.resolver_next.take().expect("automatic resolver request");
    assert_eq!(queued.generation, runtime.play_generation);
    let generation = queued.generation;
    let mut info = unsupported_info();
    info.video_codec = "h264".into(); info.audio_codec = "aac".into();
    info.audio_channels = 2; info.audio_sample_rate = 48_000; info.audio_bit_depth = 16;
    runtime.tx.send(Job::Play(generation, profile(), Ok((entry(id),
        ReadySource::External(plx_media::ff::ExternalSource::Http(PRIVATE_URL.into()), info, None)))))
        .unwrap_or_else(|_| panic!("fixture receiver disappeared"));
    let clock = plx_base::task::time::ClockSample::capture_main(app.adapters.player.mt()).unwrap();
    let mut resources = AcceptedResources::default();
    tick_with_start(app, clock, |runtime, app, profile, entry, source|
        start_resolved_with(runtime, app, profile, entry, source, &mut resources));
    assert_eq!(resources.starts, 1);
    let runtime = app.ott.as_ref().unwrap();
    assert!(runtime.play_target.is_none());
    assert!(playing_matches(runtime.playing.as_ref().unwrap(), app));
    assert!(!app.adapters.player.is_live());
    app.ott.as_mut().unwrap().resolver_busy = true;
    generation
}

#[test]
fn owned_error_eof_preserves_exact_attempt_and_bookmark_until_failure_reconciliation() {
    let _serial = plx_base::testlock::serial();
    let _reset = ResetMedia;
    let mut app = terminal_fixture();
    let generation = admit(&mut app, "first");
    // Keep even the broken EOF path from launching a real resolver.
    app.ott.as_mut().unwrap().resolver_busy = true;
    seed_bookmark(&mut app);
    let runtime = app.ott.as_ref().unwrap();
    let saved = serde_json::to_value(&runtime.saved).unwrap();
    let checkpoints = serde_json::to_value(&runtime.checkpoints).unwrap();
    let selected = runtime.selected.clone();
    let identity = plx_media::route::playback_identity(&app.player.session);
    signal_error(&app, FailureKind::TvPipeline);
    mark_eof();

    // The frame's EOF consumer runs before ordinary failure reconciliation.
    for _ in 0..2 {
        assert!(finish_catalog_playback(&mut app), "owned failure must not fall through to legacy EOF");
        let runtime = app.ott.as_ref().unwrap();
        assert_eq!(serde_json::to_value(&runtime.checkpoints).unwrap(), checkpoints,
            "Error+EOF must not write a successful-completion bookmark");
        assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), saved);
        assert_eq!(runtime.selected, selected);
        assert_eq!(runtime.play_generation, generation);
        assert!(runtime.play_target.is_none() && runtime.resolver_next.is_none());
        assert_eq!(runtime.playing.as_ref().unwrap().selection_id, "first");
        assert!(!runtime.playing.as_ref().unwrap().advance_on_failure);
        assert_eq!(plx_media::route::playback_identity(&app.player.session), identity);
        assert_eq!(plx_media::player::state(&app.player.session), PlaybackState::Error);
    }
    reconcile(&mut app);
    let runtime = app.ott.as_ref().unwrap();
    assert!(runtime.playing.is_none());
    assert!(runtime.play_target.is_none() && runtime.resolver_next.is_none());
    assert_eq!(runtime.status, "Native playback failed");
    assert_failure(&read(&mut app, "status", 1), generation, FailureKind::TvPipeline);
}

#[test]
fn error_eof_does_not_renew_the_automatic_failure_budget() {
    let _serial = plx_base::testlock::serial();
    let _reset = ResetMedia;
    let mut app = terminal_fixture();
    app.ott.as_mut().unwrap().catalog.entries.push(entry("third"));
    admit(&mut app, "first");
    let runtime = app.ott.as_mut().unwrap();
    runtime.resolver_busy = true;
    runtime.playing.as_mut().unwrap().advance_on_failure = true;
    signal_error(&app, FailureKind::TvPipeline);
    reconcile(&mut app);
    admit_successor(&mut app, "second");
    signal_error(&app, FailureKind::TvPipeline);
    mark_eof();

    assert!(finish_catalog_playback(&mut app));
    let runtime = app.ott.as_ref().unwrap();
    assert!(runtime.play_target.is_none() && runtime.resolver_next.is_none(),
        "a failed second attempt is not a successful EOF launching the third");
    reconcile(&mut app);
    reconcile(&mut app); // Duplicate observation cannot spend another attempt.
    let runtime = app.ott.as_ref().unwrap();
    let retry = runtime.continuity.pending_retry().expect("second failure must wait");
    assert_eq!(retry.failures, 2, "EOF must not renew the consecutive-failure budget");
    assert_eq!(retry.after, "second");
    assert!(retry.not_before_ms > runtime.now_ms());
    assert!(runtime.play_target.is_none() && runtime.resolver_next.is_none());
}

#[test]
fn non_error_eof_keeps_short_and_near_end_no_callback_completion() {
    let _serial = plx_base::testlock::serial();
    let _reset = ResetMedia;
    // These are terminal-consumer samples, not claims about native decoding.
    for (duration_ns, position_ns, seen) in [
        (10_000_000_000, 10_000_000_000, true),
        (250_000_000, 250_000_000, true),
        (10_000_000_000, 9_999_000_000, false),
    ] {
        let mut app = terminal_fixture();
        admit(&mut app, "first");
        app.ott.as_mut().unwrap().resolver_busy = true;
        seed_bookmark(&mut app);
        let shared = &plx_media::player::SHARED;
        shared.duration_ns.store(duration_ns, Ordering::Release);
        shared.playpos_ns.store(position_ns, Ordering::Release);
        shared.seen_frame.store(seen, Ordering::Release);
        shared.frames.store(if seen { 1 } else { 0 }, Ordering::Release);
        plx_media::player::swap_state_for_test(if seen { PlaybackState::Playing } else { PlaybackState::Buffering });
        mark_eof();
        assert!(finish_catalog_playback(&mut app));
        let runtime = app.ott.as_ref().unwrap();
        assert_eq!(runtime.play_target.as_ref().unwrap().id, "second");
        assert!(runtime.resolver_next.is_some());
        assert_eq!(runtime.checkpoints[&profile().id].position_ms, 0);
        assert!(runtime.continuity.pending_retry().is_none());
    }
}

#[test]
fn error_eof_does_not_claim_an_unowned_native_source() {
    let _serial = plx_base::testlock::serial();
    let _reset = ResetMedia;
    let mut app = terminal_fixture();
    let generation = admit(&mut app, "first");
    app.ott.as_mut().unwrap().playing.as_mut().unwrap().identity = None;
    signal_error(&app, FailureKind::TvPipeline);
    mark_eof();
    assert!(!finish_catalog_playback(&mut app), "the error guard belongs after current-owner matching");
    let runtime = app.ott.as_ref().unwrap();
    assert_eq!(runtime.play_generation, generation);
    assert!(runtime.play_target.is_none() && runtime.resolver_next.is_none());
    assert_eq!(plx_media::player::state(&app.player.session), PlaybackState::Error);
}

#[test]
fn outgoing_error_eof_does_not_replace_a_pending_viewer_choice() {
    let _serial = plx_base::testlock::serial();
    let _reset = ResetMedia;
    let mut app = terminal_fixture();
    admit(&mut app, "first");
    let generation = queue(&mut app, "second");
    signal_error(&app, FailureKind::TvPipeline);
    mark_eof();
    assert!(finish_catalog_playback(&mut app));
    reconcile(&mut app);
    let runtime = app.ott.as_ref().unwrap();
    assert_eq!(runtime.play_generation, generation);
    assert_eq!(runtime.play_target.as_ref().unwrap().id, "second");
    assert_eq!(runtime.playing.as_ref().unwrap().selection_id, "first");
    assert!(runtime.playback_failure.snapshot().is_none());
}

#[test]
fn exact_live_eof_retains_channel_and_queues_only_same_source_retry() {
    let _serial = plx_base::testlock::serial();
    let _reset = ResetMedia;
    let mut app = terminal_fixture();
    admit(&mut app, "first");
    let runtime = app.ott.as_mut().unwrap();
    runtime.resolver_busy = true;
    let mut live = profile();
    live.kind = ProviderKind::M3u;
    live.source = "https://media.invalid/list.m3u".into();
    let mut channel = entry("first");
    channel.media = MediaLocator::Http { url: PRIVATE_URL.into() };
    channel.duration_ms = None;
    channel.live = Some(Default::default());
    runtime.saved.profiles.profiles = vec![live.clone()];
    runtime.saved.profiles.active_profile = Some(live.id.clone());
    runtime.provider = ProviderKind::M3u;
    runtime.catalog.entries = vec![channel.clone()];
    let playing = runtime.playing.as_mut().unwrap();
    playing.profile = live;
    playing.entry = channel;
    playing.advance_on_failure = false;
    let selected = playing.selection_id.clone();
    let resume_key = playing.profile.id.clone();
    plx_media::player::swap_state_for_test(PlaybackState::Playing);
    mark_eof();
    assert!(finish_catalog_playback(&mut app));
    let runtime = app.ott.as_ref().unwrap();
    assert!(runtime.playing.is_none(), "an ended decoder retains no active playback owner");
    assert_eq!(runtime.checkpoints[&resume_key].entry_id, selected,
        "live EOF checkpoints the selected channel before native teardown clears ownership");
    assert_eq!(failed_playback::current(runtime, &app).unwrap().selection_id, selected);
    assert!(runtime.play_target.is_none() && runtime.resolver_next.is_none());
    let runtime = app.ott.as_mut().unwrap();
    let due = runtime.continuity.pending_retry().unwrap().not_before_ms;
    assert!(matches!(runtime.continuity.tick(runtime.generation, due, &["first", "second"], true),
        continuity::Decision::Start { entry_id, .. } if entry_id == "first"));
}

#[test]
fn cold_restore_normalizes_live_failure_policy_before_resolver_admission() {
    let _serial = plx_base::testlock::serial();
    let _reset = ResetMedia;
    let mut app = terminal_fixture();
    let runtime = app.ott.as_mut().unwrap();
    let mut live = profile();
    live.kind = ProviderKind::M3u;
    live.source = "https://media.invalid/list.m3u".into();
    let mut channel = entry("first");
    channel.media = MediaLocator::Http { url: PRIVATE_URL.into() };
    channel.duration_ms = None;
    channel.live = Some(Default::default());
    runtime.saved.profiles.profiles = vec![live.clone()];
    runtime.saved.profiles.active_profile = Some(live.id);
    runtime.provider = ProviderKind::M3u;
    runtime.catalog.entries = vec![channel];
    let request = PlaybackRequest::ordinary("first", 0);
    assert!(request.advance_on_failure, "model the saved automatic restore default");
    let (request, _, _, _) = runtime.prepare_play_request(request).unwrap();
    assert!(!request.advance_on_failure, "a failed cold restore must not advance to another TV channel");
}

#[test]
fn unresolved_favorite_failure_is_exact_without_changing_vod_eof_policy() {
    let _serial = plx_base::testlock::serial();
    let _reset = ResetMedia;
    let mut app = terminal_fixture();
    let runtime = app.ott.as_mut().unwrap();
    let favorite = selection::capture(&profile(), &entry("first")).unwrap();
    let id = format!("favorite:{}", favorite.identity());
    runtime.saved.favorites.toggle(favorite).unwrap();
    let (request, _, row, _) = runtime.prepare_play_request(PlaybackRequest::ordinary(&id, 0)).unwrap();
    assert!(row.is_none(), "the provider has not resolved this Favorite yet");
    assert!(!request.advance_on_failure, "an unknown failed Favorite cannot authorize a different channel");
    // Normal finite completion is covered by non_error_eof_keeps_short_and_near_end_no_callback_completion.
}
