use super::*;

#[test]
fn request_forms_match_classic_without_extra_keys_or_coercions() {
    for value in [
        json!({"mode":"status"}),
        json!({"mode":"off"}),
        json!({"mode":"on"}),
        json!({"mode":"on","strict":true}),
        json!({"mode":"on","query":"2","strict":false}),
        json!({"mode":"set","query":"A channel"}),
    ] {
        assert!(parse(&value).is_ok());
    }
    for value in [
        Value::Null,
        json!([]),
        json!({}),
        json!({"mode":"ON"}),
        json!({"mode":"status","query":"A"}),
        json!({"mode":"off","strict":false}),
        json!({"mode":"set"}),
        json!({"mode":"on","query":""}),
        json!({"mode":"on","query":"\u{feff}"}),
        json!({"mode":"on","query":2}),
        json!({"mode":"on","strict":1}),
        json!({"mode":"on","strict":null}),
        json!({"mode":"on","query":"A\nB"}),
        json!({"mode":"on","query":"A\u{7f}B"}),
        json!({"mode":"on","runtime":"other"}),
    ] {
        assert!(parse(&value).is_err(), "{value}");
    }
}

#[test]
fn query_bound_counts_utf8_bytes_and_trims_the_classic_space_set() {
    assert!(parse(&json!({"mode":"set","query":"é".repeat(512)})).is_ok());
    assert!(parse(&json!({"mode":"set","query":"é".repeat(513)})).is_err());
    let request = parse(&json!({"mode":"set","query":"\u{feff}\u{a0}Channel 1\u{3000}"})).unwrap();
    assert_eq!(request.query.as_deref(), Some("Channel 1"));
    assert_eq!(
        parse(&json!({"mode":"set","query":"0002"}))
            .unwrap()
            .query
            .as_deref(),
        Some("0002")
    );
}

#[test]
fn absent_policy_and_present_null_have_different_operation_fingerprints() {
    let mut settings = BTreeMap::new();
    let off = fingerprint(&settings);
    settings.insert(stored::KEY.into(), Value::Null);
    assert_ne!(off, fingerprint(&settings));
    assert!(stored::read(&settings).is_err());
}

#[test]
fn completion_must_match_the_whole_admitted_request() {
    let mut remote = Remote::disabled_for_test("dev_fixture", "fixture");
    let invocation = remote.request_for_test("kiosk", json!({"mode":"off"}));
    let pending = Pending {
        invocation: invocation.clone(),
        expected: String::new(),
        prepared: None,
        playback_epoch: 0,
        navigation_epoch: 0,
    };
    assert!(pending.matches(&invocation));
    let mut changed = invocation.clone();
    changed.params = json!({"mode":"on"});
    assert!(!pending.matches(&changed));
    changed = invocation.clone();
    changed.id.push('0');
    assert!(!pending.matches(&changed));
    changed = invocation;
    changed.deadline_ms += 1;
    assert!(!pending.matches(&changed));
}

#[test]
fn post_save_failure_never_claims_the_policy_was_not_saved() {
    let response = saved_failure("Kiosk policy was saved, but could not be applied.");
    assert_eq!(response["status"], "rejected");
    assert_eq!(response["data"]["saved"], true);
    assert!(response["data"]["error"]
        .as_str()
        .unwrap()
        .contains("was saved"));
}
