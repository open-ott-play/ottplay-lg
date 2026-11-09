//! Opt-in, bounded native-playback capture for the authenticated OTT remote.
//!
//! The wire projection is deliberately narrower than `plx_media::player::Diag`:
//! it contains counters and closed enums only, never a media identity, URL, path,
//! title, duration, position, window id, or raw Starfish text.
use super::*;
use std::collections::VecDeque;

pub(super) const SAMPLE_INTERVAL_MS: u32 = 1_000;
pub(super) const MAX_DURATION_MS: u32 = 60_000;
pub(super) const MAX_RECORDS: usize = 64;
const DEFAULT_DURATION_S: u64 = 30;

#[derive(Default)]
pub(super) struct Capture {
    capture: Option<Session>,
}

impl Drop for Capture {
    fn drop(&mut self) {
        turn_capture_readout_on(false);
    }
}

struct Session {
    id: String,
    state: State,
    started_at: u32,
    ended_at: Option<u32>,
    duration_ms: u32,
    next_sample_at: u32,
    playback: plx_media::route::PlaybackIdentity,
    records: VecDeque<Record>,
    input: VecDeque<NativeKey>,
    dropped: u32,
}

#[derive(Clone, Copy)]
struct NativeKey { t_ms: u32, state: u32, scancode: u32, sym: u32 }

impl Runtime {
    /// Only a bounded authenticated diagnostic capture may retain native input.
    /// Printable Unicode and text input are excluded, including numeric keys.
    pub(in crate::app) fn note_native_input(&mut self, state: u32, scancode: u32, sym: u32,
        now: u32, source: plx_machine::machine::Source,
        playback: Option<&plx_media::route::PlaybackIdentity>) {
        // Some native keyboards omit sym. Exclude SDL text-producing
        // scancodes too, including letters/digits, punctuation and keypads.
        let printable_scan = matches!(scancode, 4..=39 | 44..=56 | 89..=100 | 133..=143 | 176..=221);
        if source != plx_machine::machine::Source::Sdl || state & 0xff != 1 || printable_scan
            || (sym != 0 && sym & (1 << 30) == 0) { return; }
        let Some(session) = self.native_playback_capture.capture.as_mut() else { return; };
        if session.state != State::Capturing || age(now, session.started_at) >= session.duration_ms
            || playback != Some(&session.playback) { return; }
        if session.input.len() == 16 { session.input.pop_front(); }
        session.input.push_back(NativeKey { t_ms: age(now, session.started_at), state: state & 0x1ff, scancode, sym });
    }

    pub(in crate::app) fn native_input_probe(&self) -> String {
        let Some(session) = self.native_playback_capture.capture.as_ref() else { return String::new(); };
        // Retain the existing completed capture's evidence; starting another
        // capture replaces its owner and events together. Nothing is persisted.
        json!({"version":1,"state":session.state.wire(),"events":session.input.iter().map(|key|
            json!({"t_ms":key.t_ms,"state":key.state,"scancode":key.scancode,"sym":key.sym}))
            .collect::<Vec<_>>()}).to_string()
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum State {
    Capturing,
    Complete,
    Stopped,
    SessionChanged,
}

impl State {
    fn wire(self) -> &'static str {
        match self {
            Self::Capturing => "capturing",
            Self::Complete => "complete",
            Self::Stopped => "stopped",
            Self::SessionChanged => "session_changed",
        }
    }
}

#[derive(Clone)]
struct Record {
    t_ms: u32,
    stage: &'static str,
    load_state: &'static str,
    frames: u32,
    frame_age_ms: Option<u32>,
    video_queue_bytes: u32,
    audio_queue_bytes: u32,
    fed_video_units: u64,
    fed_audio_units: u64,
    fed_pts_skew_ms: Option<i32>,
    playable_buffer_ms: Option<u32>,
    feed_state: &'static str,
    callback_count: u32,
    callback_error_code: Option<i32>,
    callback_error_age_ms: Option<u32>,
    http_status: Option<u16>,
    network_rx_bytes: u64,
    video_width: u16,
    video_height: u16,
    fps_milli: u32,
}

fn age(now: u32, then: u32) -> u32 {
    now.wrapping_sub(then).min(MAX_DURATION_MS)
}

fn clamp_u32(value: i64) -> u32 {
    value.max(0).min(u32::MAX as i64) as u32
}

fn clamp_u64(value: i64) -> u64 {
    value.max(0).min(9_007_199_254_740_991) as u64
}

fn clamp_u16(value: i32) -> u16 {
    value.max(0).min(u16::MAX as i32) as u16
}

fn requested_duration_s(value: Option<&Value>) -> Option<u64> {
    match value {
        None => Some(DEFAULT_DURATION_S),
        Some(value) => value.as_u64().filter(|seconds| (5..=60).contains(seconds)),
    }
}

pub(super) fn playback_owner(
    runtime: &Runtime,
    app: &App,
) -> Option<plx_media::route::PlaybackIdentity> {
    if !app.adapters.player.is_live() {
        return None;
    }
    let playing = runtime
        .playing
        .as_ref()
        .filter(|playing| super::playing_matches(playing, app))?;
    let expected = playing.identity.as_ref()?;
    let current = plx_media::route::playback_identity(&app.player.session)?;
    (expected == &current).then_some(current)
}

fn sample(session: &mut Session, app: &App, now: u32) {
    let d = plx_media::player::diag(&app.player.session);
    let skew = (d.fed_v > 0 && d.fed_a > 0 && d.fed_v_pts >= 0 && d.fed_a_pts >= 0)
        .then(|| d.fed_v_pts.saturating_sub(d.fed_a_pts) / 1_000_000)
        .map(|value| value.clamp(-600_000, 600_000) as i32);
    let record = Record {
        t_ms: age(now, session.started_at),
        stage: match d.stage {
            0 => "loading",
            1 => "playing",
            2 => "bound",
            _ => "streaming",
        },
        load_state: if d.load_failed {
            "failed"
        } else if d.load_completed {
            "complete"
        } else {
            "pending"
        },
        frames: d.frames.max(0) as u32,
        frame_age_ms: d.seen_frame.then(|| age(now, d.frame_at)),
        video_queue_bytes: clamp_u32(d.aq_video),
        audio_queue_bytes: clamp_u32(d.aq_audio),
        fed_video_units: clamp_u64(d.fed_v),
        fed_audio_units: clamp_u64(d.fed_a),
        fed_pts_skew_ms: skew,
        playable_buffer_ms: d
            .playable_buffer_ms
            .map(|value| value.clamp(0, 60_000) as u32),
        feed_state: match d.feed_state {
            1 => "accepting",
            2 => "full",
            3 => "refused",
            4 => "holding",
            5 => "empty",
            _ => "none",
        },
        callback_count: d.cb_count,
        callback_error_code: (d.cb_err != 0).then_some(d.cb_err),
        callback_error_age_ms: (d.cb_err != 0).then(|| age(now, d.cb_err_at)),
        http_status: (100..=599)
            .contains(&d.http_status)
            .then_some(d.http_status as u16),
        network_rx_bytes: clamp_u64(d.net_rx),
        video_width: clamp_u16(d.video_w),
        video_height: clamp_u16(d.video_h),
        fps_milli: clamp_u32(d.video_fps_milli),
    };
    if session.records.len() == MAX_RECORDS {
        session.records.pop_front();
        session.dropped = session.dropped.saturating_add(1);
    }
    session.records.push_back(record);
}

fn record_json(record: &Record) -> Value {
    json!({
        "t_ms": record.t_ms,
        "stage": record.stage,
        "load_state": record.load_state,
        "frames": record.frames,
        "frame_age_ms": record.frame_age_ms,
        "video_queue_bytes": record.video_queue_bytes,
        "audio_queue_bytes": record.audio_queue_bytes,
        "fed_video_units": record.fed_video_units,
        "fed_audio_units": record.fed_audio_units,
        "fed_pts_skew_ms": record.fed_pts_skew_ms,
        "playable_buffer_ms": record.playable_buffer_ms,
        "feed_state": record.feed_state,
        "callback_count": record.callback_count,
        "callback_error_code": record.callback_error_code,
        "callback_error_age_ms": record.callback_error_age_ms,
        "http_status": record.http_status,
        "network_rx_bytes": record.network_rx_bytes,
        "video_width": record.video_width,
        "video_height": record.video_height,
        "fps_milli": record.fps_milli,
    })
}

fn turn_capture_readout_on(on: bool) {
    plx_media::player::NATIVE_CAPTURE_DIAG_ON.store(on, std::sync::atomic::Ordering::Relaxed);
}

pub(super) fn tick(
    capture: &mut Capture,
    active_playback: Option<plx_media::route::PlaybackIdentity>,
    app: &App,
    now: u32,
) {
    let Some(session) = capture.capture.as_mut() else {
        return;
    };
    if session.state != State::Capturing {
        return;
    }
    let elapsed = age(now, session.started_at);
    if active_playback.as_ref() != Some(&session.playback) {
        session.state = State::SessionChanged;
        session.ended_at = Some(now);
        turn_capture_readout_on(false);
        return;
    }
    if elapsed >= session.duration_ms {
        session.state = State::Complete;
        session.ended_at = Some(session.started_at.wrapping_add(session.duration_ms));
        turn_capture_readout_on(false);
        return;
    }
    if now.wrapping_sub(session.next_sample_at) < (1 << 31) {
        sample(session, app, now);
        session.next_sample_at = now.wrapping_add(SAMPLE_INTERVAL_MS);
    }
}

fn valid_id(value: &str) -> bool {
    value.strip_prefix("cap_").is_some_and(|suffix| {
        suffix.len() == 32
            && suffix
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    })
}

fn error(operation: &str, code: &'static str) -> Value {
    json!({"status":"rejected","data":{"version":1,"operation":operation,"error":code}})
}

fn idle_status() -> Value {
    json!({"status":"ok","data":{"version":1,"operation":"status","state":"idle","capture_id":null,"elapsed_ms":0,"records":0,"dropped":0}})
}

fn status(session: &Session, operation: &str, now: u32) -> Value {
    let elapsed_ms = session.ended_at.map_or_else(
        || age(now, session.started_at),
        |ended| age(ended, session.started_at),
    );
    json!({"status":"ok","data":{
        "version": 1,
        "operation": operation,
        "state": session.state.wire(),
        "capture_id": session.id,
        "elapsed_ms": elapsed_ms,
        "records": session.records.len(),
        "dropped": session.dropped,
    }})
}

pub(super) fn failure(inv: &Invocation, code: &'static str) -> Value {
    error(
        inv.params
            .get("operation")
            .and_then(Value::as_str)
            .unwrap_or(""),
        code,
    )
}

fn opaque_id(runtime: &mut Runtime) -> Result<String, &'static str> {
    let uuid = super::favorites::nonce(runtime).map_err(|_| "invalid_request")?;
    Ok(format!("cap_{}", uuid.replace('-', "")))
}

pub(super) fn command(runtime: &mut Runtime, app: &mut App, inv: &Invocation) -> Value {
    let p = &inv.params;
    let operation = p.get("operation").and_then(Value::as_str).unwrap_or("");
    if !["start", "status", "read", "stop"].contains(&operation) {
        return error(operation, "unsupported");
    }
    if p.get("version").and_then(Value::as_u64) != Some(1) {
        return error(operation, "invalid_request");
    }
    let Some(requested_runtime) = p.get("runtime").and_then(Value::as_str) else {
        return error(operation, "invalid_request");
    };
    if requested_runtime != runtime.remote.runtime() {
        return error(operation, "runtime_mismatch");
    }
    let now = crate::app::clock::now();

    match operation {
        "start" => {
            if p.as_object().is_none_or(|object| {
                !(3..=4).contains(&object.len())
                    || object.keys().any(|key| {
                        !["version", "runtime", "operation", "duration_s"].contains(&key.as_str())
                    })
            }) {
                return error(operation, "invalid_request");
            }
            let Some(duration_s) = requested_duration_s(p.get("duration_s")) else {
                return error(operation, "invalid_request");
            };
            if runtime
                .native_playback_capture
                .capture
                .as_ref()
                .is_some_and(|session| session.state == State::Capturing)
            {
                return error(operation, "capture_busy");
            }
            let Some(playback) = playback_owner(runtime, app) else {
                return error(operation, "no_active_playback");
            };
            let id = match opaque_id(runtime) {
                Ok(id) => id,
                Err(code) => return error(operation, code),
            };
            runtime.native_playback_capture.capture = Some(Session {
                id: id.clone(),
                state: State::Capturing,
                started_at: now,
                ended_at: None,
                duration_ms: duration_s as u32 * 1_000,
                next_sample_at: now,
                playback,
                records: VecDeque::with_capacity(MAX_RECORDS),
                input: VecDeque::with_capacity(16),
                dropped: 0,
            });
            turn_capture_readout_on(true);
            json!({"status":"ok","data":{"version":1,"operation":"start","state":"capturing","capture_id":id}})
        }
        "status" => {
            if p.as_object().is_none_or(|object| {
                object.len() > 4
                    || object.keys().any(|key| {
                        !["version", "runtime", "operation", "capture_id"].contains(&key.as_str())
                    })
            }) {
                return error(operation, "invalid_request");
            }
            let requested = p.get("capture_id").and_then(Value::as_str);
            if p.get("capture_id")
                .is_some_and(|value| !value.is_null() && value.as_str().is_none())
                || requested.is_some_and(|id| !valid_id(id))
            {
                return error(operation, "invalid_request");
            }
            let Some(session) = runtime.native_playback_capture.capture.as_ref() else {
                return idle_status();
            };
            if requested.is_some_and(|id| id != session.id) {
                return error(operation, "capture_not_found");
            }
            status(session, operation, now)
        }
        "read" | "stop" => {
            if p.as_object().is_none_or(|object| {
                object.len() != 4
                    || object.keys().any(|key| {
                        !["version", "runtime", "operation", "capture_id"].contains(&key.as_str())
                    })
            }) {
                return error(operation, "invalid_request");
            }
            let Some(id) = p
                .get("capture_id")
                .and_then(Value::as_str)
                .filter(|id| valid_id(id))
            else {
                return error(operation, "invalid_request");
            };
            let Some(session) = runtime
                .native_playback_capture
                .capture
                .as_mut()
                .filter(|session| session.id == id)
            else {
                return error(operation, "capture_not_found");
            };
            if operation == "stop" && session.state == State::Capturing {
                session.state = State::Stopped;
                session.ended_at = Some(now);
                turn_capture_readout_on(false);
            }
            if operation == "read" {
                let elapsed_ms = session.ended_at.map_or_else(
                    || age(now, session.started_at),
                    |ended| age(ended, session.started_at),
                );
                json!({"status":"ok","data":{
                    "version": 1,
                    "operation": operation,
                    "state": session.state.wire(),
                    "capture_id": session.id,
                    "elapsed_ms": elapsed_ms,
                    "records": session.records.iter().map(record_json).collect::<Vec<_>>(),
                    "dropped": session.dropped,
                }})
            } else {
                status(session, operation, now)
            }
        }
        _ => error(operation, "unsupported"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};

    #[test]
    fn omitted_capture_duration_defaults_to_thirty_seconds() {
        assert_eq!(requested_duration_s(None), Some(30));
    }

    #[test]
    fn explicit_capture_duration_accepts_only_five_through_sixty_seconds() {
        for seconds in [5, 30, 60] {
            assert_eq!(requested_duration_s(Some(&json!(seconds))), Some(seconds));
        }
        for invalid in [json!(4), json!(61), Value::Null, json!(-1), json!(5.5), json!("30")] {
            assert_eq!(requested_duration_s(Some(&invalid)), None, "{invalid}");
        }
    }
    #[test]
    #[cfg(feature = "hostsim")]
    fn native_input_probe_is_capture_owned_bounded_nonprinting_and_retained() {
        let _lock = plx_base::testlock::serial();
        let mut session = plx_media::route::PlaybackSession::default();
        plx_media::route::reset_player_control_for_test(&session);
        plx_media::route::install_plex_playback_for_test(&mut session,
            plx_plex::plex::ServerId::from_raw(0), "input-fixture");
        let playback = plx_media::route::playback_identity(&session).unwrap();
        let mut runtime = Runtime::unloaded(crate::app::ott_companion::Supervisor::inert_for_test());
        assert_eq!(runtime.native_input_probe(), "");
        runtime.native_playback_capture.capture = Some(Session { id:"fixture".into(), state:State::Capturing,
            started_at:1_000, ended_at:None, duration_ms:5_000, next_sample_at:1_000,
            playback:playback.clone(), records:VecDeque::new(), input:VecDeque::new(), dropped:0 });
        for sym in [b'p' as u32, b'1' as u32, 'Ж' as u32] {
            runtime.note_native_input(1, 42, sym, 1_001, plx_machine::machine::Source::Sdl, Some(&playback));
        }
        runtime.note_native_input(1, 480, 0, 1_001, plx_machine::machine::Source::RemoteFifo, Some(&playback));
        for scancode in [4, 30, 44, 45, 89, 99, 100, 133, 135, 176, 221] {
            runtime.note_native_input(1, scancode, 0, 1_001, plx_machine::machine::Source::Sdl, Some(&playback));
            runtime.note_native_input(1, scancode, (1 << 30) | scancode, 1_001, plx_machine::machine::Source::Sdl, Some(&playback));
        }
        assert_eq!(serde_json::from_str::<Value>(&runtime.native_input_probe()).unwrap()["events"], json!([]));
        for scancode in 480..500 {
            runtime.note_native_input(1, scancode, 0, 1_010, plx_machine::machine::Source::Sdl, Some(&playback));
        }
        let data: Value = serde_json::from_str(&runtime.native_input_probe()).unwrap();
        assert_eq!(data["events"].as_array().unwrap().len(), 16);
        assert_eq!(data["events"][0]["scancode"], 484);
        assert!(runtime.native_input_probe().len() <= 4096);
        runtime.note_native_input(1, 999, 0, 6_000, plx_machine::machine::Source::Sdl, Some(&playback));
        runtime.note_native_input(1, 998, 0, 1_100, plx_machine::machine::Source::Sdl, None);
        assert_eq!(serde_json::from_str::<Value>(&runtime.native_input_probe()).unwrap()["events"], data["events"]);
        runtime.native_playback_capture.capture.as_mut().unwrap().state = State::SessionChanged;
        runtime.note_native_input(1, 997, 0, 1_110, plx_machine::machine::Source::Sdl, Some(&playback));
        let completed: Value = serde_json::from_str(&runtime.native_input_probe()).unwrap();
        assert_eq!(completed["state"], "session_changed");
        assert_eq!(completed["events"], data["events"], "source change must preserve captured evidence without adding new input");
    }

}
