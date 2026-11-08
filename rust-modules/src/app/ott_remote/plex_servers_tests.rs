//! Source-authored producer tests; shared fixture bytes remain unmodified.
use super::*;

fn fixture() -> Value { serde_json::from_str(include_str!("plex-servers-v1.json")).unwrap() }
fn request(op: &str) -> Request {
    let params = if op == "list" { json!({"version":1,"runtime":"native-qa-1","op":"list"}) }
        else { json!({"version":1,"runtime":"native-qa-1","op":"select","server_id":"PMS-1","context":"row-opaque-example"}) };
    Request::decode(json!({"action":"plex_servers","params":params}).to_string().as_bytes())
        .unwrap_or_else(|_| panic!("valid fixture request"))
}

#[test]
fn shared_fifty_raw_request_vectors_are_decoded_before_projection() {
    let data = fixture();
    let cases = data["request_cases"].as_array().unwrap();
    assert_eq!(cases.len(), 50);
    for case in cases {
        let raw = case["request_json"].as_str().unwrap().as_bytes();
        let decoded = Request::decode(raw);
        assert_eq!(decoded.is_ok(), case["valid"].as_bool().unwrap(), "{}", case["id"]);
        if let Err(error) = decoded {
            if case["typed_correlation"] == "unavailable" {
                assert!(error.correlation.is_none(), "{}", case["id"]);
            } else if raw.len() <= 16 * 1024 && super::super::ingress::decode_unique(raw).is_ok() {
                // Oversize/malformed raw bodies may be rejected by the outer boundary.
                // Once unique JSON reaches semantic validation, retain valid runtime/op.
                assert!(error.correlation.is_some(), "{}", case["id"]);
            }
        }
    }
}

#[test]
fn semantic_error_preserves_original_runtime_and_never_defaults_operation() {
    let raw = br#"{"action":"plex_servers","params":{"version":true,"runtime":"old-runtime","op":"select"}}"#;
    let error = Request::decode(raw).err().unwrap();
    assert_eq!(error.response(), json!({"status":"rejected","data":{"version":1,"runtime":"old-runtime","op":"select","state":"error","error":"invalid_request"}}));
    for raw in [
        br#"{"action":"plex_servers","params":{"version":1,"runtime":"UPPER","op":"list"}}"#.as_slice(),
        br#"{"action":"plex_servers","params":{"version":1,"runtime":"valid","op":"LIST"}}"#.as_slice(),
        br#"{"action":"plex_servers","params":{"version":1,"runtime":"valid","op":"list","op":"list"}}"#.as_slice(),
    ] {
        let error = Request::decode(raw).err().unwrap();
        assert!(error.correlation.is_none());
        assert!(error.response()["data"].get("runtime").is_none());
        assert!(error.response()["data"].get("op").is_none());
    }
}

#[test]
fn producer_successes_match_shared_examples_and_fixed_activation_claims() {
    let data = fixture();
    for id in ["list-one", "list-empty", "select-applied"] {
        let case = data["reply_cases"].as_array().unwrap().iter().find(|v| v["id"] == id).unwrap();
        let expected: Value = serde_json::from_str(case["response_json"].as_str().unwrap()).unwrap();
        let params = &case["request_params"];
        let request = Request::decode(json!({"action":"plex_servers","params":params}).to_string().as_bytes())
            .unwrap_or_else(|_| panic!("valid request"));
        let mut reply = if id == "select-applied" { request.applied_selection_response().unwrap() }
            else {
                let rows: Vec<_> = expected["data"]["servers"].as_array().unwrap().iter().map(|row|
                    Row::new(row["server_id"].as_str().unwrap().into(), row["name"].as_str().unwrap().into(),
                        row["context"].as_str().unwrap().into(), row["expires_in_ms"].as_u64().unwrap() as u32).unwrap()).collect();
                request.list_response(&rows).unwrap()
            };
        reply["id"] = data["expected_request_id"].clone();
        assert_eq!(reply, expected, "{id}");
    }
}

#[test]
fn producer_errors_use_only_closed_codes_and_original_correlation() {
    let data = fixture();
    for op in ["list", "select"] {
        for error in [Error::InvalidRequest, Error::Unsupported, Error::Busy, Error::Stale,
            Error::SourceChanged, Error::AuthorizationConflict, Error::Unavailable, Error::Restricted,
            Error::Expired, Error::Cancelled, Error::LimitExceeded, Error::ActivationFailed, Error::OutcomeUnknown] {
            let id = format!("{op}-error-{}", error.code());
            let case = data["reply_cases"].as_array().unwrap().iter().find(|v| v["id"] == id).unwrap();
            let expected: Value = serde_json::from_str(case["response_json"].as_str().unwrap()).unwrap();
            let mut reply = request(op).correlation().rejected(error);
            reply["id"] = data["expected_request_id"].clone();
            // The contract permits both rejected and unsupported for unavailable action.
            assert_eq!(reply["data"], expected["data"], "{id}");
            assert_eq!(reply["status"], if error == Error::Unsupported { "unsupported" } else { "rejected" });
        }
    }
}

#[test]
fn row_builder_enforces_display_bounds_and_one_row_without_silent_truncation() {
    for name in ["", "\u{0}", "\u{7f}", "\u{85}"] {
        assert!(Row::new("PMS-1".into(), name.into(), "ctx".into(), 1).is_err());
    }
    assert!(Row::new("PMS-1".into(), "é".repeat(257), "ctx".into(), 1).is_err());
    for expiry in [0, 120001] {
        assert!(Row::new("PMS-1".into(), "family".into(), "ctx".into(), expiry).is_err());
    }
    let make = || Row::new("PMS-1".into(), "é".repeat(256), "ctx".into(), 120000).unwrap();
    assert_eq!(request("list").list_response(&[make(), make()]), Err(Error::LimitExceeded));
    assert_eq!(request("select").list_response(&[]), Err(Error::InvalidRequest));
    assert_eq!(request("list").applied_selection_response(), Err(Error::InvalidRequest));
    let mut largest = request("list").list_response(&[
        Row::new("A".repeat(256), "\"".repeat(512), "x".repeat(256), 120000).unwrap()
    ]).unwrap();
    largest["id"] = json!("0123456789abcdef0123456789abcdef");
    assert!(largest.to_string().len() < 2048, "bounded escaping remains below the whole 64-KiB envelope cap");
}
