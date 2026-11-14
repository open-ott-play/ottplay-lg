use super::*;
use std::collections::VecDeque;
use std::sync::Mutex;

#[derive(Default)]
struct FakeTransport {
    responses: Mutex<VecDeque<Vec<u8>>>,
    requests: Mutex<Vec<(String, String, Option<String>, Option<Vec<u8>>)>>,
}
impl FakeTransport {
    fn reply(value: &impl Serialize) -> Self {
        Self {
            responses: Mutex::new(VecDeque::from([serde_json::to_vec(value).unwrap()])),
            ..Self::default()
        }
    }
    fn raw(body: Vec<u8>) -> Self {
        Self {
            responses: Mutex::new(VecDeque::from([body])),
            ..Self::default()
        }
    }
    fn request(&self) -> (String, String, Option<String>, Option<Vec<u8>>) {
        self.requests.lock().unwrap().last().unwrap().clone()
    }
}
impl Transport for FakeTransport {
    fn send(
        &self,
        _worker: &OffFrame,
        _binding: &ServiceBinding,
        request: &ServiceRequest,
        _deadline: Deadline,
        _cancelled: &AtomicBool,
    ) -> Result<Vec<u8>, ServiceError> {
        self.requests.lock().unwrap().push((
            request.method().into(),
            request.url(_binding),
            request.request_id().map(str::to_owned),
            request.body().map(<[u8]>::to_vec),
        ));
        self.responses
            .lock()
            .unwrap()
            .pop_front()
            .ok_or(ServiceError::Transport)
    }
}

fn binding_json() -> String {
    serde_json::json!({
        "server_id":"server_A", "origin":"https://compositor.example.test",
        "audience":AUDIENCE, "installation_id":"native_A",
        "control_bearer":"synthetic-dedicated-control-bearer-0123456789"
    })
    .to_string()
}
fn service<T: Transport>(transport: T) -> ServiceClient<T> {
    let binding = ServiceBinding::from_protected_json(&binding_json())
        .unwrap()
        .unwrap();
    ServiceClient::new(binding, "native_A", transport).unwrap()
}
fn worker() -> OffFrame {
    OffFrame::for_test()
}
fn deadline(worker: &OffFrame) -> Deadline {
    Deadline::after(worker, Duration::from_secs(5))
}
fn native() -> NativeExpected {
    NativeExpected {
        runtime: "runtime_A".into(),
        expected: NativeRevisions {
            scene: "scene_A".into(),
            layout_revision: "layout_1".into(),
            focus_revision: "focus_1".into(),
            audio_revision: "audio_1".into(),
        },
        slot_id: 0,
        slot_generation: "slot_1_1".into(),
        source_revision: "source_1".into(),
    }
}
fn native_source_next() -> NativeExpected {
    let mut next = native();
    next.slot_generation = "slot_2".into();
    next.source_revision = "source_2".into();
    next
}
fn backend() -> BackendExpected {
    BackendExpected {
        runtime: 1,
        scene: "backend_scene_1".into(),
        geometry_revision: 1,
        focus_revision: 1,
        audio_revision: 1,
    }
}
fn context() -> Binding {
    Binding {
        server_epoch: "epoch_A".into(),
        session_id: "session_A".into(),
        backend_expected: backend(),
        native_expected: native(),
        backend_slot_revision: 1,
    }
}
fn geometry() -> Geometry {
    Geometry {
        canvas: Canvas {
            width: 960,
            height: 540,
            fps: 15,
        },
        revision: 1,
        mode: GeometryMode::Disjoint,
        rects: vec![Rect {
            slot_id: 0,
            x: 0,
            y: 0,
            width: 960,
            height: 540,
            z: 0,
        }],
    }
}
fn capabilities() -> Capabilities {
    Capabilities {
        version: VERSION,
        server_id: "server_A".into(),
        server_epoch: "epoch_A".into(),
        max_inputs: 4,
        qualified_input_adapters: vec!["http_checked".into()],
        output_profiles: vec![OutputProfile {
            profile_id: "profile_A".into(),
            canvas: Canvas {
                width: 960,
                height: 540,
                fps: 15,
            },
            max_inputs: 4,
            geometry_modes: vec![GeometryMode::Disjoint, GeometryMode::Pip],
        }],
        output_auth: "hmac_path_v1".into(),
        catalog_revision: "catalog_1".into(),
    }
}
fn item() -> CatalogItem {
    CatalogItem {
        source_ref: "source_A".into(),
        item_ref: "item_A".into(),
        item_revision: "item_rev_1".into(),
        identity_version: 1,
        favorite_sha256: "a".repeat(64),
        kind: "vod".into(),
        input_adapter: "http_checked".into(),
        protection: Protection {
            known: true,
            restricted: false,
        },
        descriptor: Descriptor {
            title: "A synthetic title".into(),
            provider: "m3u".into(),
        },
    }
}
fn status(state: SessionState, output: Option<Output>) -> StatusResponse {
    StatusResponse {
        version: VERSION,
        server_id: "server_A".into(),
        server_epoch: "epoch_A".into(),
        session_id: "session_A".into(),
        native_expected: native(),
        backend_expected: backend(),
        state,
        geometry: geometry(),
        slots: vec![SlotSnapshot {
            slot_id: 0,
            source_revision: 1,
            state: SlotState::Playing,
            error: None,
            frames: 10,
            position_ms: Some(1000),
            duration_ms: Some(60000),
            source_ref: Some("source_A".into()),
            item_ref: Some("item_A".into()),
        }],
        audio: AudioState {
            policy: AudioPolicy::FollowsFocus,
            slot_id: Some(0),
        },
        output,
    }
}

fn hmac_lease() -> String {
    format!("{}.{}", "a".repeat(32), "b".repeat(64))
}

#[test]
fn protected_pairing_is_exact_https_and_bound_to_persisted_installation() {
    let parsed = ServiceBinding::from_protected_json(&binding_json())
        .unwrap()
        .unwrap();
    assert_eq!(parsed.bind_for_installation("native_A"), Ok(()));
    assert_eq!(
        parsed.bind_for_installation("native_B"),
        Err(ServiceError::UnpairedInstallation)
    );
    assert!(
        ServiceBinding::from_protected_json(&binding_json().replace("https://", "http://"))
            .is_err()
    );
    assert!(ServiceBinding::from_protected_json(
        &binding_json().replace(".test\"", ".test/path\"")
    )
    .is_err());
    assert!(ServiceBinding::from_protected_json(
        &binding_json().replace(AUDIENCE, "ott-remote-control")
    )
    .is_err());
    assert!(format!("{parsed:?}").contains("<redacted>"));
    assert!(!format!("{parsed:?}").contains("synthetic-dedicated-control"));
    assert_eq!(
        canonical_https_origin("https://Compositor.Example.Test").unwrap(),
        "https://compositor.example.test",
        "compositor URLs must not inherit Plex's default port"
    );
    assert_eq!(
        canonical_https_origin("https://compositor.example.test:443").unwrap(),
        "https://compositor.example.test"
    );
    assert_eq!(
        canonical_https_origin("https://compositor.example.test:8443").unwrap(),
        "https://compositor.example.test:8443"
    );
    assert_eq!(
        canonical_https_origin("https://[2001:db8::1]:9443").unwrap(),
        "https://[2001:db8::1]:9443"
    );
    assert!(canonical_https_origin("https://compositor.example.test:0").is_err());
    assert!(canonical_https_origin("https://compositor.example.test:65536").is_err());
    let canonical = parsed.protected_json().unwrap();
    let restored = ServiceBinding::from_protected_json(&canonical)
        .unwrap()
        .unwrap();
    assert_eq!(restored.bind_for_installation("native_A"), Ok(()));
}

#[test]
fn capabilities_request_uses_only_fixed_authenticated_https_endpoint() {
    let response = capabilities();
    let fake = FakeTransport::reply(&response);
    let client = service(fake);
    let worker = worker();
    let cancelled = AtomicBool::new(false);
    assert_eq!(
        client
            .capabilities(&worker, deadline(&worker), &cancelled)
            .unwrap(),
        response
    );
    let seen = client.transport.request();
    assert_eq!(seen.0, "GET");
    assert_eq!(
        seen.1,
        "https://compositor.example.test/multiview/v1/capabilities"
    );
    assert_eq!(seen.2, None);
    assert_eq!(seen.3, None);
}

#[test]
fn response_identity_and_output_profile_must_be_admitted_before_session_creation() {
    let response = status(SessionState::Preparing, None);
    let fake = FakeTransport::reply(&response);
    let client = service(fake);
    let worker = worker();
    let cancelled = AtomicBool::new(false);
    let created = client
        .create_session(
            &worker,
            &capabilities(),
            native(),
            "profile_A",
            geometry(),
            RequestId::parse("0123456789abcdef0123456789abcdef").unwrap(),
            deadline(&worker),
            &cancelled,
        )
        .unwrap();
    assert_eq!(created.session_id, "session_A");
    let request = client.transport.request();
    assert_eq!(request.0, "POST");
    assert_eq!(
        request.2.as_deref(),
        Some("0123456789abcdef0123456789abcdef")
    );
    let body: SessionCreateRequest = serde_json::from_slice(request.3.as_deref().unwrap()).unwrap();
    assert_eq!(body.output_profile_id, "profile_A");

    let fake = FakeTransport::reply(&response);
    let client = service(fake);
    assert!(matches!(
        client.create_session(
            &worker,
            &capabilities(),
            native(),
            "unqualified",
            geometry(),
            RequestId::parse("0123456789abcdef0123456789abcdef").unwrap(),
            deadline(&worker),
            &cancelled
        ),
        Err(ServiceError::Unsupported)
    ));
}

#[test]
fn visible_geometry_capacity_is_not_mistaken_for_decoder_admission() {
    let mut grid = status(SessionState::Preparing, None);
    let rects = (0..9)
        .map(|slot| Rect {
            slot_id: slot,
            x: u32::from(slot % 3) * 320,
            y: u32::from(slot / 3) * 180,
            width: 320,
            height: 180,
            z: slot,
        })
        .collect();
    let grid_geometry = Geometry {
        rects,
        ..geometry()
    };
    grid.geometry = grid_geometry.clone();
    let client = service(FakeTransport::reply(&grid));
    let worker = worker();
    let cancelled = AtomicBool::new(false);
    let result = client.create_session(
        &worker,
        &capabilities(),
        native(),
        "profile_A",
        grid_geometry,
        RequestId::parse("0123456789abcdef0123456789abcdef").unwrap(),
        deadline(&worker),
        &cancelled,
    );
    assert!(
        result.is_ok(),
        "nine visible slots are geometry; they do not imply nine admitted inputs"
    );
}

#[test]
fn capabilities_report_no_inputs_until_a_qualified_output_profile_exists() {
    let mut disabled = capabilities();
    disabled.max_inputs = 0;
    disabled.output_profiles.clear();
    assert!(valid_capabilities(&disabled, "server_A").is_ok());
    disabled.output_profiles = capabilities().output_profiles;
    assert_eq!(
        valid_capabilities(&disabled, "server_A"),
        Err(ServiceError::InvalidResponse)
    );
    let mut mismatched = capabilities();
    mismatched.output_profiles[0].max_inputs = 3;
    assert_eq!(
        valid_capabilities(&mismatched, "server_A"),
        Err(ServiceError::InvalidResponse)
    );

    let disabled = Capabilities {
        max_inputs: 0,
        output_profiles: Vec::new(),
        ..capabilities()
    };
    let row = item();
    let worker = worker();
    let cancelled = AtomicBool::new(false);
    let client = service(FakeTransport::default());
    assert_eq!(
        client
            .input_grant(
                &worker,
                context(),
                &row,
                None,
                "catalog_1",
                5000,
                &disabled,
                RequestId::parse("11111111111111111111111111111111").unwrap(),
                deadline(&worker),
                &cancelled
            )
            .err(),
        Some(ServiceError::Unsupported)
    );
    assert_eq!(
        client
            .control(
                &worker,
                context(),
                ControlOp::Play {
                    input_grant: "grant_A".into()
                },
                None,
                &disabled,
                RequestId::parse("11111111111111111111111111111111").unwrap(),
                deadline(&worker),
                &cancelled
            )
            .err(),
        Some(ServiceError::Unsupported)
    );
}

#[test]
fn read_post_requests_have_no_mutation_id_and_echo_the_exact_binding() {
    let response = SourcesResponse {
        version: VERSION,
        server_id: "server_A".into(),
        context: context(),
        catalog_revision: "catalog_1".into(),
        items: vec![item()],
        next_cursor: None,
    };
    let fake = FakeTransport::reply(&response);
    let client = service(fake);
    let worker = worker();
    let cancelled = AtomicBool::new(false);
    let page = client
        .sources(
            &worker,
            context(),
            None,
            &capabilities(),
            deadline(&worker),
            &cancelled,
        )
        .unwrap();
    assert_eq!(page.items, vec![item()]);
    let seen = client.transport.request();
    assert_eq!(seen.0, "POST");
    assert_eq!(seen.2, None);
    assert_eq!(
        seen.1,
        "https://compositor.example.test/multiview/v1/sources"
    );
    let fake = FakeTransport::reply(&SourcesResponse {
        context: Binding {
            backend_slot_revision: 2,
            ..context()
        },
        ..response
    });
    let client = service(fake);
    assert_eq!(
        client
            .sources(
                &worker,
                context(),
                None,
                &capabilities(),
                deadline(&worker),
                &cancelled
            )
            .unwrap_err(),
        ServiceError::InvalidResponse
    );
}

#[test]
fn source_rows_are_bounded_authorized_and_unique() {
    let mut unknown = item();
    unknown.input_adapter = "http_guess".into();
    let bad = SourcesResponse {
        version: VERSION,
        server_id: "server_A".into(),
        context: context(),
        catalog_revision: "catalog_1".into(),
        items: vec![unknown],
        next_cursor: None,
    };
    let client = service(FakeTransport::reply(&bad));
    let worker = worker();
    let cancelled = AtomicBool::new(false);
    assert_eq!(
        client
            .sources(
                &worker,
                context(),
                None,
                &capabilities(),
                deadline(&worker),
                &cancelled
            )
            .unwrap_err(),
        ServiceError::InvalidResponse
    );

    let duplicate = SourcesResponse {
        version: VERSION,
        server_id: "server_A".into(),
        context: context(),
        catalog_revision: "catalog_1".into(),
        items: vec![item(), item()],
        next_cursor: None,
    };
    let client = service(FakeTransport::reply(&duplicate));
    assert_eq!(
        client
            .sources(
                &worker,
                context(),
                None,
                &capabilities(),
                deadline(&worker),
                &cancelled
            )
            .unwrap_err(),
        ServiceError::InvalidResponse
    );
}

#[test]
fn input_grant_echoes_exact_item_and_never_outlives_requested_budget() {
    let row = item();
    let good = GrantResponse {
        version: VERSION,
        server_id: "server_A".into(),
        context: context(),
        item: row.clone(),
        catalog_revision: "catalog_1".into(),
        policy_revision: "policy_1".into(),
        input_grant: "grant_A".into(),
        expires_in_ms: 4000,
    };
    let fake = FakeTransport::reply(&good);
    let client = service(fake);
    let worker = worker();
    let cancelled = AtomicBool::new(false);
    assert_eq!(
        client
            .input_grant(
                &worker,
                context(),
                &row,
                None,
                "catalog_1",
                5000,
                &capabilities(),
                RequestId::parse("11111111111111111111111111111111").unwrap(),
                deadline(&worker),
                &cancelled
            )
            .unwrap()
            .input_grant,
        "grant_A"
    );
    let request = client.transport.request();
    assert_eq!(
        request.2.as_deref(),
        Some("11111111111111111111111111111111")
    );
    let body: GrantRequest = serde_json::from_slice(request.3.as_deref().unwrap()).unwrap();
    assert_eq!(body.remaining_budget_ms, 5000);

    let expired = GrantResponse {
        expires_in_ms: 5001,
        ..good
    };
    let client = service(FakeTransport::reply(&expired));
    assert!(matches!(
        client.input_grant(
            &worker,
            context(),
            &row,
            None,
            "catalog_1",
            5000,
            &capabilities(),
            RequestId::parse("11111111111111111111111111111111").unwrap(),
            deadline(&worker),
            &cancelled
        ),
        Err(ServiceError::InvalidResponse)
    ));
}

#[test]
fn input_grant_commits_optional_source_transition_and_binds_grant_to_returned_context() {
    let prior = context();
    let next = native_source_next();
    let mut committed = prior.clone();
    committed.native_expected = next.clone();
    let row = item();
    let response = GrantResponse {
        version: VERSION,
        server_id: "server_A".into(),
        context: committed,
        item: row.clone(),
        catalog_revision: "catalog_1".into(),
        policy_revision: "policy_1".into(),
        input_grant: "grant_A".into(),
        expires_in_ms: 4000,
    };
    let client = service(FakeTransport::reply(&response));
    let worker = worker();
    let cancelled = AtomicBool::new(false);
    let granted = client
        .input_grant(
            &worker,
            prior.clone(),
            &row,
            Some(next.clone()),
            "catalog_1",
            5000,
            &capabilities(),
            RequestId::parse("33333333333333333333333333333333").unwrap(),
            deadline(&worker),
            &cancelled,
        )
        .unwrap();
    assert_eq!(granted.context.native_expected, next);
    let request = client.transport.request();
    let body: GrantRequest = serde_json::from_slice(request.3.as_deref().unwrap()).unwrap();
    assert_eq!(body.context, prior);
    assert_eq!(body.next_native, Some(granted.context.native_expected));
}

#[test]
fn input_grant_rejects_native_scene_rebinding_before_network_io() {
    let mut forged = native_source_next();
    forged.expected.scene = "another_scene".into();
    let client = service(FakeTransport::default());
    let worker = worker();
    let cancelled = AtomicBool::new(false);
    assert!(matches!(
        client.input_grant(
            &worker,
            context(),
            &item(),
            Some(forged),
            "catalog_1",
            5000,
            &capabilities(),
            RequestId::parse("44444444444444444444444444444444").unwrap(),
            deadline(&worker),
            &cancelled,
        ),
        Err(ServiceError::InvalidRequest)
    ));
    assert!(client.transport.requests.lock().unwrap().is_empty());
}

#[test]
fn control_mutations_have_ids_and_only_closed_operations() {
    let response = ControlResponse {
        version: VERSION,
        context: context(),
        accepted: true,
        operation_id: "operation_A".into(),
        state: SessionState::Preparing,
    };
    let client = service(FakeTransport::reply(&response));
    let worker = worker();
    let cancelled = AtomicBool::new(false);
    let result = client
        .control(
            &worker,
            context(),
            ControlOp::Audio {
                policy: AudioPolicy::FollowsFocus,
            },
            None,
            &capabilities(),
            RequestId::parse("22222222222222222222222222222222").unwrap(),
            deadline(&worker),
            &cancelled,
        )
        .unwrap();
    assert!(result.accepted);
    let request = client.transport.request();
    assert_eq!(
        request.1,
        "https://compositor.example.test/multiview/v1/control"
    );
    assert_eq!(
        request.2.as_deref(),
        Some("22222222222222222222222222222222")
    );
    let json: serde_json::Value = serde_json::from_slice(request.3.as_deref().unwrap()).unwrap();
    assert_eq!(json["op"]["name"], "audio");
    assert_eq!(json["op"]["policy"], "follows_focus");
    assert!(serde_json::from_str::<ControlOp>(r#"{"name":"off","credential":"x"}"#).is_err());
}

#[test]
fn focus_control_commits_only_allowed_native_revisions_and_returns_authoritative_context() {
    let prior = context();
    let mut next = native();
    next.expected.focus_revision = "focus_2".into();
    next.expected.audio_revision = "audio_2".into();
    let mut returned = prior.clone();
    returned.native_expected = next.clone();
    returned.backend_expected.focus_revision = 2;
    returned.backend_expected.audio_revision = 2;
    let response = ControlResponse {
        version: VERSION,
        context: returned,
        accepted: true,
        operation_id: "focus_operation".into(),
        state: SessionState::Preparing,
    };
    let client = service(FakeTransport::reply(&response));
    let worker = worker();
    let cancelled = AtomicBool::new(false);
    let returned = client
        .control(
            &worker,
            prior.clone(),
            ControlOp::Focus {},
            Some(next.clone()),
            &capabilities(),
            RequestId::parse("55555555555555555555555555555555").unwrap(),
            deadline(&worker),
            &cancelled,
        )
        .unwrap();
    assert_eq!(returned.context.native_expected, next);
    let request = client.transport.request();
    let body: ControlRequest = serde_json::from_slice(request.3.as_deref().unwrap()).unwrap();
    assert_eq!(body.context, prior);
    assert_eq!(body.next_native, Some(returned.context.native_expected));
}

#[test]
fn status_recovery_accepts_stale_mutable_native_revisions_but_keeps_scene_identity() {
    let mut recovered = status(SessionState::Preparing, None);
    recovered.native_expected.expected.focus_revision = "focus_after_lost_reply".into();
    recovered.native_expected.slot_generation = "slot_after_lost_reply".into();
    recovered.native_expected.source_revision = "source_after_lost_reply".into();
    let client = service(FakeTransport::reply(&recovered));
    let worker = worker();
    let cancelled = AtomicBool::new(false);
    let actual = client
        .status(
            &worker,
            "epoch_A",
            "session_A",
            native(),
            deadline(&worker),
            &cancelled,
        )
        .unwrap();
    assert_eq!(actual.native_expected, recovered.native_expected);

    let mut wrong_owner = recovered;
    wrong_owner.native_expected.runtime = "other_runtime".into();
    let client = service(FakeTransport::reply(&wrong_owner));
    assert!(matches!(
        client.status(
            &worker,
            "epoch_A",
            "session_A",
            native(),
            deadline(&worker),
            &cancelled,
        ),
        Err(ServiceError::InvalidResponse)
    ));
}

#[test]
fn focus_transition_rejects_native_source_or_layout_mutations_before_network_io() {
    let mut invalid = native();
    invalid.expected.layout_revision = "layout_forged".into();
    let client = service(FakeTransport::default());
    let worker = worker();
    let cancelled = AtomicBool::new(false);
    assert!(matches!(
        client.control(
            &worker,
            context(),
            ControlOp::Focus {},
            Some(invalid),
            &capabilities(),
            RequestId::parse("66666666666666666666666666666666").unwrap(),
            deadline(&worker),
            &cancelled,
        ),
        Err(ServiceError::InvalidRequest)
    ));
    assert!(client.transport.requests.lock().unwrap().is_empty());
}

#[test]
fn private_output_requires_ready_same_session_hmac_path() {
    let client = service(FakeTransport::default());
    let cap = capabilities();
    let not_ready = status(SessionState::Preparing, None);
    assert_eq!(
        client.private_output(&cap, &not_ready).unwrap_err(),
        ServiceError::InvalidOutput
    );
    let output = Output {
        session_id: "session_A".into(),
        server_epoch: "epoch_A".into(),
        output_generation: "output_1".into(),
        playlist_path: format!("/multiview/v1/media/{}/index.m3u8", hmac_lease()),
        lease_expires_in_ms: 90_000,
    };
    let ready = status(SessionState::Ready, Some(output));
    let private = client.private_output(&cap, &ready).unwrap();
    assert_eq!(
        private.playlist_url_for_media_worker(),
        format!(
            "https://compositor.example.test/multiview/v1/media/{}/index.m3u8",
            hmac_lease()
        )
    );
    assert!(!format!("{private:?}").contains(&hmac_lease()));
}

#[test]
fn output_path_accepts_exact_nonce_signature_hex_lease_only() {
    let valid = format!("/multiview/v1/media/{}/index.m3u8", hmac_lease());
    assert!(valid_output_path(&valid));

    for lease in [
        format!("{}.{}", "a".repeat(31), "b".repeat(64)),
        format!("{}.{}", "a".repeat(33), "b".repeat(64)),
        format!("{}.{}", "a".repeat(32), "b".repeat(63)),
        format!("{}.{}", "a".repeat(32), "b".repeat(65)),
        format!("{}.{}", "g".repeat(32), "b".repeat(64)),
        format!("{}.{}.x", "a".repeat(32), "b".repeat(64)),
    ] {
        assert!(
            !valid_output_path(&format!("/multiview/v1/media/{lease}/index.m3u8")),
            "accepted malformed lease {lease:?}"
        );
    }
    assert!(!valid_output_path(
        &valid.replace("/multiview/v1/media/", "/other/")
    ));
    assert!(!valid_output_path(
        &valid.replace("index.m3u8", "segment.ts")
    ));
}

#[test]
fn stale_server_epoch_session_and_unpaired_device_fail_closed() {
    let stale = StatusResponse {
        server_epoch: "epoch_old".into(),
        ..status(SessionState::Preparing, None)
    };
    let client = service(FakeTransport::reply(&stale));
    let worker = worker();
    let cancelled = AtomicBool::new(false);
    assert!(matches!(
        client.status(
            &worker,
            "epoch_A",
            "session_A",
            native(),
            deadline(&worker),
            &cancelled
        ),
        Err(ServiceError::InvalidResponse)
    ));
    let binding = ServiceBinding::from_protected_json(&binding_json())
        .unwrap()
        .unwrap();
    assert_eq!(
        ServiceClient::new(binding, "temporary_runtime_uuid", FakeTransport::default()).err(),
        Some(ServiceError::UnpairedInstallation)
    );
}

#[test]
fn request_and_response_budgets_and_cancel_are_checked() {
    assert!(RequestId::parse("not-an-id").is_err());
    let worker = worker();
    let cancelled = AtomicBool::new(true);
    let client = service(FakeTransport::reply(&capabilities()));
    assert_eq!(
        client
            .capabilities(&worker, deadline(&worker), &cancelled)
            .unwrap_err(),
        ServiceError::Cancelled
    );
}

#[test]
fn malformed_or_oversized_response_is_not_exposed_as_a_transport_error() {
    let worker = worker();
    let cancelled = AtomicBool::new(false);
    let client = service(FakeTransport::raw(b"not json".to_vec()));
    assert_eq!(
        client
            .capabilities(&worker, deadline(&worker), &cancelled)
            .unwrap_err(),
        ServiceError::InvalidResponse
    );
    let client = service(FakeTransport::raw(vec![b'x'; MAX_RESULT_BYTES + 1]));
    assert_eq!(
        client
            .capabilities(&worker, deadline(&worker), &cancelled)
            .unwrap_err(),
        ServiceError::ResponseTooLarge
    );
}

#[test]
fn known_service_http_failures_map_to_closed_errors_without_body_details() {
    assert_eq!(http_status_error(401), ServiceError::Unauthorized);
    assert_eq!(http_status_error(403), ServiceError::Forbidden);
    assert_eq!(http_status_error(409), ServiceError::RevisionConflict);
    assert_eq!(http_status_error(410), ServiceError::AuthorityRevoked);
    assert_eq!(http_status_error(413), ServiceError::ResourceLimit);
    assert_eq!(http_status_error(503), ServiceError::BackendUnavailable);
    assert_eq!(http_status_error(418), ServiceError::HttpStatus(418));
}

#[test]
fn real_empty_slot_zero_revisions_are_valid_authoritative_cas_values() {
    let worker = worker();
    let cancelled = AtomicBool::new(false);
    let mut binding = context();
    binding.backend_slot_revision = 0;
    binding.backend_expected.focus_revision = 0;
    binding.backend_expected.audio_revision = 0;
    assert!(binding.valid());
    let response = SourcesResponse {
        version: VERSION,
        server_id: "server_A".into(),
        context: binding.clone(),
        catalog_revision: capabilities().catalog_revision,
        items: Vec::new(),
        next_cursor: None,
    };
    let client = service(FakeTransport::reply(&response));
    let reply = client
        .sources(
            &worker,
            binding,
            None,
            &capabilities(),
            deadline(&worker),
            &cancelled,
        )
        .unwrap();
    assert_eq!(reply.context.backend_slot_revision, 0);
    assert_eq!(reply.context.backend_expected.focus_revision, 0);
    let mut response = status(SessionState::Preparing, None);
    response.backend_expected.focus_revision = 0;
    response.backend_expected.audio_revision = 0;
    let client = service(FakeTransport::reply(&response));
    let reply = client
        .status(
            &worker,
            "epoch_A",
            "session_A",
            native(),
            deadline(&worker),
            &cancelled,
        )
        .unwrap();
    assert_eq!(reply.backend_expected.focus_revision, 0);
}

fn play_response(
    prior: Binding,
    returned: Binding,
    accepted: bool,
) -> Result<ControlResponse, ServiceError> {
    let response = ControlResponse {
        version: VERSION,
        context: returned,
        accepted,
        operation_id: "play_operation".into(),
        state: SessionState::Preparing,
    };
    let client = service(FakeTransport::reply(&response));
    let worker = worker();
    let result = client.control(
        &worker,
        prior.clone(),
        ControlOp::Play {
            input_grant: "exact_redeemed_grant".into(),
        },
        None,
        &capabilities(),
        RequestId::parse("77777777777777777777777777777777").unwrap(),
        deadline(&worker),
        &AtomicBool::new(false),
    );
    let request: ControlRequest =
        serde_json::from_slice(client.transport.request().3.as_deref().unwrap()).unwrap();
    assert_eq!(
        request.context, prior,
        "response rebasing must never rewrite the grant redemption request"
    );
    assert!(request.next_native.is_none());
    assert_eq!(
        client.transport.requests.lock().unwrap().len(),
        1,
        "no mutating retry"
    );
    result
}

#[test]
fn accepted_play_acknowledges_concurrent_focus_audio_without_rebinding_source_or_redeeming_again() {
    for (focus, audio, change_tokens) in [
        (false, false, false),
        (true, false, true),
        (false, true, true),
        (true, true, true),
        (true, true, false),
    ] {
        let prior = context();
        let mut returned = prior.clone();
        returned.backend_slot_revision += 1;
        if focus {
            returned.backend_expected.focus_revision += 1;
            if change_tokens {
                returned.native_expected.expected.focus_revision = "focus_2".into();
            }
        }
        if audio {
            returned.backend_expected.audio_revision += 1;
            if change_tokens {
                returned.native_expected.expected.audio_revision = "audio_2".into();
            }
        }
        let actual = play_response(prior, returned.clone(), true).unwrap();
        assert_eq!(actual.context, returned);
        assert!(actual.accepted);
        assert_eq!(
            actual.state,
            SessionState::Preparing,
            "acknowledgement is not first-frame evidence"
        );
    }
}

#[test]
fn accepted_play_rebase_rejects_foreign_geometry_source_runtime_and_unproven_shared_tokens() {
    let prior = context();
    let mut good = prior.clone();
    good.backend_slot_revision += 1;
    good.backend_expected.focus_revision += 1;
    good.backend_expected.audio_revision += 1;
    good.native_expected.expected.focus_revision = "focus_2".into();
    good.native_expected.expected.audio_revision = "audio_2".into();
    let mut cases = Vec::new();
    macro_rules! reject {
        ($label:literal, $field:expr, $value:expr) => {{
            let mut changed = good.clone();
            ($field)(&mut changed, $value);
            cases.push(($label, changed));
        }};
    }
    reject!(
        "epoch",
        |v: &mut Binding, x| v.server_epoch = x,
        "foreign_epoch".into()
    );
    reject!(
        "session",
        |v: &mut Binding, x| v.session_id = x,
        "foreign_session".into()
    );
    reject!(
        "backend runtime",
        |v: &mut Binding, x| v.backend_expected.runtime = x,
        2
    );
    reject!(
        "backend scene",
        |v: &mut Binding, x| v.backend_expected.scene = x,
        "foreign_scene".into()
    );
    reject!(
        "geometry",
        |v: &mut Binding, x| v.backend_expected.geometry_revision = x,
        2
    );
    reject!(
        "native runtime",
        |v: &mut Binding, x| v.native_expected.runtime = x,
        "foreign_runtime".into()
    );
    reject!(
        "native scene",
        |v: &mut Binding, x| v.native_expected.expected.scene = x,
        "foreign_scene".into()
    );
    reject!(
        "layout",
        |v: &mut Binding, x| v.native_expected.expected.layout_revision = x,
        "layout_2".into()
    );
    reject!(
        "slot",
        |v: &mut Binding, x| v.native_expected.slot_id = x,
        1
    );
    reject!(
        "slot generation",
        |v: &mut Binding, x| v.native_expected.slot_generation = x,
        "slot_foreign".into()
    );
    reject!(
        "source",
        |v: &mut Binding, x| v.native_expected.source_revision = x,
        "source_foreign".into()
    );
    reject!(
        "slot not advanced",
        |v: &mut Binding, x| v.backend_slot_revision = x,
        1
    );
    reject!(
        "slot advanced twice",
        |v: &mut Binding, x| v.backend_slot_revision = x,
        3
    );
    reject!(
        "focus regression",
        |v: &mut Binding, x| v.backend_expected.focus_revision = x,
        0
    );
    reject!(
        "audio regression",
        |v: &mut Binding, x| v.backend_expected.audio_revision = x,
        0
    );
    reject!(
        "changed focus token at same counter",
        |v: &mut Binding, x| v.backend_expected.focus_revision = x,
        1
    );
    reject!(
        "changed audio token at same counter",
        |v: &mut Binding, x| v.backend_expected.audio_revision = x,
        1
    );
    for (label, returned) in cases {
        assert!(
            matches!(
                play_response(prior.clone(), returned, true),
                Err(ServiceError::InvalidResponse)
            ),
            "{label}"
        );
    }
    let mut overflow = prior.clone();
    overflow.backend_slot_revision = u64::MAX;
    let mut wrapped = overflow.clone();
    wrapped.backend_slot_revision = 0;
    assert!(matches!(
        play_response(overflow, wrapped, true),
        Err(ServiceError::InvalidResponse)
    ));
}

#[test]
fn rejected_play_and_other_operations_do_not_gain_accepted_play_rebase_permission() {
    let prior = context();
    assert!(
        !play_response(prior.clone(), prior.clone(), false)
            .unwrap()
            .accepted
    );
    let mut rebased = prior.clone();
    rebased.backend_slot_revision += 1;
    rebased.backend_expected.focus_revision += 1;
    rebased.native_expected.expected.focus_revision = "focus_2".into();
    assert!(matches!(
        play_response(prior.clone(), rebased.clone(), false),
        Err(ServiceError::InvalidResponse)
    ));
    let client = service(FakeTransport::reply(&ControlResponse {
        version: VERSION,
        context: rebased,
        accepted: true,
        operation_id: "stop_operation".into(),
        state: SessionState::Preparing,
    }));
    let worker = worker();
    assert!(matches!(
        client.control(
            &worker,
            prior,
            ControlOp::Stop {},
            None,
            &capabilities(),
            RequestId::parse("88888888888888888888888888888888").unwrap(),
            deadline(&worker),
            &AtomicBool::new(false)
        ),
        Err(ServiceError::InvalidResponse)
    ));
}
