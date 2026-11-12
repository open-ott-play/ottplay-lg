//! Selection durability has a shorter contract than periodic playhead updates.
//! The writer permit is revoked: these tests exercise actual write admission
//! without touching protected host/device state or opening any media source.
use super::*;

fn changed_selection(app: &mut App) -> String {
    local_playing(app, None, 3_700);
    let runtime = app.ott.as_mut().unwrap();
    let profile = runtime.active().unwrap().clone();
    runtime.catalog.entries.insert(0, local_entry("earlier"));
    runtime.saved.resume.insert(profile.id.clone(), SavedResume {
        entry_id: "earlier".into(), position_ms: 0,
        source_fingerprint: profile_fingerprint(&profile),
    });
    runtime.last_checkpoint_ms = runtime.now_ms();
    runtime.saving = false;
    runtime.write_permit.revoke();
    profile.id
}

#[test]
fn new_selection_checkpoints_inside_position_interval_and_cold_reload_restores_non_first() {
    let _serial = plx_base::testlock::serial();
    let _reset = ResetMedia;
    let mut app = ready_app_fixture();
    let key = changed_selection(&mut app);
    let mut runtime = app.ott.take().unwrap();
    runtime.checkpoint(&app, false);
    assert!(runtime.saving, "a new selection must enter the existing writer immediately");
    assert_eq!(runtime.checkpoints[&key].entry_id, "first");
    assert_eq!(runtime.checkpoints[&key].position_ms, 3_700);
    // Rehydrate the same checkpoint merge used by the verified writer and
    // shutdown. No live Runtime or selected row is carried into cold startup.
    let mut after = runtime.saved.clone();
    kiosk_resume::merge(&mut after, &runtime.checkpoints);
    let persisted = serde_json::to_vec(&after).unwrap();
    let mut cold = runtime_fixture();
    cold.clock_sample = runtime.clock_sample;
    cold.wall_clock_ms = runtime.wall_clock_ms;
    cold.saved = serde_json::from_slice(&persisted).unwrap();
    cold.sync_saved_context();
    cold.load_catalog();
    let generation = cold.generation;
    assert!(matches!(cold.continuity.catalog_updated(generation, &["earlier", "first"], true),
        continuity::Decision::Start { entry_id, position_ms: 3_700,
            reason: continuity::Reason::Restore } if entry_id == "first"));
}

#[test]
fn same_source_playhead_updates_keep_the_ten_second_write_interval() {
    let _serial = plx_base::testlock::serial();
    let _reset = ResetMedia;
    let mut app = ready_app_fixture();
    let key = changed_selection(&mut app);
    let mut runtime = app.ott.take().unwrap();
    runtime.saved.resume.get_mut(&key).unwrap().entry_id = "first".into();
    runtime.checkpoint(&app, false);
    assert_eq!(runtime.checkpoints[&key].position_ms, 3_700);
    assert!(!runtime.saving, "playhead-only progress retains the position throttle");
    runtime.last_checkpoint_ms = runtime.now_ms().saturating_sub(10_000);
    runtime.checkpoint(&app, false);
    assert!(runtime.saving, "ordinary progress enters the writer once its interval expires");
}

#[test]
fn changed_source_checkpoint_does_not_wait_for_position_interval() {
    let _serial = plx_base::testlock::serial();
    let _reset = ResetMedia;
    let mut app = ready_app_fixture();
    let key = changed_selection(&mut app);
    let mut runtime = app.ott.take().unwrap();
    let old = runtime.saved.resume.get_mut(&key).unwrap();
    old.entry_id = "first".into();
    old.source_fingerprint = "previous-source".into();
    runtime.checkpoint(&app, false);
    assert!(runtime.saving, "a newly admitted source must replace its old bookmark promptly");
    assert_eq!(runtime.checkpoints[&key].source_fingerprint,
        profile_fingerprint(runtime.active().unwrap()));
}


#[derive(Default)]
struct LiveEdgeResources { resume_ns: Option<i64> }
impl playback::PlaybackResources for LiveEdgeResources {
    fn request_movie(&mut self, _: &mut plx_media::route::PlaybackSession,
        _: &mut plx_data::stores::metadata::MetadataStore, _: &plx_data::pms::PmsMovie,
        _: Option<plx_plex::plex::MediaId>) -> bool { panic!("external live only") }
    fn request_episode(&mut self, _: &mut plx_media::route::PlaybackSession,
        _: &mut plx_data::stores::metadata::MetadataStore, _: &str,
        _: Option<plx_plex::plex::MediaId>) -> bool { panic!("external live only") }
    fn describe_movie(&mut self, _: &mut plx_data::stores::metadata::MetadataStore,
        _: plx_plex::plex::ServerId, _: &str) { panic!("external live only") }
    fn prepare_start(&mut self, _: &mut plx_media::route::PlaybackSession,
        _: &mut plx_media::player::adapter::PlayerAdapter, resume_ns: i64) -> bool {
        self.resume_ns = Some(resume_ns); true
    }
}

#[test]
fn historical_m3u_non_first_resume_admits_live_edge_and_checkpoints_zero_offset() {
    let _serial = plx_base::testlock::serial();
    let _reset = ResetMedia;
    let mut app = ready_app_fixture();
    let mut runtime = runtime_fixture();
    runtime.clock_sample = app.ott.as_ref().unwrap().clock_sample;
    runtime.wall_clock_ms = app.ott.as_ref().unwrap().wall_clock_ms;
    let profile = ProviderProfile { id: "m3u-resume".into(), name: "Live".into(),
        kind: ProviderKind::M3u, source: "https://resume.invalid/list.m3u".into(),
        source_alias: String::new(), enabled: true };
    let source = plx_media::ff::ExternalSource::Http("https://resume.invalid/channel".into());
    let mut channel = local_entry("saved-second");
    channel.media = MediaLocator::Http { url: "https://resume.invalid/channel".into() };
    channel.duration_ms = None;
    channel.live = Some(Default::default());
    runtime.loaded = true;
    runtime.saved.profiles.profiles = vec![profile.clone()];
    runtime.saved.profiles.active_profile = Some(profile.id.clone());
    runtime.saved.resume.insert(profile.id.clone(), SavedResume {
        entry_id: channel.id.clone(), position_ms: 7_300,
        source_fingerprint: profile_fingerprint(&profile),
    });
    runtime.sync_saved_context();
    runtime.load_catalog();
    let generation = runtime.generation;
    assert!(matches!(runtime.continuity.catalog_updated(generation, &["earlier", "saved-second"], true),
        continuity::Decision::Start { entry_id, position_ms: 7_300,
            reason: continuity::Reason::Restore } if entry_id == channel.id));
    runtime.play_target = Some(PlaybackRequest::ordinary(&channel.id, 7_300));
    runtime.restore_play_generation = Some(runtime.play_generation);
    runtime.saving = true; // hold metadata/persistence slots; no worker needed
    let mut info = unsupported_info();
    info.video_codec = "h264".into(); info.audio_codec = "aac".into();
    info.audio_channels = 2; info.audio_sample_rate = 48_000;
    info.duration_ns = 60_000_000_000; // even a misleading finite probe cannot seek live TV
    let mut resources = LiveEdgeResources::default();
    start_resolved_with(&mut runtime, &mut app, profile.clone(), channel.clone(),
        ReadySource::External(source, info, None), &mut resources).unwrap();
    assert_eq!(resources.resume_ns, Some(0));
    assert_eq!(runtime.playing.as_ref().unwrap().selection_id, channel.id);
    assert!(!runtime.continuity.issued_restore(generation, &channel.id, 7_300),
        "normalizing live edge still consumes the exact original restore receipt");
    runtime.restore_play_generation = None; // ordinary completed Job boundary
    runtime.checkpoint(&app, false);
    assert_eq!(runtime.checkpoints[&profile.id].entry_id, channel.id);
    assert_eq!(runtime.checkpoints[&profile.id].position_ms, 0);
}


#[test]
fn synchronous_live_restore_refusal_never_attempts_a_different_channel() {
    let _serial = plx_base::testlock::serial();
    let _reset = ResetMedia;
    let app = ready_app_fixture();
    for live in [true, false] {
        let mut runtime = runtime_fixture();
        runtime.clock_sample = app.ott.as_ref().unwrap().clock_sample;
        runtime.wall_clock_ms = app.ott.as_ref().unwrap().wall_clock_ms;
        let profile = ProviderProfile { id: "restore-refusal".into(), name: "Fixture".into(),
            kind: ProviderKind::M3u, source: "https://resume.invalid/list.m3u".into(),
            source_alias: String::new(), enabled: true };
        runtime.saved.profiles.profiles = vec![profile.clone()];
        runtime.saved.profiles.active_profile = Some(profile.id);
        runtime.sync_saved_context();
        let mut channel = local_entry("saved");
        channel.media = MediaLocator::Http { url: "https://resume.invalid/channel".into() };
        channel.live = live.then(Default::default);
        channel.duration_ms = if live { None } else { Some(60_000) };
        let mut earlier = channel.clone(); earlier.id = "earlier".into();
        runtime.catalog.entries = vec![earlier, channel];
        runtime.catalog_complete = true;
        runtime.continuity = continuity::Continuity::new(runtime.generation,
            Some(continuity::ResumePoint { entry_id: "saved".into(), position_ms: 0 }));
        let decision = runtime.continuity.catalog_updated(runtime.generation, &["earlier", "saved"], true);
        let before = runtime.play_generation;
        // The ordinary source-work gate refuses synchronously. No producer or
        // decoder can start, and each attempted continuation retires a ticket.
        runtime.apply_continuity(decision);
        let attempts = runtime.play_generation.wrapping_sub(before);
        if live {
            assert_eq!(attempts, 1, "failed live restore cannot fall through to the first channel");
        } else {
            assert!(attempts > 1, "finite playback keeps its existing failed-item traversal");
        }
        assert!(runtime.play_target.is_none() && runtime.resolver_next.is_none());
    }
}


#[test]
fn rejected_checkpoint_writer_backs_off_same_selection_but_new_choice_bypasses() {
    let _serial = plx_base::testlock::serial();
    let _reset = ResetMedia;
    let mut app = ready_app_fixture();
    changed_selection(&mut app);
    let mut runtime = app.ott.take().unwrap();
    runtime.checkpoint(&app, false);
    assert!(runtime.saving);
    // Model the existing rejected-writer completion: the before-image remains
    // durable, and the write slot is released. The revoked permit prevents IO.
    runtime.saving = false;
    runtime.checkpoint(&app, false);
    assert!(!runtime.saving, "the same failed selection must wait for the ordinary interval");
    let playing = runtime.playing.as_mut().unwrap();
    playing.selection_id = "another-choice".into();
    playing.entry.id = "another-choice".into();
    runtime.checkpoint(&app, false);
    assert!(runtime.saving, "a new source-owned selection has its own immediate attempt");
}

#[test]
fn selection_arriving_while_writer_busy_keeps_its_immediate_attempt() {
    let _serial = plx_base::testlock::serial();
    let _reset = ResetMedia;
    let mut app = ready_app_fixture();
    changed_selection(&mut app);
    let mut runtime = app.ott.take().unwrap();
    runtime.checkpoint(&app, false);
    assert!(runtime.saving);
    let playing = runtime.playing.as_mut().unwrap();
    playing.selection_id = "choice-while-busy".into();
    playing.entry.id = "choice-while-busy".into();
    runtime.checkpoint(&app, false);
    assert_eq!(runtime.checkpoint_write_selection.as_ref().unwrap().1, "first",
        "a busy writer cannot spend the pending selection's bypass");
    runtime.saving = false;
    runtime.checkpoint(&app, false);
    assert!(runtime.saving);
    assert_eq!(runtime.checkpoint_write_selection.as_ref().unwrap().1, "choice-while-busy");
}


#[test]
fn synchronously_refused_checkpoint_write_also_backs_off_same_selection() {
    let _serial = plx_base::testlock::serial();
    let _reset = ResetMedia;
    let mut app = ready_app_fixture();
    changed_selection(&mut app);
    let mut runtime = app.ott.take().unwrap();
    runtime.state_load = StateLoad::Failed("fixture write refusal");
    runtime.checkpoint(&app, false);
    assert!(!runtime.saving, "the real settings gate refuses before spawning a writer");
    assert_eq!(runtime.checkpoint_write_selection.as_ref().unwrap().1, "first");
    let prior_attempt = runtime.now_ms().saturating_sub(1);
    runtime.last_checkpoint_ms = prior_attempt;
    runtime.checkpoint(&app, false);
    assert_eq!(runtime.last_checkpoint_ms, prior_attempt,
        "a synchronous refusal must not trigger the selection bypass again next frame");
    let playing = runtime.playing.as_mut().unwrap();
    playing.selection_id = "new-after-refusal".into();
    playing.entry.id = "new-after-refusal".into();
    runtime.checkpoint(&app, false);
    assert_eq!(runtime.checkpoint_write_selection.as_ref().unwrap().1, "new-after-refusal");
}
