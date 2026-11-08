//! Passive observations from the current native attempt. No value here proves
//! decoded frames, audible output, a powered-on panel, or physical visibility.
use super::{playing_matches, App, Runtime};
use plx_media::player::report;
use serde::Serialize;

#[derive(Serialize)]
pub(super) struct Snapshot {
    version: u8,
    window_present_allowed: bool,
    native: Option<NativeSnapshot>,
}

#[derive(Serialize)]
struct NativeSnapshot {
    position_callbacks: u32,
    sink_report: Option<SinkReport>,
}

#[derive(Serialize)]
struct SinkReport {
    callback_type: i32,
    callback_count: u32,
    firmware_major: u32,
    // A decimal string preserves all signed 64-bit values in JavaScript clients.
    raw_value: String,
}

pub(super) fn snapshot(runtime: &Runtime, app: &mut App) -> Snapshot {
    // Identity alone survives some stops. The media getter additionally checks
    // the exact physical attempt and samples inside the active epoch's lock.
    let owns_playback = runtime.play_target.is_none() && runtime.playing.as_ref().is_some_and(|playing|
        playing.identity.is_some() && playing_matches(playing, app));
    let native = if owns_playback {
        report::video_evidence_now(&app.player.session, &mut app.adapters.player)
    } else {
        None
    };
    Snapshot {
        version: 1,
        // Initialized true at boot: this is permission, not an observed screen state.
        window_present_allowed: app.window_activity.allow_present(true),
        native: native.map(|value| NativeSnapshot {
            position_callbacks: value.position_callbacks,
            sink_report: value.sink_report.map(|report| SinkReport {
                callback_type: report.callback_type,
                callback_count: report.callback_count,
                firmware_major: report.firmware_major,
                raw_value: report.raw_value.to_string(),
            }),
        }),
    }
}
