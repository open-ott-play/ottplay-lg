use super::*;
use std::time::Instant;
use plx_data::ott::favorites::{Favorite, Locator, Origin as FavoriteOrigin};
use std::sync::{Mutex, OnceLock};

#[derive(Default)]
struct FixtureState {
    capabilities: Option<service::Capabilities>,
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
    accepted_budget_post_control: Option<fn(&str, &mut service::StatusResponse)>,
    accepted_budget_reject_control: bool,
    recovery_reject_all_control: bool,
    accepted_budget_hold_control: Option<Arc<(AtomicBool, AtomicBool)>>,
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
            serde_json::to_value(state.capabilities.clone().unwrap_or_else(capabilities)).unwrap()
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
            if state.recovery_reject_all_control
                || (state.accepted_budget_reject_control
                    && matches!(body["op"]["name"].as_str(), Some("stop" | "layout")))
            {
                return Ok(serde_json::to_vec(&json!({"version":1,"context":body["context"],"accepted":false,
                    "operation_id":"operation_fixture","state":state.status.as_ref().unwrap().state})).unwrap());
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
            if body["op"]["name"] == "stop" {
                let slot = state
                    .status
                    .as_mut()
                    .unwrap()
                    .slots
                    .iter_mut()
                    .find(|slot| slot.slot_id == context.native_expected.slot_id)
                    .unwrap();
                slot.state = service::SlotState::Closed;
                slot.source_revision += 1;
                slot.source_ref = None;
                slot.item_ref = None;
                context.backend_slot_revision = slot.source_revision;
            }
            fixture_commit(&mut state, context.clone());
            if let Some(mutate) = state.accepted_budget_post_control {
                mutate(
                    body["op"]["name"].as_str().unwrap(),
                    state.status.as_mut().unwrap(),
                );
            }
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
        } else if matches!(body["op"]["name"].as_str(), Some("stop" | "layout" | "off")) {
            state.accepted_budget_hold_control.clone()
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
// Recovery fixtures nest several setup steps on a normal host test thread.
// Keep the two large owners on the heap as each step transfers them.
type DriverFixture = (Driver, Box<App>, Box<Runtime>, Arc<Mutex<FixtureState>>);

fn setup() -> DriverFixture {
    setup_with_capabilities(capabilities())
}
fn setup_with_capabilities(advertised_capabilities: service::Capabilities) -> DriverFixture {
    let mut app = Box::new(crate::app::run::lifecycle_regression_tests::app());
    app.running = true;
    let mut runtime = Box::new(Runtime::unloaded(
        crate::app::ott_companion::Supervisor::inert_for_test(),
    ));
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
    let state = Arc::new(Mutex::new(FixtureState {
        capabilities: Some(advertised_capabilities),
        ..FixtureState::default()
    }));
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
            runtime.frame_time(),
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
fn turn_off(driver: &mut Driver, runtime: &mut Runtime, app: &mut App, id: u64) {
    let mut p = parameters(runtime, "off");
    p["mode"] = json!("restore_previous");
    let inv = invocation(runtime, id, p);
    assert!(driver.command(runtime, app, &inv).is_none());
    drive(driver, runtime, app);
    assert!(runtime.multiview.scene_status(runtime).is_none());
    assert!(driver.session.is_none());
}
fn assert_no_uncommitted_scene(driver: &Driver, runtime: &Runtime) {
    assert!(runtime.multiview.scene_status(runtime).is_none());
    assert!(driver.session.is_none());
    assert!(!driver.initial_audio_pending);
    assert_eq!(runtime.multiview.audio_policy(), None);
    assert_eq!(runtime.multiview.audio_route_state().pending_revision, None);
}
fn occupy_driver_worker_slot(driver: &mut Driver, runtime: &Runtime) {
    let (_sender, receiver) = mpsc::sync_channel(1);
    driver.pending = Some(Box::new(Pending {
        invocation: None,
        request: None,
        fence: driver.fence(runtime),
        receiver,
        thread: None,
        cancel: Arc::new(AtomicBool::new(false)),
        mutating: false,
        kind: Kind::Poll,
        reply: None,
        origin: None,
        sample: None,
        admission_deadline: None,
        output_budget_context: None,
        recovery_context: None,
        stop_completion: None,
    }));
}

#[test]
fn invalid_odd_geometry_rolls_back_before_scene_or_audio_then_valid_layout_and_off_work() {
    let _serial = plx_base::testlock::serial();
    let mut advertised = capabilities();
    advertised.output_profiles[0].canvas.width = 321;
    let (mut driver, mut app, mut runtime, state) = setup_with_capabilities(advertised);
    let mut p = parameters(&runtime, "layout");
    p["layout"] = json!("grid4");
    let rejected = invocation(&mut runtime, 250, p);
    assert_eq!(
        driver.command(&mut runtime, &app, &rejected).unwrap()["data"]["reason"],
        "unsupported"
    );
    assert_no_uncommitted_scene(&driver, &runtime);
    assert!(state
        .lock()
        .unwrap()
        .paths
        .iter()
        .all(|path| !path.ends_with("/sessions")));

    // Single uses the same advertised output profile and is raster-valid for this canvas.
    layout(&mut driver, &mut runtime, &mut app, 251, "single");
    assert!(driver.session.is_some());
    turn_off(&mut driver, &mut runtime, &mut app, 252);
}

#[test]
fn incompatible_output_profile_rolls_back_and_a_supported_layout_can_retry() {
    let _serial = plx_base::testlock::serial();
    let mut advertised = capabilities();
    advertised.output_profiles[0].geometry_modes = vec![service::GeometryMode::Pip];
    let (mut driver, mut app, mut runtime, _) = setup_with_capabilities(advertised);
    let mut p = parameters(&runtime, "layout");
    p["layout"] = json!("grid4");
    let rejected = invocation(&mut runtime, 253, p);
    assert_eq!(
        driver.command(&mut runtime, &app, &rejected).unwrap()["data"]["reason"],
        "unsupported"
    );
    assert_no_uncommitted_scene(&driver, &runtime);

    layout(&mut driver, &mut runtime, &mut app, 254, "pip");
    assert!(driver.session.is_some());
    turn_off(&mut driver, &mut runtime, &mut app, 255);
}

#[test]
fn exhausted_layout_nonce_leaves_no_scene_or_pending_audio() {
    let _serial = plx_base::testlock::serial();
    let (mut driver, app, mut runtime, state) = setup();
    driver.serial = u64::MAX;
    let mut p = parameters(&runtime, "layout");
    p["layout"] = json!("grid4");
    let rejected = invocation(&mut runtime, 256, p);
    assert_eq!(
        driver.command(&mut runtime, &app, &rejected).unwrap()["data"]["reason"],
        "limit_exceeded"
    );
    assert_no_uncommitted_scene(&driver, &runtime);
    assert!(state
        .lock()
        .unwrap()
        .paths
        .iter()
        .all(|path| !path.ends_with("/sessions")));

    // Exhaustion is terminal for this instance; do not reset the serial and risk nonce reuse.
    let mut p = parameters(&runtime, "layout");
    p["layout"] = json!("grid4");
    let again = invocation(&mut runtime, 257, p);
    assert_eq!(
        driver.command(&mut runtime, &app, &again).unwrap()["data"]["reason"],
        "limit_exceeded"
    );
    assert_no_uncommitted_scene(&driver, &runtime);
}

#[test]
fn layout_spawn_busy_rolls_back_its_new_scene_and_audio_bootstrap() {
    let _serial = plx_base::testlock::serial();
    let (mut driver, mut app, mut runtime, _) = setup();
    let mut p = parameters(&runtime, "layout");
    p["layout"] = json!("grid4");
    let inv = invocation(&mut runtime, 258, p);
    let request = wire::Request::decode(inv.multiview_projection().unwrap()).unwrap();

    // Exercise the same production `spawn` refusal reached when an existing off-frame
    // operation occupies the one-worker slot. Direct dispatch keeps this otherwise-guarded
    // branch deterministic without exhausting OS resources or adding a test-only prod switch.
    occupy_driver_worker_slot(&mut driver, &runtime);
    assert!(matches!(
        driver.dispatch(&mut runtime, &app, inv, request),
        Err(wire::Reason::Busy)
    ));
    driver.pending.take();
    assert_no_uncommitted_scene(&driver, &runtime);

    layout(&mut driver, &mut runtime, &mut app, 259, "grid4");
    turn_off(&mut driver, &mut runtime, &mut app, 260);
}

#[test]
fn metadata_worker_spawn_busy_clears_attempt_and_allows_play_retry() {
    let _serial = plx_base::testlock::serial();
    let (mut driver, mut app, mut runtime, _) = setup();
    driver.metadata_fixture = Some(metadata_fixture);
    layout(&mut driver, &mut runtime, &mut app, 262, "grid4");

    let mut p = source_parameters(&runtime, "sources");
    let sources_request = invocation(&mut runtime, 263, p.clone());
    assert!(driver
        .command(&mut runtime, &app, &sources_request)
        .is_none());
    drive(&mut driver, &mut runtime, &mut app);
    let token = runtime
        .remote
        .response_for_id_for_test(&sources_request.id)
        .unwrap()["data"]["items"][0]["ticket"]
        .clone();
    p = source_parameters(&runtime, "play");
    p["source_ticket"] = token;
    let play_request = invocation(&mut runtime, 264, p);
    let request = wire::Request::decode(play_request.multiview_projection().unwrap()).unwrap();
    occupy_driver_worker_slot(&mut driver, &runtime);

    // Match the real error branch in Driver::spawn after begin_play has made its exact ticket.
    // The externally guarded command path reports Busy before dispatch; direct private dispatch
    // proves that any later worker-admission refusal still settles the metadata-only attempt.
    assert!(matches!(
        driver.dispatch(&mut runtime, &app, play_request, request),
        Err(wire::Reason::Busy)
    ));
    driver.pending.take();
    assert_eq!(
        runtime.multiview.scene_status(&runtime).unwrap().slots[0].status,
        "empty"
    );
    assert!(driver.inputs[0].is_none());

    let retry = play_fixture(&mut driver, &mut runtime, &mut app, 265);
    assert_eq!(
        runtime.remote.response_for_id_for_test(&retry.id).unwrap()["status"],
        "ok"
    );
}

#[test]
fn failed_initial_activation_does_not_close_a_preexisting_scene() {
    let _serial = plx_base::testlock::serial();
    let (mut driver, app, mut runtime, _) = setup();
    let mut coordinator = std::mem::take(&mut runtime.multiview);
    coordinator.activate(&runtime, &app, Layout::Grid4).unwrap();
    runtime.multiview = coordinator;
    let before = runtime.multiview.scene_status(&runtime).unwrap();

    let mut p = parameters(&runtime, "layout");
    p["layout"] = json!("pip");
    let inv = invocation(&mut runtime, 261, p);
    assert!(driver.command(&mut runtime, &app, &inv).is_some());
    assert_eq!(
        runtime.multiview.scene_status(&runtime).as_ref(),
        Some(&before)
    );
    assert!(driver.session.is_none());
    assert!(!driver.initial_audio_pending);
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
fn metadata_error_fixture(
    _work: sources::SourceWork,
) -> Result<sources::PreparedMetadata, sources::Error> {
    Err(sources::Error::Resolve)
}

type MetadataErrorGate = Arc<(AtomicBool, AtomicBool)>;
static METADATA_ERROR_GATE: OnceLock<Mutex<Option<MetadataErrorGate>>> = OnceLock::new();
fn gated_metadata_error_fixture(
    _work: sources::SourceWork,
) -> Result<sources::PreparedMetadata, sources::Error> {
    let gate = METADATA_ERROR_GATE
        .get_or_init(|| Mutex::new(None))
        .lock()
        .unwrap()
        .clone()
        .expect("test installs the metadata error gate before starting the worker");
    gate.0.store(true, Ordering::Release);
    let end = Instant::now() + Duration::from_secs(5);
    while !gate.1.load(Ordering::Acquire) {
        assert!(Instant::now() < end, "metadata error gate release deadline");
        std::thread::sleep(Duration::from_millis(1));
    }
    Err(sources::Error::Resolve)
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

#[test]
fn metadata_failure_clears_only_its_attempt_and_allows_a_real_retry() {
    let _serial = plx_base::testlock::serial();
    let (mut driver, mut app, mut runtime, _) = setup();
    driver.metadata_fixture = Some(metadata_error_fixture);
    layout(&mut driver, &mut runtime, &mut app, 230, "grid4");

    let failed = start_play_fixture(&mut driver, &mut runtime, &mut app, 231);
    drive(&mut driver, &mut runtime, &mut app);
    assert_eq!(
        runtime.remote.response_for_id_for_test(&failed.id).unwrap()["status"],
        "rejected"
    );
    assert_eq!(
        runtime.multiview.scene_status(&runtime).unwrap().slots[0].status,
        "empty",
        "metadata failure must retire the exact resolver attempt, not strand Failed"
    );
    assert_eq!(runtime.multiview.admitted_pipeline_slots(), 0);
    assert!(driver.inputs[0].is_none());

    driver.metadata_fixture = Some(metadata_fixture);
    let retry = play_fixture(&mut driver, &mut runtime, &mut app, 233);
    assert_eq!(
        runtime.remote.response_for_id_for_test(&retry.id).unwrap()["status"],
        "ok",
        "the same source remains eligible for a fresh, independently ticketed attempt"
    );
    assert_eq!(
        runtime.multiview.scene_status(&runtime).unwrap().slots[0].status,
        "playing"
    );
    assert!(driver.inputs[0].is_some());
}

#[test]
fn late_metadata_failure_cannot_clear_a_successor_ticket() {
    let _serial = plx_base::testlock::serial();
    let gate: MetadataErrorGate = Arc::new((AtomicBool::new(false), AtomicBool::new(false)));
    *METADATA_ERROR_GATE
        .get_or_init(|| Mutex::new(None))
        .lock()
        .unwrap() = Some(gate.clone());
    let (mut driver, mut app, mut runtime, _) = setup();
    driver.metadata_fixture = Some(gated_metadata_error_fixture);
    layout(&mut driver, &mut runtime, &mut app, 240, "grid4");

    let failed = start_play_fixture(&mut driver, &mut runtime, &mut app, 241);
    let old_ticket = match &driver.pending.as_ref().unwrap().kind {
        Kind::Metadata { ticket, .. } => *ticket,
        _ => panic!("expected metadata worker"),
    };
    let end = Instant::now() + Duration::from_secs(5);
    while !gate.0.load(Ordering::Acquire) {
        assert!(Instant::now() < end, "metadata worker entry deadline");
        std::thread::sleep(Duration::from_millis(1));
    }

    // Retire A and create B through the real coordinator while A's worker is still in flight.
    let source = runtime.multiview.source_rows()[0].handle;
    let mut coordinator = std::mem::take(&mut runtime.multiview);
    coordinator.close_slot(SlotId::SLOT_0).unwrap();
    coordinator.bind_source(SlotId::SLOT_0, source).unwrap();
    let successor = coordinator
        .begin_play(&mut runtime, &app, SlotId::SLOT_0)
        .unwrap();
    let successor_ticket = successor
        .effects
        .iter()
        .find_map(|effect| match effect {
            scene::SceneEffect::PrepareMetadata { ticket, .. } => Some(*ticket),
            _ => None,
        })
        .expect("successor play creates a fresh resolver ticket");
    runtime.multiview = coordinator;
    assert_ne!(old_ticket, successor_ticket);
    assert_eq!(
        runtime.multiview.scene_status(&runtime).unwrap().slots[0].status,
        "starting"
    );

    gate.1.store(true, Ordering::Release);
    drive(&mut driver, &mut runtime, &mut app);
    *METADATA_ERROR_GATE
        .get_or_init(|| Mutex::new(None))
        .lock()
        .unwrap() = None;
    assert_eq!(
        runtime.remote.response_for_id_for_test(&failed.id).unwrap()["status"],
        "rejected"
    );
    assert_eq!(
        runtime.multiview.active_playback(),
        Some((SlotId::SLOT_0, successor_ticket)),
        "the stale failure must not erase the successor's exact live ticket"
    );
    assert_eq!(
        runtime.multiview.scene_status(&runtime).unwrap().slots[0].status,
        "starting"
    );

    // The synthetic successor was intentionally not launched by the driver. Retire it, then
    // prove the public path still admits a new real retry after the stale completion was ignored.
    runtime.multiview.close_slot(SlotId::SLOT_0).unwrap();
    driver.metadata_fixture = Some(metadata_fixture);
    let retry = play_fixture(&mut driver, &mut runtime, &mut app, 243);
    assert_eq!(
        runtime.remote.response_for_id_for_test(&retry.id).unwrap()["status"],
        "ok"
    );
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
fn preparing_play_fixture(id: u64) -> DriverFixture {
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
    driver.last_poll = runtime.frame_time().saturating_sub(Duration::from_secs(1));
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

// Admission-budget regressions exercise the actual Driver, service client,
// coordinator transitions and output queue. No native inspector/load is ticked.
fn accepted_budget_output(status: &mut service::StatusResponse, generation: &str, nonce: char) {
    status.state = service::SessionState::Ready;
    status.output = Some(service::Output {
        session_id: status.session_id.clone(),
        server_epoch: status.server_epoch.clone(),
        output_generation: generation.into(),
        playlist_path: format!(
            "/multiview/v1/media/{}.{}/index.m3u8",
            nonce.to_string().repeat(32),
            "b".repeat(64)
        ),
        lease_expires_in_ms: 60_000,
    });
}
fn accepted_budget_rotate_control(op: &str, status: &mut service::StatusResponse) {
    if matches!(op, "stop" | "layout" | "renew" | "focus" | "audio") {
        let ids = status
            .geometry
            .rects
            .iter()
            .map(|rect| rect.slot_id)
            .collect::<Vec<_>>();
        status.slots.retain(|slot| ids.contains(&slot.slot_id));
        for slot_id in ids {
            if !status.slots.iter().any(|slot| slot.slot_id == slot_id) {
                status.slots.push(service::SlotSnapshot {
                    slot_id,
                    source_revision: 0,
                    state: service::SlotState::Empty,
                    error: None,
                    frames: 0,
                    position_ms: None,
                    duration_ms: None,
                    source_ref: None,
                    item_ref: None,
                });
            }
        }
        accepted_budget_output(status, "accepted_budget_next", 'c');
    }
}
fn accepted_budget_slot_parameters(runtime: &Runtime, operation: &str, slot: usize) -> Value {
    let row = runtime
        .multiview
        .scene_status(runtime)
        .unwrap()
        .slots
        .remove(slot);
    let mut p = parameters(runtime, operation);
    p["slot_id"] = json!(slot);
    p["slot_generation"] = json!(row.slot_generation);
    p["source_revision"] = json!(row.source_revision);
    p
}
fn accepted_budget_play_slot(
    driver: &mut Driver,
    runtime: &mut Runtime,
    app: &mut App,
    slot: usize,
    id: u64,
) {
    let p = accepted_budget_slot_parameters(runtime, "sources", slot);
    let request = invocation(runtime, id, p);
    assert!(driver.command(runtime, app, &request).is_none());
    drive(driver, runtime, app);
    let ticket = runtime
        .remote
        .response_for_id_for_test(&request.id)
        .unwrap()["data"]["items"][0]["ticket"]
        .clone();
    assert!(ticket.is_string());
    let mut p = accepted_budget_slot_parameters(runtime, "play", slot);
    p["source_ticket"] = ticket;
    let play = invocation(runtime, id + 1, p);
    assert!(driver.command(runtime, app, &play).is_none());
    drive(driver, runtime, app);
    assert!(driver.inputs[slot].is_some());
}
fn accepted_budget_setup() -> DriverFixture {
    let (mut driver, mut app, mut runtime, state) = setup();
    driver.metadata_fixture = Some(metadata_fixture);
    layout(&mut driver, &mut runtime, &mut app, 900, "pip");
    accepted_budget_play_slot(&mut driver, &mut runtime, &mut app, 0, 901);
    accepted_budget_play_slot(&mut driver, &mut runtime, &mut app, 1, 903);
    accepted_budget_drain(&mut driver, &mut runtime, &mut app);
    accepted_budget_output(
        &mut driver.session.as_mut().unwrap().status,
        "accepted_budget_original",
        'a',
    );
    driver.session.as_mut().unwrap().lease_deadline = Some(Deadline::after(
        &OffFrame::for_test(),
        Duration::from_secs(60),
    ));
    state.lock().unwrap().status = Some(driver.session.as_ref().unwrap().status.clone());
    assert!(driver.take_ready_output(&runtime).is_some());
    driver.session.as_mut().unwrap().admission_deadline =
        Some(Deadline::after(&OffFrame::for_test(), Duration::ZERO));
    state.lock().unwrap().accepted_budget_post_control = Some(accepted_budget_rotate_control);
    (driver, app, runtime, state)
}
fn accepted_budget_expired(driver: &Driver, _runtime: &Runtime) -> bool {
    driver
        .session
        .as_ref()
        .unwrap()
        .admission_deadline
        .unwrap()
        .remaining(&OffFrame::for_test())
        .is_zero()
}
fn accepted_budget_drain(driver: &mut Driver, runtime: &mut Runtime, app: &mut App) {
    let end = Instant::now() + Duration::from_secs(5);
    while driver.pending.is_some()
        || (!driver.retiring && !runtime.multiview_effects.is_empty())
        || !driver.cleanup.is_empty()
    {
        assert!(Instant::now() < end, "accepted budget effect drain");
        driver.tick(runtime, app);
        std::thread::sleep(Duration::from_millis(1));
    }
}
fn accepted_budget_offer(
    driver: &mut Driver,
    runtime: &mut Runtime,
    app: &mut App,
) -> Result<(), &'static str> {
    // Match the real frame owner before output admission. A setup-time sample
    // must not make a deliberately expired deadline appear unexpired.
    runtime.clock_sample =
        Some(task::time::ClockSample::capture_main(app.adapters.player.mt()).unwrap());
    let proof = driver
        .take_ready_output(runtime)
        .expect("actual Ready output receipt");
    runtime.multiview_driver = std::mem::take(driver);
    let offered = super::super::multiview_output::offer(runtime, app, proof);
    *driver = std::mem::take(&mut runtime.multiview_driver);
    offered
}
#[test]
fn accepted_budget_late_stop_publishes_only_after_ack_and_preserves_playing_sibling() {
    let _serial = plx_base::testlock::serial();
    let (mut driver, mut app, mut runtime, state) = accepted_budget_setup();
    let sibling = driver.session.as_ref().unwrap().status.slots[0].clone();
    let p = accepted_budget_slot_parameters(&runtime, "stop", 1);
    let mut request = invocation(&mut runtime, 910, p);
    request.deadline_ms = runtime.now_ms().saturating_add(60_000);
    assert_eq!(
        driver.command(&mut runtime, &app, &request).unwrap()["status"],
        "ok"
    );
    assert!(
        accepted_budget_expired(&driver, &runtime),
        "queue acceptance cannot publish"
    );
    let captured = driver.accepted_output_budgets.front().unwrap().deadline;
    assert!(captured.remaining_at_sample(runtime.clock_sample.unwrap()) <= BUDGET);
    accepted_budget_drain(&mut driver, &mut runtime, &mut app);
    assert!(!accepted_budget_expired(&driver, &runtime));
    assert_eq!(
        driver
            .session
            .as_ref()
            .unwrap()
            .admission_deadline
            .unwrap()
            .remaining_at_sample(runtime.clock_sample.unwrap()),
        captured.remaining_at_sample(runtime.clock_sample.unwrap())
    );
    assert!(driver.accepted_output_budgets.is_empty());
    let status = &driver.session.as_ref().unwrap().status;
    assert_eq!(status.slots[0].source_revision, sibling.source_revision);
    assert_eq!(status.slots[0].state, service::SlotState::Playing);
    assert_eq!(status.slots[1].state, service::SlotState::Closed);
    assert!(driver.inputs[0].is_some() && driver.inputs[1].is_none());
    assert_eq!(
        status.output.as_ref().unwrap().output_generation,
        "accepted_budget_next"
    );
    let offered = accepted_budget_offer(&mut driver, &mut runtime, &mut app);
    assert!(offered.is_ok(), "post-Stop output offer: {offered:?}");
    assert!(state
        .lock()
        .unwrap()
        .requests
        .iter()
        .any(|body| body["op"]["name"] == "stop"));
}
#[test]
fn accepted_budget_late_existing_layout_refreshes_new_epoch_without_replacing_inputs() {
    let _serial = plx_base::testlock::serial();
    let (mut driver, mut app, mut runtime, _) = accepted_budget_setup();
    let revisions = driver
        .session
        .as_ref()
        .unwrap()
        .status
        .slots
        .iter()
        .take(2)
        .map(|slot| slot.source_revision)
        .collect::<Vec<_>>();
    let mut p = parameters(&runtime, "layout");
    p["layout"] = json!("grid4");
    let request = invocation(&mut runtime, 920, p);
    assert_eq!(
        driver.command(&mut runtime, &app, &request).unwrap()["status"],
        "ok"
    );
    assert!(accepted_budget_expired(&driver, &runtime));
    accepted_budget_drain(&mut driver, &mut runtime, &mut app);
    assert!(!accepted_budget_expired(&driver, &runtime));
    for (slot, revision) in driver
        .session
        .as_ref()
        .unwrap()
        .status
        .slots
        .iter()
        .take(2)
        .zip(revisions)
    {
        assert_eq!(slot.source_revision, revision);
        assert_eq!(slot.state, service::SlotState::Playing);
    }
    assert!(driver.inputs[0].is_some() && driver.inputs[1].is_some());
    assert!(accepted_budget_offer(&mut driver, &mut runtime, &mut app).is_ok());
}
#[test]
fn accepted_budget_status_poll_and_renew_never_admit_a_reported_new_generation() {
    let _serial = plx_base::testlock::serial();
    for renew in [false, true] {
        let (mut driver, mut app, mut runtime, state) = accepted_budget_setup();
        let p = json!({"version":1,"op":"status","runtime":runtime.remote.runtime()});
        let encoded = serde_json::to_vec(&json!({"action":"multiview","params":p})).unwrap();
        assert!(wire::Request::decode(&encoded).is_ok());
        let status = invocation(&mut runtime, 930, p);
        let reply = driver
            .command(&mut runtime, &app, &status)
            .expect("valid status has a synchronous response");
        assert_eq!(reply["status"], "ok");
        assert!(accepted_budget_expired(&driver, &runtime));
        accepted_budget_output(
            state.lock().unwrap().status.as_mut().unwrap(),
            "accepted_budget_unrequested",
            'd',
        );
        if renew {
            driver.last_renew = runtime.frame_time().saturating_sub(Duration::from_secs(21));
        } else {
            driver.last_poll = runtime.frame_time().saturating_sub(Duration::from_secs(1));
        }
        driver.tick(&mut runtime, &mut app);
        drive(&mut driver, &mut runtime, &mut app);
        assert!(accepted_budget_expired(&driver, &runtime));
        assert!(driver.accepted_output_budgets.is_empty());
        assert_eq!(
            accepted_budget_offer(&mut driver, &mut runtime, &mut app),
            Err("Composite output admission expired")
        );
    }
}
#[test]
fn accepted_budget_rejected_control_ack_cannot_publish_or_offer_output() {
    let _serial = plx_base::testlock::serial();
    let (mut driver, mut app, mut runtime, state) = accepted_budget_setup();
    state.lock().unwrap().accepted_budget_reject_control = true;
    let mut p = parameters(&runtime, "layout");
    p["layout"] = json!("grid4");
    let request = invocation(&mut runtime, 940, p);
    assert!(driver.command(&mut runtime, &app, &request).is_some());
    accepted_budget_drain(&mut driver, &mut runtime, &mut app);
    assert!(driver.retiring || driver.uncertain || driver.session.is_none());
    assert!(driver.take_ready_output(&runtime).is_none());
}
#[test]
fn accepted_budget_delayed_ack_keeps_the_original_expired_deadline() {
    let _serial = plx_base::testlock::serial();
    let (mut driver, mut app, mut runtime, state) = accepted_budget_setup();
    let hold = Arc::new((AtomicBool::new(false), AtomicBool::new(false)));
    state.lock().unwrap().accepted_budget_hold_control = Some(hold.clone());
    let mut p = parameters(&runtime, "layout");
    p["layout"] = json!("grid4");
    let request = invocation(&mut runtime, 950, p);
    assert!(driver.command(&mut runtime, &app, &request).is_some());
    driver.tick(&mut runtime, &mut app);
    let until = Instant::now() + Duration::from_secs(5);
    while !hold.0.load(Ordering::Acquire) {
        assert!(Instant::now() < until);
        std::thread::sleep(Duration::from_millis(1));
    }
    assert!(accepted_budget_expired(&driver, &runtime));
    // Deterministically model the captured request deadline expiring while its
    // real service worker is held. Completion must use this stored deadline,
    // never a newly computed now+BUDGET.
    driver.pending.as_mut().unwrap().admission_deadline =
        Some(Deadline::after(&OffFrame::for_test(), Duration::ZERO));
    hold.1.store(true, Ordering::Release);
    accepted_budget_drain(&mut driver, &mut runtime, &mut app);
    assert!(accepted_budget_expired(&driver, &runtime));
    assert_eq!(
        accepted_budget_offer(&mut driver, &mut runtime, &mut app),
        Err("Composite output admission expired")
    );
}
#[test]
fn accepted_budget_old_scene_and_replay_cannot_refresh_the_budget() {
    let _serial = plx_base::testlock::serial();
    let (mut driver, mut app, mut runtime, _) = accepted_budget_setup();
    let mut stale = parameters(&runtime, "layout");
    stale["layout"] = json!("grid4");
    stale["expected"]["scene"] = json!("retired_scene");
    let rejected = invocation(&mut runtime, 960, stale);
    assert_eq!(
        driver.command(&mut runtime, &app, &rejected).unwrap()["status"],
        "rejected"
    );
    assert!(
        accepted_budget_expired(&driver, &runtime) && driver.accepted_output_budgets.is_empty()
    );
    let mut p = parameters(&runtime, "layout");
    p["layout"] = json!("grid4");
    let request = invocation(&mut runtime, 961, p);
    assert!(driver.command(&mut runtime, &app, &request).is_some());
    accepted_budget_drain(&mut driver, &mut runtime, &mut app);
    driver.session.as_mut().unwrap().admission_deadline =
        Some(Deadline::after(&OffFrame::for_test(), Duration::ZERO));
    assert_eq!(
        driver.command(&mut runtime, &app, &request).unwrap()["status"],
        "rejected"
    );
    assert!(
        accepted_budget_expired(&driver, &runtime) && driver.accepted_output_budgets.is_empty()
    );
}
#[test]
fn accepted_budget_focus_audio_and_expired_invocation_never_refresh() {
    let _serial = plx_base::testlock::serial();
    for focus in [false, true] {
        let (mut driver, mut app, mut runtime, _) = accepted_budget_setup();
        let p = if focus {
            accepted_budget_slot_parameters(&runtime, "focus", 1)
        } else {
            let mut p = parameters(&runtime, "audio");
            p["policy"] = json!("muted");
            p
        };
        let request = invocation(&mut runtime, 970, p);
        assert_eq!(
            driver.command(&mut runtime, &app, &request).unwrap()["status"],
            "ok"
        );
        accepted_budget_drain(&mut driver, &mut runtime, &mut app);
        assert!(
            accepted_budget_expired(&driver, &runtime) && driver.accepted_output_budgets.is_empty()
        );
        assert_eq!(
            accepted_budget_offer(&mut driver, &mut runtime, &mut app),
            Err("Composite output admission expired")
        );
        let mut p = parameters(&runtime, "layout");
        p["layout"] = json!("grid4");
        let mut expired = invocation(&mut runtime, 971, p);
        expired.deadline_ms = 0;
        assert_eq!(
            driver.command(&mut runtime, &app, &expired).unwrap()["status"],
            "rejected"
        );
        assert!(
            accepted_budget_expired(&driver, &runtime) && driver.accepted_output_budgets.is_empty()
        );
    }
}
#[test]
fn accepted_budget_changed_scene_while_ack_pending_cannot_publish() {
    let _serial = plx_base::testlock::serial();
    let (mut driver, mut app, mut runtime, state) = accepted_budget_setup();
    let hold = Arc::new((AtomicBool::new(false), AtomicBool::new(false)));
    state.lock().unwrap().accepted_budget_hold_control = Some(hold.clone());
    let mut p = parameters(&runtime, "layout");
    p["layout"] = json!("grid4");
    let request = invocation(&mut runtime, 980, p);
    assert!(driver.command(&mut runtime, &app, &request).is_some());
    driver.tick(&mut runtime, &mut app);
    let until = Instant::now() + Duration::from_secs(5);
    while !hold.0.load(Ordering::Acquire) {
        assert!(Instant::now() < until);
        std::thread::sleep(Duration::from_millis(1));
    }
    runtime.multiview_authority_epoch += 1;
    hold.1.store(true, Ordering::Release);
    accepted_budget_drain(&mut driver, &mut runtime, &mut app);
    assert!(driver.retiring || driver.session.is_none());
    assert!(driver.take_ready_output(&runtime).is_none());
}

#[test]
fn recovery_rejected_control_ack_never_becomes_successful_status_for_any_operation() {
    let _serial = plx_base::testlock::serial();
    let (driver, _app, runtime, state) = accepted_budget_setup();
    let session = driver.session.as_ref().unwrap();
    let prior = binding(&runtime, session, SlotId::SLOT_0).unwrap();
    let worker = OffFrame::for_test();
    state.lock().unwrap().recovery_reject_all_control = true;
    for (i, operation) in [
        service::ControlOp::Focus {},
        service::ControlOp::Audio {
            policy: service::AudioPolicy::Muted,
        },
        service::ControlOp::Renew {},
        service::ControlOp::Off {},
        service::ControlOp::Stop {},
        service::ControlOp::Layout {
            geometry: session.status.geometry.clone(),
        },
        service::ControlOp::Play {
            input_grant: "grant_fixture".into(),
        },
    ]
    .into_iter()
    .enumerate()
    {
        let before = state.lock().unwrap().paths.len();
        let next = if matches!(&operation, service::ControlOp::Focus {}) {
            let mut next = prior.native_expected.clone();
            next.expected.focus_revision = "not_committed".into();
            Some(next)
        } else {
            None
        };
        let result = session.io.control(
            &worker,
            prior.clone(),
            operation,
            next,
            service::RequestId::parse(&format!("{:032x}", 4000 + i)).unwrap(),
            Deadline::after(&worker, Duration::from_secs(1)),
            &AtomicBool::new(false),
        );
        assert!(
            matches!(result, Err(service::ServiceError::RevisionConflict)),
            "operation {i}"
        );
        let state = state.lock().unwrap();
        assert_eq!(
            state.paths.len(),
            before + 1,
            "rejected ACK must not trigger a status read"
        );
        assert!(state.paths.last().unwrap().ends_with("/control"));
        assert_eq!(
            state.requests.last().unwrap()["context"],
            serde_json::to_value(&prior).unwrap()
        );
        assert_ne!(
            state.requests.last().unwrap()["next_native"],
            serde_json::to_value(&prior.native_expected).unwrap()
        );
    }
}

fn recovery_fixture() -> DriverFixture {
    let (mut driver, mut app, mut runtime, state) = accepted_budget_setup();
    runtime.multiview_connection.refresh();
    driver.tick(&mut runtime, &mut app);
    assert!(driver.recovery_only);
    assert!(driver.session.is_some());
    assert!(driver.accepted_output_budgets.is_empty());
    (driver, app, runtime, state)
}

#[test]
fn recovery_retains_original_client_but_offers_no_output_or_admission() {
    let _serial = plx_base::testlock::serial();
    let (mut driver, mut app, mut runtime, state) = recovery_fixture();
    let original = driver.session.as_ref().unwrap().client.clone();
    let paths = state.lock().unwrap().paths.len();
    let (admission, recovery) = driver.capability_markers(&runtime, &app);
    assert_eq!(
        admission,
        json!({"version":1,"operations":["status"],"layouts":[],"max_slots":0,
        "source_kinds":[],"audio_policies":[],"off_modes":[]})
    );
    assert_eq!(
        recovery,
        json!({"version":1,"runtime":runtime.remote.runtime(),
        "operations":["status","stop","off"],"off_modes":["close_only"]})
    );
    for _ in 0..3 {
        driver.tick(&mut runtime, &mut app);
    }
    assert_eq!(state.lock().unwrap().paths.len(), paths);
    assert!(Arc::ptr_eq(
        &original,
        &driver.session.as_ref().unwrap().client
    ));
    assert!(driver.take_ready_output(&runtime).is_none());
    assert!(driver.accepted_output_budgets.is_empty());
    assert!(accepted_budget_expired(&driver, &runtime));
}

#[test]
fn recovery_stop_exact_ticket_preserves_sibling_and_does_not_mint_output_budget() {
    let _serial = plx_base::testlock::serial();
    let (mut driver, mut app, mut runtime, state) = recovery_fixture();
    let sibling = driver.inputs[1].as_ref().unwrap().ticket;
    let before = state.lock().unwrap().requests.len();
    let mut p = accepted_budget_slot_parameters(&runtime, "stop", 0);
    p["recovery_version"] = json!(1);
    let inv = invocation(&mut runtime, 4100, p);
    assert_eq!(
        driver.command(&mut runtime, &app, &inv).unwrap()["status"],
        "ok"
    );
    accepted_budget_drain(&mut driver, &mut runtime, &mut app);
    assert!(driver.inputs[0].is_none());
    assert!(driver.inputs[1].as_ref().unwrap().ticket == sibling);
    assert!(driver.recovery_effects.is_empty());
    assert!(driver.accepted_output_budgets.is_empty());
    assert!(accepted_budget_expired(&driver, &runtime));
    assert!(driver.take_ready_output(&runtime).is_none());
    let state = state.lock().unwrap();
    let operations: Vec<_> = state.requests[before..]
        .iter()
        .filter_map(|v| v["op"]["name"].as_str())
        .collect();
    assert_eq!(operations, ["stop"]);
}

#[test]
fn recovery_rejects_changed_origin_foreground_and_authority_without_new_io() {
    let _serial = plx_base::testlock::serial();
    for variant in 0..6 {
        let (mut driver, mut app, mut runtime, state) = recovery_fixture();
        let before = state.lock().unwrap().paths.len();
        let mut p = accepted_budget_slot_parameters(&runtime, "stop", 0);
        p["recovery_version"] = json!(1);
        let inv = invocation(&mut runtime, 4200 + variant, p);
        match variant {
            0 => {
                runtime
                    .saved
                    .settings
                    .insert("nativeMultiviewService".into(), json!("different-origin"));
            }
            1 => app.running = false,
            2 => runtime.multiview_authority_epoch += 1,
            3 => runtime.profile_edit_epoch += 1,
            4 => {
                let origin = runtime.saved.favorites.entries()[0].origin.clone();
                let kiosk = crate::app::ott_kiosk_stored::Stored::armed(origin, true).unwrap();
                runtime
                    .saved
                    .settings
                    .insert("nativeKiosk".into(), kiosk.encode().unwrap());
            }
            _ => runtime.persistence_uncertain = true,
        }
        let reply = driver.command(&mut runtime, &app, &inv).unwrap();
        assert_ne!(reply["status"], "ok");
        assert_eq!(state.lock().unwrap().paths.len(), before);
        assert_eq!(
            driver.capability_markers(&runtime, &app).1["operations"],
            json!(["status"])
        );
        assert!(driver.accepted_output_budgets.is_empty());
    }
}

#[test]
fn recovery_close_only_off_closes_original_backend_and_native_scene() {
    let _serial = plx_base::testlock::serial();
    let (mut driver, mut app, mut runtime, state) = recovery_fixture();
    let before = state.lock().unwrap().requests.len();
    let mut p = parameters(&runtime, "off");
    p["mode"] = json!("close_only");
    p["recovery_version"] = json!(1);
    let inv = invocation(&mut runtime, 4300, p);
    assert!(driver.command(&mut runtime, &app, &inv).is_none());
    drive(&mut driver, &mut runtime, &mut app);
    assert!(driver.session.is_none());
    assert!(driver.inputs.iter().all(Option::is_none));
    assert!(runtime.multiview.scene_status(&runtime).is_none());
    assert!(driver.accepted_output_budgets.is_empty());
    let state = state.lock().unwrap();
    assert_eq!(
        state.status.as_ref().unwrap().state,
        service::SessionState::Closed
    );
    let operations: Vec<_> = state.requests[before..]
        .iter()
        .filter_map(|v| v["op"]["name"].as_str())
        .collect();
    assert_eq!(operations, ["off"]);
}

#[test]
fn recovery_wire_focus_audio_and_stale_stop_never_dispatch_or_offer_budget() {
    let _serial = plx_base::testlock::serial();
    let (mut driver, app, mut runtime, state) = recovery_fixture();
    let before = state.lock().unwrap().paths.len();
    for (i, op) in ["focus", "audio"].into_iter().enumerate() {
        let mut p = accepted_budget_slot_parameters(&runtime, op, 0);
        p["recovery_version"] = json!(1);
        if op == "audio" {
            p["policy"] = json!("muted");
        }
        let encoded = serde_json::to_vec(&json!({"action":"multiview","params":p})).unwrap();
        // Recovery focus/audio are outside the closed wire grammar, so they
        // never reach Driver dispatch or produce a structured accepted reply.
        assert!(wire::Request::decode(&encoded).is_err());
        let inv = invocation(&mut runtime, 4400 + i as u64, p);
        assert!(driver.command(&mut runtime, &app, &inv).is_none());
    }
    let mut p = accepted_budget_slot_parameters(&runtime, "stop", 0);
    p["recovery_version"] = json!(1);
    p["slot_generation"] = json!("old_generation");
    let encoded = serde_json::to_vec(&json!({"action":"multiview","params":p})).unwrap();
    assert!(wire::Request::decode(&encoded).is_ok());
    let inv = invocation(&mut runtime, 4402, p);
    let rejected = driver
        .command(&mut runtime, &app, &inv)
        .expect("valid stale Stop must receive its exact CAS rejection");
    assert_eq!(rejected["status"], "rejected");
    assert_eq!(rejected["data"]["reason"], "slot_changed");
    assert_eq!(state.lock().unwrap().paths.len(), before);
    assert!(runtime.multiview_effects.is_empty());
    assert!(driver.recovery_effects.is_empty());
    assert!(driver.accepted_output_budgets.is_empty());
}

#[test]
fn recovery_uncaptured_focus_cannot_borrow_stop_custody() {
    let _serial = plx_base::testlock::serial();
    let (mut driver, _app, mut runtime, state) = recovery_fixture();
    let before = state.lock().unwrap().paths.len();
    let scene = runtime
        .multiview
        .active_playback()
        .expect("active fixture playback")
        .1
        .scene();
    runtime
        .multiview_effects
        .push_back(scene::SceneEffect::Backend(scene::BackendIntent::Focus {
            scene,
            slot: SlotId::SLOT_0,
        }));
    assert!(driver.drain_effect(&mut runtime).is_err());
    assert_eq!(state.lock().unwrap().paths.len(), before);
    assert!(driver.pending.is_none());
    assert!(driver.accepted_output_budgets.is_empty());
}

#[test]
fn recovery_pending_stop_ack_cannot_survive_changed_protected_origin() {
    let _serial = plx_base::testlock::serial();
    let (mut driver, mut app, mut runtime, state) = recovery_fixture();
    let hold = Arc::new((AtomicBool::new(false), AtomicBool::new(false)));
    state.lock().unwrap().accepted_budget_hold_control = Some(hold.clone());
    let mut p = accepted_budget_slot_parameters(&runtime, "stop", 0);
    p["recovery_version"] = json!(1);
    let inv = invocation(&mut runtime, 4500, p);
    assert_eq!(
        driver.command(&mut runtime, &app, &inv).unwrap()["status"],
        "ok"
    );
    let until = Instant::now() + Duration::from_secs(5);
    while !hold.0.load(Ordering::Acquire) {
        assert!(Instant::now() < until);
        driver.tick(&mut runtime, &mut app);
        std::thread::sleep(Duration::from_millis(1));
    }
    runtime
        .saved
        .settings
        .insert("nativeMultiviewService".into(), json!("different-origin"));
    hold.1.store(true, Ordering::Release);
    driver.tick(&mut runtime, &mut app);
    drive(&mut driver, &mut runtime, &mut app);
    assert!(driver.retiring || driver.session.is_none());
    assert!(driver.accepted_output_budgets.is_empty());
    assert!(driver.take_ready_output(&runtime).is_none());
    assert!(state
        .lock()
        .unwrap()
        .requests
        .iter()
        .all(|request| !matches!(request["op"]["name"].as_str(), Some("renew"))));
}

#[test]
fn recovery_capability_projection_is_closed_zero_without_current_qualification() {
    let _serial = plx_base::testlock::serial();
    let (driver, app, mut runtime, _) = setup();
    let (positive, _) = driver.capability_markers(&runtime, &app);
    assert!(positive["max_slots"].as_u64().unwrap() <= 4);
    assert!(positive["max_slots"].as_u64().unwrap() > 0);
    assert_eq!(positive.as_object().unwrap().len(), 7);
    assert_eq!(positive["off_modes"], json!([]));
    assert!(!positive["operations"]
        .as_array()
        .unwrap()
        .contains(&json!("off")));
    runtime.multiview_connection.refresh();
    let (zero, recovery) = driver.capability_markers(&runtime, &app);
    assert_eq!(
        zero,
        json!({"version":1,"operations":["status"],"layouts":[],"max_slots":0,
        "source_kinds":[],"audio_policies":[],"off_modes":[]})
    );
    assert_eq!(recovery.as_object().unwrap().len(), 4);
    assert_eq!(recovery["operations"], json!(["status"]));
    assert_eq!(recovery["off_modes"], json!([]));
}

// Observe the actual worker result before the main-thread completion reducer. All
// negative mutations below are explicit test faults, never substitute successful ACKs.
fn stop_completion_reply(driver: &mut Driver) -> (Pending, Result<Reply, wire::Reason>) {
    let until = Instant::now() + Duration::from_secs(5);
    while driver
        .pending
        .as_ref()
        .unwrap()
        .thread
        .as_ref()
        .is_some_and(|t| !t.is_finished())
    {
        assert!(Instant::now() < until, "Stop worker completion bound");
        std::thread::sleep(Duration::from_millis(1));
    }
    let mut pending = *driver.pending.take().unwrap();
    pending.thread.take().unwrap().join().unwrap();
    let reply = pending
        .receiver
        .try_recv()
        .expect("finished Stop worker reply");
    (pending, reply)
}

fn queue_stop_fixture(
    driver: &mut Driver,
    runtime: &mut Runtime,
    app: &App,
    id: u64,
) -> ResolveTicket {
    let ticket = driver.inputs[1].as_ref().unwrap().ticket;
    let p = accepted_budget_slot_parameters(runtime, "stop", 1);
    let request = invocation(runtime, id, p);
    assert_eq!(
        driver.command(runtime, app, &request).unwrap()["status"],
        "ok"
    );
    ticket
}

#[test]
fn stop_ack_prediction_and_actual_commit_must_both_match_before_publication() {
    let _serial = plx_base::testlock::serial();
    for corrupt_prediction in [false, true] {
        let (mut driver, mut app, mut runtime, state) = accepted_budget_setup();
        let original_client = driver.session.as_ref().unwrap().client.clone();
        let ticket = queue_stop_fixture(&mut driver, &mut runtime, &app, 1200);
        driver.drain_effect(&mut runtime).unwrap();
        let (mut pending, reply) = stop_completion_reply(&mut driver);
        let Ok(Reply::Status(mut status)) = reply else {
            panic!("actual successful Stop reply required");
        };
        let actual_before = native_expected(&runtime, ticket.slot()).unwrap();
        assert_ne!(
            status.native_expected.slot_generation,
            actual_before.slot_generation
        );
        assert_eq!(
            status.native_expected,
            pending.stop_completion.as_ref().unwrap().native
        );
        let bad = "slot-1-generation-invalid".to_owned();
        status.native_expected.slot_generation = bad.clone();
        if corrupt_prediction {
            // Server and pending prediction agree, but the real Scene commit cannot.
            // The coordinator preview is left intact so this reaches the post-commit check.
            pending
                .stop_completion
                .as_mut()
                .unwrap()
                .native
                .slot_generation = bad;
        }
        driver.complete(&mut runtime, &mut app, pending, Ok(Reply::Status(status)));
        assert!(driver.retiring);
        assert!(
            driver.session.is_none(),
            "no status/deadline publication on mismatch"
        );
        assert!(driver.take_ready_output(&runtime).is_none());
        assert!(driver.accepted_output_budgets.is_empty());
        assert!(
            driver.inputs[0].is_some() && driver.inputs[1].is_some(),
            "retain original cleanup custody"
        );
        assert!(!driver.cleanup.is_empty());
        assert!(driver
            .cleanup
            .iter()
            .all(|cleanup| Arc::ptr_eq(&cleanup.origin.client, &original_client)));
        assert_eq!(
            runtime.multiview.slot_status(ticket.slot()).unwrap()
                == plx_data::ott::multiview::SlotStatus::Empty,
            corrupt_prediction,
            "only the post-commit fault may have retired the local slot"
        );
        assert_eq!(
            state
                .lock()
                .unwrap()
                .requests
                .iter()
                .filter(|body| body["op"]["name"] == "stop")
                .count(),
            1
        );
        // Failure never retries the write. Owned original-client cleanup is allowed to run.
        accepted_budget_drain(&mut driver, &mut runtime, &mut app);
    }
}

#[test]
fn stop_pending_scene_or_sibling_change_refuses_completion_without_new_budget() {
    let _serial = plx_base::testlock::serial();
    for change_sibling in [false, true] {
        let (mut driver, mut app, mut runtime, _) = accepted_budget_setup();
        let stopped = queue_stop_fixture(&mut driver, &mut runtime, &app, 1210);
        driver.drain_effect(&mut runtime).unwrap();
        let (pending, reply) = stop_completion_reply(&mut driver);
        if change_sibling {
            // A real sibling completion changes the whole Scene revision while
            // leaving this Stop's target generation and global wire revisions alone.
            let sibling = driver.inputs[0].as_ref().unwrap().ticket;
            runtime.multiview.close_slot(sibling.slot()).unwrap();
            runtime
                .multiview
                .stop_completed(sibling, scene::StopResult::Stopped)
                .unwrap();
        } else {
            runtime.multiview.close_scene().unwrap();
        }
        driver.complete(&mut runtime, &mut app, pending, reply);
        assert!(driver.retiring && driver.session.is_none());
        assert!(driver.inputs[1].is_some());
        assert!(driver.take_ready_output(&runtime).is_none());
        assert!(
            runtime.multiview.slot_status(stopped.slot()).unwrap()
                != plx_data::ott::multiview::SlotStatus::Empty,
            "changed pending context must not consume Stop completion"
        );
        accepted_budget_drain(&mut driver, &mut runtime, &mut app);
    }
}

#[test]
fn stop_rejected_ack_never_completes_and_duplicate_retired_ticket_never_sends_io() {
    let _serial = plx_base::testlock::serial();
    let (mut driver, mut app, mut runtime, state) = accepted_budget_setup();
    state.lock().unwrap().accepted_budget_reject_control = true;
    let ticket = queue_stop_fixture(&mut driver, &mut runtime, &app, 1220);
    driver.drain_effect(&mut runtime).unwrap();
    let (pending, reply) = stop_completion_reply(&mut driver);
    assert!(reply.is_err());
    driver.complete(&mut runtime, &mut app, pending, reply);
    assert!(driver.retiring && driver.session.is_none());
    assert!(driver.inputs[1].is_some());
    assert!(
        runtime.multiview.slot_status(ticket.slot()).unwrap()
            != plx_data::ott::multiview::SlotStatus::Empty
    );
    assert!(driver.take_ready_output(&runtime).is_none());
    accepted_budget_drain(&mut driver, &mut runtime, &mut app);

    let (mut driver, app, mut runtime, state) = accepted_budget_setup();
    let ticket = queue_stop_fixture(&mut driver, &mut runtime, &app, 1221);
    runtime
        .multiview
        .stop_completed(ticket, scene::StopResult::Stopped)
        .unwrap();
    let before = state.lock().unwrap().paths.len();
    driver.drain_effect(&mut runtime).unwrap();
    assert!(driver.pending.is_none());
    assert_eq!(state.lock().unwrap().paths.len(), before);
    assert!(runtime
        .multiview
        .preview_stop_completion(&runtime, ticket)
        .is_err());
}

// Review regressions cross actual original-service cleanup and the output
// adapter's native admission gate. Only inspection and service IO are synthetic.
fn review_qualify(runtime: &mut Runtime) {
    let protected = runtime
        .saved
        .settings
        .get("nativeMultiviewService")
        .cloned()
        .unwrap();
    let until = Instant::now() + Duration::from_secs(5);
    loop {
        runtime.multiview_connection.tick(
            runtime.frame_time(),
            true,
            true,
            Some(&protected),
            runtime.remote.persisted_device_id(),
        );
        if runtime.multiview_connection.qualified().is_some() {
            break;
        }
        assert!(
            Instant::now() < until,
            "fresh real capability client must settle"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
}
fn review_nonclosed_off(op: &str, status: &mut service::StatusResponse) {
    if op == "off" {
        status.state = service::SessionState::Ready;
    } else {
        accepted_budget_rotate_control(op, status);
    }
}
#[test]
fn review_confirmed_recovery_retirement_allows_fresh_layout_but_unconfirmed_stays_fenced() {
    let _serial = plx_base::testlock::serial();
    for confirmed in [false, true] {
        let (mut driver, mut app, mut runtime, state) = recovery_fixture();
        let old_scene = context(&runtime).scene;
        if !confirmed {
            state.lock().unwrap().accepted_budget_post_control = Some(review_nonclosed_off);
        }
        app.running = false;
        driver.tick(&mut runtime, &mut app);
        drive(&mut driver, &mut runtime, &mut app);
        app.running = true;
        review_qualify(&mut runtime);
        let before = state.lock().unwrap().paths.len();
        let mut p = parameters(&runtime, "layout");
        p["layout"] = json!("grid4");
        let request = invocation(&mut runtime, 6100 + u64::from(confirmed), p);
        let result = driver.command(&mut runtime, &app, &request);
        if !confirmed {
            assert_eq!(result.unwrap()["status"], "rejected");
            assert!(driver.retiring || driver.uncertain);
            assert!(!driver.shutdown_confirmed());
            assert!(driver.inputs.iter().any(Option::is_some));
            assert_eq!(
                state.lock().unwrap().paths.len(),
                before,
                "unconfirmed cleanup cannot create a new scene"
            );
            let generation = runtime.multiview_connection.generation();
            let status = invocation(&mut runtime, 6110, json!({"version":1,"op":"status"}));
            let _ = driver.command(&mut runtime, &app, &status);
            assert_eq!(
                runtime.multiview_connection.generation(),
                generation,
                "unconfirmed custody cannot reprobe"
            );
        } else {
            assert!(
                result.is_none(),
                "confirmed original Closed must release the recovery-only admission fence"
            );
            drive(&mut driver, &mut runtime, &mut app);
            assert_eq!(
                runtime
                    .remote
                    .response_for_id_for_test(&request.id)
                    .unwrap()["status"],
                "ok"
            );
            assert!(driver.session.is_some());
            assert!(!driver.recovery_only && !driver.retiring);
            assert_ne!(context(&runtime).scene, old_scene);
            assert!(state.lock().unwrap().paths[before..]
                .iter()
                .any(|p| p.ends_with("/sessions")));
        }
    }
}

type ReviewJobs =
    Arc<Mutex<std::collections::VecDeque<(Box<dyn FnOnce(&OffFrame) + Send>, Arc<AtomicBool>)>>>;
struct ReviewDone(Arc<AtomicBool>);
impl super::super::multiview_connection::Retirement for ReviewDone {
    fn retired(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
}
struct ReviewLauncher(ReviewJobs);
impl super::super::multiview_connection::Launcher for ReviewLauncher {
    fn spawn_io(
        &mut self,
        work: Box<dyn FnOnce(&OffFrame) + Send>,
    ) -> Option<Box<dyn super::super::multiview_connection::Retirement>> {
        let done = Arc::new(AtomicBool::new(false));
        self.0.lock().unwrap().push_back((work, done.clone()));
        Some(Box::new(ReviewDone(done)))
    }
}
fn review_media() -> plx_media::ff::inspect::MediaInfo {
    plx_media::ff::inspect::MediaInfo {
        resolved_source: None,
        hls_playback: None,
        tracks: Default::default(),
        video_codec: "h264".into(),
        audio_codec: "aac".into(),
        width: 320,
        height: 180,
        fps: 10.0,
        duration_ns: 0,
        audio_channels: 2,
        audio_sample_rate: 48_000,
        audio_bit_depth: 16,
        dovi: plx_data::metadata::Dovi::NONE,
    }
}
struct ReviewInspector;
impl super::super::multiview_output::Inspector for ReviewInspector {
    fn inspect(
        &self,
        worker: &OffFrame,
        source: &plx_media::ff::ExternalSource,
        deadline: Deadline,
        cancelled: Arc<AtomicBool>,
    ) -> Result<plx_media::ff::inspect::MediaInfo, &'static str> {
        assert!(!cancelled.load(Ordering::Acquire) && !deadline.expired(worker));
        assert!(matches!(source, plx_media::ff::ExternalSource::Hls(url)
            if url.starts_with("https://service.invalid/multiview/v1/media/") && !url.contains("fixture-bearer")));
        Ok(review_media())
    }
}
struct ReviewPictureRig {
    driver: Driver,
    app: Box<App>,
    runtime: Box<Runtime>,
    state: Arc<Mutex<FixtureState>>,
}
impl Drop for ReviewPictureRig {
    fn drop(&mut self) {
        plx_media::player::SHARED
            .duration_ns
            .store(0, Ordering::Release);
        plx_media::player::stop_bufferfeed(
            &mut self.app.player.session,
            &mut self.app.adapters.player,
        );
        plx_media::player::TX.reset();
        route::reset_player_control_for_test(&self.app.player.session);
        assert!(!self.app.adapters.player.release_has_queued_start());
        assert!(!self.app.adapters.player.is_live());
    }
}
fn review_admitted_picture() -> ReviewPictureRig {
    let (mut driver, mut app, mut runtime, state) = accepted_budget_setup();
    // Renew the expired fixture budget only through a real explicit Layout ACK.
    let mut p = parameters(&runtime, "layout");
    p["layout"] = json!("grid4");
    let request = invocation(&mut runtime, 6200, p);
    assert_eq!(
        driver.command(&mut runtime, &app, &request).unwrap()["status"],
        "ok"
    );
    accepted_budget_drain(&mut driver, &mut runtime, &mut app);
    let jobs: ReviewJobs = Default::default();
    runtime.multiview_output = super::super::multiview_output::State::with_io(
        Arc::new(ReviewInspector),
        Box::new(ReviewLauncher(jobs.clone())),
    );
    let proof = driver
        .take_ready_output(&runtime)
        .expect("actual ready service output");
    runtime.multiview_driver = driver;
    super::super::multiview_output::offer(&mut runtime, &mut app, proof).unwrap();
    super::super::multiview_output::tick(&mut runtime, &mut app);
    loop {
        let Some((work, done)) = jobs.lock().unwrap().pop_front() else {
            break;
        };
        work(&OffFrame::for_test());
        done.store(true, Ordering::Release);
    }
    app.adapters.player.begin_release_for_test(Duration::ZERO);
    super::super::multiview_output::tick(&mut runtime, &mut app);
    let driver = std::mem::take(&mut runtime.multiview_driver);
    let identity = route::playback_identity(&app.player.session)
        .expect("real deferred native admission identity");
    assert_eq!(runtime.multiview_picture.as_ref(), Some(&identity));
    assert_eq!(driver.admitted_picture.as_ref(), Some(&identity));
    assert_eq!(runtime.multiview_output.status, "admitted");
    ReviewPictureRig {
        driver,
        app,
        runtime,
        state,
    }
}
fn review_output_frame(rig: &mut ReviewPictureRig) {
    rig.runtime.multiview_driver = std::mem::take(&mut rig.driver);
    super::super::multiview_output::tick(&mut rig.runtime, &mut rig.app);
    rig.driver = std::mem::take(&mut rig.runtime.multiview_driver);
}
#[test]
fn review_confirmed_recovery_off_retires_only_owned_picture_and_allows_explicit_reprobe() {
    let _serial = plx_base::testlock::serial();
    for foreign in [false, true] {
        let mut rig = review_admitted_picture();
        let owned = rig.driver.admitted_picture.clone().unwrap();
        rig.runtime.multiview_connection.refresh();
        rig.driver.tick(&mut rig.runtime, &mut rig.app);
        assert!(rig.driver.recovery_only);
        let hold = Arc::new((AtomicBool::new(false), AtomicBool::new(false)));
        rig.state.lock().unwrap().accepted_budget_hold_control = Some(hold.clone());
        let mut p = parameters(&rig.runtime, "off");
        p["mode"] = json!("close_only");
        p["recovery_version"] = json!(1);
        let request = invocation(&mut rig.runtime, 6210, p);
        assert!(rig
            .driver
            .command(&mut rig.runtime, &rig.app, &request)
            .is_none());
        let until = Instant::now() + Duration::from_secs(5);
        while !hold.0.load(Ordering::Acquire) {
            assert!(Instant::now() < until);
            std::thread::sleep(Duration::from_millis(1));
        }
        assert_eq!(
            rig.driver.admitted_picture.as_ref(),
            Some(&owned),
            "unconfirmed Off cannot retire picture custody"
        );
        assert!(rig.driver.selected.iter().any(Option::is_some));
        let before = rig.runtime.multiview_connection.generation();
        let status = invocation(&mut rig.runtime, 6211, json!({"version":1,"op":"status"}));
        assert!(rig
            .driver
            .command(&mut rig.runtime, &rig.app, &status)
            .is_some());
        assert_eq!(
            rig.runtime.multiview_connection.generation(),
            before,
            "pending Off cannot reprobe"
        );
        let foreign_identity = if foreign {
            crate::app::playback::replace_player_in_place(
                &mut rig.app.player,
                &mut rig.app.adapters.player,
                &mut rig.app.repause_at,
                &mut rig.app.pages,
            );
            route::request_external_source(
                &mut rig.app.player.session,
                &review_media(),
                plx_media::ff::ExternalSource::Hls("https://successor.invalid/new.m3u8".into()),
                "Ordinary successor",
                "",
                crate::app::playback::start_caps(),
            )
            .unwrap();
            let identity = route::playback_identity(&rig.app.player.session).unwrap();
            assert_ne!(identity, owned);
            rig.runtime.multiview_picture = Some(identity.clone());
            Some(identity)
        } else {
            None
        };
        hold.1.store(true, Ordering::Release);
        drive(&mut rig.driver, &mut rig.runtime, &mut rig.app);
        review_output_frame(&mut rig); // Real Runtime order: Driver, then output owner.
        rig.driver.tick(&mut rig.runtime, &mut rig.app); // Next frame must not enter taskless retirement.
        assert!(!rig.driver.retiring && !rig.driver.recovery_only);
        assert!(rig.driver.admitted_picture.is_none());
        assert!(
            rig.driver.selected.iter().all(Option::is_none),
            "closed original scene releases every selected source receipt"
        );
        assert!(rig.driver.inputs.iter().all(Option::is_none));
        if let Some(identity) = foreign_identity {
            assert_eq!(rig.runtime.multiview_picture.as_ref(), Some(&identity));
            assert!(route::playback_identity_matches(
                &rig.app.player.session,
                &identity
            ));
            assert!(
                route::has_url(&rig.app.player.session),
                "original close must not stop a foreign successor"
            );
            continue;
        }
        assert!(rig.runtime.multiview_picture.is_none());
        let mut zero = capabilities();
        zero.max_inputs = 0;
        zero.output_profiles.clear();
        rig.state.lock().unwrap().capabilities = Some(zero);
        review_qualify(&mut rig.runtime);
        assert_eq!(
            rig.runtime
                .multiview_connection
                .qualified()
                .unwrap()
                .capabilities
                .max_inputs,
            0
        );
        let before = rig.runtime.multiview_connection.generation();
        rig.state.lock().unwrap().capabilities = Some(capabilities());
        let start = rig.runtime.clock_sample.unwrap();
        for (id, elapsed, due) in [(6300, 29_999, false), (6301, 30_000, true)] {
            rig.runtime.clock_sample = Some(start.advance_for_test(Duration::from_millis(elapsed)));
            let status = invocation(&mut rig.runtime, id, json!({"version":1,"op":"status"}));
            assert!(rig.driver.command(&mut rig.runtime, &rig.app, &status).is_some());
            assert_eq!(rig.runtime.multiview_connection.generation() != before, due);
        }
        review_qualify(&mut rig.runtime);
        let request = layout(
            &mut rig.driver,
            &mut rig.runtime,
            &mut rig.app,
            9000,
            "grid4",
        );
        assert_eq!(
            rig.runtime
                .remote
                .response_for_id_for_test(&request.id)
                .unwrap()["status"],
            "ok"
        );
    }
}
