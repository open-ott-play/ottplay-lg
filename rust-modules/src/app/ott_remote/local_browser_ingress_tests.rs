//! Real Delivery admission/cache paths; no receiver worker or transport starts.
use super::*;

fn request(id: u64, action: &str, params: Value, expires: u64) -> Value {
    json!({"commands":[],"server_time":1000,"requests":[{
        "id":format!("{id:032x}"),"action":action,"params":params,"expires_at":expires
    }]})
}

fn operations() -> Vec<(&'static str, Value)> {
    let mut cases = Vec::new();
    for op in ["roots", "list", "more", "select"] {
        let mut p = json!({"version":1,"runtime":"original-runtime","op":op});
        match op {
            "list" => {
                p["generation"] = json!("issued-generation");
                p["entry_id"] = json!("issued-entry");
            }
            "more" | "select" => {
                p["generation"] = json!("issued-generation");
                p["select_id"] = json!("issued-select");
            }
            _ => {}
        }
        cases.push(("local_browser", p));
    }
    for op in ["connect", "play", "shuffle"] {
        cases.push((
            "local_media",
            json!({"runtime":"original-runtime","op":op,"source_handle":"issued-handle"}),
        ));
    }
    cases
}

fn check_error(value: &Value, action: &str, params: &Value, code: &str) {
    assert_eq!(value["status"], "rejected");
    assert_eq!(
        value["data"],
        json!({"version":1,"runtime":"original-runtime","op":params["op"],
        "state":"error","error":if action == "local_media" {"runtime_changed"} else {code}})
    );
    assert!(serde_json::to_vec(value).unwrap().len() < 64 * 1024);
}

#[test]
fn controller252_expired_requests_keep_original_family_before_dispatch() {
    for (action, params) in operations() {
        let mut delivery = Delivery::default();
        let input = request(1, action, params.clone(), 999);
        assert!(delivery.poll(&input, 500, 0).unwrap().is_empty());
        assert!(delivery.pending_requests.is_empty() && delivery.cache.is_empty());
        let response = delivery.responses.front().unwrap();
        assert_eq!(response["id"], format!("{:032x}", 1));
        check_error(response, action, &params, "expired");
    }
}

#[test]
fn controller252_completed_replay_then_real_cache_eviction_never_redispatches() {
    for (action, params) in operations() {
        let mut delivery = Delivery::default();
        let first = request(1, action, params.clone(), 1020);
        let inv = delivery.poll(&first, 500, 0).unwrap().remove(0);
        // The application has already settled this request. Preserve that exact
        // response until normal cache eviction, rather than recomputing it.
        let settled = json!({"status":"rejected","data":{"version":1,"runtime":"original-runtime",
            "op":params["op"],"state":"error","error":if action == "local_media" {"source_not_found"} else {"stale"}}});
        assert!(!delivery.complete(inv, settled));
        let receipt = delivery.responses.pop_front().unwrap();
        assert!(delivery.poll(&first, 501, 0).unwrap().is_empty());
        assert_eq!(delivery.responses.pop_front().unwrap(), receipt);
        for n in 0..CACHE_LIMIT {
            let input = request(n as u64 + 2, "status", json!({}), 1020);
            let inv = delivery.poll(&input, 502, 0).unwrap().remove(0);
            assert!(delivery.complete(inv, json!({"status":"ok","data":{}})));
            delivery.responses.pop_front().unwrap();
        }
        let id = format!("{:032x}", 1);
        assert!(delivery.seen_requests.contains(&id));
        assert!(!delivery.cache.iter().any(|(key, _)| key == &id));
        assert!(delivery.poll(&first, 503, 0).unwrap().is_empty());
        let response = delivery.responses.pop_front().unwrap();
        assert_eq!(response["id"], id);
        check_error(&response, action, &params, "cancelled");
        assert!(delivery.pending_requests.is_empty());
    }
}
