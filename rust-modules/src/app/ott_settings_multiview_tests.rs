//! Settings candidate/receipt and portable-profile seams, with no storage worker,
//! transport, App, native playback or TV. The ordinary save adapter owns durability.
use super::*;

const DEVICE: &str = "dev_multiview_fixture";
const SECRET: &str = "fixture_compositor_bearer_0123456789abcdef";
fn binding(device: &str) -> Value {
    json!(json!({"server_id":"fixture-compositor","origin":"https://compositor.fixture.invalid",
        "audience":"ottplay-multiview-v1","installation_id":device,"control_bearer":SECRET}).to_string())
}
fn fixture() -> Runtime {
    let mut runtime = Runtime::unloaded(crate::app::ott_companion::Supervisor::inert_for_test());
    runtime.remote = Remote::disabled_for_test(DEVICE, "runtime:fixture");
    runtime.loaded = true;
    runtime.entropy = Some([7; 16]);
    runtime.write_permit.revoke();
    runtime.saved.settings.insert("unrelatedSetting".into(), json!({"keep":true}));
    runtime.saved.category = "Favorites".into();
    runtime
}
#[test]
fn multiview_service_is_appended_without_reordering_existing_settings() {
    assert_eq!(Setting::ALL.len(), 46);
    assert_eq!(Setting::ALL[43].id(), "nativeEpgApiV1");
    assert_eq!(Setting::ALL[44].id(), "nativeInputProbeV1");
    assert_eq!(Setting::ALL[45].id(), multiview::KEY);
    let descriptor = Setting::MultiviewService.descriptor();
    assert_eq!(descriptor["type"], "string");
    assert_eq!(descriptor["scope"], "application");
    assert_eq!(descriptor["sensitive"], true);
    assert_eq!(descriptor["access"], json!({"read":true,"write":true}));
    assert_eq!(descriptor["constraints"]["max_utf8_bytes"], 4096);
    assert_eq!(descriptor["apply"], "next_stream");
    assert!(descriptor.get("default_value").is_none());
}
#[test]
fn same_installation_candidate_preserves_every_other_setting_and_never_reloads() {
    let _lock = plx_base::testlock::serial();
    let runtime = fixture();
    let before = serde_json::to_value(&runtime.saved).unwrap();
    let epochs = (runtime.provider, runtime.generation, runtime.play_generation, runtime.playback_epoch);
    let cancel = (runtime.cancel.load(Ordering::Acquire), runtime.play_cancel.load(Ordering::Acquire));
    let value = binding(DEVICE);
    assert!(Setting::MultiviewService.valid(&value));
    let mut candidate = multiview::candidate(&runtime.saved, &value, runtime.remote.persisted_device_id()).unwrap();
    let stored = candidate.settings.remove(multiview::KEY).unwrap();
    assert_eq!(serde_json::from_str::<Value>(stored.as_str().unwrap()).unwrap(),
        serde_json::from_str::<Value>(value.as_str().unwrap()).unwrap());
    assert_eq!(serde_json::to_value(candidate).unwrap(), before);
    assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), before, "candidate is not publication");
    assert_eq!((runtime.provider, runtime.generation, runtime.play_generation, runtime.playback_epoch), epochs);
    assert_eq!((runtime.cancel.load(Ordering::Acquire), runtime.play_cancel.load(Ordering::Acquire)), cancel);
    assert!(!runtime.saving);
    assert!(runtime.rx.try_recv().is_err(), "pure candidate starts no worker or provider reload");
}
#[test]
fn syntax_alone_never_authorizes_other_or_unpersisted_installations() {
    let runtime = fixture();
    for (value, device) in [(binding("dev_other"), Some(DEVICE)), (binding(DEVICE), None)] {
        assert!(Setting::MultiviewService.valid(&value), "structural validity is not authority");
        let error = multiview::candidate(&runtime.saved, &value, device).err().unwrap();
        assert_eq!(error, "invalid_value");
        assert!(!error.contains(SECRET));
    }
    assert!(!runtime.saved.settings.contains_key(multiview::KEY));
}
#[test]
fn clear_is_explicitly_disabled_and_needs_no_pairing_authority() {
    let mut saved = Saved::default();
    saved.settings.insert(multiview::KEY.into(), binding(DEVICE));
    let cleared = multiview::candidate(&saved, &json!(""), None).unwrap();
    let snapshot = stored(&cleared, Setting::MultiviewService);
    assert_eq!(snapshot["storage"], "stored");
    assert_eq!(snapshot["configured"], false);
    assert_eq!(snapshot["redacted"], true);
    assert!(snapshot.get("value").is_none());
    assert!(cleared.settings.contains_key(multiview::KEY));
}
#[test]
fn metadata_and_revision_receipts_never_echo_private_binding_material() {
    let mut saved = Saved::default();
    assert_eq!(stored(&saved, Setting::MultiviewService), json!({
        "id":multiview::KEY,"redacted":true,"configured":false,"storage":"default"}));
    saved.settings.insert(multiview::KEY.into(), binding(DEVICE));
    let mut remote = Remote::disabled_for_test(DEVICE, "runtime:fixture");
    let invocation = remote.request_for_test("player_settings", json!({"op":"get",
        "runtime":"runtime:fixture","id":multiview::KEY}));
    let context = setting_context(&saved, ProviderKind::M3u, "runtime:fixture", &[7; 16]);
    let mut receipts = Receipts::default();
    let snapshot = with_revision(&mut receipts, &saved, Setting::MultiviewService, &context, &invocation, false, 1);
    assert_eq!(snapshot["configured"], true);
    assert_eq!(snapshot["storage"], "stored");
    assert!(snapshot["revision"].is_string());
    for key in ["value", "persisted_value", "effective_value"] { assert!(snapshot.get(key).is_none()); }
    let output = json!([snapshot, context, Setting::MultiviewService.descriptor(),
        failure(&invocation.params, "invalid_value")]).to_string();
    for private in [SECRET, DEVICE, "compositor.fixture.invalid", "fixture-compositor"] {
        assert!(!output.contains(private), "private material must never enter public receipts");
    }
}
#[test]
fn malformed_or_multiline_settings_are_rejected_without_echoing_configuration() {
    let mut object: Value = serde_json::from_str(binding(DEVICE).as_str().unwrap()).unwrap();
    object["extra"] = json!("not allowed");
    for value in [Value::Null, json!({}), json!(1), json!(true), json!(" "),
        json!("\n"), json!("x".repeat(4097)), json!("{}"), json!(object.to_string()),
        json!(format!("{}\n", binding(DEVICE).as_str().unwrap()))] {
        assert!(!Setting::MultiviewService.valid(&value));
        assert!(multiview::candidate(&Saved::default(), &value, Some(DEVICE)).is_err());
    }
    let mut invalid = Saved::default();
    invalid.settings.insert(multiview::KEY.into(), json!("private invalid config"));
    let snapshot = stored(&invalid, Setting::MultiviewService);
    assert_eq!(snapshot["storage"], "invalid");
    assert_eq!(snapshot["configured"], false);
    assert!(!snapshot.to_string().contains("private invalid config"));
}
#[test]
fn changed_private_config_invalidates_old_cas_without_public_value() {
    let first = multiview::candidate(&Saved::default(), &binding(DEVICE), Some(DEVICE)).unwrap();
    let clear = multiview::candidate(&first, &json!(""), Some(DEVICE)).unwrap();
    assert_ne!(signature(&first, Setting::MultiviewService), signature(&clear, Setting::MultiviewService));
    assert_ne!(signature(&Saved::default(), Setting::MultiviewService), signature(&clear, Setting::MultiviewService));
}
#[test]
fn local_portable_export_excludes_binding_and_import_refuses_injected_setting() {
    let mut runtime = fixture();
    runtime.provider = ProviderKind::Local;
    runtime.saved.settings.insert(multiview::KEY.into(), binding(DEVICE));
    let profile = json!({"version":1,"provider":"offline","name":"Films",
        "binding_id":"01234567-89ab-4def-8123-456789abcdef",
        "sources":[{"key":"s0","kind":"folder","name":"Films"}],"selected":null});
    runtime.saved.settings.insert("offlineProfile:1".into(), profile.clone());
    let export = runtime.remote.request_for_test("profile_export", json!({"number":1}));
    let response = super::super::offline::request(&mut runtime, &export).unwrap();
    assert_eq!(response["data"]["profile"], profile);
    let public = response.to_string();
    for private in [multiview::KEY, SECRET, DEVICE, "compositor.fixture.invalid"] { assert!(!public.contains(private)); }
    let mut injected = profile;
    injected[multiview::KEY] = binding(DEVICE);
    let import = Remote::disabled_for_test(DEVICE, "runtime:fixture").request_for_test("profile_import", json!({"number":2,"profile":injected}));
    let before = serde_json::to_value(&runtime.saved).unwrap();
    let response = super::super::offline::request(&mut runtime, &import).unwrap();
    assert_eq!(response["status"], "rejected");
    assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), before);
    assert!(!response.to_string().contains(SECRET));
}
