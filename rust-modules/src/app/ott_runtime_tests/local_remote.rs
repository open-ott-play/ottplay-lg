//! Remote local-media entry points at the real Runtime/command boundary.
//! Empty inventories are deliberate: no filesystem, storage worker or decoder runs.
use super::*;

fn local_request(app: &mut App, op: &str, extra: serde_json::Value) -> serde_json::Value {
    let runtime = app.ott.as_mut().unwrap();
    runtime.loaded = true;
    let mut params = serde_json::json!({"op":op,"runtime":runtime.remote.runtime()});
    for (key, value) in extra.as_object().unwrap() {
        params[key] = value.clone();
    }
    let now = runtime.now_ms();
    let raw = serde_json::json!({"commands":[],"server_time":1000,"requests":[{
        "id":"00000000000000000000000000000001","action":"local_media",
        "params":params,"expires_at":1020
    }]}).to_string();
    let mut invocations = runtime.remote.raw_poll_for_test(&raw, now);
    assert_eq!(invocations.len(), 1);
    let inv = invocations.pop().unwrap();
    assert!(runtime.remote.invocation_is_current(&inv));
    assert_eq!(inv.deadline_ms, now.saturating_add(20_000));
    let mut runtime = app.ott.take().unwrap();
    let response = commands::command(&mut runtime, app, &inv);
    app.ott = Some(runtime);
    response.expect("empty inventory requires no worker")
}

#[test]
fn local_sources_reports_an_empty_complete_inventory_without_mutating_saved_state() {
    let _lock = plx_base::testlock::serial();
    let mut app = app_fixture();
    let before = serde_json::to_value(&app.ott.as_ref().unwrap().saved).unwrap();
    let response = local_request(&mut app, "sources", serde_json::json!({}));
    assert_eq!(response["status"], "ok");
    assert_eq!(response["data"]["version"], 1);
    assert_eq!(response["data"]["op"], "sources");
    assert_eq!(response["data"]["state"], "ready");
    assert_eq!(response["data"]["sources"], serde_json::json!([]));
    let runtime = app.ott.as_ref().unwrap();
    assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), before);
    assert!(!runtime.saving && runtime.resolver_next.is_none());
}

#[test]
fn local_sources_remains_read_only_in_standby() {
    let _lock = plx_base::testlock::serial();
    let mut app = app_fixture();
    app.ott_standby = true;
    let response = local_request(&mut app, "sources", serde_json::json!({}));
    assert_eq!(response["status"], "ok");
    assert_eq!(response["data"]["sources"], serde_json::json!([]));
    assert!(app.ott_standby);
    assert!(app.ott.as_ref().unwrap().resolver_next.is_none());
}
