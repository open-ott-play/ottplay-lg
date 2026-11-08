//! Pre-play observations through actual job admission and authenticated status.
//! Worker jobs and the final native resource callback are modeled; no resolver,
//! inspector, decoder, protected writer or network request is executed here.
use super::*;

const PRIVATE_ERROR: &str = "failed https://preplay-user:preplay-password@preplay.invalid/media?token=preplay-token";

fn profile() -> ProviderProfile {
    ProviderProfile {
        id: "preplay-private-profile".into(), name: "Preplay private profile".into(),
        kind: ProviderKind::VPortal,
        source: "portal::[key:preplay-token]https://preplay.invalid/api".into(),
        source_alias: "Preplay private alias".into(), enabled: true,
    }
}

fn entry(id: &str) -> CatalogEntry {
    CatalogEntry {
        id: id.into(), title: "Preplay private title".into(), category: String::new(),
        description: String::new(), poster: String::new(), duration_ms: Some(10_000),
        media: MediaLocator::VPortal { request: json!({"id": id}) },
        provider_id: Some(id.into()), provider_id_field: Some("id".into()), live: None,
    }
}

fn fixture() -> Box<App> {
    let mut app = Box::new(ready_app_fixture());
    let runtime = app.ott.as_mut().unwrap();
    runtime.saved.profiles.profiles = vec![profile()];
    runtime.saved.profiles.active_profile = Some(profile().id);
    runtime.provider = ProviderKind::VPortal;
    runtime.catalog = Catalog { entries: vec![entry("first"), entry("second")], hierarchy: None };
    runtime.catalog_loading = false;
    runtime.catalog_complete = true;
    runtime.catalog_done.store(false, Ordering::Release);
    runtime.metadata_done.store(false, Ordering::Release);
    runtime.resolver_busy = true;
    runtime.plex_refresh_pending = true;
    runtime.write_permit.revoke();
    runtime.continuity.suppress_unrequested_start();
    app
}

fn begin(app: &mut App, id: &str) -> (u64, preplay::Worker) {
    let mut runtime = app.ott.take().unwrap();
    runtime.resolver_busy = true;
    let mut request = PlaybackRequest::ordinary(id, 0);
    request.advance_on_failure = false;
    runtime.play_request_with_policy(app, request).unwrap();
    // Model the already occupied worker taking precisely the admitted request.
    // Status's existing no-queued-work guard remains intact.
    let queued = runtime.resolver_next.take().expect("admitted resolver request");
    assert_eq!(queued.generation, runtime.play_generation);
    let worker = preplay::worker(&runtime, queued.generation, &queued.profile);
    let generation = queued.generation;
    app.ott = Some(runtime);
    (generation, worker)
}

fn send(app: &App, job: Job) {
    app.ott.as_ref().unwrap().tx.send(job)
        .unwrap_or_else(|_| panic!("fixture job receiver disappeared"));
}

fn read(app: &mut App, action: &str, serial: u64) -> Value {
    let response = super::provider_context::read_status(app, action, serial);
    let value = &response["data"]["preplay"];
    let keys: std::collections::BTreeSet<_> = value.as_object().expect("typed preplay object")
        .keys().map(String::as_str).collect();
    assert_eq!(keys, ["version", "phase", "attempt_generation", "phase_elapsed_ms",
        "last_failure", "cancel_reason"].into_iter().collect());
    assert_eq!(value["version"], 1);
    assert!(value["phase_elapsed_ms"].as_u64().is_some());
    let text = response.to_string();
    for secret in [PRIVATE_ERROR, "preplay-user", "preplay-password", "preplay-token",
        "preplay.invalid", "preplay-private-profile", "Preplay private profile",
        "Preplay private alias", "Preplay private title"] {
        assert!(!text.contains(secret), "status must not expose private pre-play input");
    }
    response
}

fn observation(value: &Value) -> Value {
    let mut value = value["data"]["preplay"].clone();
    value.as_object_mut().unwrap().remove("phase_elapsed_ms");
    value
}

fn ready(id: &str) -> (CatalogEntry, ReadySource) {
    (entry(id), ReadySource::External(
        plx_media::ff::ExternalSource::Http("https://preplay.invalid/media?token=preplay-token".into()),
        unsupported_info(), None))
}

#[test]
fn preplay_progress_uses_job_ingress_and_preserves_legacy_status_and_saved() {
    let _serial = plx_base::testlock::serial();
    let mut app = fixture();
    let before = read(&mut app, "status", 1);
    assert_eq!(before["data"]["preplay"]["phase"], "idle");
    let saved = serde_json::to_value(&app.ott.as_ref().unwrap().saved).unwrap();
    let identity = plx_media::route::playback_identity(&app.player.session);
    let (generation, mut worker) = begin(&mut app, "first");
    worker.phase(preplay::Phase::Resolving);
    runtime_tick(&mut app);
    let resolving = read(&mut app, "status", 2);
    assert_eq!(resolving["data"]["preplay"]["phase"], "resolving");
    assert_eq!(resolving["data"]["preplay"]["attempt_generation"], generation);
    assert_eq!(worker.inspect(|| Ok::<_, &'static str>(())), Ok(()));
    runtime_tick(&mut app);
    let inspecting = read(&mut app, "state", 3);
    assert_eq!(inspecting["data"]["preplay"]["phase"], "inspecting");
    assert_eq!(inspecting["data"]["preplay"]["attempt_generation"], generation);
    assert!(inspecting["data"]["preplay"]["last_failure"].is_null());
    assert!(inspecting["data"]["preplay"]["cancel_reason"].is_null());
    assert_eq!(inspecting["data"]["ready"], resolving["data"]["ready"]);
    assert_eq!(inspecting["data"]["playback"], resolving["data"]["playback"]);
    assert_eq!(serde_json::to_value(&app.ott.as_ref().unwrap().saved).unwrap(), saved);
    assert_eq!(plx_media::route::playback_identity(&app.player.session), identity);
    let runtime = app.ott.as_ref().unwrap();
    assert!(runtime.resolver_next.is_none() && !runtime.saving && runtime.playing.is_none());
}

#[test]
fn preplay_resolve_and_inspect_failures_stay_distinct_after_terminal_job() {
    let _serial = plx_base::testlock::serial();
    for inspecting in [false, true] {
        let mut app = fixture();
        let (generation, mut worker) = begin(&mut app, "first");
        if inspecting {
            assert_eq!(worker.inspect(|| Err::<(), _>(PRIVATE_ERROR)), Err(PRIVATE_ERROR));
        }
        worker.failed();
        send(&app, Job::Play(generation, profile(), Err(PRIVATE_ERROR)));
        let clock = plx_base::task::time::ClockSample::capture_main(app.adapters.player.mt()).unwrap();
        tick_with_start(&mut app, clock, |_, _, _, _, _| panic!("failed worker cannot admit playback"));
        let status = read(&mut app, "status", 1);
        assert_eq!(status["data"]["preplay"]["phase"], "failed");
        assert_eq!(status["data"]["preplay"]["attempt_generation"], generation);
        assert_eq!(status["data"]["preplay"]["last_failure"],
            if inspecting { "inspect_failed" } else { "resolve_failed" });
        assert!(status["data"]["preplay"]["cancel_reason"].is_null());
        assert_eq!(app.ott.as_ref().unwrap().status, PRIVATE_ERROR,
            "closed diagnostics must not replace the existing UI error");
        let state = read(&mut app, "state", 2);
        assert_eq!(observation(&state), observation(&status));
    }
}

#[test]
fn preplay_native_admission_failure_is_not_reported_as_inspect_failure() {
    let _serial = plx_base::testlock::serial();
    let mut app = fixture();
    let (generation, mut worker) = begin(&mut app, "first");
    worker.inspect(|| Ok::<_, &'static str>(())).unwrap();
    send(&app, Job::Play(generation, profile(), Ok(ready("first"))));
    let clock = plx_base::task::time::ClockSample::capture_main(app.adapters.player.mt()).unwrap();
    let mut starts = 0;
    tick_with_start(&mut app, clock, |runtime, _, _, _, _| {
        starts += 1;
        assert_eq!(json!(runtime.preplay.snapshot(runtime.now_ms()))["phase"], "admitting");
        Err(PRIVATE_ERROR)
    });
    assert_eq!(starts, 1, "exercise real pre-admission guards before the injected resource failure");
    let status = read(&mut app, "status", 1);
    assert_eq!(status["data"]["preplay"]["phase"], "failed");
    assert_eq!(status["data"]["preplay"]["last_failure"], "admission_failed");
    assert!(!plx_media::route::has_url(&app.player.session));
    assert!(app.ott.as_ref().unwrap().playing.is_none());
}

#[test]
fn preplay_admitted_means_callback_acceptance_not_playback_progress() {
    let _serial = plx_base::testlock::serial();
    let mut app = fixture();
    let (generation, mut worker) = begin(&mut app, "first");
    worker.inspect(|| Ok::<_, &'static str>(())).unwrap();
    send(&app, Job::Play(generation, profile(), Ok(ready("first"))));
    let clock = plx_base::task::time::ClockSample::capture_main(app.adapters.player.mt()).unwrap();
    let mut starts = 0;
    tick_with_start(&mut app, clock, |runtime, _, _, _, _| {
        starts += 1;
        assert_eq!(json!(runtime.preplay.snapshot(runtime.now_ms()))["phase"], "admitting");
        Ok(())
    });
    assert_eq!(starts, 1);
    let status = read(&mut app, "state", 1);
    assert_eq!(status["data"]["preplay"]["phase"], "admitted");
    assert_eq!(status["data"]["preplay"]["attempt_generation"], generation);
    assert!(status["data"]["preplay"]["last_failure"].is_null());
    assert_eq!(status["data"]["playback"]["state"], "idle");
    assert!(!plx_media::route::has_url(&app.player.session));
}

#[test]
fn preplay_old_worker_cannot_relabel_a_replacement_attempt() {
    let _serial = plx_base::testlock::serial();
    let mut app = fixture();
    let (old, mut worker) = begin(&mut app, "first");
    let (current, _current_worker) = begin(&mut app, "second");
    assert_ne!(old, current);
    let before = read(&mut app, "status", 1);
    worker.phase(preplay::Phase::Inspecting);
    worker.failed();
    send(&app, Job::Play(old, profile(), Err(PRIVATE_ERROR)));
    send(&app, Job::Play(old, profile(), Ok(ready("first"))));
    let clock = plx_base::task::time::ClockSample::capture_main(app.adapters.player.mt()).unwrap();
    tick_with_start(&mut app, clock, |_, _, _, _, _| panic!("retired attempt cannot reach native admission"));
    let after = read(&mut app, "state", 2);
    assert_eq!(observation(&after), observation(&before));
    assert_eq!(after["data"]["preplay"]["attempt_generation"], current);
    assert_eq!(app.ott.as_ref().unwrap().play_target.as_ref().unwrap().id, "second");
}

#[test]
fn preplay_same_generation_changed_captured_source_rejects_old_observations() {
    let _serial = plx_base::testlock::serial();
    let mut app = fixture();
    let (generation, mut worker) = begin(&mut app, "first");
    let before = read(&mut app, "status", 1);
    // Model a changed pending source owner, keeping the same ID and generation.
    // This does not ask diagnostics to re-resolve Saved or to cancel playback.
    let runtime = app.ott.as_mut().unwrap();
    runtime.play_profile.as_mut().unwrap().1.source.push_str("/replacement");
    assert_eq!(runtime.play_generation, generation);
    worker.phase(preplay::Phase::Inspecting);
    worker.failed();
    runtime_tick(&mut app);
    let after = read(&mut app, "state", 2);
    assert_eq!(observation(&after), observation(&before));
    assert!(app.ott.as_ref().unwrap().play_target.is_some(), "observer cannot cancel the changed owner");
}

#[test]
fn preplay_reports_source_rebind_refusal_without_authorizing_or_cancelling_itself() {
    let _serial = plx_base::testlock::serial();
    let mut app = fixture();
    let (generation, mut worker) = begin(&mut app, "first");
    worker.inspect(|| Ok::<_, &'static str>(())).unwrap();
    // The pending owner stays exactly as admitted. Only the real runtime
    // admission rebind may reject an edited Saved source; the observer must not.
    app.ott.as_mut().unwrap().saved.profiles.profiles[0].source.push_str("/replacement");
    runtime_tick(&mut app);
    let inspecting = read(&mut app, "status", 1);
    assert_eq!(inspecting["data"]["preplay"]["phase"], "inspecting");
    assert_eq!(inspecting["data"]["preplay"]["attempt_generation"], generation);
    assert!(app.ott.as_ref().unwrap().play_target.is_some());
    send(&app, Job::Play(generation, profile(), Ok(ready("first"))));
    let clock = plx_base::task::time::ClockSample::capture_main(app.adapters.player.mt()).unwrap();
    tick_with_start(&mut app, clock, |_, _, _, _, _| panic!("changed source cannot reach native start"));
    let status = read(&mut app, "state", 2);
    assert_eq!(status["data"]["preplay"]["phase"], "failed");
    assert_eq!(status["data"]["preplay"]["attempt_generation"], generation);
    assert_eq!(status["data"]["preplay"]["last_failure"], "admission_failed");
    assert!(status["data"]["preplay"]["cancel_reason"].is_null());
    assert!(!plx_media::route::has_url(&app.player.session));
    assert!(app.ott.as_ref().unwrap().playing.is_none());
}

#[test]
fn preplay_cancel_reason_survives_generic_cleanup_and_late_failure() {
    let _serial = plx_base::testlock::serial();
    let mut app = fixture();
    let (generation, mut worker) = begin(&mut app, "first");
    let runtime = app.ott.as_mut().unwrap();
    preplay::cancel(runtime, preplay::CancelReason::UserBack);
    runtime.cancel_play_request();
    assert_ne!(runtime.play_generation, generation);
    worker.phase(preplay::Phase::Inspecting);
    worker.failed();
    send(&app, Job::Play(generation, profile(), Err(PRIVATE_ERROR)));
    runtime_tick(&mut app);
    let status = read(&mut app, "status", 1);
    assert_eq!(status["data"]["preplay"]["phase"], "cancelled");
    assert_eq!(status["data"]["preplay"]["attempt_generation"], generation);
    assert_eq!(status["data"]["preplay"]["cancel_reason"], "user_back");
    assert!(status["data"]["preplay"]["last_failure"].is_null());
    let runtime = app.ott.as_ref().unwrap();
    assert!(runtime.play_target.is_none() && runtime.resolver_next.is_none());
}
