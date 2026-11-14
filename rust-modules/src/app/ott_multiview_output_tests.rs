use super::super::multiview_connection::SharedTransport;
use super::super::multiview_service::*;
use super::*;
use std::{collections::VecDeque, sync::Mutex, time::Duration};

#[derive(Default)]
struct FakeInspector {
    calls: Mutex<Vec<ExternalSource>>,
}
impl Inspector for FakeInspector {
    fn inspect(
        &self,
        _: &OffFrame,
        source: &ExternalSource,
        _: Deadline,
        _: Arc<AtomicBool>,
    ) -> Result<MediaInfo, &'static str> {
        self.calls.lock().unwrap().push(source.clone());
        Ok(media())
    }
}
struct Done(Arc<AtomicBool>);
impl Retirement for Done {
    fn retired(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
}
#[derive(Default)]
struct ManualLauncher {
    jobs: VecDeque<(Box<dyn FnOnce(&OffFrame) + Send>, Arc<AtomicBool>)>,
    refused: bool,
}
impl Launcher for ManualLauncher {
    fn spawn_io(&mut self, work: Box<dyn FnOnce(&OffFrame) + Send>) -> Option<Box<dyn Retirement>> {
        if self.refused {
            return None;
        }
        let done = Arc::new(AtomicBool::new(false));
        self.jobs.push_back((work, done.clone()));
        Some(Box::new(Done(done)))
    }
}
struct SharedLauncher(Arc<Mutex<ManualLauncher>>);
impl Launcher for SharedLauncher {
    fn spawn_io(&mut self, work: Box<dyn FnOnce(&OffFrame) + Send>) -> Option<Box<dyn Retirement>> {
        self.0.lock().unwrap().spawn_io(work)
    }
}
struct TestState {
    state: State,
    inspector: Arc<FakeInspector>,
    queue: Arc<Mutex<ManualLauncher>>,
}
impl std::ops::Deref for TestState {
    type Target = State;
    fn deref(&self) -> &State {
        &self.state
    }
}
impl std::ops::DerefMut for TestState {
    fn deref_mut(&mut self) -> &mut State {
        &mut self.state
    }
}
fn state() -> TestState {
    let inspector = Arc::new(FakeInspector::default());
    let queue = Arc::new(Mutex::new(ManualLauncher::default()));
    let state = State::with_io(inspector.clone(), Box::new(SharedLauncher(queue.clone())));
    TestState {
        state,
        inspector,
        queue,
    }
}
fn media() -> MediaInfo {
    MediaInfo {
        resolved_source: None,
        hls_playback: None,
        tracks: Default::default(),
        video_codec: "h264".into(),
        audio_codec: "aac".into(),
        width: 960,
        height: 540,
        fps: 15.0,
        duration_ns: 0,
        audio_channels: 2,
        audio_sample_rate: 48000,
        audio_bit_depth: 16,
        dovi: plx_data::metadata::Dovi::NONE,
    }
}
fn proof() -> ReadyOutput {
    let binding = ServiceBinding::from_protected_json(
        &serde_json::json!({
            "server_id":"server_A", "origin":"https://compositor.example.test:443",
            "audience":AUDIENCE, "installation_id":"native_A",
            "control_bearer":"synthetic-private-control-bearer-0123456789"
        })
        .to_string(),
    )
    .unwrap()
    .unwrap();
    let client = Arc::new(
        ServiceClient::new(
            binding,
            "native_A",
            SharedTransport::new(Arc::new(VerifiedHttpsTransport)),
        )
        .unwrap(),
    );
    let canvas = Canvas {
        width: 960,
        height: 540,
        fps: 15,
    };
    let caps = Capabilities {
        version: VERSION,
        server_id: "server_A".into(),
        server_epoch: "epoch_A".into(),
        max_inputs: 4,
        qualified_input_adapters: vec!["http_checked".into()],
        output_profiles: vec![OutputProfile {
            profile_id: "profile_A".into(),
            canvas: canvas.clone(),
            max_inputs: 4,
            geometry_modes: vec![GeometryMode::Disjoint],
        }],
        output_auth: "hmac_path_v1".into(),
        catalog_revision: "catalog_A".into(),
    };
    let native = NativeExpected {
        runtime: "runtime_A".into(),
        expected: NativeRevisions {
            scene: "scene_A".into(),
            layout_revision: "layout_1".into(),
            focus_revision: "focus_1".into(),
            audio_revision: "audio_1".into(),
        },
        slot_id: 0,
        slot_generation: "slot_0_1".into(),
        source_revision: "source_1".into(),
    };
    let status = StatusResponse {
        version: VERSION,
        server_id: "server_A".into(),
        server_epoch: "epoch_A".into(),
        session_id: "session_A".into(),
        native_expected: native,
        backend_expected: BackendExpected {
            runtime: 1,
            scene: "backend_scene_A".into(),
            geometry_revision: 1,
            focus_revision: 1,
            audio_revision: 1,
        },
        state: SessionState::Ready,
        geometry: Geometry {
            canvas,
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
        },
        slots: vec![],
        audio: AudioState {
            policy: AudioPolicy::Muted,
            slot_id: None,
        },
        output: Some(Output {
            session_id: "session_A".into(),
            server_epoch: "epoch_A".into(),
            output_generation: "output_1".into(),
            playlist_path: format!(
                "/multiview/v1/media/{}.{}/index.m3u8",
                "a".repeat(32),
                "b".repeat(64)
            ),
            lease_expires_in_ms: 60_000,
        }),
    };
    ReadyOutput {
        connection_generation: 2,
        client,
        capabilities: caps,
        status,
        lease_deadline: Deadline::after(&OffFrame::for_test(), Duration::from_secs(60)),
        admission_deadline: Deadline::after(&OffFrame::for_test(), Duration::from_secs(20)),
    }
}
fn clone_proof(p: &ReadyOutput) -> ReadyOutput {
    ReadyOutput {
        connection_generation: p.connection_generation,
        client: p.client.clone(),
        capabilities: p.capabilities.clone(),
        status: p.status.clone(),
        lease_deadline: p.lease_deadline,
        admission_deadline: p.admission_deadline,
    }
}
fn start(s: &mut TestState, p: ReadyOutput) -> Result<(), &'static str> {
    s.start(
        Arc::new(p),
        route::worker_ticket(),
        route::play_request_ticket(),
        1,
    )
}
fn scene(p: &ReadyOutput) -> multiview::SceneStatus {
    let n = &p.status.native_expected;
    multiview::SceneStatus {
        runtime: n.runtime.clone(),
        scene: n.expected.scene.clone(),
        layout_revision: n.expected.layout_revision.clone(),
        focus_revision: n.expected.focus_revision.clone(),
        audio_revision: n.expected.audio_revision.clone(),
        slots: vec![],
        admitted_pipeline_slots: 0,
    }
}

#[test]
fn actual_adapter_inspects_only_the_paired_private_hls_without_control_bearer() {
    let mut s = state();
    start(&mut s, proof()).unwrap();
    let (work, done) = s.queue.lock().unwrap().jobs.pop_front().unwrap();
    work(&OffFrame::for_test());
    assert!(s.take_retired().is_none()); // reply does not mean worker retirement
    done.store(true, Ordering::Release);
    let pending = s.take_retired().unwrap();
    assert!(pending.reply.unwrap().result.is_ok());
    let calls = s.inspector.calls.lock().unwrap();
    assert_eq!(calls.len(), 1);
    let ExternalSource::Hls(url) = &calls[0] else {
        panic!("only HLS allowed");
    };
    assert!(url.starts_with("https://compositor.example.test"));
    assert!(url.ends_with("/index.m3u8"));
    assert!(!url.contains("synthetic-private-control"));
}
#[test]
fn cancelled_worker_is_retained_and_replacement_cannot_overlap() {
    let mut s = state();
    start(&mut s, proof()).unwrap();
    let (work, done) = s.queue.lock().unwrap().jobs.pop_front().unwrap();
    s.cancel_pending();
    assert!(start(&mut s, proof()).is_err());
    assert!(s.pending.is_some());
    work(&OffFrame::for_test());
    assert!(s.inspector.calls.lock().unwrap().is_empty());
    assert!(s.take_retired().is_none());
    done.store(true, Ordering::Release);
    let old = s.take_retired().unwrap();
    assert!(old.cancelled.load(Ordering::Acquire));
    start(&mut s, proof()).unwrap();
    assert_eq!(s.queue.lock().unwrap().jobs.len(), 1);
}
#[test]
fn original_expired_lease_prevents_inspection_even_when_task_was_queued() {
    let mut s = state();
    let mut p = proof();
    p.lease_deadline = Deadline::after(&OffFrame::for_test(), Duration::ZERO);
    start(&mut s, p).unwrap();
    let (work, done) = s.queue.lock().unwrap().jobs.pop_front().unwrap();
    work(&OffFrame::for_test());
    done.store(true, Ordering::Release);
    let p = s.take_retired().unwrap();
    assert!(p.reply.unwrap().result.is_err());
    assert!(s.inspector.calls.lock().unwrap().is_empty());
}
#[test]
fn disabled_capacity_bad_geometry_and_refused_spawn_do_not_start_work() {
    let mut s = state();
    let mut p = proof();
    p.capabilities.max_inputs = 0;
    assert!(start(&mut s, p).is_err());
    let mut p = proof();
    p.status.geometry.canvas.width += 1;
    assert!(start(&mut s, p).is_err());
    assert!(s.queue.lock().unwrap().jobs.is_empty());
    s.queue.lock().unwrap().refused = true;
    assert!(start(&mut s, proof()).is_err());
    assert!(s.pending.is_none());
}
#[test]
fn lifetime_keeps_focus_audio_and_sibling_revisions_but_rejects_scene_layout_runtime() {
    let p = proof();
    let mut current = scene(&p);
    assert!(lifetime_matches(&current, &p.status.native_expected));
    current.focus_revision = "focus_2".into();
    current.audio_revision = "audio_2".into();
    // Slots may have been replaced while the same compositor output remains live.
    current.slots.clear();
    assert!(lifetime_matches(&current, &p.status.native_expected));
    current.scene = "other_scene".into();
    assert!(!lifetime_matches(&current, &p.status.native_expected));
    current = scene(&p);
    current.layout_revision = "layout_2".into();
    assert!(!lifetime_matches(&current, &p.status.native_expected));
    current = scene(&p);
    current.runtime = "runtime_B".into();
    assert!(!lifetime_matches(&current, &p.status.native_expected));
}
#[test]
fn renewal_keeps_only_exact_live_picture_binding_and_new_original_deadline() {
    let p = proof();
    let mut newer = clone_proof(&p);
    newer.lease_deadline = Deadline::after(&OffFrame::for_test(), Duration::from_secs(70));
    newer.status.native_expected.expected.focus_revision = "focus_2".into();
    newer.status.native_expected.expected.audio_revision = "audio_2".into();
    newer.status.native_expected.slot_generation = "slot_0_2".into();
    newer.status.native_expected.source_revision = "source_2".into();
    assert!(same_output(&p, &newer));
    newer.status.output.as_mut().unwrap().output_generation = "output_2".into();
    assert!(!same_output(&p, &newer));
    newer = clone_proof(&p);
    newer.connection_generation += 1;
    assert!(!same_output(&p, &newer));
    newer = clone_proof(&p);
    newer.status.session_id = "session_B".into();
    assert!(!same_output(&p, &newer));
    newer = clone_proof(&p);
    newer.status.server_epoch = "epoch_B".into();
    assert!(!same_output(&p, &newer));
    newer = proof();
    assert!(!same_output(&p, &newer)); // equal metadata cannot forge client identity
}
#[test]
fn inspector_redirect_remains_explicit_https_hls_trusted_manifest_boundary() {
    let source = ExternalSource::Hls("https://compositor.example.test/private/index.m3u8".into());
    let mut info = media();
    assert!(checked_source(source.clone(), &info).is_ok()); // simple muxed TS HLS has no rendition object
    info.resolved_source = Some(ExternalSource::Hls(
        "https://trusted-redirect.example.test/video.m3u8".into(),
    ));
    assert!(checked_source(source.clone(), &info).is_ok());
    info.resolved_source = Some(ExternalSource::Hls(
        "http://insecure.example.test/video.m3u8".into(),
    ));
    assert!(checked_source(source.clone(), &info).is_err());
    info.resolved_source = Some(ExternalSource::Http(
        "https://trusted.example.test/file.mp4".into(),
    ));
    assert!(checked_source(source, &info).is_err());
}
#[test]
fn output_generation_overflow_is_permanent_and_cancels_existing_work() {
    let mut s = state();
    start(&mut s, proof()).unwrap();
    s.generation = u64::MAX;
    assert!(s.queue(proof()).is_err());
    assert_eq!(s.generation, 0);
    assert!(s
        .pending
        .as_ref()
        .unwrap()
        .cancelled
        .load(Ordering::Acquire));
    assert!(s.queue(proof()).is_err());
    assert_eq!(s.generation, 0);
}

#[test]
fn full_response_binding_is_required_only_before_output_offer() {
    let p = proof();
    let mut current = scene(&p);
    current.slots.push(multiview::SlotView {
        slot: plx_data::ott::multiview::SlotId::SLOT_0,
        slot_generation: p.status.native_expected.slot_generation.clone(),
        source_revision: p.status.native_expected.source_revision.clone(),
        source: None,
        status: "playing",
        error: None,
    });
    assert!(response_matches(&current, &p.status.native_expected));
    current.focus_revision = "focus_new".into();
    assert!(!response_matches(&current, &p.status.native_expected));
    assert!(lifetime_matches(&current, &p.status.native_expected));
    current.focus_revision = p.status.native_expected.expected.focus_revision.clone();
    current.slots[0].source_revision = "source_new".into();
    assert!(!response_matches(&current, &p.status.native_expected));
    assert!(lifetime_matches(&current, &p.status.native_expected));
}

#[test]
fn geometry_slots_do_not_claim_decoder_capacity() {
    let mut p = proof();
    p.status.geometry.rects = (0..9)
        .map(|slot_id| Rect {
            slot_id,
            x: (u32::from(slot_id) % 3) * 320,
            y: (u32::from(slot_id) / 3) * 180,
            width: 320,
            height: 180,
            z: 0,
        })
        .collect();
    assert_eq!(p.capabilities.max_inputs, 4);
    assert!(qualified_geometry(&p.capabilities, &p.status));
}
#[test]
fn original_operation_expiry_blocks_inspection_independently_of_live_media_lease() {
    let mut s = state();
    let mut p = proof();
    p.admission_deadline = Deadline::after(&OffFrame::for_test(), Duration::ZERO);
    assert!(!p.lease_deadline.expired(&OffFrame::for_test()));
    start(&mut s, p).unwrap();
    let (work, done) = s.queue.lock().unwrap().jobs.pop_front().unwrap();
    work(&OffFrame::for_test());
    done.store(true, Ordering::Release);
    assert!(s.take_retired().unwrap().reply.unwrap().result.is_err());
    assert!(s.inspector.calls.lock().unwrap().is_empty());
}

#[test]
fn actual_server_nonce_dot_signature_lease_is_a_valid_private_output() {
    let p = proof();
    let path = &p.status.output.as_ref().unwrap().playlist_path;
    let lease = path
        .strip_prefix("/multiview/v1/media/")
        .unwrap()
        .strip_suffix("/index.m3u8")
        .unwrap();
    assert_eq!(lease.len(), 97);
    let (nonce, signature) = lease.split_once('.').unwrap();
    assert_eq!(nonce.len(), 32);
    assert_eq!(signature.len(), 64);
    assert!(nonce
        .bytes()
        .chain(signature.bytes())
        .all(|b| b.is_ascii_hexdigit()));
    // Exercise the real ServiceClient/PrivateOutput validation, not a URL bypass.
    assert!(p.client.private_output(&p.capabilities, &p.status).is_ok());
}
