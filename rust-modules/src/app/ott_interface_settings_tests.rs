use super::*;
use plx_machine::machine::UiMode;

#[test]
fn interface_schema_accepts_both_modes_without_changing_native_default() {
    let field = Setting::Interface;
    let descriptor = field.descriptor();
    assert_eq!(descriptor["id"], "interfaceMode");
    assert_eq!(descriptor["default_value"], 1);
    assert_eq!(descriptor["access"]["write"], true);
    assert_eq!(descriptor["constraints"]["values"], json!([0, 1]));
    assert_eq!(
        descriptor["choices"],
        json!([
            {"value":0,"label":"Classic","aliases":["classic"]},
            {"value":1,"label":"Modern","aliases":["modern"]}
        ])
    );
    for value in [json!(0), json!(1)] {
        assert!(field.valid(&value));
    }
    for value in [
        json!(2),
        json!(-1),
        json!(0.0),
        json!(true),
        json!("classic"),
        Value::Null,
    ] {
        assert!(!field.valid(&value));
    }
}

#[test]
fn interface_storage_preserves_explicit_classic_and_reports_invalid_values() {
    let mut saved = Saved::default();
    assert_eq!(stored(&saved, Setting::Interface)["value"], 1);
    assert_eq!(stored(&saved, Setting::Interface)["storage"], "default");
    saved.settings.insert("interfaceMode".into(), json!(0));
    let classic = stored(&saved, Setting::Interface);
    assert_eq!(classic["value"], 0);
    assert_eq!(classic["persisted_value"], 0);
    assert_eq!(classic["storage"], "stored");
    saved
        .settings
        .insert("interfaceMode".into(), json!("invalid private value"));
    let invalid = stored(&saved, Setting::Interface);
    assert_eq!(invalid["storage"], "invalid");
    assert_eq!(invalid["value"], 1);
    assert!(invalid.get("persisted_value").is_none());
}

#[test]
fn local_mode_candidates_preserve_other_saved_data_and_invalidate_remote_aba() {
    let _serial = plx_base::testlock::serial();
    let mut runtime = Runtime::unloaded(crate::app::ott_companion::Supervisor::inert_for_test());
    runtime.remote = Remote::disabled_for_test("dev_fixture", "fixture");
    runtime.saved.category = "Favorites".into();
    runtime
        .saved
        .settings
        .insert("unrelatedSetting".into(), json!({"keep":true}));
    let before = runtime.saved.clone();
    let original_signature = signature(&before, Setting::Interface);
    let classic = runtime.interface_mode_candidate(UiMode::Classic);
    assert_eq!(
        runtime.interface_mode(),
        UiMode::Modern,
        "candidate is not published"
    );
    runtime.saved = classic.clone();
    let modern = runtime.interface_mode_candidate(UiMode::Modern);
    assert_ne!(signature(&classic, Setting::Interface), original_signature);
    assert_ne!(signature(&modern, Setting::Interface), original_signature);
    assert_ne!(
        signature(&classic, Setting::Interface),
        signature(&modern, Setting::Interface)
    );
    let mut rest = modern;
    rest.settings.remove("interfaceMode");
    rest.settings.remove("settingRevision:interfaceMode");
    assert_eq!(
        serde_json::to_value(rest).unwrap(),
        serde_json::to_value(before).unwrap()
    );
}


fn public_mode_snapshot(epoch: u64, revision: u64, mode: u8)
    -> plx_platform::storage::public_preferences::Snapshot
{
    use plx_platform::storage::public_preferences::{Preferences, Snapshot};
    Snapshot {
        value: Preferences { interface_mode: Some(mode), ..Preferences::default() },
        revision,
        epoch,
    }
}

#[test]
fn public_interface_reply_revision_matches_durable_ack_and_captured_context() {
    let _serial = plx_base::testlock::serial();
    let mut runtime = Runtime::unloaded(crate::app::ott_companion::Supervisor::inert_for_test());
    runtime.remote = Remote::disabled_for_test("dev_fixture", "fixture");
    let worker = plx_base::task::OffFrame::for_test();
    runtime.clock_sample = Some(plx_base::task::time::ClockSample::capture(&worker).unwrap());
    let now = runtime.now_ms();
    let context = setting_context(&runtime.saved, runtime.provider, runtime.remote.runtime(), &[1; 16]);
    let invocation = runtime.remote.request_for_test("player_settings", json!({
        "op":"set", "runtime":"fixture", "id":"interfaceMode", "value":0,
        "context":context["token"], "revision":"revision:original"
    }));
    let candidate = runtime.interface_mode_candidate(UiMode::Classic);
    let provisional = with_revision(&mut runtime.command_receipts, &candidate,
        Setting::Interface, &context, &invocation, true, now);
    let mut response = ok(json!({"context":context, "setting":provisional,
        "persisted":true, "application":{"state":"completed"}}));
    runtime.public_preferences.accept(Some(public_mode_snapshot(7, 11, 0)));
    crate::app::ott_runtime::public_preferences::overlay(
        &runtime.public_preferences, &mut runtime.saved);
    let durable_signature = signature(&runtime.saved, Setting::Interface);
    assert_ne!(durable_signature, signature(&candidate, Setting::Interface),
        "the durable CAS marker is not known when the reply is first prepared");

    public_saved(&mut runtime, &invocation, &mut response);
    assert_eq!(response["data"]["context"], context);
    assert_eq!(response["data"]["setting"]["value"], 0);
    assert_eq!(response["data"]["setting"]["persisted_value"], 0);
    assert_eq!(response["data"]["setting"]["effective_value"], 0);
    let returned = response["data"]["setting"]["revision"].as_str().unwrap().to_owned();
    let captured = context["token"].as_str().unwrap();
    assert!(runtime.command_receipts.consume(&returned, &durable_signature, captured, 0, now),
        "the successful reply must authorize the next write against the published state");
    assert!(!runtime.command_receipts.consume(&returned, &durable_signature, captured, 0, now),
        "the replacement receipt remains single-use");

    public_saved(&mut runtime, &invocation, &mut response);
    assert!(!runtime.command_receipts.consume(&returned, &durable_signature, "other-context", 0, now),
        "post-ACK remint must retain the captured source context");
}

#[test]
fn public_interface_cas_marker_rejects_aba_and_late_load_rollback() {
    let _serial = plx_base::testlock::serial();
    let mut runtime = Runtime::unloaded(crate::app::ott_companion::Supervisor::inert_for_test());
    runtime.remote = Remote::disabled_for_test("dev_fixture", "fixture");
    runtime.public_preferences.accept(Some(public_mode_snapshot(7, 1, 0)));
    crate::app::ott_runtime::public_preferences::overlay(
        &runtime.public_preferences, &mut runtime.saved);
    let first = signature(&runtime.saved, Setting::Interface);
    let context = setting_context(&runtime.saved, runtime.provider, runtime.remote.runtime(), &[1; 16]);
    let invocation = runtime.remote.request_for_test("player_settings", json!({
        "op":"get", "runtime":"fixture", "id":"interfaceMode"
    }));
    let initial = with_revision(&mut runtime.command_receipts, &runtime.saved,
        Setting::Interface, &context, &invocation, false, 0);
    for (revision, mode) in [(2, 1), (3, 0)] {
        runtime.public_preferences.accept(Some(public_mode_snapshot(7, revision, mode)));
        crate::app::ott_runtime::public_preferences::overlay(
            &runtime.public_preferences, &mut runtime.saved);
        assert_ne!(signature(&runtime.saved, Setting::Interface), first,
            "returning to the same value must not revive its old CAS authority");
    }
    assert_eq!(runtime.interface_mode(), UiMode::Classic);
    let current = signature(&runtime.saved, Setting::Interface);
    assert!(!runtime.command_receipts.consume(initial["revision"].as_str().unwrap(),
        &current, context["token"].as_str().unwrap(), 0, 0));
    for stale in [Some(public_mode_snapshot(7, 2, 1)), None] {
        runtime.public_preferences.accept(stale);
        crate::app::ott_runtime::public_preferences::overlay(
            &runtime.public_preferences, &mut runtime.saved);
        assert_eq!(signature(&runtime.saved, Setting::Interface), current,
            "a load begun before the acknowledged write cannot roll it back");
    }
    runtime.public_preferences.accept(Some(public_mode_snapshot(8, 1, 0)));
    crate::app::ott_runtime::public_preferences::overlay(
        &runtime.public_preferences, &mut runtime.saved);
    assert_ne!(signature(&runtime.saved, Setting::Interface), current,
        "a new storage epoch must not reuse a previous epoch's revision");
}
