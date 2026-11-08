//! Closed diagnostics through existing status ingress. Fixtures only populate
//! memory; status must not read storage, launch source work or expose its inputs.
use super::*;

const PRIVATE_ID: &str = "private-profile-id-canary";
const PRIVATE_NAME: &str = "private-profile-name-canary";
const PRIVATE_ALIAS: &str = "private-profile-alias-canary";
const PRIVATE_SOURCE: &str = "https://private-user:private-password@source.invalid/private-path?token=private-token";
const PRIVATE_FAILURE: &str = "private-load-error-canary";

fn private_profile(kind: ProviderKind) -> ProviderProfile {
    ProviderProfile {
        id: PRIVATE_ID.into(), name: PRIVATE_NAME.into(), kind,
        source: PRIVATE_SOURCE.into(), source_alias: PRIVATE_ALIAS.into(), enabled: true,
    }
}

pub(super) fn read_status(app: &mut App, action: &str, serial: u64) -> Value {
    let mut runtime = app.ott.take().unwrap();
    let saved = serde_json::to_value(&runtime.saved).unwrap();
    let source = (runtime.provider, runtime.generation, runtime.catalog_revision,
        runtime.navigation_epoch, runtime.saving, runtime.catalog_loading);
    assert!(runtime.resolver_next.is_none());
    let now = runtime.now_ms();
    let body = json!({"commands":[], "server_time":1000, "requests":[{
        "id":format!("{serial:032x}"), "action":action, "params":{}, "expires_at":1020
    }]}).to_string();
    let mut incoming = runtime.remote.raw_poll_for_test(&body, now);
    assert_eq!(incoming.len(), 1);
    let invocation = incoming.pop().unwrap();
    assert!(runtime.remote.invocation_is_current(&invocation));
    let response = commands::command(&mut runtime, app, &invocation)
        .expect("status returns synchronously without a worker");
    assert_eq!(response["status"], "ok");
    assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), saved);
    assert_eq!((runtime.provider, runtime.generation, runtime.catalog_revision,
        runtime.navigation_epoch, runtime.saving, runtime.catalog_loading), source);
    assert!(runtime.resolver_next.is_none());
    let text = response.to_string();
    for secret in [PRIVATE_ID, PRIVATE_NAME, PRIVATE_ALIAS, PRIVATE_SOURCE,
        "private-user", "private-password", "private-path", "private-token", PRIVATE_FAILURE] {
        assert!(!text.contains(secret), "status exposed private fixture input");
    }
    runtime.remote.complete(invocation, response.clone());
    app.ott = Some(runtime);
    response
}

#[test]
fn provider_context_distinguishes_saved_m3u_from_effective_plex() {
    let _serial = plx_base::testlock::serial();
    let mut app = Box::new(app_fixture());
    let runtime = app.ott.as_mut().unwrap();
    runtime.loaded = true;
    runtime.provider = ProviderKind::Plex;
    runtime.saved.settings.insert("selectedProvider".into(), json!("m3u"));
    runtime.saved.profiles.profiles = vec![private_profile(ProviderKind::M3u)];
    for (serial, action) in [(1, "state"), (2, "status")] {
        let response = read_status(&mut app, action, serial);
        assert_eq!(response["data"]["provider"], "plex");
        assert_eq!(response["data"]["provider_context"], json!({
            "loaded":true, "write_permit_valid":true, "state_load":"idle",
            "selected_provider":"m3u", "saved_active_profile_present":false,
            "saved_active_profile_kind":null, "runtime_provider":"plex",
            "effective_provider":"plex", "managed_active":false, "classic_active":false
        }));
    }
}

#[test]
fn provider_context_distinguishes_absent_from_unresolved_saved_profile() {
    let _serial = plx_base::testlock::serial();
    let mut app = Box::new(app_fixture());
    let mut expected = json!({
        "loaded":false, "write_permit_valid":true, "state_load":"idle",
        "selected_provider":null, "saved_active_profile_present":false,
        "saved_active_profile_kind":null, "runtime_provider":"m3u",
        "effective_provider":"m3u", "managed_active":false, "classic_active":false
    });
    assert_eq!(read_status(&mut app, "status", 1)["data"]["provider_context"], expected);
    app.ott.as_mut().unwrap().loaded = true;
    app.ott.as_mut().unwrap().saved.profiles.active_profile = Some(PRIVATE_ID.into());
    expected["loaded"] = json!(true);
    expected["saved_active_profile_present"] = json!(true);
    assert_eq!(read_status(&mut app, "status", 2)["data"]["provider_context"], expected);
}

#[test]
fn provider_context_resolves_saved_profile_kind_without_private_values() {
    let _serial = plx_base::testlock::serial();
    let mut app = Box::new(app_fixture());
    let runtime = app.ott.as_mut().unwrap();
    runtime.loaded = true;
    runtime.provider = ProviderKind::Plex;
    runtime.saved.settings.insert("selectedProvider".into(), json!("plex"));
    runtime.saved.profiles.active_profile = Some(PRIVATE_ID.into());
    runtime.saved.profiles.profiles = vec![private_profile(ProviderKind::M3u)];
    let response = read_status(&mut app, "status", 1);
    assert_eq!(response["data"]["provider"], "m3u");
    assert_eq!(response["data"]["provider_context"], json!({
        "loaded":true, "write_permit_valid":true, "state_load":"idle",
        "selected_provider":"plex", "saved_active_profile_present":true,
        "saved_active_profile_kind":"m3u", "runtime_provider":"plex",
        "effective_provider":"m3u", "managed_active":false, "classic_active":false
    }));
}

#[test]
fn provider_context_normalizes_only_known_provider_ids() {
    let _serial = plx_base::testlock::serial();
    let mut app = Box::new(app_fixture());
    let mut expected = json!({
        "loaded":false, "write_permit_valid":true, "state_load":"idle",
        "selected_provider":null, "saved_active_profile_present":false,
        "saved_active_profile_kind":null, "runtime_provider":"m3u",
        "effective_provider":"m3u", "managed_active":false, "classic_active":false
    });
    let cases = [
        (json!("m3u"), json!("m3u")), (json!("xtream"), json!("xtream")),
        (json!("stalker"), json!("stalker")), (json!("vportal"), json!("vportal")),
        (json!("local"), json!("offline")), (json!("offline"), json!("offline")),
        (json!("plex"), json!("plex")), (json!(PRIVATE_SOURCE), Value::Null),
        (json!("M3U"), Value::Null), (json!(" m3u"), Value::Null),
        (json!(""), Value::Null), (json!(false), Value::Null), (json!(1), Value::Null),
        (json!({"private":PRIVATE_SOURCE}), Value::Null),
        (json!([PRIVATE_SOURCE]), Value::Null), (Value::Null, Value::Null),
    ];
    for (index, (stored, selected)) in cases.into_iter().enumerate() {
        app.ott.as_mut().unwrap().saved.settings.insert("selectedProvider".into(), stored);
        expected["selected_provider"] = selected;
        assert_eq!(read_status(&mut app, "status", index as u64 + 1)["data"]["provider_context"], expected);
    }
}

#[test]
fn provider_context_reports_load_and_permit_without_error_details() {
    let _serial = plx_base::testlock::serial();
    let mut app = Box::new(app_fixture());
    let mut expected = json!({
        "loaded":false, "write_permit_valid":true, "state_load":"idle",
        "selected_provider":null, "saved_active_profile_present":false,
        "saved_active_profile_kind":null, "runtime_provider":"m3u",
        "effective_provider":"m3u", "managed_active":false, "classic_active":false
    });
    for (index, (load, wire)) in [(StateLoad::Idle, "idle"),
        (StateLoad::Loading, "loading"), (StateLoad::Failed(PRIVATE_FAILURE), "failed")]
        .into_iter().enumerate()
    {
        app.ott.as_mut().unwrap().state_load = load;
        expected["state_load"] = json!(wire);
        assert_eq!(read_status(&mut app, "status", index as u64 + 1)["data"]["provider_context"], expected);
    }
    app.ott.as_ref().unwrap().write_permit.revoke();
    expected["write_permit_valid"] = json!(false);
    assert_eq!(read_status(&mut app, "status", 4)["data"]["provider_context"], expected);
}
