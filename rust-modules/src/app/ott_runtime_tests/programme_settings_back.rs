//! Inner settings return through the real Player catalog surface and key dispatcher.
use super::*;
use plx_machine::machine::UiMode;

#[test]
fn player_inner_settings_back_matches_visible_return_and_preserves_decoder() {
    let _serial = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    for mode in [UiMode::Classic, UiMode::Modern] {
        for (setting, page, back_id) in [
            ("menu.items", "menu-items", "menu:back"),
            ("interfaceMode", "interface-mode", "interface:back"),
        ] {
            for physical in [false, true] {
                let mut app = catalog_app();
                let runtime = app.ott.as_mut().unwrap();
                if setting == "interfaceMode" {
                    // The shared fixture revokes writes; opening this separate picker
                    // requires loaded public preferences, even when only testing Back.
                    runtime.write_permit = private::WritePermit::new();
                    runtime.public_preferences.accept(None);
                    runtime.last_checkpoint_ms = runtime.now_ms();
                    assert!(public_preferences::write_refusal(runtime).is_none());
                }
                runtime.saved.settings.insert("interfaceMode".into(), json!(mode.index()));
                app.bridge.apply_ui_mode(mode);
                open_more(&mut app);
                choose_catalog(&mut app);
                surface_action(&mut app, OttAction::ShowSettings);
                surface_action(&mut app, OttAction::EditSetting(setting.into()));
                let runtime = app.ott.as_ref().unwrap();
                assert_eq!(runtime.page, page);
                let back = runtime.view().rows.into_iter().find(|row| row.id == back_id).unwrap().action;
                assert_eq!(back, OttAction::ShowSettings);
                let epochs = (runtime.generation, runtime.play_generation, runtime.playback_epoch);
                let saved = serde_json::to_value(&runtime.saved).unwrap();
                let surface = ott_surface(&app).expect("settings retain the existing OTT surface");
                let owner = app.pages.top_page();
                let origin = bridge::player(&app.pages).unwrap().origin;
                let identity = route::playback_identity(&app.player.session);
                let source = route::external_source(&app.player.session);
                let position = player::SHARED.playpos_ns.load(Ordering::Acquire);
                if physical { key(&mut app, Key::Back); } else { surface_action(&mut app, back); }
                let runtime = app.ott.as_ref().unwrap();
                assert_eq!(runtime.page, "settings", "{mode:?} {page} physical={physical}");
                assert_eq!((runtime.generation, runtime.play_generation, runtime.playback_epoch), epochs);
                assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), saved);
                assert!(!runtime.cancel.load(Ordering::Acquire));
                assert_eq!(ott_surface(&app), Some(surface));
                assert_eq!(app.pages.top_page(), owner);
                assert_eq!(bridge::player(&app.pages).unwrap().origin, origin);
                assert_eq!(route::playback_identity(&app.player.session), identity);
                assert_eq!(route::external_source(&app.player.session), source);
                assert_eq!(player::SHARED.playpos_ns.load(Ordering::Acquire), position);
                assert_eq!(player::state(&app.player.session), player::PlaybackState::Paused);
                assert!(app.adapters.player.is_live());
                assert!(app.player.pending_exit.is_none());
                assert!(!app.refresh_hubs);
                assert!(!runtime.saving);
                assert_eq!(app.bridge.ui_mode(), mode);
            }
        }
    }
}

fn browse_request(app: &mut App) -> OttOut {
    let reply = super::super::super::current_ui_owner(app).expect("actual OTT input owner");
    let plx_screens::registry::PageMemory::Ott(memory) = app.pages.return_state().memory else {
        panic!("actual mounted OTT caller memory")
    };
    OttOut::BrowseSettings { reply, revision: app.ott.as_ref().unwrap().revision, memory }
}

fn deliver_browse_request(app: &mut App, request: OttOut) {
    let owner = super::super::super::current_ui_owner(app).expect("OTT owner");
    app.pages.emit(MachineId::Instance(owner), Fx::App(AppFx::Ott(request)));
    frame(app, Vec::new());
    idle(app);
}

#[test]
fn browse_settings_return_preserves_player_and_has_a_finite_back_chain() {
    let _serial = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    for mode in [UiMode::Classic, UiMode::Modern] {
        let mut app = catalog_app();
        app.ott.as_mut().unwrap().saved.settings.insert("interfaceMode".into(), json!(mode.index()));
        app.bridge.apply_ui_mode(mode);
        open_more(&mut app);
        choose_catalog(&mut app);
        let surface = ott_surface(&app).unwrap();
        let source = route::external_source(&app.player.session);
        let identity = route::playback_identity(&app.player.session);
        let epochs = { let r = app.ott.as_ref().unwrap(); (r.generation, r.play_generation, r.playback_epoch) };
        let saved = serde_json::to_value(&app.ott.as_ref().unwrap().saved).unwrap();
        let request = browse_request(&mut app);
        deliver_browse_request(&mut app, request);
        assert_eq!(app.ott.as_ref().unwrap().page, "settings");
        assert!(app.ott.as_ref().unwrap().browse_settings.is_some());
        // Legitimate newer display publications must not invalidate the caller.
        app.ott.as_mut().unwrap().status = "Updated display status".into();
        app.ott.as_mut().unwrap().changed();
        idle(&mut app);
        key(&mut app, Key::Back);
        assert_eq!(app.ott.as_ref().unwrap().page, "channels");
        assert!(app.ott.as_ref().unwrap().browse_settings.is_none());
        assert_eq!(ott_surface(&app), Some(surface));
        key(&mut app, Key::Ok);
        assert_eq!(app.ott.as_ref().unwrap().page, "settings", "Menu focus is Settings");
        key(&mut app, Key::Back);
        key(&mut app, Key::Back);
        assert_eq!(app.ott.as_ref().unwrap().page, "channels", "Menu Back restores its list");
        assert_eq!(ott_surface(&app), Some(surface));
        key(&mut app, Key::Back);
        assert!(ott_surface(&app).is_none(), "list Back returns to Player, never Settings");
        let runtime = app.ott.as_ref().unwrap();
        assert_eq!((runtime.generation, runtime.play_generation, runtime.playback_epoch), epochs);
        assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), saved);
        assert_eq!(route::playback_identity(&app.player.session), identity);
        assert_eq!(route::external_source(&app.player.session), source);
        assert!(app.adapters.player.is_live());
        assert!(app.player.pending_exit.is_none());
    }
}

#[test]
fn root_catalog_browse_settings_returns_without_a_player() {
    let _serial = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    for page in ["channels", "categories"] {
        let mut app = ready_app_fixture();
        app.running = true;
        let runtime = app.ott.as_mut().unwrap();
        runtime.catalog_done.store(false, Ordering::Release);
        runtime.metadata_done.store(false, Ordering::Release);
        runtime.resolver_busy = true;
        runtime.saved.settings.insert("interfaceMode".into(), json!(UiMode::Classic.index()));
        runtime.page = page;
        runtime.changed();
        app.bridge.apply_ui_mode(UiMode::Classic);
        bridge::show_page(&mut app.pages, AppArg::Ott);
        idle(&mut app);
        let owner = super::super::super::current_ui_owner(&app).unwrap();
        assert!(app.ott.as_ref().unwrap().playing.is_none());
        let request = browse_request(&mut app);
        deliver_browse_request(&mut app, request);
        assert_eq!(app.ott.as_ref().unwrap().page, "settings");
        key(&mut app, Key::Back);
        assert_eq!(app.ott.as_ref().unwrap().page, page);
        assert_eq!(super::super::super::current_ui_owner(&app), Some(owner));
        key(&mut app, Key::Ok);
        assert_eq!(app.ott.as_ref().unwrap().page, "settings", "actual Menu Settings focus");
        key(&mut app, Key::Back);
        key(&mut app, Key::Back);
        assert_eq!(app.ott.as_ref().unwrap().page, page);
        assert!(app.ott.as_ref().unwrap().browse_settings.is_none());
        assert!(app.ott.as_ref().unwrap().playing.is_none());
    }
}

#[test]
fn stale_browse_settings_request_does_not_navigate_or_arm_an_origin() {
    let _serial = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    let mut app = catalog_app();
    open_more(&mut app);
    choose_catalog(&mut app);
    let request = browse_request(&mut app);
    app.ott.as_mut().unwrap().changed();
    let saved = serde_json::to_value(&app.ott.as_ref().unwrap().saved).unwrap();
    deliver_browse_request(&mut app, request);
    assert_eq!(app.ott.as_ref().unwrap().page, "channels");
    assert!(app.ott.as_ref().unwrap().browse_settings.is_none());
    assert_eq!(serde_json::to_value(&app.ott.as_ref().unwrap().saved).unwrap(), saved);
}

#[test]
fn browse_settings_origin_retires_on_foreign_owner_or_source_changes() {
    let _serial = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    for changed in ["source", "mode", "cover"] {
        let mut app = catalog_app();
        app.ott.as_mut().unwrap().saved.settings.insert("interfaceMode".into(), json!(UiMode::Classic.index()));
        app.bridge.apply_ui_mode(UiMode::Classic);
        open_more(&mut app);
        choose_catalog(&mut app);
        let request = browse_request(&mut app);
        deliver_browse_request(&mut app, request);
        assert!(app.ott.as_ref().unwrap().browse_settings.is_some());
        match changed {
            "source" => app.ott.as_mut().unwrap().generation += 1,
            "mode" => app.bridge.apply_ui_mode(UiMode::Modern),
            _ => bridge::open_settings(&mut app.pages),
        }
        idle(&mut app);
        assert!(app.ott.as_ref().unwrap().browse_settings.is_none(), "{changed}");
    }
}

#[test]
fn ordinary_root_settings_without_browse_origin_keeps_provider_return() {
    let _serial = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    let mut app = ready_app_fixture();
    app.running = true;
    bridge::show_page(&mut app.pages, AppArg::Ott);
    idle(&mut app);
    let reply = super::super::super::current_ui_owner(&app).unwrap();
    app.pages.emit(MachineId::Instance(reply), Fx::App(AppFx::Ott(OttOut::Action {
        reply, action: OttAction::ShowSettings,
    })));
    frame(&mut app, Vec::new());
    idle(&mut app);
    assert_eq!(app.ott.as_ref().unwrap().page, "settings");
    assert!(app.ott.as_ref().unwrap().browse_settings.is_none());
    key(&mut app, Key::Back);
    assert_eq!(app.ott.as_ref().unwrap().page, "providers");
    assert!(matches!(app.route(), AppArg::Ott));
}

#[test]
fn queued_foreign_surface_cannot_overtake_browse_settings_back_or_publication() {
    let _serial = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    for stage in ["back", "publish", "observe"] {
        for dismiss in [false, true] {
            let mut app = catalog_app();
            app.ott.as_mut().unwrap().saved.settings.insert("interfaceMode".into(), json!(UiMode::Classic.index()));
            app.bridge.apply_ui_mode(UiMode::Classic);
            open_more(&mut app);
            choose_catalog(&mut app);
            let request = browse_request(&mut app);
            deliver_browse_request(&mut app, request);
            assert_eq!(app.ott.as_ref().unwrap().interface_mode(), app.bridge.ui_mode());
            assert_eq!(app.ott.as_ref().unwrap().page, "settings");
            assert!(app.ott.as_ref().unwrap().browse_settings.is_some(), "queue guard requires an admitted caller");
            let owner = super::super::super::current_ui_owner(&app).unwrap();
            let saved = serde_json::to_value(&app.ott.as_ref().unwrap().saved).unwrap();
            let mut runtime = app.ott.take().unwrap();
            if stage == "publish" {
                assert!(player_browse::back(&mut runtime, &mut app, &OttAction::Back).unwrap().is_ok());
                runtime.changed();
            }
            if dismiss {
                let entry = app.pages.nav.entry_of_instance(owner).unwrap();
                app.pages.request(MachineId::Nav, plx_machine::machine::NavOp::Dismiss(entry));
            } else { bridge::open_settings(&mut app.pages); }
            assert!(!app.pages.has_pending_navigation());
            assert!(app.pages.has_pending_surface_navigation());
            assert_eq!(super::super::super::current_ui_owner(&app), Some(owner),
                "queued surface navigation has not changed the current owner yet");
            match stage {
                "back" => {
                    assert!(player_browse::back(&mut runtime, &mut app, &OttAction::Back).unwrap().is_err());
                    assert_eq!(runtime.page, "settings");
                    assert!(runtime.browse_settings.is_none());
                }
                "publish" => {
                    assert!(!player_browse::publish_settings_return(&mut runtime, &mut app));
                    assert!(runtime.browse_settings.is_none(), "undeliverable return is one-shot");
                    assert!(!player_browse::publish_settings_return(&mut runtime, &mut app));
                }
                _ => {
                    player_browse::reconcile_settings(&mut runtime, &app);
                    assert!(runtime.browse_settings.is_some(), "passive retention grants no return");
                }
            }
            assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), saved);
            app.ott = Some(runtime);
            idle(&mut app);
            assert!(app.ott.as_ref().unwrap().browse_settings.is_none(),
                "settled foreign ownership retires even a passively retained origin");
        }
    }
}

#[test]
fn browse_settings_owned_language_pending_mount_cancel_and_back_keep_the_caller() {
    let _serial = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    let mut app = catalog_app();
    let runtime = app.ott.as_mut().unwrap();
    runtime.write_permit = private::WritePermit::new();
    runtime.public_preferences.accept(None);
    runtime.entropy = Some([7; 16]);
    runtime.last_checkpoint_ms = runtime.now_ms();
    runtime.saved.resume.extend(runtime.checkpoints.clone());
    runtime.saved.settings.insert("interfaceMode".into(), json!(UiMode::Classic.index()));
    app.bridge.apply_ui_mode(UiMode::Classic);
    open_more(&mut app);
    choose_catalog(&mut app);
    let caller = ott_surface(&app).unwrap();
    let request = browse_request(&mut app);
    deliver_browse_request(&mut app, request);
    let group = app.ott.as_ref().unwrap().view().rows.into_iter()
        .find(|row| row.id.ends_with("settings.open:interface")).unwrap().action;
    surface_action(&mut app, group);
    let language = app.ott.as_ref().unwrap().view().rows.into_iter()
        .find(|row| row.id.ends_with(":language")).unwrap().action;
    let mut runtime = app.ott.take().unwrap();
    assert!(settings_groups::language_action(&mut runtime, &mut app, &language).unwrap().is_ok());
    assert!(app.pages.has_pending_surface_navigation());
    player_browse::reconcile_settings(&mut runtime, &app);
    assert!(runtime.browse_settings.is_some(), "pending owned Language has no action authority");
    app.ott = Some(runtime);
    idle(&mut app);
    assert!(app.ott.as_ref().unwrap().browse_settings.is_some());
    let language = app.pages.nav.modals.surfaces.iter().find(|surface|
        matches!(surface.entry.arg, AppArg::Settings(plx_screens::family::SettingsPage::Language)))
        .expect("actual owned Language surface").entry.id;
    app.pages.request(MachineId::Nav, plx_machine::machine::NavOp::Dismiss(language));
    idle(&mut app);
    retire_closing_surfaces(&mut app);
    assert_eq!(ott_surface(&app), Some(caller));
    assert!(app.ott.as_ref().unwrap().browse_settings.is_some());
    key(&mut app, Key::Back); // Interface group -> Settings.
    key(&mut app, Key::Back); // Settings -> caller Browse.
    assert_eq!(app.ott.as_ref().unwrap().page, "channels");
    assert!(app.ott.as_ref().unwrap().browse_settings.is_none());
    key(&mut app, Key::Ok);
    assert_eq!(app.ott.as_ref().unwrap().page, "settings", "Menu focus returns to Settings");
}
