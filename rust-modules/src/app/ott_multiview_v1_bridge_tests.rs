//! Real private-v1 integration gate; requires a separately owned loopback fixture.
//! Real native pipeline against parent-owned FOSS private-v1 fixture responses.
//! Logical HTTPS -> exact owned plaintext loopback is TEST ONLY, not TLS proof.
use super::multiview_service as service;
use super::*;
use plx_base::task::{self, OffFrame, time::Deadline};
use plx_media::{
    ff::{ExternalSource, inspect::MediaInfo},
    player, route,
};
use serde::Deserialize;
use std::{
    collections::VecDeque,
    io::Read,
    net::SocketAddr,
    sync::Mutex,
    time::{Duration, Instant},
};

type Work = Box<dyn FnOnce(&OffFrame) + Send>;
type Retired = Arc<Mutex<Option<Box<dyn multiview_connection::Retirement>>>>;
struct RetirementProxy(Retired);
impl multiview_connection::Retirement for RetirementProxy {
    fn retired(&self) -> bool {
        self.0.lock().unwrap().as_ref().is_some_and(|h| h.retired())
    }
}
#[derive(Clone, Default)]
struct StagedLauncher(Arc<Mutex<VecDeque<(Work, Retired)>>>);
impl multiview_connection::Launcher for StagedLauncher {
    fn spawn_io(&mut self, work: Work) -> Option<Box<dyn multiview_connection::Retirement>> {
        let handle = Arc::new(Mutex::new(None));
        self.0.lock().unwrap().push_back((work, handle.clone()));
        Some(Box::new(RetirementProxy(handle)))
    }
}
impl StagedLauncher {
    fn queued(&self) -> bool {
        !self.0.lock().unwrap().is_empty()
    }
    fn release(&self) {
        use super::multiview_connection::Launcher;
        let mut native = multiview_connection::NativeLauncher;
        loop {
            let job = self.0.lock().unwrap().pop_front();
            let Some((work, target)) = job else {
                return;
            };
            let actual = native.spawn_io(work).expect("real owned off-frame worker");
            *target.lock().unwrap() = Some(actual);
        }
    }
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct Handshake {
    schema_version: u8,
    endpoint: String,
    logical_origin: String,
    server_id: String,
    server_epoch: String,
    installation_id: String,
    control_bearer: String,
    output_profile_id: String,
    canvas: service::Canvas,
    max_inputs: u8,
}
impl Handshake {
    fn load() -> Self {
        let path = std::env::var_os("OTTPLAY_MULTIVIEW_BRIDGE_HANDSHAKE")
            .expect("parent-owned synthetic fixture handshake required; do not skip");
        let file = std::fs::File::open(path).expect("open owned handshake");
        let metadata = file.metadata().expect("handshake metadata");
        assert!(metadata.is_file() && metadata.len() <= 16_384);
        let mut bytes = Vec::new();
        file.take(16_385)
            .read_to_end(&mut bytes)
            .expect("read owned handshake");
        assert!(bytes.len() <= 16_384);
        let cfg: Self = serde_json::from_slice(&bytes).expect("closed fixture handshake");
        assert_eq!(cfg.schema_version, 1);
        let socket: SocketAddr = cfg
            .endpoint
            .strip_prefix("http://")
            .expect("owned plaintext fixture only")
            .parse()
            .expect("literal socket address only");
        assert!(socket.ip().is_loopback() && socket.port() != 0);
        assert_eq!(cfg.endpoint, format!("http://{socket}"));
        assert!(
            cfg.control_bearer.starts_with("synthetic-")
                || cfg.control_bearer.starts_with("fixture_"),
            "only parent-owned synthetic credentials are allowed"
        );
        assert!(
            !cfg.server_epoch.is_empty() && !cfg.output_profile_id.is_empty() && cfg.max_inputs > 0
        );
        for key in [
            "HTTP_PROXY",
            "HTTPS_PROXY",
            "ALL_PROXY",
            "http_proxy",
            "https_proxy",
            "all_proxy",
        ] {
            assert!(
                std::env::var_os(key).is_none_or(|v| v.is_empty()),
                "fixture proxy environment must be cleared"
            );
        }
        cfg
    }
    fn protected(&self) -> Value {
        json!(
            json!({"server_id":self.server_id,"origin":self.logical_origin,
            "audience":service::AUDIENCE,"installation_id":self.installation_id,
            "control_bearer":self.control_bearer})
            .to_string()
        )
    }
    fn suffix<'a>(&self, logical: &'a str) -> Result<&'a str, service::ServiceError> {
        let path = logical
            .strip_prefix(&self.logical_origin)
            .ok_or(service::ServiceError::InvalidRequest)?;
        if !path.starts_with("/multiview/v1/") || path.contains(['?', '#', '%', '\\']) {
            return Err(service::ServiceError::InvalidRequest);
        }
        Ok(path)
    }
}
struct OwnedTransport {
    cfg: Arc<Handshake>,
    // Only fixed control endpoint names; never body, bearer or signed media URL.
    calls: Mutex<Vec<String>>,
    committed_native: Mutex<Option<Value>>,
    next_controls: Mutex<Vec<Value>>,
}
fn status_error(status: u16) -> service::ServiceError {
    match status {
        401 => service::ServiceError::Unauthorized,
        403 => service::ServiceError::Forbidden,
        409 => service::ServiceError::RevisionConflict,
        410 => service::ServiceError::AuthorityRevoked,
        413 | 429 => service::ServiceError::ResourceLimit,
        502..=504 => service::ServiceError::BackendUnavailable,
        code => service::ServiceError::HttpStatus(code),
    }
}
impl OwnedTransport {
    fn exchange(
        &self,
        worker: &OffFrame,
        path: &str,
        method: &str,
        body: Option<&[u8]>,
        headers: &[String],
        deadline: Deadline,
        cancelled: &AtomicBool,
        limit: usize,
    ) -> Result<Vec<u8>, service::ServiceError> {
        if cancelled.load(Ordering::Acquire) {
            return Err(service::ServiceError::Cancelled);
        }
        let remaining = deadline.remaining(worker);
        if remaining.is_zero() {
            return Err(service::ServiceError::Deadline);
        }
        let mut bytes = Vec::new();
        let mut too_large = false;
        let mut sink = |chunk: &[u8]| {
            if bytes.len().saturating_add(chunk.len()) > limit {
                too_large = true;
                return false;
            }
            bytes.extend_from_slice(chunk);
            true
        };
        let end = plx_net::net::stream_request_tls_with_timeouts(
            &format!("{}{}", self.cfg.endpoint, path),
            headers,
            method,
            body,
            plx_net::net::Tls::Ca,
            None,
            plx_net::net::Timeouts {
                connect_s: 2,
                total_s: 0,
                total_ms: i64::try_from(remaining.as_millis())
                    .unwrap_or(i64::MAX)
                    .max(1),
                low_speed_bps: 0,
                low_speed_s: 0,
            },
            Duration::from_secs(3),
            cancelled,
            &mut sink,
        );
        if too_large {
            return Err(service::ServiceError::ResponseTooLarge);
        }
        if cancelled.load(Ordering::Acquire) {
            return Err(service::ServiceError::Cancelled);
        }
        match end {
            plx_net::net::StreamEnd::Closed => Ok(bytes),
            plx_net::net::StreamEnd::Status(code) => Err(status_error(code)),
            plx_net::net::StreamEnd::Cancelled => Err(service::ServiceError::Cancelled),
            plx_net::net::StreamEnd::Idle => Err(service::ServiceError::Deadline),
            plx_net::net::StreamEnd::Unsupported | plx_net::net::StreamEnd::Refused => {
                Err(service::ServiceError::Unsupported)
            }
            plx_net::net::StreamEnd::Consumer => Err(service::ServiceError::InvalidResponse),
            plx_net::net::StreamEnd::Transport(_) => Err(service::ServiceError::Transport),
        }
    }
}
impl service::Transport for OwnedTransport {
    fn send(
        &self,
        worker: &OffFrame,
        binding: &service::ServiceBinding,
        request: &service::ServiceRequest,
        deadline: Deadline,
        cancelled: &AtomicBool,
    ) -> Result<Vec<u8>, service::ServiceError> {
        // Read the ACTUAL protected binding in memory; never override its token.
        let bound: Value = serde_json::from_str(&binding.protected_json()?)
            .map_err(|_| service::ServiceError::InvalidConfig)?;
        if bound["origin"] != self.cfg.logical_origin
            || bound["server_id"] != self.cfg.server_id
            || bound["installation_id"] != self.cfg.installation_id
            || bound["audience"] != service::AUDIENCE
            || bound["control_bearer"] != self.cfg.control_bearer
        {
            return Err(service::ServiceError::Unauthorized);
        }
        let url = request.url(binding);
        let path = self.cfg.suffix(&url)?;
        if !matches!(
            path,
            "/multiview/v1/capabilities"
                | "/multiview/v1/sessions"
                | "/multiview/v1/status"
                | "/multiview/v1/sources"
                | "/multiview/v1/input-grants"
                | "/multiview/v1/control"
        ) {
            return Err(service::ServiceError::InvalidRequest);
        }
        let actual = bound["control_bearer"]
            .as_str()
            .ok_or(service::ServiceError::InvalidConfig)?;
        let mut headers = vec![format!("Authorization: Bearer {actual}")];
        if request.body().is_some() {
            headers.push("Content-Type: application/json".into());
        }
        if let Some(id) = request.request_id() {
            headers.push(format!("X-Request-Id: {id}"));
        }
        self.calls.lock().unwrap().push(path.to_owned());
        let response = self.exchange(
            worker,
            path,
            request.method(),
            request.body(),
            &headers,
            deadline,
            cancelled,
            service::MAX_RESULT_BYTES,
        )?;
        let observed: Value = serde_json::from_slice(&response)
            .map_err(|_| service::ServiceError::InvalidResponse)?;
        if path == "/multiview/v1/status" {
            *self.committed_native.lock().unwrap() = observed.get("native_expected").cloned();
        }
        if path == "/multiview/v1/control" {
            let sent: Value = serde_json::from_slice(request.body().unwrap_or(b"{}"))
                .map_err(|_| service::ServiceError::InvalidRequest)?;
            self.next_controls
                .lock()
                .unwrap()
                .push(sent.get("next_native").cloned().unwrap_or(Value::Null));
        }
        Ok(response)
    }
}
struct MappedFixtureInspector {
    transport: Arc<OwnedTransport>,
    completed_reads: Arc<std::sync::atomic::AtomicUsize>,
}
impl multiview_output::Inspector for MappedFixtureInspector {
    fn inspect(
        &self,
        worker: &OffFrame,
        source: &ExternalSource,
        deadline: Deadline,
        cancelled: Arc<AtomicBool>,
    ) -> Result<MediaInfo, &'static str> {
        let ExternalSource::Hls(logical) = source else {
            return Err("Expected owned fixture HLS");
        };
        let path = self
            .transport
            .cfg
            .suffix(logical)
            .map_err(|_| "Unexpected fixture output origin")?;
        let tail = path
            .strip_prefix("/multiview/v1/media/")
            .ok_or("Invalid fixture media path")?;
        let parts: Vec<_> = tail.split('/').collect();
        if parts.len() != 2
            || parts[1] != "index.m3u8"
            || parts[0].is_empty()
            || !parts[0]
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
        {
            return Err("Invalid fixture media lease");
        }
        let manifest = self
            .transport
            .exchange(worker, path, "GET", None, &[], deadline, &cancelled, 65_536)
            .map_err(|_| "Fixture manifest unavailable")?;
        let text = std::str::from_utf8(&manifest).map_err(|_| "Invalid fixture manifest")?;
        if !text.starts_with("#EXTM3U") {
            return Err("Not a fixture playlist");
        }
        let segment = text
            .lines()
            .find(|line| !line.is_empty() && !line.starts_with('#'))
            .ok_or("Fixture playlist has no segment")?;
        let digits = segment
            .strip_prefix("segment-")
            .and_then(|v| v.strip_suffix(".ts"))
            .ok_or("Unconfined fixture child")?;
        if digits.len() != 6 || !digits.bytes().all(|b| b.is_ascii_digit()) {
            return Err("Invalid fixture child");
        }
        let child = format!("/multiview/v1/media/{}/{}", parts[0], segment);
        let bytes = self
            .transport
            .exchange(
                worker,
                &child,
                "GET",
                None,
                &[],
                deadline,
                &cancelled,
                8_388_608,
            )
            .map_err(|_| "Fixture segment unavailable")?;
        if bytes.is_empty()
            || bytes.len() % 188 != 0
            || bytes.chunks_exact(188).any(|p| p[0] != 0x47)
        {
            return Err("Invalid fixture TS framing");
        }
        self.completed_reads.fetch_add(1, Ordering::AcqRel);
        // Explicit inspection injection: actual HLS/TS were read, but metadata
        // below is the declared synthetic fixture profile, NOT native demux proof.
        Ok(MediaInfo {
            resolved_source: None,
            hls_playback: None,
            tracks: Default::default(),
            video_codec: "h264".into(),
            audio_codec: "aac".into(),
            width: self
                .transport
                .cfg
                .canvas
                .width
                .try_into()
                .map_err(|_| "Fixture width too large")?,
            height: self
                .transport
                .cfg
                .canvas
                .height
                .try_into()
                .map_err(|_| "Fixture height too large")?,
            fps: f64::from(self.transport.cfg.canvas.fps),
            duration_ns: 0,
            audio_channels: 2,
            audio_sample_rate: 48000,
            audio_bit_depth: 16,
            dovi: plx_data::metadata::Dovi::NONE,
        })
    }
}
struct BridgeRig {
    app: Box<App>,
    jobs: StagedLauncher,
    transport: Arc<OwnedTransport>,
    reads: Arc<std::sync::atomic::AtomicUsize>,
    saved: Value,
}
impl Drop for BridgeRig {
    fn drop(&mut self) {
        player::SHARED.duration_ns.store(0, Ordering::Release);
        player::stop_bufferfeed(&mut self.app.player.session, &mut self.app.adapters.player);
        player::TX.reset();
        route::reset_player_control_for_test(&self.app.player.session);
    }
}
fn tick_bridge(app: &mut App) {
    let sample = task::time::ClockSample::capture_main(app.adapters.player.mt()).unwrap();
    tick_with_start(app, sample, |_, _, _, _, _| {
        panic!("No ordinary native source may start in this fixture")
    });
}
fn bridge_setup() -> BridgeRig {
    let cfg = Arc::new(Handshake::load());
    assert!(
        plx_net::net::global_init()
            && plx_net::net::available()
            && plx_net::net::threaded_tls_ready()
    );
    let mut app = Box::new(crate::app::run::lifecycle_regression_tests::app());
    app.running = true;
    player::TX.reset();
    route::reset_player_control_for_test(&app.player.session);
    let mut runtime = Runtime::unloaded(crate::app::ott_companion::Supervisor::inert_for_test());
    runtime.loaded = true;
    runtime.state_load = StateLoad::Idle;
    runtime.entropy = Some([1; 16]);
    runtime.resume_source_work();
    runtime.remote = Remote::disabled_for_test(&cfg.installation_id, "runtime_real_v1_bridge");
    let sample = task::time::ClockSample::capture_main(app.adapters.player.mt()).unwrap();
    runtime.clock_sample = Some(sample);
    runtime.clock_anchor = Some(sample);
    let protected = cfg.protected();
    runtime
        .saved
        .settings
        .insert("nativeMultiviewService".into(), protected.clone());
    let transport = Arc::new(OwnedTransport {
        cfg: cfg.clone(),
        calls: Mutex::new(Vec::new()),
        committed_native: Mutex::new(None),
        next_controls: Mutex::new(Vec::new()),
    });
    runtime.multiview_connection = multiview_connection::State::with_io(
        transport.clone(),
        Box::new(multiview_connection::NativeLauncher),
    );
    let until = Instant::now() + Duration::from_secs(10);
    while runtime.multiview_connection.qualified().is_none() {
        assert!(Instant::now() < until, "Owned capabilities worker deadline");
        runtime.multiview_connection.tick(
            true,
            true,
            Some(&protected),
            runtime.remote.persisted_device_id(),
        );
        std::thread::sleep(Duration::from_millis(1));
    }
    let caps = &runtime
        .multiview_connection
        .qualified()
        .unwrap()
        .capabilities;
    assert_eq!(caps.server_epoch, cfg.server_epoch);
    assert_eq!(caps.max_inputs, cfg.max_inputs);
    assert!(
        caps.output_profiles
            .iter()
            .any(|p| p.profile_id == cfg.output_profile_id && p.canvas == cfg.canvas)
    );
    let jobs = StagedLauncher::default();
    let reads = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    runtime.multiview_output = multiview_output::State::with_io(
        Arc::new(MappedFixtureInspector {
            transport: transport.clone(),
            completed_reads: reads.clone(),
        }),
        Box::new(jobs.clone()),
    );
    let saved = serde_json::to_value(&runtime.saved).unwrap();
    app.ott = Some(runtime);
    BridgeRig {
        app,
        jobs,
        transport,
        reads,
        saved,
    }
}
fn dispatch_bridge(rig: &mut BridgeRig, serial: u64, op: &str, layout: bool) -> String {
    let mut runtime = rig.app.ott.take().unwrap();
    let expected = runtime.multiview.scene_status(&runtime).map(|s| json!({"scene":s.scene,
        "layout_revision":s.layout_revision,"focus_revision":s.focus_revision,"audio_revision":s.audio_revision}))
        .unwrap_or_else(|| json!({"scene":"off","layout_revision":"off","focus_revision":"off","audio_revision":"off"}));
    let mut params =
        json!({"version":1,"runtime":runtime.remote.runtime(),"op":op,"expected":expected});
    if layout {
        params["layout"] = json!("grid4");
    }
    if op == "audio" {
        params["policy"] = json!("muted");
    }
    if op == "focus" {
        let scene = runtime
            .multiview
            .scene_status(&runtime)
            .expect("active native scene");
        let slot = scene
            .slots
            .iter()
            .find(|s| s.slot.index() == 1)
            .expect("visible native slot");
        params["slot_id"] = json!(1);
        params["slot_generation"] = json!(slot.slot_generation);
        params["source_revision"] = json!(slot.source_revision);
    }
    let now = runtime.now_ms();
    let mut incoming = runtime.remote.raw_poll_for_test(&json!({"commands":[],"server_time":1000,
        "requests":[{"id":format!("{serial:032x}"),"action":"multiview","params":params,"expires_at":1100}]}).to_string(), now);
    assert_eq!(incoming.len(), 1);
    let inv = incoming.remove(0);
    let id = inv.id.clone();
    let reply = commands::command(&mut runtime, &mut rig.app, &inv);
    if let Some(reply) = reply {
        runtime.remote.complete(inv, reply);
    }
    rig.app.ott = Some(runtime);
    id
}
fn wait_actual_native_status(rig: &mut BridgeRig, id: &str) {
    let end = Instant::now() + Duration::from_secs(10);
    loop {
        assert!(
            Instant::now() < end,
            "actual post-transition FOSS status deadline"
        );
        tick_bridge(&mut rig.app);
        let runtime = rig.app.ott.as_ref().unwrap();
        let scene = runtime.multiview.scene_status(runtime).unwrap();
        let actual = rig.transport.committed_native.lock().unwrap().clone();
        let matched = actual.is_some_and(|n| {
            n["runtime"] == scene.runtime
                && n["expected"]["scene"] == scene.scene
                && n["expected"]["layout_revision"] == scene.layout_revision
                && n["expected"]["focus_revision"] == scene.focus_revision
                && n["expected"]["audio_revision"] == scene.audio_revision
        });
        let replied = runtime
            .remote
            .response_for_id_for_test(id)
            .is_some_and(|r| r["status"] == "ok");
        if matched && replied {
            return;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
}
#[test]
#[ignore = "requires an explicitly owned private-v1 fixture listener and handshake"]
fn actual_native_pipeline_against_owned_foss_v1_reaches_and_revokes_deferred_admission() {
    let _serial = plx_base::testlock::serial();
    let mut rig = bridge_setup();
    let id = dispatch_bridge(&mut rig, 0x7101, "layout", true);
    let until = Instant::now() + Duration::from_secs(15);
    while !rig.jobs.queued() {
        assert!(
            Instant::now() < until,
            "Actual FOSS Ready and output offer deadline"
        );
        tick_bridge(&mut rig.app);
        std::thread::sleep(Duration::from_millis(1));
    }
    assert!(!route::has_url(&rig.app.player.session));
    assert!(!rig.app.adapters.player.is_live());
    rig.app
        .adapters
        .player
        .begin_release_for_test(Duration::ZERO);
    rig.jobs.release(); // actual off-frame network worker; retirement proxy observes actual handle
    while rig.app.ott.as_ref().unwrap().multiview_output.status != "admitted" {
        assert!(
            Instant::now() < until,
            "Owned fixture inspection and native admission deadline"
        );
        tick_bridge(&mut rig.app);
        std::thread::sleep(Duration::from_millis(1));
    }
    assert!(rig.reads.load(Ordering::Acquire) > 0);
    assert!(rig.app.ott.as_ref().unwrap().multiview_picture.is_some());
    assert!(foreign_native_start_allowed(&mut rig.app));
    assert!(rig.app.adapters.player.release_has_queued_start());
    assert!(!rig.app.adapters.player.is_live());
    assert_eq!(
        rig.app
            .ott
            .as_ref()
            .unwrap()
            .remote
            .response_for_id_for_test(&id)
            .unwrap()["status"],
        "ok"
    );
    crate::app::run::publish_playback_frame(&mut rig.app, false);
    assert!(rig.app.adapters.player.release_has_queued_start());
    assert!(!rig.app.adapters.player.is_live());
    assert_eq!(
        serde_json::to_value(&rig.app.ott.as_ref().unwrap().saved).unwrap(),
        rig.saved
    );
    for endpoint in [
        "/multiview/v1/capabilities",
        "/multiview/v1/sessions",
        "/multiview/v1/status",
    ] {
        assert!(
            rig.transport
                .calls
                .lock()
                .unwrap()
                .iter()
                .any(|p| p == endpoint)
        );
    }
    // These execute real private-v1 next_native transitions. Mute first because
    // this minimal live Scene is intentionally empty and has no source audio.
    let picture = route::playback_identity(&rig.app.player.session).unwrap();
    let audio_id = dispatch_bridge(&mut rig, 0x7102, "audio", false);
    wait_actual_native_status(&mut rig, &audio_id);
    let focus_id = dispatch_bridge(&mut rig, 0x7103, "focus", false);
    wait_actual_native_status(&mut rig, &focus_id);
    assert!(
        rig.transport
            .next_controls
            .lock()
            .unwrap()
            .iter()
            .filter(|n| !n.is_null())
            .count()
            >= 2,
        "new native transitions must be sent, never repaired by fixture transport"
    );
    assert_eq!(
        route::playback_identity(&rig.app.player.session),
        Some(picture)
    );
    assert!(foreign_native_start_allowed(&mut rig.app));
    assert!(rig.app.adapters.player.release_has_queued_start());
    assert!(!rig.app.adapters.player.is_live());
    // A local pairing change cannot borrow the actual server Ready/native proof.
    {
        let runtime = rig.app.ott.as_mut().unwrap();
        runtime.multiview_connection.tick(
            true,
            true,
            runtime.saved.settings.get("nativeMultiviewService"),
            Some("different_fixture_installation"),
        );
    }
    assert!(!foreign_native_start_allowed(&mut rig.app));
    crate::app::run::publish_playback_frame(&mut rig.app, false);
    assert!(!rig.app.adapters.player.release_has_queued_start());
    assert!(!rig.app.adapters.player.is_live());
    // Parent remains sole fixture/Scene owner and must close/reap even on test failure.
}
