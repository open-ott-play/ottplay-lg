//! Tests-first compatibility boundary: this file uses only pre-existing APIs.
use super::*;

fn request(app: &mut App, serial: u64, action: &str, params: Value) -> Value {
    let mut runtime = app.ott.take().unwrap();
    let now = runtime.now_ms();
    let mut admitted=runtime.remote.raw_poll_for_test(&json!({"commands":[],"server_time":1000,
        "requests":[{"id":format!("{serial:032x}"),"action":action,"params":params,"expires_at":1020}]}).to_string(),now);
    assert_eq!(admitted.len(), 1);
    let invocation = admitted.remove(0);
    assert!(runtime.remote.invocation_is_current(&invocation));
    assert_eq!(invocation.deadline_ms, now + 20_000);
    let response =
        commands::command(&mut runtime, app, &invocation).expect("read-only immediate response");
    runtime.remote.complete(invocation, response.clone());
    app.ott = Some(runtime);
    response
}
#[test]
fn native_advertises_the_exact_items_v1_marker() {
    let _lock = plx_base::testlock::serial();
    let mut app = app_fixture();
    let response = request(&mut app, 1, "capabilities", json!({}));
    assert_eq!(
        response["data"]["favorites_global_items"],
        json!({"version":1})
    );
}
#[test]
fn native_empty_items_list_is_a_typed_read_not_an_unsupported_command() {
    let _lock = plx_base::testlock::serial();
    let mut app = app_fixture();
    app.ott.as_mut().unwrap().loaded = true;
    let before = serde_json::to_value(&app.ott.as_ref().unwrap().saved).unwrap();
    let response = request(
        &mut app,
        1,
        "favorites_global_items",
        json!({"runtime":"fixture","op":"list",
        "subject":"global","cursor":null,"max_entries":100}),
    );
    assert_eq!(response["status"], "ok");
    assert_eq!(response["data"]["op"], "list");
    assert_eq!(response["data"]["total"], 0);
    assert_eq!(response["data"]["entries"], json!([]));
    assert_eq!(
        serde_json::to_value(&app.ott.as_ref().unwrap().saved).unwrap(),
        before
    );
}
