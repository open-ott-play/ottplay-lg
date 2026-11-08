//! Existing raw-command boundary only: these regressions can precede the
//! recovery implementation. No storage, worker, media, or network is started.
use super::*;

fn setup() -> (App, Runtime) {
    let app = run::lifecycle_regression_tests::app();
    let mut runtime = runtime_fixture();
    runtime.clock_sample =
        Some(plx_base::task::time::ClockSample::capture_main(app.adapters.player.mt()).unwrap());
    (app, runtime)
}
fn request(runtime: &mut Runtime, id: u32, action: &str, params: Value) -> Invocation {
    let now = runtime.now_ms();
    let raw = json!({"commands":[],"server_time":1000,"requests":[{
        "id":format!("{id:032x}"),"action":action,"params":params,"expires_at":1020
    }]})
    .to_string();
    let mut requests = runtime.remote.raw_poll_for_test(&raw, now);
    assert_eq!(requests.len(), 1);
    let invocation = requests.pop().unwrap();
    assert!(runtime.remote.invocation_is_current(&invocation));
    assert_eq!(invocation.deadline_ms, now + 20_000);
    invocation
}
fn run(app: &mut App, runtime: &mut Runtime, invocation: Invocation) -> Value {
    let response =
        commands::command(runtime, app, &invocation).expect("non-IO request completes immediately");
    let id = invocation.id.clone();
    runtime.remote.complete(invocation, response);
    runtime
        .remote
        .response_for_id_for_test(&id)
        .expect("actual addressed response")
}

#[test]
fn recovery_advertises_its_exact_additive_marker() {
    let _serial = plx_base::testlock::serial();
    let (mut app, mut runtime) = setup();
    let invocation = request(&mut runtime, 1, "capabilities", json!({}));
    let result = run(&mut app, &mut runtime, invocation);
    assert_eq!(result["status"], "ok");
    assert_eq!(
        result["data"]["favorites_global_recovery"],
        json!({"version":1})
    );
    assert_eq!(result["data"]["favorites_global"], json!({"version":1}));
}

#[test]
fn unreadable_and_busy_native_state_are_never_an_empty_recovery_success() {
    let _serial = plx_base::testlock::serial();
    let (mut app, mut runtime) = setup();
    let parameters = json!({"op":"inspect","runtime":"fixture","subject":"global","cursor":null});
    let invocation = request(
        &mut runtime,
        1,
        "favorites_global_recovery",
        parameters.clone(),
    );
    let response = run(&mut app, &mut runtime, invocation);
    assert_eq!(response["status"], "rejected");
    assert_eq!(
        response["data"],
        json!({"version":1,"op":"inspect","runtime":"fixture","subject":"global","error":"storage_unavailable"})
    );
    runtime.loaded = true;
    runtime.saving = true;
    let invocation = request(&mut runtime, 2, "favorites_global_recovery", parameters);
    let response = run(&mut app, &mut runtime, invocation);
    assert_eq!(response["data"]["error"], "busy");
    assert!(runtime.saving);
    assert!(runtime.rx.try_recv().is_err());
}

#[test]
fn well_formed_acknowledgement_is_explicitly_unsupported_and_writes_nothing() {
    let _serial = plx_base::testlock::serial();
    let (mut app, mut runtime) = setup();
    let before = serde_json::to_value(&runtime.saved).unwrap();
    let mut params = json!({"op":"acknowledge","runtime":"fixture","subject":"global",
        "operation_id":"00000001-0000-4000-8000-000000000001","operation_runtime":"prior-runtime",
        "original_entry_sha256":"1".repeat(64),"expected_ledger_revision":"2".repeat(64),
        "expected_store_revision":"3".repeat(64),"expected_context_revision":"4".repeat(64),"backend_proof":"5".repeat(64),
        "reason":"accept-unknown-no-replay","after_ledger_sha256":"6".repeat(64),"accept_unknown":true});
    let values = vec![
        params["runtime"].clone(),
        json!("global"),
        params["operation_id"].clone(),
        params["operation_runtime"].clone(),
        params["original_entry_sha256"].clone(),
        params["expected_ledger_revision"].clone(),
        params["expected_store_revision"].clone(),
        params["expected_context_revision"].clone(),
        params["backend_proof"].clone(),
        params["reason"].clone(),
    ];
    let mut ack = vec![json!("ottplay-favorites-global-recovery-ack-v1")];
    ack.extend(values.iter().cloned());
    params["acknowledgement_id"] = json!(plx_data::ott::favorites_wire::hash(
        json!(ack).to_string().as_bytes()
    ));
    let mut plan = vec![json!("ottplay-favorites-global-recovery-plan-v1")];
    plan.extend(values);
    plan.push(params["acknowledgement_id"].clone());
    plan.push(params["after_ledger_sha256"].clone());
    params["plan_sha256"] = json!(plx_data::ott::favorites_wire::hash(
        json!(plan).to_string().as_bytes()
    ));
    let invocation = request(&mut runtime, 1, "favorites_global_recovery", params);
    let response = run(&mut app, &mut runtime, invocation);
    assert_eq!(response["status"], "unsupported");
    assert_eq!(
        response["data"],
        json!({"version":1,"op":"acknowledge","runtime":"fixture","subject":"global","error":"unsupported_persistence"})
    );
    assert!(!runtime.saving);
    assert!(!runtime.loaded);
    assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), before);
    assert!(runtime.rx.try_recv().is_err());
}

#[test]
fn unknown_request_fields_and_duplicate_raw_keys_never_reach_inspection() {
    let _serial = plx_base::testlock::serial();
    let (mut app, mut runtime) = setup();
    let invocation = request(
        &mut runtime,
        1,
        "favorites_global_recovery",
        json!({"op":"inspect","runtime":"fixture","subject":"global","cursor":null,"storage_key":"other"}),
    );
    let response = run(&mut app, &mut runtime, invocation);
    assert_eq!(response["data"]["error"], "invalid_request");
    let raw = r#"{"commands":[],"server_time":1000,"requests":[{"id":"00000000000000000000000000000002","action":"favorites_global_recovery","expires_at":1020,"params":{"op":"inspect","runtime":"fixture","subject":"global","cursor":null,"cursor":null}}]}"#;
    let now = runtime.now_ms();
    assert!(runtime.remote.raw_poll_for_test(raw, now).is_empty());
    assert!(!runtime.saving);
    assert!(runtime.rx.try_recv().is_err());
}
