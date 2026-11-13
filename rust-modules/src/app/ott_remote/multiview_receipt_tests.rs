//! Exercise the real Delivery owner, not a second replay implementation.
use super::*;

#[test]
fn generated_delivery_failures_match_controller_contract_vectors() {
    let fixture: Value = serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../contracts/multiview-v1.json"
    )))
    .unwrap();
    let vectors = fixture["native_delivery_rejections"].as_array().unwrap();
    assert_eq!(vectors.len(), 4);
    for vector in vectors {
        let mut delivery = Delivery::default();
        let id = "0123456789abcdef0123456789abcdef";
        let mut invocation = delivery
            .poll(&poll(id, vector["request"]["params"].clone()), 100, 0)
            .unwrap()
            .pop()
            .unwrap();
        invocation.multiview_raw = Some(vector["request"].to_string());
        let result = if vector["response"]["data"]["reason"] == "limit_exceeded" {
            json!({"status":"ok","data":{"oversized":"x".repeat(256 * 1024)}})
        } else {
            json!({"invalid":true})
        };
        assert!(!delivery.complete(invocation, result));
        let mut expected = vector["response"].clone();
        expected["id"] = json!(id);
        assert_eq!(
            delivery.cache.back().unwrap().1,
            expected,
            "{}",
            vector["name"]
        );
    }
}

fn poll(id: &str, params: Value) -> Value {
    json!({"commands":[], "server_time":1000, "requests":[{
        "id":id,"action":"multiview","params":params,"expires_at":1020
    }]})
}

#[test]
fn pending_duplicate_never_retargets_the_original_invocation() {
    let mut delivery = Delivery::default();
    let id = "0123456789abcdef0123456789abcdef";
    let original = json!({"version":1,"op":"focus","runtime":"first","slot_id":0});
    let mut admitted = delivery.poll(&poll(id, original.clone()), 100, 0).unwrap();
    let invocation = admitted.pop().unwrap();
    let deadline = invocation.deadline_ms;
    assert!(
        delivery
            .poll(
                &poll(
                    id,
                    json!({"version":1,"op":"focus","runtime":"second","slot_id":8})
                ),
                200,
                0
            )
            .unwrap()
            .is_empty()
    );
    assert_eq!(invocation.params, original);
    assert_eq!(invocation.deadline_ms, deadline);
    assert!(delivery.complete(
        invocation,
        json!({"status":"ok","data":{"binding":original}})
    ));
    assert_eq!(
        delivery.cache.back().unwrap().1["data"]["binding"]["slot_id"],
        0
    );
}

#[test]
fn lost_ack_replays_original_terminal_receipt_after_focus_changes() {
    let mut delivery = Delivery::default();
    let id = "0123456789abcdef0123456789abcdef";
    let mut actions = delivery
        .poll(
            &poll(id, json!({"version":1,"op":"focus","slot_id":0})),
            100,
            0,
        )
        .unwrap();
    delivery.complete(
        actions.pop().unwrap(),
        json!({"status":"ok","data":{"binding":{"slot_id":0},"outcome":"applied"}}),
    );
    let original = delivery.cache.back().unwrap().1.clone();
    delivery.responses.clear();
    assert!(
        delivery
            .poll(
                &poll(id, json!({"version":1,"op":"focus","slot_id":8})),
                200,
                0
            )
            .unwrap()
            .is_empty()
    );
    assert_eq!(delivery.responses.back(), Some(&original));
}

#[test]
fn evicted_terminal_receipt_does_not_fabricate_binding_from_redelivery() {
    let mut delivery = Delivery::default();
    for index in 0..=CACHE_LIMIT {
        let id = format!("{index:032x}");
        let mut actions = delivery
            .poll(
                &poll(&id, json!({"version":1,"op":"focus","slot_id":0})),
                100,
                0,
            )
            .unwrap();
        delivery.complete(
            actions.pop().unwrap(),
            json!({"status":"ok","data":{"binding":{"slot_id":0}}}),
        );
    }
    assert_eq!(delivery.cache.len(), CACHE_LIMIT);
    delivery.responses.clear();
    assert!(
        delivery
            .poll(
                &poll(
                    "00000000000000000000000000000000",
                    json!({"version":1,"op":"focus","slot_id":8})
                ),
                200,
                0
            )
            .unwrap()
            .is_empty()
    );
    assert!(delivery.responses.is_empty());
}

#[test]
fn full_cached_response_including_request_id_is_bounded() {
    let mut delivery = Delivery::default();
    let id = "0123456789abcdef0123456789abcdef";
    let params = json!({"version":1,"op":"status","runtime":"original-runtime"});
    let mut actions = delivery.poll(&poll(id, params.clone()), 100, 0).unwrap();
    let mut invocation = actions.pop().unwrap();
    invocation.multiview_raw = Some(json!({"action":"multiview","params":params}).to_string());
    delivery.complete(
        invocation,
        json!({"status":"ok","data":{"invalid_fixture":"x".repeat(256*1024)}}),
    );
    let receipt = &delivery.cache.back().unwrap().1;
    assert_eq!(receipt["status"], "rejected");
    assert_eq!(receipt["data"]["runtime"], "original-runtime");
    assert_eq!(receipt["data"]["version"], 1);
    assert_eq!(receipt["data"]["op"], "status");
    assert_eq!(receipt["data"]["binding"], json!({}));
    assert_eq!(receipt["data"]["reason"], "limit_exceeded");
    assert!(serde_json::to_vec(receipt).unwrap().len() <= 256 * 1024);
}

#[test]
fn expired_without_original_projection_never_invents_a_family_receipt() {
    let mut delivery = Delivery::default();
    let mut envelope = poll(
        "0123456789abcdef0123456789abcdef",
        json!({"version":1,"op":"status"}),
    );
    envelope["requests"][0]["expires_at"] = json!(999);
    assert!(delivery.poll(&envelope, 100, 0).unwrap().is_empty());
    assert!(delivery.responses.is_empty());
}

#[test]
fn invalid_completion_without_original_runtime_remains_unknown() {
    let mut delivery = Delivery::default();
    let mut actions = delivery
        .poll(
            &poll(
                "0123456789abcdef0123456789abcdef",
                json!({"version":1,"op":"status"}),
            ),
            100,
            0,
        )
        .unwrap();
    assert!(!delivery.complete(actions.pop().unwrap(), json!({"invalid":true})));
    assert!(delivery.responses.is_empty());
    assert!(delivery.cache.is_empty());
}
