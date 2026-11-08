//! Private diagnostic request owner. No polling file, socket, or public control endpoint.
//! Live callers are gated to devtriggers + native Linux ARM. Image publication uses
//! the existing guarded developer export; the UI thread only reads one bounded frame.
use super::ui_snapshot_capture::{
    self as capture, Capture, Completion, Eligibility, Failure, Viewport,
};
use super::ui_snapshot_request::{self as request, Launch, Request, RequestId};
use plx_platform::storage::diagnostics::ui_snapshot as export;
use std::sync::{mpsc, Mutex, OnceLock};
use std::thread::JoinHandle;

const REQUEST_LIMIT: usize = 32;
const RETAIN_MS: u32 = 60_000;

struct Pending {
    request: Request,
    capture: Option<Capture>,
}
struct Worker {
    handle: JoinHandle<()>,
    result: mpsc::Receiver<bool>,
    clearing: bool,
}
impl Worker {
    fn start(clearing: bool, work: impl FnOnce() -> bool + Send + 'static) -> Option<Self> {
        let (sender, result) = mpsc::sync_channel(1);
        let handle = plx_base::task::spawn("ui-snapshot", move || {
            let _ = sender.send(work());
        })?;
        Some(Self {
            handle,
            result,
            clearing,
        })
    }
}
#[derive(Default)]
struct Owner {
    seen: Vec<RequestId>,
    pending: Option<Pending>,
    worker: Option<Worker>,
    published_at: Option<u32>,
}
fn owner() -> &'static Mutex<Owner> {
    static OWNER: OnceLock<Mutex<Owner>> = OnceLock::new();
    OWNER.get_or_init(|| Mutex::new(Owner::default()))
}
fn note(message: &'static str) {
    plx_base::eventlog::log(message);
}

impl Owner {
    fn reap(&mut self, now: u32) {
        if self.worker.as_ref().is_some_and(|w| w.handle.is_finished()) {
            let worker = self.worker.take().unwrap();
            let ok = worker.handle.join().is_ok() && worker.result.try_recv().unwrap_or(false);
            // A failed clear is retried after the same bounded retention interval.
            self.published_at = if worker.clearing && ok {
                None
            } else {
                Some(now)
            };
            note(if ok {
                "ui-snapshot export=complete"
            } else {
                "ui-snapshot export=unavailable"
            });
        }
    }
    fn admit(&mut self, request: Request, now: Option<u32>) -> bool {
        if let Some(now) = now {
            self.reap(now);
        }
        // A request id is never captured twice in this process. Bounded history does
        // not evict old ids: after 32 admissions a new process is required.
        if self.seen.contains(&request.request_id) {
            note("ui-snapshot request=duplicate");
            return false;
        }
        if self.pending.is_some() || self.worker.is_some() {
            note("ui-snapshot request=busy");
            return false;
        }
        if self.seen.len() == REQUEST_LIMIT {
            note("ui-snapshot request=quota");
            return false;
        }
        self.seen.push(request.request_id);
        self.pending = Some(Pending {
            request,
            capture: now.map(Capture::new),
        });
        // Keep the previous artifact expiry until a replacement is actually published.
        true
    }
    fn publish(&mut self, pending: Pending, outcome: Completion) {
        debug_assert!(self.worker.is_none());
        let worker = Worker::start(false, move || {
            let result = match outcome {
                Ok(frame) => {
                    let (width, height, source_ticks) =
                        (frame.width, frame.height, frame.source_ticks);
                    match capture::encode_png(frame) {
                        Ok(png) => export::publish(
                            pending.request.request_id.as_str(),
                            Ok(export::Image {
                                png: &png,
                                width,
                                height,
                                source_ticks,
                            }),
                        ),
                        Err(reason) => export::publish(
                            pending.request.request_id.as_str(),
                            Err(export_failure(reason)),
                        ),
                    }
                }
                Err(reason) => export::publish(
                    pending.request.request_id.as_str(),
                    Err(export_failure(reason)),
                ),
            };
            result.is_ok()
        });
        self.worker = worker;
        if self.worker.is_none() {
            note("ui-snapshot worker=unavailable");
        }
    }
}

fn export_failure(reason: Failure) -> export::Failure {
    match reason {
        Failure::Timeout => export::Failure::Timeout,
        Failure::VideoBound => export::Failure::VideoBound,
        Failure::SensitiveUi => export::Failure::SensitiveUi,
        Failure::UnsupportedUi => export::Failure::UnsupportedUi,
        Failure::Background => export::Failure::Background,
        Failure::InvalidViewport => export::Failure::InvalidViewport,
        Failure::ReadbackFailed => export::Failure::ReadbackFailed,
        Failure::Cancelled => export::Failure::Cancelled,
        Failure::EncodingFailed => export::Failure::EncodingFailed,
        Failure::PayloadTooLarge => export::Failure::PayloadTooLarge,
    }
}

/// Filter the same batch that the normal launch consumer already owns. Every
/// ordinary payload is retained byte-for-byte/in order, not drained independently.
fn route(payloads: Vec<String>, now: Option<u32>) -> (Vec<String>, bool) {
    let selected = request::select_launches(&payloads);
    if selected == Launch::Ordinary {
        return (payloads, false);
    }
    let ordinary = payloads
        .into_iter()
        .filter(|raw| request::parse_launch(raw) == Launch::Ordinary)
        .collect();
    match selected {
        Launch::Requested(request) => {
            if owner()
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .admit(request, now)
            {
                // Redraw current UI once; do not navigate, stop playback or change settings.
                plx_machine::idle::invalidate();
            }
            (ordinary, false)
        }
        Launch::Rejected(reason) => {
            note(reason.code());
            (ordinary, true)
        }
        Launch::Ordinary => unreachable!(),
    }
}

/// Cold admission precedes the unrelated storage-canary selector, so a mixed
/// diagnostic envelope can never execute the canary as a fallback. A rejection
/// skips the canary selector but still starts the player with its ordinary inputs.
pub(crate) fn cold_launch() -> bool {
    let (ordinary, rejected) = route(plx_platform::tv::launch::take_all(), None);
    for raw in ordinary {
        plx_platform::tv::launch::offer(&raw);
    }
    rejected
}
pub(super) fn filter_launches(payloads: Vec<String>, now: u32) -> Vec<String> {
    route(payloads, Some(now)).0
}

fn eligibility(app: &super::App) -> Result<Option<Eligibility>, Failure> {
    if !app.window_activity.allow_present(true) || app.ott_standby {
        return Err(Failure::Background);
    }
    if app.player.video_plane_bound {
        return Err(Failure::VideoBound);
    }
    // Refuse overlays, transitions and all unaudited screens. The on-screen model,
    // not a future runtime page or a label guessed from focus, owns this decision.
    if app.pages.has_pending_navigation() || app.pages.nav.tabs.stack.transition.in_flight() {
        return Ok(None); // Wait for the final UI, bounded by the request deadline.
    }
    if app.pages.surface_up()
        || !super::bridge::top_settled_on(&app.pages, &plx_screens::registry::AppArg::Ott)
    {
        return Err(Failure::UnsupportedUi);
    }
    let screen = app
        .pages
        .top_screen()
        .and_then(|s| s.as_any())
        .and_then(|s| s.downcast_ref::<plx_screens::ott::OttScreen>())
        .ok_or(Failure::UnsupportedUi)?;
    if screen.handles_private_text() {
        return Err(Failure::SensitiveUi);
    }
    if !screen.ui_snapshot_allowed() {
        return Err(Failure::UnsupportedUi);
    }
    Ok(Some(Eligibility {
        foreground: true,
        video_plane_bound: false,
        sensitive_ui: false,
    }))
}

/// Once per frame, including iterations which do not draw: expiry cannot wait for
/// an unrelated input to make the screen present. No filesystem call on this thread.
pub(super) fn tick(app: &super::App, now: u32) {
    let mut owner = owner().lock().unwrap_or_else(|e| e.into_inner());
    owner.reap(now);
    if owner.pending.is_none()
        && owner.worker.is_none()
        && owner
            .published_at
            .is_some_and(|at| now.wrapping_sub(at) >= RETAIN_MS)
    {
        owner.worker = Worker::start(true, || export::clear().is_ok());
        // A refused thread keeps the expired timestamp so the next frame retries.
    }
    let Some(mut pending) = owner.pending.take() else {
        return;
    };
    // Cold launches precede SDL initialization. Their eight-second frame budget
    // starts here; warm requests are timestamped by the existing launch drain.
    let capture = pending.capture.get_or_insert_with(|| Capture::new(now));
    let outcome = match eligibility(app) {
        Ok(Some(e)) => capture.poll(now, e),
        Ok(None) => capture.poll(
            now,
            Eligibility {
                foreground: true,
                video_plane_bound: false,
                sensitive_ui: false,
            },
        ),
        Err(reason) => Some(Err(reason)),
    };
    if let Some(outcome) = outcome {
        owner.publish(pending, outcome);
    } else {
        owner.pending = Some(pending);
    }
}

/// Called immediately after the final draw and before swap, never through the
/// streaming capture's cached ping-pong buffer.
pub(super) fn after_draw(app: &super::App, now: u32, viewport: Viewport) {
    let mut owner = owner().lock().unwrap_or_else(|e| e.into_inner());
    let Some(mut pending) = owner.pending.take() else {
        return;
    };
    let outcome = match eligibility(app) {
        Err(reason) => Some(Err(reason)),
        Ok(None) => None,
        Ok(Some(e)) => pending
            .capture
            .as_mut()
            .and_then(|c| c.capture_before_swap(now, e, viewport, super::clock::now)),
    };
    if let Some(outcome) = outcome {
        owner.publish(pending, outcome);
    } else {
        owner.pending = Some(pending);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn request(index: u32) -> Request {
        let raw = format!(
            r#"{{"event":"launch","parameters":{{"ottUiSnapshot":{{"version":1,"request_id":"{index:032x}"}}}}}}"#
        );
        let Launch::Requested(request) = request::parse_launch(&raw) else {
            panic!("fixture");
        };
        request
    }
    #[test]
    fn admission_never_replays_or_displaces_inflight_request() {
        let mut owner = Owner::default();
        assert!(owner.admit(request(1), Some(0)));
        assert!(!owner.admit(request(2), Some(0)));
        assert_eq!(owner.pending.as_ref().unwrap().request, request(1));
        owner.pending = None;
        assert!(!owner.admit(request(1), Some(0)));
        assert!(owner.admit(request(2), Some(0)));
    }
    #[test]
    fn a_new_admission_keeps_the_old_artifact_cleanup_deadline() {
        let mut owner = Owner {
            published_at: Some(100),
            ..Owner::default()
        };
        assert!(owner.admit(request(1), Some(200)));
        assert_eq!(owner.published_at, Some(100));
        // A refused worker must not make the preceding image immortal.
        owner.pending = None;
        assert_eq!(owner.published_at, Some(100));
    }
    #[test]
    fn bounded_history_does_not_forget_and_recapture_old_ids() {
        let mut owner = Owner::default();
        for index in 0..REQUEST_LIMIT as u32 {
            assert!(owner.admit(request(index), Some(0)));
            owner.pending = None;
        }
        assert!(!owner.admit(request(0), Some(0)));
        assert!(!owner.admit(request(100), Some(0)));
        assert_eq!(owner.seen.len(), REQUEST_LIMIT);
    }
    #[test]
    fn ordinary_launches_keep_their_original_bytes_and_order() {
        let raw = vec![
            r#"{"parameters":{"plex":{"ratingKey":"42"}}}"#.into(),
            r#"{"event":"relaunch","parameters":{}}"#.into(),
            "not json".into(),
        ];
        let (kept, rejected) = route(raw.clone(), Some(0));
        assert!(!rejected);
        assert_eq!(kept, raw);
    }
    #[test]
    fn invalid_snapshot_is_not_replayed_as_a_normal_player_action() {
        let normal = r#"{"parameters":{"plex":{"ratingKey":"42"}}}"#.to_owned();
        let invalid = r#"{"event":"launch","parameters":{"ottUiSnapshot":{"version":1,"request_id":"invalid"},"plex":{"ratingKey":"99"}}}"#.to_owned();
        let (kept, rejected) = route(vec![normal.clone(), invalid], Some(0));
        assert!(rejected);
        assert_eq!(kept, vec![normal]);
    }
    #[cfg(feature = "hostsim")]
    mod runtime {
        use super::*;
        use std::time::Duration;

        // These tests call the real adapter and its actual worker. Only the
        // platform's existing non-ARM exporter is unsupported; no replacement
        // transport, App shape or function-pointer liveness assertion is used.
        struct IsolatedOwner {
            previous: Option<Owner>,
            launches: Vec<String>,
        }
        impl IsolatedOwner {
            fn new() -> Self {
                let previous = std::mem::take(&mut *owner().lock().unwrap());
                assert!(previous.worker.is_none(), "another test left a live snapshot worker");
                Self { previous: Some(previous), launches: plx_platform::tv::launch::take_all() }
            }
        }
        fn finished(worker: &Worker) -> bool {
            for _ in 0..5000 {
                if worker.handle.is_finished() { return true; }
                std::thread::sleep(Duration::from_millis(1));
            }
            false
        }
        impl Drop for IsolatedOwner {
            fn drop(&mut self) {
                let mut current = std::mem::take(&mut *owner().lock().unwrap_or_else(|e| e.into_inner()));
                if let Some(worker) = current.worker.take() {
                    if finished(&worker) { let _ = worker.handle.join(); }
                    else { eprintln!("snapshot test worker did not finish within 5 seconds"); }
                }
                *owner().lock().unwrap_or_else(|e| e.into_inner()) = self.previous.take().unwrap();
                let _ = plx_platform::tv::launch::take_all();
                for raw in &self.launches { plx_platform::tv::launch::offer(raw); }
            }
        }
        fn warm(index: u32, now: u32) {
            let raw = serde_json::json!({"event":"relaunch","parameters":{
                "ottUiSnapshot":{"version":1,"request_id":request(index).request_id.as_str()}
            }}).to_string();
            assert!(filter_launches(vec![raw], now).is_empty());
        }
        fn reap(now: u32) {
            let mut state = owner().lock().unwrap();
            assert!(finished(state.worker.as_ref().expect("actual snapshot worker")), "worker timed out");
            state.reap(now);
            assert!(state.worker.is_none());
        }
        const VIEW: Viewport = Viewport { x: 0, y: 0, width: 2, height: 2 };

        #[test]
        fn real_adapter_preserves_cold_inputs_and_idle_player_state() {
            let _serial = plx_base::testlock::serial();
            let _isolated = IsolatedOwner::new();
            let app = crate::app::run::lifecycle_regression_tests::app();
            let normal = r#"{"event":"launch","parameters":{"plex":{"ratingKey":"42"}}}"#;
            plx_platform::tv::launch::offer(normal);
            assert!(!cold_launch());
            assert_eq!(plx_platform::tv::launch::take_all(), vec![normal.to_owned()]);
            plx_platform::tv::launch::offer(normal);
            plx_platform::tv::launch::offer(r#"{"event":"launch","parameters":{"ottUiSnapshot":{"version":1,"request_id":"invalid"}}}"#);
            assert!(cold_launch(), "rejected snapshot must suppress the canary selector");
            assert_eq!(plx_platform::tv::launch::take_all(), vec![normal.to_owned()]);
            assert_eq!(filter_launches(vec![normal.to_owned()], 1), vec![normal.to_owned()]);
            tick(&app, 2);
            after_draw(&app, 3, VIEW);
            let state = owner().lock().unwrap();
            assert!(state.pending.is_none() && state.worker.is_none() && state.seen.is_empty());
            assert!(app.ott.is_none() && !app.ott_standby && !app.player.video_plane_bound);
            assert!(app.pages.nav.tabs.stack.entries.is_empty());
        }

        #[test]
        fn real_adapter_rejects_unaudited_ui_once_and_rearms_failed_host_cleanup() {
            let _serial = plx_base::testlock::serial();
            let _isolated = IsolatedOwner::new();
            let app = crate::app::run::lifecycle_regression_tests::app();
            assert!(matches!(eligibility(&app), Err(Failure::UnsupportedUi)));
            warm(10, 100);
            warm(11, 101); // A busy request cannot replace the pending capture.
            assert_eq!(owner().lock().unwrap().pending.as_ref().unwrap().request, request(10));
            tick(&app, 102);
            assert!(owner().lock().unwrap().pending.is_none());
            reap(103);
            assert_eq!(owner().lock().unwrap().published_at, Some(103));
            warm(10, 104); // Remembered request cannot capture a second time.
            assert!(owner().lock().unwrap().pending.is_none());
            tick(&app, 103 + RETAIN_MS - 1);
            assert!(owner().lock().unwrap().worker.is_none());
            tick(&app, 103 + RETAIN_MS);
            assert!(owner().lock().unwrap().worker.as_ref().unwrap().clearing);
            reap(104 + RETAIN_MS);
            // Host export/clear intentionally returns Unsupported, not success.
            assert_eq!(owner().lock().unwrap().published_at, Some(104 + RETAIN_MS));
        }

        #[test]
        fn real_after_draw_refuses_background_and_bound_video_without_navigation() {
            let _serial = plx_base::testlock::serial();
            let _isolated = IsolatedOwner::new();
            let mut app = crate::app::run::lifecycle_regression_tests::app();
            app.ott_standby = true;
            assert!(matches!(eligibility(&app), Err(Failure::Background)));
            app.ott_standby = false;
            app.window_activity.event(0x104);
            assert!(matches!(eligibility(&app), Err(Failure::Background)));
            app.window_activity.event(0x106);
            app.player.video_plane_bound = true;
            assert!(matches!(eligibility(&app), Err(Failure::VideoBound)));
            warm(20, 200);
            after_draw(&app, 201, VIEW);
            assert!(owner().lock().unwrap().pending.is_none());
            reap(202);
            assert_eq!(owner().lock().unwrap().published_at, Some(202));
            assert!(app.player.video_plane_bound, "capture must not stop playback");
            assert!(app.pages.nav.tabs.stack.entries.is_empty());
        }
    }

}
