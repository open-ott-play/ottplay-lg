//! Feed real raw transport completion; no worker, network, device or storage operation.
use super::*;

const ID: &str = "0123456789abcdef0123456789abcdef";
const PARAMS: &str = r#"{"version":1,"runtime":"old-runtime","op":"list"}"#;
fn document(params: &str) -> String {
    format!(r#"{{"commands":[],"server_time":1000,"requests":[{{"id":"{ID}","expires_at":1020,"action":"plex_servers","params":{params}}}]}}"#)
}
fn poll(remote: &mut Remote, body: &str) -> Vec<Invocation> {
    remote.finish(Kind::Delivery(SendKind::Poll), Outcome::Http {
        response: Ok(protocol::Response { status: 200, body: body.as_bytes().to_vec() }), elapsed_ms: 10,
    }, 100)
}
fn new_remote() -> Remote { Remote::disabled_for_test("fixture-device", "new-runtime") }

#[test]
fn received_projection_limit_is_checked_before_remembering_ids() {
    for bytes in [16384, 16385] {
        let fixed = format!(r#"{{"action":"plex_servers","params":{PARAMS}}}"#).len();
        let params = format!("{}{} }}", &PARAMS[..PARAMS.len() - 1], " ".repeat(bytes - fixed - 1));
        let projection = format!(r#"{{"action":"plex_servers","params":{params}}}"#);
        assert_eq!(projection.len(), bytes);
        let mut remote = new_remote();
        let incoming = poll(&mut remote, &document(&params));
        if bytes == 16384 {
            assert_eq!(incoming.len(), 1);
            assert_eq!(incoming[0].plex_servers_projection(), Some(projection.as_bytes()));
        } else {
            assert!(incoming.is_empty());
            assert!(remote.delivery.idle());
            assert_eq!(poll(&mut remote, &document(PARAMS)).len(), 1, "rejected poll cannot consume ID");
        }
        assert!(remote.flight.is_none());
        assert!(remote.take_effects().is_empty());
    }
}

#[test]
fn duplicate_decoded_keys_reject_before_delivery_and_preserve_retry_identity() {
    for params in [
        r#"{"version":1,"runtime":"old-runtime","op":"list","op":"list"}"#,
        r#"{"version":1,"runtime":"old-runtime","op":"list","\u0072untime":"old-runtime"}"#,
        r#"{"version":1,"runtime":"old-runtime","op":"list","extra":[{"a":1,"\u0061":1}]}"#,
    ] {
        let mut remote = new_remote();
        assert!(poll(&mut remote, &document(params)).is_empty());
        assert!(remote.delivery.idle());
        assert_eq!(poll(&mut remote, &document(PARAMS)).len(), 1);
    }
}

#[test]
fn semantic_rejection_retains_original_runtime_op_raw_bytes_and_request_id() {
    let params = "{ \"version\": true, \"runtime\": \"old-\\u0072untime\", \"op\": \"list\" }";
    let mut remote = new_remote();
    let incoming = poll(&mut remote, &document(params));
    assert_eq!(incoming.len(), 1);
    assert_eq!(incoming[0].id, ID);
    let raw = incoming[0].plex_servers_projection().unwrap();
    assert!(std::str::from_utf8(raw).unwrap().contains("\\u0072"));
    let error = plex_servers::Request::decode(raw).err().unwrap();
    assert_eq!(error.response()["data"]["runtime"], "old-runtime");
    assert_eq!(error.response()["data"]["op"], "list");
    assert_eq!(error.response()["data"]["error"], "invalid_request");
    assert!(remote.flight.is_none());
    assert!(remote.take_effects().is_empty());
}

#[test]
fn value_only_admission_cannot_mint_raw_proof() {
    let mut remote = new_remote();
    let invocation = remote.request_for_test("plex_servers", serde_json::from_str(PARAMS).unwrap());
    assert!(invocation.plex_servers_projection().is_none());
}

#[test]
fn oversized_later_request_rejects_before_the_first_request_can_be_seen() {
    let first = document(PARAMS);
    let later = format!(r#",{{"id":"fedcba9876543210fedcba9876543210","action":"plex_servers","params":{{"padding":"{}"}}}}]}}"#, "x".repeat(16384));
    let body = format!("{}{later}", &first[..first.len() - 2]);
    let mut remote = new_remote();
    assert!(poll(&mut remote, &body).is_empty());
    assert!(remote.delivery.idle());
    assert_eq!(poll(&mut remote, &first).len(), 1);
}

#[test]
fn expired_outer_request_uses_original_correlation_without_dispatch() {
    let mut remote = new_remote();
    assert!(poll(&mut remote, &document(PARAMS).replace("1020", "999")).is_empty());
    let (_, next) = remote.delivery.next(&remote.config);
    let body: Value = serde_json::from_str(next.body.as_deref().unwrap()).unwrap();
    assert_eq!(body, json!({"id":ID,"status":"rejected","data":{"version":1,
        "runtime":"old-runtime","op":"list","state":"error","error":"expired"}}));
}

#[test]
fn complete_reply_bound_counts_json_escaping_and_outer_request_id() {
    let mut remote = new_remote();
    let invocation = poll(&mut remote, &document(PARAMS)).remove(0);
    let result = json!({"status":"ok","data":{"padding":"\"".repeat(32768)}});
    assert!(!remote.delivery.complete(invocation, result));
    let (_, next) = remote.delivery.next(&remote.config);
    let raw = next.body.unwrap();
    assert!(raw.len() <= 65536);
    let body: Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(body["id"], ID);
    assert_eq!(body["data"]["runtime"], "old-runtime");
    assert_eq!(body["data"]["error"], "limit_exceeded");
}
