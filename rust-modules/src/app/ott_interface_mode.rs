//! The local picker shares the persisted player-settings field and write door.
//! Presentation changes never navigate the source owner or restart playback.
use super::*;
use plx_machine::machine::UiMode;

pub(super) fn label(mode: UiMode) -> &'static str {
    match mode {
        UiMode::Classic => "Classic",
        UiMode::Modern => "Modern",
    }
}

pub(super) fn presentation_action(runtime: &Runtime, action: &OttAction) -> bool {
    matches!(action, OttAction::EditSetting(key)
        if key == "interfaceMode" || key.starts_with("interfaceMode.select:"))
        || (runtime.page == "interface-mode"
            && matches!(action, OttAction::Back | OttAction::ShowSettings))
}

pub(super) fn view(runtime: &Runtime) -> Option<OttView> {
    if runtime.page != "interface-mode" {
        return None;
    }
    let selected = runtime.interface_mode();
    let token = runtime.interface_mode_token();
    let enabled = public_preferences::write_refusal(runtime).is_none();
    let mut rows: Vec<_> = [UiMode::Classic, UiMode::Modern]
        .into_iter()
        .map(|mode| {
            ui_projection::row(
                format!("interface:{}", mode.index()),
                label(mode).into(),
                if mode == selected { "Selected" } else { "" }.into(),
                OttAction::EditSetting(format!("interfaceMode.select:{}:{token}", mode.index())),
                false,
            )
        })
        .collect();
    for row in &mut rows {
        row.enabled = enabled;
    }
    rows.push(ui_projection::row(
        "interface:back".into(),
        "Back to Settings".into(),
        String::new(),
        OttAction::ShowSettings,
        false,
    ));
    Some(OttView {
        page_id: "interface-mode".into(),
        revision: runtime.revision,
        title: "Interface mode".into(),
        description: "Choose how the player looks".into(),
        status: public_preferences::write_refusal(runtime).unwrap_or(&runtime.status).into(),
        rows,
        selected_row: Some(format!("interface:{}", selected.index())),
        ..OttView::default()
    })
}

pub(super) fn action(
    runtime: &mut Runtime,
    action: &OttAction,
) -> Option<Result<(), &'static str>> {
    if runtime.page == "interface-mode"
        && matches!(action, OttAction::Back | OttAction::ShowSettings)
    {
        runtime.page = "settings";
        return Some(Ok(()));
    }
    let OttAction::EditSetting(key) = action else {
        return None;
    };
    if key == "interfaceMode" {
        return Some((|| {
            if let Some(error) = public_preferences::write_refusal(runtime) {
                return Err(error);
            }
            if runtime.edit_session.is_some() || runtime.setup.is_some() {
                return Err("Close the editor before changing interface mode");
            }
            // Share the existing editor session allocator: a retired picker must
            // not authorize a later picker, even when its source is unchanged.
            let session = runtime
                .edit_serial
                .checked_add(1)
                .ok_or("Editor session limit reached; reopen the player")?;
            runtime.edit_serial = session;
            runtime.settings_focus = Some("interfaceMode");
            runtime.page = "interface-mode";
            Ok(())
        })());
    }
    let selection = key.strip_prefix("interfaceMode.select:")?;
    Some((|| {
        let (index, token) = selection
            .split_once(':')
            .ok_or("Interface selection changed; select the mode again")?;
        let mode = match index {
            "0" => UiMode::Classic,
            "1" => UiMode::Modern,
            _ => return Err("Interface selection changed; select the mode again"),
        };
        if runtime.page != "interface-mode" || token != runtime.interface_mode_token() {
            return Err("Interface selection changed; select the mode again");
        }
        if let Some(error) = public_preferences::write_refusal(runtime) {
            return Err(error);
        }
        if runtime.edit_session.is_some() || runtime.setup.is_some() {
            return Err("Close the editor before changing interface mode");
        }
        if runtime.saved.settings.get("interfaceMode") == Some(&json!(mode.index())) {
            return Ok(());
        }
        runtime.save_public_preference(runtime.interface_mode_candidate(mode),
            public_preferences::Patch::InterfaceMode(mode.index() as u8), None,
            ok(json!({"persisted":true})))
    })())
}
