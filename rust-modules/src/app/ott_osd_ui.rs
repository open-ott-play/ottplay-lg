//! Local display and input choices share the typed validator and durable write door.
use super::*;

const STALE: &str = "Display settings changed; select the option again";
const CLOSE_EDITOR: &str = "Close the editor before changing display settings";
const PREFIX: &str = "osd.select:";

#[derive(Clone, Copy)]
enum Setting { Opacity, Clock, PageSize, ListPosition, ListArrows, NumericSeek, ShowNumber, ShowName, ShowProgram, ShowDescription, ShowProgress }

impl Setting {
    fn key(self) -> &'static str {
        match self {
            Self::Opacity => "osdOpacity", Self::Clock => "permanentTime",
            Self::PageSize => "pageSize", Self::ListPosition => "listPosition",
            Self::ListArrows => "arrowFun", Self::NumericSeek => "seek13Duration",
            Self::ShowNumber => "showNumber", Self::ShowName => "showName",
            Self::ShowProgram => "showProgram", Self::ShowDescription => "showDescription",
            Self::ShowProgress => "showProgress",
        }
    }
    fn page(self) -> &'static str {
        match self {
            Self::Opacity => "osd-opacity", Self::Clock => "permanent-clock",
            Self::PageSize => "classic-page-size", Self::ListPosition => "classic-list-position",
            Self::ListArrows => "classic-list-arrows", Self::NumericSeek => "numeric-seek-step",
            Self::ShowNumber => "classic-show-number", Self::ShowName => "classic-show-name",
            Self::ShowProgram => "classic-show-program", Self::ShowDescription => "classic-show-description",
            Self::ShowProgress => "classic-show-progress",
        }
    }
    fn title(self) -> &'static str {
        match self {
            Self::Opacity => "OSD background level", Self::Clock => "Permanent clock",
            Self::PageSize => "Classic list rows", Self::ListPosition => "Classic list side",
            Self::ListArrows => "Classic list arrows", Self::NumericSeek => "Seek step for 1/3 (seconds)",
            Self::ShowNumber => "Show channel number in list", Self::ShowName => "Show channel name in list",
            Self::ShowProgram => "Show program name", Self::ShowDescription => "Show description",
            Self::ShowProgress => "Show program progress",
        }
    }
    fn help(self) -> &'static str {
        match self {
            Self::Opacity => "Choose an on-screen background level (default 7)",
            Self::Clock => "Show a permanent clock during playback",
            Self::PageSize => "Preferred rows per Classic page; large text and available space can reduce the count",
            Self::ListPosition => "Choose the side of the Classic list and its video preview",
            Self::ListArrows => "Choose Left/Right behavior in Classic lists; fullscreen controls stay unchanged",
            Self::NumericSeek => "Skip backward or forward with 1/3 during a video or archive; live channels keep number entry",
            Self::ShowNumber => "Show item numbers in Classic lists; number-key selection is unchanged",
            Self::ShowName => "Show channel and file names in Classic lists",
            Self::ShowProgram => "Show available current-programme titles in Classic lists",
            Self::ShowDescription => "Show synopsis text beside the list; the full description remains available",
            Self::ShowProgress => "Show current-programme progress in Classic channel lists",
        }
    }
    fn values(self) -> &'static [&'static str] {
        match self {
            Self::Opacity => &["0", "1", "2", "3", "4", "5", "6", "7", "8", "9", "10"],
            Self::Clock => &["0", "1", "2"],
            Self::PageSize => &["10", "11", "12", "13", "14", "15", "16", "17", "18", "19", "20", "21", "22", "23", "24", "25", "26", "27", "28", "29", "30"],
            Self::ListPosition | Self::ShowNumber | Self::ShowName | Self::ShowProgram | Self::ShowDescription | Self::ShowProgress => &["0", "1"],
            Self::ListArrows => &["0", "1", "2"],
            Self::NumericSeek => &["5", "10", "15", "20", "30", "60", "120", "180", "240", "300", "600", "900", "1200", "1800", "3600"],
        }
    }
    fn label(self, value: &str) -> &str {
        match (self, value) {
            (Self::ShowNumber | Self::ShowName | Self::ShowProgram | Self::ShowDescription | Self::ShowProgress, "0") => "Off",
            (Self::ShowNumber | Self::ShowName | Self::ShowProgram | Self::ShowDescription | Self::ShowProgress, "1") => "On",
            (Self::Clock, "0") => "Off",
            (Self::Clock, "1") => "Background",
            (Self::Clock, "2") => "Transparent",
            (Self::ListPosition, "0") => "Right",
            (Self::ListPosition, "1") => "Left",
            (Self::ListArrows, "0" | "3") => "Paging",
            (Self::ListArrows, "1") => "Volume",
            (Self::ListArrows, "2") => "Categories / details",
            _ => value,
        }
    }
    fn selected_choice(self, value: &str) -> &str {
        match (self, value) {
            // Legacy 3 behaves exactly like Paging. Opening the picker must
            // neither create a duplicate choice nor rewrite the saved value.
            (Self::ListArrows, "3") => "0",
            _ => value,
        }
    }
    fn from_key(key: &str) -> Option<Self> {
        match key {
            "osdOpacity" => Some(Self::Opacity), "permanentTime" => Some(Self::Clock),
            "pageSize" => Some(Self::PageSize), "listPosition" => Some(Self::ListPosition),
            "arrowFun" => Some(Self::ListArrows), "seek13Duration" => Some(Self::NumericSeek),
            "showNumber" => Some(Self::ShowNumber), "showName" => Some(Self::ShowName),
            "showProgram" => Some(Self::ShowProgram), "showDescription" => Some(Self::ShowDescription),
            "showProgress" => Some(Self::ShowProgress),
            _ => None,
        }
    }
    fn from_page(page: &str) -> Option<Self> {
        match page {
            "osd-opacity" => Some(Self::Opacity), "permanent-clock" => Some(Self::Clock),
            "classic-page-size" => Some(Self::PageSize), "classic-list-position" => Some(Self::ListPosition),
            "classic-list-arrows" => Some(Self::ListArrows), "numeric-seek-step" => Some(Self::NumericSeek),
            "classic-show-number" => Some(Self::ShowNumber), "classic-show-name" => Some(Self::ShowName),
            "classic-show-program" => Some(Self::ShowProgram), "classic-show-description" => Some(Self::ShowDescription),
            "classic-show-progress" => Some(Self::ShowProgress),
            _ => None,
        }
    }
}

pub(super) fn detail(runtime: &Runtime, key: &str) -> Option<String> {
    let setting = Setting::from_key(key)?;
    Some(setting.label(&runtime.osd_setting_value(key)?).into())
}

pub(super) fn presentation_action(runtime: &Runtime, action: &OttAction) -> bool {
    matches!(action, OttAction::EditSetting(key)
        if Setting::from_key(key).is_some() || key.starts_with(PREFIX))
        || (Setting::from_page(runtime.page).is_some()
            && matches!(action, OttAction::Back | OttAction::ShowSettings))
}

pub(super) fn view(runtime: &Runtime) -> Option<OttView> {
    let setting = Setting::from_page(runtime.page)?;
    let stored = runtime.osd_setting_value(setting.key())?;
    let selected = setting.selected_choice(&stored);
    let token = runtime.osd_setting_token(setting.key());
    let mut rows: Vec<_> = setting.values().iter().map(|value| {
        let token_text = token.as_deref().unwrap_or("unavailable");
        let mut row = ui_projection::row(
            format!("osd:{}:{value}:{token_text}", setting.key()),
            setting.label(value).into(),
            if *value == selected { "Selected" } else { "" }.into(),
            OttAction::EditSetting(format!("{PREFIX}{}:{value}:{token_text}", setting.key())),
            false,
        );
        row.enabled = token.is_some() && runtime.edit_session.is_none() && runtime.setup.is_none();
        row
    }).collect();
    let selected_row = rows.iter().find(|row| matches!(&row.action,
        OttAction::EditSetting(key) if key.starts_with(&format!("{PREFIX}{}:{selected}:", setting.key()))))
        .map(|row| row.id.clone());
    rows.push(ui_projection::row("osd:back".into(), "Back to Settings".into(),
        String::new(), OttAction::ShowSettings, false));
    Some(OttView {
        page_id: setting.page().into(), revision: runtime.revision,
        title: setting.title().into(), description: setting.help().into(),
        status: runtime.protected_state_status().unwrap_or(&runtime.status).into(),
        rows, selected_row, ..OttView::default()
    })
}

fn selected<'a>(runtime: &Runtime, key: &'a str) -> Result<(Setting, &'a str), &'static str> {
    let mut parts = key.strip_prefix(PREFIX).ok_or(STALE)?.splitn(3, ':');
    let setting = Setting::from_key(parts.next().ok_or(STALE)?).ok_or(STALE)?;
    let value = parts.next().ok_or(STALE)?;
    let token = parts.next().ok_or(STALE)?;
    if runtime.page != setting.page() || !setting.values().contains(&value)
        || runtime.osd_setting_token(setting.key()).as_deref() != Some(token)
    { return Err(STALE); }
    Ok((setting, value))
}

/// Reject retired choices before any policy prompt or native-control mutation.
pub(super) fn validate(runtime: &Runtime, action: &OttAction) -> Result<(), &'static str> {
    if let OttAction::EditSetting(key) = action {
        if key.starts_with(PREFIX) { selected(runtime, key)?; }
    }
    Ok(())
}

pub(super) fn action(runtime: &mut Runtime, action: &OttAction) -> Option<Result<(), &'static str>> {
    if Setting::from_page(runtime.page).is_some()
        && matches!(action, OttAction::Back | OttAction::ShowSettings)
    {
        runtime.page = "settings";
        return Some(Ok(()));
    }
    let OttAction::EditSetting(key) = action else { return None; };
    if let Some(setting) = Setting::from_key(key) {
        return Some((|| {
            if let Some(error) = runtime.settings_write_refusal() { return Err(error); }
            if runtime.edit_session.is_some() || runtime.setup.is_some() { return Err(CLOSE_EDITOR); }
            runtime.osd_setting_token(setting.key()).ok_or(STALE)?;
            let serial = runtime.edit_serial.checked_add(1)
                .ok_or("Editor session limit reached; reopen the player")?;
            runtime.edit_serial = serial;
            runtime.settings_focus = Some(setting.key());
            runtime.page = setting.page();
            Ok(())
        })());
    }
    if !key.starts_with(PREFIX) { return None; }
    Some((|| {
        let (setting, value) = selected(runtime, key)?;
        if let Some(error) = runtime.settings_write_refusal() { return Err(error); }
        if runtime.edit_session.is_some() || runtime.setup.is_some() { return Err(CLOSE_EDITOR); }
        if runtime.osd_setting_value(setting.key()).as_deref() == Some(value) { return Ok(()); }
        let candidate = runtime.osd_setting_candidate(setting.key(), value, runtime.edit_serial)?;
        runtime.save(candidate, None, false)
    })())
}
