//! Authenticated in-memory ingress only; no transport, persistence or media work.
use super::*;
use super::super::Remote;

fn fixture() -> Runtime {
    let mut runtime = Runtime::unloaded(crate::app::ott_companion::Supervisor::inert_for_test());
    runtime.remote = Remote::disabled_for_test("dev_fixture", "native_123");
    runtime.clock_sample = Some(plx_base::task::time::ClockSample::capture(
        &plx_base::task::OffFrame::for_test()).unwrap());
    runtime.loaded = true;
    runtime
}
fn invocation(runtime: &mut Runtime, action: &str, params: Value, serial: u64) -> Invocation {
    let body = json!({"commands":[],"server_time":1000,"requests":[{
        "id":format!("{serial:032x}"),"action":action,"params":params,"expires_at":1020
    }]}).to_string();
    let now = runtime.now_ms();
    let mut incoming = runtime.remote.raw_poll_for_test(&body, now);
    assert_eq!(incoming.len(), 1);
    incoming.pop().unwrap()
}
fn params() -> Value { json!({"version":2,"runtime":"native_123"}) }

#[test]
fn authenticated_read_is_bounded_redacted_read_only_and_does_not_evict_or_change_v1() {
    let _serial = plx_base::testlock::serial();
    let mut runtime = fixture();
    for serial in 1..=12 {
        let inv = invocation(&mut runtime, "play", json!({"url":"private-url"}), serial);
        runtime.remote.note_command(&inv, 0);
        runtime.remote.complete(inv, json!({"status":"ok","error":"private-error"}));
    }
    let now = runtime.now_ms();
    let before = runtime.remote.command_activity(now);
    let saved = serde_json::to_value(&runtime.saved).unwrap();
    let inv = invocation(&mut runtime, ACTION, params(), 100);
    assert!(input_policy::read_only(&inv));
    // Mirror the real command dispatch's unconditional activity notification.
    runtime.remote.note_command(&inv, now);
    let reply = command(&mut runtime, &inv);
    runtime.remote.complete(inv.clone(), reply.clone());
    assert_eq!(reply["status"], "ok");
    let entries = reply["data"]["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 8);
    assert_eq!(entries[0]["sequence"], 5);
    assert_eq!(entries[7]["sequence"], 12);
    assert_eq!(entries[0]["request_id"], format!("{:032x}", 5));
    assert_eq!(entries[7]["request_id"], format!("{:032x}", 12));
    assert!(!reply.to_string().contains("private"));
    assert!(reply.to_string().len() < 8192);
    for _ in 0..12 { runtime.remote.note_command(&inv, now); command(&mut runtime, &inv); }
    assert_eq!(runtime.remote.command_activity(now), before);
    assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), saved);
}

#[test]
fn strict_schema_runtime_and_expiry_rejections_are_closed_and_do_not_echo_input() {
    let _serial = plx_base::testlock::serial();
    let mut runtime = fixture();
    for (index, invalid) in [json!(null), json!({}), json!({"version":1,"runtime":"native_123"}),
        json!({"version":2.0,"runtime":"native_123"}), json!({"version":true,"runtime":"native_123"}),
        json!({"version":2,"runtime":"native_123","extra":"private"}),
        json!({"version":2,"runtime":"https://private.invalid"}),
        json!({"version":2,"runtime":""}), json!({"version":2,"runtime":"x".repeat(97)})]
        .into_iter().enumerate() {
        // The transport rejects non-object params before creating an invocation.
        // Start from admitted authority, then exercise the handler's own schema fence.
        let mut inv = invocation(&mut runtime, ACTION, params(), 200 + index as u64);
        inv.params = invalid;
        assert_eq!(command(&mut runtime, &inv), failure("native_123", "invalid_request"));
    }
    let inv = invocation(&mut runtime, ACTION, json!({"version":2,"runtime":"other"}), 300);
    assert_eq!(command(&mut runtime, &inv), failure("native_123", "runtime_mismatch"));
    let mut inv = invocation(&mut runtime, ACTION, params(), 301);
    inv.deadline_ms = runtime.now_ms();
    assert_eq!(command(&mut runtime, &inv), failure("native_123", "invalid_request"));
}

#[test]
fn fail_closed_kiosk_allows_read_but_retired_authority_cannot_read_or_repopulate() {
    let _serial = plx_base::testlock::serial();
    let mut runtime = fixture();
    runtime.loaded = false;
    assert!(input_policy::strict(&runtime));
    let inv = invocation(&mut runtime, ACTION, params(), 400);
    assert!(input_policy::remote(&mut runtime, &inv).is_ok());
    assert_eq!(command(&mut runtime, &inv)["status"], "ok");
    let play = invocation(&mut runtime, "play", json!({}), 401);
    runtime.remote.note_command(&play, 0);
    runtime.remote.retire_for_erasure();
    runtime.remote.note_command(&play, 1);
    runtime.remote.complete(play, json!({"status":"ok"}));
    assert!(runtime.remote.command_activity(2)["entries"].as_array().unwrap().is_empty());
    assert_eq!(command(&mut runtime, &inv), failure("native_123", "unavailable"));
}

#[test]
fn authority_generation_and_runtime_replacement_cannot_inherit_activity() {
    let _serial = plx_base::testlock::serial();
    let mut runtime = fixture();
    let play = invocation(&mut runtime, "input", json!({}), 500);
    runtime.remote.note_command(&play, 0);
    let inv = invocation(&mut runtime, ACTION, params(), 501);
    runtime.remote.disable().unwrap();
    assert!(!runtime.remote.invocation_is_current(&inv));
    assert_eq!(command(&mut runtime, &inv), failure("native_123", "unavailable"));
    assert!(runtime.remote.command_activity(0)["entries"].as_array().unwrap().is_empty());
    runtime.remote = Remote::disabled_for_test("dev_fixture", "replacement");
    assert_eq!(command(&mut runtime, &inv), failure("replacement", "runtime_mismatch"));
    let current = invocation(&mut runtime, ACTION, json!({"version":2,"runtime":"replacement"}), 502);
    assert_eq!(command(&mut runtime, &current)["data"],
        json!({"version":2,"runtime":"replacement","scope":"remote_ingress","entries":[]}));
}

#[test]
fn marker_and_closed_rejections_match_shared_contract() {
    let fixture: Value = serde_json::from_str(include_str!(concat!(env!("CARGO_MANIFEST_DIR"),
        "/../ci/ottplay-integration/command-activity-v2.json"))).unwrap();
    assert_eq!(fixture["action"], ACTION);
    assert_eq!(fixture["capability"][ACTION], capability());
    assert_eq!(fixture["request"], params());
    assert_eq!(fixture["errors"], json!(["invalid_request","runtime_mismatch","unavailable","policy_denied"]));
    for code in ["invalid_request","runtime_mismatch","unavailable","policy_denied"] {
        assert_eq!(failure("native_123", code), json!({"status":"rejected",
            "data":{"version":2,"runtime":"native_123","error":code}}));
    }
}
