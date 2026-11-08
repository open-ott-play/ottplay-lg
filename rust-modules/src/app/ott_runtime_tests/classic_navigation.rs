//! Existing catalogue and navigation owners; only synthetic, already-admitted state.
//! No source load, credential IO, renderer or decoder is started by these fixtures.
use super::*;
use plx_machine::machine::UiMode;
use plx_screens::registry::ContentArg;

struct Registry;
impl Registry {
    fn new() -> Self {
        plx_plex::plex::reset_servers_for_test();
        plx_plex::plex::grant::reset_for_test();
        Self
    }
}
impl Drop for Registry {
    fn drop(&mut self) {
        plx_plex::plex::reset_servers_for_test();
        plx_plex::plex::grant::reset_for_test();
    }
}

fn ready() -> App {
    let origin = plx_plex::plex::Origin::parse("https://classic-fixture.invalid:443").unwrap();
    let sid = plx_plex::plex::register_pinned_with_client_id(
        "classic-fixture-machine", &origin, "synthetic-token", None,
        "synthetic-client", Default::default());
    assert!(plx_plex::plex::set_current(sid));
    let mut app = app_fixture();
    let mut init = plx_session::auth::SessionInit::captured(Default::default());
    init.epoch = 10;
    init.phase = plx_session::auth::Phase::Ready;
    app.bridge = bridge::Bridge::for_session_test(init);
    assert!(app.bridge.session_ready_current());
    app.bridge.apply_ui_mode(UiMode::Classic);
    let runtime = app.ott.as_mut().unwrap();
    runtime.loaded = true;
    runtime.entropy = Some([1; 16]);
    runtime.public_preferences.accept(None);
    runtime.saved.settings.insert("interfaceMode".into(), json!(0));
    let profile = ProviderProfile { id: "plex-fixture".into(), name: "Fixture".into(),
        kind: ProviderKind::Plex, source: "classic-fixture-machine".into(),
        source_alias: String::new(), enabled: true };
    runtime.saved.profiles.active_profile = Some(profile.id.clone());
    runtime.saved.profiles.profiles.push(profile);
    let mut entry = synthetic_entry();
    entry.media = MediaLocator::Plex { server_id: "classic-fixture-machine".into(),
        rating_key: "42".into() };
    runtime.catalog.entries.push(entry);
    runtime.catalog_complete = true;
    runtime.catalog_done.store(false, Ordering::Release);
    runtime.metadata_done.store(false, Ordering::Release);
    runtime.resolver_busy = true;
    runtime.plex_refresh_pending = true;
    runtime.provider = ProviderKind::Plex;
    runtime.page = "channels";
    runtime.selected = Some("late-fixture".into());
    app
}

fn mount(app: &mut App, route: AppArg) {
    bridge::nav_root(&mut app.pages, route);
    settle_pages(app);
    assert!(!app.pages.has_pending_navigation());
}

#[test]
fn classic_catalogue_projection_retains_native_stack_and_saved_source() {
    let _serial = plx_base::testlock::serial();
    let _registry = Registry::new();
    let mut app = ready();
    mount(&mut app, AppArg::Home);
    let home = app.pages.top_page().unwrap();
    bridge::nav_push(&mut app.pages, AppArg::Library);
    settle_pages(&mut app);
    let before = serde_json::to_value(&app.ott.as_ref().unwrap().saved).unwrap();
    let generation = app.ott.as_ref().unwrap().generation;
    let playback = plx_media::route::playback_identity(&app.player.session);
    assert!(reconcile_classic_navigation(&mut app));
    assert!(!reconcile_classic_navigation(&mut app), "one pending replacement only");
    settle_pages(&mut app);
    assert!(app.route() == AppArg::Ott);
    assert_eq!(app.pages.nav.tabs.stack.depth(), 3);
    assert_eq!(app.pages.nav.tabs.stack.entries[0].inst.as_ref().unwrap().id, home);
    let runtime = app.ott.as_ref().unwrap();
    assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), before);
    assert_ne!(runtime.generation, generation, "first binding queries this session afresh");
    assert_eq!(runtime.selected.as_deref(), Some("late-fixture"));
    assert_eq!(runtime.page, "channels");
    assert!(runtime.catalog_next.is_some() && !runtime.saving);
    assert_eq!(plx_media::route::playback_identity(&app.player.session), playback);
}

#[test]
fn classic_mode_change_replaces_native_detail_and_collection_without_restarting_media() {
    let _serial = plx_base::testlock::serial();
    let _registry = Registry::new();
    for collection in [false, true] {
        let mut app = ready();
        let sid = plx_plex::plex::current_server();
        let route = if collection {
            AppArg::Content(ContentArg::Collection(plx_plex::plex::collections::CollectionRef::by_rk(sid, "43", 1, "Fixture")))
        } else { AppArg::Content(ContentArg::Detail { sid, rk: "42".into() }) };
        app.bridge.apply_ui_mode(UiMode::Modern);
        mount(&mut app, route.clone());
        assert!(!reconcile_classic_navigation(&mut app));
        app.bridge.apply_ui_mode(UiMode::Classic);
        let before = plx_media::route::playback_identity(&app.player.session);
        assert!(reconcile_classic_navigation(&mut app));
        settle_pages(&mut app);
        assert!(app.route() == AppArg::Ott);
        assert_eq!(plx_media::route::playback_identity(&app.player.session), before);
        assert!(app.ott.as_ref().unwrap().play_target.is_none());
    }
}

#[test]
fn classic_catalogue_waits_for_state_session_and_navigation_authority() {
    let _serial = plx_base::testlock::serial();
    let _registry = Registry::new();
    for gate in ["public-load", "protected-load", "uncertain", "saving", "session", "standby", "suspended"] {
        let mut app = ready();
        mount(&mut app, AppArg::Home);
        match gate {
            "public-load" => app.ott.as_mut().unwrap().public_preferences = Default::default(),
            "protected-load" => app.ott.as_mut().unwrap().loaded = false,
            "uncertain" => app.ott.as_mut().unwrap().persistence_uncertain = true,
            "saving" => app.ott.as_mut().unwrap().saving = true,
            "session" => {
                let mut init = plx_session::auth::SessionInit::captured(Default::default());
                init.phase = plx_session::auth::Phase::Profiles;
                app.bridge = bridge::Bridge::for_session_test(init);
                app.bridge.apply_ui_mode(UiMode::Classic);
            }
            "standby" => app.ott_standby = true,
            "suspended" => app.pages.nav.suspended = true,
            _ => unreachable!(),
        }
        let before = app.pages.top_page();
        assert!(!reconcile_classic_navigation(&mut app), "{gate}");
        assert_eq!(app.pages.top_page(), before);
        assert!(!app.pages.has_pending_navigation(), "{gate}");
    }
}

#[test]
fn classic_catalogue_leaves_modal_pending_auth_and_player_routes_owned() {
    let _serial = plx_base::testlock::serial();
    let _registry = Registry::new();
    for route in [AppArg::Login, AppArg::Profiles, AppArg::Onboard, AppArg::Player] {
        let mut app = ready();
        mount(&mut app, route.clone());
        assert!(!reconcile_classic_navigation(&mut app));
        assert!(app.route() == route);
    }
    let mut app = ready();
    mount(&mut app, AppArg::Home);
    bridge::open_settings(&mut app.pages);
    assert!(!reconcile_classic_navigation(&mut app), "pending presentation stays owned");
    settle_pages(&mut app);
    assert!(app.pages.surface_up());
    assert!(!reconcile_classic_navigation(&mut app), "an open modal stays owned");
    assert!(app.route() == AppArg::Home);
}

#[test]
fn delayed_public_classic_preference_projects_an_already_mounted_home() {
    let _serial = plx_base::testlock::serial();
    let _registry = Registry::new();
    let mut app = ready();
    app.bridge.apply_ui_mode(UiMode::Modern);
    {
        let runtime = app.ott.as_mut().unwrap();
        runtime.public_preferences = Default::default();
        runtime.saved.settings.insert("interfaceMode".into(), json!(1));
        // Hold the existing worker slots: only the supplied preference result lands.
        runtime.catalog_done.store(false, Ordering::Release);
        runtime.metadata_done.store(false, Ordering::Release);
        runtime.resolver_busy = true;
        runtime.plex_refresh_pending = true;
        runtime.continuity.stop(runtime.generation);
    }
    mount(&mut app, AppArg::Home);
    assert!(!reconcile_classic_navigation(&mut app));
    let snapshot = public_preferences::Snapshot {
        value: plx_platform::storage::public_preferences::Preferences {
            interface_mode: Some(0), ..Default::default()
        }, revision: 1, epoch: 1,
    };
    app.ott.as_ref().unwrap().tx.send(Job::PublicLoaded(Ok(Some(snapshot))))
        .unwrap_or_else(|_| panic!("fixture receiver disappeared"));
    runtime_tick(&mut app);
    assert_eq!(app.bridge.ui_mode(), UiMode::Classic);
    assert!(app.route() == AppArg::Home, "preference publication does not reset the stack");
    assert!(reconcile_classic_navigation(&mut app));
    settle_pages(&mut app);
    assert!(app.route() == AppArg::Ott);
    assert_eq!(app.ott.as_ref().unwrap().saved.profiles.active_profile.as_deref(), Some("plex-fixture"));
    assert!(app.ott.as_ref().unwrap().play_target.is_none());
}

#[test]
fn classic_unavailable_or_refused_source_shows_providers_without_changing_retained_state() {
    let _serial = plx_base::testlock::serial();
    let _registry = Registry::new();
    for refused in [false, true] {
        let mut app = ready();
        mount(&mut app, AppArg::Home);
        // Exercise an explicit native source request, not saved-provider boot.
        bridge::nav_push(&mut app.pages, AppArg::Library);
        settle_pages(&mut app);
        let runtime = app.ott.as_mut().unwrap();
        if refused {
            let profile = synthetic_profile();
            runtime.saved.profiles.active_profile = Some(profile.id.clone());
            runtime.saved.profiles.profiles = vec![profile];
            runtime.catalog.entries = vec![synthetic_entry()];
            runtime.provider = ProviderKind::VPortal;
            let origin = json!({"provider":"m3u","profile":"Home","subprofile":null,"sourceAlias":"Local"});
            runtime.saved.settings.insert("nativeKiosk".into(), json!({"version":1,"strict":false,"origin":origin,
                "target":{"kind":"item","entry":{"title":"Fixture","origin":origin,"locator":{"kind":"channel","channelId":"fixture-1"}}}}));
            assert!(crate::app::ott_runtime::input_policy::navigation(runtime).is_ok());
            assert!(crate::app::ott_runtime::input_policy::replace_source(runtime).is_err());
        } else {
            plx_plex::plex::reset_servers_for_test();
        }
        let saved = serde_json::to_value(&runtime.saved).unwrap();
        let catalog = runtime.catalog.clone();
        let generation = runtime.generation;
        let selected = runtime.selected.clone();
        let playback = plx_media::route::playback_identity(&app.player.session);
        assert_eq!(reconcile_classic_navigation(&mut app), !refused);
        settle_pages(&mut app);
        assert!(app.route() == if refused { AppArg::Library } else { AppArg::Ott });
        let runtime = app.ott.as_ref().unwrap();
        assert_eq!(runtime.page, if refused { "channels" } else { "providers" });
        assert!(!runtime.status.is_empty());
        assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), saved);
        assert_eq!(runtime.catalog, catalog);
        assert_eq!(runtime.generation, generation);
        assert_eq!(runtime.selected, selected);
        assert!(runtime.catalog_next.is_none() && runtime.play_target.is_none() && !runtime.saving);
        assert_eq!(plx_media::route::playback_identity(&app.player.session), playback);
    }
}

#[test]
fn saved_startup_displays_normal_kiosk_source_without_replacement_authority() {
    let _serial = plx_base::testlock::serial();
    let _registry = Registry::new();
    let mut app = ready();
    mount(&mut app, AppArg::Home);
    let runtime = app.ott.as_mut().unwrap();
    let profile = synthetic_profile();
    runtime.saved.profiles.active_profile = Some(profile.id.clone());
    runtime.saved.profiles.profiles = vec![profile];
    runtime.catalog.entries = vec![synthetic_entry()];
    runtime.provider = ProviderKind::VPortal;
    let origin = json!({"provider":"m3u","profile":"Home","subprofile":null,"sourceAlias":"Local"});
    runtime.saved.settings.insert("nativeKiosk".into(), json!({"version":1,"strict":false,"origin":origin,
        "target":{"kind":"item","entry":{"title":"Fixture","origin":origin,"locator":{"kind":"channel","channelId":"fixture-1"}}}}));
    assert!(crate::app::ott_runtime::input_policy::navigation(runtime).is_ok());
    assert!(crate::app::ott_runtime::input_policy::replace_source(runtime).is_err());
    let saved = serde_json::to_value(&runtime.saved).unwrap();
    let catalog = runtime.catalog.clone();
    let generation = runtime.generation;
    let selected = runtime.selected.clone();
    let playback = plx_media::route::playback_identity(&app.player.session);
    assert!(reconcile_classic_navigation(&mut app));
    settle_pages(&mut app);
    assert!(app.route() == AppArg::Ott);
    assert_eq!(app.pages.nav.tabs.stack.depth(), 1);
    let runtime = app.ott.as_ref().unwrap();
    assert_eq!(runtime.effective_provider(), ProviderKind::VPortal);
    assert!(classic_source::profile(runtime).is_none());
    assert!(crate::app::ott_runtime::input_policy::replace_source(runtime).is_err());
    assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), saved);
    assert_eq!(runtime.catalog, catalog);
    assert_eq!(runtime.generation, generation);
    assert_eq!(runtime.selected, selected);
    assert!(runtime.catalog_next.is_none() && runtime.play_target.is_none() && !runtime.saving);
    assert_eq!(plx_media::route::playback_identity(&app.player.session), playback);
}

#[test]
fn classic_source_entry_uses_catalogue_page_and_preserves_category() {
    let _serial = plx_base::testlock::serial();
    let _registry = Registry::new();
    for (page, favorites, expected) in [
        ("settings", false, "channels"),
        ("providers", true, "favorites"),
        ("categories", false, "categories"),
    ] {
        let mut app = ready();
        mount(&mut app, AppArg::Home);
        let runtime = app.ott.as_mut().unwrap();
        runtime.page = page;
        runtime.saved.category = "Kept category".into();
        runtime.saved.favorites_selected = favorites;
        let saved = serde_json::to_value(&runtime.saved).unwrap();
        let selected = runtime.selected.clone();
        assert!(reconcile_classic_navigation(&mut app));
        settle_pages(&mut app);
        assert!(app.route() == AppArg::Ott);
        let runtime = app.ott.as_ref().unwrap();
        assert_eq!(runtime.page, expected);
        assert_eq!(runtime.view_selection(), ("Kept category", favorites));
        assert_eq!(runtime.selected, selected);
        assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), saved);
        assert!(runtime.catalog_next.is_some() && runtime.play_target.is_none() && !runtime.saving);
    }
}

fn modern(app: &mut App) {
    app.ott.as_mut().unwrap().saved.settings.insert("interfaceMode".into(), json!(1));
    app.bridge.apply_ui_mode(UiMode::Modern);
}

#[test]
fn modern_mode_restores_the_exact_retained_native_entry_and_focus() {
    let _serial = plx_base::testlock::serial();
    let _registry = Registry::new();
    let mut app = ready();
    // Restore an established selection, not a Loading page whose later source
    // result may introduce its first focus target during the round trip.
    let sid = plx_plex::plex::current_server();
    app.bridge.seed_registered_browse_for_test([sid, sid]);
    app.bridge.seed_hubs_for_directory_test(sid, 1, plx_data::pms::HubState::Ready);
    mount(&mut app, AppArg::Home);
    let origin = app.pages.top_page();
    let focus = app.pages.focus();
    assert_eq!(focus.as_ref().map(|key| key.entry),
        Some(app.pages.nav.top_page().unwrap().id),
        "seeded native Home must own a focus before Classic projection");
    let saved = serde_json::to_value(&app.ott.as_ref().unwrap().saved).unwrap();
    assert!(reconcile_classic_navigation(&mut app));
    settle_pages(&mut app);
    assert!(app.route() == AppArg::Ott);
    assert!(!reconcile_classic_navigation(&mut app));
    modern(&mut app);
    assert!(reconcile_classic_navigation(&mut app));
    assert!(!reconcile_classic_navigation(&mut app), "one pending Pop");
    settle_pages(&mut app);
    assert!(app.route() == AppArg::Home);
    assert_eq!(app.pages.top_page(), origin);
    assert_eq!(app.pages.focus(), focus);
    assert_eq!(app.pages.nav.tabs.stack.depth(), 1);
    let runtime = app.ott.as_ref().unwrap();
    let mut expected = saved;
    expected["settings"]["interfaceMode"] = json!(1);
    assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), expected);
    assert!(runtime.play_target.is_none());
}

#[test]
fn projected_providers_back_uses_root_exit_without_popping_native_origin() {
    let _serial = plx_base::testlock::serial();
    let _registry = Registry::new();
    let mut app = ready();
    mount(&mut app, AppArg::Home);
    assert!(reconcile_classic_navigation(&mut app));
    settle_pages(&mut app);
    assert!(!reconcile_classic_navigation(&mut app));
    let owner = app.pages.top_page();
    let depth = app.pages.nav.tabs.stack.depth();
    let mut runtime = app.ott.take().unwrap();
    runtime.page = "providers";
    runtime.reply = owner;
    let epoch = runtime.playback_epoch;
    handle_action(&mut runtime, &mut app, OttAction::Back).unwrap();
    handle_action(&mut runtime, &mut app, OttAction::ExitAnswer { token: 1, accept: true }).unwrap();
    assert_eq!(runtime.playback_epoch, epoch);
    app.ott = Some(runtime);
    settle_pages(&mut app);
    assert_eq!(app.pages.top_page(), owner);
    assert_eq!(app.pages.nav.tabs.stack.depth(), depth);
    assert!(app.bridge.take_reqs().iter().any(|request|
        matches!(request, plx_screens::registry::LoopReq::BackAtRoot)));
}

#[test]
fn modern_restoration_never_pops_foreign_session_surface_or_unready_state() {
    let _serial = plx_base::testlock::serial();
    let _registry = Registry::new();
    for gate in ["foreign", "session", "surface", "public-state"] {
        let mut app = ready();
        mount(&mut app, AppArg::Home);
        assert!(reconcile_classic_navigation(&mut app));
        settle_pages(&mut app);
        assert!(!reconcile_classic_navigation(&mut app));
        match gate {
            "foreign" => mount(&mut app, AppArg::Ott),
            "session" => {
                let mut init = app.bridge.snapshot_session_init();
                init.epoch += 1;
                app.bridge = bridge::Bridge::for_session_test(init);
            }
            "surface" => { bridge::open_settings(&mut app.pages); settle_pages(&mut app); }
            "public-state" => app.ott.as_mut().unwrap().public_preferences = Default::default(),
            _ => unreachable!(),
        }
        modern(&mut app);
        let owner = app.pages.top_page();
        let depth = app.pages.nav.tabs.stack.depth();
        assert!(!reconcile_classic_navigation(&mut app), "{gate}");
        assert_eq!(app.pages.top_page(), owner, "{gate}");
        assert_eq!(app.pages.nav.tabs.stack.depth(), depth, "{gate}");
        assert!(!app.pages.has_pending_navigation(), "{gate}");
    }
}

#[test]
fn classic_provider_pin_denial_never_projects_or_substitutes_a_source() {
    let _serial = plx_base::testlock::serial();
    let _registry = Registry::new();
    for missing_client in [false, true] {
        let mut app = ready();
        mount(&mut app, AppArg::Home);
        let runtime = app.ott.as_mut().unwrap();
        runtime.saved.settings.insert("parentPin".into(), json!("pbkdf2-sha256$600000$000102030405060708090a0b0c0d0e0f$ac310cc8527c6c3a4e20d2a2246ebd77d59766dae1c5fe843d65ecf6a7e20c65"));
        runtime.saved.settings.insert("requirePinForProviderSelection".into(), json!(1));
        if missing_client { plx_plex::plex::reset_servers_for_test(); }
        assert!(runtime.require_parental(parental::Target::Providers).is_err());
        let saved = serde_json::to_value(&runtime.saved).unwrap();
        let catalog = runtime.catalog.clone();
        let generation = runtime.generation;
        let owner = app.pages.top_page();
        assert!(!reconcile_classic_navigation(&mut app));
        assert!(!app.pages.has_pending_navigation());
        assert_eq!(app.pages.top_page(), owner);
        assert!(app.route() == AppArg::Home);
        let runtime = app.ott.as_ref().unwrap();
        assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), saved);
        assert_eq!(runtime.catalog, catalog);
        assert_eq!(runtime.generation, generation);
        assert!(runtime.play_target.is_none() && runtime.edit_session.is_none());
    }
}

#[test]
fn queued_surface_work_prevents_classic_projection_and_modern_restoration() {
    let _serial = plx_base::testlock::serial();
    let _registry = Registry::new();
    for restore in [false, true] {
        for queued in [false, true] {
            for consent in [false, true] {
                let mut app = ready();
                mount(&mut app, AppArg::Home);
                if restore {
                    assert!(reconcile_classic_navigation(&mut app));
                    settle_pages(&mut app);
                    assert!(!reconcile_classic_navigation(&mut app));
                    modern(&mut app);
                }
                let owner = app.pages.top_page();
                if queued {
                    let arg = if consent { AppArg::FirstRunConsent(0) }
                        else { AppArg::Settings(plx_screens::family::SettingsPage::Root) };
                    app.pages.emit(MachineId::Nav, Fx::Nav(plx_machine::machine::NavOp::Present(arg)));
                } else if consent { bridge::open_first_run_consent(&mut app.pages); }
                else { bridge::open_settings(&mut app.pages); }
                assert!(!app.pages.surface_up(), "the surface has not committed");
                assert!(!app.pages.has_pending_navigation(), "this is not a page move");
                assert!(app.pages.has_pending_surface_navigation());
                assert!(!reconcile_classic_navigation(&mut app));
                assert_eq!(app.pages.top_page(), owner);
                assert!(!app.pages.has_pending_navigation(), "no Push or Pop was added");
                settle_pages(&mut app);
                assert!(app.pages.surface_up());
                assert_eq!(app.pages.top_page(), owner);
            }
        }
    }
}


#[test]
fn projected_providers_back_defers_to_queued_settings_or_consent() {
    let _serial = plx_base::testlock::serial();
    let _registry = Registry::new();
    for queued in [false, true] {
        for consent in [false, true] {
            let mut app = ready();
            mount(&mut app, AppArg::Home);
            assert!(reconcile_classic_navigation(&mut app));
            settle_pages(&mut app);
            assert!(!reconcile_classic_navigation(&mut app));
            let owner = app.pages.top_page();
            let depth = app.pages.nav.tabs.stack.depth();
            let playback = plx_media::route::playback_identity(&app.player.session);
            let mut runtime = app.ott.take().unwrap();
            runtime.page = "providers";
            runtime.reply = owner;
            let mut row = synthetic_entry();
            row.media = MediaLocator::Plex { server_id: "classic-fixture-machine".into(),
                rating_key: "42".into() };
            runtime.catalog.entries.push(row);
            let catalog = runtime.catalog.clone();
            let next = runtime.catalog_next.clone();
            let selected = runtime.selected.clone();
            let generation = runtime.generation;
            let playback_epoch = runtime.playback_epoch;
            let saved = serde_json::to_value(&runtime.saved).unwrap();
            let arg = if consent { AppArg::FirstRunConsent(0) }
                else { AppArg::Settings(plx_screens::family::SettingsPage::Root) };
            if queued {
                app.pages.emit(MachineId::Nav,
                    Fx::Nav(plx_machine::machine::NavOp::Present(arg.clone())));
            } else if consent { bridge::open_first_run_consent(&mut app.pages); }
            else { bridge::open_settings(&mut app.pages); }
            assert!(!app.pages.surface_up());
            assert!(!app.pages.has_pending_navigation());
            assert!(app.pages.has_pending_surface_navigation());

            handle_action(&mut runtime, &mut app, OttAction::Back).unwrap();

            assert_eq!(runtime.generation, generation, "pending Present must not reach generic Back");
            assert_eq!(runtime.playback_epoch, playback_epoch);
            assert_eq!(runtime.catalog, catalog);
            assert_eq!(runtime.catalog_next, next);
            assert_eq!(runtime.selected, selected);
            assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), saved);
            assert!(app.pages.has_pending_surface_navigation());
            assert!(!app.pages.has_pending_navigation(), "Back must not enqueue a Pop");
            app.ott = Some(runtime);
            settle_pages(&mut app);
            assert!(app.pages.surface_up());
            assert_eq!(app.pages.top_page(), owner);
            assert_eq!(app.pages.nav.tabs.stack.depth(), depth);
            assert_eq!(plx_media::route::playback_identity(&app.player.session), playback);
            assert!(app.pages.nav.modals.surfaces.iter().any(|surface| surface.entry.arg == arg));
            assert!(!app.bridge.take_reqs().iter().any(|request|
                matches!(request, plx_screens::registry::LoopReq::BackAtRoot)));
        }
    }
}


// A cold native Home is a startup route, not a new user choice of Plex.
// Keep the same admitted session/held worker slots as the surrounding fixtures.
fn cold_home_with_provider_choice(choice: Option<&str>) -> App {
    let mut app = ready();
    let runtime = app.ott.as_mut().unwrap();
    runtime.saved.profiles.active_profile = None;
    runtime.saved.profiles.profiles.clear();
    runtime.saved.settings.remove("selectedProvider");
    if let Some(choice) = choice {
        runtime.saved.settings.insert("selectedProvider".into(), json!(choice));
    }
    runtime.catalog = Catalog::default();
    runtime.selected = None;
    runtime.page = "profiles";
    runtime.sync_saved_context();
    assert_eq!(runtime.navigation_epoch, 0);
    assert_eq!(runtime.playback_epoch, 0);
    assert_eq!(runtime.generation, 0);
    assert!(runtime.playing.is_none() && runtime.transient.accepted.is_none());
    assert!(classic_source::profile(runtime).is_none() && managed::profile(runtime).is_none());
    assert!(runtime.catalog_next.is_none() && runtime.play_target.is_none());
    mount(&mut app, AppArg::Home);
    app
}

#[test]
fn classic_cold_home_preserves_explicit_m3u_without_a_profile() {
    let _serial = plx_base::testlock::serial();
    let _registry = Registry::new();
    let mut app = cold_home_with_provider_choice(Some("m3u"));
    let before = serde_json::to_value(&app.ott.as_ref().unwrap().saved).unwrap();
    let last_input = app.last_input;
    let playback = plx_media::route::playback_identity(&app.player.session);

    // The real per-frame coordinator must not turn an incidental native Home
    // into a temporary Plex source that hides the explicit persisted provider.
    let projected = reconcile_classic_navigation(&mut app);
    let runtime = app.ott.as_ref().unwrap();
    assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), before);
    assert_eq!(runtime.effective_provider(), ProviderKind::M3u,
        "cold Classic Home must respect the explicit saved provider");
    assert!(classic_source::profile(runtime).is_none());
    assert!(runtime.catalog_next.is_none() && runtime.play_target.is_none() && !runtime.saving);
    assert_eq!(app.last_input, last_input, "no input was supplied");
    assert_eq!(plx_media::route::playback_identity(&app.player.session), playback);
    assert!(projected, "the saved provider still needs its Classic UI");
    settle_pages(&mut app);
    assert!(app.route() == AppArg::Ott);
    assert_eq!(app.pages.nav.tabs.stack.depth(), 1);
    assert_eq!(app.ott.as_ref().unwrap().page, "profiles");
}

#[test]
fn classic_cold_home_retains_plex_bootstrap_without_a_non_plex_choice() {
    let _serial = plx_base::testlock::serial();
    let _registry = Registry::new();
    for choice in [None, Some("plex")] {
        let mut app = cold_home_with_provider_choice(choice);
        let before = serde_json::to_value(&app.ott.as_ref().unwrap().saved).unwrap();
        assert!(reconcile_classic_navigation(&mut app));
        let runtime = app.ott.as_ref().unwrap();
        assert_eq!(runtime.effective_provider(), ProviderKind::Plex);
        assert!(classic_source::profile(runtime).is_some());
        assert!(runtime.catalog_next.is_some() && runtime.play_target.is_none() && !runtime.saving);
        assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), before);
    }
}

#[test]
fn classic_explicit_native_library_remains_available_after_saved_m3u_choice() {
    let _serial = plx_base::testlock::serial();
    let _registry = Registry::new();
    let mut app = cold_home_with_provider_choice(Some("m3u"));
    bridge::nav_push(&mut app.pages, AppArg::Library);
    settle_pages(&mut app);
    let before = serde_json::to_value(&app.ott.as_ref().unwrap().saved).unwrap();
    assert!(reconcile_classic_navigation(&mut app));
    let runtime = app.ott.as_ref().unwrap();
    assert_eq!(runtime.effective_provider(), ProviderKind::Plex);
    assert!(classic_source::profile(runtime).is_some());
    assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), before);
    assert!(runtime.play_target.is_none() && !runtime.saving);
}


#[test]
fn modern_cold_home_opens_saved_m3u_and_does_not_pop_it_on_the_next_frame() {
    let _serial = plx_base::testlock::serial();
    let _registry = Registry::new();
    let mut app = cold_home_with_provider_choice(Some("m3u"));
    modern(&mut app);
    let before = serde_json::to_value(&app.ott.as_ref().unwrap().saved).unwrap();
    assert!(reconcile_classic_navigation(&mut app));
    settle_pages(&mut app);
    let owner = app.pages.top_page();
    assert!(app.route() == AppArg::Ott);
    assert!(!reconcile_classic_navigation(&mut app));
    assert_eq!(app.pages.top_page(), owner);
    let runtime = app.ott.as_ref().unwrap();
    assert_eq!(runtime.effective_provider(), ProviderKind::M3u);
    assert_eq!(runtime.page, "profiles");
    assert!(runtime.catalog_next.is_none() && !runtime.saving);
    assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), before);
    for (setting, mode) in [(0, UiMode::Classic), (1, UiMode::Modern)] {
        app.ott.as_mut().unwrap().saved.settings.insert("interfaceMode".into(), json!(setting));
        app.bridge.apply_ui_mode(mode);
        assert!(!reconcile_classic_navigation(&mut app));
        assert_eq!(app.pages.top_page(), owner, "saved-provider startup survives mode toggles");
        assert!(app.route() == AppArg::Ott);
    }
}

#[test]
fn startup_projection_preserves_configured_m3u_catalogue_with_nonzero_generation() {
    let _serial = plx_base::testlock::serial();
    let _registry = Registry::new();
    for (use_modern, legacy) in [(false, false), (true, false), (false, true), (true, true)] {
        let mut app = cold_home_with_provider_choice(if legacy { None } else { Some("m3u") });
        if use_modern { modern(&mut app); }
        let runtime = app.ott.as_mut().unwrap();
        let profile = ProviderProfile { id: "m3u-kept".into(), name: "Kept".into(),
            kind: ProviderKind::M3u, source: "https://catalogue.invalid/live.m3u".into(),
            source_alias: String::new(), enabled: true };
        runtime.saved.profiles.active_profile = Some(profile.id.clone());
        runtime.saved.profiles.profiles.push(profile);
        runtime.generation = 17;
        let mut row = synthetic_entry();
        row.id = "kept-selection".into();
        row.media = MediaLocator::Http { url: "https://media.invalid/kept.m3u8".into() };
        runtime.catalog.entries.push(row);
        runtime.selected = Some("kept-selection".into());
        runtime.page = "channels";
        let saved = serde_json::to_value(&runtime.saved).unwrap();
        let catalog = runtime.catalog.clone();
        let playback = plx_media::route::playback_identity(&app.player.session);
        assert!(reconcile_classic_navigation(&mut app));
        let runtime = app.ott.as_ref().unwrap();
        assert_eq!(runtime.generation, 17);
        assert_eq!(runtime.catalog, catalog);
        assert_eq!(runtime.selected.as_deref(), Some("kept-selection"));
        assert_eq!(runtime.page, "channels");
        assert_eq!(runtime.effective_provider(), ProviderKind::M3u);
        assert!(classic_source::profile(runtime).is_none());
        assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), saved);
        assert_eq!(plx_media::route::playback_identity(&app.player.session), playback);
    }
}

#[test]
fn later_provider_choice_does_not_reclaim_home_after_an_owned_modern_return() {
    let _serial = plx_base::testlock::serial();
    let _registry = Registry::new();
    let mut app = cold_home_with_provider_choice(None);
    let home = app.pages.top_page();
    assert!(reconcile_classic_navigation(&mut app));
    settle_pages(&mut app);
    assert!(!reconcile_classic_navigation(&mut app));
    app.ott.as_mut().unwrap().saved.settings.insert("selectedProvider".into(), json!("m3u"));
    modern(&mut app);
    assert!(reconcile_classic_navigation(&mut app));
    settle_pages(&mut app);
    assert_eq!(app.pages.top_page(), home);
    assert!(app.route() == AppArg::Home);
    assert!(!reconcile_classic_navigation(&mut app));
    assert!(!app.pages.has_pending_navigation());
}

#[test]
fn input_before_private_load_settles_retires_saved_startup_projection() {
    let _serial = plx_base::testlock::serial();
    let _registry = Registry::new();
    let mut app = cold_home_with_provider_choice(Some("m3u"));
    modern(&mut app);
    app.ott.as_mut().unwrap().loaded = false;
    app.last_input = app.t0.wrapping_add(1);
    assert!(!reconcile_classic_navigation(&mut app));
    app.ott.as_mut().unwrap().loaded = true;
    assert!(!reconcile_classic_navigation(&mut app));
    assert!(app.route() == AppArg::Home);
    assert!(!app.pages.has_pending_navigation());
}


#[test]
fn saved_startup_waits_for_private_public_and_session_readiness() {
    let _serial = plx_base::testlock::serial();
    let _registry = Registry::new();
    for gate in ["private", "public", "session"] {
        let mut app = cold_home_with_provider_choice(Some("m3u"));
        let ready_session = app.bridge.snapshot_session_init();
        match gate {
            "private" => app.ott.as_mut().unwrap().loaded = false,
            "public" => app.ott.as_mut().unwrap().public_preferences = Default::default(),
            "session" => {
                let mut init = ready_session.clone();
                init.phase = plx_session::auth::Phase::Profiles;
                app.bridge = bridge::Bridge::for_session_test(init);
                app.bridge.apply_ui_mode(UiMode::Classic);
            },
            _ => unreachable!(),
        }
        assert!(!reconcile_classic_navigation(&mut app), "{gate}");
        assert!(!app.pages.has_pending_navigation());
        match gate {
            "private" => app.ott.as_mut().unwrap().loaded = true,
            "public" => app.ott.as_mut().unwrap().public_preferences.accept(None),
            "session" => {
                app.bridge = bridge::Bridge::for_session_test(ready_session);
                app.bridge.apply_ui_mode(UiMode::Classic);
            },
            _ => unreachable!(),
        }
        assert!(reconcile_classic_navigation(&mut app), "{gate}");
        let runtime = app.ott.as_ref().unwrap();
        assert_eq!(runtime.effective_provider(), ProviderKind::M3u);
        assert!(classic_source::profile(runtime).is_none());
    }
}


#[test]
fn saved_startup_provider_root_back_never_reveals_native_plex_home() {
    let _serial = plx_base::testlock::serial();
    let _registry = Registry::new();
    for use_modern in [false, true] {
        let mut app = cold_home_with_provider_choice(Some("m3u"));
        if use_modern { modern(&mut app); }
        assert!(reconcile_classic_navigation(&mut app));
        settle_pages(&mut app);
        assert!(app.route() == AppArg::Ott);
        assert_eq!(app.pages.nav.tabs.stack.depth(), 1);
        let owner = app.pages.top_page();
        let mut runtime = app.ott.take().unwrap();
        runtime.page = "providers";
        runtime.reply = owner;
        handle_action(&mut runtime, &mut app, OttAction::Back).unwrap();
        handle_action(&mut runtime, &mut app, OttAction::ExitAnswer { token: 1, accept: true }).unwrap();
        app.ott = Some(runtime);
        settle_pages(&mut app);
        assert_eq!(app.pages.top_page(), owner);
        assert_eq!(app.pages.nav.tabs.stack.depth(), 1);
        assert!(app.bridge.take_reqs().iter().any(|request|
            matches!(request, plx_screens::registry::LoopReq::BackAtRoot)));
    }
}


#[test]
fn projected_root_back_confirmation_open_is_not_accepted_navigation() {
    let _serial = plx_base::testlock::serial();
    let _registry = Registry::new();
    let mut app = ready();
    mount(&mut app, AppArg::Home);
    assert!(reconcile_classic_navigation(&mut app));
    settle_pages(&mut app);
    assert!(!reconcile_classic_navigation(&mut app));
    let owner = app.pages.top_page();
    let depth = app.pages.nav.tabs.stack.depth();
    let mut runtime = app.ott.take().unwrap();
    runtime.page = "providers";
    runtime.reply = owner;
    let before = (runtime.navigation_epoch, runtime.playback_epoch, runtime.generation,
        runtime.catalog.clone(), runtime.catalog_next.clone(), serde_json::to_value(&runtime.saved).unwrap());
    apply_ui_action(&mut runtime, &mut app, OttAction::Back, true, false, false);
    assert_eq!(runtime.navigation_epoch, before.0, "opening a question must not retire managed or USB navigation");
    assert_eq!(runtime.playback_epoch, before.1);
    assert_eq!(runtime.generation, before.2);
    assert_eq!(runtime.catalog, before.3);
    assert_eq!(runtime.catalog_next, before.4);
    assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), before.5);
    app.ott = Some(runtime);
    settle_pages(&mut app);
    assert_eq!(app.pages.top_page(), owner);
    assert_eq!(app.pages.nav.tabs.stack.depth(), depth);
    assert!(!app.bridge.take_reqs().iter().any(|request|
        matches!(request, plx_screens::registry::LoopReq::BackAtRoot)));
}
