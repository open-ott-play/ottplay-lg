//! Grouped settings expose existing typed candidates; this module never publishes a draft.
use super::super::*;
use plx_appkit::classic_settings::{ArrowFunction, ListPosition, Seek13Duration};
use plx_appkit::player_hud::model::PermanentClock;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Group {
    Interface,
    Colors,
    Lists,
    Channels,
    Infobar,
    Buttons,
    Menu,
}

impl Group {
    pub(super) const ALL: [Self; 7] = [
        Self::Interface, Self::Colors, Self::Lists, Self::Channels,
        Self::Infobar, Self::Buttons, Self::Menu,
    ];

    pub(super) fn id(self) -> &'static str {
        match self {
            Self::Interface => "interface",
            Self::Colors => "colors",
            Self::Lists => "lists",
            Self::Channels => "channels",
            Self::Infobar => "infobar",
            Self::Buttons => "buttons",
            Self::Menu => "menu",
        }
    }

    pub(super) fn title(self) -> &'static str {
        match self {
            Self::Interface => "Interface",
            Self::Colors => "Interface colors",
            Self::Lists => "Lists",
            Self::Channels => "Channel list",
            Self::Infobar => "Infobar",
            Self::Buttons => "Buttons",
            Self::Menu => "Menu items",
        }
    }

    pub(super) fn fields(self) -> &'static [&'static str] {
        match self {
            // Interface mode has its own public preference writer, not this draft.
            Self::Interface => &[
                "uiFontScale", "sleepTimeout", "osdOpacity", "volumeStep", "permanentTime",
            ],
            Self::Colors => &["uiColorConfig"],
            Self::Lists => &["pageSize", "listPosition"],
            Self::Channels => &["showNumber", "showName", "showProgram", "showDescription", "showProgress"],
            Self::Infobar => &["infoTimeout"],
            Self::Buttons => &["arrowFun", "seek13Duration"],
            // The group owner handles the typed visibility document separately.
            Self::Menu => &["nativeMenuVisibility"],
        }
    }
}

pub(super) fn candidate(
    runtime: &Runtime,
    key: &str,
    value: u64,
    session: u64,
) -> Result<Saved, &'static str> {
    match key {
        "uiFontScale" => runtime.font_scale_candidate(&value.to_string(), session),
        "volumeStep" => runtime.volume_step_candidate(&value.to_string(), session),
        "infoTimeout" => runtime.info_timeout_candidate(&value.to_string(), session),
        "sleepTimeout" => sleep_timer::Choice::from_index(value)
            .map(|choice| runtime.sleep_timer_candidate(choice))
            .ok_or("Select a supported sleep timer duration"),
        "osdOpacity" | "permanentTime" | "pageSize" | "listPosition" | "arrowFun"
        | "seek13Duration" | "showNumber" | "showName" | "showProgram" | "showDescription" | "showProgress" => {
            runtime.osd_setting_candidate(key, &value.to_string(), session)
        }
        _ => Err("Unknown grouped setting"),
    }
}

pub(super) fn value(runtime: &Runtime, key: &str) -> Option<Value> {
    match key {
        "uiColorConfig" => Some(json!(runtime.ui_color_config())),
        "uiFontScale" => Some(json!(runtime.font_scale_percent())),
        "volumeStep" => Some(json!(runtime.volume_step())),
        "infoTimeout" => Some(json!(runtime.info_timeout().seconds())),
        "sleepTimeout" => Some(json!(runtime.sleep_timer_choice().index())),
        "osdOpacity" | "permanentTime" | "pageSize" | "listPosition" | "arrowFun"
        | "seek13Duration" | "showNumber" | "showName" | "showProgram" | "showDescription" | "showProgress" => {
            runtime.osd_setting_value(key)?.parse::<u64>().ok().map(Value::from)
        }
        _ => None,
    }
}

pub(super) fn title(key: &str) -> &'static str {
    if let Some(role) = plx_ui::palette::Role::find(key) { return role.title(); }
    match key {
        "uiFontScale" => "Interface text scale (%)",
        "sleepTimeout" => "Sleep timer",
        "osdOpacity" => "OSD background level",
        "volumeStep" => "Volume step (%)",
        "permanentTime" => "Permanent clock",
        "pageSize" => "Classic list rows",
        "listPosition" => "Classic list side",
        "infoTimeout" => "Information display time (seconds)",
        "showNumber" => "Show channel number in list",
        "showName" => "Show channel name in list",
        "showProgram" => "Show program name",
        "showDescription" => "Show description",
        "showProgress" => "Show program progress",
        "arrowFun" => "Classic list arrows",
        "seek13Duration" => "Seek step for 1/3 (seconds)",
        "nativeMenuVisibility" => "Menu items",
        _ => "Unknown grouped setting",
    }
}

pub(super) fn choices(key: &str) -> Vec<(u64, String)> {
    match key {
        "uiFontScale" => (20..=200).map(|n| (n, format!("{n}%"))).collect(),
        "volumeStep" => (3..=10).map(|n| (n, format!("{n}%"))).collect(),
        "infoTimeout" => (3..=20).map(|n| (n, format!("{n} s"))).collect(),
        "pageSize" => (10..=30).map(|n| (n, n.to_string())).collect(),
        // Native opacity is a background-strength level, not a final-alpha percentage.
        "osdOpacity" => (0..=10).map(|n| (n, n.to_string())).collect(),
        "sleepTimeout" => sleep_timer::Choice::ALL.into_iter()
            .map(|choice| (u64::from(choice.index()), choice.label().into())).collect(),
        "permanentTime" => [
            (PermanentClock::Off, "Off"),
            (PermanentClock::Background, "Background"),
            (PermanentClock::Transparent, "Transparent"),
        ].into_iter().map(|(choice, label)| (choice.index(), label.into())).collect(),
        "listPosition" => [
            (ListPosition::Right, "Right"), (ListPosition::Left, "Left"),
        ].into_iter().map(|(choice, label)| (u64::from(choice.index()), label.into())).collect(),
        // Reading legacy 3 retains its identity; only an explicit choice replaces it.
        "arrowFun" => [
            (ArrowFunction::Paging, "Paging"),
            (ArrowFunction::Volume, "Volume"),
            (ArrowFunction::Dune, "Categories / details"),
        ].into_iter().map(|(choice, label)| (u64::from(choice.index()), label.into())).collect(),
        "seek13Duration" => Seek13Duration::CHOICES.into_iter()
            .map(|seconds| (u64::from(seconds), format!("{seconds} s"))).collect(),
        "showNumber" | "showName" | "showProgram" | "showDescription" | "showProgress" => {
            vec![(0, "No".into()), (1, "Yes".into())]
        }
        _ => Vec::new(),
    }
}
