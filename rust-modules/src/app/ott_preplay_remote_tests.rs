//! In-memory authenticated ingress and addressed worker evidence, without IO/media work.
use super::*;
use super::super::{Job, PlaybackRequest, ProviderKind, ProviderProfile, Remote};

fn fixture() -> Runtime {
    let mut runtime = Runtime::unloaded(crate::app::ott_companion::Supervisor::inert_for_test());
    runtime.remote = Remote::disabled_for_test("dev_fixture", "fixture");
    runtime.clock_sample = Some(plx_base::task::time::ClockSample::capture(
        &plx_base::task::OffFrame::for_test()).unwrap());
    runtime.loaded = true;
    let profile = ProviderProfile {
        id: "private-profile".into(), name: "private-name".into(), kind: ProviderKind::M3u,
        source: "https://private-user:private-password@private.invalid/private-path?token=private-token".into(),
        source_alias: "private-alias".into(), enabled: true,
    };
    runtime.saved.profiles.profiles = vec![profile.clone()];
    runtime.saved.profiles.active_profile = Some(profile.id.clone());
    runtime.provider = profile.kind;
    runtime.play_generation = 7;
    runtime.play_target = Some(PlaybackRequest::ordinary("private-item", 0));
    runtime.play_profile = Some((7, profile.clone()));
    preplay::begin(&mut runtime, &profile);
    runtime
}

fn invocation(runtime: &mut Runtime, params: Value, serial: u64) -> Invocation {
    let now = runtime.now_ms();
    let body = json!({"commands":[],"server_time":1000,"requests":[{
        "id":format!("{serial:032x}"),"action":ACTION,"params":params,"expires_at":1020
    }]}).to_string();
    let mut incoming = runtime.remote.raw_poll_for_test(&body, now);
    assert_eq!(incoming.len(), 1);
    incoming.pop().unwrap()
}

fn fail_inspection(runtime: &mut Runtime, reason: &'static str) {
    let profile = runtime.pending_play_profile().unwrap().clone();
    let mut worker = preplay::worker(runtime, 7, &profile);
    let result: Result<(), _> = worker.inspect(|| Err(reason));
    assert_eq!(result, Err(reason));
    for _ in 0..2 {
        match runtime.rx.try_recv().unwrap() {
            Job::Preplay(event) => preplay::receive(runtime, event),
            _ => panic!("fixture must receive only addressed preparation observations"),
        }
    }
}

#[test]
fn opt_in_preserves_hidden_legacy_snapshot_but_exposes_closed_current_failure() {
    let _serial = plx_base::testlock::serial();
    let mut runtime = fixture();
    let before = serde_json::to_value(runtime.preplay.snapshot(0)).unwrap();
    let saved = serde_json::to_value(&runtime.saved).unwrap();
    fail_inspection(&mut runtime, "Media source request timed out");
    runtime.take_completed_play_target();
    assert_eq!(serde_json::to_value(runtime.preplay.snapshot(10)).unwrap(), before);
    let inv = invocation(&mut runtime, json!({"version":1,"runtime":"fixture"}), 1);
    let reply = command(&mut runtime, &inv);
    assert_eq!(reply, json!({"status":"ok","data":{"version":1,"runtime":"fixture",
        "phase":"failed","attempt_generation":7,"last_failure":"inspect_failed","cause":"timeout",
        "retry":{"pending":false,"failures":null,"retry_after_ms":null}}}));
    assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), saved);
    assert_eq!(capability(), json!({"version":1}));
    for secret in ["private-profile", "private-name", "private-user", "private-password",
        "private.invalid", "private-path", "private-token", "private-alias", "private-item",
        "Media source request timed out"] {
        assert!(!reply.to_string().contains(secret));
    }
}

#[test]
fn unknown_error_is_not_echoed_and_profile_generation_and_runtime_changes_hide_evidence() {
    let _serial = plx_base::testlock::serial();
    let mut runtime = fixture();
    fail_inspection(&mut runtime, "private raw provider error https://password@private.invalid");
    let inv = invocation(&mut runtime, json!({"version":1,"runtime":"fixture"}), 2);
    assert_eq!(command(&mut runtime, &inv)["data"]["cause"], "unknown");
    runtime.saved.profiles.profiles[0].source.push_str("/changed");
    assert_eq!(command(&mut runtime, &inv)["data"]["phase"], "idle");
    let length = runtime.saved.profiles.profiles[0].source.len();
    runtime.saved.profiles.profiles[0].source.truncate(length - 8);
    runtime.play_target = Some(PlaybackRequest::ordinary("different-private-item", 0));
    assert_eq!(command(&mut runtime, &inv)["data"]["phase"], "idle");
    runtime.play_target = Some(PlaybackRequest::ordinary("private-item", 0));
    runtime.play_generation += 1;
    assert_eq!(command(&mut runtime, &inv)["data"]["cause"], Value::Null);
    runtime.play_generation -= 1;
    runtime.generation += 1;
    assert_eq!(command(&mut runtime, &inv)["data"]["phase"], "idle");
    runtime.generation -= 1;
    runtime.remote = Remote::disabled_for_test("dev_fixture", "replacement");
    let current = invocation(&mut runtime, json!({"version":1,"runtime":"replacement"}), 3);
    assert_eq!(command(&mut runtime, &current)["data"]["phase"], "idle");
}

#[test]
fn parser_and_expiry_reject_without_reflecting_caller_runtime() {
    let _serial = plx_base::testlock::serial();
    let mut runtime = fixture();
    for (index, params) in [json!({}), json!({"version":1,"runtime":"fixture","extra":true}),
        json!({"version":1.0,"runtime":"fixture"}), json!({"version":true,"runtime":"fixture"}),
        json!({"version":1,"runtime":"https://private.invalid"}),
        json!({"version":1,"runtime":"x".repeat(97)})].into_iter().enumerate() {
        let inv = invocation(&mut runtime, params, 100+index as u64);
        assert_eq!(command(&mut runtime, &inv), failure("fixture", "invalid_request"));
    }
    let inv = invocation(&mut runtime, json!({"version":1,"runtime":"private-other"}), 200);
    assert_eq!(command(&mut runtime, &inv), failure("fixture", "runtime_mismatch"));
    let mut inv = invocation(&mut runtime, json!({"version":1,"runtime":"fixture"}), 201);
    inv.deadline_ms = 0;
    assert_eq!(command(&mut runtime, &inv), failure("fixture", "invalid_request"));
}

#[test]
fn read_only_inspection_is_available_under_fail_closed_kiosk() {
    let _serial = plx_base::testlock::serial();
    let mut runtime = fixture();
    runtime.loaded = false; // No usable kiosk policy; source/navigation actions fail closed.
    assert!(input_policy::strict(&runtime));
    let inv = invocation(&mut runtime, json!({"version":1,"runtime":"fixture"}), 300);
    assert!(input_policy::read_only(&inv));
    assert!(input_policy::remote(&mut runtime, &inv).is_ok());
    assert_eq!(command(&mut runtime, &inv)["status"], "ok");
}

#[test]
fn pending_retry_projection_is_generation_fenced_bounded_and_contains_no_item_id() {
    use super::super::continuity::PendingRetry;
    assert_eq!(retry(None, 1, 0), json!({"pending":false,"failures":null,"retry_after_ms":null}));
    assert_eq!(retry(Some(PendingRetry { generation:2, after:"private-item", failures:9,
        not_before_ms:100 }), 1, 0)["pending"], false);
    assert_eq!(retry(Some(PendingRetry { generation:1, after:"private-item", failures:usize::MAX,
        not_before_ms:u64::MAX }), 1, 0),
        json!({"pending":true,"failures":4294967295u64,"retry_after_ms":30000}));
    assert_eq!(retry(Some(PendingRetry { generation:1, after:"private-item", failures:1,
        not_before_ms:10 }), 1, 20)["retry_after_ms"], 0);
}

#[test]
fn stale_invocation_is_refused_without_inspection_or_effects() {
    let _serial = plx_base::testlock::serial();
    let mut runtime = fixture();
    let inv = invocation(&mut runtime, json!({"version":1,"runtime":"fixture"}), 400);
    runtime.remote.disable().unwrap(); // Invalidates only in memory; no tick/IO runs.
    assert!(!runtime.remote.invocation_is_current(&inv));
    assert_eq!(command(&mut runtime, &inv), failure("fixture", "unavailable"));
}

#[test]
fn cross_profile_favorite_keeps_pending_and_failed_diagnostics_only_for_revalidated_source() {
    use plx_data::ott::favorites::{Favorite, Locator, Origin};
    let _serial = plx_base::testlock::serial();
    let mut runtime = fixture();
    let source = ProviderProfile { id:"other-profile".into(), name:"Other source".into(),
        kind:ProviderKind::VPortal, source:"https://other-private.invalid/portal".into(),
        source_alias:"Other alias".into(), enabled:true };
    runtime.saved.profiles.profiles.push(source.clone());
    let favorite = Favorite { title:"Private favorite".into(),
        origin:Origin { provider:"vportal".into(), profile:source.name.clone(),
            subprofile:None, source_alias:source.source_alias.clone() },
        locator:Locator::VPortal { id_field:"id".into(), id:"42".into() } };
    let id = format!("favorite:{}", favorite.identity());
    runtime.saved.favorites = serde_json::from_value(json!({"version":1,"entries":[favorite]})).unwrap();
    let (request, bound, _, _) = runtime.prepare_play_request(PlaybackRequest::ordinary(&id, 0)).unwrap();
    assert_eq!(bound, source);
    runtime.play_target = Some(request);
    runtime.play_profile = Some((7, source.clone()));
    preplay::begin(&mut runtime, &source);
    assert_ne!(runtime.effective_profile(), Some(&source), "browsing remains source A");
    let mut worker = preplay::worker(&runtime, 7, &source);
    worker.phase(preplay::Phase::Inspecting);
    match runtime.rx.try_recv().unwrap() {
        Job::Preplay(event) => preplay::receive(&mut runtime, event),
        _ => panic!("only addressed observation expected"),
    }
    let inv = invocation(&mut runtime, json!({"version":1,"runtime":"fixture"}), 500);
    assert_eq!(command(&mut runtime, &inv)["data"]["phase"], "inspecting");
    fail_inspection(&mut runtime, "Media source request timed out");
    runtime.take_completed_play_target();
    runtime.play_profile = None; // Completion may retire this temporary pending binding too.
    assert_eq!(command(&mut runtime, &inv)["data"]["cause"], "timeout");
    runtime.saved.profiles.profiles[1].source.push_str("/edited");
    assert_eq!(command(&mut runtime, &inv)["data"]["phase"], "idle");
    runtime.saved.profiles.profiles[1] = source.clone();
    runtime.saved.profiles.profiles[1].enabled = false;
    assert_eq!(command(&mut runtime, &inv)["data"]["phase"], "idle");
    runtime.saved.profiles.profiles.pop();
    assert_eq!(command(&mut runtime, &inv)["data"]["phase"], "idle");
    runtime.saved.profiles.profiles.push(source);
    assert_eq!(command(&mut runtime, &inv)["data"]["cause"], "timeout");
    runtime.saved.favorites = serde_json::from_value(json!({"version":1,"entries":[]})).unwrap();
    assert_eq!(command(&mut runtime, &inv)["data"]["phase"], "idle");
}
