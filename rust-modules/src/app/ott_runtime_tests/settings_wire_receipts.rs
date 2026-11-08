//! Actual dispatch, writer admission and delivery receipts. The writer has a
//! revoked permit; Saved(Ok) below models completion, not protected storage IO.
use super::*;

const LINK: &str = "portal::[key:old-private-key]https://portal.fixture.invalid/api/";
const NEW_LINK: &str = "portal::[key:new-private-key]https://other.fixture.invalid/api/";

fn setup() -> App {
    let mut app = app_fixture();
    let runtime = app.ott.as_mut().unwrap();
    runtime.loaded = true;
    runtime.entropy = Some([1; 16]);
    runtime.write_permit.revoke();
    runtime.catalog_done.store(false, Ordering::Release);
    runtime.metadata_done.store(false, Ordering::Release);
    runtime.resolver_busy = true;
    runtime.plex_refresh_pending = true;
    runtime.saved = Saved::default();
    runtime.saved.profiles.profiles.push(ProviderProfile {
        id: "wire-parent".into(), name: "Home".into(), kind: ProviderKind::M3u,
        source: "https://tv.fixture.invalid/private.m3u".into(),
        source_alias: String::new(), enabled: true,
    });
    runtime.saved.profiles.active_profile = Some("wire-parent".into());
    runtime.saved.settings.insert("selectedProvider".into(), json!("m3u"));
    runtime.saved.settings.insert("slot:wire-parent".into(), json!(7));
    runtime.saved = runtime.attachment_candidate(
        &runtime.saved.profiles.profiles[0], LINK, Some("wire-attachment"),
    ).unwrap();
    runtime.sync_saved_context();
    runtime.continuity.stop(runtime.generation);
    app
}

fn document(id: u64, action: &str, params: Value) -> String {
    json!({"commands":[],"server_time":1000,"requests":[{
        "id":format!("{id:032x}"),"action":action,"params":params,"expires_at":1020
    }]}).to_string()
}

fn invocation(app: &mut App, id: u64, action: &str, params: Value) -> Invocation {
    let runtime = app.ott.as_mut().unwrap();
    let now = runtime.now_ms();
    let mut requests = runtime.remote.raw_poll_for_test(&document(id, action, params), now);
    assert_eq!(requests.len(), 1);
    requests.pop().unwrap()
}

fn dispatch(app: &mut App, inv: &Invocation) -> Option<Value> {
    let mut runtime = app.ott.take().unwrap();
    let result = commands::command(&mut runtime, app, inv);
    app.ott = Some(runtime);
    result
}

fn get(app: &mut App, id: u64) -> Value {
    let token = app.ott.as_ref().unwrap().remote.runtime().to_owned();
    let inv = invocation(app, id, "player_settings",
        json!({"op":"get","runtime":token,"id":"m3uMediaUrl"}));
    let result = dispatch(app, &inv).unwrap();
    assert_eq!(result["status"], "ok");
    result["data"].clone()
}

fn set_params(get: &Value, value: &str) -> Value {
    json!({"op":"set","runtime":get["runtime"],"id":"m3uMediaUrl",
        "context":get["context"]["token"],"revision":get["setting"]["revision"],"value":value})
}

fn pin() -> Value {
    let origin = json!({"provider":"m3u","profile":"Home","subprofile":null,"sourceAlias":"Local"});
    json!({"version":1,"strict":true,"origin":origin,"target":{"kind":"item","entry":{
        "title":"Fixture channel","origin":origin,"locator":{"kind":"channel","channelId":"fixture-1"}
    }}})
}

fn rejected(result: &Value, params: &Value, reason: &str) {
    assert_eq!(result["status"], "rejected");
    assert_eq!(result["data"]["version"], 1);
    assert_eq!(result["data"]["op"], params["op"]);
    assert_eq!(result["data"]["runtime"], params["runtime"]);
    assert_eq!(result["data"]["reason"], reason);
    assert_eq!(result["data"].as_object().unwrap().len(), 5);
    assert!(result["data"]["error"].as_str().is_some_and(|s| !s.is_empty() && s.len() <= 256));
    for secret in [LINK, NEW_LINK, "private-key", "fixture.invalid"] {
        assert!(!result.to_string().contains(secret));
    }
}

fn outer(app: &mut App, id: u64, params: Value) -> Value {
    app.ott.as_mut().unwrap().remote.queue_poll_for_test(&document(id, "player_settings", params));
    runtime_tick(app);
    app.ott.as_ref().unwrap().remote.response_for_id_for_test(&format!("{id:032x}"))
        .expect("real outer dispatcher delivery")
}

#[test]
fn strict_policy_returns_typed_settings_refusal_at_both_dispatchers() {
    let _serial = plx_base::testlock::serial();
    let mut app = setup();
    let params = set_params(&get(&mut app, 1), NEW_LINK);
    app.ott.as_mut().unwrap().saved.settings.insert("nativeKiosk".into(), pin());
    let before = serde_json::to_value(&app.ott.as_ref().unwrap().saved).unwrap();
    let inv = invocation(&mut app, 2, "player_settings", params.clone());
    rejected(&dispatch(&mut app, &inv).unwrap(), &params, "policy_restricted");
    rejected(&outer(&mut app, 3, params.clone()), &params, "policy_restricted");
    let runtime = app.ott.as_ref().unwrap();
    assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), before);
    assert!(!runtime.saving);
    assert!(runtime.rx.try_recv().is_err());
    assert_eq!(get(&mut app, 4)["setting"]["configured"], true);
}

#[test]
fn pending_kiosk_returns_typed_settings_refusal_without_a_second_write() {
    let _serial = plx_base::testlock::serial();
    let mut app = setup();
    let params = set_params(&get(&mut app, 1), NEW_LINK);
    app.ott.as_mut().unwrap().saved.settings.insert("nativeKiosk".into(), pin());
    let inv = invocation(&mut app, 2, "kiosk", json!({"mode":"off"}));
    assert!(dispatch(&mut app, &inv).is_none());
    let _refused = capture_write(&app); // Keep the admitted transition pending.
    assert!(commands::kiosk_pending(app.ott.as_ref().unwrap()));
    let before = serde_json::to_value(&app.ott.as_ref().unwrap().saved).unwrap();
    let inv = invocation(&mut app, 3, "player_settings", params.clone());
    rejected(&dispatch(&mut app, &inv).unwrap(), &params, "policy_restricted");
    rejected(&outer(&mut app, 4, params.clone()), &params, "policy_restricted");
    let runtime = app.ott.as_ref().unwrap();
    assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), before);
    assert!(commands::kiosk_pending(runtime));
    assert!(runtime.rx.try_recv().is_err());
}

#[test]
fn expired_settings_request_is_typed_and_does_not_admit_a_write() {
    let _serial = plx_base::testlock::serial();
    let mut app = setup();
    let params = set_params(&get(&mut app, 1), NEW_LINK);
    let mut inv = invocation(&mut app, 2, "player_settings", params.clone());
    inv.deadline_ms = app.ott.as_ref().unwrap().now_ms();
    rejected(&dispatch(&mut app, &inv).unwrap(), &params, "source_changed");
    assert!(!app.ott.as_ref().unwrap().saving);
}

fn capture_write(app: &App) -> Job {
    let job = app.ott.as_ref().unwrap().rx.recv_timeout(Duration::from_secs(5)).expect("revoked writer completion");
    assert!(matches!(&job, Job::Saved(Err("Could not prepare protected OTT settings for saving"), Some(_), false, _)));
    job
}

fn stage(app: &mut App, value: &str) -> (Value, Invocation, Saved, Job) {
    let before = get(app, 1);
    let inv = invocation(app, 2, "player_settings", set_params(&before, value));
    let runtime = app.ott.as_ref().unwrap();
    let mut candidate = runtime.attachment_candidate(&runtime.saved.profiles.profiles[0], value, None).unwrap();
    // Model only the existing command's revision stamp, never a successful disk write.
    let stamp: String = plx_base::sha256::sha256(json!(inv.id).to_string().as_bytes())
        .iter().map(|b| format!("{b:02x}")).collect();
    candidate.settings.insert("settingRevision:m3uMediaUrl".into(), json!(stamp));
    assert!(dispatch(app, &inv).is_none());
    let job = capture_write(app);
    assert!(app.ott.as_ref().unwrap().remote.response_for_id_for_test(&inv.id).is_none());
    (before, inv, candidate, job)
}

fn deliver(app: &mut App, inv: &Invocation, job: Job) -> Value {
    app.ott.as_ref().unwrap().tx.send(job).unwrap_or_else(|_| panic!("fixture receiver disappeared"));
    runtime_tick(app);
    app.ott.as_ref().unwrap().remote.response_for_id_for_test(&inv.id).expect("addressed settings receipt")
}

fn successful_change(value: &str, configured: bool) {
    let mut app = setup();
    let (before, inv, candidate, job) = stage(&mut app, value);
    let Job::Saved(_, pending, reload, reply) = job else { unreachable!() };
    assert_eq!(reply["data"]["context"], before["context"]);
    assert_eq!(reply["data"]["setting"]["storage"], "stored");
    assert_eq!(reply["data"]["setting"]["configured"], configured);
    assert_eq!(reply["data"]["setting"]["redacted"], true);
    assert!(reply["data"]["setting"].get("value").is_none());
    assert!(reply["data"]["setting"].get("persisted_value").is_none());
    let result = deliver(&mut app, &inv, Job::Saved(Ok(candidate), pending, reload, reply));
    assert_eq!(result["status"], "ok");
    assert_eq!(result["data"]["context"], before["context"]);
    assert_eq!(result["data"]["persisted"], true);
    assert_eq!(result["data"]["setting"]["configured"], configured);
    let fresh = get(&mut app, 3);
    assert_ne!(fresh["context"]["token"], before["context"]["token"]);
    assert_eq!(fresh["setting"]["storage"], "stored");
    assert_eq!(fresh["setting"]["configured"], configured);
    let stale = invocation(&mut app, 4, "player_settings", set_params(&before, LINK));
    rejected(&dispatch(&mut app, &stale).unwrap(), &stale.params, "source_changed");
    assert!(!app.ott.as_ref().unwrap().saving);
    for secret in [LINK, NEW_LINK, "private-key", "fixture.invalid"] {
        assert!(!result.to_string().contains(secret));
        assert!(!fresh.to_string().contains(secret));
    }
}

#[test]
fn attachment_set_ack_keeps_request_context_and_fresh_get_advances_authority() {
    let _serial = plx_base::testlock::serial();
    successful_change(NEW_LINK, true);
}

#[test]
fn attachment_clear_ack_records_durable_empty_value_and_keeps_request_context() {
    let _serial = plx_base::testlock::serial();
    successful_change("", false);
}

#[test]
fn refused_settings_writer_delivers_typed_failure_without_saved_success() {
    let _serial = plx_base::testlock::serial();
    let mut app = setup();
    let saved = serde_json::to_value(&app.ott.as_ref().unwrap().saved).unwrap();
    let (_, inv, _, job) = stage(&mut app, NEW_LINK);
    let result = deliver(&mut app, &inv, job);
    rejected(&result, &inv.params, "storage_failed");
    assert_eq!(serde_json::to_value(&app.ott.as_ref().unwrap().saved).unwrap(), saved);
    assert!(!app.ott.as_ref().unwrap().saving);
}

#[test]
fn uncertain_settings_writer_delivers_typed_failure_and_retains_reconciliation_gate() {
    let _serial = plx_base::testlock::serial();
    let mut app = setup();
    let (_, inv, _, _refused) = stage(&mut app, NEW_LINK);
    let result = deliver(&mut app, &inv, Job::Uncertain(None, Some(inv.clone())));
    rejected(&result, &inv.params, "storage_failed");
    assert!(app.ott.as_ref().unwrap().persistence_uncertain);
    assert!(!app.ott.as_ref().unwrap().loaded);
    assert!(!app.ott.as_ref().unwrap().saving);
}

#[test]
fn malformed_attachment_registry_is_a_typed_refusal_not_a_default_value() {
    let _serial = plx_base::testlock::serial();
    let mut app = setup();
    let runtime = app.ott.as_mut().unwrap();
    runtime.saved.settings.insert(crate::app::ott_m3u_attachment::SETTINGS_KEY.into(), json!({"version":99,"entries":[]}));
    runtime.observe_m3u_attachments();
    let token = runtime.remote.runtime().to_owned();
    let params = json!({"op":"get","runtime":token,"id":"m3uMediaUrl"});
    let inv = invocation(&mut app, 1, "player_settings", params.clone());
    rejected(&dispatch(&mut app, &inv).unwrap(), &params, "source_changed");
    assert!(!app.ott.as_ref().unwrap().saving);
}
