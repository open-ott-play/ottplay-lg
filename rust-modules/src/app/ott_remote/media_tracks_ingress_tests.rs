//! Real raw transport completion; no worker, network, device or storage access.
use super::*;

const REQUEST_ID: &str = "0123456789abcdef0123456789abcdef";
const ACTION: &str = r#""media_tracks""#;
const PARAMS: &str = r#"{"op":"list","runtime":"runtime-fixture"}"#;

fn raw_poll(remote: &mut Remote, body: &str) -> Vec<Invocation> {
    remote.finish(
        Kind::Delivery(SendKind::Poll),
        Outcome::Http {
            response: Ok(protocol::Response { status: 200, body: body.as_bytes().to_vec() }),
            elapsed_ms: 10,
        },
        100,
    )
}

fn document(fields: &str) -> String {
    format!(r#"{{"commands":[{{"id":"legacy-fixture","key":"up"}}],"server_time":1000,"requests":[{{"id":"{REQUEST_ID}","expires_at":1020,{fields}}}]}}"#)
}

fn projection(action: &str, params: &str) -> String {
    format!(r#"{{"action":{action},"params":{params}}}"#)
}

fn track_document(action: &str, params: &str) -> String {
    document(&format!(r#""action":{action},"params":{params}"#))
}

fn padded_params(action: &str, bytes: usize) -> String {
    let padding = bytes.checked_sub(21 + action.len() + PARAMS.len()).unwrap();
    format!("{}{}{}", &PARAMS[..PARAMS.len() - 1], " ".repeat(padding), "}")
}

fn assert_refused_without_remembering_ids(body: &str) {
    let mut remote = Remote::disabled_for_test("dev_fixture", "runtime-fixture");
    assert!(raw_poll(&mut remote, body).is_empty());
    assert_eq!(remote.snapshot()["state"], "error");
    assert!(remote.delivery.idle(), "invalid poll must not queue acknowledgments");
    assert!(remote.flight.is_none());
    assert!(remote.take_effects().is_empty());
    let admitted = raw_poll(&mut remote, &track_document(ACTION, PARAMS));
    assert_eq!(admitted.len(), 2, "both IDs must still be eligible after rejection");
    assert_eq!(admitted[0].id, "legacy-fixture");
    assert_eq!(admitted[1].id, REQUEST_ID);
    assert_eq!(admitted[1].action, "media_tracks");
    assert_eq!(admitted[1].media_tracks_projection(), Some(projection(ACTION, PARAMS).as_bytes()));
}

#[test]
fn raw_track_projection_accepts_exact_limit_and_rejects_one_more_before_seen_ids() {
    for action in [ACTION, r#""media\u005ftracks""#] {
        let params = padded_params(action, 2048);
        let expected = projection(action, &params);
        assert_eq!(expected.len(), 2048);
        let mut remote = Remote::disabled_for_test("dev_fixture", "runtime-fixture");
        let body = track_document(action, &params);
        assert!(body.len() > 2048, "transport overhead is not part of the projection bound");
        let admitted = raw_poll(&mut remote, &body);
        assert_eq!(admitted.len(), 2);
        assert!(admitted[0].media_tracks_projection().is_none());
        assert_eq!(admitted[1].media_tracks_projection(), Some(expected.as_bytes()));
        assert_eq!(admitted[1].params, json!({"op":"list","runtime":"runtime-fixture"}));
        assert_eq!(admitted[1].deadline_ms, 20_090);
        assert!(media_tracks::Request::decode(admitted[1].media_tracks_projection().unwrap()).is_ok());
        assert!(raw_poll(&mut remote, &body).is_empty(), "valid pending request retains replay protection");

        let too_large = padded_params(action, 2049);
        assert_eq!(projection(action, &too_large).len(), 2049);
        assert_refused_without_remembering_ids(&track_document(action, &too_large));
    }
}

#[test]
fn raw_track_projection_preserves_escapes_and_internal_whitespace_only() {
    let action = r#""media\u005ftracks""#;
    let params = "{\n \"runtime\" : \"runtime-\\u0066ixture\", \"op\" : \"list\"\n}";
    let body = document(&format!("\"action\" : \n {action} \t , \"params\" : \n {params} \t"));
    let expected = projection(action, params);
    let mut remote = Remote::disabled_for_test("dev_fixture", "runtime-fixture");
    let admitted = raw_poll(&mut remote, &body);
    assert_eq!(admitted.len(), 2);
    let request = &admitted[1];
    assert_eq!(request.action, "media_tracks");
    assert_eq!(request.media_tracks_projection(), Some(expected.as_bytes()));
    assert_eq!(request.params["runtime"], "runtime-fixture");
    assert!(expected.contains("\\u0066") && expected.contains('\n'));
    assert_ne!(expected, json!({"action":"media_tracks","params":request.params.clone()}).to_string());
}

#[test]
fn transport_metadata_overhead_does_not_consume_track_projection_budget() {
    let body = track_document(ACTION, PARAMS);
    let body = body.replacen("\"server_time\":1000", &format!("\"trace\":\"{}\",\"server_time\":1000", "x".repeat(4096)), 1);
    assert!(body.len() > 4096);
    let mut remote = Remote::disabled_for_test("dev_fixture", "runtime-fixture");
    let admitted = raw_poll(&mut remote, &body);
    assert_eq!(admitted.len(), 2);
    assert_eq!(admitted[1].media_tracks_projection(), Some(projection(ACTION, PARAMS).as_bytes()));
    assert_eq!(remote.snapshot()["state"], "connected");
}

#[test]
fn malformed_track_fields_and_missing_or_null_params_never_remember_ids() {
    for fields in [
        r#""action":"media_tracks""#,
        r#""action":"media_tracks","params":null"#,
        r#""action":"media_tracks","params":[]"#,
        r#""action":"media_tracks","params":{}"#,
        r#""action":"media_tracks","params":{"op":"list","runtime":"runtime-fixture","extra":true}"#,
        r#""action":"media\u005ftracks","params":{"op":"list","runtime":"UPPER"}"#,
        r#""action":"media_tracks","params":{"op":"select","runtime":"runtime-fixture","playback":"p","source_generation":"s","catalog":"c","target":{"kind":"audio","mode":"off"}}"#,
    ] {
        assert_refused_without_remembering_ids(&document(fields));
    }
}

#[test]
fn duplicate_decoded_keys_anywhere_in_track_poll_still_reject_atomically() {
    for fields in [
        r#""action":"media_tracks","\u0061ction":"media_tracks","params":{"op":"list","runtime":"runtime-fixture"}"#,
        r#""action":"media_tracks","params":{"op":"list","op":"list","runtime":"runtime-fixture"}"#,
        r#""action":"media_tracks","params":{"op":"list","runtime":"runtime-fixture","\u0072untime":"runtime-fixture"}"#,
        r#""action":"media_tracks","params":{"op":"list","runtime":"runtime-fixture"},"metadata":{"unused":[{"a":1,"\u0061":2}]}"#,
    ] {
        assert_refused_without_remembering_ids(&document(fields));
    }
}

#[test]
fn ignored_later_track_request_is_validated_before_first_request_or_commands_are_seen() {
    let first = document(r#""action":"playback","params":{"operation":"pause"}"#);
    let later = r#",{"id":"fedcba9876543210fedcba9876543210","action":"media_tracks","params":null}]}"#;
    let body = format!("{}{later}", &first[..first.len() - 2]);
    assert_refused_without_remembering_ids(&body);
    let large = padded_params(ACTION, 2049);
    let body = [
        &first[..first.len() - 2],
        r#",{"id":"fedcba9876543210fedcba9876543210","action":"#,
        ACTION,
        r#","params":"#,
        &large,
        "}]}",
    ].concat();
    assert_refused_without_remembering_ids(&body);
}

#[test]
fn ordinary_and_unknown_actions_keep_existing_value_only_semantics() {
    for (fields, action, params) in [
        (r#""action":"playback","params":{"operation":"pause"}"#, "playback", json!({"operation":"pause"})),
        (r#""action":"future_action""#, "future_action", json!({})),
    ] {
        let mut remote = Remote::disabled_for_test("dev_fixture", "runtime-fixture");
        let admitted = raw_poll(&mut remote, &document(fields));
        assert_eq!(admitted.len(), 2);
        assert_eq!(admitted[1].action, action);
        assert_eq!(admitted[1].params, params);
        assert!(admitted[1].media_tracks_projection().is_none());
    }
}

#[test]
fn ordinary_large_params_and_command_only_envelope_keep_previous_rules() {
    let fields = format!(r#""action":"future_action","params":{{"opaque":"{}"}}"#, "x".repeat(4096));
    let mut remote = Remote::disabled_for_test("dev_fixture", "runtime-fixture");
    let admitted = raw_poll(&mut remote, &document(&fields));
    assert_eq!(admitted.len(), 2);
    assert_eq!(admitted[1].params["opaque"].as_str().unwrap().len(), 4096);
    assert!(admitted[1].media_tracks_projection().is_none());

    let mut remote = Remote::disabled_for_test("dev_fixture", "runtime-fixture");
    let admitted = raw_poll(&mut remote, r#"{"commands":[{"id":"legacy-fixture","key":"up"}],"extra":{"anything":true}}"#);
    assert_eq!(admitted.len(), 1);
    assert_eq!(admitted[0].action, "command");
    assert!(admitted[0].media_tracks_projection().is_none());
}
