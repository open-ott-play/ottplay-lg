//! Delivery-only regressions: no owner, process, clock or network fixture.
use super::*;
const ID: &str = "00000000000000000000000000000001";
fn envelope(id: &str, action: &str, params: Value) -> Value {
    json!({"commands":[],"server_time":1000,"requests":[{
        "id":id,"action":action,"params":params,"expires_at":1001}]})
}
fn selection() -> Value {
    envelope(ID, "plex_servers", json!({"version":1,"runtime":"original","op":"select",
        "server_id":"machine-one","context":"original-context"}))
}
fn take_response(delivery: &mut Delivery) -> Value {
    let (kind, request) = delivery.next(&Config::disabled("fixture".into()));
    assert!(matches!(kind, SendKind::Response));
    let body = serde_json::from_str(&request.body.unwrap()).unwrap();
    assert!(delivery.accepted(&kind, &json!({"status":"ok"})).unwrap().is_some());
    body
}
#[test]
fn expired_pending_managed_request_stays_silent_until_original_completion() {
    let mut delivery = Delivery::default();
    let original = selection();
    let invocation = delivery.poll(&original, 10, 0).unwrap().remove(0);
    assert_eq!(invocation.deadline_ms, 1010);
    assert!(delivery.poll(&original, 1000, 0).unwrap().is_empty());
    assert!(delivery.idle());
    // Expiry removes only Delivery's pending receipt. It cannot prove that
    // application effects stopped, nor correlate from a redelivered body.
    let changed = envelope(ID, "plex_servers", json!({"version":1,"runtime":"different","op":"list"}));
    assert!(delivery.poll(&changed, 1010, 0).unwrap().is_empty());
    assert!(delivery.pending_requests.is_empty());
    assert!(delivery.idle(), "no synthetic cancellation and no invitation to retry an uncertain operation");
    let result = json!({"status":"rejected","data":{"version":1,"runtime":"original",
        "op":"select","state":"error","error":"outcome_unknown"}});
    assert!(!delivery.complete(invocation, result.clone()), "a rejected result is still queued and cached");
    let mut expected = result; expected["id"] = json!(ID);
    assert_eq!(take_response(&mut delivery), expected);
    assert!(delivery.poll(&changed, 1011, 0).unwrap().is_empty());
    assert_eq!(take_response(&mut delivery), expected, "replay uses the original completed receipt");
    assert!(delivery.idle());
}
#[test]
fn evicted_managed_success_is_not_replaced_with_cancellation_or_redispatched() {
    let mut delivery = Delivery::default();
    let original = selection();
    let invocation = delivery.poll(&original, 10, 0).unwrap().remove(0);
    let success = json!({"status":"ok","data":{"version":1,"runtime":"original","op":"select",
        "state":"ready","server_id":"machine-one","context":"original-context",
        "activated":true,"persisted":false,"playback_confirmed":false}});
    assert!(delivery.complete(invocation, success.clone()));
    let mut expected = success; expected["id"] = json!(ID);
    assert_eq!(take_response(&mut delivery), expected);
    // Exercise the actual FIFO eviction through admitted/completed requests.
    for n in 0..CACHE_LIMIT {
        let id = format!("{:032x}", n + 2);
        let input = envelope(&id, "status", json!({}));
        let invocation = delivery.poll(&input, 20, 0).unwrap().remove(0);
        assert!(delivery.complete(invocation, json!({"status":"ok","data":{}})));
        assert_eq!(take_response(&mut delivery)["id"], id);
    }
    assert!(!delivery.cache.iter().any(|(id, _)| id == ID));
    assert!(delivery.seen_requests.iter().any(|id| id == ID));
    assert!(delivery.poll(&original, 30, 0).unwrap().is_empty());
    assert!(delivery.idle(), "eviction does not undo a successful activation");
}
#[test]
fn non_managed_expired_duplicate_keeps_legacy_receipt_behavior() {
    let mut delivery = Delivery::default();
    let input = envelope(ID, "favorites_global_items", json!({"runtime":"original","op":"current","subject":"global"}));
    assert_eq!(delivery.poll(&input, 10, 0).unwrap().len(), 1);
    assert!(delivery.poll(&input, 1010, 0).unwrap().is_empty());
    assert_eq!(take_response(&mut delivery)["data"]["error"], "cancelled");
}
