//! Localization at the native UI boundary. Runtime diagnostics, saved values,
//! source identities and provider metadata retain their original bytes.
use plx_platform::i18n::{self, msg, LocaleContext};
use plx_screens::ott::{OttAction, OttView};

pub(super) use plx_screens::ott::ui_text::text;
use plx_screens::ott::ui_text::{text_in, setting_text_in};

pub(super) fn view(view: OttView) -> OttView { view_in(view, i18n::current()) }

// Sleep labels belong to an application setting, never to identically named
// user media. Keep localization at this boundary, out of the runtime deadline.
fn sleep_choice_text(source: &str, cx: &LocaleContext) -> Option<String> {
    let minutes = match source {
        "Off" => return Some(msg::widgets_toggle_off_in(cx).into()),
        "30 min" => 30, "60 min" => 60, "120 min" => 120, "180 min" => 180,
        _ => return None,
    };
    Some(msg::ott_sleep_minutes_in(cx, minutes))
}

fn sleep_view_text(view: &mut OttView, cx: &LocaleContext) {
    let picker = view.page_id == "sleep-timer";
    if !picker && view.page_id != "settings" { return; }
    if picker {
        if view.title == "Sleep timer" { view.title = msg::ott_sleep_timer_in(cx).into(); }
        if view.description == "Standby after no input" {
            view.description = msg::ott_sleep_idle_in(cx).into();
        }
    }
    for row in &mut view.rows {
        match &row.action {
            OttAction::EditSetting(key) if key == "sleepTimeout" && row.id == "sleepTimeout" => {
                if row.title == "Sleep timer" { row.title = msg::ott_sleep_timer_in(cx).into(); }
                if let Some(detail) = sleep_choice_text(&row.detail, cx) { row.detail = detail; }
            }
            OttAction::EditSetting(key) if picker => {
                let Some(index) = row.id.strip_prefix("sleep:") else { continue; };
                if matches!(index, "0" | "1" | "2" | "3" | "4")
                    && key.starts_with(&format!("sleepTimeout.select:{index}:")) {
                    if let Some(title) = sleep_choice_text(&row.title, cx) { row.title = title; }
                    row.detail = text_in(&row.detail, cx).into();
                }
            }
            OttAction::ShowSettings if picker && row.id == "sleep:back" => {
                row.title = text_in(&row.title, cx).into();
            }
            _ => {}
        }
    }
}

fn display_choice_page(page: &str) -> bool {
    matches!(page, "osd-opacity" | "permanent-clock" | "classic-page-size"
        | "classic-list-position" | "classic-list-arrows" | "numeric-seek-step")
}

fn show_progress_view_text(view: &mut OttView, cx: &LocaleContext) {
    if view.page_id != "classic-show-progress" { return; }
    if view.title == "Show program progress" {
        view.title = msg::ott_classic_show_progress_in(cx).into();
    }
    if view.description == "Show current-programme progress in Classic channel lists" {
        view.description = msg::ott_classic_show_progress_help_in(cx).into();
    }
    for row in &mut view.rows {
        match &row.action {
            OttAction::EditSetting(key) => {
                let Some(choice) = key.strip_prefix("osd.select:showProgress:") else { continue; };
                let Some((value, token)) = choice.split_once(':') else { continue; };
                if token.is_empty() || row.id != format!("osd:showProgress:{choice}") { continue; }
                let (source, translated) = match value {
                    "0" => ("Off", msg::widgets_toggle_off_in(cx)),
                    "1" => ("On", msg::widgets_toggle_on_in(cx)),
                    _ => continue,
                };
                if row.title == source { row.title = translated.into(); }
                if row.detail == "Selected" { row.detail = msg::ott_interface_selected_in(cx).into(); }
            }
            OttAction::ShowSettings if row.id == "osd:back" && row.title == "Back to Settings" => {
                row.title = msg::settings_back_in(cx).into();
            }
            _ => {}
        }
    }
}

// Group headings and draft controls are app-owned only on the Settings page.
// In particular, never add short group names to the global text matcher.
fn settings_text_in<'a>(source: &'a str, cx: &LocaleContext) -> &'a str {
    match source {
        "Interface" => msg::ott_settings_group_interface_in(cx),
        "Lists" => msg::ott_settings_group_lists_in(cx),
        "Infobar" => msg::ott_settings_group_infobar_in(cx),
        "Buttons" => msg::ott_settings_group_buttons_in(cx),
        "Remote control" => msg::ott_settings_group_remote_in(cx),
        "Parental control" => msg::ott_settings_group_parental_in(cx),
        "Save Settings" => msg::ott_settings_save_in(cx),
        "Changes are saved only with Save Settings" => msg::ott_settings_draft_help_in(cx),
        "Return to the group without changing this value" => msg::ott_settings_value_return_in(cx),
        "Settings changed; reopen this group" => msg::ott_settings_changed_in(cx),
        "This group is being saved" => msg::ott_settings_saving_in(cx),
        "Settings saved" => msg::ott_settings_saved_in(cx),
        "Separate settings" => msg::ott_settings_separate_in(cx),
        "These actions save separately" => msg::ott_settings_separate_help_in(cx),
        "Closing does not undo changes already saved" => msg::ott_settings_separate_close_help_in(cx),
        "Interface text scale (%)" => msg::ott_settings_font_scale_in(cx),
        "Volume step (%)" => msg::ott_volume_step_in(cx),
        "Information display time (seconds)" => msg::ott_info_timeout_in(cx),
        "Sleep timer" => msg::ott_sleep_timer_in(cx),
        "Show channel number in list" => msg::ott_settings_show_number_in(cx),
        "Show channel name in list" => msg::ott_settings_show_name_in(cx),
        "Show program name" => msg::ott_settings_show_program_in(cx),
        "Show description" => msg::ott_settings_show_description_in(cx),
        "Show program progress" => msg::ott_classic_show_progress_in(cx),
        _ => text_in(source, cx),
    }
}

fn settings_field_title(key: &str) -> Option<&'static str> {
    Some(match key {
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
        "parental.unlock" => "Unlock parental access",
        "parental.create" => "Set parental PIN",
        "parental.disable" => "Disable parental PIN",
        "parental.channels" => "Toggle channel protection",
        "parental.settings" => "Toggle settings protection",
        "parental.providers" => "Toggle provider selection protection",
        _ => return None,
    })
}

fn settings_choice_text(key: &str, source: &str, cx: &LocaleContext) -> Option<String> {
    match key {
        "sleepTimeout" => sleep_choice_text(source, cx),
        "permanentTime" if matches!(source, "Off" | "Background" | "Transparent") => Some(text_in(source, cx).into()),
        "listPosition" if matches!(source, "Left" | "Right") => Some(text_in(source, cx).into()),
        "arrowFun" if matches!(source, "Paging" | "Volume" | "Categories / details") => Some(text_in(source, cx).into()),
        "showNumber" | "showName" | "showProgram" | "showDescription" | "showProgress" => match source {
            "Yes" => Some(msg::ott_settings_yes_in(cx).into()),
            "No" => Some(msg::ott_settings_no_in(cx).into()),
            _ => None,
        },
        // Numeric values and units retain the runtime's exact presentation.
        _ => None,
    }
}

fn settings_draft_command(key: &str) -> Option<&str> {
    let (nonce, command) = key.strip_prefix("settings.draft:")?.split_once(':')?;
    if nonce.parse::<u64>().ok()?.to_string() != nonce { return None; }
    Some(command)
}

fn settings_view_text(view: &mut OttView, cx: &LocaleContext) {
    if view.page_id != "settings" { return; }
    view.status = match view.status.as_str() {
        "Settings changed; reopen this group" | "This group is being saved" | "Settings saved" =>
            settings_text_in(&view.status, cx).into(),
        _ => view.status.clone(),
    };
    let mut has_draft = false;
    let mut has_group = false;
    for row in &mut view.rows {
        let OttAction::EditSetting(key) = &row.action else { continue; };
        if row.id != *key { continue; }
        if let Some(group) = key.strip_prefix("settings.open:") {
            let expected = match group {
                "interface" => "Interface", "lists" => "Lists", "channels" => "Channel list",
                "infobar" => "Infobar", "buttons" => "Buttons", "menu" => "Menu items",
                "profiles" => "Profiles", "remote" => "Remote control", "parental" => "Parental control",
                _ => continue,
            };
            has_group = true;
            if row.title == expected { row.title = settings_text_in(&row.title, cx).into(); }
            if row.detail == "These actions save separately" {
                row.detail = msg::ott_settings_separate_help_in(cx).into();
            }
        } else if let Some(command) = settings_draft_command(key) {
            match command {
                "save" | "back" => {
                    has_draft = true;
                    if (command == "save" && row.title == "Save Settings")
                        || (command == "back" && matches!(row.title.as_str(), "Cancel" | "Back to Settings")) {
                        row.title = settings_text_in(&row.title, cx).into();
                    }
                }
                "reset-menu" => {
                    has_draft = true;
                    if row.title == "Reset menu items" { row.title = text_in(&row.title, cx).into(); }
                    if row.detail == "Show all menu items" { row.detail = text_in(&row.detail, cx).into(); }
                }
                _ => {
                    if let Some(field) = command.strip_prefix("pick:") {
                        let Some(title) = settings_field_title(field).filter(|_| !field.starts_with("parental.")) else { continue; };
                        has_draft = true;
                        if row.title == title { row.title = settings_text_in(&row.title, cx).into(); }
                        if let Some(detail) = settings_choice_text(field, &row.detail, cx) { row.detail = detail; }
                    } else if let Some((field, value)) = command.strip_prefix("choose:").and_then(|rest| rest.split_once(':')) {
                        if settings_field_title(field).is_none() || field.starts_with("parental.")
                            || value.parse::<u64>().is_err() { continue; }
                        has_draft = true;
                        if let Some(title) = settings_choice_text(field, &row.title, cx) { row.title = title; }
                        if row.detail == "Selected" { row.detail = msg::ott_interface_selected_in(cx).into(); }
                    } else if let Some(item) = command.strip_prefix("menu:").and_then(plx_appkit::menu_visibility::Item::find) {
                        has_draft = true;
                        if row.title == item.title() {
                            row.title = match item {
                                plx_appkit::menu_visibility::Item::Audio => msg::ott_menu_audio_in(cx),
                                plx_appkit::menu_visibility::Item::Subtitles => msg::ott_menu_subtitles_in(cx),
                                _ => text_in(&row.title, cx),
                            }.into();
                        }
                        if matches!(row.detail.as_str(), "Visible" | "Hidden") { row.detail = text_in(&row.detail, cx).into(); }
                    }
                }
            }
        } else if (key == "uiFontScale" || key.starts_with("parental."))
            && settings_field_title(key) == Some(row.title.as_str()) {
            row.title = setting_text_in(key, &row.title, cx).into();
        }
    }
    // A Settings page alone does not make an arbitrary heading app-owned.
    if has_group || has_draft { view.title = settings_text_in(&view.title, cx).into(); }
    // Old flat Settings descriptions can be user profile names or server URLs.
    if has_draft && matches!(view.description.as_str(), "Changes are saved only with Save Settings"
        | "Return to the group without changing this value" | "Closing does not undo changes already saved") {
        view.description = settings_text_in(&view.description, cx).into();
    }
}

fn view_in(mut view: OttView, cx: &LocaleContext) -> OttView {
    settings_view_text(&mut view, cx);
    sleep_view_text(&mut view, cx);
    show_progress_view_text(&mut view, cx);
    view.title = if view.page_id == "m3u-media-links" { msg::ott_m3u_media_links_in(cx) }
        else { text_in(&view.title, cx) }.into();
    if view.page_id.split(':').next() == Some("profile-setup") || view.page_id == "menu-items" || view.page_id == "interface-mode" || display_choice_page(&view.page_id) {
        view.description = text_in(&view.description, cx).into();
    }
    let usb_browser = view.page_id.split(':').next() == Some("usb-folders");
    if usb_browser && (view.page_id.ends_with(':') || view.page_id == "usb-folders:opening") {
        view.description = text_in(&view.description, cx).into();
    }
    for row in &mut view.rows {
        match &row.action {
            OttAction::SelectProfile(_) if view.page_id == "profiles:m3u"
                && row.id.starts_with("action/profile.load/") =>
                row.title = msg::ott_profile_load_playlist_in(cx).into(),
            OttAction::EditProfile { .. } => {
                if let Some(name) = row.content_title.as_deref().filter(|name| !name.is_empty()) {
                    row.title = msg::ott_profile_edit_in(cx, name);
                }
            }
            OttAction::ShowM3uMediaLinks => row.title = msg::ott_m3u_media_links_in(cx).into(),
            OttAction::ShowProfiles if view.page_id == "m3u-media-links" && row.id == "m3u-media.back" =>
                row.title = msg::settings_back_in(cx).into(),
            OttAction::EditM3uMediaLink { .. } => row.detail = match row.detail.as_str() {
                "Set" => msg::ott_profile_set_in(cx),
                "Not set" => msg::ott_profile_not_set_in(cx),
                other => other,
            }.into(),
            _ => {}
        }
        if let OttAction::EditSetting(key) = &row.action {
            if view.page_id == "settings" && row.id == *key
                && matches!(key.as_str(), "volumeStep" | "infoTimeout") {
                row.title = setting_text_in(key, &row.title, cx).into();
            }
        }
        if usb_browser && row.id.starts_with("usb.") && !row.id.starts_with("usb.folder:") {
            row.title = text_in(&row.title, cx).into();
            if row.id == "usb.cancel" || row.id == "usb.more" {
                row.detail = text_in(&row.detail, cx).into();
            }
        }
        // Never translate a profile name, category name, media title or source path.
        let owned = match &row.action {
            OttAction::MenuVisibility { .. } => view.page_id == "menu-items",
            OttAction::ShowSettings => (view.page_id == "menu-items" && row.id == "menu:back")
                || (view.page_id == "interface-mode" && row.id == "interface:back")
                || (display_choice_page(&view.page_id) && row.id == "osd:back")
                || (row.id == "action/settings"
                    && (view.page_id == "providers" || view.page_id.starts_with("profiles:"))),
            OttAction::EditSetting(key) => matches!(key.as_str(),
                "profile.add" | "profile.name" | "profile.source" | "profile.history_hours" | "remote.address" | "remote.find" | "native.settings"
                | "native.audio" | "native.subtitles" | "plex.connect" | "menu.items" | "interfaceMode" | "osdOpacity" | "permanentTime"
                | "pageSize" | "listPosition" | "arrowFun" | "seek13Duration")
                || key.starts_with("profile.setup.")
                || (view.page_id == "interface-mode" && key.starts_with("interfaceMode.select:"))
                || (view.page_id == "osd-opacity" && key.starts_with("osd.select:osdOpacity:"))
                || (view.page_id == "permanent-clock" && key.starts_with("osd.select:permanentTime:"))
                || (view.page_id == "classic-page-size" && key.starts_with("osd.select:pageSize:"))
                || (view.page_id == "classic-list-position" && key.starts_with("osd.select:listPosition:"))
                || (view.page_id == "classic-list-arrows" && key.starts_with("osd.select:arrowFun:"))
                || (view.page_id == "numeric-seek-step" && key.starts_with("osd.select:seek13Duration:")),
            OttAction::SelectProvider(_) | OttAction::ShowFavorites => true,
            OttAction::SelectCategory(category) | OttAction::SelectCategoryAt { category, .. } => row.id == "all" && category.is_empty(),
            _ => false,
        };
        if owned {
            // These doors reuse the labels of the native track panels. Keep the
            // mapping on the exact app action so identically named media,
            // profiles and user headings retain their original bytes.
            row.title = match &row.action {
                OttAction::EditSetting(key) if key == "native.audio" => match row.title.as_str() {
                    "Audio tracks" => msg::widgets_tracks_audio_in(cx),
                    other => other,
                },
                OttAction::EditSetting(key) if key == "native.subtitles" => match row.title.as_str() {
                    "Subtitles and appearance" => msg::widgets_tracks_subtitles_in(cx),
                    other => other,
                },
                OttAction::MenuVisibility { .. } => match row.title.as_str() {
                    "Audio tracks" => msg::ott_menu_audio_in(cx),
                    "Subtitles and appearance" => msg::ott_menu_subtitles_in(cx),
                    _ => text_in(&row.title, cx),
                },
                _ => text_in(&row.title, cx),
            }.into();
            row.detail = text_in(&row.detail, cx).into();
        } else if matches!(&row.action, OttAction::EditSetting(key) if key.starts_with("offline.bind:")) {
            // The label is the device's user-defined folder name; only its hint is ours.
            row.detail = text_in(&row.detail, cx).into();
        }
    }
    view.status = text_in(&view.status, cx).into();
    if view.status.is_empty() && view.catalog_progress.is_some_and(|p| p.complete && !p.loading && p.loaded == 0) {
        view.status = if view.search.as_ref().is_some_and(|search| !search.query.is_empty()) {
            msg::ott_profile_no_matches_in(cx)
        } else {
            msg::ott_profile_empty_in(cx)
        }.into();
    }
    view
}

#[cfg(test)]
mod settings_group_tests {
    use super::*;
    use plx_screens::ott::OttRow;

    fn row(key: &str, title: &str, detail: &str) -> OttRow {
        OttRow { current_programme: None, content_title: None, guide: None, guide_available: false, records_available: false,
            media: None, id: key.into(), title: title.into(), detail: detail.into(),
            description: String::new(), poster: None, enabled: true, favorite: false,
            action: OttAction::EditSetting(key.into()) }
    }

    fn locale(tag: &str) -> LocaleContext {
        LocaleContext::resolve(i18n::Preference::from_tag(tag), None, None, None, None)
    }

    #[test]
    fn group_labels_are_localized_only_for_addressed_settings_controls() {
        for language in i18n::SHIPPED {
            let cx = locale(language.tag());
            for (group, title, expected) in [
                ("interface", "Interface", msg::ott_settings_group_interface_in(&cx)),
                ("lists", "Lists", msg::ott_settings_group_lists_in(&cx)),
                ("channels", "Channel list", msg::ott_channel_list_in(&cx)),
                ("infobar", "Infobar", msg::ott_settings_group_infobar_in(&cx)),
                ("buttons", "Buttons", msg::ott_settings_group_buttons_in(&cx)),
                ("menu", "Menu items", msg::ott_menu_items_in(&cx)),
                ("profiles", "Profiles", msg::ott_profiles_in(&cx)),
                ("remote", "Remote control", msg::ott_settings_group_remote_in(&cx)),
                ("parental", "Parental control", msg::ott_settings_group_parental_in(&cx)),
            ] {
                let key = format!("settings.open:{group}");
                let own = row(&key, title, "These actions save separately");
                let mut media = own.clone();
                media.action = OttAction::Play(key.clone());
                let mut profile = own.clone();
                profile.action = OttAction::SelectProfile(key.clone());
                let mut category = own.clone();
                category.action = OttAction::SelectCategory(key.clone());
                let mut wrong_id = own.clone();
                wrong_id.id = "unrelated".into();
                let view = view_in(OttView { page_id: "settings".into(), title: title.into(),
                    rows: vec![own.clone(), media, profile, category, wrong_id], ..Default::default() }, &cx);
                assert_eq!(view.title, expected);
                assert_eq!(view.rows[0].title, expected);
                assert_eq!(view.rows[0].detail, msg::ott_settings_separate_help_in(&cx));
                assert_eq!(view.rows[0].action, own.action);
                assert_eq!(view.rows[0].id, key);
                for item in &view.rows[1..] {
                    assert_eq!(item.title, title);
                    assert_eq!(item.detail, "These actions save separately");
                }
            }
        }
    }

    #[test]
    fn draft_fields_choices_and_menu_rows_keep_values_and_addresses() {
        for language in i18n::SHIPPED {
            let cx = locale(language.tag());
            for (field, title, expected, value, display) in [
                ("uiFontScale", "Interface text scale (%)", msg::ott_settings_font_scale_in(&cx), "120%", "120%".to_owned()),
                ("volumeStep", "Volume step (%)", msg::ott_volume_step_in(&cx), "5%", "5%".to_owned()),
                ("infoTimeout", "Information display time (seconds)", msg::ott_info_timeout_in(&cx), "7 s", "7 s".to_owned()),
                ("sleepTimeout", "Sleep timer", msg::ott_sleep_timer_in(&cx), "30 min", msg::ott_sleep_minutes_in(&cx, 30)),
                ("osdOpacity", "OSD background level", msg::ott_osd_level_in(&cx), "7", "7".to_owned()),
                ("permanentTime", "Permanent clock", msg::ott_osd_clock_in(&cx), "Transparent", msg::ott_osd_transparent_in(&cx).to_owned()),
                ("pageSize", "Classic list rows", msg::ott_classic_rows_in(&cx), "15", "15".to_owned()),
                ("listPosition", "Classic list side", msg::ott_classic_side_in(&cx), "Left", msg::ott_classic_left_in(&cx).to_owned()),
                ("arrowFun", "Classic list arrows", msg::ott_classic_arrows_in(&cx), "Categories / details", msg::ott_classic_context_in(&cx).to_owned()),
                ("seek13Duration", "Seek step for 1/3 (seconds)", msg::ott_classic_seek_in(&cx), "30 s", "30 s".to_owned()),
            ] {
                let key = format!("settings.draft:42:pick:{field}");
                let original = row(&key, title, value);
                let view = view_in(OttView { page_id: "settings".into(), title: title.into(),
                    rows: vec![original.clone()], ..Default::default() }, &cx);
                assert_eq!(view.title, expected);
                assert_eq!(view.rows[0].title, expected);
                assert_eq!(view.rows[0].detail, display);
                assert_eq!(view.rows[0].action, original.action);
                assert_eq!(view.rows[0].id, key);
            }
            for (key, label, expected) in [
                ("settings.draft:42:choose:sleepTimeout:1", "30 min", msg::ott_sleep_minutes_in(&cx, 30)),
                ("settings.draft:42:choose:listPosition:1", "Left", msg::ott_classic_left_in(&cx).to_owned()),
                ("settings.draft:42:choose:volumeStep:5", "5%", "5%".to_owned()),
                ("settings.draft:42:menu:audio", "Audio tracks", msg::ott_menu_audio_in(&cx).to_owned()),
                ("settings.draft:42:menu:subtitles", "Subtitles and appearance", msg::ott_menu_subtitles_in(&cx).to_owned()),
                ("settings.draft:42:reset-menu", "Reset menu items", msg::ott_menu_reset_in(&cx).to_owned()),
            ] {
                let detail = if key.contains(":choose:") { "Selected" }
                    else if key.ends_with(":reset-menu") { "Show all menu items" } else { "Hidden" };
                let original = row(key, label, detail);
                let view = view_in(OttView { page_id: "settings".into(), rows: vec![original.clone()], ..Default::default() }, &cx);
                assert_eq!(view.rows[0].title, expected);
                assert_eq!(view.rows[0].detail, text_in(detail, &cx));
                assert_eq!(view.rows[0].action, original.action);
                assert_eq!(view.rows[0].id, original.id);
            }
        }
    }

    #[test]
    fn draft_save_cancel_help_and_status_are_scoped_without_mutating_wire_copy() {
        for language in i18n::SHIPPED {
            let cx = locale(language.tag());
            for (description, expected) in [
                ("Changes are saved only with Save Settings", msg::ott_settings_draft_help_in(&cx)),
                ("Return to the group without changing this value", msg::ott_settings_value_return_in(&cx)),
                ("Closing does not undo changes already saved", msg::ott_settings_separate_close_help_in(&cx)),
            ] {
                let raw = OttView { page_id: "settings".into(), title: "Separate settings".into(),
                    description: description.into(), status: "Settings changed; reopen this group".into(), rows: vec![
                        row("settings.draft:42:save", "Save Settings", ""),
                        row("settings.draft:42:back", "Cancel", ""),
                    ], ..Default::default() };
                let view = view_in(raw.clone(), &cx);
                assert_eq!(view.title, msg::ott_settings_separate_in(&cx));
                assert_eq!(view.description, expected);
                assert_eq!(view.status, msg::ott_settings_changed_in(&cx));
                assert_eq!(view.rows[0].title, msg::ott_settings_save_in(&cx));
                assert_eq!(view.rows[1].title, msg::settings_cancel_in(&cx));
                assert_eq!(raw.status, "Settings changed; reopen this group");
                assert_eq!(raw.rows[0].title, "Save Settings");
            }
            for (status, expected) in [("This group is being saved", msg::ott_settings_saving_in(&cx)),
                ("Settings saved", msg::ott_settings_saved_in(&cx))] {
                let view = view_in(OttView { page_id: "settings".into(), status: status.into(), ..Default::default() }, &cx);
                assert_eq!(view.status, expected);
                assert_eq!(text_in(status, &cx), status);
            }
        }
    }

    #[test]
    fn ordinary_font_and_parental_labels_require_the_exact_setting_identity() {
        for language in i18n::SHIPPED {
            let cx = locale(language.tag());
            for (source, expected) in [
                ("Interface text scale (20–200%)", msg::ott_settings_font_scale_prompt_in(&cx)),
                ("Enter an integer from 20 to 200", msg::ott_settings_font_scale_invalid_in(&cx)),
            ] {
                assert_eq!(setting_text_in("uiFontScale", source, &cx), expected);
                assert_eq!(setting_text_in("other", source, &cx), source);
                assert_eq!(text_in(source, &cx), source);
            }
            for (key, title, expected) in [
                ("uiFontScale", "Interface text scale (%)", msg::ott_settings_font_scale_in(&cx)),
                ("parental.unlock", "Unlock parental access", msg::ott_settings_parental_unlock_in(&cx)),
                ("parental.create", "Set parental PIN", msg::ott_settings_parental_create_in(&cx)),
                ("parental.disable", "Disable parental PIN", msg::ott_settings_parental_disable_in(&cx)),
                ("parental.channels", "Toggle channel protection", msg::ott_settings_parental_channels_in(&cx)),
                ("parental.settings", "Toggle settings protection", msg::ott_settings_parental_settings_in(&cx)),
                ("parental.providers", "Toggle provider selection protection", msg::ott_settings_parental_providers_in(&cx)),
            ] {
                let view = view_in(OttView { page_id: "settings".into(), rows: vec![row(key, title, "Private profile")], ..Default::default() }, &cx);
                assert_eq!(view.rows[0].title, expected);
                assert_eq!(view.rows[0].detail, "Private profile");
                let wrong = format!("{key}.other");
                let view = view_in(OttView { page_id: "settings".into(), rows: vec![row(&wrong, title, "")], ..Default::default() }, &cx);
                assert_eq!(view.rows[0].title, title);
            }
        }
    }

    #[test]
    fn user_labels_foreign_pages_and_unrecognized_draft_addresses_are_unchanged() {
        let cx = locale("ru");
        let foreign = OttView { page_id: "settings-other".into(), title: "Interface".into(),
            description: "Changes are saved only with Save Settings".into(), rows: vec![
                row("settings.open:interface", "Interface", "These actions save separately"),
                row("settings.draft:42:save", "Save Settings", ""),
                row("uiFontScale", "Interface text scale (%)", "120%"),
            ], ..Default::default() };
        assert_eq!(view_in(foreign.clone(), &cx), foreign);
        for title in ["Interface", "Lists", "Infobar", "Buttons", "Remote control", "Parental control",
            "Save Settings", "Interface text scale (%)", "Unlock parental access"] {
            let mut media = row("settings.draft:42:save", title, "These actions save separately");
            media.action = OttAction::Play("runtime:42:private".into());
            media.content_title = Some(title.into());
            media.description = "Return to the group without changing this value".into();
            let before = OttView { page_id: "channels:private".into(), title: title.into(),
                description: media.description.clone(), status: "Settings saved".into(), rows: vec![media], ..Default::default() };
            assert_eq!(view_in(before.clone(), &cx), before);
        }
        for key in ["settings.open:other", "settings.draft:x:save", "settings.draft:01:save",
            "settings.draft:18446744073709551616:save", "settings.draft:42:save:other",
            "settings.draft:42:choose:other:1", "settings.draft:42:choose:listPosition:x",
            "settings.draft:42:menu:other", "settings.draft:42:pick:parental.unlock"] {
            let before = row(key, "Save Settings", "These actions save separately");
            let view = view_in(OttView { page_id: "settings".into(), description: "Changes are saved only with Save Settings".into(),
                rows: vec![before.clone()], ..Default::default() }, &cx);
            assert_eq!(view.rows[0], before);
            assert_eq!(view.description, "Changes are saved only with Save Settings");
        }
        let before = row("settings.open:interface", "Buttons", "Private profile");
        let view = view_in(OttView { page_id: "settings".into(), rows: vec![before.clone()], ..Default::default() }, &cx);
        assert_eq!(view.rows[0], before);
    }

    #[test]
    fn settings_headings_without_group_or_draft_controls_remain_user_text() {
        for language in i18n::SHIPPED {
            let cx = locale(language.tag());
            for title in ["Interface", "Lists", "Infobar", "Buttons", "Remote control",
                "Parental control", "Separate settings", "Volume step (%)",
                "Information display time (seconds)"] {
                for key in ["settings.open:other", "settings.draft:01:save", "settings.draft:42:save:other"] {
                    let view = view_in(OttView { page_id: "settings".into(), title: title.into(),
                        rows: vec![row(key, "Unrecognized", "")], ..Default::default() }, &cx);
                    assert_eq!(view.title, title);
                }
                let mut media = row("settings.open:interface", title, "");
                media.action = OttAction::Play("media".into());
                let view = view_in(OttView { page_id: "settings".into(), title: title.into(),
                    rows: vec![media], ..Default::default() }, &cx);
                assert_eq!(view.title, title);
            }
        }
    }

    #[test]
    fn channel_draft_fields_and_boolean_pickers_localize_without_changing_values() {
        for language in i18n::SHIPPED {
            let cx = locale(language.tag());
            for (field, title, expected) in [
                ("showNumber", "Show channel number in list", msg::ott_settings_show_number_in(&cx)),
                ("showName", "Show channel name in list", msg::ott_settings_show_name_in(&cx)),
                ("showProgram", "Show program name", msg::ott_settings_show_program_in(&cx)),
                ("showDescription", "Show description", msg::ott_settings_show_description_in(&cx)),
            ] {
                for (value, label, translated) in [
                    (0, "No", msg::ott_settings_no_in(&cx)),
                    (1, "Yes", msg::ott_settings_yes_in(&cx)),
                ] {
                    let key = format!("settings.draft:42:pick:{field}");
                    let original = row(&key, title, label);
                    let view = view_in(OttView { page_id: "settings".into(), title: "Channel list".into(),
                        rows: vec![original.clone()], ..Default::default() }, &cx);
                    assert_eq!(view.title, msg::ott_channel_list_in(&cx));
                    assert_eq!(view.rows[0].title, expected);
                    assert_eq!(view.rows[0].detail, translated);
                    assert_eq!(view.rows[0].id, original.id);
                    assert_eq!(view.rows[0].action, original.action);
                    for detail in ["", "Selected"] {
                        let key = format!("settings.draft:42:choose:{field}:{value}");
                        let original = row(&key, label, detail);
                        let view = view_in(OttView { page_id: "settings".into(), title: title.into(),
                            rows: vec![original.clone()], ..Default::default() }, &cx);
                        assert_eq!(view.title, expected);
                        assert_eq!(view.rows[0].title, translated);
                        assert_eq!(view.rows[0].detail,
                            if detail.is_empty() { "" } else { msg::ott_interface_selected_in(&cx) });
                        assert_eq!(view.rows[0].id, original.id);
                        assert_eq!(view.rows[0].action, original.action);
                    }
                }
            }
        }
    }

    #[test]
    fn channel_settings_words_remain_raw_outside_addressed_draft_controls() {
        let cx = locale("ru");
        for title in ["Show channel number in list", "Show channel name in list",
            "Show program name", "Show description", "Yes", "No"] {
            assert_eq!(text_in(title, &cx), title);
            let key = "settings.draft:42:pick:showNumber";
            let original = row(key, title, "Yes");
            let foreign = OttView { page_id: "channels:private".into(), title: title.into(),
                rows: vec![original.clone()], ..Default::default() };
            assert_eq!(view_in(foreign.clone(), &cx), foreign);
            let mut mismatch = original.clone();
            mismatch.id = "private-row".into();
            let mut media = original;
            media.action = OttAction::Play("runtime:42:private".into());
            media.content_title = Some(title.into());
            for original in [mismatch, media] {
                let view = OttView { page_id: "settings".into(), title: title.into(),
                    rows: vec![original], ..Default::default() };
                assert_eq!(view_in(view.clone(), &cx), view);
            }
        }
        for key in ["settings.draft:01:choose:showNumber:1", "settings.draft:42:choose:unknown:1",
            "settings.draft:42:choose:showNumber:x"] {
            let original = row(key, "Yes", "Selected");
            let view = OttView { page_id: "settings".into(), title: "Show channel number in list".into(),
                rows: vec![original], ..Default::default() };
            assert_eq!(view_in(view.clone(), &cx), view);
        }
        assert!(settings_choice_text("showNumber", "1", &cx).is_none());
        assert!(settings_choice_text("showNumber", "true", &cx).is_none());
        assert!(settings_choice_text("pageSize", "Yes", &cx).is_none());
    }

    #[test]
    fn show_progress_group_field_keeps_boolean_payloads_and_scoped_heading() {
        for language in i18n::SHIPPED {
            let cx = locale(language.tag());
            for (value, label, expected) in [
                (0, "No", msg::ott_settings_no_in(&cx)),
                (1, "Yes", msg::ott_settings_yes_in(&cx)),
            ] {
                let pick = row("settings.draft:42:pick:showProgress", "Show program progress", label);
                let group = view_in(OttView { page_id: "settings".into(), title: "Channel list".into(),
                    rows: vec![pick.clone()], ..Default::default() }, &cx);
                assert_eq!(group.rows[0].title, msg::ott_classic_show_progress_in(&cx));
                assert_eq!(group.rows[0].detail, expected);
                assert_eq!(group.rows[0].id, pick.id);
                assert_eq!(group.rows[0].action, pick.action);
                let choice = row(&format!("settings.draft:42:choose:showProgress:{value}"), label, "Selected");
                let picker = view_in(OttView { page_id: "settings".into(), title: "Show program progress".into(),
                    rows: vec![choice.clone()], ..Default::default() }, &cx);
                assert_eq!(picker.title, msg::ott_classic_show_progress_in(&cx));
                assert_eq!(picker.rows[0].title, expected);
                assert_eq!(picker.rows[0].detail, msg::ott_interface_selected_in(&cx));
                assert_eq!(picker.rows[0].id, choice.id);
                assert_eq!(picker.rows[0].action, choice.action);
                let mut media = pick;
                media.action = OttAction::Play("runtime:42:private".into());
                let raw = OttView { page_id: "settings".into(), title: "Show program progress".into(),
                    rows: vec![media], ..Default::default() };
                assert_eq!(view_in(raw.clone(), &cx), raw);
            }
        }
    }

    #[test]
    fn show_progress_standalone_picker_uses_exact_page_row_and_action_addresses() {
        for language in i18n::SHIPPED {
            let cx = locale(language.tag());
            for (value, label, translated) in [
                ("0", "Off", msg::widgets_toggle_off_in(&cx)),
                ("1", "On", msg::widgets_toggle_on_in(&cx)),
            ] {
                let mut choice = row(&format!("osd:showProgress:{value}:opaque"), label, "Selected");
                choice.action = OttAction::EditSetting(format!("osd.select:showProgress:{value}:opaque"));
                choice.enabled = false;
                let mut back = row("osd:back", "Back to Settings", "");
                back.action = OttAction::ShowSettings;
                let raw = OttView { page_id: "classic-show-progress".into(), title: "Show program progress".into(),
                    description: "Show current-programme progress in Classic channel lists".into(),
                    rows: vec![choice.clone(), back.clone()], selected_row: Some(choice.id.clone()), ..Default::default() };
                let view = view_in(raw.clone(), &cx);
                assert_eq!(view.title, msg::ott_classic_show_progress_in(&cx));
                assert_eq!(view.description, msg::ott_classic_show_progress_help_in(&cx));
                assert_eq!(view.rows[0].title, translated);
                assert_eq!(view.rows[0].detail, msg::ott_interface_selected_in(&cx));
                assert_eq!(view.rows[0].id, choice.id);
                assert_eq!(view.rows[0].action, choice.action);
                assert_eq!(view.rows[0].enabled, choice.enabled);
                assert_eq!(view.selected_row, raw.selected_row);
                assert_eq!(view.rows[1].title, msg::settings_back_in(&cx));
                assert_eq!(view.rows[1].id, back.id);
                assert_eq!(view.rows[1].action, back.action);
                let mut foreign_page = raw;
                foreign_page.page_id = "channels:private".into();
                assert_eq!(view_in(foreign_page.clone(), &cx), foreign_page);
                let mut wrong_id = choice.clone();
                wrong_id.id = "source-owned".into();
                let mut wrong_action = choice.clone();
                wrong_action.action = OttAction::Play("private-media".into());
                let mut wrong_field = choice.clone();
                wrong_field.action = OttAction::EditSetting(format!("osd.select:showNumber:{value}:opaque"));
                let mut wrong_token = choice.clone();
                wrong_token.action = OttAction::EditSetting(format!("osd.select:showProgress:{value}:other"));
                for row in [wrong_id, wrong_action, wrong_field, wrong_token] {
                    let view = view_in(OttView { page_id: "classic-show-progress".into(),
                        rows: vec![row.clone()], ..Default::default() }, &cx);
                    assert_eq!(view.rows[0], row);
                }
            }
            assert_eq!(text_in("Show program progress", &cx), "Show program progress");
            assert_eq!(text_in("Show current-programme progress in Classic channel lists", &cx),
                "Show current-programme progress in Classic channel lists");
        }
    }

    #[test]
    fn neutral_order_errors_and_legacy_aliases_share_the_display_translation() {
        for language in i18n::SHIPPED {
            let cx = locale(language.tag());
            for (current, legacy, expected) in [
                ("The list changed; reopen Actions", "The Local list changed; reopen Actions", msg::ott_order_changed_in(&cx)),
                ("List order is unavailable for this source", "List order is unavailable for this Local source", msg::ott_order_unavailable_in(&cx)),
                ("This list is too large to save its order", "This list is too large to save its order. Use temporary Local shuffle.", msg::ott_order_too_large_in(&cx)),
            ] {
                assert_eq!(text_in(current, &cx), expected);
                assert_eq!(text_in(legacy, &cx), expected);
                let unknown = format!("{current}: source-specific detail");
                assert_eq!(text_in(&unknown, &cx), unknown);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use plx_screens::ott::{OttCatalogProgress, OttRow, OttSearch};

    fn row(id: &str, title: &str, action: OttAction) -> OttRow {
        OttRow { current_programme: None, content_title: None, guide: None, media: None, id: id.into(), title: title.into(), detail: "Not set".into(),
            description: String::new(), poster: None, action, enabled: true, favorite: false, guide_available: false, records_available: false }
    }

    fn locale(tag: &str) -> LocaleContext {
        LocaleContext::resolve(i18n::Preference::from_tag(tag), None, None, None, None)
    }

    #[test]
    fn m3u_load_label_localizes_only_the_owned_load_action() {
        for language in i18n::SHIPPED {
            let cx = locale(language.tag());
            let action = OttAction::SelectProfile("profile-a".into());
            let mut load = row("action/profile.load/opaque", "Load playlist", action.clone());
            load.content_title = Some("Saved name".into());
            let named = row("profile-a", "Load playlist", action.clone());
            let foreign = row("action/profile.load/foreign", "Load playlist", OttAction::Play("media".into()));
            let view = view_in(OttView { page_id: "profiles:m3u".into(),
                rows: vec![load.clone(), named, foreign], ..Default::default() }, &cx);
            assert_eq!(view.rows[0].title, msg::ott_profile_load_playlist_in(&cx));
            assert_eq!(view.rows[0].action, action);
            assert_eq!(view.rows[0].content_title.as_deref(), Some("Saved name"));
            assert_eq!(view.rows[1].title, "Load playlist");
            assert_eq!(view.rows[2].title, "Load playlist");
            let elsewhere = view_in(OttView { page_id: "profiles:stalker".into(),
                rows: vec![load], ..Default::default() }, &cx);
            assert_eq!(elsewhere.rows[0].title, "Load playlist");
        }
    }

    #[test]
    fn profile_edit_label_uses_typed_action_and_clean_name_without_prefix_parsing() {
        for language in i18n::SHIPPED {
            let cx = locale(language.tag());
            let action = OttAction::EditProfile { target: "profile-b".into(), context: "opaque".into() };
            let mut named = row("edit-b", "Edit misleading legacy text", action.clone());
            named.content_title = Some("Edit Settings {name}".into());
            let missing = row("missing", "Edit preserve this raw title", action.clone());
            let mut empty = row("empty", "Edit also preserve this title", action.clone());
            empty.content_title = Some(String::new());
            let mut media = named.clone();
            media.action = OttAction::Play("video".into());
            let before_media = media.title.clone();
            let view = view_in(OttView { page_id: "profiles:m3u".into(),
                rows: vec![named, missing, empty, media], ..Default::default() }, &cx);
            assert_eq!(view.rows[0].title, msg::ott_profile_edit_in(&cx, "Edit Settings {name}"));
            assert_eq!(view.rows[0].content_title.as_deref(), Some("Edit Settings {name}"));
            assert_eq!(view.rows[0].action, action);
            assert_eq!(view.rows[1].title, "Edit preserve this raw title");
            assert_eq!(view.rows[2].title, "Edit also preserve this title");
            assert_eq!(view.rows[3].title, before_media);
        }
    }

    #[test]
    fn m3u_media_link_copy_localizes_controls_but_preserves_target_names_and_tokens() {
        for language in i18n::SHIPPED {
            let cx = locale(language.tag());
            let target = OttAction::EditM3uMediaLink { profile: "profile-b".into(), context: "opaque".into() };
            let mut configured = row("opaque-b", "Media library links…", target.clone());
            configured.detail = "Set".into();
            let unknown = row("opaque-c", "Not set", target.clone());
            let mut custom = row("opaque-d", "Settings", target.clone());
            custom.detail = "Unknown owner detail".into();
            let unrelated = row("unrelated", "Back", OttAction::ShowProfiles);
            let view = view_in(OttView { page_id: "m3u-media-links".into(), title: "Media library links…".into(),
                rows: vec![configured, unknown, custom,
                    row("m3u-media.back", "Back", OttAction::ShowProfiles), unrelated],
                ..Default::default() }, &cx);
            assert_eq!(view.title, msg::ott_m3u_media_links_in(&cx));
            assert_eq!(view.rows[0].title, "Media library links…");
            assert_eq!(view.rows[0].detail, msg::ott_profile_set_in(&cx));
            assert_eq!(view.rows[0].action, target);
            assert_eq!(view.rows[0].id, "opaque-b");
            assert_eq!(view.rows[1].title, "Not set");
            assert_eq!(view.rows[1].detail, msg::ott_profile_not_set_in(&cx));
            assert_eq!(view.rows[2].title, "Settings");
            assert_eq!(view.rows[2].detail, "Unknown owner detail");
            assert_eq!(view.rows[3].title, msg::settings_back_in(&cx));
            assert_eq!(view.rows[4].title, "Back");
            let root = view_in(OttView { page_id: "profiles:m3u".into(), rows: vec![
                row("open", "Media library links…", OttAction::ShowM3uMediaLinks),
                row("profile", "Media library links…", OttAction::SelectProfile("profile".into())),
                row("m3u-media.back", "Back", OttAction::ShowProfiles),
            ], ..Default::default() }, &cx);
            assert_eq!(root.rows[0].title, msg::ott_m3u_media_links_in(&cx));
            assert_eq!(root.rows[1].title, "Media library links…");
            assert_eq!(root.rows[2].title, "Back");
        }
    }

    #[test]
    fn m3u_media_link_fixed_errors_are_projected_without_changing_unknown_diagnostics() {
        for language in i18n::SHIPPED {
            let cx = locale(language.tag());
            assert_eq!(text_in("M3U media settings changed; reopen the editor", &cx), msg::ott_m3u_media_changed_in(&cx));
            assert_eq!(text_in("Invalid attached VPortal link", &cx), msg::ott_m3u_media_invalid_in(&cx));
            assert_eq!(text_in("Unknown attachment error", &cx), "Unknown attachment error");
            assert_eq!(text_in("Invalid attached VPortal link: private detail", &cx), "Invalid attached VPortal link: private detail");
        }
    }

    #[test]
    fn playback_preference_rows_localize_only_owned_labels_and_preserve_values() {
        for language in i18n::SHIPPED {
            let cx = locale(language.tag());
            let mut volume = row("volumeStep", "Volume step (%)", OttAction::EditSetting("volumeStep".into()));
            volume.detail = "5%".into();
            let mut info = row("infoTimeout", "Information display time (seconds)", OttAction::EditSetting("infoTimeout".into()));
            info.detail = "7 s".into();
            let view = view_in(OttView { page_id: "settings".into(), rows: vec![volume, info], ..Default::default() }, &cx);
            assert_eq!(view.rows[0].title, msg::ott_volume_step_in(&cx));
            assert_eq!(view.rows[1].title, msg::ott_info_timeout_in(&cx));
            for (item, key, detail) in [(&view.rows[0], "volumeStep", "5%"), (&view.rows[1], "infoTimeout", "7 s")] {
                assert_eq!(item.id, key);
                assert_eq!(item.action, OttAction::EditSetting(key.into()));
                assert_eq!(item.detail, detail);
                assert!(item.enabled);
                assert!(!item.favorite);
            }
        }
    }

    #[test]
    fn playback_preference_text_preserves_user_names_wrong_keys_and_nonsettings_pages() {
        for language in i18n::SHIPPED {
            let cx = locale(language.tag());
            for (key, label) in [("volumeStep", "Volume step (%)"), ("infoTimeout", "Information display time (seconds)")] {
                let rows = vec![
                    row(key, label, OttAction::SelectProfile("profile".into())),
                    row(key, label, OttAction::SelectCategory("category".into())),
                    row(key, label, OttAction::Play("media".into())),
                    row(key, label, OttAction::EditSetting(format!("{key}.other"))),
                    row("unrelated", label, OttAction::EditSetting(key.into())),
                ];
                let view = view_in(OttView { page_id: "settings".into(), title: label.into(),
                    description: label.into(), rows, ..Default::default() }, &cx);
                assert_eq!(view.title, label);
                assert_eq!(view.description, label);
                for item in view.rows { assert_eq!(item.title, label); }
                let view = view_in(OttView { page_id: "channels:source".into(),
                    rows: vec![row(key, label, OttAction::EditSetting(key.into()))], ..Default::default() }, &cx);
                assert_eq!(view.rows[0].title, label);
            }
        }
    }

    #[test]
    fn playback_preference_editor_text_requires_exact_key_and_fixed_message() {
        for language in i18n::SHIPPED {
            let cx = locale(language.tag());
            for (key, source, localized) in [
                ("volumeStep", "Volume step (%)", msg::ott_volume_step_in(&cx)),
                ("volumeStep", "Volume step (3–10%)", msg::ott_volume_step_prompt_in(&cx)),
                ("volumeStep", "Enter an integer from 3 to 10", msg::ott_volume_step_invalid_in(&cx)),
                ("infoTimeout", "Information display time (seconds)", msg::ott_info_timeout_in(&cx)),
                ("infoTimeout", "Information display time (3–20 seconds)", msg::ott_info_timeout_prompt_in(&cx)),
                ("infoTimeout", "Enter an integer from 3 to 20", msg::ott_info_timeout_invalid_in(&cx)),
            ] {
                assert_eq!(setting_text_in(key, source, &cx), localized);
                assert_eq!(text_in(source, &cx), source, "no global heading translation");
                let other_key = if key == "volumeStep" { "infoTimeout" } else { "volumeStep" };
                assert_eq!(setting_text_in(other_key, source, &cx), source);
                assert_eq!(setting_text_in(&format!("{key}.other"), source, &cx), source);
                let unknown = format!("{source}: source-owned detail");
                assert_eq!(setting_text_in(key, &unknown, &cx), unknown);
            }
            assert_eq!(setting_text_in("profile.name", "Profile name", &cx), msg::ott_profile_name_in(&cx));
        }
    }

    #[test]
    fn osd_choices_localize_only_their_owned_pages_and_actions() {
        for language in i18n::SHIPPED {
            let cx = locale(language.tag());
            let selected = row("osd:permanentTime:2:opaque", "Transparent",
                OttAction::EditSetting("osd.select:permanentTime:2:opaque".into()));
            let mut foreign = selected.clone();
            foreign.id = "source-title".into();
            foreign.action = OttAction::Play("source-title".into());
            let view = view_in(OttView {
                page_id: "permanent-clock".into(), title: "Permanent clock".into(),
                description: "Show a permanent clock during playback".into(),
                rows: vec![selected.clone(), foreign], ..OttView::default()
            }, &cx);
            assert_eq!(view.title, msg::ott_osd_clock_in(&cx));
            assert_eq!(view.description, msg::ott_osd_clock_help_in(&cx));
            assert_eq!(view.rows[0].title, msg::ott_osd_transparent_in(&cx));
            assert_eq!(view.rows[1].title, "Transparent");
            assert_eq!(view.rows[0].id, selected.id);
            assert_eq!(view.rows[0].action, selected.action);
            let other = view_in(OttView { page_id: "channels".into(),
                rows: vec![selected], ..OttView::default() }, &cx);
            assert_eq!(other.rows[0].title, "Transparent");
        }
    }

    #[test]
    fn classic_choices_localize_only_their_own_page_and_keep_source_names() {
        for language in i18n::SHIPPED {
            let cx = locale(language.tag());
            for (page, key, value, label, localized) in [
                ("classic-list-position", "listPosition", "1", "Left", msg::ott_classic_left_in(&cx)),
                ("classic-list-arrows", "arrowFun", "2", "Categories / details", msg::ott_classic_context_in(&cx)),
            ] {
                let selected = row("owned-choice", label,
                    OttAction::EditSetting(format!("osd.select:{key}:{value}:opaque")));
                let foreign = row("source-title", label, OttAction::Play("source-title".into()));
                let view = view_in(OttView {
                    page_id: page.into(), rows: vec![selected.clone(), foreign], ..OttView::default()
                }, &cx);
                assert_eq!(view.rows[0].title, localized);
                assert_eq!(view.rows[0].action, selected.action);
                assert_eq!(view.rows[1].title, label);
                let other = view_in(OttView { page_id: "channels".into(),
                    rows: vec![selected], ..OttView::default() }, &cx);
                assert_eq!(other.rows[0].title, label);
            }
            for (key, title, localized) in [
                ("pageSize", "Classic list rows", msg::ott_classic_rows_in(&cx)),
                ("listPosition", "Classic list side", msg::ott_classic_side_in(&cx)),
                ("arrowFun", "Classic list arrows", msg::ott_classic_arrows_in(&cx)),
                ("seek13Duration", "Seek step for 1/3 (seconds)", msg::ott_classic_seek_in(&cx)),
            ] {
                let view = view_in(OttView { page_id: "settings".into(),
                    rows: vec![row(key, title, OttAction::EditSetting(key.into()))], ..OttView::default() }, &cx);
                assert_eq!(view.rows[0].title, localized);
            }
        }
    }

    #[test]
    fn first_run_settings_row_localizes_only_the_exact_application_action() {
        for language in i18n::SHIPPED {
            let cx = locale(language.tag());
            for page in ["providers", "profiles:m3u", "profiles:plex", "profiles:local"] {
                let view = view_in(OttView {
                    page_id: page.into(),
                    rows: vec![
                        row("action/settings", "Settings", OttAction::ShowSettings),
                        row("saved", "Settings", OttAction::SelectProfile("saved".into())),
                        row("movie", "Settings", OttAction::Play("movie".into())),
                        row("unrelated", "Settings", OttAction::ShowSettings),
                    ],
                    ..OttView::default()
                }, &cx);
                assert_eq!(view.rows[0].title, msg::settings_title_in(&cx));
                for row in &view.rows[1..] { assert_eq!(row.title, "Settings"); }
            }
            let other_page = view_in(OttView {
                page_id: "channels".into(),
                rows: vec![row("action/settings", "Settings", OttAction::ShowSettings)],
                ..OttView::default()
            }, &cx);
            assert_eq!(other_page.rows[0].title, "Settings");
        }
    }

    #[test]
    fn guide_order_stale_page_diagnostic_is_localized_at_display_boundary() {
        let source = "Reopen the guide list to change its order";
        assert_eq!(text_in(source, &locale("en")), source);
        assert_eq!(text_in(source, &locale("ru")), "Откройте список программ, чтобы изменить порядок");
    }

    #[test]
    fn usb_picker_translates_controls_without_translating_folder_names_or_paths() {
        for language in i18n::SHIPPED {
            let cx = locale(language.tag());
            let view = view_in(OttView {
                page_id: "usb-folders:7:Use this folder".into(),
                title: "USB folders".into(),
                description: "Use this folder".into(),
                status: "USB selection changed; close this picker".into(),
                rows: vec![
                    row("usb.use", "Use this folder", OttAction::EditSetting("local.usb.browse:7:0:use".into())),
                    row("usb.folder:/Use this folder", "Use this folder", OttAction::EditSetting("local.usb.browse:7:0:enter:0".into())),
                ], ..OttView::default()
            }, &cx);
            assert_eq!(view.title, msg::ott_usb_browser_title_in(&cx));
            assert_eq!(view.rows[0].title, msg::ott_usb_browser_use_in(&cx));
            assert_eq!(view.rows[1].title, "Use this folder");
            assert_eq!(view.description, "Use this folder");
            assert_eq!(view.status, msg::ott_usb_browser_changed_in(&cx));
        }
    }

    #[test]
    fn profile_dto_translates_only_application_owned_text() {
        for language in i18n::SHIPPED {
            let cx = locale(language.tag());
            let view = view_in(OttView { page_id: "profile-setup:xtream".into(),
                title: "Profile setup".into(),
                description: "Enter the connection details supplied by your provider".into(),
                rows: vec![
                    row("profile.setup.name", "Profile name", OttAction::EditSetting("profile.setup.name".into())),
                    row("saved", "Profile name", OttAction::SelectProfile("saved".into())),
                    row("movie", "Settings", OttAction::Play("movie".into())),
                    row("category", "All", OttAction::SelectCategory("All".into())),
                    row("remote.server:0", "Settings", OttAction::EditSetting("remote.server:0".into())),
                    row("offline.bind:0", "Profile name", OttAction::EditSetting("offline.bind:0".into())),
                ], ..OttView::default() }, &cx);
            assert_eq!(view.title, msg::ott_profile_setup_in(&cx));
            assert_eq!(view.description, msg::ott_profile_help_in(&cx));
            assert_eq!(view.rows[0].title, msg::ott_profile_name_in(&cx));
            assert_eq!(view.rows[0].detail, msg::ott_profile_not_set_in(&cx));
            assert_eq!(view.rows[1].title, "Profile name");
            assert_eq!(view.rows[1].detail, "Not set");
            assert_eq!(view.rows[2].title, "Settings");
            assert_eq!(view.rows[3].title, "All");
            assert_eq!(view.rows[4].title, "Settings");
            assert_eq!(view.rows[5].title, "Profile name");
        }
    }

    #[test]
    fn setup_description_uses_exact_qualified_page_namespace() {
        let cx = locale("ru");
        let raw = "Enter the connection details supplied by your provider";
        for page in ["profile-setup:xtream", "profile-setup:stalker"] {
            let view = view_in(OttView { page_id: page.into(), description: raw.into(),
                ..OttView::default() }, &cx);
            assert_eq!(view.description, msg::ott_profile_help_in(&cx));
        }
        let view = view_in(OttView { page_id: "profile-setup-other:xtream".into(),
            description: raw.into(), ..OttView::default() }, &cx);
        assert_eq!(view.description, raw);
    }

    #[test]
    fn profile_error_localization_keeps_internal_diagnostic_bytes() {
        let cx = locale("ru");
        let diagnostic = String::from("This provider already has 15 profiles");
        assert_eq!(text_in(&diagnostic, &cx), msg::ott_profile_provider_limit_in(&cx));
        assert_eq!(diagnostic, "This provider already has 15 profiles");
        assert_eq!(text_in("Unknown source failure", &cx), "Unknown source failure");
        assert_eq!(text_in("Source address or folder", &cx), msg::ott_profile_source_in(&cx));
    }

    #[test]
    fn empty_list_search_loading_and_errors_have_distinct_projected_copy() {
        let cx = locale("ru");
        let base = OttView { page_id: "channels:owner".into(),
            catalog_progress: Some(OttCatalogProgress { loaded: 0, total: Some(0), loading: false, complete: true }),
            search: Some(OttSearch { query: String::new(), scope: "Home".into() }),
            ..OttView::default() };
        assert_eq!(view_in(base.clone(), &cx).status, msg::ott_profile_empty_in(&cx));
        let mut filtered = base.clone();
        filtered.search.as_mut().unwrap().query = "no match".into();
        assert_eq!(view_in(filtered.clone(), &cx).status, msg::ott_profile_no_matches_in(&cx));
        filtered.catalog_progress.as_mut().unwrap().loading = true;
        filtered.catalog_progress.as_mut().unwrap().complete = false;
        assert!(view_in(filtered, &cx).status.is_empty());
        let mut failed = base;
        failed.status = "Unknown source failure".into();
        assert_eq!(view_in(failed, &cx).status, "Unknown source failure");
    }

    #[test]
    fn native_track_settings_use_existing_panel_labels_in_every_shipped_locale() {
        for language in i18n::SHIPPED {
            let cx = locale(language.tag());
            let view = view_in(OttView { page_id: "settings".into(), rows: vec![
                row("native.audio", "Audio tracks", OttAction::EditSetting("native.audio".into())),
                row("native.subtitles", "Subtitles and appearance", OttAction::EditSetting("native.subtitles".into())),
            ], ..OttView::default() }, &cx);
            assert_eq!(view.rows[0].title, msg::widgets_tracks_audio_in(&cx));
            assert_eq!(view.rows[1].title, msg::widgets_tracks_subtitles_in(&cx));
            for (item, key) in view.rows.iter().zip(["native.audio", "native.subtitles"]) {
                assert_eq!(item.id, key);
                assert_eq!(item.detail, msg::ott_profile_not_set_in(&cx));
                assert!(matches!(&item.action, OttAction::EditSetting(actual) if actual == key));
                assert!(item.enabled);
                assert!(!item.favorite);
            }
        }
    }

    #[test]
    fn native_track_translation_does_not_rewrite_user_text_or_unknown_actions() {
        let cx = locale("ru");
        let rows = vec![
            row("media", "Audio tracks", OttAction::Play("media".into())),
            row("profile", "Subtitles and appearance", OttAction::SelectProfile("profile".into())),
            row("category", "Audio tracks", OttAction::SelectCategory("category".into())),
            row("native.audio.other", "Audio tracks", OttAction::EditSetting("native.audio.other".into())),
            row("native.subtitles.other", "Subtitles and appearance", OttAction::EditSetting("native.subtitles.other".into())),
            row("unknown", "Audio tracks", OttAction::EditSetting("unknown".into())),
            row("offline.bind:0", "Subtitles and appearance", OttAction::EditSetting("offline.bind:0".into())),
            row("native.audio", "Unknown audio label", OttAction::EditSetting("native.audio".into())),
            row("native.subtitles", "Unknown subtitle label", OttAction::EditSetting("native.subtitles".into())),
            row("mismatched", "Subtitles and appearance", OttAction::EditSetting("native.audio".into())),
        ];
        let expected: Vec<_> = rows.iter().map(|item| item.title.clone()).collect();
        let mut view = OttView { page_id: "channels:owner".into(), title: "Audio tracks".into(),
            description: "Subtitles and appearance".into(), rows, ..OttView::default() };
        for item in &mut view.rows {
            item.detail = "Audio tracks from my source".into();
            item.description = "Subtitles and appearance".into();
            item.poster = Some(plx_data::metadata::MediaArt::Http {
                url: "https://example.invalid/art.jpg".into(),
            });
        }
        let view = view_in(view, &cx);
        assert_eq!(view.title, "Audio tracks");
        assert_eq!(view.description, "Subtitles and appearance");
        for (item, expected) in view.rows.iter().zip(expected) {
            assert_eq!(item.title, expected);
            assert_eq!(item.detail, "Audio tracks from my source");
            assert_eq!(item.description, "Subtitles and appearance");
            assert_eq!(item.poster, Some(plx_data::metadata::MediaArt::Http {
                url: "https://example.invalid/art.jpg".into(),
            }));
        }
        assert_eq!(text_in("Audio tracks", &cx), "Audio tracks");
        assert_eq!(text_in("Subtitles and appearance", &cx), "Subtitles and appearance");
    }
}

#[cfg(test)]
mod menu_tests {
    use super::*;
    use plx_screens::ott::OttRow;
    use plx_appkit::menu_visibility::{Change, Item};
    #[test]
    fn menu_text_is_localized_but_identically_named_provider_media_is_not() {
        for language in i18n::SHIPPED {
            let cx = LocaleContext::resolve(i18n::Preference::from_tag(language.tag()), None, None, None, None);
            let base = OttRow { current_programme: None, content_title: None, guide: None, guide_available: false, records_available: false, media: None, id: "menu:audio".into(), title: "Audio tracks".into(),
                detail: "Visible".into(), description: String::new(), poster: None, enabled: true, favorite: false,
                action: OttAction::MenuVisibility { revision: 0, context: 1, change: Change::Set { item: Item::Audio, visible: false } } };
            let mut media = base.clone(); media.id = "film".into(); media.action = OttAction::Play("film".into());
            let view = view_in(OttView { page_id: "menu-items".into(), title: "Menu items".into(),
                description: "Choose which items appear in menus".into(), rows: vec![base, media], ..Default::default() }, &cx);
            assert_eq!(view.title, msg::ott_menu_items_in(&cx));
            assert_eq!(view.description, msg::ott_menu_help_in(&cx));
            assert_eq!(view.rows[0].title, msg::ott_menu_audio_in(&cx));
            assert_eq!(view.rows[0].detail, msg::ott_menu_visible_in(&cx));
            assert_eq!(view.rows[1].title, "Audio tracks");
            assert_eq!(view.rows[1].detail, "Visible");
        }
    }
}


#[cfg(test)]
mod remote_discovery_tests {
    use super::*;
    use plx_screens::ott::OttRow;

    #[test]
    fn discovery_copy_is_localized_without_translating_server_identity() {
        for language in i18n::SHIPPED {
            let cx = LocaleContext::resolve(i18n::Preference::from_tag(language.tag()), None, None, None, None);
            let find = OttRow { current_programme: None, content_title: None, guide: None, media: None, id: "remote.find".into(),
                title: "Find command server".into(), detail: "Select a command server".into(),
                description: String::new(), poster: None, action: OttAction::EditSetting("remote.find".into()),
                enabled: true, favorite: false, guide_available: false, records_available: false };
            let mut candidate = find.clone();
            candidate.id = "remote.server:0".into();
            candidate.title = "https://home.invalid:443/control".into();
            candidate.detail = "home._ottplay-ctrl._tcp.test.".into();
            candidate.action = OttAction::EditSetting("remote.choice:91:0".into());
            let view = view_in(OttView { page_id: "settings".into(), rows: vec![find, candidate],
                status: "Discovery failed. Try again or enter an address.".into(), ..Default::default() }, &cx);
            assert_eq!(view.rows[1].title, "https://home.invalid:443/control");
            assert_eq!(view.rows[1].detail, "home._ottplay-ctrl._tcp.test.");
            assert_eq!(view.rows[1].action, OttAction::EditSetting("remote.choice:91:0".into()));
            if language.tag() != "en" {
                assert_ne!(view.rows[0].title, "Find command server", "{}", language.tag());
                assert_ne!(view.rows[0].detail, "Select a command server", "{}", language.tag());
                assert_ne!(view.status, "Discovery failed. Try again or enter an address.", "{}", language.tag());
            }
        }
    }
}

#[cfg(test)]
mod sleep_timer_tests {
    use super::*;
    use plx_screens::ott::OttRow;

    fn row(id: &str, title: &str, detail: &str, action: OttAction) -> OttRow {
        OttRow { current_programme: None, content_title: None, guide: None, guide_available: false, records_available: false, media: None,
            id: id.into(), title: title.into(), detail: detail.into(), description: String::new(),
            poster: None, enabled: true, favorite: false, action }
    }

    #[test]
    fn sleep_selector_and_summary_use_the_selected_locale_without_changing_actions() {
        for language in i18n::SHIPPED {
            let cx = LocaleContext::resolve(i18n::Preference::from_tag(language.tag()), None, None, None, None);
            let selection = OttAction::EditSetting("sleepTimeout.select:1:opaque".into());
            let view = view_in(OttView { page_id: "sleep-timer".into(), title: "Sleep timer".into(),
                description: "Standby after no input".into(), rows: vec![
                    row("sleep:0", "Off", "", OttAction::EditSetting("sleepTimeout.select:0:opaque".into())),
                    row("sleep:1", "30 min", "Selected", selection.clone()),
                    row("sleep:back", "Back to Settings", "", OttAction::ShowSettings),
                ], ..Default::default() }, &cx);
            assert_eq!(view.title, msg::ott_sleep_timer_in(&cx));
            assert_eq!(view.description, msg::ott_sleep_idle_in(&cx));
            assert_eq!(view.rows[0].title, msg::widgets_toggle_off_in(&cx));
            assert_eq!(view.rows[1].title, msg::ott_sleep_minutes_in(&cx, 30));
            assert_eq!(view.rows[1].detail, msg::ott_interface_selected_in(&cx));
            assert_eq!(view.rows[1].action, selection);
            assert_eq!(view.rows[2].title, msg::settings_back_in(&cx));
            let summary = view_in(OttView { page_id: "settings".into(), rows: vec![
                row("sleepTimeout", "Sleep timer", "180 min", OttAction::EditSetting("sleepTimeout".into())),
            ], ..Default::default() }, &cx);
            assert_eq!(summary.rows[0].title, msg::ott_sleep_timer_in(&cx));
            assert_eq!(summary.rows[0].detail, msg::ott_sleep_minutes_in(&cx, 180));
        }
    }

    #[test]
    fn sleep_translation_never_rewrites_user_titles_or_unknown_controls() {
        let cx = LocaleContext::resolve(i18n::Preference::from_tag("ru"), None, None, None, None);
        let rows = vec![
            row("film", "Sleep timer", "30 min", OttAction::Play("film".into())),
            row("profile", "Off", "180 min", OttAction::SelectProfile("profile".into())),
            row("sleep:1", "30 min", "Selected", OttAction::EditSetting("other".into())),
            row("unknown", "Sleep timer", "30 min", OttAction::EditSetting("sleepTimeout".into())),
        ];
        let before: Vec<_> = rows.iter().map(|row| (row.title.clone(), row.detail.clone(), row.action.clone())).collect();
        let view = view_in(OttView { page_id: "channels:fixture".into(), title: "Sleep timer".into(),
            description: "Standby after no input".into(), rows, ..Default::default() }, &cx);
        assert_eq!(view.title, "Sleep timer");
        assert_eq!(view.description, "Standby after no input");
        for (row, expected) in view.rows.iter().zip(before) {
            assert_eq!((row.title.clone(), row.detail.clone(), row.action.clone()), expected);
        }
    }
}
