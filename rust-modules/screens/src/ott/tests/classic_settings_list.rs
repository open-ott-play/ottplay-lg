use super::*;
use plx_machine::machine::UiMode;

fn settings(rows: &[(&str, &str, &str)], revision: u64) -> Arc<OttView> {
    let mut view = (*snapshot(revision, &[])).clone();
    view.page_id = "settings".into();
    view.title = "Settings".into();
    view.rows = rows.iter().map(|(id, title, detail)| {
        let mut row = snapshot(1, &[id]).rows[0].clone();
        row.title = (*title).into();
        row.detail = (*detail).into();
        row.action = OttAction::EditSetting((*id).into());
        row
    }).collect();
    view.selected_row = rows.first().map(|(id, _, _)| (*id).into());
    Arc::new(view)
}
fn visible_action(screen: &OttScreen, id: &str) -> Option<FormAction> {
    let index = screen.form.index_of(&id.to_owned())?;
    match screen.form.activate(index)? { Activation::Action(action) => Some(action), _ => None }
}

#[test]
fn root_settings_are_one_line_with_selection_help_and_original_actions() {
    let _serial = plx_base::testlock::serial();
    let measure = plx_ui::fixture::FixtureMeasure;
    let view = settings(&[("settings.open:interface", "Interface", ""),
        ("settings.open:profiles", "Profiles", "These actions save separately"),
        ("interface.mode", "Interface mode", "Classic")], 1);
    let mut screen = OttScreen::new(EntryId(1), InstanceId(1));
    screen.install(view.clone());
    let before = screen.keys["item:settings.open:profiles"];
    deliver(&mut screen, ScreenEvent::FocusMoved { from: None,
        to: FocusKey { entry: EntryId(1), elem: before }, by: plx_ui::screen::By::Dir });
    screen.refresh_presentation(UiMode::Classic, &measure);
    let table = &screen.form.table;
    assert!(table.is_classic_choices());
    assert!(!table.is_classic_catalogue());
    assert_eq!(table.sections.len(), 1);
    assert!(table.sections[0].header.is_empty());
    assert!(table.sections[0].rows.iter().all(|row| row.detail.is_empty()));
    assert_eq!(table.sections[0].rows[1].value, None, "help is not a setting value");
    assert_eq!(table.sections[0].rows[2].value.as_deref(), Some("Classic"));
    let frame = screen.table_frame();
    let first = table.row_frame(frame, 0).unwrap();
    assert_eq!(first.y, frame.y, "no duplicate SETTINGS header/padding");
    assert_eq!(table.row_frame(frame, 2).unwrap().h, first.h, "values cannot create tall rows");
    assert_eq!(screen.form.selected_id().map(String::as_str), Some("item:settings.open:profiles"));
    assert_eq!(screen.keys["item:settings.open:profiles"], before);
    assert!(matches!(visible_action(&screen, "item:settings.open:profiles"),
        Some(FormAction::Forward(OttAction::EditSetting(key))) if key == "settings.open:profiles"));
    assert!(visible_action(&screen, "nav:back").is_none(), "Close is not duplicated inside the list");
    let back = screen.keys["nav:back"];
    let header = screen.header(&measure).unwrap();
    assert_eq!(header.stops.iter().map(|(key, _)| *key).collect::<Vec<_>>(), vec![back]);
    assert_eq!(header.page.spec().left[0].label, msg::settings_login_close());
    let close = effects(|fx| screen.activate(back, false, fx));
    assert_eq!(close.iter().filter(|event| matches!(&event.fx,
        Fx::App(AppFx::Ott(OttOut::Action { action: OttAction::Back, .. })))).count(), 1);
    assert_eq!(screen.selected_snapshot_row().map(|row| row.id.as_str()), Some("settings.open:profiles"));
    let help = crate::ott::classic_settings_list::copy(screen.selected_snapshot_row(), "");
    assert_eq!(help, "Profiles\n\nThese actions save separately");
    screen.refresh_presentation(UiMode::Modern, &measure);
    assert!(!screen.form.table.is_classic_choices());
    assert_eq!(screen.form.table.sections[0].header, "Settings");
    assert_eq!(screen.form.table.sections[0].rows[1].detail, "These actions save separately");
    assert!(matches!(visible_action(&screen, "nav:back"), Some(FormAction::Forward(OttAction::Back))));
}

#[test]
fn grouped_draft_choices_keep_nonce_disabled_save_and_owner_back() {
    let _serial = plx_base::testlock::serial();
    let measure = plx_ui::fixture::FixtureMeasure;
    let mut screen = OttScreen::new(EntryId(1), InstanceId(1));
    screen.install(settings(&[("settings.open:lists", "Lists", "")], 0));
    screen.refresh_presentation(UiMode::Classic, &measure);
    let retired_close = screen.keys["nav:back"];
    let mut view = (*settings(&[("settings.draft:17:pick:pageSize", "Page size", "25"),
        ("settings.draft:17:save", "Save Settings", ""),
        ("settings.draft:17:back", "Cancel", "")], 1)).clone();
    view.rows[1].enabled = false;
    view.description = "Changes are saved only with Save Settings".into();
    screen.install(Arc::new(view));
    screen.refresh_presentation(UiMode::Classic, &measure);
    assert_eq!(screen.form.table.sections[0].rows[0].value.as_deref(), Some("25"));
    assert!(effects(|fx| screen.activate(retired_close, false, fx)).is_empty(),
        "a retired root Close must not bypass the group's current owner Back");
    assert!(visible_action(&screen, "item:settings.draft:17:save").is_none());
    assert!(visible_action(&screen, "nav:back").is_none(), "owner cancel remains the only Back");
    assert!(matches!(visible_action(&screen, "item:settings.draft:17:back"),
        Some(FormAction::Forward(OttAction::EditSetting(key))) if key == "settings.draft:17:back"));
    let help = crate::ott::classic_settings_list::copy(screen.snapshot.rows.first(), &screen.copy);
    assert!(help.ends_with("Changes are saved only with Save Settings"));
    screen.install(settings(&[("settings.draft:18:choose:pageSize:25", "25", "Selected"),
        ("settings.draft:18:back", "Back to Settings", "")], 2));
    screen.refresh_presentation(UiMode::Classic, &measure);
    assert!(visible_action(&screen, "item:settings.draft:17:back").is_none());
    assert!(matches!(visible_action(&screen, "item:settings.draft:18:choose:pageSize:25"),
        Some(FormAction::Forward(OttAction::EditSetting(key))) if key == "settings.draft:18:choose:pageSize:25"));
    assert_eq!(screen.form.table.sections[0].rows[0].value.as_deref(), Some("Selected"));
}

#[test]
fn actual_editor_and_provider_setup_keep_form_semantics() {
    let _serial = plx_base::testlock::serial();
    let measure = plx_ui::fixture::FixtureMeasure;
    let mut screen = OttScreen::new(EntryId(1), InstanceId(1));
    screen.install(settings(&[("settings.open:remote", "Remote control", "")], 1));
    screen.refresh_presentation(UiMode::Classic, &measure);
    editor(&mut screen, "draft source");
    screen.refresh_presentation(UiMode::Classic, &measure);
    assert!(!screen.form.table.is_classic_choices());
    assert!(!screen.form.table.is_classic_catalogue());
    assert!(matches!(visible_action(&screen, "edit:save"), Some(FormAction::Save)));
    assert!(matches!(visible_action(&screen, "edit:cancel"), Some(FormAction::Cancel)));
    effects(|fx| screen.cancel_editor(fx));
    assert!(screen.form.table.is_classic_choices());
    let mut view = (*settings(&[("profile.name", "Name", "Set")], 2)).clone();
    view.page_id = "profile-setup:m3u".into();
    screen.install(Arc::new(view));
    screen.refresh_presentation(UiMode::Classic, &measure);
    assert!(!screen.form.table.is_classic_choices());
    assert_eq!(screen.form.table.sections[0].rows[0].detail, "Set");
}

#[test]
fn classic_settings_group_footer_reuses_current_actions_without_duplicate_focus() {
    let _serial = plx_base::testlock::serial();
    let measure = plx_ui::fixture::FixtureMeasure;
    let mut screen = OttScreen::new(EntryId(1), InstanceId(1));
    screen.install(settings(&[("settings.draft:17:pick:pageSize", "Page size", "25"),
        ("settings.draft:17:save", "Save Settings", ""),
        ("settings.draft:17:back", "Cancel", "")], 1));
    screen.refresh_presentation(UiMode::Classic, &measure);
    let selected = screen.form.selected_key();
    let snapshot = screen.snapshot.clone();
    let back = screen.keys["settings-footer:settings.draft:17:back"];
    let save = screen.keys["settings-footer:settings.draft:17:save"];
    let header = screen.header(&measure).unwrap();
    assert_eq!(header.page.spec().left.iter().map(|item| item.label.as_str()).collect::<Vec<_>>(),
        vec!["Cancel", "Save Settings"]);
    assert_eq!(header.stops.iter().map(|(key, _)| *key).collect::<Vec<_>>(), vec![back, save]);
    assert!(header.classic_footer_fits(Rect::FULL.w));
    for (key, action) in [(back, "settings.draft:17:back"), (save, "settings.draft:17:save")] {
        assert!(screen.form.index_of_key(RowKey(key)).is_none(), "no duplicate body/footer focus identity");
        let output = effects(|fx| screen.activate(key, false, fx));
        let actions: Vec<_> = output.iter().filter_map(|event| match &event.fx {
            Fx::App(AppFx::Ott(OttOut::Action { action, .. })) => Some(action), _ => None,
        }).collect();
        assert_eq!(actions, vec![&OttAction::EditSetting(action.into())]);
    }
    assert_eq!(screen.form.selected_key(), selected);
    assert!(Arc::ptr_eq(&snapshot, &screen.snapshot));
    assert!(visible_action(&screen, "item:settings.draft:17:save").is_some());
    let mut translated = (*screen.snapshot).clone();
    translated.revision += 1;
    translated.rows[1].title = "Сохранить настройки".into();
    translated.rows[2].title = "Отмена".into();
    screen.install(Arc::new(translated));
    let relabeled = screen.header(&measure).unwrap();
    assert!(!std::rc::Rc::ptr_eq(&header, &relabeled), "a locale change must replace cached text");
    assert_eq!(relabeled.page.spec().left.iter().map(|item| item.label.as_str()).collect::<Vec<_>>(),
        vec!["Отмена", "Сохранить настройки"]);
    assert_eq!(relabeled.stops.iter().map(|(key, _)| *key).collect::<Vec<_>>(), vec![back, save]);
    assert_eq!(screen.form.selected_key(), selected);
}

#[test]
fn classic_settings_footer_tracks_picker_busy_and_replaced_nonce() {
    let _serial = plx_base::testlock::serial();
    let measure = plx_ui::fixture::FixtureMeasure;
    let mut screen = OttScreen::new(EntryId(1), InstanceId(1));
    screen.install(settings(&[("settings.draft:17:save", "Save Settings", ""),
        ("settings.draft:17:back", "Cancel", "")], 1));
    screen.refresh_presentation(UiMode::Classic, &measure);
    let old_back = screen.keys["settings-footer:settings.draft:17:back"];
    let old_save = screen.keys["settings-footer:settings.draft:17:save"];
    screen.install(settings(&[("settings.draft:18:choose:pageSize:25", "25", "Selected"),
        ("settings.draft:18:back", "Back to Settings", "")], 2));
    let header = screen.header(&measure).unwrap();
    assert_eq!(header.page.spec().left.iter().map(|item| item.label.as_str()).collect::<Vec<_>>(),
        vec!["Back to Settings"]);
    assert_eq!(header.stops.len(), 1, "a child picker has no Save action or hint");
    for key in [old_back, old_save] {
        assert!(effects(|fx| screen.activate(key, false, fx)).is_empty(), "retired draft footer");
    }
    let mut busy = (*settings(&[("settings.draft:19:save", "Save Settings", ""),
        ("settings.draft:19:back", "Back to Settings", "")], 3)).clone();
    busy.rows[0].enabled = false;
    screen.install(Arc::new(busy));
    assert_eq!(screen.header(&measure).unwrap().stops.len(), 1, "no enabled Save means no hint");
    let mut duplicate = (*settings(&[("settings.draft:20:save", "Save Settings", ""),
        ("settings.draft:20:back", "Cancel", "")], 4)).clone();
    let mut extra = duplicate.rows[0].clone();
    extra.id = "duplicate-save-row".into();
    duplicate.rows.push(extra);
    screen.install(Arc::new(duplicate));
    assert_eq!(screen.header(&measure).unwrap().stops.len(), 1, "ambiguous Save remains refused");
    for (index, case) in ["disabled", "mismatched-action", "duplicate", "noncanonical"].into_iter().enumerate() {
        screen.install(settings(&[("settings.draft:21:save", "Save Settings", ""),
            ("settings.draft:21:back", "Cancel", "")], 5 + index as u64 * 2));
        let back = screen.keys["settings-footer:settings.draft:21:back"];
        let mut invalid = (*screen.snapshot).clone();
        invalid.revision += 1;
        match case {
            "disabled" => invalid.rows[1].enabled = false,
            "mismatched-action" => invalid.rows[1].action = OttAction::Back,
            "duplicate" => {
                let mut extra = invalid.rows[1].clone();
                extra.id = "duplicate-back-row".into();
                invalid.rows.push(extra);
            }
            "noncanonical" => {
                invalid.rows[1].id = "settings.draft:021:back".into();
                invalid.rows[1].action = OttAction::EditSetting(invalid.rows[1].id.clone());
            }
            _ => unreachable!(),
        }
        screen.install(Arc::new(invalid));
        assert!(!screen.header(&measure).unwrap().stops.iter().any(|(key, _)| *key == back), "{case}");
        assert!(effects(|fx| screen.activate(back, false, fx)).is_empty(), "{case}");
    }
}

#[test]
fn classic_settings_footer_does_not_escape_modern_or_modal_owners() {
    let _serial = plx_base::testlock::serial();
    let measure = plx_ui::fixture::FixtureMeasure;
    let mut screen = OttScreen::new(EntryId(1), InstanceId(1));
    screen.install(settings(&[("settings.draft:17:save", "Save Settings", ""),
        ("settings.draft:17:back", "Cancel", "")], 1));
    screen.refresh_presentation(UiMode::Classic, &measure);
    let save = screen.keys["settings-footer:settings.draft:17:save"];
    screen.refresh_presentation(UiMode::Modern, &measure);
    assert!(!screen.header(&measure).unwrap().stops.iter().any(|(key, _)| *key == save));
    assert!(effects(|fx| screen.activate(save, false, fx)).is_empty());
    screen.refresh_presentation(UiMode::Classic, &measure);
    editor(&mut screen, "private draft");
    screen.refresh_presentation(UiMode::Classic, &measure);
    assert!(screen.header(&measure).is_none());
    assert!(effects(|fx| screen.activate(save, false, fx)).is_empty());
    effects(|fx| screen.cancel_editor(fx));
    effects(|fx| screen.open_browse(fx));
    assert!(screen.browse);
    assert!(effects(|fx| screen.activate(save, false, fx)).is_empty());
}
