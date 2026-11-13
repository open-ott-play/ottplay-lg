use super::*;
use std::{collections::VecDeque, sync::Mutex};

#[derive(Default)]
struct FakeTransport {
    calls: Mutex<usize>,
    urls: Mutex<Vec<String>>,
    replies: Mutex<VecDeque<Result<Vec<u8>, ServiceError>>>,
}
impl Transport for FakeTransport {
    fn send(
        &self,
        _: &OffFrame,
        binding: &ServiceBinding,
        request: &ServiceRequest,
        _: Deadline,
        _: &AtomicBool,
    ) -> Result<Vec<u8>, ServiceError> {
        *self.calls.lock().unwrap() += 1;
        self.urls.lock().unwrap().push(request.url(binding));
        self.replies
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(Err(ServiceError::Transport))
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
    queued: VecDeque<(Work, Arc<AtomicBool>)>,
    refused: bool,
}
impl Launcher for ManualLauncher {
    fn spawn_io(&mut self, work: Work) -> Option<Box<dyn Retirement>> {
        if self.refused {
            return None;
        }
        let done = Arc::new(AtomicBool::new(false));
        self.queued.push_back((work, done.clone()));
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
    transport: Arc<FakeTransport>,
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
    let transport = Arc::new(FakeTransport::default());
    let queue = Arc::new(Mutex::new(ManualLauncher::default()));
    let state = State::with_io(transport.clone(), Box::new(SharedLauncher(queue.clone())));
    TestState {
        state,
        transport,
        queue,
    }
}
fn config() -> Value {
    Value::String(
        serde_json::json!({"server_id":"server_A","origin":"https://compositor.example.test",
        "audience":"ottplay-multiview-v1","installation_id":"native_A",
        "control_bearer":"synthetic-private-control-bearer-0123456789"})
        .to_string(),
    )
}
fn reply(state: &TestState, epoch: &str) {
    let body = serde_json::json!({"version":1,"server_id":"server_A","server_epoch":epoch,
        "max_inputs":0,"qualified_input_adapters":[],"output_profiles":[],
        "output_auth":"hmac_path_v1","catalog_revision":"catalog_A"});
    state
        .transport
        .replies
        .lock()
        .unwrap()
        .push_back(Ok(serde_json::to_vec(&body).unwrap()));
}
fn tick(state: &mut TestState) {
    state.tick(true, true, Some(&config()), Some("native_A"));
}
fn finish(state: &mut TestState) {
    let (work, done) = state.queue.lock().unwrap().queued.pop_front().unwrap();
    work(&OffFrame::for_test());
    done.store(true, Ordering::Release);
}
#[test]
fn disabled_background_and_not_ready_do_no_work() {
    let mut s = state();
    s.tick(true, true, None, Some("native_A"));
    assert_eq!(s.status, "disabled");
    s.tick(false, true, Some(&config()), Some("native_A"));
    s.tick(true, false, Some(&config()), Some("native_A"));
    assert!(s.pending.is_none());
    assert!(s.queue.lock().unwrap().queued.is_empty());
    assert_eq!(*s.transport.calls.lock().unwrap(), 0);
}
#[test]
fn invalid_and_unpaired_are_closed_without_network() {
    let mut s = state();
    s.tick(
        true,
        true,
        Some(&Value::String("private-secret-invalid-json".into())),
        Some("native_A"),
    );
    assert_eq!(s.error, Some("invalid_config"));
    s.tick(true, true, Some(&config()), Some("another_native"));
    assert_eq!(s.error, Some("unpaired_installation"));
    s.tick(true, true, Some(&config()), None);
    assert_eq!(s.error, Some("unpaired_installation"));
    assert!(s.pending.is_none());
}
#[test]
fn authenticated_zero_capacity_is_available_metadata_not_playback_ready() {
    let mut s = state();
    reply(&s, "epoch_A");
    tick(&mut s);
    let generation = s.generation();
    tick(&mut s);
    assert_eq!(s.queue.lock().unwrap().queued.len(), 1);
    finish(&mut s);
    tick(&mut s);
    assert_eq!(s.status, "backend_unavailable");
    let q = s.qualified().unwrap();
    assert_eq!(q.generation, generation);
    assert_eq!(q.capabilities.max_inputs, 0);
    assert_eq!(q.capabilities.server_epoch, "epoch_A");
    tick(&mut s);
    assert_eq!(*s.transport.calls.lock().unwrap(), 1);
}
#[test]
fn cancellation_retains_worker_until_retirement_before_replacement() {
    let mut s = state();
    reply(&s, "epoch_A");
    tick(&mut s);
    let (work, done) = s.queue.lock().unwrap().queued.pop_front().unwrap();
    // Work has sent its reply but has not retired yet.
    work(&OffFrame::for_test());
    s.tick(true, false, Some(&config()), Some("native_A"));
    assert!(s.pending.is_some());
    assert!(s.qualified().is_none());
    tick(&mut s);
    assert!(s.queue.lock().unwrap().queued.is_empty());
    done.store(true, Ordering::Release);
    tick(&mut s);
    assert_eq!(s.queue.lock().unwrap().queued.len(), 1);
    assert!(s.qualified().is_none());
}
#[test]
fn identity_and_exact_config_changes_cancel_old_generation() {
    let mut s = state();
    tick(&mut s);
    let generation = s.generation();
    let cancel = s.pending.as_ref().unwrap().cancelled.clone();
    s.tick(true, true, Some(&config()), Some("another_native"));
    assert!(cancel.load(Ordering::Acquire));
    assert!(s.generation() > generation);
    finish(&mut s);
    s.tick(true, true, Some(&config()), Some("another_native"));
    assert_eq!(s.error, Some("unpaired_installation"));
    tick(&mut s);
    assert!(s.pending.is_some());
    let mut changed = config().as_str().unwrap().to_owned();
    changed.push(' ');
    let generation = s.generation();
    s.tick(true, true, Some(&Value::String(changed)), Some("native_A"));
    assert!(s.generation() > generation);
    assert!(s
        .pending
        .as_ref()
        .unwrap()
        .cancelled
        .load(Ordering::Acquire));
}
#[test]
fn refresh_clears_old_epoch_client_and_accepts_only_new_generation() {
    let mut s = state();
    reply(&s, "epoch_A");
    tick(&mut s);
    finish(&mut s);
    tick(&mut s);
    let old = s.generation();
    assert!(s.qualified().is_some());
    s.refresh();
    assert!(s.qualified().is_none());
    assert!(s.generation() > old);
    reply(&s, "epoch_B");
    tick(&mut s);
    finish(&mut s);
    tick(&mut s);
    assert_eq!(s.qualified().unwrap().capabilities.server_epoch, "epoch_B");
}
#[test]
fn generation_overflow_permanently_disables_work() {
    let mut s = state();
    s.generation = u64::MAX;
    tick(&mut s);
    assert_eq!(s.generation(), 0);
    assert!(s.pending.is_none());
    s.refresh();
    s.tick(true, false, None, None);
    tick(&mut s);
    assert_eq!(s.generation(), 0);
    assert!(s.pending.is_none());
    assert_eq!(s.error, Some("generation_exhausted"));
}
#[test]
fn refused_worker_and_transport_errors_never_retry_automatically() {
    let mut s = state();
    s.queue.lock().unwrap().refused = true;
    tick(&mut s);
    assert_eq!(s.error, Some("service_unavailable"));
    tick(&mut s);
    assert!(s.pending.is_none());
    s.queue.lock().unwrap().refused = false;
    s.refresh();
    tick(&mut s);
    finish(&mut s);
    tick(&mut s);
    assert_eq!(s.error, Some("service_unavailable"));
    tick(&mut s);
    assert_eq!(*s.transport.calls.lock().unwrap(), 1);
}

#[test]
fn background_invalidates_authenticated_caps_and_same_config_foreground_rebinds() {
    let mut s = state();
    reply(&s, "epoch_A");
    tick(&mut s);
    finish(&mut s);
    tick(&mut s);
    let generation = s.generation();
    assert!(s.qualified().is_some());
    s.tick(true, false, Some(&config()), Some("native_A"));
    assert!(s.qualified().is_none());
    assert_eq!(s.status, "idle");
    assert!(s.generation() > generation);
    assert!(s.pending.is_none());
    reply(&s, "epoch_B");
    tick(&mut s);
    assert_eq!(s.queue.lock().unwrap().queued.len(), 1);
    finish(&mut s);
    tick(&mut s);
    assert_eq!(s.qualified().unwrap().capabilities.server_epoch, "epoch_B");
    assert_eq!(*s.transport.calls.lock().unwrap(), 2);
}

#[test]
fn disconnected_reply_is_not_worker_retirement() {
    let mut s = state();
    tick(&mut s);
    let (work, done) = s.queue.lock().unwrap().queued.pop_front().unwrap();
    drop(work); // Worker failed before replying, but has not retired yet.
    tick(&mut s);
    assert!(s.pending.is_some());
    assert!(s.queue.lock().unwrap().queued.is_empty());
    done.store(true, Ordering::Release);
    tick(&mut s);
    assert!(s.pending.is_none());
    assert_eq!(s.error, Some("service_unavailable"));
    tick(&mut s);
    assert!(s.pending.is_none());
}

#[test]
fn cancelled_before_dispatch_never_calls_transport() {
    let mut s = state();
    tick(&mut s);
    s.tick(false, false, Some(&config()), Some("native_A"));
    finish(&mut s);
    s.tick(false, false, Some(&config()), Some("native_A"));
    assert_eq!(*s.transport.calls.lock().unwrap(), 0);
    assert!(s.pending.is_none());
    assert!(s.qualified().is_none());
}

#[test]
fn captured_client_never_regains_currentness_after_refresh_background_or_rebinding() {
    let mut s = state();
    reply(&s, "epoch_A");
    tick(&mut s);
    finish(&mut s);
    tick(&mut s);
    let first = s.qualified().unwrap();
    let old_generation = first.generation;
    let old_client = Arc::clone(&first.client);
    assert!(s.is_current(old_generation, "epoch_A", &old_client));
    assert!(!s.is_current(old_generation, "different_epoch", &old_client));
    s.refresh();
    assert!(!s.is_current(old_generation, "epoch_A", &old_client));
    reply(&s, "epoch_B");
    tick(&mut s);
    finish(&mut s);
    tick(&mut s);
    let second = s.qualified().unwrap();
    let second_generation = second.generation;
    let second_client = Arc::clone(&second.client);
    assert!(s.is_current(second_generation, "epoch_B", &second_client));
    // Generation and epoch alone cannot launder a previously authenticated client.
    assert!(!s.is_current(second_generation, "epoch_B", &old_client));
    s.tick(true, false, Some(&config()), Some("native_A"));
    assert!(!s.is_current(second_generation, "epoch_B", &second_client));
    let mut replacement: Value = serde_json::from_str(config().as_str().unwrap()).unwrap();
    replacement["origin"] = Value::String("https://replacement.example.test".into());
    let replacement = Value::String(replacement.to_string());
    reply(&s, "epoch_C");
    s.tick(true, true, Some(&replacement), Some("native_A"));
    finish(&mut s);
    s.tick(true, true, Some(&replacement), Some("native_A"));
    let third = s.qualified().unwrap();
    assert!(s.is_current(third.generation, "epoch_C", &third.client));
    assert!(!s.is_current(third.generation, "epoch_C", &second_client));
    assert!(!s.is_current(old_generation, "epoch_A", &old_client));
    assert_eq!(
        s.transport.urls.lock().unwrap().as_slice(),
        [
            "https://compositor.example.test/multiview/v1/capabilities",
            "https://compositor.example.test/multiview/v1/capabilities",
            "https://replacement.example.test/multiview/v1/capabilities",
        ]
    );
}

#[test]
fn terminal_shutdown_retains_cancelled_reply_until_retirement_and_never_rediscovers() {
    let mut s = state();
    reply(&s, "epoch_A");
    tick(&mut s);
    let (work, done) = s.queue.lock().unwrap().queued.pop_front().unwrap();
    work(&OffFrame::for_test()); // reply exists, but worker has not retired
    s.shutdown();
    assert!(s.qualified().is_none());
    assert!(s.shutdown_pending());
    assert!(s
        .pending
        .as_ref()
        .unwrap()
        .cancelled
        .load(Ordering::Acquire));
    done.store(true, Ordering::Release);
    assert!(!s.shutdown_pending());
    s.refresh();
    tick(&mut s);
    assert!(s.queue.lock().unwrap().queued.is_empty());
    assert!(s.qualified().is_none());
    assert_eq!(*s.transport.calls.lock().unwrap(), 1);
}
