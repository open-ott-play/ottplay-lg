//! Local result transport contract at the actual remote/runtime boundary.
//! Real temporary-directory grants are used where needed; catalog/resolver slots
//! remain held. Worker failure is an injected completion, not a disk-failure or
//! network test. No decoder, protected writer or additional worker is started.
use super::*;
use crate::app::ott_local_session::LocalError;

fn command(app: &mut App, serial: u64, op: &str) -> (Invocation, Option<Value>) {
    let mut runtime = app.ott.take().unwrap();
    let now = runtime.now_ms();
    let raw = json!({"commands":[],"server_time":1000,"requests":[{
        "id":format!("{serial:032x}"),"action":"local_media",
        "params":{"op":op,"runtime":runtime.remote.runtime()},"expires_at":1020
    }]})
    .to_string();
    let mut invocations = runtime.remote.raw_poll_for_test(&raw, now);
    assert_eq!(invocations.len(), 1);
    let invocation = invocations.pop().unwrap();
    assert!(runtime.remote.invocation_is_current(&invocation));
    assert_eq!(invocation.deadline_ms, now.saturating_add(20_000));
    let response = commands::command(&mut runtime, app, &invocation);
    if let Some(response) = &response {
        runtime
            .remote
            .complete(invocation.clone(), response.clone());
    }
    app.ott = Some(runtime);
    (invocation, response)
}

fn delivered(app: &App, invocation: &Invocation) -> Value {
    app.ott
        .as_ref()
        .unwrap()
        .remote
        .response_for_id_for_test(&invocation.id)
        .expect("actual addressed delivery result")
}

fn assert_error(response: &Value, invocation: &Invocation, code: &str) {
    assert_eq!(response["status"], "rejected");
    assert_eq!(
        response["data"],
        json!({
            "version":1,"runtime":invocation.params["runtime"],
            "op":invocation.params["op"],"state":"error","error":code
        }),
        "the typed Local data is preserved verbatim, without an extra wrapper"
    );
    assert_eq!(response["data"].as_object().unwrap().len(), 5);
}

#[test]
fn immediate_sources_busy_is_rejected_with_unchanged_typed_data() {
    let _serial = plx_base::testlock::serial();
    let (_temp, mut app, _root) = local_integration::setup();
    let (first, pending) = command(&mut app, 1, "sources");
    assert!(pending.is_none());
    assert!(app
        .ott
        .as_ref()
        .unwrap()
        .remote
        .response_for_id_for_test(&first.id)
        .is_none());
    let generation = app.ott.as_ref().unwrap().generation;
    let (second, response) = command(&mut app, 2, "sources");
    assert_error(
        &response.expect("overlapping operation is finite"),
        &second,
        "operation_in_progress",
    );
    assert_error(&delivered(&app, &second), &second, "operation_in_progress");
    assert_eq!(app.ott.as_ref().unwrap().generation, generation);
    assert!(app.ott.as_ref().unwrap().transient.candidate.is_some());
    assert!(app
        .ott
        .as_ref()
        .unwrap()
        .remote
        .response_for_id_for_test(&first.id)
        .is_none());
}

#[test]
fn asynchronous_binding_failure_keeps_typed_error_and_rejected_delivery() {
    let _serial = plx_base::testlock::serial();
    let (_temp, mut app, _root) = local_integration::setup();
    let (invocation, pending) = command(&mut app, 1, "sources");
    assert!(pending.is_none());
    let runtime = app.ott.as_ref().unwrap();
    assert!(runtime
        .remote
        .response_for_id_for_test(&invocation.id)
        .is_none());
    runtime
        .catalog_tx
        .send(CatalogEvent::LocalBindings(
            runtime.generation,
            Err(LocalError::NativeError),
        ))
        .unwrap_or_else(|_| panic!("fixture receiver disappeared"));
    runtime_tick(&mut app);
    assert_error(&delivered(&app, &invocation), &invocation, "native_error");
    assert!(app.ott.as_ref().unwrap().transient.candidate.is_none());
}

#[test]
fn asynchronous_ready_sources_stays_ok_through_real_observation_and_delivery() {
    let _serial = plx_base::testlock::serial();
    let (_temp, mut app, root) = local_integration::setup();
    let (invocation, pending) = command(&mut app, 1, "sources");
    assert!(pending.is_none());
    let runtime = app.ott.as_mut().unwrap();
    let Some(ott_catalog::Task::LocalBindings(request)) = transient::take_binding_task(runtime)
    else {
        panic!("query uses the existing binding operation");
    };
    let result = ott_catalog::validate_local_bindings_for_test(
        &plx_base::task::OffFrame::for_test(),
        request,
        runtime.cancel.as_ref(),
    );
    runtime
        .catalog_tx
        .send(CatalogEvent::LocalBindings(runtime.generation, result))
        .unwrap_or_else(|_| panic!("fixture receiver disappeared"));
    runtime_tick(&mut app);
    let response = delivered(&app, &invocation);
    assert_eq!(response["status"], "ok");
    assert_eq!(
        response["data"],
        json!({
            "version":1,"runtime":invocation.params["runtime"],"op":"sources","state":"ready",
            "sources":[{"id":root.id,"name":root.name,"kind":"folder","available":true}]
        })
    );
    assert!(app.ott.as_ref().unwrap().transient.candidate.is_none());
}

#[test]
fn immediate_empty_sources_stays_ok_without_a_binding_operation() {
    let _serial = plx_base::testlock::serial();
    let mut app = app_fixture();
    app.ott.as_mut().unwrap().loaded = true;
    let (invocation, response) = command(&mut app, 1, "sources");
    let response = response.expect("empty list does not require a worker");
    assert_eq!(response["status"], "ok");
    assert_eq!(
        response["data"],
        json!({
            "version":1,"runtime":invocation.params["runtime"],"op":"sources","state":"ready","sources":[]
        })
    );
    assert_eq!(delivered(&app, &invocation)["status"], "ok");
    assert!(app.ott.as_ref().unwrap().transient.candidate.is_none());
}
