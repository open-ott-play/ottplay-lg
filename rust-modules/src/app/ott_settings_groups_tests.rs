//! Real group projection/actions and merge; the injected writer models admission,
//! not disk durability or the outer App/parental dispatch. No worker is started.
use super::*;

fn runtime() -> Runtime {
    let mut runtime = Runtime::unloaded(crate::app::ott_companion::Supervisor::inert_for_test());
    runtime.remote = Remote::disabled_for_test("group_fixture", "fixture");
    runtime.loaded = true;
    runtime.state_load = StateLoad::Idle;
    runtime.entropy = Some([1; 16]);
    runtime.page = "settings";
    runtime.reply = Some(InstanceId(7));
    runtime.navigation_epoch = 11;
    runtime.generation = 3;
    runtime.status.clear();
    runtime.write_permit.revoke();
    for (key, value) in [("uiFontScale", 100), ("volumeStep", 5), ("pageSize", 25), ("arrowFun", 3)] {
        runtime.saved.settings.insert(key.into(), json!(value));
        runtime.saved.settings.insert(format!("settingRevision:{key}"), json!("initial"));
    }
    runtime
}

fn saved(runtime: &Runtime) -> Value { serde_json::to_value(&runtime.saved).unwrap() }
fn projected(runtime: &Runtime) -> OttView { view(runtime, runtime.raw_view()) }
fn apply(runtime: &mut Runtime, selected: OttAction) -> Result<(), &'static str> {
    action(runtime, &selected).expect("group owns fixture action")
}
fn open_group(runtime: &mut Runtime, group: &str) {
    let key = format!("settings.open:{group}");
    let selected = projected(runtime).rows.into_iter().find(|row| row.id == key)
        .expect("visible group").action;
    apply(runtime, selected).unwrap();
}
fn command(runtime: &Runtime, wanted: &str) -> OttAction {
    projected(runtime).rows.into_iter().find(|row| matches!(&row.action,
        OttAction::EditSetting(key) if key.strip_prefix(PREFIX)
            .and_then(|tail| tail.split_once(':')).is_some_and(|(_, command)| command == wanted)))
        .expect("visible group command").action
}
fn perform(runtime: &mut Runtime, wanted: &str) {
    let selected = command(runtime, wanted);
    apply(runtime, selected).unwrap();
}
fn stage(runtime: &mut Runtime, key: &str, value: u64) {
    perform(runtime, &format!("pick:{key}"));
    perform(runtime, &format!("choose:{key}:{value}"));
}

#[test]
fn native_subtitle_group_stages_typed_preferences_and_saves_them_together() {
    let _serial = plx_base::testlock::serial();
    let mut runtime = runtime();
    let before = saved(&runtime);
    open_group(&mut runtime, "subtitles");
    assert!(projected(&runtime).rows.iter().any(|row| row.detail == "Off (manual selection)"));
    stage(&mut runtime, "nativeSubtitleMode", 1);
    stage(&mut runtime, "nativeSubtitleLanguage", 2);
    assert_eq!(saved(&runtime), before, "draft cannot change the active policy");
    assert!(projected(&runtime).rows.iter().any(|row| row.detail == "Automatic"));
    assert!(projected(&runtime).rows.iter().any(|row| row.detail == "Russian"));
    let candidate = merge(&runtime, address(&runtime).unwrap()).unwrap().unwrap();
    let policy = subtitle_settings::selection::read(&candidate).unwrap();
    assert_eq!(policy.mode, plx_media::route::HlsSubtitleMode::Auto);
    assert_eq!(policy.language, "ru");
    assert_eq!(candidate.settings[subtitle_settings::REVISION], json!(1));
    assert_eq!(saved(&runtime), before);
}

#[test]
fn root_groups_preserve_separate_actions_and_opening_never_saves() {
    let _serial = plx_base::testlock::serial();
    let mut runtime = runtime();
    let before = saved(&runtime);
    let root = projected(&runtime);
    for id in ["settings.open:interface", "settings.open:colors", "settings.open:lists", "settings.open:infobar",
        "settings.open:buttons", "settings.open:menu", "settings.open:profiles",
        "settings.open:parental", "settings.open:remote", "interfaceMode", "native.audio"] {
        assert!(root.rows.iter().any(|row| row.id == id), "missing {id}");
    }
    assert!(!root.rows.iter().any(|row| row.id == "volumeStep"));
    open_group(&mut runtime, "interface");
    assert!(matches!(command(&runtime, "save"), OttAction::EditSetting(_)));
    assert!(projected(&runtime).rows.iter().any(|row| row.detail == "100%"));
    assert_eq!(saved(&runtime), before);
    assert!(!runtime.saving);
    assert!(runtime.rx.try_recv().is_err());
}

#[test]
fn actual_choices_stage_without_publishing_then_merge_all_fields_once() {
    let _serial = plx_base::testlock::serial();
    let mut runtime = runtime();
    let before = saved(&runtime);
    let epochs = (runtime.generation, runtime.navigation_epoch, runtime.playback_epoch);
    let display = runtime.classic_preferences();
    open_group(&mut runtime, "interface");
    stage(&mut runtime, "uiFontScale", 150);
    stage(&mut runtime, "volumeStep", 8);
    assert_eq!(saved(&runtime), before);
    assert_eq!(runtime.font_scale_percent(), 100);
    assert_eq!(runtime.volume_step(), 5);
    assert_eq!(runtime.classic_preferences(), display);
    assert_eq!((runtime.generation, runtime.navigation_epoch, runtime.playback_epoch), epochs);
    let mut writes = 0;
    save_with(&mut runtime, |runtime, candidate| {
        writes += 1;
        assert_eq!(candidate.settings["uiFontScale"], 150);
        assert_eq!(candidate.settings["volumeStep"], 8);
        assert_ne!(candidate.settings["settingRevision:uiFontScale"], "initial");
        assert_ne!(candidate.settings["settingRevision:volumeStep"], "initial");
        runtime.saving = true;
        Ok(())
    }).unwrap();
    assert_eq!(writes, 1);
    assert!(runtime.settings_groups.draft.pending().is_some());
    assert_eq!(saved(&runtime), before);
    assert!(runtime.rx.try_recv().is_err());
    assert!(save_with(&mut runtime, |_, _| panic!("duplicate writer")).is_err());
}

#[test]
fn picker_back_preserves_other_staging_but_group_cancel_discards_everything() {
    let _serial = plx_base::testlock::serial();
    let mut runtime = runtime();
    let before = saved(&runtime);
    open_group(&mut runtime, "interface");
    stage(&mut runtime, "volumeStep", 7);
    perform(&mut runtime, "pick:uiFontScale");
    let mapped = back(&runtime, OttAction::Back);
    assert!(matches!(mapped, OttAction::ShowSettings));
    apply(&mut runtime, mapped).unwrap();
    assert!(runtime.settings_groups.picker.is_none());
    assert_eq!(displayed(&runtime, "volumeStep"), Some(json!(7)));
    perform(&mut runtime, "back");
    assert!(runtime.settings_groups.section.is_none());
    assert_eq!(saved(&runtime), before);
    open_group(&mut runtime, "interface");
    assert_eq!(displayed(&runtime, "volumeStep"), Some(json!(5)));
}

#[test]
fn stale_choice_and_invalid_value_cannot_change_or_replace_current_draft() {
    let _serial = plx_base::testlock::serial();
    let mut runtime = runtime();
    open_group(&mut runtime, "interface");
    perform(&mut runtime, "pick:volumeStep");
    let stale = command(&runtime, "choose:volumeStep:8");
    let nonce = runtime.settings_groups.nonce;
    let invalid = OttAction::EditSetting(format!("{PREFIX}{nonce}:choose:volumeStep:11"));
    assert!(apply(&mut runtime, invalid).is_err());
    assert_eq!(runtime.settings_groups.nonce, nonce);
    perform(&mut runtime, "choose:volumeStep:7");
    assert!(apply(&mut runtime, stale.clone()).is_err());
    assert_eq!(displayed(&runtime, "volumeStep"), Some(json!(7)));
    perform(&mut runtime, "back");
    open_group(&mut runtime, "interface");
    perform(&mut runtime, "pick:volumeStep");
    assert!(apply(&mut runtime, stale).is_err());
    assert_eq!(displayed(&runtime, "volumeStep"), Some(json!(5)));
}

#[test]
fn dirty_merge_preserves_newer_unrelated_settings_and_inactive_profile() {
    let _serial = plx_base::testlock::serial();
    let mut runtime = runtime();
    runtime.saved.profiles.profiles.push(ProviderProfile { id: "other".into(), name: "Before".into(),
        kind: ProviderKind::Plex, source: "fixture-server".into(), source_alias: String::new(), enabled: true });
    open_group(&mut runtime, "interface");
    stage(&mut runtime, "volumeStep", 8);
    runtime.saved.profiles.profiles[0].name = "After".into();
    runtime.saved.settings.insert("nativeRemoteServerAddress".into(), json!("https://new.example.invalid"));
    runtime.saved.settings.insert("pageSize".into(), json!(12));
    runtime.saved.settings.insert("settingRevision:pageSize".into(), json!("remote"));
    runtime.saved.category = "new category".into();
    let current = saved(&runtime);
    let candidate = merge(&runtime, address(&runtime).unwrap()).unwrap().unwrap();
    let mut expected = current;
    expected["settings"]["volumeStep"] = json!(8);
    expected["settings"]["settingRevision:volumeStep"] = candidate.settings["settingRevision:volumeStep"].clone();
    assert_eq!(serde_json::to_value(candidate).unwrap(), expected);
}

#[test]
fn changed_dirty_field_and_revision_only_aba_refuse_before_writer() {
    let _serial = plx_base::testlock::serial();
    for revision_only in [false, true] {
        let mut runtime = runtime();
        open_group(&mut runtime, "interface");
        stage(&mut runtime, "volumeStep", 8);
        if revision_only { runtime.saved.settings.insert("settingRevision:volumeStep".into(), json!("after-aba")); }
        else { runtime.saved.settings.insert("volumeStep".into(), json!(6)); }
        assert_eq!(save_with(&mut runtime, |_, _| panic!("conflict reached writer")), Err(STALE));
        assert_eq!(displayed(&runtime, "volumeStep"), Some(json!(8)));
        assert!(runtime.settings_groups.draft.pending().is_none());
    }
}

#[test]
fn owner_navigation_generation_and_source_changes_fence_save() {
    let _serial = plx_base::testlock::serial();
    for changed in 0..4 {
        let mut runtime = runtime();
        open_group(&mut runtime, "interface");
        stage(&mut runtime, "volumeStep", 8);
        match changed {
            0 => runtime.reply = Some(InstanceId(8)),
            1 => runtime.navigation_epoch += 1,
            2 => runtime.generation += 1,
            _ => { runtime.saved.settings.insert("selectedProvider".into(), json!("local")); }
        }
        assert_eq!(save_with(&mut runtime, |_, _| panic!("stale owner reached writer")), Err(STALE));
        assert!(runtime.settings_groups.draft.pending().is_none());
    }
}

#[test]
fn menu_toggles_share_one_compound_merge_and_cancel_preserves_saved_document() {
    use plx_appkit::menu_visibility::{Change, Item};
    let _serial = plx_base::testlock::serial();
    let mut runtime = runtime();
    let before = saved(&runtime);
    let search = Item::find("search").unwrap();
    let profiles = Item::find("profiles").unwrap();
    let first = menu_visibility::candidate(&runtime.saved, Change::Set { item: search, visible: false }).unwrap();
    let expected = menu_visibility::candidate(&first, Change::Set { item: profiles, visible: false }).unwrap();
    open_group(&mut runtime, "menu");
    perform(&mut runtime, "menu:search");
    perform(&mut runtime, "menu:profiles");
    let candidate = merge(&runtime, address(&runtime).unwrap()).unwrap().unwrap();
    assert_eq!(serde_json::to_value(candidate).unwrap(), serde_json::to_value(expected).unwrap());
    assert_eq!(saved(&runtime), before);
    perform(&mut runtime, "back");
    assert_eq!(saved(&runtime), before);
}

#[test]
fn compound_menu_remote_change_is_not_overwritten() {
    use plx_appkit::menu_visibility::{Change, Item};
    let _serial = plx_base::testlock::serial();
    let mut runtime = runtime();
    open_group(&mut runtime, "menu");
    perform(&mut runtime, "menu:search");
    runtime.saved = menu_visibility::candidate(&runtime.saved,
        Change::Set { item: Item::find("profiles").unwrap(), visible: false }).unwrap();
    let before = saved(&runtime);
    assert_eq!(save_with(&mut runtime, |_, _| panic!("compound conflict reached writer")), Err(STALE));
    assert_eq!(saved(&runtime), before);
}

#[test]
fn admission_failure_and_known_rejection_retain_draft_for_retry() {
    let _serial = plx_base::testlock::serial();
    let mut runtime = runtime();
    open_group(&mut runtime, "lists");
    stage(&mut runtime, "pageSize", 20);
    let save = command(&runtime, "save");
    runtime.loaded = false;
    assert!(apply(&mut runtime, save).is_err(), "real Save action must refuse unready storage");
    assert!(!runtime.saving);
    assert!(runtime.rx.try_recv().is_err());
    runtime.loaded = true;
    assert_eq!(save_with(&mut runtime, |_, _| Err("writer unavailable")), Err("writer unavailable"));
    assert!(runtime.settings_groups.draft.pending().is_none());
    assert_eq!(displayed(&runtime, "pageSize"), Some(json!(20)));
    save_with(&mut runtime, |runtime, _| { runtime.saving = true; Ok(()) }).unwrap();
    runtime.saving = false; // the existing Job handler releases its writer slot first
    runtime.status = "known rejection".into();
    finish(&mut runtime, Some(InstanceId(7)), false, false);
    assert!(runtime.settings_groups.draft.pending().is_none());
    assert!(runtime.settings_groups.section.is_some());
    assert_eq!(displayed(&runtime, "pageSize"), Some(json!(20)));
    assert_eq!(runtime.status, "known rejection");
    save_with(&mut runtime, |runtime, _| { runtime.saving = true; Ok(()) }).unwrap();
    assert!(runtime.settings_groups.draft.pending().is_some());
}

#[test]
fn durable_completion_closes_draft_but_apply_failure_is_not_reported_as_success() {
    let _serial = plx_base::testlock::serial();
    for applied in [false, true] {
        let mut runtime = runtime();
        open_group(&mut runtime, "lists");
        stage(&mut runtime, "pageSize", 20);
        let mut admitted = None;
        save_with(&mut runtime, |runtime, candidate| { runtime.saving = true; admitted = Some(candidate); Ok(()) }).unwrap();
        runtime.saved = admitted.unwrap(); // model already accepted durable Saved publication
        runtime.saving = false;
        runtime.status = "Saved, but could not apply settings".into();
        finish(&mut runtime, Some(InstanceId(7)), true, applied);
        assert!(runtime.settings_groups.section.is_none());
        assert!(runtime.settings_groups.draft.pending().is_none());
        assert_eq!(runtime.saved.settings["pageSize"], 20);
        assert_eq!(runtime.status, if applied { "Settings saved" } else { "Saved, but could not apply settings" });
    }
}

#[test]
fn cancelled_pending_completion_cannot_steal_new_wrapper_or_reopen_old_group() {
    let _serial = plx_base::testlock::serial();
    let mut runtime = runtime();
    open_group(&mut runtime, "lists");
    stage(&mut runtime, "pageSize", 20);
    save_with(&mut runtime, |runtime, _| { runtime.saving = true; Ok(()) }).unwrap();
    perform(&mut runtime, "back");
    assert!(runtime.settings_groups.draft.pending().is_some());
    open_group(&mut runtime, "remote");
    runtime.saving = false;
    finish(&mut runtime, Some(InstanceId(7)), true, true);
    assert!(matches!(runtime.settings_groups.section, Some(Section::Remote)));
    assert!(runtime.settings_groups.draft.pending().is_none());
    assert_ne!(runtime.status, "Settings saved");
}

#[test]
fn rejected_old_owner_and_uncertainty_retire_the_group_without_success() {
    let _serial = plx_base::testlock::serial();
    for unknown in [false, true] {
        let mut runtime = runtime();
        open_group(&mut runtime, "lists");
        stage(&mut runtime, "pageSize", 20);
        save_with(&mut runtime, |runtime, _| { runtime.saving = true; Ok(()) }).unwrap();
        runtime.saving = false;
        runtime.status = "reconciliation required".into();
        if unknown { uncertain(&mut runtime); }
        else { runtime.reply = Some(InstanceId(8)); finish(&mut runtime, Some(InstanceId(8)), false, false); }
        assert!(runtime.settings_groups.section.is_none());
        assert!(runtime.settings_groups.draft.pending().is_none());
        assert_eq!(runtime.status, "reconciliation required");
        assert_eq!(runtime.saved.settings["pageSize"], 25);
    }
}

#[test]
fn only_parental_wrapper_and_current_back_use_navigation_only_gate() {
    let _serial = plx_base::testlock::serial();
    let mut runtime = runtime();
    assert!(navigation_only(&runtime, &OttAction::EditSetting("settings.open:parental".into())));
    assert!(!navigation_only(&runtime, &OttAction::EditSetting("settings.open:interface".into())));
    open_group(&mut runtime, "interface");
    let back = command(&runtime, "back");
    assert!(navigation_only(&runtime, &back));
    assert!(!navigation_only(&runtime, &command(&runtime, "save")));
    assert!(!navigation_only(&runtime, &command(&runtime, "pick:volumeStep")));
    let malformed = OttAction::EditSetting(format!("{PREFIX}{}:back:extra", runtime.settings_groups.nonce));
    assert!(!navigation_only(&runtime, &malformed));
    perform(&mut runtime, "pick:volumeStep");
    assert!(!navigation_only(&runtime, &back));
    runtime.page = "providers";
    assert!(!navigation_only(&runtime, &OttAction::EditSetting("settings.open:parental".into())));
}

#[test]
fn refresh_preserves_draft_and_external_navigation_discards_it() {
    let _serial = plx_base::testlock::serial();
    let mut runtime = runtime();
    open_group(&mut runtime, "buttons");
    assert_eq!(displayed(&runtime, "arrowFun"), Some(json!(3)));
    perform(&mut runtime, "pick:arrowFun");
    assert!(projected(&runtime).rows.iter().any(|row| row.detail == "Selected"));
    perform(&mut runtime, "back");
    assert_eq!(runtime.saved.settings["arrowFun"], 3, "opening legacy alias does not rewrite it");
    stage(&mut runtime, "seek13Duration", 20);
    external_action(&mut runtime, &OttAction::Refresh);
    assert!(runtime.settings_groups.draft.address().is_some());
    external_action(&mut runtime, &OttAction::ShowProviders);
    assert!(runtime.settings_groups.draft.address().is_none());
    assert!(runtime.saved.settings.get("seek13Duration").is_none());
}

#[test]
fn separate_interface_mode_action_retires_remembered_group_focus() {
    let _serial = plx_base::testlock::serial();
    let mut runtime = runtime();
    // Only make the existing public-preference row selectable for projection;
    // this test never dispatches its writer or a storage operation.
    runtime.write_permit = private::WritePermit::new();
    runtime.public_preferences.accept(None);
    open_group(&mut runtime, "lists");
    perform(&mut runtime, "back");
    assert_eq!(runtime.settings_groups.focus.as_deref(), Some("settings.open:lists"));
    assert_eq!(projected(&runtime).selected_row.as_deref(), Some("settings.open:lists"));
    external_action(&mut runtime, &OttAction::EditSetting("interfaceMode".into()));
    runtime.settings_focus = Some("interfaceMode");
    assert!(runtime.settings_groups.focus.is_none());
    let raw = runtime.raw_view();
    assert_eq!(raw.selected_row.as_deref(), Some("interfaceMode"));
    assert_eq!(view(&runtime, raw).selected_row.as_deref(), Some("interfaceMode"));
}

#[test]
fn reconcile_retires_changed_owner_or_generation_without_reverting_saved_or_pending_write() {
    let _serial = plx_base::testlock::serial();
    for changed_owner in [false, true] {
        for accepted in [false, true] {
            let mut runtime = runtime();
            let before = saved(&runtime);
            open_group(&mut runtime, "lists");
            stage(&mut runtime, "pageSize", 20);
            let original = address(&runtime).unwrap();
            reconcile(&mut runtime, Some(InstanceId(7)));
            assert_eq!(runtime.settings_groups.draft.address(), Some(original));
            if accepted {
                save_with(&mut runtime, |runtime, _| { runtime.saving = true; Ok(()) }).unwrap();
            }
            let owner = if changed_owner { Some(InstanceId(8)) } else {
                runtime.generation += 1;
                Some(InstanceId(7))
            };
            reconcile(&mut runtime, owner);
            assert!(runtime.settings_groups.section.is_none());
            assert!(runtime.settings_groups.draft.address().is_none());
            assert_eq!(runtime.settings_groups.draft.pending(), accepted.then_some(original));
            assert_eq!(runtime.saving, accepted);
            assert_eq!(saved(&runtime), before);
            assert_eq!(runtime.status, STALE);
            if accepted {
                runtime.saving = false;
                finish(&mut runtime, owner, false, false);
                assert!(runtime.settings_groups.draft.pending().is_none());
                assert!(runtime.settings_groups.section.is_none());
                assert_eq!(saved(&runtime), before);
            }
        }
    }
}

#[test]
fn channel_group_uses_the_existing_provider_scoped_storage_owner() {
    let _serial = plx_base::testlock::serial();
    let mut runtime = runtime();
    runtime.provider = ProviderKind::M3u;
    let [key, revision] = runtime.osd_setting_storage_keys("showNumber").unwrap();
    assert_ne!(key, "showNumber");
    assert_ne!(revision, "settingRevision:showNumber");
    // A legacy application-wide value must not masquerade as this provider's value.
    runtime.saved.settings.insert("showNumber".into(), json!(0));
    runtime.saved.settings.insert("settingRevision:showNumber".into(), json!("legacy"));
    open_group(&mut runtime, "channels");
    assert_eq!(displayed(&runtime, "showNumber"), Some(json!(1)));
    stage(&mut runtime, "showNumber", 0);
    let candidate = merge(&runtime, address(&runtime).unwrap()).unwrap().unwrap();
    assert_eq!(candidate.settings[&key], 0);
    assert!(candidate.settings.contains_key(&revision));
    assert_eq!(candidate.settings["showNumber"], 0);
    assert_eq!(candidate.settings["settingRevision:showNumber"], "legacy");
    assert!(runtime.saved.settings.get(&key).is_none());
    runtime.saved = candidate;
    cancel(&mut runtime);
    runtime.provider = ProviderKind::Plex;
    open_group(&mut runtime, "channels");
    assert_eq!(displayed(&runtime, "showNumber"), Some(json!(1)));
    assert_eq!(runtime.saved.settings[&key], 0);
}

#[test]
fn channel_draft_merges_multiple_flags_and_fences_provider_revision_aba() {
    let _serial = plx_base::testlock::serial();
    let mut runtime = runtime();
    open_group(&mut runtime, "channels");
    stage(&mut runtime, "showName", 0);
    stage(&mut runtime, "showDescription", 0);
    let candidate = merge(&runtime, address(&runtime).unwrap()).unwrap().unwrap();
    for id in ["showName", "showDescription"] {
        let [key, revision] = runtime.osd_setting_storage_keys(id).unwrap();
        assert_eq!(candidate.settings[&key], 0);
        assert!(candidate.settings.contains_key(&revision));
        assert!(candidate.settings.get(id).is_none());
    }
    let [_, revision] = runtime.osd_setting_storage_keys("showName").unwrap();
    runtime.saved.settings.insert(revision, json!("remote-aba"));
    assert_eq!(save_with(&mut runtime, |_, _| panic!("provider conflict reached writer")), Err(STALE));
    runtime.provider = ProviderKind::Plex;
    assert_eq!(save_with(&mut runtime, |_, _| panic!("provider changed")), Err(STALE));
}

#[test]
fn channel_flags_are_shared_by_slots_of_the_same_provider() {
    let _serial = plx_base::testlock::serial();
    let mut runtime = runtime();
    runtime.provider = ProviderKind::M3u;
    for id in ["first", "second"] {
        runtime.saved.profiles.profiles.push(ProviderProfile {
            id: id.into(), name: id.into(), kind: ProviderKind::M3u,
            source: format!("http://127.0.0.1/{id}.m3u"),
            source_alias: String::new(), enabled: true,
        });
    }
    runtime.saved.profiles.active_profile = Some("first".into());
    let first_keys = runtime.osd_setting_storage_keys("showProgram").unwrap();
    open_group(&mut runtime, "channels");
    stage(&mut runtime, "showProgram", 0);
    runtime.saved = merge(&runtime, address(&runtime).unwrap()).unwrap().unwrap();
    cancel(&mut runtime);
    runtime.saved.profiles.active_profile = Some("second".into());
    assert_eq!(runtime.osd_setting_storage_keys("showProgram").unwrap(), first_keys);
    open_group(&mut runtime, "channels");
    assert_eq!(displayed(&runtime, "showProgram"), Some(json!(0)));
}

#[test]
fn progress_and_programme_drafts_are_independent_and_cancel_without_publication() {
    let _serial = plx_base::testlock::serial();
    for (edited, untouched) in [("showProgress", "showProgram"), ("showProgram", "showProgress")] {
        let mut runtime = runtime();
        runtime.provider = ProviderKind::M3u;
        let before = saved(&runtime);
        let accepted = runtime.classic_display.clone();
        open_group(&mut runtime, "channels");
        assert!(projected(&runtime).rows.iter().any(|row| row.title == "Show program progress"));
        stage(&mut runtime, edited, 0);
        assert_eq!(displayed(&runtime, edited), Some(json!(0)));
        assert_eq!(displayed(&runtime, untouched), Some(json!(1)));
        assert!(runtime.classic_preferences().show_program());
        assert!(runtime.classic_preferences().show_progress());
        let candidate = merge(&runtime, address(&runtime).unwrap()).unwrap().unwrap();
        let [key, revision] = runtime.osd_setting_storage_keys(edited).unwrap();
        assert_eq!(key, format!("provider:m3u:{edited}"));
        assert_eq!(revision, format!("settingRevision:{key}"));
        let mut expected = before.clone();
        expected["settings"][&key] = json!(0);
        expected["settings"][&revision] = candidate.settings[&revision].clone();
        assert_eq!(serde_json::to_value(candidate).unwrap(), expected);
        perform(&mut runtime, "back");
        assert!(runtime.settings_groups.section.is_none());
        assert_eq!(saved(&runtime), before);
        assert_eq!(runtime.classic_display, accepted);
        assert!(!runtime.saving);
        assert!(runtime.rx.try_recv().is_err());
    }
}

#[test]
fn clean_channel_draft_rejects_provider_only_change_before_writer() {
    let _serial = plx_base::testlock::serial();
    let mut runtime = runtime();
    runtime.provider = ProviderKind::M3u;
    open_group(&mut runtime, "channels");
    assert_eq!(displayed(&runtime, "showProgress"), Some(json!(1)));
    stage(&mut runtime, "showProgress", 0);
    let before = saved(&runtime);
    let accepted = runtime.classic_display.clone();
    let preferences = runtime.classic_preferences;
    let epochs = (runtime.generation, runtime.navigation_epoch, runtime.playback_epoch);
    // The valid staged change must not reach the writer after only the provider changes.
    runtime.provider = ProviderKind::Plex;
    let mut writes = 0;
    assert_eq!(save_with(&mut runtime, |_, _| { writes += 1; Ok(()) }), Err(STALE));
    assert_eq!(writes, 0);
    assert_eq!(saved(&runtime), before);
    assert_eq!(runtime.classic_display, accepted);
    assert_eq!(runtime.classic_preferences, preferences);
    assert_eq!((runtime.generation, runtime.navigation_epoch, runtime.playback_epoch), epochs);
    assert!(!runtime.saving);
    assert!(runtime.settings_groups.draft.pending().is_none());
    assert!(runtime.rx.try_recv().is_err());
}

#[test]
fn progress_draft_rejects_revision_only_aba_without_changing_accepted_state() {
    let _serial = plx_base::testlock::serial();
    let mut runtime = runtime();
    runtime.provider = ProviderKind::M3u;
    let [key, revision] = runtime.osd_setting_storage_keys("showProgress").unwrap();
    runtime.saved.settings.insert(key.clone(), json!(1));
    runtime.saved.settings.insert(revision.clone(), json!("before"));
    open_group(&mut runtime, "channels");
    stage(&mut runtime, "showProgress", 0);
    runtime.saved.settings.insert(key.clone(), json!(0));
    runtime.saved.settings.insert(key, json!(1));
    runtime.saved.settings.insert(revision, json!("after-aba"));
    let before = saved(&runtime);
    let accepted = runtime.classic_display.clone();
    let mut writes = 0;
    assert_eq!(save_with(&mut runtime, |_, _| { writes += 1; Ok(()) }), Err(STALE));
    assert_eq!(writes, 0);
    assert_eq!(saved(&runtime), before);
    assert_eq!(runtime.classic_display, accepted);
    assert_eq!(displayed(&runtime, "showProgress"), Some(json!(0)));
    assert!(runtime.settings_groups.draft.pending().is_none());
}

#[test]
fn unchanged_save_returns_focus_to_its_group_without_starting_a_write() {
    let _serial = plx_base::testlock::serial();
    for group in ["interface", "colors", "lists", "infobar", "buttons", "channels", "menu"] {
        let mut runtime = runtime();
        let before = saved(&runtime);
        open_group(&mut runtime, group);
        if group == "lists" {
            stage(&mut runtime, "pageSize", 20);
            stage(&mut runtime, "pageSize", 25);
        }
        perform(&mut runtime, "save");
        assert!(runtime.settings_groups.section.is_none());
        assert!(runtime.settings_groups.draft.address().is_none());
        assert!(runtime.settings_groups.draft.pending().is_none());
        assert_eq!(projected(&runtime).selected_row, Some(format!("settings.open:{group}")));
        assert_eq!(saved(&runtime), before);
        assert!(!runtime.saving);
        assert!(runtime.rx.try_recv().is_err());
    }
}

#[test]
fn unsuccessful_apply_or_replaced_owner_does_not_restore_the_saved_group_focus() {
    let _serial = plx_base::testlock::serial();
    for (owner, applied) in [(InstanceId(7), false), (InstanceId(8), true)] {
        let mut runtime = runtime();
        open_group(&mut runtime, "lists");
        stage(&mut runtime, "pageSize", 20);
        save_with(&mut runtime, |runtime, _| { runtime.saving = true; Ok(()) }).unwrap();
        runtime.saving = false;
        runtime.status = "Saved, but could not apply settings".into();
        finish(&mut runtime, Some(owner), true, applied);
        assert!(runtime.settings_groups.section.is_none());
        assert!(runtime.settings_groups.draft.pending().is_none());
        assert!(runtime.settings_groups.focus.is_none());
        assert_ne!(projected(&runtime).selected_row.as_deref(), Some("settings.open:lists"));
        assert_eq!(runtime.status, "Saved, but could not apply settings");
    }
}

#[cfg(feature = "hostsim")]
mod saved_delivery {
    use super::*;
    use plx_machine::machine::Tick;
    use plx_screens::registry::AppArg;

    fn tick(app: &mut App) {
        let sample = plx_base::task::time::ClockSample::capture_main(app.adapters.player.mt())
            .expect("host fixture clock");
        crate::app::ott_runtime::tick(app, sample);
    }

    fn setup() -> Box<App> {
        let mut app = Box::new(crate::app::run::lifecycle_regression_tests::app());
        app.ott = Some(Box::new(runtime()));
        let runtime = app.ott.as_mut().unwrap();
        runtime.catalog_done.store(false, Ordering::Release);
        runtime.metadata_done.store(false, Ordering::Release);
        runtime.resolver_busy = true;
        runtime.plex_refresh_pending = true;
        runtime.continuity.stop(runtime.generation);
        crate::app::bridge::show_page(&mut app.pages, AppArg::Ott);
        for _ in 0..24 {
            app.loop_t = app.loop_t.wrapping_add(16);
            let frame = Tick { ms: app.loop_t, dt_us: 16_000 };
            let target = app.as_mut();
            crate::app::bridge::frame(&mut target.pages, &mut target.bridge, frame, Vec::new());
        }
        tick(&mut app);
        let owner = current_ui_owner(&app).expect("mounted interactive OTT screen");
        let runtime = app.ott.as_mut().unwrap();
        runtime.page = "settings";
        runtime.reply = Some(owner);
        runtime.settings_focus = None;
        app
    }

    // Inject only writer admission/completion: no protected write worker. The
    // completion itself traverses the real runtime Job::Saved handler.
    fn stage_save(app: &mut App) -> Saved {
        let runtime = app.ott.as_mut().unwrap();
        open_group(runtime, "lists");
        stage(runtime, "pageSize", 20);
        let mut candidate = None;
        save_with(runtime, |runtime, next| {
            candidate = Some(next);
            runtime.saving = true;
            runtime.save_navigation = SaveNavigation::Preserve;
            runtime.save_ui_owner = None;
            runtime.save_playback_epoch = runtime.playback_epoch;
            Ok(())
        }).unwrap();
        assert_eq!(runtime.saved.settings["pageSize"], 25);
        assert!(runtime.settings_groups.draft.pending().is_some());
        candidate.unwrap()
    }

    fn deliver(app: &mut App, candidate: Saved) {
        app.ott.as_ref().unwrap().tx.send(Job::Saved(Ok(candidate), None, false, Value::Null))
            .unwrap_or_else(|_| panic!("fixture receiver disappeared"));
        tick(app);
        let runtime = app.ott.as_ref().unwrap();
        assert_eq!(runtime.saved.settings["pageSize"], 20);
        assert!(!runtime.saving);
        assert!(runtime.settings_groups.draft.pending().is_none());
    }

    #[test]
    fn accepted_saved_job_restores_the_current_groups_root_row() {
        let _serial = plx_base::testlock::serial();
        let mut app = setup();
        let owner = current_ui_owner(&app);
        let candidate = stage_save(&mut app);
        deliver(&mut app, candidate);
        let runtime = app.ott.as_ref().unwrap();
        assert_eq!(current_ui_owner(&app), owner);
        assert!(runtime.settings_groups.section.is_none());
        assert_eq!(runtime.status, "Settings saved");
        assert_eq!(projected(runtime).selected_row.as_deref(), Some("settings.open:lists"));
    }

    #[test]
    fn late_saved_job_preserves_focus_from_the_newer_wrapper() {
        let _serial = plx_base::testlock::serial();
        let mut app = setup();
        let candidate = stage_save(&mut app);
        let runtime = app.ott.as_mut().unwrap();
        perform(runtime, "back");
        open_group(runtime, "remote");
        perform(runtime, "back");
        assert_eq!(projected(runtime).selected_row.as_deref(), Some("settings.open:remote"));
        deliver(&mut app, candidate);
        let runtime = app.ott.as_ref().unwrap();
        assert!(runtime.settings_groups.section.is_none());
        assert_eq!(projected(runtime).selected_row.as_deref(), Some("settings.open:remote"));
        assert_ne!(runtime.status, "Settings saved");
    }
    #[test]
    fn addressed_hex_editor_stages_only_its_own_draft_and_keeps_outer_save_cancel() {
        let _serial = plx_base::testlock::serial();
        let mut app = setup();
        let mut runtime = app.ott.take().unwrap();
        let before = saved(&runtime);
        let epochs = (runtime.generation, runtime.navigation_epoch, runtime.playback_epoch);
        let palette = plx_ui::palette::current();
        open_group(&mut runtime, "colors");
        perform(&mut runtime, "pick:background");
        let stale_swatch = command(&runtime, "choose:background:#FFFFFF");
        let start = command(&runtime, "hex:background");
        color_action(&mut runtime, &mut app, &start).unwrap().unwrap();
        assert!(action(&mut runtime, &stale_swatch).unwrap().is_err(), "opening HEX retires old list controls");
        let editor = runtime.edit_session.clone().expect("ordinary addressed editor opened");
        assert_eq!(editor.key, "settings.color.background");
        assert_eq!(runtime.settings_groups.picker, Some("background"));
        let submit = |value: &str| OttAction::SubmitSetting { session: editor.id,
            key: editor.key.clone(), value: value.into() };
        assert!(presentation_action(&runtime, &submit("abc123")));
        let wrong = OttAction::SubmitSetting { session: editor.id, key: "settings.color.text".into(), value: "FFFFFF".into() };
        assert!(color_action(&mut runtime, &mut app, &wrong).unwrap().is_err());
        assert!(runtime.edit_session.as_ref() == Some(&editor));
        color_action(&mut runtime, &mut app, &submit("nothex")).unwrap().unwrap();
        assert!(runtime.edit_session.as_ref() == Some(&editor), "invalid input remains editable");
        assert_eq!(saved(&runtime), before);
        color_action(&mut runtime, &mut app, &submit("abc123")).unwrap().unwrap();
        assert!(runtime.edit_session.is_none());
        assert_eq!(displayed(&runtime, "uiColorConfig").unwrap(),
            json!(r##"{"version":1,"colors":{"background":"#ABC123"}}"##));
        assert_eq!(saved(&runtime), before);
        assert_eq!(plx_ui::palette::current(), palette);
        assert_eq!((runtime.generation, runtime.navigation_epoch, runtime.playback_epoch), epochs);
        assert!(color_action(&mut runtime, &mut app, &submit("FFFFFF")).unwrap().is_err(), "old submit cannot reenter");
        let old_cancel = OttAction::CancelSetting { session: editor.id };
        assert!(presentation_action(&runtime, &old_cancel));
        let staged = displayed(&runtime, "uiColorConfig");
        assert!(color_action(&mut runtime, &mut app, &old_cancel).unwrap().is_err());
        assert_eq!(displayed(&runtime, "uiColorConfig"), staged, "old Cancel cannot discard staged HEX");
        let candidate = merge(&runtime, address(&runtime).unwrap()).unwrap().unwrap();
        assert_eq!(candidate.settings["uiColorConfig"], displayed(&runtime, "uiColorConfig").unwrap());

        perform(&mut runtime, "pick:text");
        let start = command(&runtime, "hex:text");
        color_action(&mut runtime, &mut app, &start).unwrap().unwrap();
        let active = runtime.edit_session.clone();
        let nonce = runtime.settings_groups.nonce;
        assert!(presentation_action(&runtime, &old_cancel));
        assert!(color_action(&mut runtime, &mut app, &old_cancel).unwrap().is_err());
        assert!(runtime.edit_session == active, "old Cancel cannot retire the new form");
        assert_eq!(runtime.settings_groups.nonce, nonce);
        assert_eq!(displayed(&runtime, "uiColorConfig"), staged);
        let cancel = OttAction::CancelSetting { session: runtime.edit_session.as_ref().unwrap().id };
        assert!(presentation_action(&runtime, &cancel));
        color_action(&mut runtime, &mut app, &cancel).unwrap().unwrap();
        assert!(runtime.edit_session.is_none());
        assert_eq!(runtime.settings_groups.picker, Some("text"));
        perform(&mut runtime, "back");
        perform(&mut runtime, "back");
        assert_eq!(saved(&runtime), before, "outer Cancel drops staged HEX");
        assert_eq!(plx_ui::palette::current(), palette);
    }

    #[test]
    fn addressed_hex_editor_rejects_source_replacement_and_empty_value_inherits() {
        let _serial = plx_base::testlock::serial();
        let mut app = setup();
        let mut runtime = app.ott.take().unwrap();
        open_group(&mut runtime, "colors");
        perform(&mut runtime, "pick:text");
        perform(&mut runtime, "choose:text:#FFFFFF");
        perform(&mut runtime, "pick:text");
        let start = command(&runtime, "hex:text");
        color_action(&mut runtime, &mut app, &start).unwrap().unwrap();
        let editor = runtime.edit_session.clone().unwrap();
        color_action(&mut runtime, &mut app, &OttAction::SubmitSetting { session: editor.id,
            key: editor.key, value: String::new() }).unwrap().unwrap();
        assert_eq!(displayed(&runtime, "uiColorConfig"), Some(json!("")));
        perform(&mut runtime, "pick:text");
        let start = command(&runtime, "hex:text");
        color_action(&mut runtime, &mut app, &start).unwrap().unwrap();
        let editor = runtime.edit_session.clone().unwrap();
        runtime.navigation_epoch += 1;
        color_action(&mut runtime, &mut app, &OttAction::SubmitSetting { session: editor.id,
            key: editor.key, value: "FFFFFF".into() }).unwrap().unwrap();
        assert_eq!(displayed(&runtime, "uiColorConfig"), Some(json!("")), "stale form cannot stage");
        assert!(runtime.edit_session.is_some());
        cancel(&mut runtime);
        assert!(runtime.edit_session.is_none(), "retiring the group also retires its own form");
    }

}

#[test]
fn color_roles_stage_one_canonical_document_without_publishing_or_navigation() {
    let _serial = plx_base::testlock::serial();
    let mut runtime = runtime();
    let before = saved(&runtime);
    let projected_before = plx_ui::palette::current();
    let epochs = (runtime.generation,runtime.navigation_epoch,runtime.playback_epoch);
    open_group(&mut runtime,"colors");
    for role in plx_ui::palette::Role::ALL { let _ = command(&runtime,&format!("pick:{}",role.id())); }
    perform(&mut runtime,"pick:background");
    perform(&mut runtime,"choose:background:#0F172A");
    perform(&mut runtime,"pick:epgTime");
    perform(&mut runtime,"choose:epgTime:#60A5FA");
    assert_eq!(saved(&runtime),before);
    assert_eq!(plx_ui::palette::current(),projected_before);
    assert_eq!((runtime.generation,runtime.navigation_epoch,runtime.playback_epoch),epochs);
    let candidate = merge(&runtime,address(&runtime).unwrap()).unwrap().unwrap();
    assert_eq!(candidate.settings["uiColorConfig"], r##"{"version":1,"colors":{"background":"#0F172A","epgTime":"#60A5FA"}}"##);
    assert!(candidate.settings.contains_key("settingRevision:uiColorConfig"));
    let mut writes = 0;
    save_with(&mut runtime,|_,value| { writes+=1;assert_eq!(value.settings,candidate.settings);Ok(()) }).unwrap();
    assert_eq!(writes,1);
    assert!(runtime.settings_groups.draft.pending().is_some());
    assert!(save_with(&mut runtime,|_,_|panic!("duplicate palette write")).is_err());
}

#[test]
fn color_picker_back_group_cancel_and_reset_preserve_saved_values() {
    let _serial = plx_base::testlock::serial();
    let mut runtime = runtime();
    let initial = plx_ui::palette::Config::preset("midnight").unwrap().encode();
    runtime.saved.settings.insert("uiColorConfig".into(),json!(initial));
    let before = saved(&runtime);
    open_group(&mut runtime,"colors");
    perform(&mut runtime,"scheme:light");
    perform(&mut runtime,"pick:text");
    perform(&mut runtime,"back");
    assert!(runtime.settings_groups.picker.is_none());
    perform(&mut runtime,"back");
    assert_eq!(saved(&runtime),before);
    open_group(&mut runtime,"colors");
    assert_eq!(displayed(&runtime,"uiColorConfig"),Some(json!(initial)));
    perform(&mut runtime,"scheme:inherit");
    assert_eq!(merge(&runtime,address(&runtime).unwrap()).unwrap().unwrap().settings["uiColorConfig"],"");
    perform(&mut runtime,"back");
    assert_eq!(saved(&runtime),before);
}

#[test]
fn color_draft_rejects_external_revision_change_and_noncanonical_action() {
    let _serial = plx_base::testlock::serial();
    let mut runtime = runtime();
    open_group(&mut runtime,"colors");
    perform(&mut runtime,"pick:text");
    let invalid = OttAction::EditSetting(format!("{PREFIX}{}:choose:text:#ffffff",runtime.settings_groups.nonce));
    assert!(apply(&mut runtime,invalid).is_err());
    perform(&mut runtime,"choose:text:#FFFFFF");
    runtime.saved.settings.insert("settingRevision:uiColorConfig".into(),json!("external"));
    assert_eq!(save_with(&mut runtime,|_,_|panic!("stale palette write")),Err(STALE));
    assert!(runtime.settings_groups.draft.pending().is_none());
}

#[test]
fn corrupt_stored_color_is_repairable_only_after_explicit_save() {
    let _serial = plx_base::testlock::serial();
    for invalid in [json!("not JSON"), json!(12), json!(r#"{"version":2,"colors":{}}"#)] {
        let mut runtime = runtime();
        runtime.saved.settings.insert("uiColorConfig".into(), invalid.clone());
        let before = saved(&runtime);
        open_group(&mut runtime, "colors");
        for role in plx_ui::palette::Role::ALL {
            assert!(projected(&runtime).rows.iter().any(|row| row.title == role.title()));
        }
        perform(&mut runtime, "back");
        assert_eq!(saved(&runtime), before);
        open_group(&mut runtime, "colors");
        perform(&mut runtime, "scheme:inherit");
        assert_eq!(saved(&runtime), before);
        let candidate = merge(&runtime, address(&runtime).unwrap()).unwrap().unwrap();
        assert_eq!(candidate.settings["uiColorConfig"], "");
    }
}

#[test]
fn remote_group_shows_only_durable_identity_as_read_only() {
    let _serial = plx_base::testlock::serial();
    let mut runtime = runtime();
    open_group(&mut runtime, "remote");
    let rows = projected(&runtime).rows;
    let row = rows.iter().find(|row| row.id == "remote.device_id").unwrap();
    assert_eq!(row.detail, "group_fixture");
    assert!(!row.enabled);
    runtime.remote = Remote::from_install("provisional_fixture", "fixture");
    let rows = projected(&runtime).rows;
    let row = rows.iter().find(|row| row.id == "remote.device_id").unwrap();
    assert_eq!(row.detail, "Identity loading or unavailable");
    assert!(!row.enabled);
    assert!(!row.detail.contains("provisional_fixture"));
}
