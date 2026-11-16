//! Pure pre-play observation tests; no resolver, network or decoder work.
use super::*;
use serde_json::{json, Value};
use std::sync::mpsc;

fn binding(generation: u64) -> Arc<Binding> {
    Arc::new(Binding { generation, catalog_generation: 7, runtime_token: "fixture".into(), profile: ProviderProfile {
        id: "private-profile".into(), name: "private-title".into(),
        kind: ProviderKind::VPortal,
        source: "portal::[key:private-token]https://private.invalid/path".into(),
        source_alias: String::new(), enabled: true,
    }, request: PlaybackRequest::ordinary("private-item", 0), scope: Scope::VPortal })
}

fn live_hls_binding(generation: u64) -> Arc<Binding> {
    let mut owner = (*binding(generation)).clone();
    owner.profile.kind = ProviderKind::M3u;
    owner.scope = Scope::M3uLiveHlsCandidate;
    Arc::new(owner)
}
fn value(state: &State, now: u64) -> Value {
    serde_json::to_value(state.snapshot(now)).unwrap()
}
fn event(binding: &Arc<Binding>, update: Update) -> Event {
    Event { binding: binding.clone(), update }
}
fn next(receiver: &mpsc::Receiver<Job>) -> Event {
    match receiver.try_recv().ok().unwrap() {
        Job::Preplay(event) => event,
        _ => panic!("unexpected worker job"),
    }
}

#[test]
fn snapshot_has_exact_closed_fields_and_never_serializes_binding() {
    let mut state = State::default();
    assert_eq!(value(&state, 100), json!({"version":1,"phase":"idle",
        "attempt_generation":null,"phase_elapsed_ms":0,"last_failure":null,"cancel_reason":null}));
    state.begin(binding(7), 100);
    assert_eq!(value(&state, 120), json!({"version":1,"phase":"resolving",
        "attempt_generation":7,"phase_elapsed_ms":20,"last_failure":null,"cancel_reason":null}));
}

#[test]
fn phases_use_supplied_time_ignore_duplicates_and_freeze_terminal_duration() {
    let owner = binding(7);
    let mut state = State::default();
    state.begin(owner.clone(), 100);
    state.apply(event(&owner, Update::Phase(Phase::Resolving)), 110);
    assert_eq!(value(&state, 120)["phase_elapsed_ms"], 20);
    state.apply(event(&owner, Update::Phase(Phase::Inspecting)), 120);
    assert_eq!(value(&state, 110)["phase_elapsed_ms"], 0);
    state.apply(event(&owner, Update::Failed(Failure::InspectFailed)), 145);
    let terminal = value(&state, 145);
    state.apply(event(&owner, Update::Phase(Phase::Admitting)), 150);
    state.apply(event(&owner, Update::Failed(Failure::AdmissionFailed)), 155);
    state.apply(event(&owner, Update::Cancelled(CancelReason::Cancelled)), 160);
    assert_eq!(value(&state, 900), terminal);
    assert_eq!(terminal["phase_elapsed_ms"], 25);
    assert_eq!(terminal["last_failure"], "inspect_failed");
}

#[test]
fn fresh_attempt_rejects_old_events_even_with_equal_binding_values() {
    let old = binding(7);
    let fresh = binding(7);
    let mut state = State::default();
    state.begin(old.clone(), 10);
    state.apply(event(&old, Update::Cancelled(CancelReason::Superseded)), 20);
    state.begin(fresh.clone(), 30);
    state.apply(event(&old, Update::Failed(Failure::ResolveFailed)), 40);
    assert_eq!(value(&state, 40)["phase"], "resolving");
    state.apply(event(&fresh, Update::Phase(Phase::Inspecting)), 45);
    assert_eq!(value(&state, 46)["phase"], "inspecting");
}

#[test]
fn binding_requires_full_profile_request_and_generation() {
    let owner = binding(7);
    assert!(owner.matches(7, Some(&owner.profile), Some(&owner.request)));
    assert!(!owner.matches(8, Some(&owner.profile), Some(&owner.request)));
    let mut profile = owner.profile.clone();
    profile.source.push_str("/changed");
    assert!(!owner.matches(7, Some(&profile), Some(&owner.request)));
    let mut request = owner.request.clone();
    request.position_ms = 100;
    assert!(!owner.matches(7, Some(&owner.profile), Some(&request)));
    assert!(!owner.matches(7, None, Some(&owner.request)));
    assert!(!owner.matches(7, Some(&owner.profile), None));
}

#[test]
fn inspect_boundary_preserves_error_and_reports_the_operation_stage() {
    let owner = binding(7);
    let (sender, receiver) = mpsc::channel();
    let mut worker = Worker { binding: Some(owner.clone()), sender, phase: Phase::Resolving };
    let mut state = State::default();
    state.begin(owner, 0);
    let error = "provider-private-error";
    let result: Result<(), _> = worker.inspect(|| Err(error));
    assert_eq!(result, Err(error));
    state.apply(next(&receiver), 10);
    state.apply(next(&receiver), 40);
    worker.failed(); // The outer panic/result boundary may also report it.
    state.apply(next(&receiver), 50);
    let snapshot = value(&state, 100);
    assert_eq!(snapshot["last_failure"], "inspect_failed");
    assert_eq!(snapshot["phase_elapsed_ms"], 30);
    assert!(!snapshot.to_string().contains(error));
}

#[test]
fn successful_inspect_preserves_value_and_does_not_report_failure() {
    let (sender, receiver) = mpsc::channel();
    let mut worker = Worker { binding: Some(binding(7)), sender, phase: Phase::Resolving };
    assert_eq!(worker.inspect(|| Ok(42)), Ok(42));
    assert!(matches!(next(&receiver).update, Update::Phase(Phase::Inspecting)));
    assert!(receiver.try_recv().is_err());
    worker.binding = None;
    assert_eq!(worker.inspect(|| Ok(43)), Ok(43));
    assert!(receiver.try_recv().is_err());
}

#[test]
fn failure_before_inspection_is_resolve_failed() {
    let owner = binding(7);
    let (sender, receiver) = mpsc::channel();
    let worker = Worker { binding: Some(owner.clone()), sender, phase: Phase::Resolving };
    let mut state = State::default();
    state.begin(owner, 10);
    worker.failed();
    state.apply(next(&receiver), 20);
    assert_eq!(value(&state, 90)["last_failure"], "resolve_failed");
}

#[test]
fn quality_write_failure_keeps_retry_open_then_admission_is_terminal() {
    let owner = binding(7);
    let mut state = State::default();
    state.begin(owner.clone(), 0);
    state.apply(event(&owner, Update::Phase(Phase::AwaitingQuality)), 10);
    state.apply(event(&owner, Update::Failed(Failure::QualitySaveFailed)), 20);
    assert_eq!(value(&state, 20)["phase"], "awaiting_quality");
    assert_eq!(value(&state, 20)["last_failure"], "quality_save_failed");
    state.apply(event(&owner, Update::Phase(Phase::Inspecting)), 30);
    state.apply(event(&owner, Update::Phase(Phase::Admitting)), 40);
    state.apply(event(&owner, Update::Phase(Phase::Admitted)), 45);
    state.apply(event(&owner, Update::Cancelled(CancelReason::UserBack)), 50);
    assert_eq!(value(&state, 60)["phase"], "admitted");
    assert_eq!(value(&state, 60)["phase_elapsed_ms"], 5);
}

#[test]
fn m3u_candidate_is_hidden_until_hls_and_reports_bounded_phase_durations() {
    let owner = live_hls_binding(8);
    let mut state = State::default();
    state.begin(owner.clone(), 10);
    assert_eq!(value(&state, 20)["phase"], "idle");

    state.apply(event(&owner, Update::LiveHls), 20);
    state.apply(event(&owner, Update::Phase(Phase::Inspecting)), 30);
    state.apply(event(&owner, Update::Phase(Phase::Admitting)), 55);
    state.apply(event(&owner, Update::Phase(Phase::Admitted)), 90);
    assert_eq!(value(&state, 100), json!({
        "version": 1,
        "phase": "admitted",
        "attempt_generation": 8,
        "phase_elapsed_ms": 35,
        "phase_durations_ms": {
            "resolving_ms": 20,
            "inspecting_ms": 25,
            "admitting_ms": 35
        },
        "total_elapsed_ms": 80,
        "last_failure": null,
        "cancel_reason": null
    }));
}

#[test]
fn inspection_causes_are_closed_exact_and_do_not_classify_private_substrings() {
    for (reason, expected) in [
        ("Media source request timed out", "timeout"),
        ("Selected HLS playlist request timed out", "timeout"),
        ("Media source network transport failed", "transport"),
        ("HLS playlist transient transport failed", "transport"),
        ("HLS playlist server returned HTTP 403", "http_403"),
        ("Could not read media source", "media_read_failed"),
        ("Unsupported or unavailable media container", "media_container_unavailable"),
        ("Media inspection cancelled or exceeded its limit", "inspection_stopped"),
        ("private https://user:password@invalid/?token=Media source request timed out", "unknown"),
        ("Media source request timed out extra", "unknown"),
    ] {
        assert_eq!(serde_json::to_value(inspection_cause(reason)).unwrap(), expected);
    }
}

#[test]
fn original_inspection_cause_survives_outer_failure_and_is_absent_from_legacy_snapshot() {
    let owner = live_hls_binding(7);
    let (sender, receiver) = mpsc::channel();
    let mut worker = Worker { binding: Some(owner.clone()), sender, phase: Phase::Resolving };
    let mut state = State::default();
    state.begin(owner, 0);
    let before = value(&state, 0);
    let result: Result<(), _> = worker.inspect(|| Err("Media source network transport failed"));
    assert_eq!(result, Err("Media source network transport failed"));
    state.apply(next(&receiver), 10);
    state.apply(next(&receiver), 20);
    worker.failed();
    state.apply(next(&receiver), 30);
    let attempt = state.attempt.as_ref().unwrap();
    assert_eq!(attempt.last_failure, Some(Failure::InspectFailed));
    assert_eq!(attempt.cause, Some(Cause::Transport));
    assert_eq!(value(&state, 40), before, "unconfirmed M3U remains hidden in legacy v1");
    let fresh = live_hls_binding(8);
    state.begin(fresh, 50);
    assert_eq!(state.attempt.as_ref().unwrap().cause, None);
}

#[test]
fn preparation_enums_match_the_shared_consumer_contract() {
    let contract: Value = serde_json::from_str(include_str!(concat!(env!("CARGO_MANIFEST_DIR"),
        "/../ci/ottplay-integration/playback-preparation-diagnostics-v1.json"))).unwrap();
    assert_eq!(contract["synthetic"], true);
    assert_eq!(contract["action"], super::super::preplay_remote::ACTION);
    assert_eq!(contract["capability"]["playback_preparation_diagnostics"],
        super::super::preplay_remote::capability());
    assert_eq!(serde_json::to_value([Phase::Idle, Phase::Resolving, Phase::Inspecting,
        Phase::AwaitingQuality, Phase::Admitting, Phase::Admitted, Phase::Failed,
        Phase::Cancelled]).unwrap(), contract["phases"]);
    assert_eq!(serde_json::to_value([Failure::ResolveFailed, Failure::InspectFailed,
        Failure::WorkerUnavailable, Failure::AdmissionFailed,
        Failure::QualitySaveFailed]).unwrap(), contract["failures"]);
    assert_eq!(serde_json::to_value([Cause::Unknown, Cause::Timeout, Cause::Transport,
        Cause::Http401, Cause::Http403, Cause::Http404, Cause::Http429, Cause::HttpServer,
        Cause::HlsInvalidHeader, Cause::HlsInvalidUtf8, Cause::MediaOpenRefused,
        Cause::MediaReadFailed, Cause::MediaContainerUnavailable, Cause::MediaStreamProbeFailed,
        Cause::MediaNoVideo, Cause::InspectionStopped, Cause::InspectionCancelled,
        Cause::InspectionUnavailable, Cause::ResourceLimit, Cause::TrackIdentity,
        Cause::CodecUnsupported]).unwrap(), contract["causes"]);
}

#[test]
fn all_actual_closed_hls_codec_refusals_map_exactly_without_matching_private_extensions() {
    for reason in [
        "Native HLS does not support HEVC video without audio",
        "Native HLS does not support HEVC video with E-AC-3 audio",
        "Native HLS does not support HEVC video with MP2 audio",
        "Native HLS does not support H.264 video with AC-3 audio",
        "Native HLS does not support H.264 video with E-AC-3 audio",
        "Native HLS does not support H.264 video with MP2 audio",
        "Native HLS does not support H.264 video with MP3 audio",
        "Native HLS does not support MPEG-2 video with AAC audio",
        "Native HLS does not support MPEG-2 video with MP2 audio",
        "Native HLS does not support MPEG-2 video with AC-3 audio",
        "Native HLS requires H.264 video and AAC audio",
    ] {
        assert_eq!(inspection_cause(reason), Cause::CodecUnsupported);
        assert_eq!(inspection_cause(&format!("{reason}: https://private.invalid")), Cause::Unknown);
    }
    assert_eq!(inspection_cause("Unsupported video codec"), Cause::Unknown,
        "non-producer text does not acquire a codec interpretation");
}
