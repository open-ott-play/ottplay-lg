//! Whole-app list/refusal/startup regressions. Candidate verification is replaced
//! by a controlled worker result; raw Delivery, command/UI policy, coordinator,
//! and owner/adapter queues remain real. These are not activation qualification.
use super::*;
use crate::app::ott_import::managed_candidate_for_test;
use crate::app::ott_kiosk_stored::Stored;
use plx_data::ott::favorites::{Locator, Origin};
use plx_plex::plex::{self, session::{Session, SourceRef}};
use std::sync::mpsc::Sender;

const SERVER: &str = "managed-fixture-machine";
const PIN_RECORD: &str = "pbkdf2-sha256$600000$000102030405060708090a0b0c0d0e0f$ac310cc8527c6c3a4e20d2a2246ebd77d59766dae1c5fe843d65ecf6a7e20c65";

struct RegistryReset;
impl RegistryReset {
    fn new() -> Self {
        plex::reset_servers_for_test();
        plex::grant::reset_for_test();
        Self
    }
}
impl Drop for RegistryReset {
    fn drop(&mut self) {
        plx_media::player::SHARED.reset_session();
        plx_media::player::TX.reset();
        plx_media::route::reset_player_control_for_test(&plx_media::route::PlaybackSession::IDLE);
        plex::reset_servers_for_test();
        plex::grant::reset_for_test();
    }
}

fn source() -> SourceRef {
    SourceRef {
        machine_id: SERVER.into(),
        name: "Managed fixture".into(),
        address: "managed.invalid".into(),
        port: 32400,
        origin_url: "https://managed.invalid:32400".into(),
        token: "synthetic-managed-pms-token".into(),
        tier: Some(plex::probe::Location::Local),
        ..Default::default()
    }
}

fn hold_startup(app: &mut App) -> (Session, Sender<managed::CandidateResult>) {
    let mut runtime = app.ott.take().unwrap();
    let mut captured = None;
    assert!(managed::start_with(&mut runtime, app, |saved, cancelled, sender| {
        assert!(!cancelled.load(Ordering::Acquire));
        captured = Some((saved, sender));
        true
    }));
    assert!(runtime.managed.has_candidate(), "the real startup receiver is retained");
    app.ott = Some(runtime);
    captured.unwrap()
}

fn deliver_candidate(app: &mut App, saved: &Session, sender: Sender<managed::CandidateResult>) {
    let candidate = managed_candidate_for_test(saved, source());
    let mut runtime = app.ott.take().unwrap();
    // An explicit foreground intent rules out automatic boot activation. This
    // does not grant candidate eligibility or inject an offered row/context.
    managed::navigation(&mut runtime.managed);
    assert!(sender.send(Ok(Some(candidate))).is_ok());
    managed::tick(&mut runtime, app);
    app.ott = Some(runtime);
}

fn fixture() -> App {
    let mut app = ready_app_fixture();
    app.running = true;
    {
        let runtime = app.ott.as_mut().unwrap();
        runtime.write_permit.revoke();
        runtime.catalog_done.store(false, Ordering::Release);
        runtime.metadata_done.store(false, Ordering::Release);
        runtime.resolver_busy = true;
    }
    let (saved, sender) = hold_startup(&mut app);
    deliver_candidate(&mut app, &saved, sender);
    app
}

/// The real raw JSON ingress assigns runtime authority and request deadline.
fn raw_request(runtime: &mut Runtime, serial: u64, params: Value) -> Invocation {
    let now = runtime.now_ms();
    let body = json!({"commands":[],"server_time":1000,"requests":[{
        "id":format!("{serial:032x}"),"action":"plex_servers",
        "params":params,"expires_at":1020
    }]}).to_string();
    let mut requests = runtime.remote.raw_poll_for_test(&body, now);
    assert_eq!(requests.len(), 1, "one request crosses the actual raw Delivery boundary");
    let invocation = requests.remove(0);
    assert!(runtime.remote.invocation_is_current(&invocation));
    assert_eq!(invocation.deadline_ms, now + 20_000);
    invocation
}

fn dispatch(app: &mut App, invocation: Invocation) -> Value {
    let mut runtime = app.ott.take().unwrap();
    let id = invocation.id.clone();
    let response = commands::command(&mut runtime, app, &invocation)
        .expect("list/refusal must finish without starting selection work");
    runtime.remote.complete(invocation, response);
    let retained = runtime.remote.response_for_id_for_test(&id).unwrap();
    app.ott = Some(runtime);
    retained
}

fn request(app: &mut App, serial: u64, params: Value) -> Value {
    let invocation = raw_request(app.ott.as_mut().unwrap(), serial, params);
    dispatch(app, invocation)
}

fn list(app: &mut App, serial: u64) -> Value {
    let runtime = app.ott.as_ref().unwrap().remote.runtime().to_owned();
    let response = request(app, serial, json!({"version":1,"runtime":runtime,"op":"list"}));
    assert_eq!(response["status"], "ok");
    assert_eq!(response["data"]["state"], "ready");
    assert_eq!(response["data"]["servers"].as_array().unwrap().len(), 1);
    assert_eq!(response["data"]["servers"][0]["server_id"], SERVER);
    response["data"]["servers"][0].clone()
}

fn select_params(app: &App, row: &Value) -> Value {
    json!({"version":1,"runtime":app.ott.as_ref().unwrap().remote.runtime(),"op":"select",
        "server_id":row["server_id"],"context":row["context"]})
}

fn ui_action(app: &mut App, action: OttAction) -> Result<(), &'static str> {
    let mut runtime = app.ott.take().unwrap();
    let result = handle_action(&mut runtime, app, action);
    app.ott = Some(runtime);
    result
}

fn unchanged_fields(app: &App) -> Value {
    let runtime = app.ott.as_ref().unwrap();
    json!({
        "saved_bytes":serde_json::to_string(&runtime.saved).unwrap(),
        "session_owner":serde_json::to_value(app.bridge.snapshot_session_init()).unwrap(),
        "provider":runtime.provider.canonical_id(),"page":runtime.page,
        "generation":runtime.generation,"navigation":runtime.navigation_epoch,
        "playback":runtime.playback_epoch,"selected":runtime.selected,
        "catalog_revision":runtime.catalog_revision,
        "catalog_entries":runtime.catalog.entries.iter().map(|e| (&e.id,&e.title)).collect::<Vec<_>>(),
        "playing_selection":runtime.playing.as_ref().map(|p| &p.selection_id),
        "effective_profile":runtime.effective_profile().map(|p| &p.id),
    })
}

fn assert_no_handoff(app: &mut App) {
    assert!(plex::id_of_machine(SERVER).is_none());
    let runtime = app.ott.as_ref().unwrap();
    assert!(!runtime.saving);
    assert!(runtime.rx.try_recv().is_err());
    // Drain actual queued owner/adapter effects: an accidentally queued managed
    // command must not be hidden by checking the owner before its next frame.
    settle_pages(app);
    let owner = app.bridge.snapshot_session_init();
    assert!(owner.managed.is_none());
    assert!(owner.pending_commit.is_none());
    assert!(owner.pending.is_empty());
    assert!(plex::id_of_machine(SERVER).is_none());
}

#[test]
fn managed_candidate_delivery_does_not_block_the_actual_loaded_result() {
    let _serial = plx_base::testlock::serial();
    let _reset = RegistryReset::new();
    let mut app = app_fixture();
    app.running = true;
    let (saved, sender) = hold_startup(&mut app);
    assert!(!app.ott.as_ref().unwrap().loaded);
    assert!(app.ott.as_ref().unwrap().tx.send(Job::Loaded(Ok((Saved::default(), [1;16])))).is_ok());
    runtime_tick(&mut app);
    let runtime = app.ott.as_ref().unwrap();
    assert!(runtime.loaded, "held candidate work must not hold protected-state readiness");
    assert_eq!(runtime.state_load, StateLoad::Idle);
    let before = unchanged_fields(&app);
    deliver_candidate(&mut app, &saved, sender);
    assert_eq!(unchanged_fields(&app), before);
    list(&mut app, 1);
    assert_no_handoff(&mut app);
}

#[test]
fn managed_list_preserves_an_admitted_local_session_and_reuses_context() {
    let _serial = plx_base::testlock::serial();
    let _reset = RegistryReset::new();
    // Reuse the existing real Local root/admission fixture rather than creating
    // a second model of Local ownership. Only this case opens a temporary root.
    let (_directory, mut app, root) = local_integration::setup();
    app.running = true;
    app.ott.as_mut().unwrap().saving = false;
    app.ott.as_mut().unwrap().entropy = Some([1;16]);
    local_integration::install(&mut app, Some(&root), "local-before-managed-list");
    let (saved, sender) = hold_startup(&mut app);
    deliver_candidate(&mut app, &saved, sender);
    let identity = plx_media::route::playback_identity(&app.player.session);
    let source = plx_media::route::external_source(&app.player.session);
    assert!(identity.is_some());
    let before = unchanged_fields(&app);
    let first = list(&mut app, 1);
    let second = list(&mut app, 2);
    assert_eq!(first["context"], second["context"]);
    assert_eq!(first["expires_in_ms"], second["expires_in_ms"]);
    assert_eq!(unchanged_fields(&app), before);
    assert_eq!(plx_media::route::playback_identity(&app.player.session), identity);
    assert_eq!(plx_media::route::external_source(&app.player.session), source);
    assert!(app.ott.as_ref().unwrap().transient.accepted.is_some());
    assert_no_handoff(&mut app);
}

#[test]
fn managed_ui_and_raw_remote_select_obey_providers_pin_without_saving() {
    let _serial = plx_base::testlock::serial();
    let _reset = RegistryReset::new();
    let mut app = fixture();
    let row = list(&mut app, 1);
    let action = managed::row(app.ott.as_ref().unwrap()).unwrap().action;
    let runtime = app.ott.as_mut().unwrap();
    runtime.saved.settings.insert("parentPin".into(), json!(PIN_RECORD));
    runtime.saved.settings.insert("requirePinForProviderSelection".into(), json!(1));
    let before = unchanged_fields(&app);
    assert!(ui_action(&mut app, action).is_err());
    let params = select_params(&app, &row);
    let response = request(&mut app, 2, params);
    assert_eq!(response["status"], "rejected");
    assert_eq!(response["data"]["error"], "restricted");
    assert_eq!(unchanged_fields(&app), before);
    assert_no_handoff(&mut app);
}

#[test]
fn managed_ui_and_raw_remote_select_obey_normal_and_strict_kiosk() {
    let _serial = plx_base::testlock::serial();
    let _reset = RegistryReset::new();
    for strict in [false, true] {
        let mut app = fixture();
        let row = list(&mut app, 1);
        let action = managed::row(app.ott.as_ref().unwrap()).unwrap().action;
        let pin = Favorite {
            title: "Pinned Local film".into(),
            origin: Origin { provider:"local".into(),profile:"Local".into(),
                subprofile:None,source_alias:"Movies".into() },
            locator: Locator::Local { root_alias:"Movies".into(),relative_path:"film.mp4".into() },
        };
        app.ott.as_mut().unwrap().saved.settings.insert("nativeKiosk".into(),
            Stored::item(pin, strict).unwrap().encode().unwrap());
        let before = unchanged_fields(&app);
        assert!(ui_action(&mut app, action).is_err());
        let params = select_params(&app, &row);
        let response = request(&mut app, 2, params);
        assert_eq!(response["status"], "rejected");
        assert_eq!(response["data"]["error"], "restricted");
        assert_eq!(unchanged_fields(&app), before);
        assert_no_handoff(&mut app);
    }
}

#[test]
fn managed_wrong_context_and_cross_runtime_requests_never_reach_handoff() {
    let _serial = plx_base::testlock::serial();
    let _reset = RegistryReset::new();
    let mut app = fixture();
    let row = list(&mut app, 1);
    let before = unchanged_fields(&app);
    assert!(ui_action(&mut app, OttAction::SelectManagedPlex {
        server_id:SERVER.into(),context:"unknown-context".into(),
    }).is_err());
    let mut params = select_params(&app, &row);
    params["context"] = json!("unknown-context");
    assert_eq!(request(&mut app, 2, params)["data"]["error"], "stale");
    let mut params = select_params(&app, &row);
    params["runtime"] = json!("previous-runtime");
    let response = request(&mut app, 3, params);
    assert_eq!(response["data"]["error"], "stale");
    assert_eq!(response["data"]["runtime"], "previous-runtime");
    assert_eq!(unchanged_fields(&app), before);
    assert_no_handoff(&mut app);
}

#[test]
fn managed_expired_original_invocation_never_reaches_handoff() {
    let _serial = plx_base::testlock::serial();
    let _reset = RegistryReset::new();
    let mut app = fixture();
    let row = list(&mut app, 1);
    let params = select_params(&app, &row);
    let runtime = app.ott.as_mut().unwrap();
    let mut invocation = raw_request(runtime, 2, params);
    // Model a queued raw-admitted invocation reaching dispatch at its original
    // deadline, without sleeping or replacing production clock construction.
    invocation.deadline_ms = runtime.now_ms();
    let before = unchanged_fields(&app);
    let response = dispatch(&mut app, invocation);
    assert_eq!(response["status"], "rejected");
    assert_eq!(response["data"]["error"], "expired");
    assert_eq!(unchanged_fields(&app), before);
    assert_no_handoff(&mut app);
}

#[test]
fn managed_network_invalidation_replaces_context_without_activating_source() {
    let _serial = plx_base::testlock::serial();
    let _reset = RegistryReset::new();
    let mut app = fixture();
    let first = list(&mut app, 1);
    let before = unchanged_fields(&app);
    plex::grant::network_changed();
    let second = list(&mut app, 2);
    assert_ne!(first["context"], second["context"]);
    let params = select_params(&app, &first);
    assert_eq!(request(&mut app, 3, params)["data"]["error"], "stale");
    assert_eq!(unchanged_fields(&app), before);
    assert_no_handoff(&mut app);
}

#[test]
fn managed_list_deduplicates_saved_label_only_after_actual_compatible_authorization() {
    let _serial = plx_base::testlock::serial();
    let _reset = RegistryReset::new();
    let mut app = fixture();
    let saved_profile = ProviderProfile {
        id: "saved-matching-machine".into(), name: "User's existing label".into(),
        kind: ProviderKind::Plex, source: SERVER.into(), source_alias: String::new(), enabled: true,
    };
    app.ott.as_mut().unwrap().saved.profiles.profiles.push(saved_profile.clone());
    let before = unchanged_fields(&app);
    let stale = list(&mut app, 1);
    assert_eq!(stale["name"], "Managed fixture");
    assert!(!managed::shadows_profile(app.ott.as_ref().unwrap(), &saved_profile));
    assert!(plex::id_of_machine(SERVER).is_none(), "a matching Saved string grants no registry authority");
    assert_eq!(unchanged_fields(&app), before);

    // Arrange a real already-authorized compatible client. Listing itself must
    // neither create this lease nor repoint the current server.
    let verified = crate::app::ott_import::verified_source_for_test(source());
    let lease = plex::register_imported(&verified.proof, &verified.source.token, "fixture-client",
        plex::ConnectionFacts::new(verified.source.tier, plex::IpVersion::of_host(&verified.source.address)), None)
        .expect("synthetic proof admits the arranged existing client");
    let current = plex::current_server();
    let compatible = list(&mut app, 2);
    assert_eq!(compatible["name"], "User's existing label");
    assert!(managed::shadows_profile(app.ott.as_ref().unwrap(), &saved_profile));
    assert_eq!(plex::id_of_machine(SERVER), Some(lease.server_id()));
    assert_eq!(plex::current_server(), current);
    assert_eq!(unchanged_fields(&app), before);
    settle_pages(&mut app);
    assert!(app.bridge.snapshot_session_init().managed.is_none());
    assert!(app.bridge.snapshot_session_init().pending.is_empty());
    assert_eq!(unchanged_fields(&app), before);
}
