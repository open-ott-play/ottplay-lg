//! The actual verified-source completion admits the actual writer, whose revoked
//! permit refuses before storage IO. We retain that writer's addressed response
//! and inject Saved(Ok) only as a completion-model boundary, not durable IO proof.
//! No verifier HTTP request, protected-state write or native playback is started.
use super::*;
use crate::app::ott_import;
use plx_plex::plex::session::SourceRef;

const SERVER: &str = "http://127.0.0.1:32401";
const NEW_SERVER: &str = "http://127.0.0.1:32402";
const TOKEN: &str = "field-ack-original-token";
const NEW_TOKEN: &str = "field-ack-replacement-token";

struct Reset;
impl Drop for Reset {
    fn drop(&mut self) {
        let _ = ott_import::set_authorized_sources(&[]);
        plx_plex::plex::reset_servers_for_test();
        plx_plex::plex::grant::reset_for_test();
    }
}

fn source(settings: &Value) -> SourceRef {
    let address = settings["server"].as_str().unwrap_or(SERVER);
    let origin = plx_plex::plex::Origin::parse(address).unwrap();
    SourceRef {
        machine_id: "field-ack-fixture".into(),
        name: "Field acknowledgement fixture".into(),
        address: origin.host().to_owned(),
        port: i64::from(origin.port()),
        origin_url: origin.base(),
        token: settings["token"].as_str().unwrap_or(TOKEN).into(),
        ..Default::default()
    }
}

fn setup() -> (Reset, App) {
    plx_plex::plex::reset_servers_for_test();
    plx_plex::plex::grant::reset_for_test();
    ott_import::set_authorized_sources(&[]).unwrap();
    let mut app = mounted();
    let runtime = app.ott.as_mut().unwrap();
    let original = source(&json!({}));
    runtime.saved = Saved::default();
    runtime.saved.profiles.profiles = vec![ProviderProfile {
        id: "field-ack".into(),
        name: original.name.clone(),
        kind: ProviderKind::Plex,
        source: original.machine_id.clone(),
        source_alias: String::new(),
        enabled: true,
    }];
    runtime.saved.profiles.active_profile = Some("field-ack".into());
    runtime
        .saved
        .settings
        .insert("selectedProvider".into(), json!("plex"));
    runtime
        .saved
        .settings
        .insert("nativePlexSources".into(), json!([original]));
    runtime.saved.validate().unwrap();
    assert!(!runtime.plex_refresh_pending, "no restoration worker was admitted");
    // This fixture models verifier/save completion, not boot/source restoration.
    // The unloaded tick can queue restoration; do not let it race the writer on
    // the redirected completion channel after this synthetic state is loaded.
    runtime.plex_refresh_requested = false;
    runtime.loaded = true;
    runtime.entropy = Some([1; 16]);
    runtime.sync_saved_context();
    (Reset, app)
}

fn invocation(app: &mut App, settings: Value) -> Invocation {
    let runtime = app.ott.as_mut().unwrap();
    let now = runtime.now_ms();
    let raw = json!({"commands":[],"server_time":1000,"requests":[{
        "id":"0123456789abcdef0123456789abcdef","action":"provider_settings",
        "params":{"provider":"plex","settings":settings},"expires_at":1020
    }]})
    .to_string();
    let mut requests = runtime.remote.raw_poll_for_test(&raw, now);
    assert_eq!(requests.len(), 1);
    let request = requests.pop().unwrap();
    assert!(runtime.remote.invocation_is_current(&request));
    assert_eq!(request.deadline_ms, now.saturating_add(20_000));
    request
}

fn admit_verifier_context(app: &mut App) {
    let runtime = app.ott.as_mut().unwrap();
    // Same real activating-save metadata seam as the existing navigation test.
    // This first refused writer is drained before injecting the verifier event.
    runtime.save(runtime.saved.clone(), None, true).unwrap();
    let _ = capture_refused_write(app);
}

// Never format Job or its payload: a completion may contain private source
// settings. These closed labels distinguish expiry from the expected refusal.
fn write_completion_diagnostic(
    job: &Job,
    request: &Invocation,
) -> (&'static str, &'static str, Option<bool>, Option<bool>) {
    let same_invocation = |invocation: &Option<Invocation>| {
        invocation.as_ref().map(|inv| inv.id == request.id)
    };
    match job {
        Job::Saved(result, invocation, reload, _) => (
            "saved",
            match result {
                Ok(_) => "ok",
                Err("Could not prepare protected OTT settings for saving") => "prepare_refused",
                Err("Settings request expired") => "expired",
                Err(_) => "other_error",
            },
            same_invocation(invocation),
            Some(*reload),
        ),
        Job::SaveRejected(_, invocation, reload, _) => (
            "save_rejected",
            "not_applied",
            same_invocation(invocation),
            Some(*reload),
        ),
        Job::Uncertain(_, invocation) => (
            "uncertain",
            "unknown",
            same_invocation(invocation),
            None,
        ),
        Job::Preplay(..) => ("preplay", "not_write", None, None),
        Job::VPortalPrepared(..) => ("vportal_prepared", "not_write", None, None),
        Job::HlsTrack(..) => ("hls_track", "not_write", None, None),
        Job::Parental(..) => ("parental", "not_write", None, None),
        Job::PublicLoaded(..) => ("public_loaded", "not_write", None, None),
        Job::PublicSaved(..) => ("public_saved", "not_write", None, None),
        Job::Loaded(..) => ("loaded", "not_write", None, None),
        Job::LoadedItemsUncertain(..) => ("loaded_items_uncertain", "not_write", None, None),
        Job::PlexRestored(..) => ("plex_restored", "not_write", None, None),
        Job::PlexVerified(..) => ("plex_verified", "not_write", None, None),
        Job::Play(..) => ("play", "not_write", None, None),
        Job::PlexPrepared(..) => ("plex_prepared", "not_write", None, None),
        Job::FavoriteCommit(..) => ("favorite_commit", "not_write", None, None),
        Job::FavoriteRecovery(..) => ("favorite_recovery", "not_write", None, None),
    }
}

fn verified_write(app: &mut App, request: &Invocation) -> (Saved, Job) {
    let verified_source = source(&request.params["settings"]);
    let mut candidate = app.ott.as_ref().unwrap().saved.clone();
    candidate
        .settings
        .insert("nativePlexSources".into(), json!([verified_source.clone()]));
    candidate.validate().unwrap();
    admit_verifier_context(app);

    // Divert only the admitted writer's reply so an immediately completing
    // worker cannot be consumed by the same tick as PlexVerified. This uses the
    // existing channel, not a production hook or a substitute persistence API.
    let (write_tx, write_rx) = std::sync::mpsc::channel();
    let original_tx = std::mem::replace(&mut app.ott.as_mut().unwrap().tx, write_tx);
    original_tx
        .send(Job::PlexVerified(
            request.clone(),
            Ok(ott_import::verified_source_for_test(verified_source)),
        ))
        .unwrap_or_else(|_| panic!("fixture receiver disappeared"));
    runtime_tick(app);
    app.ott.as_mut().unwrap().tx = original_tx;
    assert!(
        !app.ott.as_ref().unwrap().plex_refresh_pending,
        "verifier/save fixture must not admit background source restoration"
    );
    let job = write_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("actual writer completion");
    assert!(
        matches!(&job, Job::Saved(Err("Could not prepare protected OTT settings for saving"), Some(inv), true, _)
        if inv.id == request.id),
        "actual writer completion: {:?}; request_deadline_ms={}; acceptance_sample_ms={}",
        write_completion_diagnostic(&job, request),
        request.deadline_ms,
        app.ott.as_ref().unwrap().now_ms()
    );
    let runtime = app.ott.as_ref().unwrap();
    assert!(runtime.saving);
    assert_eq!(runtime.verified_imports.len(), 1);
    assert!(
        runtime
            .remote
            .response_for_id_for_test(&request.id)
            .is_none(),
        "verification and write admission must not publish saved:true"
    );
    (candidate, job)
}

fn response(app: &App, request: &Invocation) -> Value {
    app.ott
        .as_ref()
        .unwrap()
        .remote
        .response_for_id_for_test(&request.id)
        .expect("addressed delivery response")
}

fn no_secrets(value: &Value) {
    let serialized = value.to_string();
    for secret in [SERVER, NEW_SERVER, TOKEN, NEW_TOKEN] {
        assert!(
            !serialized.contains(secret),
            "receipt must contain names, not values"
        );
    }
}

fn confirmed_fields(settings: Value, expected: Value) {
    let _serial = plx_base::testlock::serial();
    let (_reset, mut app) = setup();
    let request = invocation(&mut app, settings.clone());
    let (candidate, job) = verified_write(&mut app, &request);
    let Job::Saved(_, invocation, reload, reply) = job else {
        unreachable!()
    };
    assert_eq!(reply["data"]["fields"], expected);
    no_secrets(&reply);
    // This no-boot fixture has no persisted native client ID. Supply the exact
    // compatible registry source through the existing no-IO test seam, so the
    // actual register_verified step can borrow it after the modeled save. This
    // exercises ACK/application, not first-time session or import persistence.
    let stored = candidate.plex_sources().unwrap();
    let verified_source = &stored[0];
    let origin = verified_source.origin().unwrap();
    assert!(plx_plex::plex::grant::allow_imported_local(
        plx_plex::plex::grant::scope(),
        &origin,
        &verified_source.machine_id,
        &verified_source.machine_id,
    ));
    let sid = plx_plex::plex::register_pinned_with_client_id(
        &verified_source.machine_id,
        &origin,
        &verified_source.token,
        None,
        "field-ack-fixture-client",
        plx_plex::plex::ConnectionFacts::default(),
    );
    let client = plx_plex::plex::client_for(sid).expect("compatible fixture source");
    assert!(client.origin() == &origin);
    assert!(client.uses_token(&verified_source.token));

    // A modeled Durable/readback-success completion traverses the real Saved
    // handler and delivery cache. The actual revoked write above did no IO.
    deliver(
        &mut app,
        Job::Saved(Ok(candidate), invocation, reload, reply),
    );
    let result = response(&app, &request);
    no_secrets(&result);
    assert_eq!(
        result["status"],
        "ok",
        "saved application failed: {}",
        result["data"]["error"].as_str().unwrap_or("no error detail")
    );
    assert_eq!(result["data"]["provider"], "plex");
    assert_eq!(result["data"]["saved"], true);
    assert_eq!(result["data"]["fields"], expected);
    no_secrets(&result);
    let sources = app.ott.as_ref().unwrap().saved.plex_sources().unwrap();
    assert_eq!(sources.len(), 1);
    assert_eq!(
        sources[0].origin_url,
        settings["server"].as_str().unwrap_or(SERVER)
    );
    assert_eq!(
        sources[0].token,
        settings["token"].as_str().unwrap_or(TOKEN)
    );
}

#[test]
fn token_only_verified_save_ack_lists_only_token() {
    confirmed_fields(json!({"token":NEW_TOKEN}), json!(["token"]));
}

#[test]
fn server_only_verified_save_ack_lists_only_server() {
    confirmed_fields(json!({"server":NEW_SERVER}), json!(["server"]));
}

#[test]
fn two_field_verified_save_ack_lists_both_submitted_fields() {
    confirmed_fields(
        json!({"server":NEW_SERVER,"token":NEW_TOKEN}),
        json!(["server", "token"]),
    );
}

#[test]
fn verification_failure_does_not_claim_saved_or_echo_credentials() {
    let _serial = plx_base::testlock::serial();
    let (_reset, mut app) = setup();
    let before = serde_json::to_value(&app.ott.as_ref().unwrap().saved).unwrap();
    let request = invocation(&mut app, json!({"token":NEW_TOKEN}));
    admit_verifier_context(&mut app);
    deliver(
        &mut app,
        Job::PlexVerified(
            request.clone(),
            Err("Plex server did not authorize library access"),
        ),
    );
    let result = response(&app, &request);
    assert_eq!(result["status"], "rejected");
    assert_ne!(result["data"]["saved"], true);
    no_secrets(&result);
    assert_eq!(
        serde_json::to_value(&app.ott.as_ref().unwrap().saved).unwrap(),
        before
    );
}

#[test]
fn actual_refused_write_does_not_send_the_prepared_success_ack() {
    let _serial = plx_base::testlock::serial();
    let (_reset, mut app) = setup();
    let before = serde_json::to_value(&app.ott.as_ref().unwrap().saved).unwrap();
    let request = invocation(&mut app, json!({"token":NEW_TOKEN}));
    let (_, failed_write) = verified_write(&mut app, &request);
    deliver(&mut app, failed_write);
    let result = response(&app, &request);
    assert_eq!(result["status"], "rejected");
    assert_ne!(result["data"]["saved"], true);
    no_secrets(&result);
    assert_eq!(
        serde_json::to_value(&app.ott.as_ref().unwrap().saved).unwrap(),
        before
    );
}

#[test]
fn unknown_write_outcome_keeps_success_ack_unpublished() {
    let _serial = plx_base::testlock::serial();
    let (_reset, mut app) = setup();
    let request = invocation(&mut app, json!({"server":NEW_SERVER}));
    let (_, job) = verified_write(&mut app, &request);
    let Job::Saved(_, invocation, _, _) = job else {
        unreachable!()
    };
    deliver(&mut app, Job::Uncertain(None, invocation));
    let result = response(&app, &request);
    assert_eq!(result["status"], "rejected");
    assert_ne!(result["data"]["saved"], true);
    no_secrets(&result);
    assert!(app.ott.as_ref().unwrap().persistence_uncertain);
}
