//! Back crosses the actual screen, dispatcher and OTT runner. No platform Home
//! operation is performed: its ordinary loop request is observed at the boundary.
use super::*;
use plx_machine::machine::Key;
use plx_screens::registry::LoopReq;

fn mount_ott(app: &mut App, page: &'static str, pushed: bool) {
    // The synthetic Saved fixture is verified. Unloaded state deliberately
    // denies navigation through the independent kiosk/persistence fence.
    let runtime = app.ott.as_mut().unwrap();
    runtime.loaded = true;
    runtime.page = page;
    if pushed {
        bridge::nav_push(&mut app.pages, AppArg::Ott);
    } else {
        bridge::show_page(&mut app.pages, AppArg::Ott);
    }
    settle_pages(app);
    runtime_tick(app);
    settle_pages(app);
    assert!(matches!(app.route(), AppArg::Ott));
    assert!(!app.pages.has_pending_navigation());
}

fn press_back(app: &mut App) {
    let tick = next_tick(app);
    bridge::frame(
        &mut app.pages,
        &mut app.bridge,
        tick,
        bridge::script_key(Key::Back, tick),
    );
    runtime_tick(app);
    settle_pages(app);
}

#[test]
fn back_from_internal_ott_pages_returns_to_its_parent_without_leaving_ott() {
    let _serial = plx_base::testlock::serial();
    for (page, parent) in [
        ("profiles", "providers"),
        ("categories", "channels"),
        ("channels", "profiles"),
        ("favorites", "profiles"),
        ("settings", "providers"),
    ] {
        let mut app = app_fixture();
        mount_ott(&mut app, page, false);
        let ott = app.pages.top_page();

        press_back(&mut app);

        assert_eq!(app.ott.as_ref().unwrap().page, parent, "{page}");
        assert_eq!(app.pages.top_page(), ott, "{page} must remain inside OTT");
        assert!(matches!(app.route(), AppArg::Ott));
        assert!(!app
            .bridge
            .take_reqs()
            .iter()
            .any(|req| matches!(req, LoopReq::BackAtRoot)));
    }
}

#[test]
fn back_from_pushed_ott_providers_restores_the_existing_host_page() {
    let _serial = plx_base::testlock::serial();
    let mut app = app_fixture();
    bridge::show_page(&mut app.pages, AppArg::Home);
    settle_pages(&mut app);
    let home = app.pages.top_page();
    assert!(home.is_some());
    mount_ott(&mut app, "providers", true);
    assert_eq!(app.pages.nav.tabs.stack.depth(), 2);
    assert_ne!(app.pages.top_page(), home);

    press_back(&mut app);

    assert_eq!(
        app.pages.top_page(),
        home,
        "Back must reuse the previous host page and its focus memory"
    );
    assert!(matches!(app.route(), AppArg::Home));
    assert_eq!(app.pages.nav.tabs.stack.depth(), 1);
    assert!(!app
        .bridge
        .take_reqs()
        .iter()
        .any(|req| matches!(req, LoopReq::BackAtRoot)));
}

#[test]
fn back_from_root_ott_providers_requests_standard_root_back_once() {
    let _serial = plx_base::testlock::serial();
    let mut app = app_fixture();
    app.running = true;
    mount_ott(&mut app, "providers", false);
    let ott = app.pages.top_page();
    assert_eq!(app.pages.nav.tabs.stack.depth(), 1);

    press_back(&mut app);
    assert!(!app.bridge.take_reqs().iter().any(|request| matches!(request, LoopReq::BackAtRoot)));
    root_exit_action(&mut app, OttAction::ExitAnswer { token: 1, accept: true });

    assert_eq!(
        app.pages.top_page(),
        ott,
        "root Back must not replace the page"
    );
    assert!(
        app.running,
        "root Back is a platform Home request, not process termination"
    );
    let requests = app.bridge.take_reqs();
    assert_eq!(
        requests
            .iter()
            .filter(|req| matches!(req, LoopReq::BackAtRoot))
            .count(),
        1
    );
    runtime_tick(&mut app);
    settle_pages(&mut app);
    assert!(
        !app.bridge
            .take_reqs()
            .iter()
            .any(|req| matches!(req, LoopReq::BackAtRoot)),
        "an idle tick must not replay Back"
    );
}

#[test]
fn back_to_profiles_fences_an_earlier_save_reload_and_its_first_batch() {
    let _serial = plx_base::testlock::serial();
    let mut app = app_fixture();
    let profile = ProviderProfile {
        id: "back-save-fixture".into(),
        name: "Fixture folder".into(),
        kind: ProviderKind::Local,
        source: "/media/back-save-fixture".into(),
        source_alias: "Fixture folder".into(),
        enabled: true,
    };
    let saved = {
        let runtime = app.ott.as_mut().unwrap();
        runtime.saved.profiles.profiles = vec![profile.clone()];
        runtime.saved.profiles.active_profile = Some(profile.id);
        runtime.provider = ProviderKind::Local;
        // Represent an already-admitted save, and keep both worker boundaries
        // inert. Only their explicitly delivered replies run in this test.
        runtime.saving = true;
        runtime.save_playback_epoch = runtime.playback_epoch;
        runtime.catalog_done.store(false, Ordering::Release);
        runtime.resolver_busy = true;
        let mut saved = runtime.saved.clone();
        saved.category = "Fixture folder".into();
        saved
    };
    mount_ott(&mut app, "channels", false);

    press_back(&mut app);
    assert_eq!(app.ott.as_ref().unwrap().page, "profiles");
    assert!(app.ott.as_ref().unwrap().saving);

    app.ott
        .as_ref()
        .unwrap()
        .tx
        .send(Job::Saved(Ok(saved), None, true, json!({"status":"ok"})))
        .unwrap_or_else(|_| panic!("fixture saved-reply receiver disappeared"));
    runtime_tick(&mut app);
    let runtime = app.ott.as_ref().unwrap();
    assert!(!runtime.saving, "the durable save must still be accepted");
    assert_eq!(runtime.saved.category, "Fixture folder");
    runtime
        .catalog_tx
        .send(CatalogEvent::Batch(
            runtime.generation,
            Catalog { hierarchy: None,
                entries: vec![CatalogEntry {
                    id: "first".into(),
                    title: "first.mp4".into(),
                    category: "Fixture folder".into(),
                    description: String::new(),
                    poster: String::new(),
                    media: MediaLocator::Local {
                        relative_path: "first.mp4".into(),
                    },
                    duration_ms: Some(60_000),
                    provider_id: None,
                    provider_id_field: None,
                    live: None,
                }],
            },
        ))
        .unwrap_or_else(|_| panic!("fixture catalogue receiver disappeared"));
    runtime_tick(&mut app);
    settle_pages(&mut app);

    let runtime = app.ott.as_ref().unwrap();
    assert!(runtime
        .catalog
        .entries
        .iter()
        .any(|entry| entry.id == "first"));
    assert!(
        runtime.resolver_next.is_none() && runtime.play_target.is_none(),
        "the pre-Back save must not turn its first catalogue batch into new playback"
    );
    assert_eq!(
        runtime.page, "profiles",
        "a background refresh must not undo Back"
    );
    assert!(runtime.playing.is_none());
    assert!(matches!(app.route(), AppArg::Ott));
}

fn m3u_navigation_fixture() -> App {
    let mut app = app_fixture();
    let runtime = app.ott.as_mut().unwrap();
    runtime.loaded = true;
    runtime.catalog_done.store(false, Ordering::Release);
    runtime.metadata_done.store(false, Ordering::Release);
    runtime.resolver_busy = true;
    runtime.provider = ProviderKind::M3u;
    runtime.saved.profiles.profiles = ["home", "second"].map(|id| ProviderProfile {
        id: id.into(), name: id.into(), kind: ProviderKind::M3u,
        source: format!("https://media.invalid/{id}.m3u"),
        source_alias: String::new(), enabled: true,
    }).to_vec();
    runtime.saved.profiles.active_profile = Some("home".into());
    runtime.saved.category = "News".into();
    runtime.catalog.entries = vec![m3u_navigation_entry("one")];
    runtime.catalog_loading = true;
    runtime.catalog_complete = false;
    runtime.cancel = Arc::new(AtomicBool::new(false));
    runtime.selected = Some("one".into());
    runtime.continuity.stop(runtime.generation);
    app
}

fn m3u_navigation_entry(id: &str) -> CatalogEntry {
    let mut entry = synthetic_entry();
    entry.id = id.into();
    entry.title = id.into();
    entry.category = "News".into();
    entry.media = MediaLocator::Http { url: format!("https://media.invalid/{id}.mp4") };
    entry.provider_id = None;
    entry.provider_id_field = None;
    entry
}

#[test]
fn category_back_keeps_source_progress_and_accepts_the_next_catalog_batch() {
    let _serial = plx_base::testlock::serial();
    for favorites in [false, true] {
        let mut app = m3u_navigation_fixture();
        app.ott.as_mut().unwrap().saved.favorites_selected = favorites;
        mount_ott(&mut app, "categories", false);
        let runtime = app.ott.as_ref().unwrap();
        let saved = serde_json::to_value(&runtime.saved).unwrap();
        let generation = runtime.generation;
        let playback_epoch = runtime.playback_epoch;
        let cancel = runtime.cancel.clone();

        press_back(&mut app);

        let runtime = app.ott.as_ref().unwrap();
        assert_eq!(runtime.page, if favorites { "favorites" } else { "channels" });
        assert_eq!(runtime.generation, generation, "same catalogue remains authorized");
        assert_eq!(runtime.playback_epoch, playback_epoch, "same-source Back is not playback exit");
        assert!(!cancel.load(Ordering::Acquire));
        assert!(runtime.catalog_loading && !runtime.catalog_complete);
        assert_eq!(runtime.provider, ProviderKind::M3u);
        assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), saved,
            "Back must not rewrite categories, favorites or the permanent profile");
        runtime.catalog_tx.send(CatalogEvent::Batch(generation, Catalog { hierarchy: None,
            entries: vec![m3u_navigation_entry("two")],
        })).unwrap_or_else(|_| panic!("fixture catalogue receiver disappeared"));
        runtime_tick(&mut app);
        let runtime = app.ott.as_ref().unwrap();
        assert_eq!(runtime.catalog.entries.len(), 2, "the ongoing scan must still land");
        assert_eq!(runtime.selected.as_deref(), Some("one"));
        assert_eq!(runtime.page, if favorites { "favorites" } else { "channels" });
        assert!(runtime.resolver_next.is_none(), "navigation alone must not start playback");
    }
}

#[test]
fn category_back_preserves_an_accepted_transient_local_source() {
    let _serial = plx_base::testlock::serial();
    let (_root_dir, mut app, _root) = super::local_integration::setup();
    mount_ott(&mut app, "categories", false);
    let runtime = app.ott.as_ref().unwrap();
    let saved = serde_json::to_value(&runtime.saved).unwrap();
    let profile = runtime.effective_profile().cloned().unwrap();
    let generation = runtime.generation;
    let playback_epoch = runtime.playback_epoch;
    let transient::TransientMedia::Local(session) = runtime.transient.accepted.as_ref().unwrap() else {
        panic!("accepted local fixture");
    };
    let read = session.read.clone();
    let cancel = runtime.cancel.clone();

    press_back(&mut app);

    let runtime = app.ott.as_ref().unwrap();
    assert_eq!(runtime.page, "channels");
    assert_eq!(runtime.effective_profile(), Some(&profile));
    assert_eq!(runtime.generation, generation);
    assert_eq!(runtime.playback_epoch, playback_epoch);
    assert!(!cancel.load(Ordering::Acquire));
    assert!(runtime.catalog_loading && !runtime.catalog_complete);
    assert!(runtime.local_source_read_is_current(&read), "Back must retain the live source grant");
    let transient::TransientMedia::Local(session) = runtime.transient.accepted.as_ref().unwrap() else {
        panic!("same accepted local source must remain");
    };
    assert!(session.read.same_grant(&read));
    assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), saved);
}

#[test]
fn channel_back_keeps_m3u_profiles_and_then_returns_to_the_same_provider() {
    let _serial = plx_base::testlock::serial();
    let mut app = m3u_navigation_fixture();
    mount_ott(&mut app, "channels", false);
    let playback_epoch = app.ott.as_ref().unwrap().playback_epoch;
    let cancel = app.ott.as_ref().unwrap().cancel.clone();

    press_back(&mut app);

    let runtime = app.ott.as_ref().unwrap();
    assert_eq!(runtime.page, "profiles");
    assert_eq!(runtime.provider, ProviderKind::M3u);
    assert_ne!(runtime.playback_epoch, playback_epoch, "leaving the source still fences late playback");
    assert!(cancel.load(Ordering::Acquire));
    let profiles = runtime.raw_view();
    assert_eq!(profiles.page_id, "profiles:m3u");
    assert_eq!(profiles.selected_row.as_deref(), Some("home"));
    assert_eq!(profiles.rows.iter().filter_map(|row| match &row.action {
        OttAction::SelectProfile(id) => Some(id.as_str()), _ => None,
    }).collect::<Vec<_>>(), vec!["home", "second"]);

    press_back(&mut app);

    let runtime = app.ott.as_ref().unwrap();
    assert_eq!(runtime.page, "providers");
    assert_eq!(runtime.raw_view().selected_row.as_deref(), Some("m3u"));
}

#[test]
fn channel_list_return_after_back_restarts_an_unfinished_m3u_catalog_without_playing() {
    let _serial = plx_base::testlock::serial();
    for had_first_batch in [false, true] {
        let mut app = m3u_navigation_fixture();
        if !had_first_batch {
            app.ott.as_mut().unwrap().catalog.entries.clear();
        }
        mount_ott(&mut app, "channels", false);
        let retired_generation = app.ott.as_ref().unwrap().generation;
        let old_cancel = app.ott.as_ref().unwrap().cancel.clone();
        let before = serde_json::to_value(&app.ott.as_ref().unwrap().saved).unwrap();

        press_back(&mut app);
        let runtime = app.ott.as_ref().unwrap();
        assert_eq!(runtime.page, "profiles");
        assert!(old_cancel.load(Ordering::Acquire));
        assert_ne!(runtime.generation, retired_generation);
        assert!(!runtime.catalog_loading && !runtime.catalog_complete);
        assert!(runtime.catalog_next.is_none());
        // The cancelled worker cannot repair the incomplete list on a later tick.
        runtime.catalog_tx.send(CatalogEvent::Batch(retired_generation, Catalog {
            hierarchy: None, entries: vec![m3u_navigation_entry("retired")],
        })).unwrap();
        runtime.catalog_tx.send(CatalogEvent::Finished(retired_generation, Ok(()))).unwrap();
        runtime_tick(&mut app);
        runtime_tick(&mut app);
        assert_eq!(app.ott.as_ref().unwrap().catalog.entries.len(), if had_first_batch { 1 } else { 0 });
        assert!(app.ott.as_ref().unwrap().catalog_next.is_none());

        // Browse -> Channel list is offered on the Profiles page by the real
        // navigation generator. Deliver that existing action through Bridge.
        let reply = app.pages.top_page().expect("mounted OTT instance");
        app.pages.emit(MachineId::Instance(reply), Fx::App(AppFx::Ott(OttOut::Action {
            reply, action: OttAction::ShowCatalog,
        })));
        let tick = next_tick(&mut app);
        bridge::frame(&mut app.pages, &mut app.bridge, tick, Vec::new());
        runtime_tick(&mut app);
        settle_pages(&mut app);

        let runtime = app.ott.as_ref().unwrap();
        assert_eq!(runtime.page, "channels");
        assert_eq!(runtime.provider, ProviderKind::M3u);
        assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), before);
        assert!(!runtime.saving && runtime.play_target.is_none() && runtime.resolver_next.is_none());
        // The existing fixture holds the worker slot: a replacement load must
        // be visible as queued work, with no actual source or decoder IO.
        assert!(runtime.catalog_loading,
            "Channel list must restart its retired unfinished catalog; first_batch={had_first_batch}");
        assert_eq!(runtime.catalog_next.as_ref().map(|p| p.id.as_str()), Some("home"));
        assert!(!runtime.cancel.load(Ordering::Acquire));
        let current_generation = runtime.generation;
        assert_ne!(current_generation, retired_generation);
        runtime.catalog_tx.send(CatalogEvent::Batch(current_generation, Catalog {
            hierarchy: None, entries: vec![m3u_navigation_entry("fresh")],
        })).unwrap();
        runtime.catalog_tx.send(CatalogEvent::Finished(current_generation, Ok(()))).unwrap();
        runtime_tick(&mut app);
        let runtime = app.ott.as_ref().unwrap();
        assert!(runtime.catalog_complete);
        assert!(runtime.catalog.entries.iter().any(|entry| entry.id == "fresh"));
        assert!(!runtime.catalog.entries.iter().any(|entry| entry.id == "retired"));
        assert!(runtime.play_target.is_none() && runtime.resolver_next.is_none(),
            "returning to Channel list is browsing, not a playback command");
    }
}

fn show_channel_list(app: &mut App) {
    let reply = app.pages.top_page().expect("mounted OTT instance");
    app.pages.emit(MachineId::Instance(reply), Fx::App(AppFx::Ott(OttOut::Action {
        reply, action: OttAction::ShowCatalog,
    })));
    let tick = next_tick(app);
    bridge::frame(&mut app.pages, &mut app.bridge, tick, Vec::new());
    runtime_tick(app);
    settle_pages(app);
}

#[test]
fn channel_list_return_keeps_live_and_completed_m3u_catalogs() {
    let _serial = plx_base::testlock::serial();
    for complete in [false, true] {
        let mut app = m3u_navigation_fixture();
        app.ott.as_mut().unwrap().catalog_loading = !complete;
        app.ott.as_mut().unwrap().catalog_complete = complete;
        mount_ott(&mut app, "channels", false);
        if complete { press_back(&mut app); }
        let generation = app.ott.as_ref().unwrap().generation;
        let cancel = app.ott.as_ref().unwrap().cancel.clone();
        show_channel_list(&mut app);
        let runtime = app.ott.as_ref().unwrap();
        assert_eq!(runtime.page, "channels");
        assert_eq!(runtime.generation, generation, "return must not duplicate existing data/work");
        if complete {
            assert!(cancel.load(Ordering::Acquire), "Back keeps the old producer retired");
            assert!(!Arc::ptr_eq(&runtime.cancel, &cancel), "retained data gets fresh action authority");
            assert!(!runtime.cancel.load(Ordering::Acquire));
        } else {
            assert!(Arc::ptr_eq(&runtime.cancel, &cancel), "live work retains its owner");
            assert!(!cancel.load(Ordering::Acquire));
        }
        assert!(runtime.catalog_next.is_none());
        assert_eq!(runtime.catalog_loading, !complete);
        assert_eq!(runtime.catalog.entries.len(), 1);
        assert!(runtime.play_target.is_none() && runtime.resolver_next.is_none());
        assert!(!runtime.saving);
    }
}

#[test]
fn interrupted_m3u_return_survives_profile_back_but_not_profile_or_source_replacement() {
    let _serial = plx_base::testlock::serial();
    for replacement in ["none", "source", "profile"] {
        let mut app = m3u_navigation_fixture();
        mount_ott(&mut app, "channels", false);
        press_back(&mut app);
        press_back(&mut app);
        assert_eq!(app.ott.as_ref().unwrap().page, "providers");
        if replacement == "source" {
            // Same stable profile ID, different source: the retired scan must
            // not lend an implicit reload to this replacement descriptor.
            app.ott.as_mut().unwrap().saved.profiles.profiles[0].source =
                "https://media.invalid/replaced.m3u".into();
        } else if replacement == "profile" {
            app.ott.as_mut().unwrap().saved.profiles.active_profile = Some("second".into());
        }
        let generation = app.ott.as_ref().unwrap().generation;
        show_channel_list(&mut app);
        let runtime = app.ott.as_ref().unwrap();
        if replacement != "none" {
            assert_eq!(runtime.generation, generation);
            assert!(!runtime.catalog_loading && runtime.catalog_next.is_none());
        } else {
            assert!(runtime.catalog_loading);
            assert_eq!(runtime.catalog_next.as_ref().map(|p| p.id.as_str()), Some("home"));
            assert_ne!(runtime.generation, generation);
        }
        assert!(runtime.play_target.is_none() && runtime.resolver_next.is_none());
        assert!(!runtime.saving);
    }
}


#[test]
fn root_back_confirmation_open_preserves_navigation_and_source_work() {
    let _serial = plx_base::testlock::serial();
    let mut app = m3u_navigation_fixture();
    mount_ott(&mut app, "providers", false);
    let mut runtime = app.ott.take().unwrap();
    runtime.reply = app.pages.top_page();
    runtime.catalog_next = runtime.active().cloned();
    runtime.continuity = continuity::Continuity::new(runtime.generation,
        Some(continuity::ResumePoint { entry_id: "one".into(), position_ms: 12_000 }));
    let before = (runtime.navigation_epoch, runtime.playback_epoch, runtime.generation,
        runtime.play_generation, runtime.catalog.clone(), runtime.catalog_next.clone(),
        runtime.continuity.pending_restore().cloned(), serde_json::to_value(&runtime.saved).unwrap());
    let cancel = runtime.cancel.clone();
    let play_cancel = runtime.play_cancel.clone();
    apply_ui_action(&mut runtime, &mut app, OttAction::Back, true, false, false);
    assert_eq!(runtime.navigation_epoch, before.0, "opening a question is not navigation");
    assert_eq!(runtime.playback_epoch, before.1);
    assert_eq!(runtime.generation, before.2);
    assert_eq!(runtime.play_generation, before.3);
    assert_eq!(runtime.catalog, before.4);
    assert_eq!(runtime.catalog_next, before.5);
    assert_eq!(runtime.continuity.pending_restore(), before.6.as_ref());
    assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), before.7);
    assert!(!cancel.load(Ordering::Acquire));
    assert!(!play_cancel.load(Ordering::Acquire));
    app.ott = Some(runtime);
    settle_pages(&mut app);
    assert!(!app.bridge.take_reqs().iter().any(|request| matches!(request, LoopReq::BackAtRoot)),
        "no Home request before an explicit affirmative answer");
}


fn root_exit_action(app: &mut App, action: OttAction) {
    let mut runtime = app.ott.take().unwrap();
    apply_ui_action(&mut runtime, app, action, true, false, false);
    app.ott = Some(runtime);
    settle_pages(app);
}

#[test]
fn root_exit_cancel_and_wrong_token_preserve_the_current_question() {
    let _serial = plx_base::testlock::serial();
    let mut app = m3u_navigation_fixture();
    mount_ott(&mut app, "providers", false);
    let before = {
        let r = app.ott.as_ref().unwrap();
        (r.navigation_epoch, r.playback_epoch, r.generation, r.catalog.clone(), r.catalog_next.clone(),
            serde_json::to_value(&r.saved).unwrap())
    };
    root_exit_action(&mut app, OttAction::Back);
    root_exit_action(&mut app, OttAction::ExitAnswer { token: 1, accept: false });
    let r = app.ott.as_ref().unwrap();
    assert_eq!((r.navigation_epoch, r.playback_epoch, r.generation), (before.0, before.1, before.2));
    assert_eq!(r.catalog, before.3);
    assert_eq!(r.catalog_next, before.4);
    assert_eq!(serde_json::to_value(&r.saved).unwrap(), before.5);
    assert!(app.bridge.take_reqs().is_empty());
    root_exit_action(&mut app, OttAction::Back);
    root_exit_action(&mut app, OttAction::ExitAnswer { token: 1, accept: true });
    assert!(app.bridge.take_reqs().is_empty(), "old answer must not consume the new question");
    root_exit_action(&mut app, OttAction::ExitAnswer { token: 2, accept: true });
    assert_eq!(app.ott.as_ref().unwrap().navigation_epoch, before.0 + 1);
    assert_eq!(app.bridge.take_reqs().iter().filter(|r| matches!(r, LoopReq::BackAtRoot)).count(), 1);
    root_exit_action(&mut app, OttAction::ExitAnswer { token: 2, accept: true });
    assert!(app.bridge.take_reqs().is_empty(), "confirmation is one-shot");
    assert_eq!(app.ott.as_ref().unwrap().navigation_epoch, before.0 + 1);
}

#[test]
fn root_exit_answer_rejects_changed_owner_mode_source_root_and_policy() {
    let _serial = plx_base::testlock::serial();
    for changed in 0..7 {
        let mut app = m3u_navigation_fixture();
        mount_ott(&mut app, "providers", false);
        root_exit_action(&mut app, OttAction::Back);
        match changed {
            0 => {
                let old_owner = app.pages.top_page();
                // Root(Ott) over the same sole root is deliberately inert; Replace mints an owner.
                app.pages.request(MachineId::Nav, plx_machine::machine::NavOp::Replace(AppArg::Ott));
                settle_pages(&mut app);
                assert_ne!(app.pages.top_page(), old_owner, "the stale-answer case needs a new instance");
            },
            1 => {
                app.bridge.apply_ui_mode(plx_machine::machine::UiMode::Classic);
                app.ott.as_mut().unwrap().saved.settings.insert("interfaceMode".into(), json!(0));
            },
            2 => app.ott.as_mut().unwrap().generation += 1,
            3 => app.ott.as_mut().unwrap().navigation_epoch += 1,
            4 => app.ott.as_mut().unwrap().page = "profiles",
            5 => bridge::open_settings(&mut app.pages),
            _ => app.ott.as_mut().unwrap().loaded = false,
        }
        let epoch = app.ott.as_ref().unwrap().navigation_epoch;
        root_exit_action(&mut app, OttAction::ExitAnswer { token: 1, accept: true });
        assert!(!app.bridge.take_reqs().iter().any(|r| matches!(r, LoopReq::BackAtRoot)), "case {changed}");
        assert_eq!(app.ott.as_ref().unwrap().navigation_epoch, epoch, "case {changed}");
    }
}

#[test]
fn root_exit_pending_surface_does_not_open_or_accept_navigation() {
    let _serial = plx_base::testlock::serial();
    let mut app = m3u_navigation_fixture();
    mount_ott(&mut app, "providers", false);
    let mut runtime = app.ott.take().unwrap();
    runtime.reply = app.pages.top_page();
    let before = (runtime.navigation_epoch, runtime.generation, runtime.catalog.clone());
    bridge::open_settings(&mut app.pages);
    assert!(app.pages.has_pending_surface_navigation());
    apply_ui_action(&mut runtime, &mut app, OttAction::Back, true, false, false);
    assert_eq!(runtime.navigation_epoch, before.0);
    assert_eq!(runtime.generation, before.1);
    assert_eq!(runtime.catalog, before.2);
    assert!(app.pages.has_pending_surface_navigation());
    app.ott = Some(runtime);
    settle_pages(&mut app);
    assert!(!app.bridge.take_reqs().iter().any(|r| matches!(r, LoopReq::BackAtRoot)));
}

include!("catalog_terminal_return.rs");
include!("catalog_return_identity.rs");
include!("catalog_return_lifecycle.rs");
