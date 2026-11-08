// Included by navigation.rs: exercise its real Bridge actions and lifecycle
// entry points. Worker results are delivered through the existing typed inbox.

fn lifecycle_return_batch(id: &str) -> Catalog {
    plx_data::ott::parse_vportal_response(
        &json!({"type":"videoportal","items":[{
            "type":"stream","id":id,"title":id,"request":{"id":id}
        }]}).to_string(),
        &synthetic_profile(),
        "",
    ).expect("valid fixture portal response").catalog
}

fn lifecycle_return_app() -> Box<App> {
    let mut app = Box::new(terminal_catalog_return_app());
    let runtime = app.ott.as_mut().unwrap();
    let mut other = synthetic_profile();
    other.id = "other-portal".into();
    other.source = "portal::[key:other]https://media.invalid/other/api".into();
    runtime.provider = ProviderKind::VPortal;
    runtime.saved.profiles.profiles = vec![synthetic_profile(), other];
    runtime.saved.profiles.active_profile = Some(synthetic_profile().id);
    runtime.saved.category.clear();
    runtime.catalog = Catalog::default();
    runtime.selected = None;
    runtime.continuity.stop(runtime.generation);
    mount_ott(&mut app, "channels", false);
    let runtime = app.ott.as_ref().unwrap();
    runtime.catalog_tx.send(CatalogEvent::Batch(runtime.generation,
        lifecycle_return_batch("partial"))).unwrap();
    runtime_tick(&mut app);
    let runtime = app.ott.as_ref().unwrap();
    assert!(runtime.catalog_loading && !runtime.catalog_complete);
    assert_eq!(runtime.catalog.entries.len(), 1);
    assert!(!runtime.catalog_done.load(Ordering::Acquire));
    assert!(runtime.catalog_next.is_none() && runtime.catalog_return.is_none());
    assert!(!runtime.saving);
    assert_eq!(app.bridge.ui_mode(), plx_machine::machine::UiMode::Classic);
    lifecycle_return_no_play(&app);
    app
}

fn lifecycle_return_no_play(app: &App) {
    let runtime = app.ott.as_ref().unwrap();
    assert!(runtime.play_target.is_none() && runtime.resolver_next.is_none());
    assert!(runtime.playing.is_none() && runtime.resume_pending.is_none());
    assert!(runtime.pending_play.is_none());
    assert!(!plx_media::route::has_url(&app.player.session));
}

fn lifecycle_return_sleep_wake(app: &mut App) {
    let generation = app.ott.as_ref().unwrap().generation;
    assert!(app.ott.as_ref().unwrap().source_work_allowed());
    super::super::suspend(app);
    crate::app::ott_standby::set(app, true);
    let runtime = app.ott.as_ref().unwrap();
    assert!(!runtime.source_work_allowed());
    assert_ne!(runtime.generation, generation);
    assert!(!runtime.catalog_done.load(Ordering::Acquire),
        "suspension cannot retire the worker on its behalf");
    crate::app::ott_standby::set(app, false);
    super::super::resume(app);
    let runtime = app.ott.as_ref().unwrap();
    assert!(runtime.source_work_allowed());
    assert!(!runtime.catalog_work_allowed(), "wake still awaits the old worker");
}

#[test]
fn interrupted_vportal_return_survives_lifecycle_and_reloads_without_playing() {
    let _serial = plx_base::testlock::serial();
    for cycles in [1, 2] {
        let mut app = lifecycle_return_app();
        let runtime = app.ott.as_ref().unwrap();
        let old_generation = runtime.generation;
        let partial = runtime.catalog.clone();
        let saved = serde_json::to_value(&runtime.saved).unwrap();
        let source = runtime.effective_profile().unwrap().clone();
        let old_cancel = runtime.cancel.clone();
        press_back(&mut app);
        let runtime = app.ott.as_ref().unwrap();
        assert_eq!(runtime.page, "profiles");
        assert!(runtime.catalog_return.is_some());
        assert!(old_cancel.load(Ordering::Acquire));
        assert!(!runtime.catalog_loading && !runtime.catalog_complete);
        assert!(runtime.catalog_next.is_none());
        for _ in 0..cycles { lifecycle_return_sleep_wake(&mut app); }

        let runtime = app.ott.as_ref().unwrap();
        runtime.catalog_tx.send(CatalogEvent::Batch(old_generation,
            lifecycle_return_batch("retired"))).unwrap();
        runtime.catalog_tx.send(CatalogEvent::Finished(old_generation, Ok(()))).unwrap();
        runtime_tick(&mut app);
        let runtime = app.ott.as_ref().unwrap();
        assert_eq!(runtime.catalog, partial, "old scan cannot repair the stopped list");
        assert!(!runtime.catalog_loading && !runtime.catalog_complete);
        assert!(runtime.catalog_next.is_none());
        let before_return = runtime.generation;
        let play_generation = runtime.play_generation;

        show_channel_list(&mut app);

        let runtime = app.ott.as_ref().unwrap();
        assert_eq!(runtime.page, "channels");
        // Causal RED on 175779f8: ShowCatalog takes the old-generation marker
        // and rejects it, leaving catalog_loading false and no queued source.
        assert!(runtime.catalog_loading,
            "Back's unfinished source must remain resumable after {cycles} lifecycle cycles");
        assert_eq!(runtime.catalog_next.as_ref(), Some(&source));
        assert_ne!(runtime.generation, before_return);
        assert!(runtime.catalog_return.is_none());
        assert!(!runtime.catalog_work_allowed());
        assert_eq!(runtime.play_generation, play_generation);
        lifecycle_return_no_play(&app);

        // Exercise actual retirement/rearm and task admission without running
        // the task's IO body. This is the same work a real worker would receive.
        let mut runtime = app.ott.take().unwrap();
        runtime.rearm_source_work(&mut app);
        assert!(!runtime.catalog_work_allowed());
        runtime.catalog_done.store(true, Ordering::Release);
        runtime.rearm_source_work(&mut app);
        assert!(runtime.catalog_work_allowed());
        assert_eq!(runtime.catalog_next.as_ref(), Some(&source));
        assert!(!runtime.cancel.load(Ordering::Acquire));
        match runtime.take_catalog_task_checked(Some(&app)) {
            Some(ott_catalog::Task::Provider(profile)) => assert_eq!(profile, source),
            _ => panic!("returned source must admit exactly its provider task"),
        }
        assert!(runtime.catalog_next.is_none());
        let fresh = lifecycle_return_batch("fresh");
        runtime.catalog_tx.send(CatalogEvent::Batch(runtime.generation, fresh.clone())).unwrap();
        runtime.catalog_tx.send(CatalogEvent::Finished(runtime.generation, Ok(()))).unwrap();
        app.ott = Some(runtime);
        runtime_tick(&mut app);
        let runtime = app.ott.as_ref().unwrap();
        assert!(runtime.catalog_complete && !runtime.catalog_loading);
        assert_eq!(runtime.catalog, fresh);
        assert_eq!(runtime.play_generation, play_generation,
            "fresh completion must not turn a browse return into playback");
        assert!(!runtime.saving);
        assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), saved);
        lifecycle_return_no_play(&app);
    }
}

#[test]
fn catalog_return_lifecycle_does_not_reauthorize_a_changed_source_or_generation() {
    let _serial = plx_base::testlock::serial();
    for change in ["profile", "source", "generation", "disabled"] {
        let mut app = lifecycle_return_app();
        press_back(&mut app);
        let runtime = app.ott.as_mut().unwrap();
        assert!(runtime.catalog_return.is_some());
        let profiles = runtime.saved.profiles.clone();
        match change {
            "profile" => runtime.saved.profiles.active_profile = Some("other-portal".into()),
            "source" => runtime.saved.profiles.profiles[0].source =
                "portal::[key:replaced]https://media.invalid/replaced/api".into(),
            "generation" => runtime.generation = runtime.generation.wrapping_add(1),
            _ => runtime.saved.profiles.profiles[0].enabled = false,
        }

        lifecycle_return_sleep_wake(&mut app);

        let runtime = app.ott.as_mut().unwrap();
        // Restore the descriptor after the real lifecycle edge. A blind
        // generation rebase would now lend old intent to this restored source.
        runtime.saved.profiles = profiles;
        runtime.status = "Current source outcome".into();
        let generation = runtime.generation;
        let saved = serde_json::to_value(&runtime.saved).unwrap();
        let retained = runtime.catalog.clone();
        show_channel_list(&mut app);
        let mut runtime = app.ott.take().unwrap();
        runtime.catalog_done.store(true, Ordering::Release);
        runtime.rearm_source_work(&mut app);
        assert!(runtime.catalog_work_allowed());
        assert_eq!(runtime.generation, generation, "{change}");
        assert_eq!(runtime.status, "Current source outcome", "{change}");
        assert!(runtime.catalog_return.is_none() && runtime.catalog_next.is_none(), "{change}");
        assert!(!runtime.catalog_loading && !runtime.catalog_complete, "{change}");
        assert_eq!(runtime.catalog, retained, "{change}");
        assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), saved);
        assert!(!runtime.saving);
        app.ott = Some(runtime);
        lifecycle_return_no_play(&app);
    }
}

#[test]
fn terminal_vportal_outcome_survives_lifecycle_without_creating_reload_intent() {
    let _serial = plx_base::testlock::serial();
    for completed in [false, true] {
        for backed_out in [false, true] {
            let mut app = lifecycle_return_app();
            let runtime = app.ott.as_ref().unwrap();
            runtime.catalog_tx.send(CatalogEvent::Finished(runtime.generation,
                if completed { Ok(()) } else { Err("Fixture source unavailable") })).unwrap();
            runtime_tick(&mut app);
            let runtime = app.ott.as_ref().unwrap();
            let status = runtime.status.clone();
            let rows = runtime.catalog.clone();
            let saved = serde_json::to_value(&runtime.saved).unwrap();
            assert_eq!(runtime.catalog_complete, completed);
            assert!(!runtime.catalog_loading);
            if backed_out { press_back(&mut app); }
            assert_eq!(app.ott.as_ref().unwrap().catalog_return.is_some(), backed_out);
            lifecycle_return_sleep_wake(&mut app);
            if !backed_out {
                assert!(app.ott.as_ref().unwrap().catalog_return.is_none(),
                    "lifecycle alone must not manufacture Back intent");
            }
            let generation = app.ott.as_ref().unwrap().generation;
            show_channel_list(&mut app);
            let mut runtime = app.ott.take().unwrap();
            runtime.catalog_done.store(true, Ordering::Release);
            runtime.rearm_source_work(&mut app);
            assert!(runtime.catalog_work_allowed());
            assert_eq!(runtime.generation, generation);
            assert_eq!(runtime.catalog_complete, completed, "{completed}/{backed_out}");
            assert_eq!(runtime.status, status, "{completed}/{backed_out}");
            assert_eq!(runtime.catalog, rows);
            assert!(!runtime.catalog_loading);
            assert!(runtime.catalog_return.is_none() && runtime.catalog_next.is_none());
            assert!(!runtime.saving);
            assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), saved);
            app.ott = Some(runtime);
            lifecycle_return_no_play(&app);
        }
    }
}
