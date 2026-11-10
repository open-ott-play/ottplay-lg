use super::*;
use plx_base::task::{time::ClockSample, OffFrame};
fn fixture() -> Runtime {
    let mut runtime = Runtime::unloaded(crate::app::ott_companion::Supervisor::inert_for_test());
    runtime.remote = Remote::disabled_for_test("dev_fixture", "native-fixture");
    runtime.loaded = true;
    runtime.clock_sample = Some(ClockSample::capture(&OffFrame::for_test()).unwrap());
    let index = insert_profile(
        &mut runtime.saved,
        ProviderKind::M3u,
        1,
        "https://example.invalid/list.m3u".into(),
        None,
    )
    .unwrap();
    runtime.saved.profiles.active_profile = Some(runtime.saved.profiles.profiles[index].id.clone());
    runtime.provider = ProviderKind::M3u;
    runtime
}
fn request(runtime: &Runtime, action: &str, params: Value) -> Invocation {
    let mut remote = Remote::disabled_for_test("dev_fixture", "native-fixture");
    let body=json!({"commands":[],"server_time":1000,"requests":[{"id":"0123456789abcdef0123456789abcdef",
        "action":action,"params":params,"expires_at":1600}]}).to_string();
    remote.raw_poll_for_test(&body, runtime.now_ms()).remove(0)
}
fn compare(
    runtime: &mut Runtime,
    provider: &str,
    slot: usize,
    patch: Value,
) -> (Invocation, Value) {
    let inv = request(
        runtime,
        "settings_compare",
        json!({"version":1,"runtime":"native-fixture","provider":provider,"number":slot,"settings":patch}),
    );
    let response = command_with(runtime, &inv, runtime.now_ms(), |_, _, _, _, _| {
        panic!("compare cannot save")
    })
    .unwrap();
    (inv, response)
}
fn set_request(runtime: &Runtime, compare: &Invocation, response: &Value) -> Invocation {
    let mut p = compare.params.clone();
    p["context"] = response["data"]["context"].clone();
    p["revision"] = response["data"]["revision"].clone();
    request(runtime, "provider_settings", p)
}
fn refuse_save(
    _: &mut Runtime,
    _: Saved,
    _: Invocation,
    _: bool,
    _: Value,
) -> Result<(), &'static str> {
    panic!("must not reach persistence")
}
#[test]
fn provider_config_v1_canonical_defaults_and_noop_do_not_save_or_reload() {
    let _lock = plx_base::testlock::serial();
    let mut runtime = fixture();
    let kind = ProviderKind::Operator(plx_data::ott::OperatorId::from_id("shura").unwrap());
    insert_profile(
        &mut runtime.saved,
        kind,
        4,
        json!({"username":"private-key","server":"1"}).to_string(),
        None,
    )
    .unwrap();
    let (inv, response) = compare(&mut runtime, "shura", 4, json!({"server":""}));
    assert_eq!(response["data"]["matched"], true);
    let set = set_request(&runtime, &inv, &response);
    let before = serde_json::to_value(&runtime.saved).unwrap();
    let result = command_with(&mut runtime, &set, inv.deadline_ms - 1, refuse_save).unwrap();
    assert_eq!(result["data"]["saved"], true);
    assert_eq!(result["data"]["reloaded"], false);
    assert_eq!(result["data"]["reload_required"], false);
    assert_eq!(result["data"]["context"], response["data"]["context"]);
    assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), before);
    assert_eq!(
        command_with(&mut runtime, &set, inv.deadline_ms - 1, refuse_save).unwrap()["data"]
            ["reason"],
        "revision_conflict"
    );
}
#[test]
fn provider_config_v1_patch_slot_runtime_and_epoch_changes_are_bound_one_use() {
    let _lock = plx_base::testlock::serial();
    let mut runtime = fixture();
    for scenario in ["patch", "slot", "runtime", "edit-aba", "navigation-aba"] {
        let (inv, response) = compare(&mut runtime, "edem", 3, json!({"key":"private_key"}));
        let mut set = set_request(&runtime, &inv, &response);
        if scenario == "patch" {
            set.params["settings"]["key"] = json!("changed_key");
        }
        if scenario == "slot" {
            set.params["number"] = json!(4);
        }
        if scenario == "runtime" {
            set.params["runtime"] = json!("other-runtime");
        }
        if scenario == "edit-aba" {
            runtime.profile_edit_epoch += 2;
        }
        if scenario == "navigation-aba" {
            runtime.navigation_epoch += 2;
        }
        // Re-admit exact received bytes for the altered request; no Value-only proof.
        set = request(&runtime, "provider_settings", set.params.clone());
        let result = command_with(&mut runtime, &set, inv.deadline_ms - 1, refuse_save).unwrap();
        assert_ne!(result["status"], "ok", "{scenario}");
        assert!(!result.to_string().contains("private_key"));
    }
}
#[test]
fn provider_config_v1_source_away_and_back_retires_receipt() {
    let _lock = plx_base::testlock::serial();
    let mut runtime = fixture();
    insert_profile(
        &mut runtime.saved,
        ProviderKind::Edem,
        3,
        json!({"key":"initial_key"}).to_string(),
        None,
    )
    .unwrap();
    let (inv, response) = compare(&mut runtime, "edem", 3, json!({"key":"next_key"}));
    let original = runtime.saved.clone();
    let mut changed = original.clone();
    let index = profile_index(&changed, ProviderKind::Edem, 3).unwrap();
    changed.profiles.profiles[index].source = json!({"key":"different_key"}).to_string();
    runtime.observe_profile_edits(&changed);
    runtime.saved = changed;
    runtime.observe_profile_edits(&original);
    runtime.saved = original;
    let set = set_request(&runtime, &inv, &response);
    assert_eq!(
        command_with(&mut runtime, &set, inv.deadline_ms - 1, refuse_save).unwrap()["data"]
            ["reason"],
        "source_changed"
    );
}
#[test]
fn provider_config_v1_inactive_write_waits_for_confirmed_persistence_and_preserves_active_source() {
    let _lock = plx_base::testlock::serial();
    let mut runtime = fixture();
    let active = runtime.saved.profiles.active_profile.clone();
    let selected = runtime.provider;
    let (inv, response) = compare(
        &mut runtime,
        "edem",
        15,
        json!({"key":"private_key","list":3}),
    );
    let set = set_request(&runtime, &inv, &response);
    let mut durable = None;
    let now = runtime.now_ms();
    assert!(
        command_with(&mut runtime, &set, now, |runtime, saved, _, reload, _| {
            assert!(!reload);
            runtime.saving = true;
            durable = Some(saved);
            Ok(())
        })
        .is_none()
    );
    assert!(runtime
        .saved
        .profiles
        .profiles
        .iter()
        .all(|p| p.kind != ProviderKind::Edem));
    assert!(before_saved(&mut runtime, Some(&set)));
    let saved = durable.take().unwrap();
    runtime.observe_profile_edits(&saved);
    runtime.saved = saved;
    runtime.saving = false;
    let result = settled(&mut runtime, &set, true, true).unwrap();
    assert_eq!(result["data"]["saved"], true);
    assert_eq!(result["data"]["number"], 15);
    assert_eq!(result["data"]["reloaded"], false);
    assert_eq!(result["data"]["reload_required"], false);
    assert_eq!(runtime.saved.profiles.active_profile, active);
    assert_eq!(runtime.provider, selected);
    assert_eq!(result["data"]["revision"], response["data"]["revision"]);
    assert!(!result.to_string().contains("private_key"));
    assert!(result["data"].to_string().len() <= 2048);
}
#[test]
fn provider_config_v1_expiry_storage_failure_and_late_source_change_never_claim_saved() {
    let _lock = plx_base::testlock::serial();
    let mut runtime = fixture();
    let (inv, response) = compare(&mut runtime, "edem", 2, json!({"key":"private_key"}));
    let set = set_request(&runtime, &inv, &response);
    let expired = runtime.now_ms() + RECEIPT_TTL;
    assert_eq!(
        command_with(&mut runtime, &set, expired, refuse_save).unwrap()["data"]["reason"],
        "expired"
    );
    let (inv, response) = compare(&mut runtime, "edem", 2, json!({"key":"private_key"}));
    let set = set_request(&runtime, &inv, &response);
    let now = runtime.now_ms();
    let result = command_with(&mut runtime, &set, now, |_, _, _, _, _| {
        Err("private-system-failure")
    })
    .unwrap();
    assert_eq!(result["data"]["reason"], "storage_failed");
    assert!(!result.to_string().contains("private-system"));
    let (inv, response) = compare(&mut runtime, "edem", 2, json!({"key":"private_key"}));
    let set = set_request(&runtime, &inv, &response);
    let now = runtime.now_ms();
    assert!(command_with(&mut runtime, &set, now, |_, _, _, _, _| Ok(())).is_none());
    runtime.navigation_epoch += 1;
    assert!(!before_saved(&mut runtime, Some(&set)));
    assert_eq!(
        settled(&mut runtime, &set, true, true).unwrap()["data"]["reason"],
        "source_changed"
    );
}
#[test]
fn provider_config_v1_unknown_outcome_is_not_false_noop_and_private_receipts_are_bounded() {
    let _lock = plx_base::testlock::serial();
    let mut runtime = fixture();
    for _ in 0..100 {
        let (_, result) = compare(&mut runtime, "edem", 1, json!({"key":"private_key"}));
        assert_eq!(result["status"], "ok");
    }
    assert_eq!(
        runtime.command_receipts.native_provider.rows.len(),
        RECEIPT_LIMIT
    );
    let (inv, response) = compare(&mut runtime, "edem", 1, json!({"key":"private_key"}));
    let set = set_request(&runtime, &inv, &response);
    let now = runtime.now_ms();
    assert!(command_with(&mut runtime, &set, now, |_, _, _, _, _| Ok(())).is_none());
    let result = uncertain(&mut runtime, &set).unwrap();
    assert_eq!(result["data"]["reason"], "storage_failed");
    assert!(result["data"].get("saved").is_none());
}
#[test]
fn provider_config_v1_value_only_and_unguarded_legacy_writes_refuse_and_fields_are_closed() {
    let _lock = plx_base::testlock::serial();
    let mut runtime = fixture();
    let (mut inv, _) = compare(&mut runtime, "edem", 1, json!({"key":"private_key"}));
    inv = runtime
        .remote
        .request_for_test("settings_compare", inv.params.clone());
    assert_eq!(
        command_with(&mut runtime, &inv, 100, refuse_save).unwrap()["data"]["reason"],
        "invalid_request"
    );
    for patch in [
        json!({"key":"bad key"}),
        json!({"key":"private_key","cdn":"evil"}),
        json!({"key":"private_key","list":1.0}),
        json!({"key":"private_key","portal":"bad\nlink"}),
    ] {
        let (_, response) = compare(&mut runtime, "edem", 1, patch);
        assert_eq!(response["data"]["reason"], "invalid_value");
    }
    let inv = request(
        &runtime,
        "provider_settings",
        json!({"provider":"edem","settings":{"key":"private_key"}}),
    );
    assert_eq!(
        command_with(&mut runtime, &inv, 100, refuse_save).unwrap()["data"]["reason"],
        "invalid_request"
    );
}
#[test]
fn provider_config_v1_capability_and_reply_shapes_match_shared_fixture() {
    let document: Value = serde_json::from_str(include_str!(
        "../../../contracts/native-provider-settings-v1.json"
    ))
    .unwrap();
    let cap = capability();
    let actual = cap["providers"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p.as_str().unwrap())
        .collect::<std::collections::BTreeSet<_>>();
    let expected = document["provider_ids"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p.as_str().unwrap())
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(actual, expected);
    assert_eq!(cap["operations"], json!(["compare", "set"]));
    let _lock = plx_base::testlock::serial();
    let mut runtime = fixture();
    let (inv, response) = compare(&mut runtime, "edem", 1, json!({"key":"private_key"}));
    assert_eq!(response["data"].as_object().unwrap().len(), 7);
    let state = &runtime.command_receipts.native_provider;
    let receipt = state.rows.last().unwrap();
    assert_ne!(response["data"]["context"], receipt.binding.source);
    assert_ne!(response["data"]["revision"], receipt.binding.source);
    let response = rejected_reason(&inv, Reason::StorageFailed);
    assert_eq!(response["data"].as_object().unwrap().len(), 5);
}
