//! Shared Classic/Modern sleep setting, selector and Standby adapter.
use super::*;
#[path = "ott_sleep_timer_state.rs"]
mod state;
pub(super) use state::State;

/// The same persisted indices as Classic, not a free-form millisecond timeout.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) enum Choice {
    #[default]
    Off,
    Minutes30,
    Minutes60,
    Minutes120,
    Minutes180,
}

impl Choice {
    pub(super) const ALL: [Self; 5] = [Self::Off, Self::Minutes30, Self::Minutes60,
        Self::Minutes120, Self::Minutes180];

    pub(super) fn from_index(index: u64) -> Option<Self> {
        match index {
            0 => Some(Self::Off), 1 => Some(Self::Minutes30), 2 => Some(Self::Minutes60),
            3 => Some(Self::Minutes120), 4 => Some(Self::Minutes180), _ => None,
        }
    }
    pub(super) fn index(self) -> u8 {
        match self {
            Self::Off => 0, Self::Minutes30 => 1, Self::Minutes60 => 2,
            Self::Minutes120 => 3, Self::Minutes180 => 4,
        }
    }
    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Off => "Off", Self::Minutes30 => "30 min", Self::Minutes60 => "60 min",
            Self::Minutes120 => "120 min", Self::Minutes180 => "180 min",
        }
    }
    fn duration_ms(self) -> Option<u64> {
        match self {
            Self::Off => None, Self::Minutes30 => Some(1_800_000), Self::Minutes60 => Some(3_600_000),
            Self::Minutes120 => Some(7_200_000), Self::Minutes180 => Some(10_800_000),
        }
    }
}

pub(super) fn tick(runtime: &mut Runtime, app: &mut App) {
    let now_ms = runtime.now_ms();
    tick_at(runtime, app, now_ms);
}

/// Time is sampled by the existing frame boundary and projected through its
/// monotonic anchor. This seam also lets host tests use synthetic deadlines.
pub(super) fn tick_at(runtime: &mut Runtime, app: &mut App, now_ms: u64) {
    let permitted = runtime.loaded && !runtime.persistence_uncertain
        && !matches!(runtime.state_load, StateLoad::Failed(_))
        && playback_allowed(app) && !input_policy::pending(runtime)
        && matches!(runtime.track_selection_kiosk(), Ok(crate::app::ott_kiosk_policy::State::Off));
    let choice = runtime.sleep_timer_choice();
    let sample = state::Sample {
        now_ms, choice,
        revision: runtime.saved.settings.get("settingRevision:sleepTimeout").and_then(Value::as_str),
        // Input ingestion covers keyboard, pointer/touch and admitted remote
        // control. Its wrapping stamp is an identity only, never elapsed time.
        activity: app.last_input,
        permitted,
        // Wait for the single Saved writer: an Off commit on this frame must
        // win. A failed/unrelated write does not reset the old deadline.
        defer_expiry: runtime.saving,
    };
    if choice != Choice::Off && app.rec.failure().is_none() {
        app.rec.refuse("Sleep timer deadlines are not bound in this recording format");
    }
    if runtime.sleep_timer.step(sample) == Some(state::Fx::EnterStandby) {
        // Exactly the remote Standby door: retire catalog/resolver/companion
        // ownership before parking playback. Never toggle or shut down the OS.
        runtime.suspend_with_retry_reason(app, continuity_diagnostics::Reason::Standby);
        crate::app::ott_standby::set(app, true);
        runtime.changed();
    }
}

pub(super) fn presentation_action(runtime: &Runtime, action: &OttAction) -> bool {
    matches!(action, OttAction::EditSetting(key)
        if key == "sleepTimeout" || key.starts_with("sleepTimeout.select:"))
        || (runtime.page == "sleep-timer"
            && matches!(action, OttAction::Back | OttAction::ShowSettings))
}

pub(super) fn view(runtime: &Runtime) -> Option<OttView> {
    if runtime.page != "sleep-timer" { return None; }
    let selected = runtime.sleep_timer_choice();
    let token = runtime.sleep_timer_token();
    let enabled = runtime.settings_write_refusal().is_none();
    let mut rows: Vec<_> = Choice::ALL.into_iter().map(|choice| {
        let mut row = ui_projection::row(
            format!("sleep:{}", choice.index()), choice.label().into(),
            if choice == selected { "Selected" } else { "" }.into(),
            OttAction::EditSetting(format!("sleepTimeout.select:{}:{token}", choice.index())), false,
        );
        row.enabled = enabled;
        row
    }).collect();
    rows.push(ui_projection::row("sleep:back".into(), "Back to Settings".into(),
        String::new(), OttAction::ShowSettings, false));
    Some(OttView {
        page_id: "sleep-timer".into(), revision: runtime.revision,
        title: "Sleep timer".into(), description: "Standby after no input".into(),
        status: runtime.protected_state_status().unwrap_or(&runtime.status).into(), rows,
        selected_row: Some(format!("sleep:{}", selected.index())), ..OttView::default()
    })
}

pub(super) fn action(runtime: &mut Runtime, action: &OttAction) -> Option<Result<(), &'static str>> {
    if runtime.page == "sleep-timer" && matches!(action, OttAction::Back | OttAction::ShowSettings) {
        runtime.page = "settings";
        return Some(Ok(()));
    }
    let OttAction::EditSetting(key) = action else { return None; };
    if key == "sleepTimeout" {
        return Some((|| {
            if let Some(error) = runtime.settings_write_refusal() { return Err(error); }
            if runtime.edit_session.is_some() || runtime.setup.is_some() {
                return Err("Close the editor before changing the sleep timer");
            }
            runtime.edit_serial = runtime.edit_serial.checked_add(1)
                .ok_or("Editor session limit reached; reopen the player")?;
            runtime.settings_focus = Some("sleepTimeout");
            runtime.page = "sleep-timer";
            Ok(())
        })());
    }
    let selection = key.strip_prefix("sleepTimeout.select:")?;
    Some((|| {
        let (index, token) = selection.split_once(':')
            .ok_or("Sleep timer selection changed; select it again")?;
        let choice = match index {
            "0" => Choice::Off, "1" => Choice::Minutes30, "2" => Choice::Minutes60,
            "3" => Choice::Minutes120, "4" => Choice::Minutes180,
            _ => return Err("Sleep timer selection changed; select it again"),
        };
        if runtime.page != "sleep-timer" || token != runtime.sleep_timer_token() {
            return Err("Sleep timer selection changed; select it again");
        }
        if let Some(error) = runtime.settings_write_refusal() { return Err(error); }
        if runtime.edit_session.is_some() || runtime.setup.is_some() {
            return Err("Close the editor before changing the sleep timer");
        }
        // Selecting the current duration intentionally starts a fresh interval,
        // but only after the same acknowledged durable commit as a new value.
        runtime.save(runtime.sleep_timer_candidate(choice), None, false)
    })())
}
