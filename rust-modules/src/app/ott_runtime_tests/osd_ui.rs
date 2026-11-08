//! Choice-page admission and durable settlement. Workers cannot write in this fixture.
use super::*;
use plx_machine::machine::UiMode;

fn mounted(mode: UiMode) -> App {
    let mut app = app_fixture();
    let runtime = app.ott.as_mut().unwrap();
    runtime.loaded = true;
    runtime.entropy = Some([1; 16]);
    runtime.write_permit.revoke();
    runtime.catalog_done.store(false, Ordering::Release);
    runtime.metadata_done.store(false, Ordering::Release);
    runtime.resolver_busy = true;
    runtime.plex_refresh_pending = true;
    runtime.continuity.stop(runtime.generation);
    runtime.saved.settings.insert("interfaceMode".into(), json!(mode.index()));
    runtime.page = "settings";
    app.bridge.apply_ui_mode(mode);
    bridge::show_page(&mut app.pages, AppArg::Ott);
    settle_pages(&mut app);
    runtime_tick(&mut app);
    app
}

fn act(app: &mut App, action: OttAction) -> Result<(), &'static str> {
    let mut runtime = app.ott.take().unwrap();
    let result = handle_action(&mut runtime, app, action);
    app.ott = Some(runtime);
    result
}

fn ui(app: &mut App, action: OttAction) {
    let reply = app.pages.top_page().expect("mounted OTT instance");
    app.pages.emit(MachineId::Instance(reply), Fx::App(AppFx::Ott(
        OttOut::Action { reply, action },
    )));
    let tick = next_tick(app);
    bridge::frame(&mut app.pages, &mut app.bridge, tick, Vec::new());
    runtime_tick(app);
    settle_pages(app);
}

fn choice(app: &App, id: &str, value: &str) -> plx_screens::ott::OttRow {
    let prefix = format!("osd.select:{id}:{value}:");
    app.ott.as_ref().unwrap().view().rows.into_iter().find(|row|
        matches!(&row.action, OttAction::EditSetting(key) if key.starts_with(&prefix))
    ).expect("choice is produced by the real view")
}

#[test]
fn osd_choice_pages_keep_defaults_and_restore_settings_focus_in_both_modes() {
    let _serial = plx_base::testlock::serial();
    for mode in [UiMode::Classic, UiMode::Modern] {
        let mut app = mounted(mode);
        for (id, page, selected, count) in [
            ("osdOpacity", "osd-opacity", "7", 12),
            ("permanentTime", "permanent-clock", "0", 4),
        ] {
            let before = app.ott.as_ref().unwrap();
            let saved = serde_json::to_value(&before.saved).unwrap();
            let authority = (before.provider, before.selected.clone(), before.generation,
                before.playback_epoch, before.navigation_epoch);
            assert!(before.view().rows.iter().any(|row|
                row.action == OttAction::EditSetting("settings.open:interface".into()) && row.enabled));
            // Retain the dedicated legacy picker API and its choice-count contract.
            ui(&mut app, OttAction::EditSetting(id.into()));
            let view = app.ott.as_ref().unwrap().view();
            assert_eq!(view.page_id, page);
            assert_eq!(view.rows.len(), count);
            assert_eq!(view.selected_row, Some(choice(&app, id, selected).id));
            assert!(view.rows.iter().all(|row| row.enabled));
            ui(&mut app, OttAction::Back);
            let runtime = app.ott.as_ref().unwrap();
            assert_eq!(runtime.page, "settings");
            assert_eq!(runtime.view().selected_row.as_deref(), Some("settings.open:interface"));
            assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), saved);
            assert_eq!((runtime.provider, runtime.selected.clone(), runtime.generation,
                runtime.playback_epoch, runtime.navigation_epoch), authority);
            // The actual restored form focus opens the owning group on OK.
            let tick = next_tick(&mut app);
            bridge::frame(&mut app.pages, &mut app.bridge, tick,
                bridge::script_key(plx_machine::machine::Key::Ok, tick));
            runtime_tick(&mut app);
            settle_pages(&mut app);
            let runtime = app.ott.as_ref().unwrap();
            assert_eq!(runtime.page, "settings");
            assert!(runtime.view().rows.iter().any(|row| row.enabled && matches!(&row.action,
                OttAction::EditSetting(key) if key.ends_with(&format!(":pick:{id}")))));
            assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), saved);
            ui(&mut app, OttAction::ShowSettings);
        }
    }
}

#[test]
fn osd_retired_and_mismatched_choices_never_start_a_write() {
    let _serial = plx_base::testlock::serial();
    let mut app = mounted(UiMode::Modern);
    ui(&mut app, OttAction::EditSetting("osdOpacity".into()));
    let old = choice(&app, "osdOpacity", "3");
    ui(&mut app, OttAction::Back);
    ui(&mut app, OttAction::EditSetting("osdOpacity".into()));
    let fresh = choice(&app, "osdOpacity", "3");
    assert_ne!(old.id, fresh.id);
    assert_ne!(old.action, fresh.action);
    let OttAction::EditSetting(key) = fresh.action else { panic!("choice action"); };
    let token = key.strip_prefix("osd.select:osdOpacity:3:").unwrap();
    let before = serde_json::to_value(&app.ott.as_ref().unwrap().saved).unwrap();
    for action in [old.action,
        OttAction::EditSetting(format!("osd.select:permanentTime:1:{token}")),
        OttAction::EditSetting(format!("osd.select:osdOpacity:11:{token}")),
        OttAction::EditSetting(format!("osd.select:unknown:1:{token}")),
    ] {
        assert!(act(&mut app, action).is_err());
        let runtime = app.ott.as_ref().unwrap();
        assert!(!runtime.saving);
        assert_eq!(runtime.page, "osd-opacity");
        assert!(runtime.edit_session.is_none());
        assert!(runtime.setup.is_none());
        assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), before);
    }
    let stale = choice(&app, "osdOpacity", "3").action;
    app.ott.as_mut().unwrap().navigation_epoch += 1;
    assert!(act(&mut app, stale).is_err());
    let stale = choice(&app, "osdOpacity", "3").action;
    app.ott.as_mut().unwrap().saved.settings.insert(
        "settingRevision:osdOpacity".into(), json!("accepted-remote-change"));
    assert!(act(&mut app, stale).is_err());
    assert!(!app.ott.as_ref().unwrap().saving);
}

#[test]
fn osd_failed_save_keeps_old_value_and_choice_page() {
    let _serial = plx_base::testlock::serial();
    for (id, page, old, new) in [("osdOpacity", "osd-opacity", "7", "3"),
        ("permanentTime", "permanent-clock", "0", "2")]
    {
        let mut app = mounted(UiMode::Modern);
        act(&mut app, OttAction::EditSetting(id.into())).unwrap();
        let before = serde_json::to_value(&app.ott.as_ref().unwrap().saved).unwrap();
        let selected = choice(&app, id, new).action;
        act(&mut app, selected).unwrap();
        let runtime = app.ott.as_ref().unwrap();
        assert!(runtime.saving);
        assert!(matches!(runtime.save_navigation, SaveNavigation::Preserve));
        assert_eq!(runtime.osd_setting_value(id).as_deref(), Some(old));
        let job = runtime.rx.recv_timeout(Duration::from_secs(1)).unwrap();
        runtime.tx.send(job).unwrap_or_else(|_| panic!("fixture receiver disappeared"));
        runtime_tick(&mut app);
        let runtime = app.ott.as_ref().unwrap();
        assert!(!runtime.saving);
        assert_eq!(runtime.page, page);
        assert!(!runtime.status.is_empty());
        assert_eq!(runtime.osd_setting_value(id).as_deref(), Some(old));
        assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), before);
        assert!(choice(&app, id, new).enabled);
    }
}

#[test]
fn osd_accepted_settings_restore_without_navigation_or_source_changes() {
    let _serial = plx_base::testlock::serial();
    let mut app = mounted(UiMode::Classic);
    act(&mut app, OttAction::EditSetting("permanentTime".into())).unwrap();
    let runtime = app.ott.as_mut().unwrap();
    runtime.selected = Some("retained-browse-selection".into());
    let before = (runtime.provider, runtime.generation, runtime.playback_epoch,
        runtime.navigation_epoch);
    let saved = runtime.osd_setting_candidate("permanentTime", "2", runtime.edit_serial).unwrap();
    runtime.saving = true;
    runtime.save_navigation = SaveNavigation::Preserve;
    runtime.save_playback_epoch = runtime.playback_epoch;
    runtime.tx.send(Job::Saved(Ok(saved.clone()), None, false, json!({"status":"ok"})))
        .unwrap_or_else(|_| panic!("fixture receiver disappeared"));
    runtime_tick(&mut app);
    let runtime = app.ott.as_ref().unwrap();
    assert_eq!(runtime.osd_setting_value("permanentTime").as_deref(), Some("2"));
    assert_eq!(runtime.page, "permanent-clock");
    assert_eq!(runtime.selected.as_deref(), Some("retained-browse-selection"));
    assert_eq!((runtime.provider, runtime.generation, runtime.playback_epoch,
        runtime.navigation_epoch), before);
    assert_eq!(runtime.view().selected_row, Some(choice(&app, "permanentTime", "2").id));
    drop(app);
    let mut reopened = mounted(UiMode::Classic);
    let runtime = reopened.ott.as_mut().unwrap();
    runtime.loaded = false;
    runtime.tx.send(Job::Loaded(Ok((saved, [1; 16]))))
        .unwrap_or_else(|_| panic!("fixture receiver disappeared"));
    runtime_tick(&mut reopened);
    assert_eq!(reopened.ott.as_ref().unwrap().osd_setting_value("permanentTime").as_deref(), Some("2"));
}

#[test]
fn osd_unready_or_busy_settings_never_offer_enabled_choices() {
    let _serial = plx_base::testlock::serial();
    let mut app = mounted(UiMode::Modern);
    app.ott.as_mut().unwrap().loaded = false;
    assert!(act(&mut app, OttAction::EditSetting("osdOpacity".into())).is_err());
    app.ott.as_mut().unwrap().loaded = true;
    act(&mut app, OttAction::EditSetting("osdOpacity".into())).unwrap();
    for uncertain in [false, true] {
        let runtime = app.ott.as_mut().unwrap();
        runtime.saving = !uncertain;
        runtime.persistence_uncertain = uncertain;
        let rows = runtime.view().rows;
        assert!(rows.iter().filter(|row| matches!(row.action, OttAction::EditSetting(_)))
            .all(|row| !row.enabled));
        assert!(rows.iter().any(|row| row.action == OttAction::ShowSettings && row.enabled));
    }
}

#[test]
fn classic_choice_pages_use_the_shared_picker_and_restore_focus() {
    let _serial = plx_base::testlock::serial();
    for mode in [UiMode::Classic, UiMode::Modern] {
        let mut app = mounted(mode);
        for (id, page, selected, count, group) in [
            ("pageSize", "classic-page-size", "25", 22, "settings.open:lists"),
            ("listPosition", "classic-list-position", "0", 3, "settings.open:lists"),
            ("arrowFun", "classic-list-arrows", "0", 4, "settings.open:buttons"),
            ("seek13Duration", "numeric-seek-step", "15", 16, "settings.open:buttons"),
        ] {
            let before = app.ott.as_ref().unwrap();
            let saved = serde_json::to_value(&before.saved).unwrap();
            let authority = (before.provider, before.selected.clone(), before.generation,
                before.playback_epoch, before.navigation_epoch);
            assert!(before.view().rows.iter().any(|row|
                row.action == OttAction::EditSetting(group.into()) && row.enabled));
            // The scalar picker remains a supported action, outside the group draft.
            ui(&mut app, OttAction::EditSetting(id.into()));
            let view = app.ott.as_ref().unwrap().view();
            assert_eq!(view.page_id, page);
            assert_eq!(view.rows.len(), count);
            assert_eq!(view.selected_row, Some(choice(&app, id, selected).id));
            ui(&mut app, OttAction::Back);
            let runtime = app.ott.as_ref().unwrap();
            assert_eq!(runtime.page, "settings");
            assert_eq!(runtime.view().selected_row.as_deref(), Some(group));
            assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), saved);
            assert_eq!((runtime.provider, runtime.selected.clone(), runtime.generation,
                runtime.playback_epoch, runtime.navigation_epoch), authority);
        }
    }
}

#[test]
fn classic_stale_choices_cannot_change_a_new_page_or_begin_a_write() {
    let _serial = plx_base::testlock::serial();
    let mut app = mounted(UiMode::Classic);
    for (id, alternate) in [("pageSize", "10"), ("listPosition", "1"),
        ("arrowFun", "2"), ("seek13Duration", "30")]
    {
        act(&mut app, OttAction::EditSetting(id.into())).unwrap();
        let stale = choice(&app, id, alternate).action;
        act(&mut app, OttAction::Back).unwrap();
        act(&mut app, OttAction::EditSetting(id.into())).unwrap();
        let before = serde_json::to_value(&app.ott.as_ref().unwrap().saved).unwrap();
        assert!(act(&mut app, stale).is_err());
        let runtime = app.ott.as_ref().unwrap();
        assert!(!runtime.saving);
        assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), before);
        let stale = choice(&app, id, alternate).action;
        app.ott.as_mut().unwrap().saved.settings.insert(
            format!("settingRevision:{id}"), json!("accepted-remote-change"));
        assert!(act(&mut app, stale).is_err());
        act(&mut app, OttAction::Back).unwrap();
    }
}

#[test]
fn classic_legacy_paging_has_one_choice_and_opening_never_rewrites_it() {
    let _serial = plx_base::testlock::serial();
    let mut app = mounted(UiMode::Classic);
    let mut runtime = app.ott.take().unwrap();
    runtime.saved.settings.insert("arrowFun".into(), json!(3));
    // Model an accepted legacy load through the same publication seam as Loaded.
    commands::settings_after_saved(&mut runtime, &mut app).unwrap();
    app.ott = Some(runtime);
    let before = serde_json::to_value(&app.ott.as_ref().unwrap().saved).unwrap();
    act(&mut app, OttAction::EditSetting("arrowFun".into())).unwrap();
    let runtime = app.ott.as_ref().unwrap();
    let view = runtime.view();
    assert_eq!(view.rows.len(), 4, "three distinct modes and Back");
    assert_eq!(view.selected_row, Some(choice(&app, "arrowFun", "0").id));
    assert_eq!(runtime.osd_setting_value("arrowFun").as_deref(), Some("3"));
    assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), before);
    assert!(!runtime.saving);
    act(&mut app, OttAction::Back).unwrap();
    assert_eq!(serde_json::to_value(&app.ott.as_ref().unwrap().saved).unwrap(), before);
    act(&mut app, OttAction::EditSetting("arrowFun".into())).unwrap();
    let action = choice(&app, "arrowFun", "0").action;
    act(&mut app, action).unwrap();
    assert!(app.ott.as_ref().unwrap().saving, "an explicit Paging choice may canonicalize 3 to 0");
}

#[test]
fn classic_failed_saves_preserve_consumers_and_keep_the_picker_usable() {
    let _serial = plx_base::testlock::serial();
    for (id, old, new) in [("pageSize", "25", "10"), ("listPosition", "0", "1"),
        ("arrowFun", "0", "2"), ("seek13Duration", "15", "30")]
    {
        let mut app = mounted(UiMode::Classic);
        act(&mut app, OttAction::EditSetting(id.into())).unwrap();
        let before = app.ott.as_ref().unwrap().classic_preferences;
        let action = choice(&app, id, new).action;
        act(&mut app, action).unwrap();
        let runtime = app.ott.as_ref().unwrap();
        assert!(runtime.saving);
        assert!(matches!(runtime.save_navigation, SaveNavigation::Preserve));
        assert_eq!(runtime.classic_preferences, before);
        assert_eq!(runtime.osd_setting_value(id).as_deref(), Some(old));
        let job = runtime.rx.recv_timeout(Duration::from_secs(1)).unwrap();
        runtime.tx.send(job).unwrap_or_else(|_| panic!("fixture receiver disappeared"));
        runtime_tick(&mut app);
        let runtime = app.ott.as_ref().unwrap();
        assert!(!runtime.saving);
        assert_eq!(runtime.classic_preferences, before);
        assert_eq!(runtime.view().classic_preferences, before);
        assert_eq!(runtime.osd_setting_value(id).as_deref(), Some(old));
        assert!(choice(&app, id, new).enabled);
    }
}

#[test]
fn progress_picker_uses_scoped_choices_and_rejects_a_reopened_page_token() {
    let _serial = plx_base::testlock::serial();
    for mode in [UiMode::Classic, UiMode::Modern] {
        let mut app = mounted(mode);
        let before = serde_json::to_value(&app.ott.as_ref().unwrap().saved).unwrap();
        act(&mut app, OttAction::EditSetting("showProgress".into())).unwrap();
        let view = app.ott.as_ref().unwrap().view();
        assert_eq!(view.page_id, "classic-show-progress");
        assert_eq!(view.rows.len(), 3);
        assert_eq!(view.selected_row, Some(choice(&app, "showProgress", "1").id));
        let stale = choice(&app, "showProgress", "0").action;
        act(&mut app, OttAction::Back).unwrap();
        act(&mut app, OttAction::EditSetting("showProgress".into())).unwrap();
        assert!(act(&mut app, stale).is_err());
        let r = app.ott.as_ref().unwrap();
        assert_eq!(serde_json::to_value(&r.saved).unwrap(), before);
        assert!(r.classic_preferences().show_progress());
        assert!(r.classic_preferences().show_program());
        assert!(!r.saving);
        assert!(r.rx.try_recv().is_err());
    }
}

#[test]
fn progress_group_draft_provider_navigation_aba_retires_the_original_save() {
    let _serial = plx_base::testlock::serial();
    let mut app = mounted(UiMode::Classic);
    let group_action = |app: &App, suffix: &str| app.ott.as_ref().unwrap().view().rows
        .iter().find_map(|row| match &row.action {
            OttAction::EditSetting(key) if key.starts_with("settings.draft:") && key.ends_with(suffix) => Some(row.action.clone()),
            _ => None,
        }).expect("actual grouped setting row");
    ui(&mut app, OttAction::SelectProvider("m3u".into()));
    ui(&mut app, OttAction::ShowSettings);
    ui(&mut app, OttAction::EditSetting("settings.open:channels".into()));
    let pick = group_action(&app, ":pick:showProgress");
    ui(&mut app, pick);
    let off = group_action(&app, ":choose:showProgress:0");
    ui(&mut app, off);
    let old_save = group_action(&app, ":save");
    let r = app.ott.as_ref().unwrap();
    let before = serde_json::to_value(&r.saved).unwrap();
    let accepted = r.classic_display.clone();
    let navigation = r.navigation_epoch;
    ui(&mut app, OttAction::SelectProvider("stalker".into()));
    ui(&mut app, OttAction::SelectProvider("m3u".into()));
    ui(&mut app, OttAction::ShowSettings);
    ui(&mut app, OttAction::EditSetting("settings.open:channels".into()));
    assert_ne!(group_action(&app, ":save"), old_save);
    assert!(act(&mut app, old_save).is_err());
    let r = app.ott.as_ref().unwrap();
    assert_eq!(r.provider, ProviderKind::M3u);
    assert!(r.navigation_epoch > navigation);
    assert_eq!(serde_json::to_value(&r.saved).unwrap(), before);
    assert_eq!(r.classic_display, accepted);
    assert!(r.classic_preferences().show_progress());
    assert!(!r.saving);
    assert!(r.rx.try_recv().is_err());
}
