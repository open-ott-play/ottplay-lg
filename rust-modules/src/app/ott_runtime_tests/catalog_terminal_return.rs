// Included by navigation.rs: use its real Bridge navigation and held worker fixture.

fn terminal_catalog_return_app() -> App {
    let mut app = m3u_navigation_fixture();
    let runtime = app.ott.as_mut().unwrap();
    let saved = runtime.interface_mode_candidate(plx_machine::machine::UiMode::Classic);
    runtime.last_checkpoint_ms = runtime.now_ms();
    runtime.saving = true;
    runtime.save_navigation = SaveNavigation::Preserve;
    runtime.save_playback_epoch = runtime.playback_epoch;
    runtime.tx.send(Job::Saved(Ok(saved), None, false, json!({"status":"ok"}))).unwrap();
    runtime_tick(&mut app);
    assert_eq!(app.bridge.ui_mode(), plx_machine::machine::UiMode::Classic);
    assert!(!app.ott.as_ref().unwrap().saving);
    app
}

fn terminal_catalog_status(app: &mut App, serial: u64) -> Value {
    super::provider_context::read_status(app, "status", serial)["data"]["catalog"].clone()
}

#[test]
fn completed_empty_catalog_return_retains_completion_without_reloading() {
    let _serial = plx_base::testlock::serial();
    let mut app = terminal_catalog_return_app();
    app.ott.as_mut().unwrap().catalog.entries.clear();
    app.ott.as_mut().unwrap().selected = None;
    mount_ott(&mut app, "channels", false);
    let generation = app.ott.as_ref().unwrap().generation;
    app.ott.as_ref().unwrap().catalog_tx
        .send(CatalogEvent::Finished(generation, Ok(()))).unwrap();
    runtime_tick(&mut app);
    settle_pages(&mut app);
    let runtime = app.ott.as_ref().unwrap();
    assert!(runtime.catalog_complete && !runtime.catalog_loading);
    assert!(runtime.catalog.entries.is_empty());
    assert_eq!(runtime.raw_view().catalog_progress.unwrap().total, Some(0));
    let saved = serde_json::to_value(&runtime.saved).unwrap();

    press_back(&mut app);
    assert_eq!(app.ott.as_ref().unwrap().page, "profiles");
    show_channel_list(&mut app);

    let runtime = app.ott.as_ref().unwrap();
    let view = runtime.raw_view();
    let progress = view.catalog_progress.expect("returned channel list has progress");
    assert!(runtime.catalog_complete && progress.complete,
        "a completed empty catalogue must remain known complete after Back");
    assert_eq!(progress.total, Some(0));
    assert!(!runtime.catalog_loading && !progress.loading);
    assert!(runtime.catalog_next.is_none(), "return must not reload a completed catalogue");
    assert_eq!(runtime.page, "channels");
    assert!(runtime.catalog.entries.is_empty());
    assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), saved);
    assert!(!runtime.saving && runtime.play_target.is_none() && runtime.resolver_next.is_none());
    assert!(runtime.playing.is_none(), "returning to an empty list cannot start playback");
    let generation = runtime.generation;
    assert_eq!(terminal_catalog_status(&mut app, 1), json!({
        "version":1,"scope":"source","generation":generation,
        "loading":false,"complete":true,"count":0,"error_code":null
    }));
}

#[test]
fn failed_catalog_return_retains_visible_error_and_rows_without_retrying() {
    let _serial = plx_base::testlock::serial();
    const FAILURE: &str = "Source unavailable";
    for had_batch in [false, true] {
        let mut app = terminal_catalog_return_app();
        app.ott.as_mut().unwrap().catalog.entries.clear();
        app.ott.as_mut().unwrap().selected = None;
        mount_ott(&mut app, "channels", false);
        let runtime = app.ott.as_ref().unwrap();
        let generation = runtime.generation;
        if had_batch {
            runtime.catalog_tx.send(CatalogEvent::Batch(generation, Catalog {
                hierarchy: None, entries: vec![m3u_navigation_entry("partial")],
            })).unwrap();
        }
        runtime.catalog_tx.send(CatalogEvent::Finished(generation, Err(FAILURE))).unwrap();
        runtime_tick(&mut app);
        settle_pages(&mut app);
        let runtime = app.ott.as_ref().unwrap();
        assert!(!runtime.catalog_complete && !runtime.catalog_loading);
        assert!(runtime.raw_view().status.contains(FAILURE));
        let saved = serde_json::to_value(&runtime.saved).unwrap();
        let entries = runtime.catalog.entries.clone();
        let cancelled = runtime.cancel.clone();
        assert_eq!(terminal_catalog_status(&mut app, 1), json!({
            "version":1,"scope":"source","generation":generation,
            "loading":false,"complete":false,"count":entries.len(),"error_code":"catalog_failed"
        }));

        press_back(&mut app);
        assert_eq!(app.ott.as_ref().unwrap().page, "profiles");
        show_channel_list(&mut app);

        let runtime = app.ott.as_ref().unwrap();
        let view = runtime.raw_view();
        assert!(view.status.contains(FAILURE),
            "a failed catalogue must retain its visible error after Back; had_batch={had_batch}");
        assert_eq!(runtime.catalog.entries, entries);
        assert!(!runtime.catalog_complete && !runtime.catalog_loading);
        assert!(Arc::ptr_eq(&runtime.cancel, &cancelled));
        assert!(cancelled.load(Ordering::Acquire), "failed return must not rearm the cancelled owner");
        assert!(runtime.catalog_next.is_none(), "return must not automatically retry a failed catalogue");
        let progress = view.catalog_progress.expect("returned channel list has progress");
        assert!(!progress.complete && !progress.loading);
        assert_eq!(progress.total, None);
        assert_eq!(runtime.page, "channels");
        assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), saved);
        assert!(!runtime.saving && runtime.play_target.is_none() && runtime.resolver_next.is_none());
        assert!(runtime.playing.is_none(), "returning to a failed list cannot start playback");
        let returned_generation = runtime.generation;
        assert_ne!(returned_generation, generation, "Back retires the old source worker");
        assert_eq!(terminal_catalog_status(&mut app, 2), json!({
            "version":1,"scope":"source","generation":returned_generation,
            "loading":false,"complete":false,"count":entries.len(),"error_code":"catalog_failed"
        }));
    }
}

#[test]
fn terminal_catalog_return_cannot_restore_failure_into_a_changed_source_or_generation() {
    let _serial = plx_base::testlock::serial();
    for replacement in ["profile", "source", "generation"] {
        let mut app = terminal_catalog_return_app();
        mount_ott(&mut app, "channels", false);
        let runtime = app.ott.as_ref().unwrap();
        runtime.catalog_tx.send(CatalogEvent::Finished(runtime.generation,
            Err("Original catalogue failed"))).unwrap();
        runtime_tick(&mut app);
        assert_eq!(terminal_catalog_status(&mut app, 1)["error_code"], "catalog_failed");
        press_back(&mut app);
        press_back(&mut app);
        let runtime = app.ott.as_mut().unwrap();
        assert_eq!(runtime.page, "providers");
        assert!(runtime.catalog_return.is_some());
        match replacement {
            "profile" => runtime.saved.profiles.active_profile = Some("second".into()),
            "source" => runtime.saved.profiles.profiles[0].source = "https://media.invalid/new.m3u".into(),
            _ => runtime.generation = runtime.generation.wrapping_add(1),
        }
        // A typed receipt now belongs to the new current source. The old Back
        // marker must neither replace it nor erase it when ShowCatalog opens.
        runtime.catalog_diagnostics.failed(runtime.generation,
            crate::app::ott_runtime::catalog_diagnostics::Failure::SourceUnavailable);
        runtime.status = "New source unavailable".into();
        let generation = runtime.generation;
        let saved = serde_json::to_value(&runtime.saved).unwrap();
        let expected = terminal_catalog_status(&mut app, 2);
        assert_eq!(expected["error_code"], "source_unavailable");

        show_channel_list(&mut app);

        assert_eq!(terminal_catalog_status(&mut app, 3), expected, "{replacement}");
        let runtime = app.ott.as_ref().unwrap();
        assert_eq!(runtime.generation, generation);
        assert_eq!(runtime.status, "New source unavailable");
        assert!(runtime.catalog_return.is_none() && runtime.catalog_next.is_none());
        assert!(!runtime.catalog_loading && !runtime.catalog_complete);
        assert!(!runtime.saving && runtime.play_target.is_none() && runtime.resolver_next.is_none());
        assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), saved);
    }
}


fn terminal_catalog_action(app: &mut App, action: OttAction) {
    let reply = app.pages.top_page().expect("mounted OTT instance");
    app.pages.emit(MachineId::Instance(reply), Fx::App(AppFx::Ott(OttOut::Action {
        reply, action,
    })));
    let tick = next_tick(app);
    bridge::frame(&mut app.pages, &mut app.bridge, tick, Vec::new());
    runtime_tick(app);
    settle_pages(app);
}

fn completed_catalog_with_actions() -> App {
    let mut app = terminal_catalog_return_app();
    let (first_id, second) = {
        let runtime = app.ott.as_mut().unwrap();
        runtime.entropy = Some([7; 16]);
        // Actions uses organization identities, unlike the generic navigation
        // fixture's bare IDs. Admit the same rows through the actual M3U parser.
        let mut catalog = plx_data::ott::parse_m3u(
            "#EXTM3U\n#EXTINF:-1 group-title=\"News\",One\nhttps://media.invalid/one.mp4\n\
             #EXTINF:-1 group-title=\"News\",Two\nhttps://media.invalid/two.mp4\n",
            runtime.active().unwrap(),
        ).unwrap();
        let second = catalog.entries.pop().unwrap();
        let first_id = catalog.entries[0].id.clone();
        runtime.catalog = catalog;
        runtime.catalog_revision = runtime.catalog_revision.wrapping_add(1);
        runtime.selected = Some(first_id.clone());
        (first_id, second)
    };
    mount_ott(&mut app, "channels", false);
    let runtime = app.ott.as_ref().unwrap();
    runtime.catalog_tx.send(CatalogEvent::Batch(runtime.generation, Catalog {
        hierarchy: None, entries: vec![second],
    })).unwrap();
    runtime.catalog_tx.send(CatalogEvent::Finished(runtime.generation, Ok(()))).unwrap();
    runtime_tick(&mut app);
    settle_pages(&mut app);
    let runtime = app.ott.as_mut().unwrap();
    // Start with an already committed browse selection. Navigation must not
    // need a focus-only writer; this fixture never starts persistence IO.
    browse_focus::record(runtime, &first_id);
    let mut saved = runtime.saved.clone();
    browse_focus::prepare(runtime, &mut saved).unwrap();
    runtime.saved = saved;
    runtime.sync_saved_context();
    assert!(runtime.raw_view().order_context.is_some(), "ready source offers Actions");
    app
}

#[test]
fn completed_catalog_settings_and_channel_back_preserve_actions_without_reviving_workers() {
    let _serial = plx_base::testlock::serial();
    for through_settings in [false, true] {
        let mut app = completed_catalog_with_actions();
        let runtime = app.ott.as_ref().unwrap();
        let old_generation = runtime.generation;
        let old_cancel = runtime.cancel.clone();
        let old_actions = runtime.raw_view().order_context.unwrap();
        let entries = runtime.catalog.entries.clone();
        let selected = runtime.selected.clone();
        let saved = serde_json::to_value(&runtime.saved).unwrap();
        if through_settings {
            // The actual Settings detour: enter the Interface group, close it,
            // then close root Settings. No preference is changed in this test.
            terminal_catalog_action(&mut app, OttAction::ShowSettings);
            assert_eq!(app.ott.as_ref().unwrap().page, "settings");
            terminal_catalog_action(&mut app,
                OttAction::EditSetting("settings.open:interface".into()));
            assert!(app.ott.as_ref().unwrap().view().rows.iter().any(|row|
                matches!(&row.action, OttAction::EditSetting(key) if key.ends_with(":language"))));
            press_back(&mut app);
            assert!(app.ott.as_ref().unwrap().view().rows.iter().any(|row|
                row.id == "settings.open:interface"));
        }
        press_back(&mut app);
        let runtime = app.ott.as_ref().unwrap();
        assert_eq!(runtime.page, if through_settings { "providers" } else { "profiles" });
        assert!(old_cancel.load(Ordering::Acquire), "Back still retires the old producer");
        assert_ne!(runtime.generation, old_generation);
        show_channel_list(&mut app);

        let runtime = app.ott.as_ref().unwrap();
        let view = runtime.raw_view();
        let progress = view.catalog_progress.unwrap();
        assert!(progress.complete && !progress.loading, "settings={through_settings}");
        assert_eq!(progress.total, Some(entries.len()));
        let new_actions = view.order_context.expect("completed source still offers Actions");
        assert_ne!(new_actions, old_actions, "old action authority must stay retired");
        assert!(!Arc::ptr_eq(&runtime.cancel, &old_cancel));
        assert!(!runtime.cancel.load(Ordering::Acquire));
        assert!(old_cancel.load(Ordering::Acquire), "never clear a retired worker's token");
        assert!(runtime.catalog_next.is_none() && !runtime.catalog_loading);
        assert_eq!(runtime.catalog.entries, entries);
        assert_eq!(runtime.selected, selected);
        assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), saved);
        assert!(!runtime.saving && runtime.play_target.is_none() && runtime.resolver_next.is_none());
        assert!(runtime.playing.is_none());
        // A late producer cannot modify the retained list under its new epoch.
        runtime.catalog_tx.send(CatalogEvent::Batch(old_generation, Catalog {
            hierarchy: None, entries: vec![m3u_navigation_entry("retired")],
        })).unwrap();
        runtime.catalog_tx.send(CatalogEvent::Finished(old_generation, Err("Retired failure"))).unwrap();
        runtime_tick(&mut app);
        let runtime = app.ott.as_ref().unwrap();
        assert_eq!(runtime.catalog.entries, entries);
        assert!(runtime.catalog_complete && runtime.raw_view().order_context.is_some());
        assert!(!runtime.status.contains("Retired failure"));
    }
}

#[test]
fn settings_catalog_return_does_not_rearm_a_replaced_source_or_generation() {
    let _serial = plx_base::testlock::serial();
    for replacement in ["profile", "source", "generation"] {
        let mut app = completed_catalog_with_actions();
        terminal_catalog_action(&mut app, OttAction::ShowSettings);
        press_back(&mut app);
        let runtime = app.ott.as_mut().unwrap();
        assert_eq!(runtime.page, "providers");
        assert!(runtime.catalog_return.is_some(), "capture the Settings terminal outcome first");
        let cancelled = runtime.cancel.clone();
        match replacement {
            "profile" => runtime.saved.profiles.active_profile = Some("second".into()),
            "source" => runtime.saved.profiles.profiles[0].source = "https://media.invalid/replaced.m3u".into(),
            _ => runtime.generation = runtime.generation.wrapping_add(1),
        }
        let generation = runtime.generation;
        let saved = serde_json::to_value(&runtime.saved).unwrap();
        show_channel_list(&mut app);
        let runtime = app.ott.as_ref().unwrap();
        assert!(Arc::ptr_eq(&runtime.cancel, &cancelled), "{replacement}");
        assert!(cancelled.load(Ordering::Acquire));
        assert_eq!(runtime.generation, generation);
        assert!(!runtime.catalog_complete && !runtime.catalog_loading);
        assert!(runtime.raw_view().order_context.is_none());
        assert!(runtime.catalog_return.is_none() && runtime.catalog_next.is_none());
        assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), saved);
        assert!(!runtime.saving && runtime.play_target.is_none() && runtime.resolver_next.is_none());
        assert!(runtime.playing.is_none());
    }
}


#[test]
fn settings_category_return_restores_complete_actions_without_reviving_workers() {
    let _serial = plx_base::testlock::serial();
    let mut app = Box::new(completed_catalog_with_actions());
    app.ott.as_mut().unwrap().write_permit.revoke();
    let owner = app.pages.top_page();
    let runtime = app.ott.as_ref().unwrap();
    let old_generation = runtime.generation;
    let old_cancel = runtime.cancel.clone();
    let old_actions = runtime.raw_view().order_context.unwrap();
    let entries = runtime.catalog.entries.clone();
    let selected = runtime.selected.clone();
    let saved = serde_json::to_value(&runtime.saved).unwrap();

    terminal_catalog_action(&mut app, OttAction::ShowSettings);
    assert_eq!(app.ott.as_ref().unwrap().page, "settings");
    press_back(&mut app);
    let runtime = app.ott.as_ref().unwrap();
    assert_eq!(runtime.page, "providers");
    assert!(runtime.catalog_return.is_some());
    assert!(old_cancel.load(Ordering::Acquire));
    assert_ne!(runtime.generation, old_generation);
    let returned_generation = runtime.generation;

    // Blue Category is another entrance to the retained catalogue. It must
    // restore the same completed source before the user chooses a category.
    terminal_catalog_action(&mut app, OttAction::ShowCategories);
    let runtime = app.ott.as_ref().unwrap();
    assert_eq!(runtime.page, "categories");
    assert!(runtime.catalog_complete, "Category must consume the completed return intent");
    assert!(!runtime.catalog_loading && runtime.catalog_next.is_none());
    assert!(runtime.catalog_return.is_none());
    assert_eq!(runtime.generation, returned_generation);
    assert!(!Arc::ptr_eq(&runtime.cancel, &old_cancel));
    assert!(!runtime.cancel.load(Ordering::Acquire));
    assert!(old_cancel.load(Ordering::Acquire), "the old worker must remain retired");
    assert_eq!(runtime.catalog.entries, entries);
    assert_eq!(runtime.selected, selected);
    assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), saved);
    assert!(!runtime.saving && runtime.play_target.is_none() && runtime.resolver_next.is_none());
    assert!(runtime.playing.is_none());

    press_back(&mut app);
    let runtime = app.ott.as_ref().unwrap();
    assert_eq!(runtime.page, "channels");
    assert_eq!(app.pages.top_page(), owner);
    assert_eq!(runtime.generation, returned_generation);
    let view = runtime.raw_view();
    let progress = view.catalog_progress.unwrap();
    assert!(progress.complete && !progress.loading);
    assert_eq!(progress.total, Some(entries.len()));
    let actions = view.order_context.expect("return through Category must retain Actions");
    assert_ne!(actions, old_actions, "retired action authority cannot be reused");
    assert_eq!(runtime.catalog.entries, entries);
    assert_eq!(runtime.selected, selected);
    assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), saved);
    assert!(runtime.catalog_next.is_none() && runtime.play_target.is_none() && runtime.resolver_next.is_none());
    assert!(!runtime.saving && runtime.playing.is_none());

    runtime.catalog_tx.send(CatalogEvent::Batch(old_generation, Catalog {
        hierarchy: None, entries: vec![m3u_navigation_entry("retired-category-return")],
    })).unwrap();
    runtime.catalog_tx.send(CatalogEvent::Finished(old_generation, Err("Retired category failure"))).unwrap();
    runtime_tick(&mut app);
    let runtime = app.ott.as_ref().unwrap();
    assert_eq!(runtime.catalog.entries, entries);
    assert_eq!(runtime.selected, selected);
    assert!(runtime.catalog_complete && runtime.raw_view().order_context.is_some());
    assert!(!runtime.status.contains("Retired category failure"));
    assert!(runtime.catalog_next.is_none() && runtime.play_target.is_none() && runtime.resolver_next.is_none());
    assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), saved);
}

#[test]
fn settings_category_return_rejects_stale_profile_source_and_generation() {
    let _serial = plx_base::testlock::serial();
    for replacement in ["profile", "source", "generation"] {
        let mut app = Box::new(completed_catalog_with_actions());
        app.ott.as_mut().unwrap().write_permit.revoke();
        terminal_catalog_action(&mut app, OttAction::ShowSettings);
        press_back(&mut app);
        let runtime = app.ott.as_mut().unwrap();
        assert_eq!(runtime.page, "providers");
        assert!(runtime.catalog_return.is_some());
        let cancelled = runtime.cancel.clone();
        match replacement {
            "profile" => runtime.saved.profiles.active_profile = Some("second".into()),
            "source" => runtime.saved.profiles.profiles[0].source = "https://media.invalid/category-replaced.m3u".into(),
            _ => runtime.generation = runtime.generation.wrapping_add(1),
        }
        let generation = runtime.generation;
        let entries = runtime.catalog.entries.clone();
        let saved = serde_json::to_value(&runtime.saved).unwrap();
        terminal_catalog_action(&mut app, OttAction::ShowCategories);
        let runtime = app.ott.as_ref().unwrap();
        assert_eq!(runtime.page, "categories");
        assert_eq!(runtime.generation, generation, "{replacement}");
        assert!(Arc::ptr_eq(&runtime.cancel, &cancelled), "{replacement}");
        assert!(cancelled.load(Ordering::Acquire));
        assert!(!runtime.catalog_complete && !runtime.catalog_loading);
        assert!(runtime.catalog_return.is_none() && runtime.catalog_next.is_none());
        assert_eq!(runtime.catalog.entries, entries);
        assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), saved);
        assert!(!runtime.saving && runtime.play_target.is_none() && runtime.resolver_next.is_none());
        assert!(runtime.playing.is_none());

        press_back(&mut app);
        let runtime = app.ott.as_ref().unwrap();
        assert_eq!(runtime.page, "channels");
        assert_eq!(runtime.generation, generation);
        assert!(runtime.raw_view().order_context.is_none(), "stale {replacement} cannot regain Actions");
        assert!(Arc::ptr_eq(&runtime.cancel, &cancelled));
        assert_eq!(runtime.catalog.entries, entries);
        assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), saved);
        assert!(runtime.catalog_next.is_none() && runtime.play_target.is_none() && runtime.resolver_next.is_none());
        assert!(!runtime.saving && runtime.playing.is_none());
    }
}
