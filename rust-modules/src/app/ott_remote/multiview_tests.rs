//! Synthetic codec tests. No source resolution, decoder, capability or device I/O.
use super::*;

const FIXTURE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../contracts/multiview-v1.json"
));

fn fixture() -> Value {
    serde_json::from_str(FIXTURE).unwrap()
}

#[test]
fn shared_public_text_corpus_preserves_queries_without_exposing_private_syntax() {
    let fixture = fixture();
    for accepted in fixture["public_text_cases"]["accepted_queries"]
        .as_array()
        .unwrap()
    {
        assert!(
            public_text(accepted.as_str().unwrap(), false),
            "accepted query {accepted:?}"
        );
    }
    for rejected in fixture["public_text_cases"]["rejected"].as_array().unwrap() {
        assert!(
            !public_text(rejected.as_str().unwrap(), false),
            "private query {rejected:?}"
        );
    }
}
fn fixture_request(name: &str) -> Value {
    fixture()["requests"][name].clone()
}
fn decode(value: &Value) -> Request {
    Request::decode(&serde_json::to_vec(value).unwrap()).unwrap()
}
fn refused(value: &Value) {
    assert!(Request::decode(&serde_json::to_vec(value).unwrap()).is_err());
}
fn snapshot() -> Value {
    fixture()["results"]["status"]["snapshot"].clone()
}
fn reply(name: &str, expected: &Value) -> Value {
    let request = decode(&fixture_request(name));
    match request.operation() {
        Operation::Status => {
            request.status_response("native-runtime-1", expected["snapshot"].clone())
        }
        Operation::Sources { .. } => request.sources_response(
            "native-runtime-1",
            expected["items"].as_array().unwrap().clone(),
            expected["cursor"].as_str(),
        ),
        _ => request.accepted(
            "native-runtime-1",
            if expected["outcome"] == "applied" {
                Outcome::Applied
            } else {
                Outcome::Requested
            },
        ),
    }
    .unwrap()
}

#[test]
fn shared_contract_requests_successes_and_rejections_round_trip() {
    let fixture = fixture();
    assert_eq!(fixture["contract"], "multiview");
    assert_eq!(fixture["version"], 1);
    let requests = fixture["requests"].as_object().unwrap();
    assert_eq!(requests.len(), 14);
    for (name, raw) in requests {
        let decoded = decode(raw);
        let mut binding = raw["params"].as_object().unwrap().clone();
        for key in ["version", "runtime", "op"] {
            binding.remove(key);
        }
        assert_eq!(decoded.binding, binding, "{name}");
        assert_eq!(
            reply(name, &fixture["results"][name]),
            json!({"status":"ok","data":fixture["results"][name]}),
            "{name}"
        );
        let reason =
            Reason::parse(fixture["rejections"][name]["reason"].as_str().unwrap()).unwrap();
        assert_eq!(
            decoded.rejected("native-runtime-1", reason).unwrap(),
            json!({"status":"rejected","data":fixture["rejections"][name]}),
            "{name}"
        );
    }
}

#[test]
fn typed_accessors_preserve_context_and_slot_authority() {
    let request = decode(&fixture_request("sources_filtered"));
    assert_eq!(request.runtime(), Some("native-runtime-1"));
    assert_eq!(request.op(), "sources");
    assert_eq!(request.expected().unwrap().scene, "scene-1");
    assert_eq!(request.expected().unwrap().focus_revision, "focus-1");
    assert_eq!(request.slot().unwrap().slot_id, 0);
    assert_eq!(request.slot().unwrap().source_revision, "slot-0-source-1");
    let Operation::Sources { filters, .. } = request.operation() else {
        panic!("wrong operation");
    };
    assert_eq!(filters.provider.as_deref(), Some("m3u"));
    assert_eq!(filters.profile.as_deref(), Some("profile-home"));
    assert_eq!(filters.query.as_deref(), Some("News"));
    assert_eq!(filters.cursor.as_deref(), Some("cursor_1"));
}

#[test]
fn entire_projection_byte_limit_includes_whitespace() {
    let small = serde_json::to_vec(&fixture_request("status")).unwrap();
    let mut exact = vec![b' '; MAX_REQUEST_BYTES - small.len()];
    exact.extend_from_slice(&small);
    assert!(Request::decode(&exact).is_ok());
    exact.push(b' ');
    assert_eq!(
        Request::decode(&exact).err().unwrap().reason,
        Reason::LimitExceeded
    );
}

#[test]
fn original_integer_spellings_and_types_cannot_be_normalized() {
    for (name, field, original, invalid_values) in [
        (
            "status",
            "version",
            "1",
            vec!["true", "null", "1.0", "1e0", "\"1\"", "0", "-0", "2"],
        ),
        (
            "play",
            "slot_id",
            "0",
            vec!["-0", "0.0", "0e0", "true", "null", "-1", "9", "\"0\""],
        ),
        (
            "layout_perimeter",
            "satellites",
            "8",
            vec!["-0", "8.0", "8e0", "true", "null", "-1", "9", "\"8\""],
        ),
    ] {
        let raw = serde_json::to_string(&fixture_request(name)).unwrap();
        let needle = format!("\"{field}\":{original}");
        assert!(raw.contains(&needle));
        for invalid in invalid_values {
            let changed = raw.replace(&needle, &format!("\"{field}\":{invalid}"));
            assert!(
                Request::decode(changed.as_bytes()).is_err(),
                "{name}/{field}/{invalid}"
            );
        }
    }
}

#[test]
fn duplicates_unknown_fields_and_malformed_unicode_are_rejected() {
    for raw in [
        r#"{"action":"multiview","action":"multiview","params":{"version":1,"op":"status"}}"#,
        r#"{"action":"multiview","params":{"version":1,"op":"status","\u006fp":"status"}}"#,
        r#"{"action":"multiview","params":{"version":1,"op":"status","runtime":"\ud800"}}"#,
        r#"{"action":"multiview","params":{"version":NaN,"op":"status"}}"#,
    ] {
        assert!(Request::decode(raw.as_bytes()).is_err());
    }
    let raw = serde_json::to_string(&fixture_request("play")).unwrap();
    let duplicate = raw.replace(
        "\"scene\":\"scene-1\"",
        "\"scene\":\"scene-1\",\"scene\":\"scene-2\"",
    );
    assert_ne!(raw, duplicate);
    assert!(Request::decode(duplicate.as_bytes()).is_err());
    let mut invalid_utf8 =
        br#"{"action":"multiview","params":{"version":1,"op":"status","runtime":""#.to_vec();
    invalid_utf8.extend([0xff, b'"', b'}', b'}']);
    assert!(Request::decode(&invalid_utf8).is_err());
    let mut raw = fixture_request("play");
    raw["params"]["expected"]["uri"] = json!("https://private.invalid/");
    refused(&raw);
    let mut raw = fixture_request("play");
    raw["extra"] = json!(false);
    refused(&raw);
    let mut raw = fixture_request("status");
    raw["params"]["slot_id"] = json!(0);
    refused(&raw);
}

#[test]
fn runtime_context_and_slot_fields_are_exact_and_bounded() {
    for name in ["sources", "layout", "play", "focus", "audio", "stop", "off"] {
        let mut raw = fixture_request(name);
        raw["params"].as_object_mut().unwrap().remove("runtime");
        refused(&raw);
        let mut raw = fixture_request(name);
        raw["params"]["expected"]["scene"] = json!(null);
        refused(&raw);
        let mut raw = fixture_request(name);
        raw["params"]["runtime"] = json!("r".repeat(65));
        refused(&raw);
    }
    for value in [
        json!(null),
        json!(""),
        json!("profile/1"),
        json!("token=private"),
        json!("t".repeat(65)),
    ] {
        let mut raw = fixture_request("play");
        raw["params"]["slot_generation"] = value;
        refused(&raw);
    }
    let mut raw = fixture_request("play");
    raw["params"]["slot_id"] = json!(8);
    assert_eq!(decode(&raw).slot().unwrap().slot_id, 8);
    let mut raw = fixture_request("play");
    raw["params"]["source_ticket"] = json!("t".repeat(256));
    decode(&raw);
    raw["params"]["source_ticket"] = json!("t".repeat(257));
    refused(&raw);
}

#[test]
fn conditional_audio_layout_and_off_fields_never_gain_defaults_on_wire() {
    for name in ["audio_follow", "audio_muted"] {
        for field in ["slot_id", "slot_generation", "source_revision"] {
            let mut raw = fixture_request(name);
            raw["params"][field] = json!(null);
            refused(&raw);
        }
    }
    for field in ["slot_id", "slot_generation", "source_revision"] {
        let mut raw = fixture_request("audio");
        raw["params"].as_object_mut().unwrap().remove(field);
        refused(&raw);
    }
    let mut raw = fixture_request("layout");
    raw["params"]["satellites"] = json!(0);
    refused(&raw);
    let mut raw = fixture_request("layout_perimeter");
    raw["params"].as_object_mut().unwrap().remove("satellites");
    refused(&raw);
    for count in 0..=8 {
        let mut raw = fixture_request("layout_perimeter");
        raw["params"]["satellites"] = json!(count);
        let request = decode(&raw);
        let Operation::Layout(layout) = request.operation() else {
            panic!("wrong layout");
        };
        assert_eq!(layout.name(), "perimeter");
        assert_eq!(layout.slots(), count as u8 + 1);
    }
    let mut raw = fixture_request("off");
    raw["params"].as_object_mut().unwrap().remove("mode");
    refused(&raw);
}

#[test]
fn queries_preserve_unicode_but_refuse_private_syntax_and_format_characters() {
    for query in [
        String::new(),
        "News / Weather".into(),
        "Три кота 日本語".into(),
        "ordinary\u{2065}title".into(),
        "я".repeat(256),
    ] {
        let mut raw = fixture_request("sources");
        raw["params"]["query"] = json!(query);
        decode(&raw);
    }
    for query in [
        "https://private.invalid/?token=private",
        "look /private/file",
        "look ~/Movies",
        "look C:\\Movies",
        "token=private",
        "password: private",
        "Authorization=private",
        "Bearer private",
        "safe\ntext",
        "safe\u{200b}text",
        "safe\u{e0001}text",
    ] {
        let mut raw = fixture_request("sources");
        raw["params"]["query"] = json!(query);
        refused(&raw);
    }
    let mut raw = fixture_request("sources");
    raw["params"]["query"] = json!("я".repeat(257));
    refused(&raw);
    for provider in [
        "",
        "M3U",
        "/m3u",
        "m3u/",
        "a/b/c",
        "http://private",
        "a".repeat(65).as_str(),
    ] {
        let mut raw = fixture_request("sources");
        raw["params"]["provider"] = json!(provider);
        refused(&raw);
    }
    let mut raw = fixture_request("sources");
    raw["params"]["provider"] = json!("operator/edem");
    raw["params"]["cursor"] = json!("c".repeat(256));
    decode(&raw);
}

#[test]
fn runtime_change_rejection_keeps_original_binding_without_new_authority() {
    let request = decode(&fixture_request("play"));
    let result = request
        .rejected("new-runtime", Reason::RuntimeChanged)
        .unwrap();
    assert_eq!(result["status"], "rejected");
    assert_eq!(result["data"]["runtime"], "native-runtime-1");
    assert_eq!(
        result["data"]["binding"],
        Value::Object(request.binding.clone())
    );
    assert_eq!(result["data"].as_object().unwrap().len(), 5);
    assert_eq!(
        request
            .accepted("new-runtime", Outcome::Applied)
            .err()
            .unwrap()
            .reason,
        Reason::RuntimeChanged
    );
    let result = request
        .rejected("new-runtime", Reason::Unsupported)
        .unwrap();
    assert_eq!(result["status"], "unsupported");
    assert_eq!(result["data"]["reason"], "unsupported");
    let status = decode(&super::tests::fixture_request("status"));
    assert_eq!(
        status
            .rejected("current-runtime", Reason::Unavailable)
            .unwrap()["data"]["runtime"],
        "current-runtime"
    );
}

#[test]
fn stale_ticket_rejections_do_not_mint_a_new_ticket_or_context() {
    let mut raw = fixture_request("play");
    raw["params"]["expected"]["scene"] = json!("fresh-scene");
    raw["params"]["slot_generation"] = json!("replacement-slot");
    let request = decode(&raw);
    // Decoding is not ticket admission. The native owner's private resolver must
    // reject this old ticket despite the caller supplying fresh public context.
    let result = request
        .rejected("native-runtime-1", Reason::SourceExpired)
        .unwrap();
    assert_eq!(
        result["data"]["binding"]["source_ticket"],
        "source_ticket_1"
    );
    assert_eq!(
        result["data"]["binding"],
        Value::Object(
            raw["params"]
                .as_object()
                .unwrap()
                .iter()
                .filter(|(k, _)| !["version", "runtime", "op"].contains(&k.as_str()))
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect::<Map<_, _>>()
        )
    );
    assert!(result["data"].get("context").is_none());
    assert!(result["data"].get("ticket").is_none());
}

#[test]
fn mutation_receipts_do_not_claim_playback_or_supply_new_state() {
    let request = decode(&fixture_request("play"));
    for outcome in [Outcome::Requested, Outcome::Applied] {
        let response = request.accepted("native-runtime-1", outcome).unwrap();
        assert_eq!(response["data"].as_object().unwrap().len(), 6);
        assert!(response["data"].get("snapshot").is_none());
        assert!(response["data"].get("playback_confirmed").is_none());
    }
    assert!(decode(&super::tests::fixture_request("status"))
        .accepted("native-runtime-1", Outcome::Applied)
        .is_err());
    assert!(decode(&super::tests::fixture_request("sources"))
        .accepted("native-runtime-1", Outcome::Applied)
        .is_err());
}

#[test]
fn snapshots_resolve_slots_by_identity_and_enforce_audio_and_source_rules() {
    let request = decode(&fixture_request("status"));
    let mut state = snapshot();
    state["slots"].as_array_mut().unwrap().reverse();
    assert!(request.status_response("native-runtime-1", state).is_ok());
    assert!(request
        .status_response(
            "native-runtime-1",
            fixture()["empty_status"]["snapshot"].clone()
        )
        .is_ok());
    for policy in ["muted", "follows_focus", "slot"] {
        let mut state = snapshot();
        state["audio_policy"] = json!(policy);
        state["audio_slot"] = Value::Null;
        assert_eq!(
            request.status_response("native-runtime-1", state).is_ok(),
            policy != "slot"
        );
    }
    for (field, value) in [
        ("focus_slot", json!(4)),
        ("focus_slot", Value::Null),
        ("audio_slot", json!(4)),
        ("audio_slot", json!(1)),
        ("satellites", json!(1)),
    ] {
        let mut state = snapshot();
        state[field] = value;
        assert!(request.status_response("native-runtime-1", state).is_err());
    }
    for state_name in ["starting", "playing", "stalled"] {
        let mut state = snapshot();
        state["slots"][0]["state"] = json!(state_name);
        state["slots"][0]["source"] = Value::Null;
        assert!(request.status_response("native-runtime-1", state).is_err());
    }
    let mut state = snapshot();
    state["slots"][1]["slot_id"] = json!(0);
    assert!(request.status_response("native-runtime-1", state).is_err());
    let mut state = snapshot();
    state["slots"][0]["source"]["ticket"] = json!("private_ticket");
    assert!(request.status_response("native-runtime-1", state).is_err());
}

#[test]
fn paused_slot_retains_its_source_and_distinct_state() {
    let request = decode(&fixture_request("status"));
    let mut state = snapshot();
    state["slots"][0]["state"] = json!("paused");
    let source = state["slots"][0]["source"].clone();
    let response = request
        .status_response("native-runtime-1", state.clone())
        .unwrap();
    assert_eq!(response["data"]["snapshot"]["slots"][0]["state"], "paused");
    assert_eq!(response["data"]["snapshot"]["slots"][0]["source"], source);
    state["slots"][0]["source"] = Value::Null;
    assert!(request.status_response("native-runtime-1", state).is_err());
}

#[test]
fn failed_slot_errors_are_closed_strings_and_never_private_messages() {
    let request = decode(&fixture_request("status"));
    for error in [
        Value::Null,
        json!({"reason":"unavailable"}),
        json!("https://private.invalid"),
        json!("unknown_reason"),
    ] {
        let mut state = snapshot();
        state["slots"][0]["state"] = json!("failed");
        state["slots"][0]["error"] = error;
        assert!(request.status_response("native-runtime-1", state).is_err());
    }
    let mut state = snapshot();
    state["slots"][0]["state"] = json!("failed");
    state["slots"][0]["error"] = json!("source_unavailable");
    assert!(request.status_response("native-runtime-1", state).is_ok());
}

#[test]
fn source_pages_are_closed_unique_and_cursor_consistent() {
    let request = decode(&fixture_request("sources"));
    let row = fixture()["results"]["sources"]["items"][0].clone();
    let response = request
        .sources_response("native-runtime-1", vec![row.clone()], Some("next_cursor"))
        .unwrap();
    assert_eq!(response["data"]["complete"], false);
    assert_eq!(response["data"]["cursor"], "next_cursor");
    assert!(request
        .sources_response("native-runtime-1", vec![], Some(""))
        .is_err());
    assert!(request
        .sources_response("native-runtime-1", vec![row.clone(); 129], None)
        .is_err());
    assert!(request
        .sources_response("native-runtime-1", vec![row.clone(), row.clone()], None)
        .is_err());
    for (key, value) in [
        ("source_id", json!("s".repeat(65))),
        ("profile", Value::Null),
        ("label", json!(" ")),
        ("label", json!("file:/private/movie")),
        ("uri", json!("https://private.invalid")),
    ] {
        let mut bad = row.clone();
        bad[key] = value;
        assert!(request
            .sources_response("native-runtime-1", vec![bad], None)
            .is_err());
    }
    let mut other = row.clone();
    other["source_id"] = json!("other");
    assert!(request
        .sources_response("native-runtime-1", vec![row.clone(), other], None)
        .is_err());
    let mut other = row.clone();
    other["ticket"] = json!("other");
    assert!(request
        .sources_response("native-runtime-1", vec![row, other], None)
        .is_err());
}

#[test]
fn response_budget_reserves_space_for_the_original_delivery_id() {
    let request = decode(&fixture_request("status"));
    // Exercise the private encoder's byte budget independently from the smaller
    // valid snapshot/source-page shapes. Public callers still validate those.
    let base = request
        .reply("native-runtime-1", "ok", json!({"padding":""}))
        .unwrap();
    let overhead = serde_json::to_vec(&base).unwrap().len();
    let padding = "x".repeat(MAX_CODEC_RESPONSE_BYTES - overhead);
    let response = request
        .reply("native-runtime-1", "ok", json!({"padding":padding}))
        .unwrap();
    assert_eq!(
        serde_json::to_vec(&response).unwrap().len(),
        MAX_CODEC_RESPONSE_BYTES
    );
    let mut delivered = response;
    delivered["id"] = json!("a".repeat(32));
    assert!(serde_json::to_vec(&delivered).unwrap().len() <= MAX_RESPONSE_BYTES);
    assert_eq!(
        request
            .reply(
                "native-runtime-1",
                "ok",
                json!({"padding":"x".repeat(MAX_CODEC_RESPONSE_BYTES - overhead + 1)})
            )
            .err()
            .unwrap()
            .reason,
        Reason::LimitExceeded
    );
}

#[test]
fn maximum_source_page_fits_the_complete_delivery_envelope() {
    let request = decode(&fixture_request("sources"));
    let rows = (0..128)
        .map(|index| {
            json!({
                "source_id":format!("s{index:03}{}", "s".repeat(60)),
                "ticket":format!("t{index:03}{}", "t".repeat(252)),
                "label":"\"".repeat(512),
                "provider":"p".repeat(64),
                "profile":"p".repeat(64),
                "kind":"file"
            })
        })
        .collect();
    let mut response = request
        .sources_response("native-runtime-1", rows, Some(&"c".repeat(256)))
        .unwrap();
    response["id"] = json!("b".repeat(32));
    assert_eq!(response["data"]["items"].as_array().unwrap().len(), 128);
    assert!(serde_json::to_vec(&response).unwrap().len() <= MAX_RESPONSE_BYTES);
}
