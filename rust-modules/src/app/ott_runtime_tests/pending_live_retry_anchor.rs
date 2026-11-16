//! Exact pending live requests retain their ordinary list proof across a
//! transient preflight refusal. Real request admission is used with the
//! resolver held; no worker, network, storage writer or native decoder starts.
use super::*;

struct ResetMedia;
impl Drop for ResetMedia {
    fn drop(&mut self) {
        plx_media::player::SHARED.reset_session();
        plx_media::player::TX.reset();
        plx_media::route::reset_player_control_for_test(&plx_media::route::PlaybackSession::IDLE);
    }
}

fn fixture(finite: bool) -> (Box<App>, u64) {
    fixture_with_categories(finite, false)
}

fn fixture_with_categories(finite: bool, separate_outgoing: bool) -> (Box<App>, u64) {
    plx_media::player::SHARED.reset_session();
    plx_media::player::TX.reset();
    let mut app = Box::new(crate::app::run::lifecycle_regression_tests::app());
    let mut runtime = Runtime::unloaded(crate::app::ott_companion::Supervisor::inert_for_test());
    runtime.remote = Remote::disabled_for_test("dev_fixture", "fixture");
    runtime.resolver_hold_for_test = true;
    runtime.tx.send(Job::Loaded(Ok((Saved::default(), [1; 16]))))
        .unwrap_or_else(|_| panic!("fixture receiver disappeared"));
    app.ott = Some(Box::new(runtime));
    let clock = plx_base::task::time::ClockSample::capture_main(app.adapters.player.mt()).unwrap();
    super::super::tick(&mut app, clock);
    let mut runtime = app.ott.take().unwrap();
    assert!(runtime.loaded && playback_allowed(&app));
    let profile = ProviderProfile { id: "pending-live-fixture".into(), name: "Fixture".into(),
        kind: ProviderKind::M3u, source: "https://fixture.invalid/list.m3u".into(),
        source_alias: String::new(), enabled: true };
    runtime.saved.profiles.profiles = vec![profile.clone()];
    runtime.saved.profiles.active_profile = Some(profile.id);
    runtime.provider = ProviderKind::M3u;
    runtime.saved.category = if separate_outgoing { "Outgoing" } else { "Playing" }.into();
    runtime.catalog = Catalog { entries: ["first", "second", "other"].into_iter().map(|id| CatalogEntry {
        id: id.into(), title: id.into(), category: if id == "other" { "Other" }
            else if id == "first" && separate_outgoing { "Outgoing" } else { "Playing" }.into(),
        description: String::new(), poster: String::new(), synopsis: None, protection: None,
        duration_ms: finite.then_some(10_000), live: (!finite).then(Default::default),
        media: MediaLocator::Http { url: format!("https://fixture.invalid/{id}.m3u8") },
        provider_id: Some(id.into()), provider_id_field: Some("id".into()),
    }).collect(), hierarchy: None };
    runtime.catalog_revision += 1;
    runtime.catalog_loading = false;
    runtime.catalog_complete = true;
    runtime.catalog_done.store(false, Ordering::Release);
    runtime.metadata_done.store(false, Ordering::Release);
    runtime.resolver_busy = true;
    runtime.plex_refresh_pending = true;
    runtime.write_permit.revoke();
    runtime.continuity.suppress_unrequested_start();
    runtime.play_with_policy(&mut app, "first", false).unwrap();
    let request = runtime.resolver_next.take().expect("actual admitted resolver request");
    let generation = request.generation;
    assert_eq!(generation, runtime.play_generation);
    assert!(!runtime.play_target.as_ref().unwrap().advance_on_failure);
    assert_eq!(runtime.resolver_launches_for_test, 0);
    app.ott = Some(runtime);
    (app, generation)
}

#[derive(Default)]
struct AcceptedResources { starts: usize }
impl crate::app::playback::PlaybackResources for AcceptedResources {
    fn request_movie(&mut self, _: &mut plx_media::route::PlaybackSession,
        _: &mut plx_data::stores::metadata::MetadataStore, _: &plx_data::pms::PmsMovie,
        _: Option<plx_plex::plex::MediaId>) -> bool { panic!("external fixture only") }
    fn request_episode(&mut self, _: &mut plx_media::route::PlaybackSession,
        _: &mut plx_data::stores::metadata::MetadataStore, _: &str,
        _: Option<plx_plex::plex::MediaId>) -> bool { panic!("external fixture only") }
    fn describe_movie(&mut self, _: &mut plx_data::stores::metadata::MetadataStore,
        _: plx_plex::plex::ServerId, _: &str) { panic!("external fixture only") }
    fn prepare_start(&mut self, _: &mut plx_media::route::PlaybackSession,
        _: &mut plx_media::player::adapter::PlayerAdapter, _: i64) -> bool {
        self.starts += 1;
        true
    }
}

fn admit_first_without_decoder(app: &mut App, generation: u64) {
    let runtime = app.ott.as_ref().unwrap();
    let profile = runtime.effective_profile().unwrap().clone();
    let entry = runtime.catalog.entries.iter().find(|entry| entry.id == "first").unwrap().clone();
    let MediaLocator::Http { url } = &entry.media else { panic!("HTTP fixture") };
    let source = ReadySource::External(plx_media::ff::ExternalSource::Hls(url.clone()),
        plx_media::ff::inspect::MediaInfo {
            tracks: Default::default(), resolved_source: None, hls_playback: None,
            video_codec: "h264".into(), audio_codec: "aac".into(), width: 1280, height: 720,
            fps: 24.0, duration_ns: 0, audio_channels: 2, audio_sample_rate: 48_000,
            audio_bit_depth: 16, dovi: Default::default(),
        }, None);
    runtime.tx.send(Job::Play(generation, profile, Ok((entry, source))))
        .unwrap_or_else(|_| panic!("fixture receiver disappeared"));
    let clock = plx_base::task::time::ClockSample::capture_main(app.adapters.player.mt()).unwrap();
    let mut resources = AcceptedResources::default();
    super::super::tick_with_start(app, clock, |runtime, app, profile, entry, source|
        super::super::start_resolved_with(runtime, app, profile, entry, source, &mut resources));
    assert_eq!(resources.starts, 1, "real logical admission uses only the inert resource seam");
    let runtime = app.ott.as_ref().unwrap();
    assert!(playing_matches(runtime.playing.as_ref().unwrap(), app));
    assert_eq!(runtime.playing.as_ref().unwrap().selection_id, "first");
    assert!(runtime.play_target.is_none() && runtime.resolver_next.is_none());
    assert!(runtime.explicit_step.admitted.is_some());
    assert_eq!(runtime.resolver_launches_for_test, 0);
    assert!(!app.adapters.player.is_live(), "logical route admission never starts an Engine");
    app.ott.as_mut().unwrap().resolver_busy = true;
}

fn settle_pending(runtime: &mut Runtime, app: &App, generation: u64) {
    // The real handler captures before taking this ticket/target. Resolution
    // retires its input anchor, and only the captured continuation may retain it.
    runtime.play_ticket.take();
    resolved(runtime, app, generation, true);
    assert!(runtime.explicit_step.anchor.is_none());
    assert_eq!(runtime.take_completed_play_target().unwrap().id, "first");
    assert!(runtime.play_target.is_none() && runtime.resolver_next.is_none());
}

fn held_retry(app: &mut App, generation: u64) {
    let mut runtime = app.ott.take().unwrap();
    let proof = capture_pending_live_retry(&runtime, app, generation).expect("current pending live proof");
    let ids = continuation_ids(&runtime).expect("pending source list");
    settle_pending(&mut runtime, app, generation);
    with_failure_continuation(&mut runtime, Some(proof), |runtime| {
        let decision = runtime.continuity.live_disconnected(runtime.generation, "first", runtime.now_ms());
        assert_eq!(decision, continuity::Decision::Wait);
        assert!(continue_at(runtime, "second", 0).is_err(), "even a synchronous continuation cannot borrow another row");
        runtime.apply_continuity(decision);
    });
    assert!(runtime.explicit_step.anchor.as_ref().is_some_and(|anchor| anchor.failed && anchor.retry_exact));
    assert!(runtime.explicit_step.retry_exact_scope.is_none());
    assert_eq!(continuation_ids(&runtime), Some(ids));
    assert_eq!(runtime.continuity.pending_retry().unwrap().after, "first");
    assert_eq!(runtime.resolver_launches_for_test, 0);
    assert!(runtime.resolver_next.is_none());
    assert!(!app.adapters.player.is_live());
    app.ott = Some(runtime);
}

#[test]
fn exact_pending_live_request_has_a_same_source_retry_proof_without_finite_advance_authority() {
    let _serial = plx_base::testlock::serial();
    let _reset = ResetMedia;
    let (app, generation) = fixture(false);
    let runtime = app.ott.as_ref().unwrap();
    assert!(capture_pending_failure(runtime, &app, generation).is_none());
    let proof = capture_pending_live_retry(runtime, &app, generation).unwrap();
    let anchor = proof.anchor.unwrap();
    assert!(anchor.retry_exact && !anchor.failed && !anchor.request.advance_on_failure);
    assert_eq!(anchor.request.id, "first");
    assert!(proof.list.is_some());
    assert_eq!(runtime.resolver_launches_for_test, 0);
}

#[test]
fn retained_pending_live_retry_queues_only_its_exact_original_request() {
    let _serial = plx_base::testlock::serial();
    let _reset = ResetMedia;
    let (mut app, generation) = fixture(false);
    held_retry(&mut app, generation);
    let runtime = app.ott.as_mut().unwrap();
    assert!(continue_at(runtime, "second", 0).is_err());
    assert!(runtime.play_target.is_none() && runtime.resolver_next.is_none());
    continue_at(runtime, "first", 0).unwrap();
    assert_eq!(runtime.play_target.as_ref().unwrap().id, "first");
    assert!(!runtime.play_target.as_ref().unwrap().advance_on_failure);
    assert_eq!(runtime.resolver_next.as_ref().unwrap().generation, runtime.play_generation);
    assert_ne!(runtime.play_generation, generation);
    assert_eq!(runtime.resolver_launches_for_test, 0);
}

#[test]
fn pending_live_retry_cannot_capture_a_stale_source_request_or_ticket() {
    let _serial = plx_base::testlock::serial();
    let _reset = ResetMedia;
    for changed in ["navigation", "playback", "generation", "request", "profile", "entry", "order", "cancel", "ticket"] {
        let (mut app, generation) = fixture(false);
        let mut runtime = app.ott.take().unwrap();
        match changed {
            "navigation" => runtime.navigation_epoch += 1,
            "playback" => runtime.playback_epoch += 1,
            "generation" => runtime.generation += 1,
            "request" => runtime.play_generation += 1,
            "profile" => runtime.saved.profiles.profiles[0].source = "https://fixture.invalid/replaced.m3u".into(),
            "entry" => runtime.catalog.entries[0].media = MediaLocator::Http { url: "https://fixture.invalid/replaced.m3u8".into() },
            "order" => { runtime.catalog.entries.swap(0, 1); runtime.catalog_revision += 1; }
            "cancel" => runtime.play_cancel.store(true, Ordering::Release),
            "ticket" => plx_media::route::cancel_play(&mut app.player.session),
            _ => unreachable!(),
        }
        assert!(capture_pending_live_retry(&runtime, &app, generation).is_none(), "{changed}");
        assert_eq!(runtime.resolver_launches_for_test, 0);
        assert!(!app.adapters.player.is_live());
        app.ott = Some(runtime);
    }
}

#[test]
fn pending_live_retry_proof_does_not_change_finite_request_failure_policy() {
    let _serial = plx_base::testlock::serial();
    let _reset = ResetMedia;
    let (app, generation) = fixture(true);
    let runtime = app.ott.as_ref().unwrap();
    assert!(capture_pending_live_retry(runtime, &app, generation).is_none());
    assert!(capture_pending_failure(runtime, &app, generation).is_none());
    assert!(!runtime.play_target.as_ref().unwrap().advance_on_failure);
}

#[test]
fn retained_pending_live_retry_fails_closed_when_the_original_binding_changes() {
    let _serial = plx_base::testlock::serial();
    let _reset = ResetMedia;
    let (mut app, generation) = fixture(false);
    held_retry(&mut app, generation);
    let runtime = app.ott.as_mut().unwrap();
    runtime.catalog.entries[0].media = MediaLocator::Http { url: "https://fixture.invalid/replaced.m3u8".into() };
    runtime.catalog_revision += 1;
    assert!(continuation_denied(runtime));
    assert!(continue_at(runtime, "first", 0).is_err());
    assert!(runtime.play_target.is_none() && runtime.resolver_next.is_none());
    assert_eq!(runtime.resolver_launches_for_test, 0);
}

#[test]
fn newer_explicit_selection_cannot_be_replaced_by_an_older_live_retry_proof() {
    let _serial = plx_base::testlock::serial();
    let _reset = ResetMedia;
    let (mut app, generation) = fixture(false);
    let mut runtime = app.ott.take().unwrap();
    let proof = capture_pending_live_retry(&runtime, &app, generation).unwrap();
    runtime.play_with_policy(&mut app, "second", false).unwrap();
    let newer = runtime.play_generation;
    with_failure_continuation(&mut runtime, Some(proof), |runtime| {
        let decision = runtime.continuity.live_disconnected(runtime.generation, "first", runtime.now_ms());
        assert_eq!(decision, continuity::Decision::Idle);
        runtime.apply_continuity(decision);
    });
    assert_eq!(runtime.play_generation, newer);
    assert_eq!(runtime.play_target.as_ref().unwrap().id, "second");
    assert_eq!(runtime.resolver_next.as_ref().unwrap().generation, newer);
    assert_eq!(runtime.explicit_step.anchor.as_ref().unwrap().request.id, "second");
    assert!(runtime.explicit_step.retry_exact_scope.is_none());
    assert_eq!(runtime.resolver_launches_for_test, 0);
    app.ott = Some(runtime);
}

#[test]
fn outgoing_admitted_source_cannot_replace_the_retained_exact_retry_destination() {
    let _serial = plx_base::testlock::serial();
    let _reset = ResetMedia;
    let (mut app, first_generation) = fixture_with_categories(false, true);
    admit_first_without_decoder(&mut app, first_generation);
    let mut runtime = app.ott.take().unwrap();
    let outgoing = plx_media::route::playback_identity(&app.player.session).unwrap();
    assert_eq!(runtime.explicit_step.admitted.as_ref().unwrap().list.ids, ["first"]);
    // Accept the same category Saved image used by the UI, without invoking a
    // protected writer. The outgoing source keeps its own one-item list.
    let candidate = runtime.category_choice_candidate(Some("Playing")).unwrap();
    runtime.pending_category_choice = Some(category_state::Pending::capture(&runtime, Some("Playing")));
    runtime.saved = candidate;
    assert!(runtime.finish_category_choice(&app, true));
    assert_eq!(commands::selected_ids(&runtime), ["second"]);
    runtime.play_with_policy(&mut app, "second", false).unwrap();
    let pending = runtime.resolver_next.take().unwrap();
    let generation = pending.generation;
    let proof = capture_pending_live_retry(&runtime, &app, generation).unwrap();
    assert_eq!(proof.anchor.as_ref().unwrap().outgoing.as_ref(), Some(&outgoing));
    assert_eq!(proof.list.as_ref().unwrap().ids, ["second"]);
    runtime.play_ticket.take();
    resolved(&mut runtime, &app, generation, true);
    let retained = runtime.explicit_step.anchor.as_ref().unwrap();
    assert!(retained.failed && !retained.retry_exact, "outgoing playback retains its refused pending anchor");
    assert_eq!(runtime.take_completed_play_target().unwrap().id, "second");
    with_failure_continuation(&mut runtime, Some(proof), |runtime| {
        let decision = runtime.continuity.live_disconnected(runtime.generation, "second", runtime.now_ms());
        assert_eq!(decision, continuity::Decision::Wait);
        assert!(continue_at(runtime, "first", 0).is_err());
        runtime.apply_continuity(decision);
    });
    assert!(runtime.explicit_step.anchor.as_ref().is_some_and(|anchor|
        anchor.failed && anchor.retry_exact && anchor.request.id == "second"));
    assert_eq!(continuation_ids(&runtime).unwrap(), ["second"], "never borrow the outgoing source's list");
    assert!(continue_at(&mut runtime, "first", 0).is_err());
    continue_at(&mut runtime, "second", 0).unwrap();
    assert_eq!(runtime.resolver_next.as_ref().unwrap().entry.as_ref().unwrap().id, "second");
    assert!(!runtime.play_target.as_ref().unwrap().advance_on_failure);
    assert_eq!(runtime.playing.as_ref().unwrap().selection_id, "first");
    assert_eq!(plx_media::route::playback_identity(&app.player.session), Some(outgoing));
    assert_eq!(runtime.resolver_launches_for_test, 0);
    assert!(!app.adapters.player.is_live());
    app.ott = Some(runtime);
}
