//! Existing command admission with an inert runtime; no writer or transport starts.
use super::*;

const LINK: &str = "portal::[key:fixture-secret]https://portal.fixture.invalid/api/";

fn fixture(kind: ProviderKind, name: &str) -> Runtime {
    let mut runtime = Runtime::unloaded(crate::app::ott_companion::Supervisor::inert_for_test());
    let worker = plx_base::task::OffFrame::for_test();
    runtime.clock_sample = Some(plx_base::task::time::ClockSample::capture(&worker).unwrap());
    runtime.remote = Remote::disabled_for_test("dev_fixture", "fixture-runtime");
    runtime.write_permit.revoke();
    runtime.provider = kind;
    let source = if kind == ProviderKind::VPortal { LINK } else { "https://tv.fixture.invalid/list.m3u" };
    let index = insert_profile(&mut runtime.saved, kind, 7, source.into(), Some(name.into())).unwrap();
    runtime.saved.profiles.active_profile = Some(runtime.saved.profiles.profiles[index].id.clone());
    runtime.saved.profiles.profiles[index].validate().unwrap();
    runtime
}
fn request(runtime: &mut Runtime, serial: u32, action: &str, params: Value) -> Invocation {
    let now = runtime.now_ms();
    let body = json!({"commands":[],"server_time":1000,"requests":[{
        "id":format!("{serial:032x}"),"action":action,"params":params,"expires_at":1020
    }]}).to_string();
    let mut received = runtime.remote.raw_poll_for_test(&body, now);
    assert_eq!(received.len(), 1);
    received.pop().unwrap()
}
fn portal_compare(runtime: &mut Runtime, serial: u32, name: Value) -> Value {
    let params = json!({"provider":"vportal","runtime":runtime.remote.runtime(),"number":7,
        "settings":{"name":name,"vportal":LINK}});
    let inv = request(runtime, serial, "settings_compare", params);
    provider_settings(runtime, &inv).unwrap()
}
fn assert_no_save(runtime: &Runtime, before: &Value) {
    assert_eq!(&serde_json::to_value(&runtime.saved).unwrap(), before);
    assert!(!runtime.saving);
    assert!(runtime.catalog_next.is_none());
    assert!(runtime.resolver_next.is_none());
}

#[test]
fn vportal_compare_accepts_512_byte_saved_names_without_mutation_or_echo() {
    for (name, mismatch) in [("x".repeat(512), "y".repeat(512)), ("é".repeat(256), "è".repeat(256))] {
        assert_eq!(name.len(), 512);
        let mut runtime = fixture(ProviderKind::VPortal, &name);
        let before = serde_json::to_value(&runtime.saved).unwrap();
        for (serial, expected, matched) in [(1, name.clone(), true), (2, mismatch, false)] {
            let response = portal_compare(&mut runtime, serial, json!(expected));
            assert_eq!(response, ok(json!({"provider":"vportal","number":7,"matched":matched,
                "runtime":runtime.remote.runtime(),"context":context(&runtime),
                "revision":format!("revision:{serial:032x}")})));
            assert!(!response.to_string().contains(&name));
            assert!(!response.to_string().contains(LINK));
            assert_no_save(&runtime, &before);
            assert_eq!(runtime.provider, ProviderKind::VPortal);
        }
        // Compare still issues its established ephemeral receipt; it does not mutate Saved.
        assert_eq!(runtime.command_receipts.rows.len(), 2);
    }
}

#[test]
fn vportal_compare_rejects_oversize_and_control_names_without_mutation() {
    let mut runtime = fixture(ProviderKind::VPortal, "Existing");
    let before = serde_json::to_value(&runtime.saved).unwrap();
    let invalid = [json!("x".repeat(513)), json!("é".repeat(257)),
        json!("bad\u{0000}name"), json!("bad\u{007f}name"), json!("bad\u{0085}name"), json!(false)];
    for (at, name) in invalid.into_iter().enumerate() {
        let response = portal_compare(&mut runtime, at as u32 + 1, name);
        assert_eq!(response, rejected("Unsupported VPortal settings fields"));
        assert_no_save(&runtime, &before);
        assert!(runtime.command_receipts.rows.is_empty());
    }
}

#[test]
fn vportal_setter_keeps_256_byte_limit_and_does_not_consume_receipt_for_257() {
    let mut runtime = fixture(ProviderKind::VPortal, "Existing");
    let before = serde_json::to_value(&runtime.saved).unwrap();
    let compared = portal_compare(&mut runtime, 1, json!("Existing"));
    assert_eq!(compared["data"]["matched"], true);
    let params = |name: String| json!({"provider":"vportal","runtime":compared["data"]["runtime"],
        "context":compared["data"]["context"],"revision":compared["data"]["revision"],
        "settings":{"profile":7,"name":name,"vportal":LINK}});
    let too_long = request(&mut runtime, 2, "provider_settings", params("x".repeat(257)));
    assert_eq!(provider_settings(&mut runtime, &too_long), Some(rejected("Unsupported VPortal settings fields")));
    assert_eq!(runtime.command_receipts.rows.len(), 1, "field refusal must not consume the compare receipt");
    assert_no_save(&runtime, &before);
    let valid = request(&mut runtime, 3, "provider_settings", params("x".repeat(256)));
    // The genuine receipt and 256-byte name pass field admission. Unloaded state
    // deliberately stops before any write; this is not a successful persistence claim.
    let refusal = runtime.settings_write_refusal().unwrap();
    assert_eq!(provider_settings(&mut runtime, &valid), Some(rejected(refusal)));
    assert!(!runtime.command_receipts.rows.iter().any(|row|
        Some(row.token.as_str()) == compared["data"]["revision"].as_str()));
    // The existing write path prepares its next ephemeral receipt before defer.
    assert_eq!(runtime.command_receipts.rows.len(), 1);
    assert_eq!(runtime.command_receipts.rows[0].token, format!("revision:{:032x}", 3));
    assert_no_save(&runtime, &before);
}

#[test]
fn m3u_compare_already_accepts_512_byte_saved_names_without_mutation_or_echo() {
    for (name, mismatch) in [("x".repeat(512), "y".repeat(512)), ("é".repeat(256), "è".repeat(256))] {
        let mut runtime = fixture(ProviderKind::M3u, &name);
        let before = serde_json::to_value(&runtime.saved).unwrap();
        for (serial, expected, matched) in [(1, name.clone(), true), (2, mismatch, false)] {
            let params = json!({"provider":"m3u","number":7,"settings":{
                "name":expected,"playlist":"https://tv.fixture.invalid/list.m3u",
                "history_hours":metadata(&runtime.saved, ProviderKind::M3u, 7)["history_hours"]}});
            let inv = request(&mut runtime, serial, "settings_compare", params);
            let response = provider_settings(&mut runtime, &inv).unwrap();
            assert_eq!(response, ok(json!({"provider":"m3u","number":7,"matched":matched})));
            assert!(!response.to_string().contains(&name));
            assert_no_save(&runtime, &before);
            assert_eq!(runtime.provider, ProviderKind::M3u);
        }
    }
}

#[test]
fn explicit_empty_source_retains_slot_metadata_and_compare_identity() {
    for kind in [ProviderKind::M3u, ProviderKind::VPortal] {
        let mut runtime = fixture(kind, "Keep name");
        let before = runtime.saved.clone();
        let id = before.profiles.active_profile.clone().unwrap();
        runtime.saved.settings.insert(format!("history:{id}"), json!(72));
        let index = insert_profile(&mut runtime.saved, kind, 7, String::new(), None).unwrap();
        assert_eq!(runtime.saved.profiles.profiles[index].id, id);
        assert_eq!(runtime.saved.profiles.profiles[index].name, "Keep name");
        assert_eq!(runtime.saved.profiles.active_profile, before.profiles.active_profile);
        assert_eq!(runtime.saved.settings[&format!("history:{id}")], 72);
        assert!(runtime.saved.profiles.validate().is_ok());
        assert_eq!(metadata(&runtime.saved, kind, 7)["active"], true);
        let key = if kind == ProviderKind::VPortal { "vportal" } else { "playlist" };
        let mut settings = json!({"name":"Keep name"});
        settings[key] = json!("");
        if kind == ProviderKind::M3u { settings["history_hours"] = json!(72); }
        let mut params = json!({"provider":wire_provider(kind),"number":7,"settings":settings});
        if kind == ProviderKind::VPortal { params["runtime"] = json!(runtime.remote.runtime()); }
        let inv = request(&mut runtime, 1, "settings_compare", params);
        let response = provider_settings(&mut runtime, &inv).unwrap();
        assert_eq!(response["data"]["matched"], true);
        let configured = if kind == ProviderKind::VPortal { "vportal_configured" } else { "playlist_configured" };
        assert_eq!(metadata(&runtime.saved, kind, 7)[configured], false);
        assert!(!runtime.saving);
        let source = if kind == ProviderKind::VPortal { LINK } else { "https://tv.fixture.invalid/refill.m3u" };
        assert_eq!(insert_profile(&mut runtime.saved, kind, 7, source.into(), None).unwrap(), index);
        assert!(runtime.saved.profiles.profiles[index].validate().is_ok());
        assert_eq!(runtime.saved.profiles.profiles[index].name, "Keep name");
    }
}

#[test]
fn explicit_empty_source_reaches_existing_writer_but_invalid_values_do_not() {
    for kind in [ProviderKind::M3u, ProviderKind::VPortal] {
        let mut runtime = fixture(kind, "Existing");
        let before = serde_json::to_value(&runtime.saved).unwrap();
        let key = if kind == ProviderKind::VPortal { "vportal" } else { "playlist" };
        let mut settings = json!({});
        settings[key] = json!("");
        let inv = request(&mut runtime, 1, "profile_settings", json!({"number":7,"settings":settings}));
        let refusal = runtime.settings_write_refusal().unwrap();
        assert_eq!(profiles(&mut runtime, &inv), Some(rejected(refusal)));
        assert_no_save(&runtime, &before);
        for (at, value) in [json!(" "), json!("invalid"), json!("file:///tmp/source"),
            json!("https://fixture.invalid/\n"), json!(false), json!(null)].into_iter().enumerate() {
            let mut settings = json!({});
            settings[key] = value;
            let inv = request(&mut runtime, at as u32 + 2, "profile_settings", json!({"number":7,"settings":settings}));
            let response = profiles(&mut runtime, &inv).unwrap();
            assert_eq!(response["status"], "rejected");
            assert_ne!(response, rejected(refusal), "invalid source must fail before the write gate");
            assert_no_save(&runtime, &before);
        }
    }
}

#[test]
fn dedicated_vportal_empty_source_uses_bound_receipt_and_keeps_invalid_sources_rejected() {
    for (at, source) in ["", " ", "invalid", "file:///tmp/portal", "https://fixture.invalid/\n"].into_iter().enumerate() {
        let mut runtime = fixture(ProviderKind::VPortal, "Existing");
        let before = serde_json::to_value(&runtime.saved).unwrap();
        let params = json!({"provider":"vportal", "runtime":runtime.remote.runtime(),
            "number":7,"settings":{"vportal":source}});
        let compare = request(&mut runtime, 1, "settings_compare", params);
        let compared = provider_settings(&mut runtime, &compare).unwrap();
        if compared["status"] != "ok" {
            assert_ne!(at, 0, "empty compare must be supported");
            assert_no_save(&runtime, &before);
            continue;
        }
        let params = json!({"provider":"vportal", "runtime":runtime.remote.runtime(),
            "context":compared["data"]["context"],"revision":compared["data"]["revision"],
            "settings":{"profile":7,"vportal":source}});
        let inv = request(&mut runtime, 2, "provider_settings", params);
        let refusal = runtime.settings_write_refusal().unwrap();
        let response = provider_settings(&mut runtime, &inv).unwrap();
        assert_eq!(response["status"], "rejected");
        if source.is_empty() { assert_eq!(response, rejected(refusal)); }
        else { assert_ne!(response, rejected(refusal)); }
        assert_no_save(&runtime, &before);
    }
}
