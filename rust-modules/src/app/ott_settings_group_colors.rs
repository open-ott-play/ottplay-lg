//! Color selection stages one canonical value in the existing owned Settings draft.
//! Picking a swatch or scheme never publishes, saves, or changes the playing source.
use super::*;
use plx_ui::palette::{Config, Role, KEY, SWATCHES};

fn config(runtime: &Runtime) -> Result<Config, &'static str> {
    // Keep malformed Saved bytes in the draft CAS baseline, but let the user
    // repair them explicitly. Merely opening or cancelling never writes a value.
    Ok(displayed(runtime, KEY).as_ref().and_then(Value::as_str)
        .and_then(|text| Config::decode(text).ok()).unwrap_or_default())
}

pub(super) fn rows(runtime: &Runtime) -> Vec<OttRow> {
    let Ok(config) = config(runtime) else { return Vec::new(); };
    if let Some(role) = runtime.settings_groups.picker.and_then(Role::find) {
        let current = config.hex(role);
        let mut choices = vec![("inherit".to_owned(), "Player default".to_owned(), current.is_none())];
        if let Some(value) = current.as_ref().filter(|value| !SWATCHES.contains(&value.as_str())) {
            choices.push((value.clone(), format!("Current: {value}"), true));
        }
        choices.extend(SWATCHES.into_iter().map(|hex| (hex.to_owned(), hex.to_owned(), current.as_deref() == Some(hex))));
        let mut rows: Vec<_> = choices.into_iter().map(|(value, label, selected)|
            action_row(runtime, &format!("choose:{}:{value}",role.id()), &label,
                if selected { "Selected".into() } else { String::new() })).collect();
        rows.insert(0, action_row(runtime, &format!("hex:{}",role.id()), "Enter HEX color", "#RRGGBB; leave empty for Player default".into()));
        return rows;
    }
    let mut rows: Vec<_> = Role::ALL.into_iter().map(|role|
        action_row(runtime, &format!("pick:{}",role.id()), role.title(),
            config.hex(role).unwrap_or_else(|| "Player default".into()))).collect();
    for (id, label) in [("midnight","Midnight"),("ocean","Ocean"),("warm-graphite","Warm graphite"),("high-contrast","High contrast"),("light","Light"),("inherit","Reset colors")] {
        rows.push(action_row(runtime, &format!("scheme:{id}"), label,
            if Config::preset(id) == Some(config) { "Selected".into() } else { String::new() }));
    }
    rows
}

pub(super) fn edit(runtime: &mut Runtime, command: &str, address: draft::Address) -> Result<(), &'static str> {
    if let Some(id) = command.strip_prefix("pick:") {
        if runtime.settings_groups.picker.is_some() { return Err(STALE); }
        let role = Role::find(id).ok_or(STALE)?;
        runtime.settings_groups.picker = Some(role.id());
        return Ok(());
    }
    let (next, focus) = if let Some(id) = command.strip_prefix("scheme:") {
        if runtime.settings_groups.picker.is_some() { return Err(STALE); }
        (Config::preset(id).ok_or(STALE)?, command.to_owned())
    } else {
        let (id, value) = command.strip_prefix("choose:").and_then(|s| s.split_once(':')).ok_or(STALE)?;
        let role = Role::find(id).ok_or(STALE)?;
        if runtime.settings_groups.picker != Some(role.id()) { return Err(STALE); }
        (config(runtime)?.with(role, if value == "inherit" { None } else { Some(value) })?, format!("pick:{id}"))
    };
    let value = next.encode();
    // Validate through the same native setting candidate as the final merge.
    runtime.ui_color_candidate(&value, address.id)?;
    runtime.settings_groups.draft.stage(address, KEY, json!(value), |_, value|
        Config::parse(value.as_str().ok_or(STALE)?).map(|_| ())).map_err(error)?;
    runtime.settings_groups.picker = None;
    runtime.settings_groups.focus = Some(focus);
    Ok(())
}

const EDITOR_PREFIX: &str = "settings.color.";
#[derive(Clone)]
pub(super) struct Editor {
    pub(super) session: EditorSession,
    address: draft::Address,
    role: Role,
}

pub(super) fn presentation_action(runtime: &Runtime, action: &OttAction) -> bool {
    matches!(runtime.settings_groups.section, Some(Section::Draft(Group::Colors)))
        && matches!(action, OttAction::SubmitSetting { .. } | OttAction::CancelSetting { .. })
}

fn owns_action(runtime: &Runtime, action: &OttAction) -> bool {
    let Some(held) = runtime.settings_groups.color_editor.as_ref() else { return false; };
    match action {
        OttAction::SubmitSetting { session, key, .. } => *session == held.session.id && key == &held.session.key,
        OttAction::CancelSetting { session } => *session == held.session.id,
        _ => false,
    }
}

fn editor_spec(runtime: &Runtime, role: Role) -> Result<plx_screens::ott::OttEditor, &'static str> {
    use plx_screens::ott::{OttEditor, OttEditorKind, OttTextPolicy};
    Ok(OttEditor { session: 0, kind: OttEditorKind::Setting, text_policy: OttTextPolicy::Literal,
        key: format!("{EDITOR_PREFIX}{}", role.id()), title: format!("{} — HEX #RRGGBB", role.title()),
        value: config(runtime)?.hex(role).unwrap_or_default(), secret: false, limit: 7 })
}

fn stage_hex(runtime: &mut Runtime, held: &Editor, value: &str) -> Result<(), &'static str> {
    if runtime.edit_session.as_ref() != Some(&held.session)
        || address(runtime)? != held.address
        || !matches!(runtime.settings_groups.section, Some(Section::Draft(Group::Colors)))
        || runtime.settings_groups.picker != Some(held.role.id()) { return Err(STALE); }
    if runtime.settings_groups.draft.pending().is_some() { return Err(BUSY); }
    if let Some(error) = runtime.settings_write_refusal() { return Err(error); }
    // The UI accepts lowercase and an omitted '#'; remote writes remain canonical-only.
    let text = value.trim();
    let canonical = if text.is_empty() { None }
        else { Some(format!("#{}", text.strip_prefix('#').unwrap_or(text).to_ascii_uppercase())) };
    let next = config(runtime)?.with(held.role, canonical.as_deref())
        .map_err(|_| "Enter six hexadecimal digits, or leave empty for Player default")?;
    let encoded = next.encode();
    runtime.ui_color_candidate(&encoded, held.address.id)?;
    runtime.settings_groups.draft.stage(held.address, KEY, json!(encoded), |_, value|
        Config::parse(value.as_str().ok_or(STALE)?).map(|_| ())).map_err(error)?;
    Ok(())
}

/// Reuse the ordinary addressed text editor, but only stage the owned palette.
/// Its Save returns to the group; Save Settings alone reaches the Saved writer.
pub(super) fn action(runtime: &mut Runtime, app: &mut App,
    action: &OttAction) -> Option<Result<(), &'static str>> {
    if let OttAction::EditSetting(key) = action {
        let (nonce, command) = key.strip_prefix(PREFIX)?.split_once(':')?;
        let id = command.strip_prefix("hex:")?;
        return Some((|| {
            if nonce != runtime.settings_groups.nonce.to_string()
                || runtime.edit_session.is_some() || runtime.settings_groups.color_editor.is_some()
                || !matches!(runtime.settings_groups.section, Some(Section::Draft(Group::Colors))) { return Err(STALE); }
            let captured = address(runtime)?;
            let role = Role::find(id).ok_or(STALE)?;
            if runtime.settings_groups.picker != Some(role.id())
                || current_ui_owner(app) != Some(captured.owner)
                || app.pages.has_pending_navigation() { return Err(STALE); }
            if runtime.settings_groups.draft.pending().is_some() { return Err(BUSY); }
            if let Some(error) = runtime.settings_write_refusal() { return Err(error); }
            let spec = editor_spec(runtime, role)?;
            let next_nonce = runtime.settings_groups.nonce.checked_add(1).ok_or(STALE)?;
            runtime.open_setting_editor(app, spec)?;
            let session = runtime.edit_session.clone().ok_or(STALE)?;
            runtime.settings_groups.color_editor = Some(Editor { session, address: captured, role });
            runtime.settings_groups.nonce = next_nonce;
            Ok(())
        })());
    }
    let held = runtime.settings_groups.color_editor.clone();
    let is_color_submit = matches!(action, OttAction::SubmitSetting { key, .. } if key.starts_with(EDITOR_PREFIX));
    if !presentation_action(runtime, action) && !is_color_submit { return None; }
    Some((|| {
        let held = held.ok_or(STALE)?;
        if !owns_action(runtime, action) || runtime.edit_session.as_ref() != Some(&held.session)
            || current_ui_owner(app) != Some(held.session.owner) { return Err(STALE); }
        if let OttAction::SubmitSetting { value, .. } = action {
            let next_nonce = runtime.settings_groups.nonce.checked_add(1).ok_or(STALE)?;
            let result = stage_hex(runtime, &held, value);
            // An invalid input remains editable. Never release or acknowledge a different form.
            runtime.editor_result(app, &held.session, result.as_ref().err().map(|error| (*error).into()));
            if result.is_ok() {
                runtime.settings_groups.color_editor = None;
                runtime.settings_groups.nonce = next_nonce;
                runtime.settings_groups.picker = None;
                runtime.settings_groups.focus = Some(format!("pick:{}", held.role.id()));
            }
        } else {
            runtime.cancel_editor();
            runtime.settings_groups.color_editor = None;
            runtime.settings_groups.focus = Some(format!("hex:{}", held.role.id()));
        }
        Ok(())
    })())
}
