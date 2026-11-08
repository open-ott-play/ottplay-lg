//! Exercise the real raw poll completion before Delivery sees a Value.
//! No worker, network request, storage access or application command is run.
use super::*;

const REQUEST_ID: &str = "0123456789abcdef0123456789abcdef";

fn raw_poll(remote: &mut Remote, body: &[u8]) -> Vec<Invocation> {
    remote.finish(
        Kind::Delivery(SendKind::Poll),
        Outcome::Http {
            response: Ok(protocol::Response {
                status: 200,
                body: body.to_vec(),
            }),
            elapsed_ms: 10,
        },
        100,
    )
}

fn request_document(fields: &str) -> String {
    format!(
        r#"{{"commands":[{{"id":"legacy-fixture","key":"up"}}],"server_time":1000,"requests":[{{"id":"{REQUEST_ID}","expires_at":1020,{fields}}}]}}"#
    )
}

fn ordinary_document() -> String {
    request_document(r#""action":"playback","params":{"operation":"pause"}"#)
}

fn assert_refused_before_admission(body: &[u8]) {
    let mut remote = Remote::disabled_for_test("dev_fixture", "runtime-fixture");
    assert!(raw_poll(&mut remote, body).is_empty(), "ambiguous poll dispatched an action");
    assert_eq!(remote.snapshot()["state"], "error");
    assert!(remote.delivery.idle(), "invalid poll queued an acknowledgment");
    assert!(remote.flight.is_none());
    assert!(remote.take_effects().is_empty());

    // A later well-formed poll using the same IDs must still be admitted. This
    // proves that neither the legacy command nor request was marked as seen.
    let admitted = raw_poll(&mut remote, ordinary_document().as_bytes());
    assert_eq!(admitted.len(), 2);
    assert_eq!(admitted[0].id, "legacy-fixture");
    assert_eq!(admitted[1].id, REQUEST_ID);
    assert_eq!(admitted[1].action, "playback");
    assert_eq!(admitted[1].params, json!({"operation":"pause"}));
}

#[test]
fn raw_poll_rejects_duplicate_action_params_and_nested_keys_atomically() {
    for fields in [
        r#""action":"playback","action":"lifecycle","params":{}"#,
        r#""action":"playback","params":{"operation":"pause"},"params":{"operation":"play"}"#,
        r#""action":"playback","params":{"operation":"pause","operation":"play"}"#,
        r#""action":"media_tracks","params":{"op":"select","target":{"kind":"audio","mode":"track","id":"one","id":"two"}}"#,
        r#""action":"playback","params":{"nested":[{"value":1,"value":2}]}"#,
        r#""action":"playback","params":{"operation":"pause","\u006fperation":"play"}"#,
    ] {
        assert_refused_before_admission(request_document(fields).as_bytes());
    }
}

#[test]
fn raw_poll_rejects_duplicate_envelope_and_legacy_fields_atomically() {
    let ordinary = ordinary_document();
    let duplicate_envelope = ordinary.replacen(
        "\"server_time\":1000,",
        "\"server_time\":999,\"server_time\":1000,",
        1,
    );
    assert_refused_before_admission(duplicate_envelope.as_bytes());
    let duplicate_legacy = ordinary.replacen(
        "\"key\":\"up\"",
        "\"key\":\"down\",\"key\":\"up\"",
        1,
    );
    assert_refused_before_admission(duplicate_legacy.as_bytes());
    let duplicate_unused_request = ordinary.replacen(
        "]}",
        ",{\"id\":\"fedcba9876543210fedcba9876543210\",\"action\":\"playback\",\"params\":{\"operation\":\"pause\",\"operation\":\"play\"}}]}",
        1,
    );
    assert_refused_before_admission(duplicate_unused_request.as_bytes());
}

#[test]
fn raw_poll_body_limit_is_checked_before_admission() {
    let mut exact = ordinary_document().into_bytes();
    exact.resize(protocol::BODY_LIMIT, b' ');
    let mut remote = Remote::disabled_for_test("dev_fixture", "runtime-fixture");
    assert_eq!(raw_poll(&mut remote, &exact).len(), 2);
    assert_eq!(remote.snapshot()["state"], "connected");
    exact.push(b' ');
    assert_refused_before_admission(&exact);
}

#[test]
fn raw_poll_preserves_ordinary_values_and_existing_replay_rules() {
    let mut remote = Remote::disabled_for_test("dev_fixture", "runtime-fixture");
    let params = json!({
        "operation":"pause", "signed":-2, "fraction":1.25,
        "large":18446744073709551615u64, "unicode":"日本語 café",
        "nested":[null,true,false,{"first":1},{"first":2}]
    });
    let document = request_document(&format!("\"action\":\"playback\",\"params\":{params}"));
    let first = raw_poll(&mut remote, document.as_bytes());
    assert_eq!(first.len(), 2);
    assert_eq!(first[0].action, "command");
    assert_eq!(first[1].params, params);
    assert_eq!(first[1].deadline_ms, 20_090);
    assert!(raw_poll(&mut remote, document.as_bytes()).is_empty(),
        "strict decoding must not alter pending-request replay protection");
}

#[test]
fn raw_poll_rejects_bad_json_and_excessive_nesting_without_dispatch() {
    let deep = format!("{}0{}", "[".repeat(200), "]".repeat(200));
    let deep_document = request_document(&format!(
        "\"action\":\"playback\",\"params\":{{\"nested\":{deep}}}"
    ));
    assert_refused_before_admission(deep_document.as_bytes());
    for body in [
        format!("{} true", ordinary_document()),
        request_document(r#""action":"playback","params":{"bad":1e999}"#),
        request_document(r#""action":"playback","params":{"bad":NaN}"#),
    ] {
        assert_refused_before_admission(body.as_bytes());
    }
}
