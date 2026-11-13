use super::*;
use plx_data::ott::favorites::{Favorite, Locator, Origin as FavoriteOrigin};
use std::sync::Mutex;

#[derive(Default)]
struct FixtureState {
    requests: Vec<Value>,
    paths: Vec<String>,
    status: Option<service::StatusResponse>,
    items: Vec<service::CatalogItem>,
    bindings: std::collections::BTreeMap<u8, service::Binding>,
    grant: Option<service::Binding>,
    lose_play_reply: bool,
    hold_create: Option<Arc<(AtomicBool, AtomicBool)>>,
    hold_play_reply: Option<Arc<(AtomicBool, AtomicBool)>>,
    post_play_status: Option<fn(&mut service::StatusResponse)>,
    reject_play: bool,
    focus_slot: u8,
}
#[derive(Clone)]
struct FixtureTransport(Arc<Mutex<FixtureState>>);
impl service::Transport for FixtureTransport {
    fn send(
        &self,
        _: &OffFrame,
        binding: &service::ServiceBinding,
        request: &service::ServiceRequest,
        _: Deadline,
        cancelled: &AtomicBool,
    ) -> Result<Vec<u8>, service::ServiceError> {
        if cancelled.load(Ordering::Acquire) {
            return Err(service::ServiceError::Cancelled);
        }
        let path = request.url(binding);
        let body: Value = serde_json::from_slice(request.body().unwrap_or(b"{}")).unwrap();
        if path.ends_with("/sessions") {
            let hold = self.0.lock().unwrap().hold_create.clone();
            if let Some(hold) = hold {
                hold.0.store(true, Ordering::Release);
                let end = Instant::now() + Duration::from_secs(5);
                while !hold.1.load(Ordering::Acquire) {
                    if cancelled.load(Ordering::Acquire) {
                        return Err(service::ServiceError::Cancelled);
                    }
                    assert!(Instant::now() < end, "fixture create release deadline");
                    std::thread::sleep(Duration::from_millis(1));
                }
            }
        }
        let mut state = self.0.lock().unwrap();
        state.paths.push(path.clone());
        state.requests.push(body.clone());
        let value = if path.ends_with("/capabilities") {
            serde_json::to_value(capabilities()).unwrap()
        } else if path.ends_with("/sessions") {
            let native: service::NativeExpected =
                serde_json::from_value(body["native_expected"].clone()).unwrap();
            let geometry: service::Geometry =
                serde_json::from_value(body["geometry"].clone()).unwrap();
            let status = service::StatusResponse {
                version: 1,
                server_id: "server_fixture".into(),
                server_epoch: "epoch_fixture".into(),
                session_id: "session_fixture".into(),
                native_expected: native,
                backend_expected: service::BackendExpected {
                    runtime: 1,
                    scene: "backend_scene".into(),
                    geometry_revision: 1,
                    focus_revision: 1,
                    audio_revision: 1,
                },
                state: service::SessionState::Preparing,
                slots: geometry
                    .rects
                    .iter()
                    .map(|r| service::SlotSnapshot {
                        slot_id: r.slot_id,
                        source_revision: 0,
                        state: service::SlotState::Empty,
                        error: None,
                        frames: 0,
                        position_ms: None,
                        duration_ms: None,
                        source_ref: None,
                        item_ref: None,
                    })
                    .collect(),
                geometry,
                audio: service::AudioState {
                    policy: service::AudioPolicy::Muted,
                    slot_id: None,
                },
                output: None,
            };
            state.bindings.clear();
            state
                .bindings
                .insert(status.native_expected.slot_id, binding_from_status(&status));
            state.status = Some(status.clone());
            serde_json::to_value(status).unwrap()
        } else if path.ends_with("/sources") {
            let prior: service::Binding = serde_json::from_value(body["context"].clone()).unwrap();
            fixture_require_prior(&mut state, &prior, true)?;
            json!({"version":1,"server_id":"server_fixture","context":prior,"catalog_revision":"catalog_fixture",
                "items":state.items,"next_cursor":null})
        } else if path.ends_with("/input-grants") {
            let mut committed: service::Binding =
                serde_json::from_value(body["context"].clone()).unwrap();
            fixture_require_prior(&mut state, &committed, false)?;
            if !body["next_native"].is_null() {
                committed.native_expected =
                    serde_json::from_value(body["next_native"].clone()).unwrap();
            }
            fixture_commit(&mut state, committed.clone());
            state.grant = Some(committed.clone());
            let item = state
                .items
                .iter()
                .find(|item| item.item_ref == body["item_ref"])
                .unwrap();
            json!({"version":1,"server_id":"server_fixture","context":committed,"item":item,
                "catalog_revision":body["catalog_revision"],"policy_revision":"policy_fixture","input_grant":"grant_fixture",
                "expires_in_ms":body["remaining_budget_ms"]})
        } else if path.ends_with("/control") {
            let mut context: service::Binding =
                serde_json::from_value(body["context"].clone()).unwrap();
            fixture_require_prior(&mut state, &context, true)?;
            if !body["next_native"].is_null() {
                context.native_expected =
                    serde_json::from_value(body["next_native"].clone()).unwrap();
            }
            if body["op"]["name"] == "play" && state.reject_play {
                return Ok(serde_json::to_vec(&json!({"version":1,"context":context,"accepted":false,"operation_id":"operation_fixture","state":state.status.as_ref().unwrap().state})).unwrap());
            }
            if body["op"]["name"] == "audio" {
                let policy: service::AudioPolicy =
                    serde_json::from_value(body["op"]["policy"].clone()).unwrap();
                if state.status.as_ref().unwrap().audio.policy != policy
                    || (policy == service::AudioPolicy::Slot
                        && state.status.as_ref().unwrap().audio.slot_id
                            != Some(context.native_expected.slot_id))
                {
                    context.backend_expected.audio_revision += 1;
                }
                let owner = match policy {
                    service::AudioPolicy::Muted => None,
                    service::AudioPolicy::Slot => Some(context.native_expected.slot_id),
                    service::AudioPolicy::FollowsFocus => state
                        .status
                        .as_ref()
                        .unwrap()
                        .slots
                        .iter()
                        .find(|slot| {
                            slot.slot_id == state.focus_slot
                                && slot.state == service::SlotState::Playing
                        })
                        .map(|slot| slot.slot_id),
                };
                state.status.as_mut().unwrap().audio = service::AudioState {
                    policy,
                    slot_id: owner,
                };
            }
            if body["op"]["name"] == "focus" {
                state.focus_slot = context.native_expected.slot_id;
                context.backend_expected.focus_revision += 1;
                if state.status.as_ref().unwrap().audio.policy == service::AudioPolicy::FollowsFocus
                {
                    context.backend_expected.audio_revision += 1;
                    let owner = state
                        .status
                        .as_ref()
                        .unwrap()
                        .slots
                        .iter()
                        .find(|slot| {
                            slot.slot_id == state.focus_slot
                                && slot.state == service::SlotState::Playing
                        })
                        .map(|slot| slot.slot_id);
                    state.status.as_mut().unwrap().audio.slot_id = owner;
                }
            }
            if body["op"]["name"] == "layout" {
                let geometry: service::Geometry =
                    serde_json::from_value(body["op"]["geometry"].clone()).unwrap();
                context.backend_expected.geometry_revision = geometry.revision;
                state.status.as_mut().unwrap().geometry = geometry;
            }
            if body["op"]["name"] == "off" {
                state.status.as_mut().unwrap().state = service::SessionState::Closed;
            }
            if body["op"]["name"] == "play" {
                if state.grant.take().as_ref() != Some(&context) {
                    return Err(service::ServiceError::AuthorityRevoked);
                }
                let item = state.items[0].clone();
                let slot = state
                    .status
                    .as_mut()
                    .unwrap()
                    .slots
                    .iter_mut()
                    .find(|slot| slot.slot_id == context.native_expected.slot_id)
                    .unwrap();
                slot.state = service::SlotState::Playing;
                slot.frames = 1;
                slot.source_revision += 1;
                context.backend_slot_revision = slot.source_revision;
                slot.source_ref = Some(item.source_ref);
                slot.item_ref = Some(item.item_ref);
            }
            fixture_commit(&mut state, context.clone());
            if body["op"]["name"] == "play" && state.lose_play_reply {
                return Err(service::ServiceError::Transport);
            }
            json!({"version":1,"context":context,"accepted":true,"operation_id":"operation_fixture","state":state.status.as_ref().unwrap().state})
        } else if path.ends_with("/status") {
            let requested: service::NativeExpected =
                serde_json::from_value(body["native_expected"].clone()).unwrap();
            let authoritative = state
                .bindings
                .get(&requested.slot_id)
                .ok_or(service::ServiceError::RevisionConflict)?
                .clone();
            if requested.runtime != authoritative.native_expected.runtime
                || requested.expected.scene != authoritative.native_expected.expected.scene
            {
                return Err(service::ServiceError::RevisionConflict);
            }
            let mut status = state.status.as_ref().unwrap().clone();
            status.native_expected = authoritative.native_expected;
            status.backend_expected = authoritative.backend_expected;
            if status
                .slots
                .iter()
                .any(|slot| slot.state == service::SlotState::Playing)
            {
                if let Some(mutate) = state.post_play_status.take() {
                    mutate(&mut status);
                }
            }
            serde_json::to_value(status).unwrap()
        } else {
            return Err(service::ServiceError::InvalidRequest);
        };
        let hold = if body["op"]["name"] == "play" {
            state.hold_play_reply.clone()
        } else {
            None
        };
        drop(state);
        if let Some(hold) = hold {
            hold.0.store(true, Ordering::Release);
            let end = Instant::now() + Duration::from_secs(5);
            while !hold.1.load(Ordering::Acquire) {
                if cancelled.load(Ordering::Acquire) {
                    return Err(service::ServiceError::Cancelled);
                }
                assert!(Instant::now() < end, "fixture Play reply release deadline");
                std::thread::sleep(Duration::from_millis(1));
            }
        }
        Ok(serde_json::to_vec(&value).unwrap())
    }
}
// Fixture models the real server's enrolled-slot CAS rather than echoing native intent.
fn fixture_require_prior(
    state: &mut FixtureState,
    prior: &service::Binding,
    enroll: bool,
) -> Result<(), service::ServiceError> {
    let id = prior.native_expected.slot_id;
    if let Some(current) = state.bindings.get(&id) {
        return if current == prior {
            Ok(())
        } else {
            Err(service::ServiceError::RevisionConflict)
        };
    }
    let current = state
        .bindings
        .values()
        .next()
        .ok_or(service::ServiceError::RevisionConflict)?;
    let empty = state.status.as_ref().unwrap().slots.iter().any(|slot| {
        slot.slot_id == id && slot.source_revision == 0 && slot.state == service::SlotState::Empty
    });
    if !enroll
        || !empty
        || prior.backend_slot_revision != 0
        || prior.backend_expected != current.backend_expected
        || prior.native_expected.runtime != current.native_expected.runtime
        || prior.native_expected.expected != current.native_expected.expected
    {
        return Err(service::ServiceError::RevisionConflict);
    }
    state.bindings.insert(id, prior.clone());
    Ok(())
}
fn fixture_commit(state: &mut FixtureState, context: service::Binding) {
    for prior in state.bindings.values_mut() {
        prior.backend_expected = context.backend_expected.clone();
        prior.native_expected.expected = context.native_expected.expected.clone();
    }
    state
        .bindings
        .insert(context.native_expected.slot_id, context.clone());
    let status = state.status.as_mut().unwrap();
    status.native_expected = context.native_expected;
    status.backend_expected = context.backend_expected;
}
fn capabilities() -> service::Capabilities {
    service::Capabilities {
        version: 1,
        server_id: "server_fixture".into(),
        server_epoch: "epoch_fixture".into(),
        max_inputs: 4,
        qualified_input_adapters: vec!["http_checked".into()],
        output_auth: "hmac_path_v1".into(),
        catalog_revision: "catalog_fixture".into(),
        output_profiles: vec![service::OutputProfile {
            profile_id: "profile_fixture".into(),
            canvas: service::Canvas {
                width: 320,
                height: 180,
                fps: 10,
            },
            max_inputs: 4,
            geometry_modes: vec![service::GeometryMode::Disjoint, service::GeometryMode::Pip],
        }],
    }
}
fn binding_fixture() -> service::ServiceBinding {
    service::ServiceBinding::from_protected_json(&json!({"server_id":"server_fixture","origin":"https://service.invalid",
        "audience":service::AUDIENCE,"installation_id":"dev_driver","control_bearer":"fixture-bearer-abcdefghijklmnopqrstuvwxyz"}).to_string()).unwrap().unwrap()
}
fn setup() -> (Driver, App, Runtime, Arc<Mutex<FixtureState>>) {
    let mut app = crate::app::run::lifecycle_regression_tests::app();
    app.running = true;
    let mut runtime = Runtime::unloaded(crate::app::ott_companion::Supervisor::inert_for_test());
    runtime.loaded = true;
    runtime.state_load = StateLoad::Idle;
    runtime.entropy = Some([1; 16]);
    runtime.resume_source_work();
    runtime.remote = Remote::disabled_for_test("dev_driver", "runtime_fixture");
    let sample = task::time::ClockSample::capture_main(app.adapters.player.mt()).unwrap();
    runtime.clock_sample = Some(sample);
    runtime.clock_anchor = Some(sample);
    let favorite = Favorite {
        title: "Channel".into(),
        origin: FavoriteOrigin {
            provider: "m3u".into(),
            profile: "Fixture".into(),
            subprofile: None,
            source_alias: "fixture".into(),
        },
        locator: Locator::ChannelName {
            name: "Channel".into(),
            group: Some("Documentary".into()),
        },
    };
    runtime.saved.profiles.profiles.push(ProviderProfile {
        id: "m3u-fixture".into(),
        name: "Fixture".into(),
        kind: ProviderKind::M3u,
        source: "https://example.invalid/list.m3u".into(),
        source_alias: "fixture".into(),
        enabled: true,
    });
    runtime.saved.profiles.active_profile = Some("m3u-fixture".into());
    runtime.provider = ProviderKind::M3u;
    runtime.saved.favorites.toggle(favorite.clone()).unwrap();
    let state = Arc::new(Mutex::new(FixtureState::default()));
    state.lock().unwrap().items.push(service::CatalogItem {
        source_ref: "server_source".into(),
        item_ref: "server_item".into(),
        item_revision: "item_revision".into(),
        identity_version: 1,
        favorite_sha256: plx_data::ott::favorites_wire::hash(favorite.identity().as_bytes()),
        kind: "live".into(),
        input_adapter: "http_checked".into(),
        protection: service::Protection {
            known: true,
            restricted: false,
        },
        descriptor: service::Descriptor {
            title: "Channel".into(),
            provider: "m3u".into(),
        },
    });
    let protected = json!(binding_fixture().protected_json().unwrap());
    runtime
        .saved
        .settings
        .insert("nativeMultiviewService".into(), protected.clone());
    runtime.multiview_connection = super::super::multiview_connection::State::with_io(
        Arc::new(FixtureTransport(state.clone())),
        Box::new(super::super::multiview_connection::NativeLauncher),
    );
    let until = Instant::now() + Duration::from_secs(5);
    while runtime.multiview_connection.qualified().is_none() {
        assert!(
            Instant::now() < until,
            "real capabilities qualification must settle"
        );
        runtime.multiview_connection.tick(
            true,
            true,
            Some(&protected),
            runtime.remote.persisted_device_id(),
        );
        std::thread::sleep(Duration::from_millis(1));
    }
    let driver = Driver::default();
    (driver, app, runtime, state)
}
fn invocation(runtime: &mut Runtime, id: u64, params: Value) -> Invocation {
    let now = runtime.now_ms();
    runtime
        .remote
        .raw_poll_for_test(
            &json!({"commands":[],"server_time":1000,"requests":[{"id":format!("{id:032x}"),
        "action":"multiview","params":params,"expires_at":1100}]})
            .to_string(),
            now,
        )
        .remove(0)
}
fn parameters(runtime: &Runtime, op: &str) -> Value {
    json!({"version":1,"runtime":runtime.remote.runtime(),"op":op,"expected":context_json(&context(runtime))})
}
fn drive(driver: &mut Driver, runtime: &mut Runtime, app: &mut App) {
    let end = Instant::now() + Duration::from_secs(5);
    while driver.pending.is_some() || !driver.cleanup.is_empty() {
        assert!(Instant::now() < end, "owned fixture worker deadline");
        driver.tick(runtime, app);
        std::thread::sleep(Duration::from_millis(1));
    }
}
fn layout(
    driver: &mut Driver,
    runtime: &mut Runtime,
    app: &mut App,
    id: u64,
    kind: &str,
) -> Invocation {
    let mut params = parameters(runtime, "layout");
    params["layout"] = json!(kind);
    let inv = invocation(runtime, id, params);
    assert!(driver.command(runtime, app, &inv).is_none());
    drive(driver, runtime, app);
    let until = Instant::now() + Duration::from_secs(5);
    while driver.initial_audio_pending {
        assert!(
            Instant::now() < until,
            "initial policy must be observed before controls"
        );
        driver.tick(runtime, app);
        drive(driver, runtime, app);
    }
    inv
}
#[test]
fn actual_raw_command_and_tick_create_validated_service_scene_without_native_admission() {
    let _serial = plx_base::testlock::serial();
    let (mut driver, mut app, mut runtime, state) = setup();
    let before = serde_json::to_value(&runtime.saved).unwrap();
    let inv = layout(&mut driver, &mut runtime, &mut app, 1, "grid9");
    assert_eq!(driver.session.as_ref().unwrap().status.slots.len(), 9);
    assert_eq!(driver.session.as_ref().unwrap().capacity, 4);
    assert_eq!(
        runtime.remote.response_for_id_for_test(&inv.id).unwrap()["status"],
        "ok"
    );
    assert!(state
        .lock()
        .unwrap()
        .paths
        .iter()
        .any(|path| path.ends_with("/sessions")));
    assert!(runtime.multiview_picture.is_none());
    assert_eq!(runtime.multiview.admitted_pipeline_slots(), 0);
    assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), before);
    assert!(driver.take_ready_output(&runtime).is_none());
    // An initial picture-none is preparation, not an ordinary-source supersession.
    driver.tick(&mut runtime, &mut app);
    assert!(driver.session.is_some());
    assert!(!driver.retiring);
    assert!(driver.cleanup.is_empty());
    assert!(state
        .lock()
        .unwrap()
        .requests
        .iter()
        .all(|request| request["op"]["name"] != "off"));
}
#[test]
fn original_projection_expiry_and_context_are_checked_before_any_service_effect() {
    let _serial = plx_base::testlock::serial();
    let (mut driver, app, mut runtime, state) = setup();
    // Setup performed the real capabilities qualification. Neither rejected operation may
    // add another service request after this independently captured baseline.
    let before = state.lock().unwrap().requests.len();
    let unproven = runtime
        .remote
        .request_for_test("multiview", json!({"version":1,"op":"status"}));
    assert!(driver.command(&mut runtime, &app, &unproven).is_none());
    let mut p = parameters(&runtime, "layout");
    p["layout"] = json!("grid4");
    let mut inv = invocation(&mut runtime, 2, p);
    inv.deadline_ms = 0;
    assert_eq!(
        driver.command(&mut runtime, &app, &inv).unwrap()["data"]["reason"],
        "expired"
    );
    assert_eq!(state.lock().unwrap().requests.len(), before);
    assert!(runtime.multiview.scene_status(&runtime).is_none());
}
#[test]
fn actual_sources_read_joins_only_exact_digest_and_ambiguous_page_is_not_ticketed() {
    let _serial = plx_base::testlock::serial();
    let (mut driver, mut app, mut runtime, state) = setup();
    layout(&mut driver, &mut runtime, &mut app, 3, "grid4");
    let row = runtime
        .multiview
        .scene_status(&runtime)
        .unwrap()
        .slots
        .remove(0);
    let mut p = parameters(&runtime, "sources");
    p["slot_id"] = json!(0);
    p["slot_generation"] = json!(row.slot_generation);
    p["source_revision"] = json!(row.source_revision);
    let inv = invocation(&mut runtime, 4, p.clone());
    assert!(driver.command(&mut runtime, &app, &inv).is_none());
    drive(&mut driver, &mut runtime, &mut app);
    let response = runtime.remote.response_for_id_for_test(&inv.id).unwrap();
    assert_eq!(response["data"]["items"].as_array().unwrap().len(), 1);
    let text = response.to_string();
    assert!(!text.contains("example.invalid"));
    assert!(!text.contains("control_bearer"));
    let duplicate = {
        let state = state.lock().unwrap();
        let mut item = state.items[0].clone();
        item.item_ref = "other_item".into();
        item
    };
    state.lock().unwrap().items.push(duplicate);
    let inv = invocation(&mut runtime, 5, p);
    assert!(driver.command(&mut runtime, &app, &inv).is_none());
    drive(&mut driver, &mut runtime, &mut app);
    assert_eq!(
        runtime.remote.response_for_id_for_test(&inv.id).unwrap()["data"]["items"],
        json!([])
    );
}
#[test]
fn late_create_cleanup_uses_originating_client_after_authority_changes() {
    let _serial = plx_base::testlock::serial();
    let (mut driver, mut app, mut runtime, state) = setup();
    let mut p = parameters(&runtime, "layout");
    p["layout"] = json!("grid4");
    let inv = invocation(&mut runtime, 6, p);
    assert!(driver.command(&mut runtime, &app, &inv).is_none());
    runtime.multiview_authority_epoch += 1;
    drive(&mut driver, &mut runtime, &mut app);
    assert!(state
        .lock()
        .unwrap()
        .paths
        .iter()
        .any(|path| path.ends_with("/control")));
    assert!(driver.session.is_none());
    assert!(runtime.multiview_picture.is_none());
    assert_eq!(
        runtime.remote.response_for_id_for_test(&inv.id).unwrap()["status"],
        "rejected"
    );
}

#[test]
fn observed_server_off_closed_applies_once_and_releases_scene_for_new_layout() {
    let _serial = plx_base::testlock::serial();
    let (mut driver, mut app, mut runtime, _) = setup();
    layout(&mut driver, &mut runtime, &mut app, 7, "grid4");
    let mut p = parameters(&runtime, "off");
    p["mode"] = json!("restore_previous");
    let invocation = invocation(&mut runtime, 8, p);
    assert!(driver.command(&mut runtime, &app, &invocation).is_none());
    drive(&mut driver, &mut runtime, &mut app);
    let reply = runtime
        .remote
        .response_for_id_for_test(&invocation.id)
        .unwrap();
    assert_eq!(reply["data"]["outcome"], "applied");
    assert!(runtime.multiview.scene_status(&runtime).is_none());
    assert!(driver.session.is_none());
    layout(&mut driver, &mut runtime, &mut app, 9, "pip");
    assert!(driver.session.is_some());
}

fn metadata_fixture(
    work: sources::SourceWork,
) -> Result<sources::PreparedMetadata, sources::Error> {
    sources::prepared_http_fixture_for_test(
        work,
        CatalogEntry {
            id: "fixture-channel".into(),
            title: "Channel".into(),
            category: "Documentary".into(),
            description: String::new(),
            poster: String::new(),
            synopsis: None,
            protection: Some(false),
            media: MediaLocator::Http {
                url: "https://example.invalid/video.m3u8".into(),
            },
            duration_ms: None,
            provider_id: None,
            provider_id_field: None,
            live: None,
        },
    )
}
fn source_parameters(runtime: &Runtime, operation: &str) -> Value {
    let row = runtime
        .multiview
        .scene_status(runtime)
        .unwrap()
        .slots
        .remove(0);
    let mut p = parameters(runtime, operation);
    p["slot_id"] = json!(0);
    p["slot_generation"] = json!(row.slot_generation);
    p["source_revision"] = json!(row.source_revision);
    p
}
fn start_play_fixture(
    driver: &mut Driver,
    runtime: &mut Runtime,
    app: &mut App,
    id: u64,
) -> Invocation {
    let p = source_parameters(runtime, "sources");
    let inv = invocation(runtime, id, p);
    assert!(driver.command(runtime, app, &inv).is_none());
    drive(driver, runtime, app);
    let token = runtime.remote.response_for_id_for_test(&inv.id).unwrap()["data"]["items"][0]
        ["ticket"]
        .clone();
    assert!(token.is_string());
    let mut p = source_parameters(runtime, "play");
    p["source_ticket"] = token;
    let inv = invocation(runtime, id + 1, p);
    assert!(driver.command(runtime, app, &inv).is_none());
    inv
}
fn play_fixture(driver: &mut Driver, runtime: &mut Runtime, app: &mut App, id: u64) -> Invocation {
    let invocation = start_play_fixture(driver, runtime, app, id);
    drive(driver, runtime, app);
    invocation
}
#[test]
fn actual_play_keeps_prior_server_binding_and_redeems_exact_committed_grant() {
    let _serial = plx_base::testlock::serial();
    let (mut driver, mut app, mut runtime, state) = setup();
    driver.metadata_fixture = Some(metadata_fixture);
    layout(&mut driver, &mut runtime, &mut app, 20, "grid4");
    let prior = driver.session.as_ref().unwrap().bindings[0]
        .clone()
        .unwrap();
    let invocation = play_fixture(&mut driver, &mut runtime, &mut app, 21);
    assert_eq!(
        runtime
            .remote
            .response_for_id_for_test(&invocation.id)
            .unwrap()["status"],
        "ok"
    );
    let guard = state.lock().unwrap();
    let grant = guard
        .requests
        .iter()
        .find(|r| r.get("catalog_revision").is_some() && r.get("remaining_budget_ms").is_some())
        .unwrap();
    assert_eq!(grant["context"], serde_json::to_value(&prior).unwrap());
    assert_ne!(
        grant["next_native"]["source_revision"],
        grant["context"]["native_expected"]["source_revision"]
    );
    let play = guard
        .requests
        .iter()
        .find(|r| r["op"]["name"] == "play")
        .unwrap();
    assert_eq!(play["context"]["native_expected"], grant["next_native"]);
    assert!(play.get("next_native").is_none());
    assert!(driver.inputs[0].is_some());
    assert!(driver.inputs_current(&runtime));
    assert!(guard.grant.is_none());
    drop(guard);
    // A consumed input grant cannot be replayed, even with the now-current context.
    let session = driver.session.as_ref().unwrap();
    let context = binding(&runtime, session, SlotId::SLOT_0).unwrap();
    let worker = OffFrame::for_test();
    let result = session.client.control(
        &worker,
        context,
        service::ControlOp::Play {
            input_grant: "grant_fixture".into(),
        },
        None,
        &session.capabilities,
        service::RequestId::parse("000000000000000000000000000000ff").unwrap(),
        Deadline::after(&worker, Duration::from_secs(1)),
        &AtomicBool::new(false),
    );
    assert!(matches!(
        result,
        Err(service::ServiceError::AuthorityRevoked)
    ));
}
#[test]
fn actual_status_recovers_authoritative_binding_without_applying_stale_native_revisions() {
    let _serial = plx_base::testlock::serial();
    let (mut driver, mut app, mut runtime, state) = setup();
    layout(&mut driver, &mut runtime, &mut app, 30, "grid4");
    let session = driver.session.as_ref().unwrap();
    let mut stale = session.status.native_expected.clone();
    let authoritative = {
        let mut state = state.lock().unwrap();
        let mut context = state.bindings[&0].clone();
        context.native_expected.expected.focus_revision = "fresh_focus".into();
        context.backend_expected.focus_revision += 1;
        fixture_commit(&mut state, context.clone());
        context
    };
    stale.source_revision = "stale_source".into();
    let worker = OffFrame::for_test();
    let observed = session
        .client
        .status(
            &worker,
            &authoritative.server_epoch,
            &authoritative.session_id,
            stale,
            Deadline::after(&worker, Duration::from_secs(1)),
            &AtomicBool::new(false),
        )
        .unwrap();
    assert_eq!(observed.native_expected, authoritative.native_expected);
    assert_eq!(observed.backend_expected, authoritative.backend_expected);
}
#[test]
fn playing_backend_without_retained_input_proof_never_admits_output() {
    let _serial = plx_base::testlock::serial();
    let (mut driver, mut app, mut runtime, _) = setup();
    layout(&mut driver, &mut runtime, &mut app, 40, "grid4");
    let slot = &mut driver.session.as_mut().unwrap().status.slots[0];
    slot.state = service::SlotState::Playing;
    slot.source_ref = Some("server_source".into());
    slot.item_ref = Some("server_item".into());
    assert!(!driver.inputs_current(&runtime));
    assert!(driver.authorize_inputs(&mut runtime, &app).is_err());
    assert!(driver.take_ready_output(&runtime).is_none());
}
#[test]
fn lost_play_reply_retains_input_authority_and_blocks_unconfirmed_output() {
    let _serial = plx_base::testlock::serial();
    let (mut driver, mut app, mut runtime, state) = setup();
    driver.metadata_fixture = Some(metadata_fixture);
    layout(&mut driver, &mut runtime, &mut app, 50, "grid4");
    state.lock().unwrap().lose_play_reply = true;
    let inv = start_play_fixture(&mut driver, &mut runtime, &mut app, 51);
    let end = Instant::now() + Duration::from_secs(5);
    while driver.pending.is_some() {
        assert!(Instant::now() < end);
        driver.tick(&mut runtime, &mut app);
        std::thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(
        runtime.remote.response_for_id_for_test(&inv.id).unwrap()["status"],
        "rejected"
    );
    assert_eq!(
        state.lock().unwrap().status.as_ref().unwrap().slots[0].state,
        service::SlotState::Playing
    );
    assert!(
        driver.inputs[0].is_some(),
        "lost reply must retain exact policy/Plex witness until owned cleanup"
    );
    assert!(driver.selected[0].is_some());
    assert!(driver.uncertain);
    assert!(driver.take_ready_output(&runtime).is_none());
    drive(&mut driver, &mut runtime, &mut app);
    assert_eq!(
        state.lock().unwrap().status.as_ref().unwrap().state,
        service::SessionState::Closed
    );
    assert!(driver.inputs[0].is_none());
    assert!(driver.shutdown_confirmed());
}

#[test]
fn actual_focus_enrolls_untouched_empty_slot_before_any_sources_request() {
    let _serial = plx_base::testlock::serial();
    let (mut driver, mut app, mut runtime, state) = setup();
    layout(&mut driver, &mut runtime, &mut app, 60, "grid4");
    {
        let state = state.lock().unwrap();
        assert_eq!(state.bindings.len(), 1);
        assert!(state.bindings.contains_key(&0));
        assert!(!state.bindings.contains_key(&1));
    }
    let mut p = parameters(&runtime, "audio");
    p["policy"] = json!("muted");
    let audio = invocation(&mut runtime, 61, p);
    assert_eq!(
        driver.command(&mut runtime, &app, &audio).unwrap()["status"],
        "ok"
    );
    driver.tick(&mut runtime, &mut app);
    drive(&mut driver, &mut runtime, &mut app);
    assert!(!driver.uncertain);
    let prior_shared = driver
        .session
        .as_ref()
        .unwrap()
        .status
        .native_expected
        .expected
        .clone();
    let row = runtime
        .multiview
        .scene_status(&runtime)
        .unwrap()
        .slots
        .remove(1);
    let mut p = parameters(&runtime, "focus");
    p["slot_id"] = json!(1);
    p["slot_generation"] = json!(row.slot_generation);
    p["source_revision"] = json!(row.source_revision);
    let focus = invocation(&mut runtime, 62, p);
    assert_eq!(
        driver.command(&mut runtime, &app, &focus).unwrap()["status"],
        "ok"
    );
    driver.tick(&mut runtime, &mut app);
    drive(&mut driver, &mut runtime, &mut app);
    assert!(!driver.uncertain);
    let state = state.lock().unwrap();
    assert_eq!(state.bindings.len(), 2);
    let enrolled = state.bindings.get(&1).unwrap();
    assert_eq!(enrolled.backend_slot_revision, 0);
    assert_eq!(
        enrolled.native_expected,
        native_expected(&runtime, SlotId::new(1).unwrap()).unwrap()
    );
    let focus_request = state
        .requests
        .iter()
        .find(|r| r["op"]["name"] == "focus")
        .unwrap();
    assert_eq!(focus_request["context"]["native_expected"]["slot_id"], 1);
    assert_eq!(focus_request["context"]["backend_slot_revision"], 0);
    assert_eq!(
        focus_request["context"]["native_expected"]["expected"],
        serde_json::to_value(prior_shared).unwrap()
    );
    assert_ne!(
        focus_request["next_native"]["expected"]["focus_revision"],
        focus_request["context"]["native_expected"]["expected"]["focus_revision"]
    );
    assert!(state.paths.iter().all(|path| !path.ends_with("/sources")));
    assert!(driver.session.as_ref().unwrap().bindings[1].is_some());
}

#[test]
fn delayed_create_shutdown_drains_origin_off_without_accepting_new_source_work() {
    let _serial = plx_base::testlock::serial();
    let (mut driver, mut app, mut runtime, state) = setup();
    let hold = Arc::new((AtomicBool::new(false), AtomicBool::new(false)));
    state.lock().unwrap().hold_create = Some(hold.clone());
    let mut p = parameters(&runtime, "layout");
    p["layout"] = json!("grid4");
    let create = invocation(&mut runtime, 70, p);
    assert!(driver.command(&mut runtime, &app, &create).is_none());
    let end = Instant::now() + Duration::from_secs(5);
    while !hold.0.load(Ordering::Acquire) {
        assert!(Instant::now() < end);
        std::thread::sleep(Duration::from_millis(1));
    }
    driver.begin_shutdown(&mut runtime, &mut app);
    assert!(driver.shutdown_pending());
    let p = source_parameters(&runtime, "sources");
    let sources = invocation(&mut runtime, 71, p);
    assert_eq!(
        driver.command(&mut runtime, &app, &sources).unwrap()["status"],
        "rejected"
    );
    assert!(
        !driver
            .pending
            .as_ref()
            .unwrap()
            .cancel
            .load(Ordering::Acquire),
        "dispatched create must settle before exact origin cleanup"
    );
    hold.1.store(true, Ordering::Release);
    while driver.shutdown_pending() {
        assert!(
            Instant::now() < end,
            "shutdown uses caller's one bounded drain deadline"
        );
        driver.tick_shutdown(&mut runtime, &mut app);
        std::thread::sleep(Duration::from_millis(1));
    }
    let state = state.lock().unwrap();
    assert_eq!(
        state.status.as_ref().unwrap().state,
        service::SessionState::Closed
    );
    assert_eq!(
        state
            .requests
            .iter()
            .filter(|r| r["op"]["name"] == "off")
            .count(),
        1
    );
    assert!(state
        .paths
        .iter()
        .all(|path| !path.ends_with("/sources") && !path.ends_with("/input-grants")));
    assert!(driver.session.is_none());
    assert!(runtime.multiview_picture.is_none());
    assert_eq!(
        runtime.remote.response_for_id_for_test(&create.id).unwrap()["status"],
        "rejected"
    );
}

#[test]
fn taskless_uncertain_shutdown_is_not_confirmed_closed() {
    let mut driver = Driver::default();
    driver.uncertain = true;
    assert!(!driver.shutdown_pending());
    assert!(!driver.shutdown_confirmed());
}

#[test]
fn client_play_preserves_committed_receipt_separately_from_newer_owner_status() {
    let _serial = plx_base::testlock::serial();
    let (mut driver, mut app, mut runtime, state) = setup();
    layout(&mut driver, &mut runtime, &mut app, 80, "grid4");
    state.lock().unwrap().post_play_status = Some(|status| {
        status.backend_expected.focus_revision += 1;
        status.native_expected.expected.focus_revision = "focus_after_commit".into();
    });
    let session = driver.session.as_ref().unwrap();
    let prior = binding(&runtime, session, SlotId::SLOT_0).unwrap();
    let mut next = prior.native_expected.clone();
    next.slot_generation = "next_generation".into();
    next.source_revision = "next_source".into();
    let item = state.lock().unwrap().items[0].clone();
    let worker = OffFrame::for_test();
    let played = session
        .io
        .play(
            &worker,
            prior.clone(),
            &item,
            Some(next.clone()),
            "catalog_fixture",
            1000,
            service::RequestId::parse("00000000000000000000000000000081").unwrap(),
            service::RequestId::parse("00000000000000000000000000000082").unwrap(),
            Deadline::after(&worker, Duration::from_secs(1)),
            &AtomicBool::new(false),
        )
        .unwrap();
    assert_eq!(played.committed.native_expected, next);
    assert_eq!(
        played.committed.backend_slot_revision,
        prior.backend_slot_revision + 1
    );
    assert_ne!(
        played.committed.native_expected.expected.focus_revision,
        played.status.native_expected.expected.focus_revision
    );
    assert_eq!(
        played.status.native_expected.slot_generation,
        played.committed.native_expected.slot_generation
    );
    assert_eq!(
        state
            .lock()
            .unwrap()
            .requests
            .iter()
            .filter(|r| r["op"]["name"] == "play")
            .count(),
        1
    );
    assert!(
        driver.inputs[0].is_none(),
        "off-frame receipt is not native admission"
    );
}

#[test]
fn owner_status_cannot_retarget_play_committed_source_or_slot_revision() {
    let _serial = plx_base::testlock::serial();
    let cases: [fn(&mut service::StatusResponse); 3] = [
        |status| status.native_expected.source_revision = "later_foreign_source".into(),
        |status| status.native_expected.slot_generation = "later_foreign_generation".into(),
        |status| status.slots[0].source_revision += 1,
    ];
    for (index, mutate) in cases.into_iter().enumerate() {
        let (mut driver, mut app, mut runtime, state) = setup();
        driver.metadata_fixture = Some(metadata_fixture);
        layout(
            &mut driver,
            &mut runtime,
            &mut app,
            90 + index as u64 * 3,
            "grid4",
        );
        state.lock().unwrap().post_play_status = Some(mutate);
        let inv = play_fixture(&mut driver, &mut runtime, &mut app, 91 + index as u64 * 3);
        assert_eq!(
            runtime.remote.response_for_id_for_test(&inv.id).unwrap()["status"],
            "rejected"
        );
        assert_eq!(runtime.multiview.admitted_pipeline_slots(), 0);
        assert!(driver.inputs.iter().all(Option::is_none));
        assert!(driver.session.is_none());
        let state = state.lock().unwrap();
        assert_eq!(
            state.status.as_ref().unwrap().state,
            service::SessionState::Closed
        );
        assert_eq!(
            state
                .requests
                .iter()
                .filter(|r| r["op"]["name"] == "off")
                .count(),
            1
        );
        assert_eq!(
            state
                .requests
                .iter()
                .filter(|r| r["op"]["name"] == "play")
                .count(),
            1
        );
    }
}

#[test]
fn rejected_model_admission_after_actual_attachment_cannot_publish_session_or_input() {
    let _serial = plx_base::testlock::serial();
    let (mut driver, mut app, mut runtime, state) = setup();
    driver.metadata_fixture = Some(metadata_fixture);
    layout(&mut driver, &mut runtime, &mut app, 110, "grid4");
    let inv = start_play_fixture(&mut driver, &mut runtime, &mut app, 111);
    let end = Instant::now() + Duration::from_secs(5);
    // Drive metadata only; hold main-thread publication once real Play work starts.
    while !matches!(
        driver.pending.as_ref().map(|p| &p.kind),
        Some(Kind::Play { .. })
    ) {
        assert!(Instant::now() < end);
        driver.tick(&mut runtime, &mut app);
        std::thread::sleep(Duration::from_millis(1));
    }
    while state.lock().unwrap().status.as_ref().unwrap().slots[0].state
        != service::SlotState::Playing
    {
        assert!(Instant::now() < end);
        std::thread::sleep(Duration::from_millis(1));
    }
    let ticket = match &driver.pending.as_ref().unwrap().kind {
        Kind::Play { ticket, .. } => *ticket,
        _ => unreachable!(),
    };
    // A terminal earlier model verdict is a valid stale-completion transition;
    // rebinding a Resolved slot would correctly fail SlotBusy instead.
    runtime
        .multiview
        .backend_start_completed(ticket, AdmissionOutcome::Rejected)
        .unwrap();
    assert!(matches!(
        runtime.multiview.slot_status(SlotId::SLOT_0),
        Ok(plx_data::ott::multiview::SlotStatus::Failed { .. })
    ));
    while driver.pending.is_some() {
        assert!(Instant::now() < end);
        driver.tick(&mut runtime, &mut app);
        std::thread::sleep(Duration::from_millis(1));
    }
    assert!(
        driver.session.is_none(),
        "rejected ticket cannot publish returned service status"
    );
    assert_eq!(runtime.multiview.admitted_pipeline_slots(), 0);
    assert!(
        driver.inputs[0].is_some(),
        "attached source remains in cleanup custody only"
    );
    assert!(driver.take_ready_output(&runtime).is_none());
    assert_eq!(
        runtime.remote.response_for_id_for_test(&inv.id).unwrap()["status"],
        "rejected"
    );
    drive(&mut driver, &mut runtime, &mut app);
    assert!(driver.inputs[0].is_none());
    assert!(driver.shutdown_confirmed());
}

#[test]
fn rejected_play_ack_never_becomes_success_via_status_read() {
    let _serial = plx_base::testlock::serial();
    let (mut driver, mut app, mut runtime, state) = setup();
    driver.metadata_fixture = Some(metadata_fixture);
    layout(&mut driver, &mut runtime, &mut app, 120, "grid4");
    state.lock().unwrap().reject_play = true;
    let inv = play_fixture(&mut driver, &mut runtime, &mut app, 121);
    assert_eq!(
        runtime.remote.response_for_id_for_test(&inv.id).unwrap()["status"],
        "rejected"
    );
    assert_eq!(runtime.multiview.admitted_pipeline_slots(), 0);
    assert!(driver.inputs.iter().all(Option::is_none));
    assert!(driver.session.is_none());
    assert_eq!(
        state.lock().unwrap().status.as_ref().unwrap().state,
        service::SessionState::Closed
    );
}

#[test]
fn whole_status_staging_rejects_incomparable_bundles_and_invalidates_changed_foreign_slot_pair() {
    let _serial = plx_base::testlock::serial();
    let (mut driver, mut app, mut runtime, _) = setup();
    layout(&mut driver, &mut runtime, &mut app, 130, "grid4");
    let row = runtime
        .multiview
        .scene_status(&runtime)
        .unwrap()
        .slots
        .remove(1);
    let mut p = parameters(&runtime, "sources");
    p["slot_id"] = json!(1);
    p["slot_generation"] = json!(row.slot_generation);
    p["source_revision"] = json!(row.source_revision);
    let inv = invocation(&mut runtime, 131, p);
    assert!(driver.command(&mut runtime, &app, &inv).is_none());
    drive(&mut driver, &mut runtime, &mut app);
    let session = driver.session.as_mut().unwrap();
    assert!(
        session.bindings[1].is_some(),
        "real Sources response enrolled slot1"
    );
    let before = session.bindings[1].clone().unwrap();
    let mut changed = session.status.clone();
    changed.slots[1].source_revision += 1;
    changed.slots[1].source_ref = Some("foreign_source".into());
    changed.slots[1].item_ref = Some("foreign_item".into());
    let staged = stage_status(session, &changed).unwrap();
    assert!(
        staged[1].is_none(),
        "new backend source revision cannot inherit old native source identity"
    );
    assert_eq!(
        session.bindings[1].as_ref(),
        Some(&before),
        "staging is nonmutating"
    );
    session.status.backend_expected.focus_revision += 1;
    session.status.native_expected.expected.focus_revision = "observed_focus_2".into();
    let mut delayed = session.status.clone();
    delayed.backend_expected.focus_revision -= 1;
    delayed.native_expected.expected.focus_revision = "old_focus_1".into();
    delayed.backend_expected.audio_revision += 1;
    delayed.native_expected.expected.audio_revision = "new_audio_2".into();
    assert!(
        stage_status(session, &delayed).is_err(),
        "cannot build a max-per-field tuple from incomparable snapshots"
    );
    assert_eq!(
        session.status.native_expected.expected.focus_revision,
        "observed_focus_2"
    );
    assert_ne!(
        session.status.native_expected.expected.audio_revision,
        "new_audio_2"
    );
}

fn finish_current_worker(driver: &mut Driver, runtime: &mut Runtime, app: &mut App) {
    let end = Instant::now() + Duration::from_secs(5);
    while driver.pending.is_some() {
        assert!(Instant::now() < end);
        driver.tick(runtime, app);
        std::thread::sleep(Duration::from_millis(1));
    }
}
fn preparing_play_fixture(id: u64) -> (Driver, App, Runtime, Arc<Mutex<FixtureState>>) {
    let (mut driver, mut app, mut runtime, state) = setup();
    driver.metadata_fixture = Some(metadata_fixture);
    layout(&mut driver, &mut runtime, &mut app, id, "grid4");
    state.lock().unwrap().post_play_status = Some(|status| {
        status.slots[0].state = service::SlotState::Preparing;
        status.slots[0].frames = 0;
    });
    let inv = play_fixture(&mut driver, &mut runtime, &mut app, id + 1);
    assert_eq!(
        runtime.remote.response_for_id_for_test(&inv.id).unwrap()["status"],
        "ok"
    );
    assert!(driver.inputs[0].is_some());
    assert!(matches!(
        runtime.multiview.slot_status(SlotId::SLOT_0),
        Ok(plx_data::ott::multiview::SlotStatus::Resolved { .. })
    ));
    assert_eq!(runtime.multiview.admitted_pipeline_slots(), 0);
    (driver, app, runtime, state)
}
fn begin_poll(driver: &mut Driver, runtime: &mut Runtime, app: &mut App) {
    assert!(runtime.multiview_effects.is_empty());
    driver.last_poll = Instant::now() - Duration::from_secs(1);
    driver.tick(runtime, app);
    assert!(matches!(
        driver.pending.as_ref().map(|p| &p.kind),
        Some(Kind::Poll)
    ));
}
#[test]
fn pure_poll_cannot_advance_response_target_source_pair_even_with_identical_refs() {
    let _serial = plx_base::testlock::serial();
    let (mut driver, mut app, mut runtime, state) = preparing_play_fixture(140);
    state.lock().unwrap().post_play_status = Some(|status| status.slots[0].source_revision += 1);
    begin_poll(&mut driver, &mut runtime, &mut app);
    finish_current_worker(&mut driver, &mut runtime, &mut app);
    assert_eq!(
        runtime.multiview.admitted_pipeline_slots(),
        0,
        "uncommitted target revision must not get a Playing ACK"
    );
    assert!(matches!(
        runtime.multiview.slot_status(SlotId::SLOT_0),
        Ok(plx_data::ott::multiview::SlotStatus::Resolved { .. })
    ));
    assert!(driver.session.is_none());
    assert!(
        driver.inputs[0].is_some(),
        "retirement still owns source custody"
    );
    drive(&mut driver, &mut runtime, &mut app);
    assert_eq!(
        state.lock().unwrap().status.as_ref().unwrap().state,
        service::SessionState::Closed
    );
}
#[test]
fn pure_poll_cannot_admit_unwitnessed_sibling_revision_even_with_identical_refs() {
    let _serial = plx_base::testlock::serial();
    let (mut driver, mut app, mut runtime, state) = preparing_play_fixture(150);
    // This regression isolates source-pair publication. Explicitly synchronize
    // the operator's audio policy before Focus through the real command lane.
    let mut muted = parameters(&runtime, "audio");
    muted["policy"] = json!("muted");
    let muted = invocation(&mut runtime, 154, muted);
    assert_eq!(
        driver.command(&mut runtime, &app, &muted).unwrap()["status"],
        "ok"
    );
    driver.tick(&mut runtime, &mut app);
    drive(&mut driver, &mut runtime, &mut app);
    assert!(driver.session.is_some());
    let row = runtime
        .multiview
        .scene_status(&runtime)
        .unwrap()
        .slots
        .remove(1);
    let mut p = parameters(&runtime, "focus");
    p["slot_id"] = json!(1);
    p["slot_generation"] = json!(row.slot_generation);
    p["source_revision"] = json!(row.source_revision);
    let focus = invocation(&mut runtime, 153, p);
    assert_eq!(
        driver.command(&mut runtime, &app, &focus).unwrap()["status"],
        "ok"
    );
    driver.tick(&mut runtime, &mut app);
    drive(&mut driver, &mut runtime, &mut app);
    assert_eq!(
        driver
            .session
            .as_ref()
            .unwrap()
            .status
            .native_expected
            .slot_id,
        1
    );
    assert_eq!(runtime.multiview.admitted_pipeline_slots(), 0);
    state.lock().unwrap().post_play_status = Some(|status| status.slots[0].source_revision += 1);
    begin_poll(&mut driver, &mut runtime, &mut app);
    finish_current_worker(&mut driver, &mut runtime, &mut app);
    assert_eq!(
        runtime.multiview.admitted_pipeline_slots(),
        0,
        "missing staged sibling binding must block all admission ACKs"
    );
    assert!(matches!(
        runtime.multiview.slot_status(SlotId::SLOT_0),
        Ok(plx_data::ott::multiview::SlotStatus::Resolved { .. })
    ));
    assert!(driver.inputs[0].is_some());
    drive(&mut driver, &mut runtime, &mut app);
}
#[test]
fn pure_poll_matching_witnessed_source_pair_can_confirm_first_frames() {
    let _serial = plx_base::testlock::serial();
    let (mut driver, mut app, mut runtime, _) = preparing_play_fixture(160);
    begin_poll(&mut driver, &mut runtime, &mut app);
    finish_current_worker(&mut driver, &mut runtime, &mut app);
    assert_eq!(runtime.multiview.admitted_pipeline_slots(), 1);
    assert!(matches!(
        runtime.multiview.slot_status(SlotId::SLOT_0),
        Ok(plx_data::ott::multiview::SlotStatus::Playing { .. })
    ));
    assert!(driver.session.is_some());
    assert!(!driver.retiring);
}
#[test]
fn played_unknown_active_sibling_blocks_target_ack_before_any_model_publication() {
    let _serial = plx_base::testlock::serial();
    let (mut driver, mut app, mut runtime, state) = setup();
    driver.metadata_fixture = Some(metadata_fixture);
    layout(&mut driver, &mut runtime, &mut app, 170, "grid4");
    state.lock().unwrap().post_play_status = Some(|status| {
        status.slots[1].state = service::SlotState::Playing;
        status.slots[1].frames = 1;
        status.slots[1].source_revision = 1;
        status.slots[1].source_ref = Some("unknown_sibling".into());
        status.slots[1].item_ref = Some("unknown_item".into());
    });
    let inv = start_play_fixture(&mut driver, &mut runtime, &mut app, 171);
    finish_current_worker(&mut driver, &mut runtime, &mut app);
    assert_eq!(
        runtime.remote.response_for_id_for_test(&inv.id).unwrap()["status"],
        "rejected"
    );
    assert_eq!(runtime.multiview.admitted_pipeline_slots(), 0);
    assert!(matches!(
        runtime.multiview.slot_status(SlotId::SLOT_0),
        Ok(plx_data::ott::multiview::SlotStatus::Resolved { .. })
    ));
    assert!(driver.session.is_none());
    assert!(driver.inputs[0].is_some());
    drive(&mut driver, &mut runtime, &mut app);
}
#[test]
fn shutdown_replaces_whole_queued_cleanup_with_late_committed_play_receipt() {
    let _serial = plx_base::testlock::serial();
    let (mut driver, mut app, mut runtime, state) = setup();
    driver.metadata_fixture = Some(metadata_fixture);
    layout(&mut driver, &mut runtime, &mut app, 180, "grid4");
    let hold = Arc::new((AtomicBool::new(false), AtomicBool::new(false)));
    state.lock().unwrap().hold_play_reply = Some(hold.clone());
    let _inv = start_play_fixture(&mut driver, &mut runtime, &mut app, 181);
    let end = Instant::now() + Duration::from_secs(5);
    while !matches!(
        driver.pending.as_ref().map(|p| &p.kind),
        Some(Kind::Play { .. })
    ) {
        assert!(Instant::now() < end);
        driver.tick(&mut runtime, &mut app);
        std::thread::sleep(Duration::from_millis(1));
    }
    while !hold.0.load(Ordering::Acquire) {
        assert!(Instant::now() < end);
        std::thread::sleep(Duration::from_millis(1));
    }
    driver.begin_shutdown(&mut runtime, &mut app);
    assert_eq!(
        driver.cleanup.front().unwrap().status.slots[0].source_revision,
        0
    );
    hold.1.store(true, Ordering::Release);
    while driver.pending.is_some() {
        assert!(Instant::now() < end);
        driver.tick_shutdown(&mut runtime, &mut app);
        std::thread::sleep(Duration::from_millis(1));
    }
    let queued = &driver.cleanup.front().unwrap().status;
    assert_eq!(queued.slots[0].source_revision, 1);
    assert_eq!(
        queued.native_expected.source_revision,
        native_expected(&runtime, SlotId::SLOT_0)
            .unwrap()
            .source_revision
    );
    assert!(driver.inputs[0].is_some());
    while driver.shutdown_pending() {
        assert!(Instant::now() < end);
        driver.tick_shutdown(&mut runtime, &mut app);
        std::thread::sleep(Duration::from_millis(1));
    }
    assert!(driver.shutdown_confirmed());
    let state = state.lock().unwrap();
    let off: Vec<_> = state
        .requests
        .iter()
        .filter(|request| request["op"]["name"] == "off")
        .collect();
    assert_eq!(off.len(), 1);
    assert_eq!(off[0]["context"]["backend_slot_revision"], 1);
    assert_eq!(
        state.status.as_ref().unwrap().state,
        service::SessionState::Closed
    );
}

#[test]
fn old_closed_reply_cannot_settle_new_coordinator_or_erase_its_queued_effects() {
    let _serial = plx_base::testlock::serial();
    let (mut driver, mut app, mut runtime, state) = setup();
    layout(&mut driver, &mut runtime, &mut app, 190, "grid4");
    let mut off = parameters(&runtime, "off");
    off["mode"] = json!("restore_previous");
    let invocation = invocation(&mut runtime, 191, off);
    assert!(driver.command(&mut runtime, &app, &invocation).is_none());
    drive(&mut driver, &mut runtime, &mut app);
    let old_closed = state.lock().unwrap().status.as_ref().unwrap().clone();
    assert_eq!(old_closed.state, service::SessionState::Closed);
    layout(&mut driver, &mut runtime, &mut app, 192, "grid4");
    let transition = runtime.multiview.focus(SlotId::new(1).unwrap()).unwrap();
    queue_effects(&mut runtime, transition).unwrap();
    let before = runtime.multiview.scene_status(&runtime).unwrap();
    let effects = runtime.multiview_effects.len();
    assert!(effects > 0);
    assert_ne!(before.scene, old_closed.native_expected.expected.scene);
    settle_closed_scene(&mut runtime, &old_closed);
    assert_eq!(
        runtime.multiview.scene_status(&runtime).as_ref(),
        Some(&before)
    );
    assert_eq!(runtime.multiview_effects.len(), effects);
    let mut wrong_runtime = driver.session.as_ref().unwrap().status.clone();
    wrong_runtime.state = service::SessionState::Closed;
    wrong_runtime.native_expected.runtime = "foreign_runtime".into();
    settle_closed_scene(&mut runtime, &wrong_runtime);
    assert_eq!(
        runtime.multiview.scene_status(&runtime).as_ref(),
        Some(&before)
    );
    assert_eq!(runtime.multiview_effects.len(), effects);
    let mut matching = driver.session.as_ref().unwrap().status.clone();
    matching.state = service::SessionState::Closed;
    assert_ne!(
        matching.native_expected.expected.focus_revision, before.focus_revision,
        "mutable focus revisions are deliberately irrelevant to scene closure"
    );
    settle_closed_scene(&mut runtime, &matching);
    assert!(runtime.multiview.scene_status(&runtime).is_none());
    assert!(runtime.multiview_effects.is_empty());
}

#[test]
fn matching_closed_settles_retired_snapshot_without_reviving_source_admission() {
    let _serial = plx_base::testlock::serial();
    let (mut driver, mut app, mut runtime, _) = setup();
    layout(&mut driver, &mut runtime, &mut app, 200, "grid4");
    let mut closed = driver.session.as_ref().unwrap().status.clone();
    closed.state = service::SessionState::Closed;
    runtime.multiview_authority_epoch += 1;
    assert!(runtime.multiview.scene_status(&runtime).is_none());
    assert!(runtime
        .multiview
        .validate_active_owner(&runtime, &app)
        .is_err());
    assert!(runtime.multiview.matches_scene_owner(
        &closed.native_expected.runtime,
        &closed.native_expected.expected.scene
    ));
    settle_closed_scene(&mut runtime, &closed);
    assert!(!runtime.multiview.matches_scene_owner(
        &closed.native_expected.runtime,
        &closed.native_expected.expected.scene
    ));
    assert!(runtime.multiview.scene_status(&runtime).is_none());
    let mut coordinator = std::mem::take(&mut runtime.multiview);
    assert!(
        coordinator.activate(&runtime, &app, Layout::Grid4).is_ok(),
        "confirmed old cleanup releases scene for a newly authorized snapshot"
    );
    runtime.multiview = coordinator;
    assert!(runtime
        .multiview
        .validate_active_owner(&runtime, &app)
        .is_ok());
    assert!(!runtime.multiview.matches_scene_owner(
        &closed.native_expected.runtime,
        &closed.native_expected.expected.scene
    ));
}

#[test]
fn retaining_previous_canvas_requires_every_old_source_receipt_to_remain_current() {
    let _serial = plx_base::testlock::serial();
    let (driver, _app, mut runtime, _) = preparing_play_fixture(210);
    let previous = driver.session.as_ref().unwrap().status.clone();
    assert!(driver.retained_output_sources_current(&runtime, &previous));
    let mut wrong_revision = previous.clone();
    wrong_revision.slots[0].source_revision += 1;
    assert!(!driver.retained_output_sources_current(&runtime, &wrong_revision));
    let mut wrong_source = previous.clone();
    wrong_source.slots[0].source_ref = Some("removed_source".into());
    assert!(!driver.retained_output_sources_current(&runtime, &wrong_source));
    driver.inputs[0]
        .as_ref()
        .unwrap()
        .cancellation()
        .store(true, Ordering::Release);
    assert!(
        !driver.retained_output_sources_current(&runtime, &previous),
        "revoked input cannot remain behind a replacement inspector"
    );
    // Policy/config/favorites epoch invalidation is an independent immediate fence.
    runtime.multiview_authority_epoch += 1;
    assert!(!driver.retained_output_sources_current(&runtime, &previous));
}

#[test]
fn fresh_layout_bootstraps_follow_focus_before_immediate_controls_or_ui_output() {
    let _serial = plx_base::testlock::serial();
    let (mut driver, mut app, mut runtime, state) = setup();
    let mut p = parameters(&runtime, "layout");
    p["layout"] = json!("grid4");
    let create = invocation(&mut runtime, 220, p);
    assert!(driver.command(&mut runtime, &app, &create).is_none());
    drive(&mut driver, &mut runtime, &mut app);
    assert!(driver.initial_audio_pending);
    assert_eq!(
        driver.session.as_ref().unwrap().status.audio.policy,
        service::AudioPolicy::Muted
    );
    assert!(driver.take_ready_output(&runtime).is_none());
    assert!(driver.admitted_geometry(&runtime).is_none());
    let before = runtime.multiview.scene_status(&runtime).unwrap();
    let row = before.slots[1].clone();
    let mut p = parameters(&runtime, "focus");
    p["slot_id"] = json!(1);
    p["slot_generation"] = json!(row.slot_generation);
    p["source_revision"] = json!(row.source_revision);
    let early_focus = invocation(&mut runtime, 221, p);
    assert_eq!(
        driver.command(&mut runtime, &app, &early_focus).unwrap()["data"]["reason"],
        "busy"
    );
    let mut p = parameters(&runtime, "audio");
    p["policy"] = json!("muted");
    let early_audio = invocation(&mut runtime, 222, p);
    assert_eq!(
        driver.command(&mut runtime, &app, &early_audio).unwrap()["data"]["reason"],
        "busy"
    );
    assert_eq!(
        runtime.multiview.scene_status(&runtime).as_ref(),
        Some(&before)
    );
    assert!(state
        .lock()
        .unwrap()
        .requests
        .iter()
        .all(|r| r["op"]["name"] != "focus" && r["op"]["name"] != "audio"));
    driver.tick(&mut runtime, &mut app);
    drive(&mut driver, &mut runtime, &mut app);
    assert!(!driver.initial_audio_pending);
    assert_eq!(driver.audio_mode, service::AudioPolicy::FollowsFocus);
    assert_eq!(
        driver.session.as_ref().unwrap().status.audio.policy,
        service::AudioPolicy::FollowsFocus
    );
    assert_eq!(
        runtime.multiview.audio_route_state().applied,
        None,
        "policy configuration is not evidence of audible video in empty slates"
    );
    let mut p = parameters(&runtime, "focus");
    p["slot_id"] = json!(1);
    p["slot_generation"] = json!(before.slots[1].slot_generation);
    p["source_revision"] = json!(before.slots[1].source_revision);
    let focus = invocation(&mut runtime, 223, p);
    assert_eq!(
        driver.command(&mut runtime, &app, &focus).unwrap()["status"],
        "ok"
    );
    driver.tick(&mut runtime, &mut app);
    drive(&mut driver, &mut runtime, &mut app);
    let end = Instant::now() + Duration::from_secs(5);
    while !runtime.multiview_effects.is_empty() {
        assert!(Instant::now() < end);
        driver.tick(&mut runtime, &mut app);
        drive(&mut driver, &mut runtime, &mut app);
    }
    let session = driver
        .session
        .as_ref()
        .expect("default focus must not retire the synchronized scene");
    assert_eq!(session.status.native_expected.slot_id, 1);
    assert_eq!(session.status.backend_expected.focus_revision, 2);
    assert_eq!(session.status.backend_expected.audio_revision, 3);
    assert_eq!(
        session.status.native_expected.expected,
        native_expected(&runtime, SlotId::new(1).unwrap())
            .unwrap()
            .expected
    );
    assert_eq!(runtime.multiview.audio_route_state().applied, None);
    let state = state.lock().unwrap();
    let first_audio = state
        .requests
        .iter()
        .find(|r| r["op"]["name"] == "audio")
        .unwrap();
    assert_eq!(first_audio["op"]["policy"], "follows_focus");
    assert!(state
        .status
        .as_ref()
        .unwrap()
        .slots
        .iter()
        .all(|slot| slot.state == service::SlotState::Empty && slot.frames == 0));
    assert!(state.requests.iter().all(|r| r["op"]["name"] != "play"));
}
